//! Independent geometric touch proposals: ball-trajectory breaks near an observed player.
//! Unlike RAVEL Touch peaks, this path can recover contacts without neural Touch logits.
//! It is deliberately conservative: a ball flight near a person alone is not a Touch.

use super::{pixel_bbox, standing_height, TouchFrame};
use crate::motion::{ball_motion_change, BallObservation, MotionEvidence, TOUCH_MOTION_CONFIG};

#[derive(Clone, Copy, Debug)]
pub(super) struct GeometryContact {
    pub frame: usize,
    pub track_id: u64,
    pub probability: f64,
    pub motion: MotionEvidence,
    pub ball_x: f64,
    pub ball_y: f64,
}

/// Do not interpret a switch from RAVEL to VballNet (or back) as an impact.
fn stable_ball_source(frames: &[TouchFrame], index: usize) -> bool {
    if index < 5 || index + 5 >= frames.len() {
        return false;
    }
    let source = match frames[index].selected_ball.as_ref() {
        Some(ball) => ball.source_model.as_str(),
        None => return false,
    };
    let mut left = 0;
    let mut right = 0;
    for (relative, frame) in frames[index - 5..=index + 5].iter().enumerate() {
        if let Some(ball) = frame.selected_ball.as_ref() {
            if ball.source_model != source {
                return false;
            }
            if relative < 5 {
                left += 1;
            } else if relative > 5 {
                right += 1;
            }
        }
    }
    left >= 3 && right >= 3
}

/// A single wrong ball localization can create two huge fictitious velocity changes.
fn single_frame_outlier(frames: &[TouchFrame], index: usize, width: f64, height: f64) -> bool {
    if index < 2 || index + 2 >= frames.len() {
        return false;
    }
    let center = |i: usize| -> Option<(f64, f64)> {
        let box_ = frames[i].selected_ball.as_ref()?.bbox_xyxy_norm;
        Some((
            (box_[0] + box_[2]) * width * 0.5,
            (box_[1] + box_[3]) * height * 0.5,
        ))
    };
    let Some((ax, ay)) = center(index - 2) else {
        return false;
    };
    let Some((bx, by)) = center(index - 1) else {
        return false;
    };
    let Some((cx, cy)) = center(index) else {
        return false;
    };
    let Some((dx, dy)) = center(index + 1) else {
        return false;
    };
    let Some((ex, ey)) = center(index + 2) else {
        return false;
    };
    let normal_step = (ax - bx).hypot(ay - by).max((dx - ex).hypot(dy - ey));
    let expected = (bx + dx) * 0.5;
    let expected_y = (by + dy) * 0.5;
    (cx - expected).hypot(cy - expected_y) > 16.0_f64.max(4.0 * normal_step)
}

/// The motion fitter sees up to five observations on either side of an impact.
/// A single bad localization must not become a nearby RAVEL or geometric Touch.
pub(super) fn motion_window_has_outlier(
    frames: &[TouchFrame],
    index: usize,
    width: f64,
    height: f64,
) -> bool {
    (index.saturating_sub(6)..=(index + 6).min(frames.len() - 1))
        .any(|neighbor| single_frame_outlier(frames, neighbor, width, height))
}

/// Physical footprint of one observed ball, in a single CPU source frame.
#[derive(Clone, Copy, Debug)]
struct ReachEvidence {
    width: f64,
    height: f64,
    ball_x: f64,
    ball_y: f64,
    ball_radius: f64,
    max_reach_ratio: f64,
}

/// Returns the most plausible SAME-FRAME player; no invented or interpolated Track IDs.
fn touching_player(
    frames: &[TouchFrame],
    index: usize,
    reach_evidence: ReachEvidence,
) -> Option<(u64, f64, f64)> {
    let ReachEvidence {
        width,
        height,
        ball_x,
        ball_y,
        ball_radius,
        max_reach_ratio,
    } = reach_evidence;
    let mut best: Option<(u64, f64, f64)> = None;
    for player in &frames[index].players {
        if player.score < 0.25 {
            continue;
        }
        let [x1, y1, x2, y2] = pixel_bbox(player.bbox_xyxy_norm, width, height);
        let bbox_height = y2 - y1;
        if bbox_height < 14.0 || x2 - x1 < 6.0 {
            continue;
        }
        let Some(standing) = standing_height(frames, index, player.track_id, height) else {
            continue;
        };
        let standing = standing.max(bbox_height);
        let reach_x = ((x2 - x1) * 0.70).max(standing * 0.42) + ball_radius;
        let reach_top = y2 - standing * 1.45 - ball_radius;
        let reach_bottom = y2 + standing * 0.15 + ball_radius;
        let reach_mid = (reach_top + reach_bottom) * 0.5;
        let reach_y = (reach_bottom - reach_top) * 0.5;
        let reach =
            ((ball_x - (x1 + x2) * 0.5) / reach_x).hypot((ball_y - reach_mid) / reach_y.max(1.0));
        // A geometry-only proposal must be closer than the broader RAVEL gate.
        let reach_limit = (0.55 + max_reach_ratio * 0.65).clamp(0.58, 1.0);
        if reach > reach_limit {
            continue;
        }
        let body_x = (ball_x - (x1 + x2) * 0.5).abs() - (x2 - x1) * 0.5;
        let body_y = (y1.min(y2 - standing) - ball_y).max(ball_y - y2);
        let body = body_x.max(0.0).hypot(body_y.max(0.0)) / standing;
        let ratio = ball_radius / standing;
        if ratio <= 0.0 {
            continue;
        }
        let mismatch = if ratio < 0.05675 * 0.75 {
            (ratio / (0.05675 * 0.75)).log2().abs()
        } else if ratio > 0.05675 * 1.25 {
            (ratio / (0.05675 * 1.25)).log2().abs()
        } else {
            0.0
        };
        let cost = body + 0.25 * reach + 1.25 * mismatch;
        if best.is_none_or(|(_, old_cost, _)| cost < old_cost) {
            best = Some((player.track_id, cost, player.score));
        }
    }
    best
}

/// Scan the bounded chunk. Unobserved ball/player frames never become proposals.
pub(super) fn detect(
    frames: &[TouchFrame],
    balls: &[BallObservation],
    width: f64,
    height: f64,
    max_reach_ratio: f64,
) -> Vec<GeometryContact> {
    if balls.len() < 7 {
        return Vec::new();
    }
    let mut result = Vec::new();
    for (index, frame) in frames.iter().enumerate() {
        // A one-frame outlier also contaminates neighboring velocity estimates.
        if !stable_ball_source(frames, index)
            || motion_window_has_outlier(frames, index, width, height)
        {
            continue;
        }
        let Some(ball) = frame.selected_ball.as_ref() else {
            continue;
        };
        let Some(motion) = ball_motion_change(balls, index as i64, 0, TOUCH_MOTION_CONFIG) else {
            continue;
        };
        let speed = motion.speed_before.max(motion.speed_after);
        // More stringent than the RAVEL-supported path to limit false contacts.
        if speed < 2.0
            || motion.residual < 8.0_f64.max(0.45 * speed)
            || (motion.angle < 35.0 && (motion.speed_after - motion.speed_before).abs() < 6.0)
        {
            continue;
        }
        let box_ = ball.bbox_xyxy_norm;
        let radius = ((box_[2] - box_[0]) * width + (box_[3] - box_[1]) * height) * 0.25;
        let x = (box_[0] + box_[2]) * 0.5 * width;
        let y = (box_[1] + box_[3]) * 0.5 * height;
        let Some((track_id, cost, player_score)) = touching_player(
            frames,
            index,
            ReachEvidence {
                width,
                height,
                ball_x: x,
                ball_y: y,
                ball_radius: radius,
                max_reach_ratio,
            },
        ) else {
            continue;
        };
        // An evidence score, NOT a calibrated P(touch) or P(action-kind).
        let probability = (0.30
            + 0.23 * motion.residual / (motion.residual + 12.0)
            + 0.16 * (1.0 - cost.min(1.0))
            + 0.11 * ball.score
            + 0.10 * player_score)
            .clamp(0.0, 0.85);
        result.push(GeometryContact {
            frame: index,
            track_id,
            probability,
            motion,
            ball_x: x,
            ball_y: y,
        });
    }
    result
}
