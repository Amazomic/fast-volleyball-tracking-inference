//! Unverified volleyball technique hypotheses from ball/player geometry.
//!
//! Ports the depth/posture boundaries in Python `player_contacts.describe_technique`.
//! A hypothesis must never be confused with a manually reviewed game action.

use serde::{Deserialize, Serialize};

const OVERHEAD_TOP_DEPTH: f64 = -0.05;
const OVERHEAD_DEPTH_LIMIT: f64 = 0.25;
const DIG_DEPTH_LIMIT: f64 = 0.95;
const WIDE_POSTURE_RATIO: f64 = 0.65;
const WIDE_POSTURE_OVERHEAD_LIMIT: f64 = 0.0;
const BALL_RADIUS_CM: f64 = 10.5;
const PLAYER_HEIGHT_CM: f64 = 185.0;
const MIN_PLAYER_HEIGHT_PX: f64 = 14.0;
const MIN_PLAYER_WIDTH_PX: f64 = 6.0;

/// The four techniques distinguishable without verified court or rally context.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    Attack,
    OverheadPass,
    Dig,
    LowTouch,
    Block,
    Serve,
}

impl ActionKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Attack => "attack",
            Self::OverheadPass => "overhead_pass",
            Self::Dig => "dig",
            Self::LowTouch => "low_touch",
            Self::Block => "block",
            Self::Serve => "serve",
        }
    }
}

/// Stronger labels require independently verified court/rally evidence.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CourtContext {
    #[default]
    Unavailable,
    NearNet,
    ServeAtRallyStart,
}

/// A stable audit trail for an automatic (not human-verified) hypothesis.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActionPrediction {
    pub kind: ActionKind,
    /// Combined touch/track evidence score, NOT calibrated technique probability.
    pub evidence_score: f64,
    pub method: PredictionMethod,
    pub depth_ratio: f64,
    pub posture_ratio: f64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PredictionMethod {
    ContactGeometryV1,
    PoseKeypointsV1,
}

impl ActionPrediction {
    #[must_use]
    pub fn valid(&self) -> bool {
        self.evidence_score.is_finite()
            && (0.0..=1.0).contains(&self.evidence_score)
            && self.depth_ratio.is_finite()
            && (-2.0..=3.0).contains(&self.depth_ratio)
            && self.posture_ratio.is_finite()
            && (0.0..=4.0).contains(&self.posture_ratio)
            && matches!(self.method, PredictionMethod::ContactGeometryV1 | PredictionMethod::PoseKeypointsV1)
    }
}

/// The inputs already available from a same-frame, motion-supported Touch.
/// Caller must only supply a non-default context from verified court/rally data.
#[derive(Clone, Copy, Debug)]
pub struct PositionEvidence {
    pub ball_y_px: f64,
    pub ball_radius_px: f64,
    pub player_bbox_px: [f64; 4],
    pub standing_height_px: f64,
    pub touch_score: f64,
    pub track_score: f64,
    pub context: CourtContext,
}

/// Restore the Python technique boundaries without claiming a verified action.
///
/// # Returns
///
/// `None` means no reliable source player or invalid geometry: remain unknown.
#[must_use]
pub fn predict(evidence: PositionEvidence) -> Option<ActionPrediction> {
    let [x1, y1, x2, y2] = evidence.player_bbox_px;
    let numbers = [
        x1,
        y1,
        x2,
        y2,
        evidence.ball_y_px,
        evidence.ball_radius_px,
        evidence.standing_height_px,
        evidence.touch_score,
        evidence.track_score,
    ];
    if numbers.iter().any(|value| !value.is_finite())
        || x2 - x1 < MIN_PLAYER_WIDTH_PX
        || y2 - y1 < MIN_PLAYER_HEIGHT_PX
        || evidence.ball_radius_px <= 0.0
        || evidence.standing_height_px <= 0.0
        || !(0.0..=1.0).contains(&evidence.touch_score)
        || !(0.0..=1.0).contains(&evidence.track_score)
    {
        return None;
    }
    // The original depth estimator uses ball radius as a perspective ruler.
    // Standing height is still measured to audit the perspective mismatch.
    let depth_height = (evidence.ball_radius_px * PLAYER_HEIGHT_CM / BALL_RADIUS_CM).max(1.0);
    let depth = (evidence.ball_y_px - (y2 - depth_height)) / depth_height;
    let posture = (x2 - x1) / (y2 - y1);
    let kind = if evidence.context == CourtContext::ServeAtRallyStart {
        ActionKind::Serve
    } else if depth < OVERHEAD_TOP_DEPTH {
        if evidence.context == CourtContext::NearNet {
            ActionKind::Block
        } else {
            ActionKind::Attack
        }
    } else {
        let overhead_limit = if posture >= WIDE_POSTURE_RATIO {
            WIDE_POSTURE_OVERHEAD_LIMIT
        } else {
            OVERHEAD_DEPTH_LIMIT
        };
        if depth < overhead_limit {
            ActionKind::OverheadPass
        } else if depth <= DIG_DEPTH_LIMIT {
            ActionKind::Dig
        } else {
            ActionKind::LowTouch
        }
    };
    let prediction = ActionPrediction {
        kind,
        evidence_score: evidence.touch_score.min(evidence.track_score),
        method: PredictionMethod::ContactGeometryV1,
        depth_ratio: (depth * 1000.0).round() / 1000.0,
        posture_ratio: (posture * 1000.0).round() / 1000.0,
    };
    prediction.valid().then_some(prediction)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(ball_y_px: f64) -> PositionEvidence {
        PositionEvidence {
            ball_y_px,
            ball_radius_px: 10.5 * 180.0 / 185.0,
            player_bbox_px: [100.0, 100.0, 140.0, 280.0],
            standing_height_px: 180.0,
            touch_score: 0.93,
            track_score: 0.88,
            context: CourtContext::Unavailable,
        }
    }

    #[test]
    fn python_describe_technique_attack_overhead_dig_low() {
        assert_eq!(
            predict(sample(80.0)).expect("above head").kind,
            ActionKind::Attack
        );
        assert_eq!(
            predict(sample(105.0)).expect("head level").kind,
            ActionKind::OverheadPass
        );
        assert_eq!(
            predict(sample(180.0)).expect("chest level").kind,
            ActionKind::Dig
        );
        assert_eq!(
            predict(sample(285.0)).expect("below knees").kind,
            ActionKind::LowTouch
        );
    }

    #[test]
    fn python_wide_posture_changes_head_level_pass_to_dig() {
        let mut evidence = sample(120.0);
        assert_eq!(
            predict(evidence).expect("upright").kind,
            ActionKind::OverheadPass
        );
        evidence.player_bbox_px[2] = 250.0;
        assert_eq!(predict(evidence).expect("wide").kind, ActionKind::Dig);
    }

    #[test]
    fn serve_and_block_require_verified_context() {
        let mut evidence = sample(80.0);
        assert_eq!(
            predict(evidence).expect("no context").kind,
            ActionKind::Attack
        );
        evidence.context = CourtContext::NearNet;
        assert_eq!(predict(evidence).expect("court").kind, ActionKind::Block);
        evidence.context = CourtContext::ServeAtRallyStart;
        assert_eq!(predict(evidence).expect("serve").kind, ActionKind::Serve);
    }

    #[test]
    fn invalid_or_unattached_geometry_is_not_an_action() {
        let mut evidence = sample(105.0);
        evidence.player_bbox_px[3] = 103.0;
        assert!(predict(evidence).is_none());
        evidence = sample(105.0);
        evidence.ball_radius_px = f64::NAN;
        assert!(predict(evidence).is_none());
        evidence = sample(105.0);
        evidence.touch_score = 1.01;
        assert!(predict(evidence).is_none());
    }

    #[test]
    fn prediction_is_explicitly_unverified_and_json_roundtrips() {
        let prediction = predict(sample(80.0)).expect("position evidence");
        assert_eq!(prediction.evidence_score, 0.88);
        let value = serde_json::to_value(&prediction).expect("serialize");
        assert_eq!(value["kind"], "attack");
        assert_eq!(value["method"], "contact_geometry_v1");
        assert_eq!(
            serde_json::from_value::<ActionPrediction>(value).expect("deserialize"),
            prediction
        );
    }
}
