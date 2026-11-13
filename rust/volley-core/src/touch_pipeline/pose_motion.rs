use super::{
    center, geometry, pixel_bbox, standing_height, PhysicalTouchCandidate, TouchFrame,
    TouchSettings,
};
use crate::{
    action_prediction::{self, CourtContext, PositionEvidence},
    motion::{ball_motion_change, BallObservation, TOUCH_MOTION_CONFIG},
    pose_classification::{self, PoseEvidence},
    pose_contact::{self, ContactSample, ContactTrajectory},
};

#[derive(Clone, Debug)]
pub struct PosePlayer {
    pub track_id: u64,
    pub keypoints: [[f64; 3]; 17],
}

#[derive(Clone, Debug)]
pub struct PoseFrame {
    pub frame_index: u64,
    pub players: Vec<PosePlayer>,
}

/// One rebound may create several nearby gravity-compensated motion peaks.
/// For the same Track ID, only the strongest physical discontinuity survives
/// the longer (up to 0.4s) impact window, not the strongest neural Touch score.
fn deduplicate_pose_events(
    mut events: Vec<PhysicalTouchCandidate>,
    settings: TouchSettings,
) -> Vec<PhysicalTouchCandidate> {
    events.sort_by(|a, b| {
        b.velocity_residual_px_frame
            .total_cmp(&a.velocity_residual_px_frame)
            .then_with(|| b.probability.total_cmp(&a.probability))
            .then_with(|| a.frame_index.cmp(&b.frame_index))
    });
    let gap =
        ((settings.min_interval_sec * 30.0).round().max(1.0) as u64).max(settings.min_gap as u64);
    let same_player_gap = gap.saturating_mul(2).max(10);
    let mut unique: Vec<PhysicalTouchCandidate> = Vec::new();
    for event in events {
        if unique.iter().all(|old| {
            let distance = old.frame_index.abs_diff(event.frame_index);
            distance > gap
                && (old.provisional_track_id != event.provisional_track_id
                    || distance > same_player_gap)
        }) {
            unique.push(event);
        }
    }
    unique.sort_by_key(|event| event.frame_index);
    unique
}

/// Source-matched RTMPose interpolation + ball/limb temporal coincidence.
/// Both pose samples bracket the same video frame within 9 frames and must
/// belong to the candidate's existing CPU Track ID.
#[must_use]
pub fn pose_contact_at_frame(
    frames: &[TouchFrame],
    poses: &[PoseFrame],
    current: &TouchFrame,
    track_id: u64,
    width: u32,
    height: u32,
    settings: TouchSettings,
) -> Option<(PoseEvidence, f64)> {
    pose_contact_with_ball(frames, poses, current, track_id, width, height, settings, None)
}

/// The override is only used after a dual-sided observed VBallNet trajectory
/// fit. An inferred contact point is not an observed ball detection.
fn pose_contact_with_ball(
    frames: &[TouchFrame],
    poses: &[PoseFrame],
    current: &TouchFrame,
    track_id: u64,
    width: u32,
    height: u32,
    settings: TouchSettings,
    inferred_ball: Option<([f64; 2], f64)>,
) -> Option<(PoseEvidence, f64)> {
    let first = frames.first()?.frame_index;
    let (w, h) = (width as f64, height as f64);
    let player = current.players.iter().find(|p| p.track_id == track_id)?;
    let player_height = (player.bbox_xyxy_norm[3] - player.bbox_xyxy_norm[1]) * h;
    if player_height < 26.0 || !player_height.is_finite() {
        return None;
    }
    let (point, radius) = if let Some((position, radius)) = inferred_ball {
        if position.iter().any(|v| !v.is_finite())
            || !(0.0..=w).contains(&position[0])
            || !(0.0..=h).contains(&position[1])
            || !radius.is_finite()
            || radius <= 0.0
        {
            return None;
        }
        ([position[0] / w, position[1] / h], radius)
    } else {
        let ball = current.selected_ball.as_ref().filter(|b| b.source_model == "vballnet")?;
        let radius = ((ball.bbox_xyxy_norm[2] - ball.bbox_xyxy_norm[0]) * w
            + (ball.bbox_xyxy_norm[3] - ball.bbox_xyxy_norm[1]) * h) * 0.25;
        (center(ball.bbox_xyxy_norm), radius)
    };
    // RTMPose and VBallNet are sampled independently. A missing VBallNet
    // observation on the exact pose frame must not invalidate an otherwise
    // real contact. Use a nearby observed ball, never a synthetic or RAVEL one;
    // keep the observation on the same side of the impact.
    let sample_ball = |sample: &PoseFrame, before_impact: bool| -> Option<[f64; 2]> {
        let impact = current.frame_index;
        let (lo, hi) = if before_impact {
            (sample.frame_index.saturating_sub(3), sample.frame_index.saturating_add(3).min(impact.saturating_sub(1)))
        } else {
            (sample.frame_index.saturating_sub(3).max(impact.saturating_add(1)), sample.frame_index.saturating_add(3))
        };
        if lo > hi { return None; }
        (lo..=hi)
            .filter_map(|at| {
                let offset = at.checked_sub(first)?;
                let frame = frames.get(usize::try_from(offset).ok()?)?;
                if frame.frame_index != at { return None; }
                let ball = frame.selected_ball.as_ref().filter(|b| b.source_model == "vballnet")?;
                let [x, y] = center(ball.bbox_xyxy_norm);
                Some((at.abs_diff(sample.frame_index), [x * w, y * h]))
            })
            .min_by_key(|v| v.0)
            .map(|v| v.1)
    };
    let before = poses.iter().rev().find(|p| {
        p.frame_index < current.frame_index
            && current.frame_index - p.frame_index <= 9
            && p.players.iter().any(|v| v.track_id == track_id)
            && sample_ball(p, true).is_some()
    })?;
    let after = poses.iter().find(|p| {
        p.frame_index > current.frame_index
            && p.frame_index - current.frame_index <= 9
            && p.players.iter().any(|v| v.track_id == track_id)
            && sample_ball(p, false).is_some()
    })?;
    let prior = before.players.iter().find(|v| v.track_id == track_id)?;
    let next = after.players.iter().find(|v| v.track_id == track_id)?;
    let fraction = (current.frame_index - before.frame_index) as f64
        / (after.frame_index - before.frame_index) as f64;
    let observed = poses
        .iter()
        .find(|p| p.frame_index == current.frame_index)
        .and_then(|p| p.players.iter().find(|v| v.track_id == track_id));
    let mut interpolated = [[0.0; 3]; 17];
    for (joint, target) in interpolated.iter_mut().enumerate() {
        let (a, b) = (prior.keypoints[joint], next.keypoints[joint]);
        if a.iter().chain(b.iter()).any(|v| !v.is_finite())
            || a[0..2]
                .iter()
                .chain(b[0..2].iter())
                .any(|v| !(0.0..=1.0).contains(v))
            || a[2] < settings.pose_min_confidence
            || b[2] < settings.pose_min_confidence
            || a[2] > 1.0
            || b[2] > 1.0
        {
            continue;
        }
        let estimate = [
            a[0] + fraction * (b[0] - a[0]),
            a[1] + fraction * (b[1] - a[1]),
            a[2].min(b[2]),
        ];
        *target = observed
            .map(|pose| pose.keypoints[joint])
            .filter(|v| {
                v.iter().all(|n| n.is_finite() && (0.0..=1.0).contains(n))
                    && v[2] >= settings.pose_min_confidence
            })
            .unwrap_or(estimate);
    }
    let evidence = ContactTrajectory {
        before: ContactSample {
            keypoints: prior.keypoints,
            ball_px: sample_ball(before, true)?,
        },
        impact: ContactSample {
            keypoints: interpolated,
            ball_px: [point[0] * w, point[1] * h],
        },
        after: ContactSample {
            keypoints: next.keypoints,
            ball_px: sample_ball(after, false)?,
        },
        video_width: w,
        video_height: h,
        player_height_px: player_height,
        ball_radius_px: radius,
        min_confidence: settings.pose_min_confidence,
        max_distance_ratio: settings.pose_max_distance,
    };
    let separation = pose_contact::trajectory_body_intersection(&evidence)?;
    Some((
        PoseEvidence {
            keypoints: interpolated,
            ball_x_norm: point[0],
            ball_y_norm: point[1],
            video_width: w,
            video_height: h,
            player_bbox_norm: player.bbox_xyxy_norm,
        },
        separation,
    ))
}

/// Missing ball at impact is common under a striking hand. Estimate only
/// a *hypothetical contact point* from two authentic VBallNet observations
/// on each side, fitting independent inbound and outbound velocities.
/// Never bridge a trajectory with one straight line across a likely rebound.
fn occluded_ball_impact(
    frames: &[TouchFrame],
    index: usize,
    width: u32,
    height: u32,
    settings: TouchSettings,
) -> Option<([f64; 2], f64, crate::motion::MotionEvidence)> {
    let frame = frames.get(index)?;
    if frame.selected_ball.is_some() || index < 9 || index + 9 >= frames.len() {
        return None;
    }
    let (w, h) = (width as f64, height as f64);
    let observed = |i: usize| -> Option<(f64, [f64; 2], f64)> {
        let row = frames.get(i)?;
        let ball = row.selected_ball.as_ref().filter(|b| {
            b.source_model == "vballnet" && b.score >= settings.min_ball_score
        })?;
        let p = center(ball.bbox_xyxy_norm);
        let radius = ((ball.bbox_xyxy_norm[2] - ball.bbox_xyxy_norm[0]) * w
            + (ball.bbox_xyxy_norm[3] - ball.bbox_xyxy_norm[1]) * h) * 0.25;
        (radius > 1.0 && radius < 20.0).then_some((row.frame_index as f64, [p[0] * w, p[1] * h], radius))
    };
    let left = (index - 9..index).rev().filter_map(observed).take(2).collect::<Vec<_>>();
    let right = (index + 1..=index + 9).filter_map(observed).take(2).collect::<Vec<_>>();
    if left.len() != 2 || right.len() != 2 {
        return None;
    }
    let (l1, l2, r1, r2) = (left[0], left[1], right[0], right[1]);
    // Short occlusion, with two close observations on each physical flight.
    if frame.frame_index as f64 - l1.0 > 4.0
        || r1.0 - frame.frame_index as f64 > 4.0
        || l1.0 - l2.0 > 4.0
        || r2.0 - r1.0 > 4.0
        || r1.0 - l1.0 > 7.0
    {
        return None;
    }
    let before = [(l1.1[0] - l2.1[0]) / (l1.0 - l2.0),
                  (l1.1[1] - l2.1[1]) / (l1.0 - l2.0)];
    let after = [(r2.1[0] - r1.1[0]) / (r2.0 - r1.0),
                 (r2.1[1] - r1.1[1]) / (r2.0 - r1.0)];
    let (speed_before, speed_after) = (before[0].hypot(before[1]), after[0].hypot(after[1]));
    if speed_before < 2.0 || speed_after < 2.0
        || speed_before > 55.0 || speed_after > 55.0 {
        return None;
    }
    let residual = (after[0] - before[0]).hypot(after[1] - before[1]);
    let angle = ((before[0] * after[0] + before[1] * after[1])
        / (speed_before * speed_after)).clamp(-1.0, 1.0).acos().to_degrees();
    // A ball accelerating along its unchanged flight direction is not a
    // player touch, even if its speed increases through a brief dropout.
    // The minimum physical turn is part of the saved global CPU profile.
    if residual < 6.0 || angle < settings.occluded_min_turn_deg {
        return None;
    }
    let t = frame.frame_index as f64;
    let inbound = [l1.1[0] + before[0] * (t - l1.0),
                   l1.1[1] + before[1] * (t - l1.0)];
    let outbound = [r1.1[0] - after[0] * (r1.0 - t),
                    r1.1[1] - after[1] * (r1.0 - t)];
    // The two real-flight extrapolations must converge within a bounded
    // one-ball-width tolerance. Fast hits can place the inferred physical
    // collision between the nearest sampled frames.
    if (inbound[0] - outbound[0]).hypot(inbound[1] - outbound[1]) > 42.0 {
        return None;
    }
    let impact = [(inbound[0] + outbound[0]) * 0.5,
                  (inbound[1] + outbound[1]) * 0.5];
    if !(0.0..=w).contains(&impact[0]) || !(0.0..=h).contains(&impact[1]) {
        return None;
    }
    let radius = (l1.2 + r1.2) * 0.5;
    Some((impact, radius, crate::motion::MotionEvidence {
        motion_frame: frame.frame_index as i64,
        speed_before,
        speed_after,
        angle,
        residual,
    }))
}

/// A pose overlap is not a Touch if the velocity break belongs to a
/// *later* frame. Test the immediate observed flight on both sides; this
/// rejects anticipatory contacts during otherwise smooth acceleration while
/// retaining an independently observed reversal at the actual impact.
fn local_observed_velocity_break(
    frames: &[TouchFrame],
    index: usize,
    width: f64,
    height: f64,
) -> Option<f64> {
    let point = |at: usize| -> Option<[f64; 2]> {
        let ball = frames.get(at)?.selected_ball.as_ref()?;
        if ball.source_model != "vballnet" {
            return None;
        }
        let p = center(ball.bbox_xyxy_norm);
        Some([p[0] * width, p[1] * height])
    };
    let left = point(index.checked_sub(3)?)?;
    let left_near = point(index.checked_sub(1)?)?;
    let right_near = point(index + 1)?;
    let right = point(index + 3)?;
    let before = [(left_near[0] - left[0]) * 0.5, (left_near[1] - left[1]) * 0.5];
    let after = [(right[0] - right_near[0]) * 0.5, (right[1] - right_near[1]) * 0.5];
    // Allow gravity-induced acceleration, but not a large nonlinear turn
    // from the natural ballistic arc alone.
    Some((after[0] - before[0]).hypot(after[1] - before[1] - 4.0))
}

/// Distinguish a pose overlap during descending ball flight from the later
/// actual rebound. Check the next three independently observed VBallNet
/// samples, skipping dropouts; they must continue descending together.
fn still_descending_after_pose_overlap(frames: &[TouchFrame], index: usize) -> bool {
    let Some(ball) = frames.get(index).and_then(|f| f.selected_ball.as_ref())
        .filter(|b| b.source_model == "vballnet") else {
        return false;
    };
    let mut y_previous = center(ball.bbox_xyxy_norm)[1];
    let mut descent_count = 0usize;
    for frame in frames.iter().skip(index + 1).take(5) {
        let Some(ball) = frame.selected_ball.as_ref().filter(|b| b.source_model == "vballnet") else {
            continue;
        };
        let y = center(ball.bbox_xyxy_norm)[1];
        if y <= y_previous {
            return false;
        }
        y_previous = y;
        descent_count += 1;
        if descent_count >= 3 {
            return true;
        }
    }
    false
}

/// A later trajectory change cannot certify an earlier pose overlap.
/// Missing nearby VBallNet observations are absence of evidence, not proof
/// of an impact. A true impact at its own motion frame is not restricted.
fn unsupported_anticipated_impact(
    frames: &[TouchFrame],
    index: usize,
    motion_frame: i64,
    width: f64,
    height: f64,
) -> bool {
    (motion_frame > index as i64
        && still_descending_after_pose_overlap(frames, index))
        || (motion_frame > index as i64 + 2
            && local_observed_velocity_break(frames, index, width, height)
                .is_none_or(|residual| residual < 12.0))
}

pub fn evaluate_pose_block(
    frames: &[TouchFrame],
    poses: &[PoseFrame],
    width: u32,
    height: u32,
    emit_start: u64,
    emit_end: u64,
    settings: TouchSettings,
) -> Result<Vec<PhysicalTouchCandidate>, String> {
    settings.validate()?;
    if frames.is_empty()
        || frames.len() > 1200
        || !(64..=16384).contains(&width)
        || !(64..=16384).contains(&height)
        || poses
            .windows(2)
            .any(|p| p[0].frame_index >= p[1].frame_index)
    {
        return Err("Invalid pose touch block".into());
    }
    let first = frames[0].frame_index;
    let end = first
        .checked_add(frames.len() as u64)
        .ok_or("Frame overflow")?;
    if emit_start < first
        || emit_end > end
        || emit_start > emit_end
        || frames
            .iter()
            .enumerate()
            .any(|(i, f)| f.frame_index != first + i as u64)
    {
        return Err("Invalid pose touch frame range".into());
    }
    let (w, h) = (width as f64, height as f64);
    let balls: Vec<_> = frames
        .iter()
        .filter_map(|f| {
            let ball = f.selected_ball.as_ref()?;
            if ball.source_model != "vballnet" {
                return None;
            }
            let p = center(ball.bbox_xyxy_norm);
            Some(BallObservation {
                frame: (f.frame_index - first) as i64,
                x: p[0] * w,
                y: p[1] * h,
            })
        })
        .collect();
    let mut result = Vec::new();
    for (index, frame) in frames.iter().enumerate() {
        if !geometry::stable_pose_ball_source(frames, index)
            || geometry::motion_window_has_outlier(frames, index, w, h)
        {
            continue;
        }
        let Some(ball) = frame
            .selected_ball
            .as_ref()
            .filter(|b| b.source_model == "vballnet")
        else {
            continue;
        };
        // Brief, explicitly discarded localization islands may span six to nine
        // frames. Both sides still require real, same-source ball samples.
        let motion_config = crate::motion::MotionConfig {
            max_gap_frames: 9,
            ..TOUCH_MOTION_CONFIG
        };
        // Pose/ball overlap identifies the contact timestamp. The fitted
        // trajectory kink can peak a few frames later because velocity is
        // estimated from multiple observations. Keep the real contact frame
        // and look for independent motion evidence within 0.1 second.
        let Some(motion) = ball_motion_change(&balls, index as i64, 3, motion_config) else {
            continue;
        };
        // The wider pose-motion search can find a collision up to three
        // frames *after* the proposed overlap. In that case only keep an
        // earlier pose hit if its own observed local trajectory also breaks.
        if unsupported_anticipated_impact(frames, index, motion.motion_frame, w, h) {
            continue;
        }
        let point = center(ball.bbox_xyxy_norm);
        let ball_px = [point[0] * w, point[1] * h];
        let mut best: Option<(f64, u64, PoseEvidence)> = None;
        for player in &frame.players {
            let Some((evidence, distance)) = pose_contact_at_frame(
                frames,
                poses,
                frame,
                player.track_id,
                width,
                height,
                settings,
            ) else {
                continue;
            };
            if best.as_ref().is_none_or(|candidate| distance < candidate.0) {
                best = Some((distance, player.track_id, evidence));
            }
        }
        let Some((distance, track_id, evidence)) = best else {
            continue;
        };
        let Some(player) = frame.players.iter().find(|p| p.track_id == track_id) else {
            continue;
        };
        let bbox = pixel_bbox(player.bbox_xyxy_norm, w, h);
        let radius = ((ball.bbox_xyxy_norm[2] - ball.bbox_xyxy_norm[0]) * w
            + (ball.bbox_xyxy_norm[3] - ball.bbox_xyxy_norm[1]) * h)
            * 0.25;
        let score = (0.45
            + 0.2 * (1.0 - distance / settings.pose_max_distance)
            + 0.15 * ball.score
            + 0.1 * player.score)
            .clamp(0.0, 0.9);
        let action_prediction = standing_height(frames, index, track_id, h)
            .and_then(|standing| {
                action_prediction::predict(PositionEvidence {
                    ball_y_px: ball_px[1],
                    ball_radius_px: radius,
                    player_bbox_px: bbox,
                    standing_height_px: standing,
                    touch_score: score,
                    track_score: player.score,
                    context: CourtContext::Unavailable,
                })
            })
            .map(|geometry| pose_classification::refine(&evidence, &geometry).unwrap_or(geometry));
        result.push(PhysicalTouchCandidate {
            frame_index: frame.frame_index,
            provisional_track_id: Some(track_id),
            probability: score,
            ball_x_px: ball_px[0],
            ball_y_px: ball_px[1],
            motion_frame_index: first + motion.motion_frame as u64,
            direction_change_deg: motion.angle,
            speed_before_px_frame: motion.speed_before,
            speed_after_px_frame: motion.speed_after,
            velocity_residual_px_frame: motion.residual,
            technique: "unknown_touch",
            action_prediction,
            court: None,
            physical_motion_supported: true,
            body_contact_supported: true,
            ball_occluded_at_contact: false,
            inferred_body_reach_supported: false,
            athlete_identity_verified: false,
        });
    }
    // Independent occlusion branch. It never alters original ball frames.
    // Every inferred collision requires a real track, a bracketing COCO17
    // body segment and a convergent pair of measured ball flights.
    for (index, frame) in frames.iter().enumerate() {
        let Some((point, radius, motion)) =
            occluded_ball_impact(frames, index, width, height, settings)
        else {
            continue;
        };
        let mut best: Option<(f64, u64, PoseEvidence)> = None;
        for player in &frame.players {
            let Some((evidence, distance)) = pose_contact_with_ball(
                frames, poses, frame, player.track_id, width, height, settings,
                Some((point, radius)),
            ) else {
                continue;
            };
            if best.as_ref().is_none_or(|candidate| distance < candidate.0) {
                best = Some((distance, player.track_id, evidence));
            }
        }
        // If COCO17 joints disappear during a jump, keep a lower-confidence
        // reach hypothesis rather than silently dropping a genuine trajectory
        // kink. The body must be a stable, already tracked person, never a
        // fleeting duplicate or an arbitrary neighboring detection.
        let reach_candidate = if best.is_none() && !settings.require_pose {
            frame.players.iter().filter_map(|player| {
                let bb = pixel_bbox(player.bbox_xyxy_norm, w, h);
                let height = bb[3] - bb[1];
                if height < 25.0 {
                    return None;
                }
                let continuity = frames[index.saturating_sub(6)..(index + 7).min(frames.len())]
                    .iter()
                    .filter(|row| row.players.iter().any(|p| p.track_id == player.track_id))
                    .count();
                if continuity < 6 {
                    return None;
                }
                // A fully extended attack can meet the ball above a cropped
                // torso box; geometry alone remains an unconfirmed proposal.
                let top = bb[1] - 0.55 * height;
                let bottom = bb[3] + 0.10 * height;
                let margin = 0.28 * (bb[2] - bb[0]);
                if point[0] < bb[0] - margin || point[0] > bb[2] + margin
                    || point[1] < top || point[1] > bottom {
                    return None;
                }
                let head_distance = (point[0] - (bb[0] + bb[2]) * 0.5).abs()
                    / (bb[2] - bb[0]).max(1.0);
                Some((head_distance - 0.15 * player.score, player.track_id))
            }).min_by(|a, b| a.0.total_cmp(&b.0))
        } else {
            None
        };
        let (track_id, distance, witness) = if let Some((distance, track_id, evidence)) = best {
            (track_id, distance, Some(evidence))
        } else if let Some((_, track_id)) = reach_candidate {
            (track_id, settings.pose_max_distance * 0.8, None)
        } else {
            continue;
        };
        let Some(player) = frame.players.iter().find(|p| p.track_id == track_id) else {
            continue;
        };
        let bbox = pixel_bbox(player.bbox_xyxy_norm, w, h);
        let score = if witness.is_some() {
            (0.48 + 0.20 * (1.0 - distance / settings.pose_max_distance)
                + 0.12 * player.score).clamp(0.0, 0.85)
        } else {
            (0.49 + 0.12 * player.score + 0.07 * (motion.residual / 30.0).min(1.0))
                .clamp(0.0, 0.70)
        };
        let action_prediction = standing_height(frames, index, track_id, h)
            .and_then(|standing| action_prediction::predict(PositionEvidence {
                ball_y_px: point[1], ball_radius_px: radius,
                player_bbox_px: bbox, standing_height_px: standing,
                touch_score: score, track_score: player.score,
                context: CourtContext::Unavailable,
            }))
            .map(|geometry| {
                witness.as_ref()
                    .and_then(|evidence| pose_classification::refine(evidence, &geometry))
                    .unwrap_or(geometry)
            });
        result.push(PhysicalTouchCandidate {
            frame_index: frame.frame_index,
            provisional_track_id: Some(track_id),
            probability: score,
            ball_x_px: point[0],
            ball_y_px: point[1],
            motion_frame_index: frame.frame_index,
            direction_change_deg: motion.angle,
            speed_before_px_frame: motion.speed_before,
            speed_after_px_frame: motion.speed_after,
            velocity_residual_px_frame: motion.residual,
            technique: "unknown_touch",
            action_prediction,
            court: None,
            physical_motion_supported: true,
            body_contact_supported: witness.is_some(),
            ball_occluded_at_contact: true,
            inferred_body_reach_supported: witness.is_none(),
            athlete_identity_verified: false,
        });
    }
    let mut unique = deduplicate_pose_events(result, settings);
    unique.retain(|event| event.frame_index >= emit_start && event.frame_index < emit_end);
    Ok(unique)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::touch_pipeline::{TouchBall, TouchPlayer};

    #[test]
    fn volley_20_7s_ballistic_acceleration_during_occlusion_is_not_contact() {
        // Genuine observations in saved new3 around the remote VBallNet
        // false localization at 622; a missing observation must not turn
        // gravity-driven speed growth into an impact.
        let observations = [
            (619, [0.533, 0.406]),
            (620, [0.531, 0.427]),
            (621, [0.529, 0.441]),
            (623, [0.525, 0.510]),
            (624, [0.521, 0.535]),
            (625, [0.521, 0.580]),
        ];
        let mut frames = (613..636u64).map(|frame_index| TouchFrame {
            frame_index,
            players: vec![],
            selected_ball: None,
        }).collect::<Vec<_>>();
        for (frame_index, p) in observations {
            frames[(frame_index-613) as usize].selected_ball = Some(TouchBall {
                source_model: "vballnet".into(),score:0.65,
                bbox_xyxy_norm: [p[0]-0.005,p[1]-0.006,p[0]+0.005,p[1]+0.006],
            });
        }
        let at = (622-613) as usize;
        let diagnostic = occluded_ball_impact(
            &frames, at, 1280, 720,
            TouchSettings { occluded_min_turn_deg: 0.0, ..TouchSettings::default() },
        );
        assert!(diagnostic.is_some(), "speed-only false witness must be reproducible");
        assert!(
            occluded_ball_impact(&frames, at, 1280, 720, TouchSettings::default()).is_none(),
            "The saved global turning threshold must reject this ballistic gap"
        );
    }

    #[test]
    fn volley_20_7s_missing_vballnet_never_authorizes_future_impact() {
        // Original tracked positions at 619-631, including VBallNet's
        // remote right-edge false positives at 622, 627 and 630. Their
        // repaired touch-input observations are absent, not real impacts.
        let observed = [
            (619, Some([0.533, 0.406])),
            (620, Some([0.531, 0.427])),
            (621, Some([0.529, 0.441])),
            (622, None),
            (623, Some([0.525, 0.510])),
            (624, Some([0.521, 0.535])),
            (625, Some([0.521, 0.580])),
            (626, Some([0.518, 0.604])),
            (627, None),
            (628, Some([0.508, 0.701])),
            (629, Some([0.508, 0.743])),
            (630, None),
            (631, None),
        ];
        let frames = observed.into_iter().map(|(frame_index,xy)| TouchFrame {
            frame_index,
            selected_ball: xy.map(|p| TouchBall {
                source_model: "vballnet".to_string(), score:0.65,
                bbox_xyxy_norm: [p[0]-0.005,p[1]-0.005,p[0]+0.005,p[1]+0.005],
            }),
            players: vec![],
        }).collect::<Vec<_>>();
        let at_625 = 625 - 619;
        assert_eq!(
            local_observed_velocity_break(&frames, at_625, 1280.0, 720.0),
            None,
            "The source has an absent 622 observation"
        );
        assert!(
            still_descending_after_pose_overlap(&frames, 624 - 619),
            "624 is still on the descending flight, not a contact"
        );
        assert!(
            unsupported_anticipated_impact(&frames, 624 - 619, 626 - 619, 1280.0, 720.0),
            "Earlier pose overlap must not borrow the later 626 motion peak"
        );
        assert!(
            unsupported_anticipated_impact(&frames, at_625, 628 - 619, 1280.0, 720.0),
            "The frame-628 reversal must not be borrowed by frame 625"
        );
        assert!(!unsupported_anticipated_impact(
            &frames, 630 - 619, 630 - 619, 1280.0, 720.0
        ), "Contact at its own motion frame is unaffected");
    }

    #[test]
    fn straight_ball_near_pose_at_38_4s_does_not_claim_later_attack() {
        let samples = [
            [0.520,0.417], [0.529,0.438], [0.537,0.451],
            [0.549,0.479], [0.555,0.503], [0.570,0.535],
            [0.576,0.563], [0.588,0.590], [0.600,0.625],
            [0.611,0.670], [0.619,0.698], [0.621,0.660],
            [0.627,0.649], [0.631,0.601], [0.631,0.556],
        ];
        let frames = samples.iter().enumerate().map(|(i, p)| TouchFrame {
            frame_index: 1147 + i as u64,
            selected_ball: Some(TouchBall {
                source_model: "vballnet".to_owned(),
                score: 0.65,
                bbox_xyxy_norm: [p[0]-0.005,p[1]-0.005,p[0]+0.005,p[1]+0.005],
            }),
            players: vec![],
        }).collect::<Vec<_>>();
        let local_false = local_observed_velocity_break(&frames, 5, 1280.0, 720.0).unwrap();
        let local_rebound = local_observed_velocity_break(&frames, 10, 1280.0, 720.0).unwrap();
        assert!(local_false < 12.0, "smooth frame 1152: {local_false}");
        assert!(local_rebound > 12.0, "real frame 1157 rebound: {local_rebound}");
    }

    #[test]
    fn volley_23_4s_rebound_through_ball_dropout_is_provisional_touch() {
        // Coordinates are from the saved new3 VBallNet replay at 695..708.
        // Frames 697..700 were a remote false localization, suppressed
        // before reaching this detector. COCO17 is unavailable at the impact.
        let mut frames = (690..715u64).map(|frame_index| TouchFrame {
            frame_index,
            players: vec![TouchPlayer {
                track_id: 8,
                score: 0.74,
                bbox_xyxy_norm: [0.448, 0.452, 0.493, 0.605],
                touch_probability: None,
            }],
            selected_ball: None,
        }).collect::<Vec<_>>();
        let evidence = [
            (695, [0.477, 0.372]), (696, [0.475, 0.382]),
            (701, [0.443, 0.354]), (702, [0.430, 0.347]),
            (707, [0.389, 0.337]), (708, [0.385, 0.337]),
        ];
        for (frame, point) in evidence {
            frames[frame - 690].selected_ball = Some(crate::touch_pipeline::TouchBall {
                source_model: "vballnet".to_owned(),
                score: 0.72,
                bbox_xyxy_norm: [
                    point[0] - 0.005, point[1] - 0.008,
                    point[0] + 0.005, point[1] + 0.008,
                ],
            });
        }
        let events = evaluate_pose_block(
            &frames, &[], 1280, 720, 695, 709, TouchSettings::default(),
        ).unwrap();
        assert!(events.iter().any(|event| (697..=700).contains(&event.frame_index)
            && event.provisional_track_id == Some(8)
            && event.ball_occluded_at_contact
            && event.inferred_body_reach_supported
            && !event.body_contact_supported),
            "a jump-contact trajectory kink may yield an unverified tracked Touch: {events:?}");
        // The same physical kink without an observed player must NOT become a Touch.
        for frame in &mut frames {
            frame.players.clear();
        }
        assert!(evaluate_pose_block(
            &frames, &[], 1280, 720, 695, 709, TouchSettings::default()
        ).unwrap().is_empty());
    }

    fn sparse_rebound(rebound: bool) -> (Vec<TouchFrame>, Vec<PoseFrame>) {
        let frames = (0..80u64)
            .map(|frame_index| {
                let x = if rebound && frame_index > 32 {
                    0.20 + 32.0 * 0.008 - (frame_index - 32) as f64 * 0.008
                } else {
                    0.20 + frame_index as f64 * 0.008
                };
                TouchFrame {
                    frame_index,
                    players: vec![TouchPlayer {
                        track_id: 42,
                        score: 0.95,
                        touch_probability: None,
                        bbox_xyxy_norm: [0.30, 0.12, 0.60, 0.75],
                    }],
                    selected_ball: [28, 30, 32, 34, 36].contains(&frame_index).then_some(
                        TouchBall {
                            source_model: "vballnet".into(),
                            score: 0.96,
                            bbox_xyxy_norm: [x, 0.35, x + 0.02, 0.38],
                        },
                    ),
                }
            })
            .collect();
        let poses = [(28, 0.42), (32, 0.466), (36, 0.50)]
            .into_iter()
            .map(|(frame_index, wrist_x)| {
                let mut keypoints = [[0.0; 3]; 17];
                keypoints[9] = [wrist_x, 0.365, 0.97];
                PoseFrame {
                    frame_index,
                    players: vec![PosePlayer {
                        track_id: 42,
                        keypoints,
                    }],
                }
            })
            .collect();
        (frames, poses)
    }

    #[test]
    fn nearby_motion_peak_can_confirm_earlier_body_contact() {
        // Model a contact near frame 30 while the fitted motion break peaks
        // at frame 32. Only observed VBallNet-like coordinates are used.
        let balls: Vec<_> = (20..=40)
            .map(|frame| BallObservation {
                frame,
                x: if frame <= 32 {
                    140.0 + 5.0 * (frame - 20) as f64
                } else {
                    200.0 - 7.0 * (frame - 32) as f64
                },
                y: 130.0,
            })
            .collect();
        let config = crate::motion::MotionConfig {
            max_gap_frames: 9,
            ..TOUCH_MOTION_CONFIG
        };
        let offset_contact = (27..=31).find(|&frame| {
            ball_motion_change(&balls, frame, 0, config).is_none()
                && ball_motion_change(&balls, frame, 3, config).is_some()
        });
        assert!(offset_contact.is_some(), "motion window must recover a nearby kink");
    }

    #[test]
    fn pose_contact_uses_adjacent_real_ball_when_pose_sample_has_no_ball() {
        let (mut frames, poses) = sparse_rebound(true);
        // The two bracketing RTMPose samples remain valid, but VBallNet
        // missed the ball on their exact timestamps.
        for at in [28usize, 36] {
            let observed = frames[at].selected_ball.take().unwrap();
            frames[at - 1].selected_ball = Some(observed);
        }
        let event = pose_contact_at_frame(
            &frames, &poses, &frames[32], 42, 640, 360, TouchSettings::default(),
        );
        assert!(event.is_some(), "independent pose/ball sampling must not erase contact");
    }

    #[test]
    fn secondary_motion_peak_of_one_player_is_one_touch() {
        let (frames, poses) = sparse_rebound(true);
        let mut found =
            evaluate_pose_block(&frames, &poses, 640, 360, 0, 80, TouchSettings::default())
                .unwrap();
        assert_eq!(found.len(), 1);
        let primary = found.remove(0);
        let mut duplicate = primary.clone();
        duplicate.frame_index += 7;
        duplicate.motion_frame_index += 4;
        duplicate.velocity_residual_px_frame *= 0.30;
        duplicate.probability = 1.0;
        let kept =
            deduplicate_pose_events(vec![duplicate, primary.clone()], TouchSettings::default());
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].frame_index, primary.frame_index);
    }


    #[test]
    fn occluded_ball_with_real_inbound_outbound_flights_can_hit_wrist() {
        let (mut frames, poses) = sparse_rebound(true);
        frames[32].selected_ball = None;
        let inferred = occluded_ball_impact(
            &frames, 32, 640, 360, TouchSettings::default()
        ).expect("two real flights must converge at an occluded collision");
        assert!(inferred.2.angle > 35.0);
        assert!(pose_contact_with_ball(
            &frames, &poses, &frames[32], 42, 640, 360,
            TouchSettings::default(), Some((inferred.0, inferred.1))
        ).is_some(), "a body witness must confirm the hypothesized rebound");
    }

    #[test]
    fn missing_ball_during_straight_flight_is_not_a_contact() {
        let (mut frames, _) = sparse_rebound(false);
        frames[32].selected_ball = None;
        assert!(occluded_ball_impact(
            &frames, 32, 640, 360, TouchSettings::default()
        ).is_none(), "occlusion alone cannot create a touch");
    }
    #[test]
    fn sparse_vballnet_rebound_is_not_lost_when_pose_confirms_contact() {
        let (frames, poses) = sparse_rebound(true);
        assert!(!geometry::stable_ball_source(&frames, 32));
        assert!(geometry::stable_ball_source_with_min_samples(
            &frames, 32, 2
        ));
        let found = evaluate_pose_block(&frames, &poses, 640, 360, 0, 80, TouchSettings::default())
            .unwrap();
        assert_eq!(
            found.len(),
            1,
            "a wrist collision with a motion reversal is a contact"
        );
        assert_eq!(found[0].frame_index, 32);
        assert_eq!(found[0].provisional_track_id, Some(42));
        assert!(
            found[0].action_prediction.is_some(),
            "geometry label is retained"
        );

        let (straight, _) = sparse_rebound(false);
        assert!(
            evaluate_pose_block(&straight, &poses, 640, 360, 0, 80, TouchSettings::default(),)
                .unwrap()
                .is_empty(),
            "straight flight near the wrist must not create a touch"
        );
    }
}
