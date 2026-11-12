pub mod action_prediction;
pub mod action_review;
pub mod continuity;
pub mod contracts;
pub mod court_geometry;
pub mod event_evidence;
pub mod evidence_source;
pub mod fusion;
pub mod gold;
pub mod identity_review;
pub mod motion;
pub mod rallies;
pub mod rally_reconstruction;
pub mod rally_selector;
pub mod reid;
pub mod statistics;
pub mod touch;
pub mod touch_pipeline;
pub mod tracker;

/// Must match the positive-area intersection-over-union operation used by
/// `scripts/volleyball_studio/replay_player_tracking.py` for valid boxes.
/// Input order: left, top, right, bottom (normalized frame coordinates).
#[inline]
pub fn bbox_iou(a: [f64; 4], b: [f64; 4]) -> f64 {
    let left = a[0].max(b[0]);
    let top = a[1].max(b[1]);
    let right = a[2].min(b[2]);
    let bottom = a[3].min(b[3]);
    let intersection = (right - left).max(0.0) * (bottom - top).max(0.0);
    let area_a = (a[2] - a[0]).max(0.0) * (a[3] - a[1]).max(0.0);
    let area_b = (b[2] - b[0]).max(0.0) * (b[3] - b[1]).max(0.0);
    intersection / (area_a + area_b - intersection).max(1e-9)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iou_identity_disjoint_and_touching_edges() {
        let a = [0.0, 0.0, 2.0, 2.0];
        assert_eq!(bbox_iou(a, a), 1.0);
        assert_eq!(bbox_iou(a, [3.0, 3.0, 4.0, 4.0]), 0.0);
        assert_eq!(bbox_iou(a, [2.0, 0.0, 4.0, 2.0]), 0.0);
    }

    #[test]
    fn iou_partial_overlap_and_degenerate_boxes() {
        let overlap = bbox_iou([0.0, 0.0, 2.0, 2.0], [1.0, 1.0, 3.0, 3.0]);
        assert!((overlap - 1.0 / 7.0).abs() < 1e-14);
        assert_eq!(bbox_iou([0.0, 0.0, 0.0, 2.0], [0.0, 0.0, 2.0, 2.0]), 0.0);
    }

    #[test]
    fn iou_normalized_player_boxes_is_symmetric() {
        let a = [0.125, 0.20, 0.30, 0.80];
        let b = [0.20, 0.25, 0.35, 0.75];
        let ab = bbox_iou(a, b);
        assert!(ab > 0.0 && ab < 1.0);
        assert_eq!(ab, bbox_iou(b, a));
    }
}

pub mod pose_classification;
