//! CPU-only RAVEL-013 Touch peaks and physical ball-contact gating.
//! Neural probabilities propose touches; actual ball motion and reachable player
//! geometry must support them before the proposal becomes a touch event.

use crate::{
    bbox_iou,
    motion::{ball_motion_change, BallObservation, MotionConfig, MotionEvidence},
};
use serde::Serialize;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug)]
pub struct TouchCandidate {
    pub frame: u32,
    pub probability: f64,
    pub bbox: [f64; 4],
}

#[derive(Clone, Copy, Debug)]
pub struct TrackedPlayer {
    pub frame: u32,
    pub track_id: i64,
    pub bbox: [f64; 4],
}

#[derive(Clone, Debug, Serialize)]
pub struct TouchPeak {
    pub frame_index: u32,
    pub action_score: f64,
    pub raw_action_score: f64,
    pub bbox_xyxy: [f64; 4],
    pub track_id: Option<i64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TouchContact {
    pub frame: u32,
    pub ball_x: f64,
    pub ball_y: f64,
    pub contact_type: &'static str,
    pub confidence: f64,
    pub player_track_id: Option<i64>,
    pub motion_frame: i64,
    pub speed_before_px_frame: f64,
    pub speed_after_px_frame: f64,
    pub direction_change_deg: f64,
    pub velocity_residual_px_frame: f64,
    pub side: &'static str,
    pub origin: &'static str,
}

#[derive(Clone, Copy, Debug)]
pub struct PeakConfig {
    pub threshold: f64,
    pub smooth: usize,
    pub min_gap: usize,
}

impl Default for PeakConfig {
    fn default() -> Self {
        Self {
            threshold: 0.5,
            smooth: 5,
            min_gap: 10,
        }
    }
}

impl PeakConfig {
    pub fn validate(self) -> Result<(), &'static str> {
        if !self.threshold.is_finite() || !(0.0..=1.0).contains(&self.threshold) {
            return Err("Touch threshold must be in 0..=1");
        }
        if !(1..=31).contains(&self.smooth) || !(1..=150).contains(&self.min_gap) {
            return Err("Touch smooth/min_gap exceeds supported limits");
        }
        Ok(())
    }
}

fn bbox_ok(b: &[f64; 4]) -> bool {
    b.iter().all(|c| c.is_finite()) && b[2] > b[0] && b[3] > b[1]
}

/// Float32 signal, strongest-first stable NMS, nearest observed actor.
///
/// Do not replace the sparse curve with a dense per-player average: original
/// RAVEL selection uses one strongest candidate on each observed frame.
pub fn select_touch_peaks(
    candidates: &[TouchCandidate],
    config: PeakConfig,
) -> Result<Vec<TouchPeak>, &'static str> {
    config.validate()?;
    let mut best: HashMap<u32, TouchCandidate> = HashMap::new();
    for candidate in candidates {
        if candidate.frame > 2_000_000
            || !candidate.probability.is_finite()
            || candidate.probability <= 0.0
            || candidate.probability > 1.0
            || !bbox_ok(&candidate.bbox)
        {
            continue;
        }
        let replace = best
            .get(&candidate.frame)
            .is_none_or(|existing| candidate.probability > existing.probability);
        if replace {
            best.insert(candidate.frame, *candidate);
        }
    }
    if best.is_empty() {
        return Ok(Vec::new());
    }
    let length = best.keys().max().copied().unwrap_or(0) as usize + 1;
    let mut curve = vec![0_f32; length];
    for (frame, item) in &best {
        curve[*frame as usize] = item.probability as f32;
    }

    // numpy.convolve(..., mode="same") uses a centered, zero-padded window.
    // NumPy 'same' extracts the central portion of full convolution;
    // for even kernel lengths the extra sample lies on the left.
    let left = config.smooth / 2;
    let right = (config.smooth - 1) / 2;
    let scale = 1_f32 / config.smooth as f32;
    let mut smoothed = vec![0_f32; curve.len()];
    for (index, score) in smoothed.iter_mut().enumerate() {
        let begin = index.saturating_sub(left);
        let stop = (index + right + 1).min(curve.len());
        let mut sum = 0_f32;
        for value in &curve[begin..stop] {
            sum += *value * scale;
        }
        *score = sum;
    }
    // argsort(-averaged, kind="stable") with original frame order retained.
    let mut ranked = (0..curve.len()).collect::<Vec<_>>();
    ranked.sort_by(|a, b| smoothed[*b].total_cmp(&smoothed[*a]).then_with(|| a.cmp(b)));
    let mut picked = Vec::<usize>::new();
    let mut peaks = Vec::new();
    for frame in ranked {
        let score = smoothed[frame] as f64;
        if score < config.threshold {
            break;
        }
        if picked
            .iter()
            .any(|previous| frame.abs_diff(*previous) < config.min_gap)
        {
            continue;
        }
        let start = frame.saturating_sub(config.smooth / 2);
        let stop = (frame + config.smooth / 2 + 1).min(curve.len());
        let candidate = (start..stop)
            .filter_map(|f| best.get(&(f as u32)))
            .max_by(|a, b| {
                a.probability.total_cmp(&b.probability).then_with(|| {
                    frame
                        .abs_diff(b.frame as usize)
                        .cmp(&frame.abs_diff(a.frame as usize))
                })
            });
        if let Some(actor) = candidate {
            picked.push(frame);
            peaks.push(TouchPeak {
                frame_index: frame as u32,
                action_score: score,
                raw_action_score: actor.probability,
                bbox_xyxy: actor.bbox,
                track_id: None, // Raw RAVEL ID is not a verified stable Track ID.
            });
        }
    }
    peaks.sort_by_key(|p| p.frame_index);
    Ok(peaks)
}

/// Match a proposal to non-interpolated tracked player boxes within +/-2
/// frames. Reject unconfirmed IoU (<0.3), never attribute it to an arbitrary
/// nearest player. Input track IDs are already stable CPU tracks.
pub fn associate_touch_players(peaks: &mut [TouchPeak], tracked: &[TrackedPlayer]) {
    let mut nearby: HashMap<u32, Vec<TrackedPlayer>> = HashMap::new();
    for row in tracked {
        if bbox_ok(&row.bbox) {
            nearby.entry(row.frame).or_default().push(*row);
        }
    }
    for peak in peaks {
        peak.track_id = None;
        let mut best: Option<(f64, usize, i64)> = None;
        let frame = peak.frame_index;
        for checked in frame.saturating_sub(2)..=frame.saturating_add(2) {
            if let Some(rows) = nearby.get(&checked) {
                for player in rows {
                    let iou = bbox_iou(peak.bbox_xyxy, player.bbox);
                    let distance = frame.abs_diff(checked) as usize;
                    if best.as_ref().is_none_or(|previous| {
                        iou > previous.0 || (iou == previous.0 && distance < previous.1)
                    }) {
                        best = Some((iou, distance, player.track_id));
                    }
                }
            }
        }
        if let Some((iou, _, track_id)) = best {
            if iou >= 0.3 {
                peak.track_id = Some(track_id);
            }
        }
    }
}

fn rounded(value: f64, places: i32) -> f64 {
    let factor = 10_f64.powi(places);
    (value * factor).round() / factor
}

fn default_motion() -> MotionConfig {
    MotionConfig {
        velocity_window: 5,
        min_samples: 2,
        max_gap_frames: 4,
        min_speed: 1.0,
        min_residual: 6.0,
        relative_residual: 0.30,
        gravity: 1.0,
    }
}

fn in_reach(peak: &TouchPeak, ball_x: f64, ball_y: f64, max_reach: f64) -> bool {
    let [left, top, right, bottom] = peak.bbox_xyxy;
    let height = bottom - top;
    if height <= 0.0 {
        return false;
    }
    let outside_x = (left - ball_x).max(ball_x - right).max(0.0);
    let outside_y = (top - ball_y).max(ball_y - bottom).max(0.0);
    outside_x.max(outside_y) <= height * max_reach
}

/// Confirm a RAVEL temporal peak against both reachable ball proximity and a
/// gravity-corrected trajectory discontinuity. RAVEL-only output has unknown
/// technique: no speculative serve/block/attack class.
pub fn confirm_ravel_contacts(
    peaks: &[TouchPeak],
    observations: &[BallObservation],
    start_frame: i64,
    end_frame: i64,
    threshold: f64,
    max_reach: f64,
    min_interval_sec: f64,
) -> Vec<TouchContact> {
    if observations.is_empty()
        || !threshold.is_finite()
        || !max_reach.is_finite()
        || !min_interval_sec.is_finite()
        || !(0.0..=1.0).contains(&threshold)
        || !(0.0..=1.0).contains(&max_reach)
        || !(0.0..=3.0).contains(&min_interval_sec)
    {
        return Vec::new();
    }
    let mut candidates = Vec::new();
    for peak in peaks {
        let frame = peak.frame_index as i64;
        if frame < start_frame || frame > end_frame || peak.action_score < threshold {
            continue;
        }
        let idx = observations.partition_point(|row| row.frame < frame);
        let closest = [idx.checked_sub(1), Some(idx)]
            .into_iter()
            .flatten()
            .filter_map(|index| observations.get(index))
            .filter(|row| row.frame.abs_diff(frame) <= 2)
            .min_by_key(|row| row.frame.abs_diff(frame));
        let Some(ball) = closest else {
            continue;
        };
        if !in_reach(peak, ball.x, ball.y, max_reach) {
            continue;
        }
        let Some(MotionEvidence {
            motion_frame,
            speed_before,
            speed_after,
            angle,
            residual,
        }) = ball_motion_change(observations, frame, 4, default_motion())
        else {
            continue;
        };
        candidates.push(TouchContact {
            frame: peak.frame_index,
            ball_x: ball.x,
            ball_y: ball.y,
            contact_type: "unknown_touch",
            confidence: peak.action_score,
            player_track_id: peak.track_id,
            motion_frame,
            speed_before_px_frame: rounded(speed_before, 3),
            speed_after_px_frame: rounded(speed_after, 3),
            direction_change_deg: rounded(angle, 2),
            velocity_residual_px_frame: rounded(residual, 3),
            side: "unknown",
            origin: "ravel_touch",
        });
    }
    candidates.sort_by_key(|row| row.frame);
    let merge_frames = (30.0 * min_interval_sec).round().max(1.0) as u32;
    let mut result = Vec::<TouchContact>::new();
    let mut last_frame: Option<u32> = None;
    for candidate in candidates {
        if last_frame.is_some_and(|last| candidate.frame.saturating_sub(last) <= merge_frames) {
            let current = result.last_mut().expect("cluster must exist");
            if candidate.confidence > current.confidence {
                *current = candidate.clone();
            }
        } else {
            result.push(candidate.clone());
        }
        last_frame = Some(candidate.frame);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn peaks_keep_strongest_per_frame_and_stable_order() {
        let predictions = vec![
            TouchCandidate {
                frame: 10,
                probability: 0.6,
                bbox: [0., 0., 5., 5.],
            },
            TouchCandidate {
                frame: 10,
                probability: 0.9,
                bbox: [1., 1., 6., 6.],
            },
            TouchCandidate {
                frame: 11,
                probability: 0.8,
                bbox: [1., 1., 6., 6.],
            },
            TouchCandidate {
                frame: 90,
                probability: 0.95,
                bbox: [0., 0., 4., 4.],
            },
        ];
        let peaks = select_touch_peaks(
            &predictions,
            PeakConfig {
                threshold: 0.2,
                smooth: 5,
                min_gap: 10,
            },
        )
        .unwrap();
        assert!(peaks.iter().any(|r| (r.frame_index as i32 - 10).abs() <= 2));
        assert!(peaks.iter().any(|r| (r.frame_index as i32 - 90).abs() <= 2));
        assert!(peaks.iter().all(|r| r.track_id.is_none()));
    }
    #[test]
    fn low_iou_never_creates_an_identity() {
        let mut peaks = vec![TouchPeak {
            frame_index: 50,
            action_score: 0.8,
            raw_action_score: 0.9,
            bbox_xyxy: [0., 0., 1., 1.],
            track_id: None,
        }];
        associate_touch_players(
            &mut peaks,
            &[TrackedPlayer {
                frame: 50,
                track_id: 999,
                bbox: [5., 5., 9., 9.],
            }],
        );
        assert_eq!(peaks[0].track_id, None);
    }
    #[test]
    fn straight_flight_alone_is_not_touch() {
        let points = (0..40)
            .map(|i| BallObservation {
                frame: i,
                x: i as f64 * 5.0,
                y: i as f64 * 2.0,
            })
            .collect::<Vec<_>>();
        let peak = TouchPeak {
            frame_index: 20,
            action_score: 0.8,
            raw_action_score: 0.8,
            bbox_xyxy: [95., 35., 110., 60.],
            track_id: Some(1),
        };
        let contacts = confirm_ravel_contacts(&[peak], &points, 0, 39, 0.5, 0.35, 0.35);
        assert!(contacts.is_empty());
    }
}
