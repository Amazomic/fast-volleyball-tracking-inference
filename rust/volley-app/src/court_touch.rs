//! Court-informed CPU Touch classification, using only source-frame evidence.
//! Python parity: PlayerStore foot homography and stable per-track side,
//! player_contacts block threshold, preparation/serve-zone gating.

use std::collections::HashMap;
use volley_core::{
    action_prediction::{ActionKind, PredictionMethod},
    court_geometry::{CourtSide, CourtTransform},
    touch_pipeline::{PhysicalTouchCandidate, TouchFrame},
};

/// Discard spectators and benches from the Touch association stage.
/// Track IDs remain unchanged in the immutable CPU replay.
#[must_use]
pub(crate) fn filter_on_court(frames: &[TouchFrame], court: &CourtTransform) -> Vec<TouchFrame> {
    frames
        .iter()
        .cloned()
        .map(|mut frame| {
            frame
                .players
                .retain(|player| court.player_evidence(player.bbox_xyxy_norm).is_some());
            frame
        })
        .collect()
}

fn stable_sides(frames: &[TouchFrame], court: &CourtTransform) -> HashMap<u64, CourtSide> {
    let mut positions: HashMap<u64, Vec<f64>> = HashMap::new();
    for frame in frames {
        for player in &frame.players {
            if let Some(value) = court.player_evidence(player.bbox_xyxy_norm) {
                positions
                    .entry(player.track_id)
                    .or_default()
                    .push(value.x_m);
            }
        }
    }
    positions
        .into_iter()
        .filter_map(|(id, mut samples)| {
            if samples.is_empty() {
                return None;
            }
            samples.sort_by(f64::total_cmp);
            let median = samples[samples.len() / 2];
            Some((
                id,
                if median < 0.0 {
                    CourtSide::Near
                } else {
                    CourtSide::Far
                },
            ))
        })
        .collect()
}

/// Python reach ellipse: locate ball held by the serving player before a
/// strike, without confusing an airborne projected ball with a ground point.
fn ball_held_near_player(frame: &TouchFrame, track_id: u64) -> bool {
    let Some(player) = frame.players.iter().find(|p| p.track_id == track_id) else {
        return false;
    };
    let Some(ball) = frame.selected_ball.as_ref() else {
        return false;
    };
    let [x1, y1, x2, y2] = player.bbox_xyxy_norm;
    let [bx1, by1, bx2, by2] = ball.bbox_xyxy_norm;
    let (height, width) = ((y2 - y1).max(0.001), (x2 - x1).max(0.001));
    let radius = ((bx2 - bx1).abs() + (by2 - by1).abs()) * 0.25;
    let reach_x = (width * 0.70).max(height * 0.42) + radius;
    let reach_top = y1 - (height * 0.45 + radius);
    let reach_bottom = y2 + (height * 0.15 + radius);
    let reach_y = ((reach_bottom - reach_top) * 0.5).max(0.001);
    let center_y = (reach_bottom + reach_top) * 0.5;
    let x = (bx1 + bx2) * 0.5;
    let y = (by1 + by2) * 0.5;
    ((x - (x1 + x2) * 0.5) / reach_x).hypot((y - center_y) / reach_y) <= 0.60
}

/// Python find_rally_start requires >=0.5 s of handling and then a flight.
/// For CPU Touch this is a conservative local witness (not a verified serve).
fn observed_serve_preparation(frames: &[TouchFrame], frame_index: u64, track_id: u64) -> bool {
    const PREPARATION_FRAMES: usize = 15;
    const SERVE_TOSS_FRAMES: u64 = 75;
    const RELEASE_FLIGHT_FRAMES: usize = 9;
    let mut flight = 0usize;
    let mut preparation = 0usize;
    for frame in frames.iter().rev() {
        if frame.frame_index >= frame_index {
            continue;
        }
        if frame_index - frame.frame_index > SERVE_TOSS_FRAMES {
            break;
        }
        // An undetected ball is not evidence of a released toss.
        if frame.selected_ball.is_none() {
            flight = 0;
            preparation = 0;
            continue;
        }
        if flight < RELEASE_FLIGHT_FRAMES {
            if ball_held_near_player(frame, track_id) {
                flight = 0;
            } else {
                flight += 1;
            }
            continue;
        }
        if ball_held_near_player(frame, track_id) {
            preparation += 1;
            if preparation >= PREPARATION_FRAMES {
                return true;
            }
        } else {
            preparation = 0;
        }
    }
    false
}

/// Annotate only same-frame valid Track IDs. Pose evidence is already applied,
/// and court constraints are the last refinement of the unverified technique.
pub(crate) fn annotate(
    events: &mut [PhysicalTouchCandidate],
    frames: &[TouchFrame],
    court: &CourtTransform,
) {
    let Some(first) = frames.first().map(|f| f.frame_index) else {
        return;
    };
    let sides = stable_sides(frames, court);
    for event in events.iter_mut() {
        let Some(track_id) = event.provisional_track_id else {
            continue;
        };
        let Some(frame) = event
            .frame_index
            .checked_sub(first)
            .and_then(|idx| frames.get(idx as usize))
            .filter(|f| f.frame_index == event.frame_index)
        else {
            continue;
        };
        let Some(player) = frame.players.iter().find(|p| p.track_id == track_id) else {
            continue;
        };
        let Some(mut evidence) = court.player_evidence(player.bbox_xyxy_norm) else {
            continue;
        };
        evidence.side = sides.get(&track_id).copied().unwrap_or(evidence.side);
        event.court = Some(evidence);
        let Some(ref mut prediction) = event.action_prediction else {
            continue;
        };
        // Original Python: above the head AND within 1.6 m of net -> block.
        if evidence.near_net && prediction.depth_ratio < -0.05 {
            prediction.kind = ActionKind::Block;
            prediction.method = PredictionMethod::CourtGeometryV1;
        }
        // Original Python: serving zone alone is not enough to call a serve.
        // There must be preparation, source-frame reach, and acceleration.
        if evidence.in_serve_zone
            && event.speed_after_px_frame / event.speed_before_px_frame.max(0.1) >= 1.2
            && observed_serve_preparation(frames, event.frame_index, track_id)
        {
            prediction.kind = ActionKind::Serve;
            prediction.method = PredictionMethod::CourtGeometryV1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use volley_core::action_prediction::{ActionPrediction, PredictionMethod};

    fn court() -> CourtTransform {
        CourtTransform::new([[0.1, 0.2], [0.9, 0.2], [0.9, 0.8], [0.1, 0.8]]).unwrap()
    }

    fn frame(index: u64, foot_x: f64, ball: bool) -> TouchFrame {
        use volley_core::touch_pipeline::{TouchBall, TouchPlayer};
        let x1 = foot_x - 0.03;
        let x2 = foot_x + 0.03;
        TouchFrame {
            frame_index: index,
            players: vec![TouchPlayer {
                track_id: 3,
                score: 0.96,
                touch_probability: None,
                bbox_xyxy_norm: [x1, 0.35, x2, 0.75],
            }],
            selected_ball: ball.then_some(TouchBall {
                source_model: "vballnet".into(),
                score: 0.95,
                bbox_xyxy_norm: [foot_x - 0.01, 0.48, foot_x + 0.01, 0.50],
            }),
        }
    }

    fn event(index: u64) -> PhysicalTouchCandidate {
        PhysicalTouchCandidate {
            frame_index: index,
            provisional_track_id: Some(3),
            probability: 0.9,
            ball_x_px: 320.0,
            ball_y_px: 100.0,
            motion_frame_index: index,
            direction_change_deg: 45.0,
            speed_before_px_frame: 2.0,
            speed_after_px_frame: 9.0,
            velocity_residual_px_frame: 8.0,
            technique: "unknown_touch",
            action_prediction: Some(ActionPrediction {
                kind: ActionKind::Attack,
                evidence_score: 0.92,
                method: PredictionMethod::ContactGeometryV1,
                depth_ratio: -0.1,
                posture_ratio: 0.3,
            }),
            court: None,
            physical_motion_supported: true,
            athlete_identity_verified: false,
        }
    }

    #[test]
    fn spectators_are_excluded_but_source_track_ids_remain() {
        let mut frames = vec![frame(0, 0.5, false)];
        frames[0]
            .players
            .push(volley_core::touch_pipeline::TouchPlayer {
                track_id: 91,
                score: 0.92,
                touch_probability: None,
                bbox_xyxy_norm: [0.97, 0.32, 1.0, 0.74],
            });
        let kept = filter_on_court(&frames, &court());
        assert_eq!(kept[0].players.len(), 2);
        frames[0].players[1].bbox_xyxy_norm = [0.01, 0.32, 0.04, 0.74];
        let kept = filter_on_court(&frames, &court());
        assert_eq!(kept[0].players.len(), 2); // still within 3.5 m end margin
        frames[0].players[1].bbox_xyxy_norm = [0.01, 0.02, 0.04, 0.06];
        let kept = filter_on_court(&frames, &court());
        assert_eq!(kept[0].players.len(), 1);
        assert_eq!(kept[0].players[0].track_id, 3);
    }

    #[test]
    fn blocks_need_overhead_hit_and_net_proximity() {
        let frames = (0..60).map(|i| frame(i, 0.5, false)).collect::<Vec<_>>();
        let mut events = vec![event(30)];
        annotate(&mut events, &frames, &court());
        assert_eq!(
            events[0].action_prediction.as_ref().unwrap().kind,
            ActionKind::Block
        );
        assert!(events[0].court.unwrap().near_net);
        let far = (0..60).map(|i| frame(i, 0.2, false)).collect::<Vec<_>>();
        let mut events = vec![event(30)];
        annotate(&mut events, &far, &court());
        assert_eq!(
            events[0].action_prediction.as_ref().unwrap().kind,
            ActionKind::Attack
        );
    }

    #[test]
    fn serve_requires_preparation_not_merely_baseline_location() {
        let mut frames = (0..60).map(|i| frame(i, 0.125, true)).collect::<Vec<_>>();
        for frame in frames
            .iter_mut()
            .filter(|f| (26..=44).contains(&f.frame_index))
        {
            frame.selected_ball.as_mut().unwrap().bbox_xyxy_norm = [0.65, 0.1, 0.67, 0.12];
        }
        let mut events = vec![event(50)];
        annotate(&mut events, &frames, &court());
        assert_eq!(
            events[0].action_prediction.as_ref().unwrap().kind,
            ActionKind::Serve
        );
        let frames = (0..60).map(|i| frame(i, 0.125, false)).collect::<Vec<_>>();
        let mut events = vec![event(50)];
        annotate(&mut events, &frames, &court());
        assert_eq!(
            events[0].action_prediction.as_ref().unwrap().kind,
            ActionKind::Attack
        );
    }
}
