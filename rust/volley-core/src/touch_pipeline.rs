//! Streaming-friendly CPU interpretation of RAVEL Touch proposals against
//! actual, continuous selected-ball motion and provisional RAVEL Track IDs.
//! Detection is intentionally not a verified athlete identity or action label.
use crate::{
    motion::BallObservation,
    touch::{
        associate_touch_players, confirm_ravel_contacts, select_touch_peaks, PeakConfig,
        TouchCandidate, TouchContact, TrackedPlayer,
    },
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TouchPlayer {
    pub track_id: u64,
    pub score: f64,
    pub touch_probability: Option<f64>,
    /// Geometry normalized to video width and height.
    pub bbox_xyxy_norm: [f64; 4],
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TouchBall {
    pub source_model: String,
    pub score: f64,
    pub bbox_xyxy_norm: [f64; 4],
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TouchFrame {
    pub frame_index: u64,
    pub players: Vec<TouchPlayer>,
    pub selected_ball: Option<TouchBall>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TouchSettings {
    pub threshold: f64,
    pub smooth: usize,
    pub min_gap: usize,
    pub max_reach_ratio: f64,
    pub min_interval_sec: f64,
}
impl Default for TouchSettings {
    fn default() -> Self {
        Self {
            threshold: 0.50,
            smooth: 5,
            min_gap: 10,
            max_reach_ratio: 0.35,
            min_interval_sec: 0.35,
        }
    }
}
impl TouchSettings {
    pub fn validate(self) -> Result<Self, String> {
        PeakConfig {
            threshold: self.threshold,
            smooth: self.smooth,
            min_gap: self.min_gap,
        }
        .validate()
        .map_err(str::to_owned)?;
        if !self.max_reach_ratio.is_finite()
            || !(0.05..=1.0).contains(&self.max_reach_ratio)
            || !self.min_interval_sec.is_finite()
            || !(0.10..=3.0).contains(&self.min_interval_sec)
        {
            return Err("Invalid ball reach or contact separation".into());
        }
        Ok(self)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct PhysicalTouchCandidate {
    pub frame_index: u64,
    pub provisional_track_id: Option<u64>,
    pub probability: f64,
    pub ball_x_px: f64,
    pub ball_y_px: f64,
    pub motion_frame_index: u64,
    pub direction_change_deg: f64,
    pub speed_before_px_frame: f64,
    pub speed_after_px_frame: f64,
    pub velocity_residual_px_frame: f64,
    pub technique: &'static str,
    pub physical_motion_supported: bool,
    pub athlete_identity_verified: bool,
}
fn valid_bbox(b: [f64; 4]) -> bool {
    b.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)) && b[2] > b[0] && b[3] > b[1]
}
fn pixel_bbox(b: [f64; 4], width: f64, height: f64) -> [f64; 4] {
    [b[0] * width, b[1] * height, b[2] * width, b[3] * height]
}
fn center(b: [f64; 4]) -> [f64; 2] {
    [(b[0] + b[2]) * 0.5, (b[1] + b[3]) * 0.5]
}
/// Evaluate at most 1200 frames including lookbehind and lookahead. Caller
/// emits only [emit_start,emit_end); 32+ frame overlap keeps local smoothing,
/// physical motion and time-based dedup decisions away from block boundaries.
/// Peaks are relative to first frame so this never allocates a full-game curve.
pub fn evaluate_block(
    frames: &[TouchFrame],
    width: u32,
    height: u32,
    emit_start: u64,
    emit_end: u64,
    settings: TouchSettings,
) -> Result<Vec<PhysicalTouchCandidate>, String> {
    settings.validate()?;
    if !(64..=16384).contains(&width) || !(64..=16384).contains(&height) {
        return Err("Invalid prepared-video width or height".into());
    }
    if frames.is_empty() || frames.len() > 1200 {
        return Err("Touch analysis block must contain 1..=1200 frames".into());
    }
    let first = frames[0].frame_index;
    let end = first
        .checked_add(frames.len() as u64)
        .ok_or("Touch frame index overflow")?;
    if emit_start < first || emit_start > emit_end || emit_end > end {
        return Err("Invalid touch emission frame range".into());
    }
    let (w, h) = (width as f64, height as f64);
    let mut proposals = Vec::new();
    let mut tracks = Vec::new();
    let mut balls = Vec::new();
    for (index, frame) in frames.iter().enumerate() {
        if frame.frame_index != first + index as u64 || frame.players.len() > 128 {
            return Err("Non-contiguous touch frames or too many player boxes".into());
        }
        let relative = index as u32;
        for player in &frame.players {
            if player.track_id == 0
                || player.track_id > i64::MAX as u64
                || !valid_bbox(player.bbox_xyxy_norm)
                || !player.score.is_finite()
                || !(0.0..=1.0).contains(&player.score)
                || player
                    .touch_probability
                    .is_some_and(|p| !p.is_finite() || !(0.0..=1.0).contains(&p))
            {
                return Err("Invalid tracked RAVEL player evidence".into());
            }
            let bbox = pixel_bbox(player.bbox_xyxy_norm, w, h);
            tracks.push(TrackedPlayer {
                frame: relative,
                track_id: player.track_id as i64,
                score: player.score,
                bbox,
            });
            if let Some(probability) = player.touch_probability.filter(|p| *p > 0.0) {
                proposals.push(TouchCandidate {
                    frame: relative,
                    probability,
                    bbox,
                });
            }
        }
        if let Some(ball) = &frame.selected_ball {
            if !matches!(ball.source_model.as_str(), "vballnet" | "ravel")
                || !valid_bbox(ball.bbox_xyxy_norm)
                || !ball.score.is_finite()
                || !(0.0..=1.0).contains(&ball.score)
            {
                return Err("Invalid selected-ball evidence".into());
            }
            let point = center(ball.bbox_xyxy_norm);
            balls.push(BallObservation {
                frame: relative as i64,
                x: point[0] * w,
                y: point[1] * h,
            });
        }
    }
    if proposals.is_empty() || balls.len() < 5 {
        return Ok(Vec::new());
    }
    let mut peaks = select_touch_peaks(
        &proposals,
        PeakConfig {
            threshold: settings.threshold,
            smooth: settings.smooth,
            min_gap: settings.min_gap,
        },
    )
    .map_err(str::to_owned)?;
    associate_touch_players(&mut peaks, &tracks);
    // False ball-source transitions can look like impacts. A contact must
    // have a uniform selected source on a short motion window around the peak,
    // with ball samples on both sides; otherwise retain it unconfirmed.
    peaks.retain(|peak| {
        let f = peak.frame_index as i64;
        let (mut before, mut after) = (0usize, 0usize);
        let mut source = None;
        let mut mixed = false;
        for other in frames {
            let delta = other.frame_index as i64 - first as i64 - f;
            if delta.abs() > 8 {
                continue;
            }
            if let Some(ball) = &other.selected_ball {
                if delta < 0 {
                    before += 1;
                } else if delta > 0 {
                    after += 1;
                }
                match source {
                    None => source = Some(ball.source_model.as_str()),
                    Some(current) if current != ball.source_model.as_str() => mixed = true,
                    _ => {}
                }
            }
        }
        !mixed && before >= 3 && after >= 3
    });
    if peaks.is_empty() {
        return Ok(Vec::new());
    }
    let confirmed: Vec<TouchContact> = confirm_ravel_contacts(
        &peaks,
        &balls,
        0,
        (frames.len() - 1) as i64,
        settings.threshold,
        settings.max_reach_ratio,
        settings.min_interval_sec,
    );
    let mut result = Vec::with_capacity(confirmed.len());
    for touch in confirmed {
        let frame = first + touch.frame as u64;
        if frame < emit_start || frame >= emit_end {
            continue;
        }
        result.push(PhysicalTouchCandidate {
            frame_index: frame,
            provisional_track_id: touch.player_track_id.map(|id| id as u64),
            probability: touch.confidence,
            ball_x_px: touch.ball_x,
            ball_y_px: touch.ball_y,
            motion_frame_index: first + touch.motion_frame as u64,
            direction_change_deg: touch.direction_change_deg,
            speed_before_px_frame: touch.speed_before_px_frame,
            speed_after_px_frame: touch.speed_after_px_frame,
            velocity_residual_px_frame: touch.velocity_residual_px_frame,
            technique: "unknown_touch",
            physical_motion_supported: true,
            athlete_identity_verified: false,
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scenario(curve: bool, close: bool, switching: bool) -> Vec<TouchFrame> {
        (0..80u64)
            .map(|i| {
                let x = if curve && i >= 32 {
                    0.20 + 32.0 * 0.005 - (i - 32) as f64 * 0.005
                } else {
                    0.20 + i as f64 * 0.005
                };
                let player = TouchPlayer {
                    track_id: 1,
                    score: 0.94,
                    touch_probability: Some(if (28..=36).contains(&i) { 0.95 } else { 0.01 }),
                    bbox_xyxy_norm: if close {
                        [0.20, 0.15, 0.45, 0.65]
                    } else {
                        [0.70, 0.15, 0.95, 0.65]
                    },
                };
                TouchFrame {
                    frame_index: i,
                    players: vec![player],
                    selected_ball: Some(TouchBall {
                        source_model: if switching && i >= 32 {
                            "ravel"
                        } else {
                            "vballnet"
                        }
                        .into(),
                        score: 0.95,
                        bbox_xyxy_norm: [x, 0.35, x + 0.02, 0.38],
                    }),
                }
            })
            .collect()
    }
    #[test]
    fn changing_ball_motion_plus_ravel_peaks_and_reach_proposes_contact() {
        let rows = evaluate_block(
            &scenario(true, true, false),
            640,
            360,
            0,
            80,
            TouchSettings::default(),
        )
        .unwrap();
        assert!(!rows.is_empty());
        assert!(rows
            .iter()
            .all(|p| p.physical_motion_supported && !p.athlete_identity_verified));
        assert!(rows.iter().any(|p| p.provisional_track_id == Some(1)));
        assert!(rows.iter().all(|p| p.technique == "unknown_touch"));
    }
    #[test]
    fn straight_flight_or_distant_player_does_not_produce_contact() {
        let normal = TouchSettings::default();
        assert!(
            evaluate_block(&scenario(false, true, false), 640, 360, 0, 80, normal)
                .unwrap()
                .is_empty()
        );
        assert!(
            evaluate_block(&scenario(true, false, false), 640, 360, 0, 80, normal)
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn model_switch_near_peak_is_not_a_physical_impact() {
        assert!(evaluate_block(
            &scenario(true, true, true),
            640,
            360,
            0,
            80,
            TouchSettings::default()
        )
        .unwrap()
        .is_empty());
    }
    #[test]
    fn no_reach_or_invalid_settings_or_nonmonotonic_frames_fail_closed() {
        let mut frames = scenario(true, true, false);
        frames[4].frame_index = 7;
        assert!(evaluate_block(&frames, 640, 360, 0, 80, TouchSettings::default()).is_err());
        assert!(TouchSettings {
            smooth: 0,
            ..TouchSettings::default()
        }
        .validate()
        .is_err());
        assert!(evaluate_block(
            &scenario(true, true, false),
            640,
            360,
            80,
            81,
            TouchSettings::default()
        )
        .is_err());
    }
    #[test]
    fn emission_bounds_do_not_repeat_events() {
        let frames = scenario(true, true, false);
        let a = evaluate_block(&frames, 640, 360, 0, 30, TouchSettings::default()).unwrap();
        let b = evaluate_block(&frames, 640, 360, 30, 80, TouchSettings::default()).unwrap();
        assert!(a.iter().all(|c| c.frame_index < 30));
        assert!(b.iter().all(|c| c.frame_index >= 30));
        assert!(!b.is_empty());
    }
}
