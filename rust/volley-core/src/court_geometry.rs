//! Four-point, court-plane projective geometry, ported from Python
//! court_transformer.py and player_contacts.py.
//! Input points are image-normalized; output is meters centered at the net.
//! The point order is the same as the Python homography: near end-line
//! sideline corner, opposite end-line on that sideline, then the remaining
//! two corners in clockwise/counter-clockwise perimeter order.

use serde::{Deserialize, Serialize};

pub const COURT_LENGTH_M: f64 = 18.0;
pub const COURT_WIDTH_M: f64 = 9.0;
pub const ON_COURT_LENGTH_MARGIN_M: f64 = 3.5;
pub const ON_COURT_WIDTH_MARGIN_M: f64 = 1.5;
pub const SERVE_ZONE_MARGIN_M: f64 = 1.2;
pub const BLOCK_NEAR_NET_M: f64 = 1.6;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CourtContactEvidence {
    pub x_m: f64,
    pub y_m: f64,
    /// Negative length-axis = near, positive = far, as in Python PlayerStore.
    pub side: CourtSide,
    pub near_net: bool,
    pub in_serve_zone: bool,
}
impl CourtContactEvidence {
    #[must_use]
    pub fn valid(self) -> bool {
        self.x_m.is_finite()
            && self.y_m.is_finite()
            && self.x_m.abs() <= COURT_LENGTH_M / 2.0 + ON_COURT_LENGTH_MARGIN_M + 1e-6
            && self.y_m.abs() <= COURT_WIDTH_M / 2.0 + ON_COURT_WIDTH_MARGIN_M + 1e-6
            // Side is median-stabilized across each provisional track.
            && self.near_net == (self.x_m.abs() <= BLOCK_NEAR_NET_M)
            && self.in_serve_zone
                == (self.x_m.abs() >= COURT_LENGTH_M / 2.0 - SERVE_ZONE_MARGIN_M)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CourtSide {
    Near,
    Far,
}

#[derive(Clone, Debug)]
pub struct CourtTransform {
    matrix: [f64; 9],
}

impl CourtTransform {
    /// Map four ordered normalized pixels to the 18 x 9 meter playing plane.
    /// The same four targets are used by Python CourtTransformer.
    ///
    /// Reject self-intersections, collinear corners, badly conditioned
    /// quadrilaterals and non-finite inputs rather than using a fake identity
    /// transform. Coordinates outside the image are allowed to +/-0.5.
    pub fn new(corners: [[f64; 2]; 4]) -> Result<Self, String> {
        if corners
            .iter()
            .flatten()
            .any(|v| !v.is_finite() || !(-0.5..=1.5).contains(v))
        {
            return Err("Court points must be finite and within [-0.5, 1.5]".into());
        }
        let cross = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| {
            (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0])
        };
        let turns = std::array::from_fn::<_, 4, _>(|i| {
            cross(corners[i], corners[(i + 1) % 4], corners[(i + 2) % 4])
        });
        if turns.iter().any(|t| t.abs() < 1e-5)
            || turns.iter().any(|t| t.signum() != turns[0].signum())
        {
            return Err("Court corners must form an ordered convex quadrilateral".into());
        }
        let area = (0..4)
            .map(|i| {
                let a = corners[i];
                let b = corners[(i + 1) % 4];
                a[0] * b[1] - b[0] * a[1]
            })
            .sum::<f64>()
            .abs()
            * 0.5;
        if area < 1e-3 {
            return Err("Court area is too small for reliable calibration".into());
        }
        let targets = [
            [-COURT_LENGTH_M / 2.0, -COURT_WIDTH_M / 2.0],
            [COURT_LENGTH_M / 2.0, -COURT_WIDTH_M / 2.0],
            [COURT_LENGTH_M / 2.0, COURT_WIDTH_M / 2.0],
            [-COURT_LENGTH_M / 2.0, COURT_WIDTH_M / 2.0],
        ];
        let mut rows = [[0.0; 9]; 8];
        for (i, (&[x, y], &[cx, cy])) in corners.iter().zip(targets.iter()).enumerate() {
            rows[2 * i] = [x, y, 1.0, 0.0, 0.0, 0.0, -x * cx, -y * cx, cx];
            rows[2 * i + 1] = [0.0, 0.0, 0.0, x, y, 1.0, -x * cy, -y * cy, cy];
        }
        // Partial-pivot Gaussian elimination of the 8-DOF homography.
        for pivot in 0..8 {
            let best = (pivot..8)
                .max_by(|&a, &b| rows[a][pivot].abs().total_cmp(&rows[b][pivot].abs()))
                .ok_or("Court homography has no pivot")?;
            if rows[best][pivot].abs() < 1e-10 {
                return Err("Court homography is singular".into());
            }
            rows.swap(pivot, best);
            let scale = rows[pivot][pivot];
            for value in &mut rows[pivot][pivot..] {
                *value /= scale;
            }
            // Copy the pivot row: simultaneous mutable/immutable borrows
            // of different rows are unnecessary for only eight equations.
            let normalized = rows[pivot];
            for (other, row) in rows.iter_mut().enumerate() {
                if other == pivot {
                    continue;
                }
                let factor = row[pivot];
                for (target, source) in row[pivot..].iter_mut().zip(normalized[pivot..].iter()) {
                    *target -= factor * source;
                }
            }
        }
        let matrix = [
            rows[0][8], rows[1][8], rows[2][8], rows[3][8], rows[4][8], rows[5][8], rows[6][8],
            rows[7][8], 1.0,
        ];
        let transform = Self { matrix };
        for (point, target) in corners.iter().zip(targets) {
            let got = transform
                .to_court(*point)
                .ok_or("Invalid court homography")?;
            if (got[0] - target[0]).abs() > 1e-5 || (got[1] - target[1]).abs() > 1e-5 {
                return Err("Court homography does not reproduce its calibration".into());
            }
        }
        Ok(transform)
    }

    /// Project a normalized image point onto the ground plane.
    #[must_use]
    pub fn to_court(&self, [x, y]: [f64; 2]) -> Option<[f64; 2]> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let h = &self.matrix;
        let w = h[6] * x + h[7] * y + h[8];
        if !w.is_finite() || w.abs() < 1e-10 {
            return None;
        }
        let point = [
            (h[0] * x + h[1] * y + h[2]) / w,
            (h[3] * x + h[4] * y + h[5]) / w,
        ];
        point.iter().all(|v| v.is_finite()).then_some(point)
    }

    /// The Python on-court corridor includes substitutes slightly beyond the
    /// lines, but excludes spectators and players far outside the game.
    #[must_use]
    pub fn evidence_for_feet(&self, feet_norm: [f64; 2]) -> Option<CourtContactEvidence> {
        let [x_m, y_m] = self.to_court(feet_norm)?;
        let evidence = CourtContactEvidence {
            x_m,
            y_m,
            side: if x_m < 0.0 {
                CourtSide::Near
            } else {
                CourtSide::Far
            },
            near_net: x_m.abs() <= BLOCK_NEAR_NET_M,
            in_serve_zone: x_m.abs() >= COURT_LENGTH_M / 2.0 - SERVE_ZONE_MARGIN_M,
        };
        evidence.valid().then_some(evidence)
    }

    /// Player foot anchor, not box center; jumps may shift the instantaneous
    /// estimate, so callers should use track context for side stability.
    #[must_use]
    pub fn player_evidence(&self, bbox: [f64; 4]) -> Option<CourtContactEvidence> {
        self.evidence_for_feet([(bbox[0] + bbox[2]) * 0.5, bbox[3]])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUAD: [[f64; 2]; 4] = [[0.1, 0.2], [0.9, 0.2], [0.9, 0.8], [0.1, 0.8]];

    #[test]
    fn python_court_transform_matches_corners_and_net() {
        let h = CourtTransform::new(QUAD).unwrap();
        let centre = h.to_court([0.5, 0.5]).unwrap();
        assert!(centre[0].abs() < 1e-7 && centre[1].abs() < 1e-7);
        for (pixel, expected) in
            QUAD.into_iter()
                .zip([[-9.0, -4.5], [9.0, -4.5], [9.0, 4.5], [-9.0, 4.5]])
        {
            let found = h.to_court(pixel).unwrap();
            assert!((found[0] - expected[0]).abs() < 1e-7);
            assert!((found[1] - expected[1]).abs() < 1e-7);
        }
    }

    #[test]
    fn perspective_calibration_remains_stable_off_image() {
        let h = CourtTransform::new([[-0.15, -0.1], [0.93, 0.14], [0.81, 0.88], [-0.33, 1.28]])
            .unwrap();
        assert!(h.to_court([0.5, 0.5]).is_some());
    }

    #[test]
    fn court_side_block_and_serve_zones_match_python_thresholds() {
        let h = CourtTransform::new(QUAD).unwrap();
        let center = h.evidence_for_feet([0.5, 0.5]).unwrap();
        assert_eq!(center.side, CourtSide::Far);
        assert!(center.near_net);
        assert!(!center.in_serve_zone);
        let server = h.evidence_for_feet([0.125, 0.5]).unwrap();
        assert_eq!(server.side, CourtSide::Near);
        assert!(server.in_serve_zone);
        assert!(!server.near_net);
        assert!(h.evidence_for_feet([0.98, 0.5]).is_some()); // 1.8 m outside end line
        assert!(h.evidence_for_feet([1.3, 0.5]).is_none());
    }

    #[test]
    fn invalid_geometry_never_silently_falls_back() {
        assert!(CourtTransform::new([[0.1, 0.1]; 4]).is_err());
        assert!(CourtTransform::new([[0.1, 0.1], [0.9, 0.9], [0.1, 0.9], [0.9, 0.1]]).is_err());
        assert!(CourtTransform::new([[0.1, 0.1], [0.9, 0.1], [0.1, 0.9], [0.9, 0.9]]).is_err());
        assert!(
            CourtTransform::new([[f64::NAN, 0.2], [0.9, 0.1], [0.9, 0.9], [0.1, 0.9],]).is_err()
        );
    }
}
