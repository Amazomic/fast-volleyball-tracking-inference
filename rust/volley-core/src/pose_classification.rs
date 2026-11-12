//! Conservative RTMPose Body17 volleyball technique refinement.
//! Pose evidence is only used after a motion-supported ball contact exists.
//! No serving or blocking verdict without verified rally/court context.
use crate::action_prediction::{ActionKind, ActionPrediction, PredictionMethod};

#[derive(Clone, Copy, Debug)]
pub struct PoseEvidence {
    /// COCO17 points [x_norm,y_norm,confidence]; input contains no athlete ID.
    pub keypoints: [[f64; 3]; 17],
    pub ball_x_norm: f64,
    pub ball_y_norm: f64,
    pub video_width: f64,
    pub video_height: f64,
    pub player_bbox_norm: [f64; 4],
}

fn pixel(e: &PoseEvidence, index: usize) -> Option<[f64; 2]> {
    let [x, y, confidence] = e.keypoints[index];
    if x.is_finite()
        && y.is_finite()
        && confidence.is_finite()
        && (0.0..=1.0).contains(&x)
        && (0.0..=1.0).contains(&y)
        && confidence >= 0.35
    {
        Some([x * e.video_width, y * e.video_height])
    } else {
        None
    }
}

fn distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// Use wrists, elbows and shoulders; when anything is ambiguous, keep
/// the older, source-backed contact geometry classification unchanged.
#[must_use]
pub fn refine(e: &PoseEvidence, geometry: &ActionPrediction) -> Option<ActionPrediction> {
    if !geometry.valid()
        || ![e.ball_x_norm, e.ball_y_norm, e.video_width, e.video_height]
            .iter()
            .all(|v| v.is_finite())
        || !(0.0..=1.0).contains(&e.ball_x_norm)
        || !(0.0..=1.0).contains(&e.ball_y_norm)
        || e.video_width < 64.0
        || e.video_height < 64.0
    {
        return None;
    }
    let [x1, y1, x2, y2] = e.player_bbox_norm;
    if ![x1, y1, x2, y2]
        .iter()
        .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
        || x1 >= x2
        || y1 >= y2
    {
        return None;
    }
    let height = (y2 - y1) * e.video_height;
    if height < 26.0 {
        return None;
    }
    let ball = [
        e.ball_x_norm * e.video_width,
        e.ball_y_norm * e.video_height,
    ];
    let shoulder_left = pixel(e, 5);
    let shoulder_right = pixel(e, 6);
    let pairs = [(5, 7, 9), (6, 8, 10)];
    let mut arms = Vec::with_capacity(2);
    for (shoulder, elbow, wrist) in pairs {
        let (Some(s), Some(el), Some(w)) = (pixel(e, shoulder), pixel(e, elbow), pixel(e, wrist))
        else {
            continue;
        };
        let proximity = distance(w, ball) / height;
        // A single wrist projected onto a passing ball is not physical evidence.
        if proximity > 0.30 || distance(w, el) / height > 0.85 {
            continue;
        }
        arms.push((s, el, w, proximity));
    }
    if arms.is_empty() {
        return None;
    }
    // Use the wrist with the strongest geometric proximity to the ball.
    arms.sort_by(|a, b| a.3.total_cmp(&b.3));
    let (_, elbow, wrist, nearest) = arms[0];
    let shoulders = [shoulder_left, shoulder_right]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    if shoulders.is_empty() {
        return None;
    }
    let shoulder_y = shoulders.iter().map(|p| p[1]).sum::<f64>() / shoulders.len() as f64;
    let high = wrist[1] < shoulder_y - height * 0.11;
    let above_head = wrist[1] < (y1 * e.video_height) + height * 0.12;
    let low = wrist[1] > shoulder_y + height * 0.18;
    let both_hands_near =
        arms.len() == 2 && arms[1].3 <= 0.30 && distance(arms[0].2, arms[1].2) / height < 0.35;
    let classification = if high {
        if both_hands_near {
            // Both hands near the ball above the shoulders: typical set.
            ActionKind::OverheadPass
        } else if above_head && elbow[1] > wrist[1] {
            // Elevated, extended one-arm strike, rather than a two-hand set.
            ActionKind::Attack
        } else {
            return None;
        }
    } else if low {
        if wrist[1] > (y1 + 0.93 * (y2 - y1)) * e.video_height {
            ActionKind::LowTouch
        } else {
            ActionKind::Dig
        }
    } else if both_hands_near {
        ActionKind::Dig
    } else {
        return None;
    };
    let wrist_confidence = if arms.len() == 2 {
        e.keypoints[9][2].min(e.keypoints[10][2])
    } else {
        e.keypoints[9][2].max(e.keypoints[10][2])
    };
    let prediction = ActionPrediction {
        kind: classification,
        // Evidence score is an uncalibrated support measure, never a probability.
        evidence_score: (geometry.evidence_score
            * wrist_confidence
            * (1.0 - nearest / 0.6).clamp(0.0, 1.0))
        .clamp(0.0, 1.0),
        method: PredictionMethod::PoseKeypointsV1,
        depth_ratio: geometry.depth_ratio,
        posture_ratio: geometry.posture_ratio,
    };
    prediction.valid().then_some(prediction)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn geom() -> ActionPrediction {
        ActionPrediction {
            kind: ActionKind::Attack,
            evidence_score: 0.85,
            method: PredictionMethod::ContactGeometryV1,
            depth_ratio: -0.1,
            posture_ratio: 0.4,
        }
    }
    fn data() -> PoseEvidence {
        let mut p = [[0.5, 0.5, 0.0]; 17];
        // A standing subject with extended arms near the overhead ball.
        p[5] = [0.43, 0.44, 0.95];
        p[6] = [0.57, 0.44, 0.95];
        p[7] = [0.41, 0.31, 0.96];
        p[8] = [0.59, 0.31, 0.96];
        p[9] = [0.49, 0.22, 0.96];
        p[10] = [0.52, 0.22, 0.96];
        PoseEvidence {
            keypoints: p,
            ball_x_norm: 0.5,
            ball_y_norm: 0.20,
            video_width: 640.0,
            video_height: 360.0,
            player_bbox_norm: [0.35, 0.17, 0.65, 0.90],
        }
    }
    #[test]
    fn distinguishes_two_hand_set_from_single_arm_attack() {
        let mut d = data();
        assert_eq!(refine(&d, &geom()).unwrap().kind, ActionKind::OverheadPass);
        d.keypoints[10][2] = 0.0;
        assert_eq!(refine(&d, &geom()).unwrap().kind, ActionKind::Attack);
    }
    #[test]
    fn missing_or_distant_wrist_never_overrides_geometry() {
        let mut d = data();
        d.keypoints[9][2] = 0.0;
        d.keypoints[10][2] = 0.0;
        assert!(refine(&d, &geom()).is_none());
        d = data();
        d.ball_x_norm = 0.97;
        assert!(refine(&d, &geom()).is_none());
    }
    #[test]
    fn low_wrist_infers_dig() {
        let mut d = data();
        d.keypoints[7] = [0.42, 0.52, 0.9];
        d.keypoints[9] = [0.47, 0.67, 0.9];
        d.keypoints[10][2] = 0.0;
        d.ball_x_norm = 0.47;
        d.ball_y_norm = 0.67;
        assert_eq!(refine(&d, &geom()).unwrap().kind, ActionKind::Dig);
    }
}
