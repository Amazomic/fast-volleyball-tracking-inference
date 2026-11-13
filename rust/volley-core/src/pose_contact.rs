//! Geometric coincidence of a changing ball trajectory with a single
//! source-bound player's COCO17 pose. Only positive, temporal evidence counts.
//! This is a 2D contact hypothesis, not depth-aware proof of a physical hit.

/// One timestamp of a ball trajectory and the *same tracked player's* pose.
#[derive(Clone, Copy, Debug)]
pub struct ContactSample {
    pub ball_px: [f64; 2],
    pub keypoints: [[f64; 3]; 17],
}

/// Three source timestamps bracketing a ball-motion discontinuity.
///
/// The caller must separately verify a velocity/heading change in the same
/// immutable ball trajectory and continuity of the selected Track ID.
#[derive(Clone, Copy, Debug)]
pub struct ContactTrajectory {
    pub before: ContactSample,
    pub impact: ContactSample,
    pub after: ContactSample,
    pub video_width: f64,
    pub video_height: f64,
    pub player_height_px: f64,
    pub ball_radius_px: f64,
    pub min_confidence: f64,
    pub max_distance_ratio: f64,
}

fn pixel(sample: &ContactSample, index: usize, value: &ContactTrajectory) -> Option<[f64; 2]> {
    let [x, y, confidence] = sample.keypoints[index];
    if !x.is_finite()
        || !y.is_finite()
        || !confidence.is_finite()
        || !(0.0..=1.0).contains(&x)
        || !(0.0..=1.0).contains(&y)
        || !(value.min_confidence..=1.0).contains(&confidence)
    {
        return None;
    }
    Some([x * value.video_width, y * value.video_height])
}

fn distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn to_segment(point: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let vx = b[0] - a[0];
    let vy = b[1] - a[1];
    let squared = vx * vx + vy * vy;
    if squared <= 1e-9 {
        return distance(point, a);
    }
    let projection = ((point[0] - a[0]) * vx + (point[1] - a[1]) * vy) / squared;
    let t = projection.clamp(0.0, 1.0);
    distance(point, [a[0] + t * vx, a[1] + t * vy])
}

fn joint_distance(index: usize, evidence: &ContactTrajectory) -> Option<[f64; 3]> {
    Some([
        distance(
            evidence.before.ball_px,
            pixel(&evidence.before, index, evidence)?,
        ),
        distance(
            evidence.impact.ball_px,
            pixel(&evidence.impact, index, evidence)?,
        ),
        distance(
            evidence.after.ball_px,
            pixel(&evidence.after, index, evidence)?,
        ),
    ])
}

fn limb_distance(a: usize, b: usize, evidence: &ContactTrajectory) -> Option<[f64; 3]> {
    let distance_at = |sample: &ContactSample| {
        Some(to_segment(
            sample.ball_px,
            pixel(sample, a, evidence)?,
            pixel(sample, b, evidence)?,
        ))
    };
    Some([
        distance_at(&evidence.before)?,
        distance_at(&evidence.impact)?,
        distance_at(&evidence.after)?,
    ])
}

/// Find a body point/limb that the ball approached, met, and separated from.
///
/// A ball near a pose in one frame, a straight fly-by, or a velocity change
/// remote from the player must not count. Both hands, arms, torso, head, legs
/// and feet are eligible (technique classification belongs to a later stage).
/// Returns normalized minimum separation, not a probability.
#[must_use]
pub fn trajectory_body_intersection(evidence: &ContactTrajectory) -> Option<f64> {
    if ![
        evidence.video_width,
        evidence.video_height,
        evidence.player_height_px,
        evidence.ball_radius_px,
        evidence.min_confidence,
        evidence.max_distance_ratio,
    ]
    .iter()
    .all(|value| value.is_finite())
        || evidence.video_width < 64.0
        || evidence.video_height < 64.0
        || evidence.player_height_px < 26.0
        || evidence.ball_radius_px <= 0.0
        || evidence.ball_radius_px > evidence.player_height_px * 0.2
        || !(0.0..=1.0).contains(&evidence.min_confidence)
        || !(0.01..=1.0).contains(&evidence.max_distance_ratio)
        || [evidence.before, evidence.impact, evidence.after]
            .iter()
            .any(|sample| {
                !sample.ball_px[0].is_finite()
                    || !sample.ball_px[1].is_finite()
                    || sample.ball_px[0] < 0.0
                    || sample.ball_px[1] < 0.0
                    || sample.ball_px[0] > evidence.video_width
                    || sample.ball_px[1] > evidence.video_height
            })
    {
        return None;
    }
    // A ball is not a dimensionless point. Allow a small landmark error but
    // never the broad reach ellipse formerly used by GPU proximity overlays.
    let contact_limit =
        evidence.ball_radius_px + evidence.player_height_px * evidence.max_distance_ratio.min(0.16);
    let approach_margin = (evidence.player_height_px * 0.012).max(1.5);
    let mut closest: Option<f64> = None;
    let mut consider = |distances: Option<[f64; 3]>| {
        let Some([before, contact, after]) = distances else {
            return;
        };
        if contact <= contact_limit
            && before > contact + approach_margin
            && after > contact + approach_margin
        {
            let normalized =
                (contact - evidence.ball_radius_px).max(0.0) / evidence.player_height_px;
            if closest.is_none_or(|old| normalized < old) {
                closest = Some(normalized);
            }
        }
    };
    for joint in 0..17 {
        consider(joint_distance(joint, evidence));
    }
    // COCO17 skeletal neighbors; a ball may strike a forearm, not only the
    // exact wrist coordinate. Each segment follows the SAME body part in time.
    const LIMBS: [(usize, usize); 16] = [
        (5, 7),
        (7, 9),
        (6, 8),
        (8, 10), // arms
        (5, 6),
        (5, 11),
        (6, 12),
        (11, 12), // torso
        (11, 13),
        (13, 15),
        (12, 14),
        (14, 16), // legs
        (0, 1),
        (0, 2),
        (1, 3),
        (2, 4), // face/head
    ];
    for (a, b) in LIMBS {
        consider(limb_distance(a, b, evidence));
    }
    closest
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(ball_x: f64, hand_x: f64) -> ContactSample {
        let mut keypoints = [[0.0; 3]; 17];
        keypoints[9] = [hand_x, 0.4, 0.95];
        ContactSample {
            ball_px: [ball_x, 144.0],
            keypoints,
        }
    }
    fn evidence() -> ContactTrajectory {
        ContactTrajectory {
            before: sample(200.0, 320.0 / 640.0),
            impact: sample(320.0, 320.0 / 640.0),
            after: sample(215.0, 320.0 / 640.0),
            video_width: 640.0,
            video_height: 360.0,
            player_height_px: 150.0,
            ball_radius_px: 6.0,
            min_confidence: 0.35,
            max_distance_ratio: 0.30,
        }
    }
    #[test]
    fn rebound_off_still_hand_is_contact() {
        assert_eq!(trajectory_body_intersection(&evidence()), Some(0.0));
    }
    #[test]
    fn position_overlap_without_approach_and_departure_is_not_contact() {
        let mut e = evidence();
        e.before.ball_px[0] = 320.0;
        assert!(trajectory_body_intersection(&e).is_none());
    }
    #[test]
    fn ball_changes_direction_away_from_body_not_contact() {
        let mut e = evidence();
        for sample in [&mut e.before, &mut e.impact, &mut e.after] {
            sample.keypoints[9][0] = 0.90;
        }
        assert!(trajectory_body_intersection(&e).is_none());
    }
    #[test]
    fn low_confidence_landmark_is_not_contact() {
        let mut e = evidence();
        e.before.keypoints[9][2] = 0.1;
        assert!(trajectory_body_intersection(&e).is_none());
    }
    #[test]
    fn forearm_contact_does_not_require_exact_joint_overlap() {
        let mut e = evidence();
        for sample in [&mut e.before, &mut e.impact, &mut e.after] {
            sample.keypoints[7] = [0.5, 0.25, 0.92];
            sample.keypoints[9] = [0.5, 0.55, 0.92];
        }
        e.impact.ball_px = [325.0, 144.0];
        assert!(trajectory_body_intersection(&e).is_some());
    }
    #[test]
    fn invalid_or_oversize_ball_not_a_touch() {
        let mut e = evidence();
        e.ball_radius_px = f64::NAN;
        assert!(trajectory_body_intersection(&e).is_none());
        e.ball_radius_px = 60.0;
        assert!(trajectory_body_intersection(&e).is_none());
    }
}
