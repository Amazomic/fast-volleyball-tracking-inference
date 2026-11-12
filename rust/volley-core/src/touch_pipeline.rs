//! Streaming-friendly CPU interpretation of RAVEL Touch proposals against
//! actual, continuous selected-ball motion and provisional RAVEL Track IDs.
//! Detection is intentionally not a verified athlete identity or action label.
mod geometry;

use crate::{
    action_prediction::{self, ActionPrediction, CourtContext, PositionEvidence},
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
    /// Unverified, source-backed geometry hypothesis; never a reviewed game action.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_prediction: Option<ActionPrediction>,
    /// Player-foot projection on the user-calibrated court, if available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub court: Option<crate::court_geometry::CourtContactEvidence>,
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
/// Python PlayerStore's centered 61-observation, 80th-percentile height.
fn standing_height(
    frames: &[TouchFrame],
    frame_index: usize,
    track_id: u64,
    video_height: f64,
) -> Option<f64> {
    let heights = |frame: &TouchFrame| {
        frame
            .players
            .iter()
            .find(|player| player.track_id == track_id)
            .map(|player| (player.bbox_xyxy_norm[3] - player.bbox_xyxy_norm[1]) * video_height)
    };
    let mut observed: Vec<f64> = frames[..=frame_index]
        .iter()
        .rev()
        .filter_map(heights)
        .take(31)
        .collect();
    observed.extend(
        frames[frame_index + 1..]
            .iter()
            .filter_map(heights)
            .take(30),
    );
    if observed.is_empty() {
        return None;
    }
    observed.sort_by(f64::total_cmp);
    let quantile = (observed.len() - 1) as f64 * 0.8;
    let lower = quantile.floor() as usize;
    let upper = quantile.ceil() as usize;
    let estimate = observed[lower] + (observed[upper] - observed[lower]) * quantile.fract();
    Some(estimate)
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
    if balls.len() < 5 {
        return Ok(Vec::new());
    }
    let mut peaks = if proposals.is_empty() {
        Vec::new()
    } else {
        select_touch_peaks(
            &proposals,
            PeakConfig {
                threshold: settings.threshold,
                smooth: settings.smooth,
                min_gap: settings.min_gap,
            },
        )
        .map_err(str::to_owned)?
    };
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
        !mixed
            && before >= 3
            && after >= 3
            && !geometry::motion_window_has_outlier(frames, peak.frame_index as usize, w, h)
    });
    let geometry = geometry::detect(frames, &balls, w, h, settings.max_reach_ratio);
    let confirmed: Vec<TouchContact> = confirm_ravel_contacts(
        &peaks,
        &balls,
        0,
        (frames.len() - 1) as i64,
        settings.threshold,
        settings.max_reach_ratio,
        settings.min_interval_sec,
    );
    let mut result = Vec::with_capacity(confirmed.len() + geometry.len());
    for touch in confirmed {
        let frame = first + touch.frame as u64;
        if frame < emit_start || frame >= emit_end {
            continue;
        }
        // Peak association allows a +/-2 frame gap, but native CPU rallies
        // accept only an observed player and selected ball on the EXACT
        // contact frame. A nearby Track ID is a suggestion, not same-frame
        // evidence: preserve the physical Touch as unknown-player instead
        // of publishing an invalid foreign-frame reference.
        let Some(source) = frames.get(touch.frame as usize) else {
            continue;
        };
        if source.selected_ball.is_none() {
            continue;
        }
        let same_frame_track = touch.player_track_id.and_then(|id| {
            u64::try_from(id)
                .ok()
                .filter(|&id| source.players.iter().any(|player| player.track_id == id))
        });
        let action_prediction = same_frame_track.and_then(|track_id| {
            let player = source
                .players
                .iter()
                .find(|player| player.track_id == track_id)?;
            let ball = source.selected_ball.as_ref()?;
            let bbox = pixel_bbox(player.bbox_xyxy_norm, w, h);
            let radius = ((ball.bbox_xyxy_norm[2] - ball.bbox_xyxy_norm[0]) * w
                + (ball.bbox_xyxy_norm[3] - ball.bbox_xyxy_norm[1]) * h)
                * 0.25;
            action_prediction::predict(PositionEvidence {
                ball_y_px: touch.ball_y,
                ball_radius_px: radius,
                player_bbox_px: bbox,
                standing_height_px: standing_height(frames, touch.frame as usize, track_id, h)?,
                touch_score: touch.confidence,
                track_score: player.score,
                context: CourtContext::Unavailable,
            })
        });
        result.push(PhysicalTouchCandidate {
            frame_index: frame,
            provisional_track_id: same_frame_track,
            probability: touch.confidence,
            ball_x_px: touch.ball_x,
            ball_y_px: touch.ball_y,
            motion_frame_index: first + touch.motion_frame as u64,
            direction_change_deg: touch.direction_change_deg,
            speed_before_px_frame: touch.speed_before_px_frame,
            speed_after_px_frame: touch.speed_after_px_frame,
            velocity_residual_px_frame: touch.velocity_residual_px_frame,
            technique: "unknown_touch",
            action_prediction,
            court: None,
            physical_motion_supported: true,
            athlete_identity_verified: false,
        });
    }
    for candidate in geometry {
        let frame = first + candidate.frame as u64;
        if frame < emit_start || frame >= emit_end {
            continue;
        }
        let source = &frames[candidate.frame];
        let player = source
            .players
            .iter()
            .find(|p| p.track_id == candidate.track_id);
        let ball = source.selected_ball.as_ref();
        let (Some(player), Some(ball)) = (player, ball) else {
            continue;
        };
        let bbox = pixel_bbox(player.bbox_xyxy_norm, w, h);
        let radius = ((ball.bbox_xyxy_norm[2] - ball.bbox_xyxy_norm[0]) * w
            + (ball.bbox_xyxy_norm[3] - ball.bbox_xyxy_norm[1]) * h)
            * 0.25;
        let action_prediction = standing_height(frames, candidate.frame, candidate.track_id, h)
            .and_then(|height_px| {
                action_prediction::predict(PositionEvidence {
                    ball_y_px: candidate.ball_y,
                    ball_radius_px: radius,
                    player_bbox_px: bbox,
                    standing_height_px: height_px,
                    touch_score: candidate.probability,
                    track_score: player.score,
                    context: CourtContext::Unavailable,
                })
            });
        result.push(PhysicalTouchCandidate {
            frame_index: frame,
            provisional_track_id: Some(candidate.track_id),
            probability: candidate.probability,
            ball_x_px: candidate.ball_x,
            ball_y_px: candidate.ball_y,
            motion_frame_index: first + candidate.motion.motion_frame as u64,
            direction_change_deg: candidate.motion.angle,
            speed_before_px_frame: candidate.motion.speed_before,
            speed_after_px_frame: candidate.motion.speed_after,
            velocity_residual_px_frame: candidate.motion.residual,
            technique: "unknown_touch",
            action_prediction,
            court: None,
            physical_motion_supported: true,
            athlete_identity_verified: false,
        });
    }
    // RAVEL and geometric detection may describe the same impact. Keep the
    // most strongly supported proposal; never double-count it in a rally.
    result.sort_by(|a, b| {
        b.probability
            .total_cmp(&a.probability)
            .then_with(|| a.frame_index.cmp(&b.frame_index))
    });
    let mut unique: Vec<PhysicalTouchCandidate> = Vec::new();
    let min_gap = (settings.min_interval_sec * 30.0).round().max(1.0) as u64;
    for candidate in result {
        if unique
            .iter()
            .all(|old| old.frame_index.abs_diff(candidate.frame_index) > min_gap)
        {
            unique.push(candidate);
        }
    }
    unique.sort_by_key(|event| event.frame_index);
    Ok(unique)
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
    fn geometry_recovers_a_real_impact_without_any_ravel_touch_logits() {
        let mut frames = scenario(true, true, false);
        for (i, row) in frames.iter_mut().enumerate() {
            row.players[0].touch_probability = None;
            let x = if i <= 32 {
                0.20 + i as f64 * 0.008
            } else {
                0.20 + 32.0 * 0.008 - (i - 32) as f64 * 0.008
            };
            row.selected_ball.as_mut().unwrap().bbox_xyxy_norm[0] = x;
            row.selected_ball.as_mut().unwrap().bbox_xyxy_norm[2] = x + 0.02;
        }
        let contacts = evaluate_block(&frames, 640, 360, 0, 80, TouchSettings::default()).unwrap();
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].provisional_track_id, Some(1));
        assert!(contacts[0].action_prediction.is_some());
        assert_eq!(contacts[0].technique, "unknown_touch");
    }

    #[test]
    fn geometry_rejects_linear_flight_far_player_and_ball_source_switch() {
        for (curve, close, switch) in [
            (false, true, false),
            (true, false, false),
            (true, true, true),
        ] {
            let mut frames = scenario(curve, close, switch);
            for row in &mut frames {
                row.players[0].touch_probability = None;
            }
            let contacts =
                evaluate_block(&frames, 640, 360, 0, 80, TouchSettings::default()).unwrap();
            assert!(
                contacts.is_empty(),
                "curve={curve} close={close} switch={switch}"
            );
        }
    }

    #[test]
    fn geometry_rejects_one_frame_wrong_ball_localization() {
        let mut frames = scenario(false, true, false);
        for row in &mut frames {
            row.players[0].touch_probability = None;
        }
        // An isolated model mislocalization must not create two fake contacts.
        frames[32].selected_ball.as_mut().unwrap().bbox_xyxy_norm[0] = 0.70;
        frames[32].selected_ball.as_mut().unwrap().bbox_xyxy_norm[2] = 0.72;
        assert!(
            evaluate_block(&frames, 640, 360, 0, 80, TouchSettings::default())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn ravel_does_not_call_one_frame_ball_glitch_a_touch() {
        let mut frames = scenario(false, true, false);
        frames[32].selected_ball.as_mut().unwrap().bbox_xyxy_norm[0] = 0.70;
        frames[32].selected_ball.as_mut().unwrap().bbox_xyxy_norm[2] = 0.72;
        assert!(
            evaluate_block(&frames, 640, 360, 0, 80, TouchSettings::default())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn ravel_and_geometry_never_double_count_same_impact() {
        let frames = scenario(true, true, false);
        let contacts = evaluate_block(&frames, 640, 360, 0, 80, TouchSettings::default()).unwrap();
        assert_eq!(contacts.len(), 1);
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
        assert!(rows.iter().any(|p| {
            p.action_prediction.as_ref().is_some_and(|prediction| {
                prediction.kind == action_prediction::ActionKind::Dig
                    && prediction.evidence_score > 0.0
            })
        }));
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
    fn nearby_track_can_support_motion_but_not_claim_missing_source_frame() {
        // The strongest smoothed peak is frame 30, but RAVEL has no player
        // detection there. The +/-2 frame associator sees player 1 on 29/31.
        // The resulting physical Touch must retain UNKNOWN player identity.
        let mut frames = scenario(true, true, false);
        for (index, row) in frames.iter_mut().enumerate() {
            for player in &mut row.players {
                player.touch_probability = Some(if (28..=32).contains(&index) {
                    0.95
                } else {
                    0.01
                });
            }
        }
        frames[30].players.clear();
        let events = evaluate_block(&frames, 640, 360, 0, 80, TouchSettings::default()).unwrap();
        assert!(
            !events.is_empty(),
            "Motion-supported Touch must survive a one-frame player gap"
        );
        assert!(events
            .iter()
            .any(|event| event.frame_index == 30 && event.provisional_track_id.is_none()));
        assert!(events
            .iter()
            .all(|event| event.provisional_track_id.is_none_or(|id| frames
                [event.frame_index as usize]
                .players
                .iter()
                .any(|player| player.track_id == id))));
        assert!(events
            .iter()
            .all(|event| frames[event.frame_index as usize].selected_ball.is_some()));
    }

    #[test]
    fn absent_exact_frame_ball_cannot_be_published_from_nearby_motion() {
        let mut frames = scenario(true, true, false);
        for (index, row) in frames.iter_mut().enumerate() {
            for player in &mut row.players {
                player.touch_probability = Some(if (28..=32).contains(&index) {
                    0.95
                } else {
                    0.01
                });
            }
        }
        frames[30].selected_ball = None;
        let events = evaluate_block(&frames, 640, 360, 0, 80, TouchSettings::default()).unwrap();
        assert!(events
            .iter()
            .all(|event| frames[event.frame_index as usize].selected_ball.is_some()));
    }

    #[test]
    fn geometry_events_remain_inside_emission_bounds() {
        let mut frames = scenario(true, true, false);
        for (i, row) in frames.iter_mut().enumerate() {
            row.players[0].touch_probability = None;
            let x = if i <= 32 {
                0.20 + i as f64 * 0.008
            } else {
                0.20 + 32.0 * 0.008 - (i - 32) as f64 * 0.008
            };
            row.selected_ball.as_mut().unwrap().bbox_xyxy_norm[0] = x;
            row.selected_ball.as_mut().unwrap().bbox_xyxy_norm[2] = x + 0.02;
        }
        let before = evaluate_block(&frames, 640, 360, 0, 20, TouchSettings::default()).unwrap();
        let after = evaluate_block(&frames, 640, 360, 20, 80, TouchSettings::default()).unwrap();
        assert!(
            before.is_empty(),
            "A lookahead contact must not be emitted early"
        );
        assert_eq!(after.len(), 1);
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
