//! Bounded VBallNet-only reconstruction of invisible portions of a flight.
//! Every inferred point is marked synthetic and is NEVER Touch evidence.

/// Maximum interior dropout (0.9 s at 30 fps); never invent an unanchored tail.
pub const MAX_MISSING_BALL_FRAMES: usize = 27;
const MAX_DISPLACEMENT_PER_FRAME: f64 = 0.12;
const MAX_ENDPOINT_VELOCITY_MISMATCH: f64 = 0.018;
const MAX_HORIZONTAL_ACCEL: f64 = 0.006;
const MAX_VERTICAL_ACCEL: f64 = 0.010;

/// Fit one continuous flight between real observations. The left/right
/// velocity estimates use only other *real* VBallNet observations.
/// Two-sided velocity agreement, nonnegative downward gravity and an entirely
/// in-frame curve are required when enough evidence exists. Missing endpoint
/// velocities fall back to a straight chord, never to RAVEL or extrapolation
/// beyond the last real observation.
pub fn interpolate_short_gaps(points: &mut [Option<[f64; 2]>]) -> Vec<bool> {
    let mut synthetic = vec![false; points.len()];
    let mut previous: Option<usize> = None;
    for right in 0..points.len() {
        let Some(end) = points[right].filter(|p| valid_point(*p)) else {
            if points[right].is_some() {
                previous = None;
            }
            continue;
        };
        if let Some(left) = previous {
            let missing = right - left - 1;
            if (1..=MAX_MISSING_BALL_FRAMES).contains(&missing) {
                let begin = points[left].expect("observed left anchor");
                let duration = (right - left) as f64;
                let delta = [end[0] - begin[0], end[1] - begin[1]];
                if delta[0].hypot(delta[1]) <= MAX_DISPLACEMENT_PER_FRAME * duration {
                    // Velocity at each edge is measured outside the gap only.
                    let left_velocity = left
                        .checked_sub(1)
                        .filter(|&i| !synthetic[i])
                        .and_then(|i| points[i].filter(|p| valid_point(*p)))
                        .map(|p| [begin[0] - p[0], begin[1] - p[1]]);
                    let right_velocity = (right + 1 < points.len())
                        .then_some(right + 1)
                        .and_then(|i| points[i].filter(|p| valid_point(*p)))
                        .map(|p| [p[0] - end[0], p[1] - end[1]]);
                    // Fit x(t), y(t) = P_left + v_left*t + 0.5*a*t^2,
                    // exactly constrained by the other observed endpoint.
                    let v_left =
                        left_velocity.unwrap_or([delta[0] / duration, delta[1] / duration]);
                    let acceleration = [
                        2.0 * (delta[0] - v_left[0] * duration) / duration.powi(2),
                        2.0 * (delta[1] - v_left[1] * duration) / duration.powi(2),
                    ];
                    // Long gaps need observed velocities from *both* sides; a
                    // chord alone does not determine the flight parabola.
                    let enough_anchors = missing <= 12
                        || (left_velocity.is_some() && right_velocity.is_some());
                    let compatible = enough_anchors
                        && acceleration[0].abs() <= MAX_HORIZONTAL_ACCEL
                        && acceleration[1] >= -0.0015
                        && acceleration[1] <= MAX_VERTICAL_ACCEL
                        && right_velocity.is_none_or(|v| {
                            (v[0] - v_left[0] - acceleration[0] * duration)
                                .hypot(v[1] - v_left[1] - acceleration[1] * duration)
                                <= MAX_ENDPOINT_VELOCITY_MISMATCH
                        });
                    if compatible {
                        let estimates: Vec<_> = (1..=missing)
                            .map(|offset| {
                                let t = offset as f64;
                                [
                                    begin[0] + v_left[0] * t + 0.5 * acceleration[0] * t * t,
                                    begin[1] + v_left[1] * t + 0.5 * acceleration[1] * t * t,
                                ]
                            })
                            .collect();
                        if estimates.iter().all(|p| valid_point(*p)) {
                            for (offset, point) in estimates.into_iter().enumerate() {
                                let index = left + offset + 1;
                                points[index] = Some(point);
                                synthetic[index] = true;
                            }
                        }
                    }
                }
            }
        }
        previous = Some(right);
    }
    synthetic
}

/// Display-only reconstruction of a short gap that may contain a player
/// contact. Unlike one ballistic parabola, the two observed flights are
/// joined at a single velocity-changing kink within a tracked player's body
/// reach. Neither the interpolated samples nor this candidate impact frame
/// are authoritative Touch evidence.
pub fn interpolate_player_rebound_gaps(
    points: &mut [Option<[f64; 2]>],
    synthetic: &mut [bool],
    players: &[Vec<[f64; 4]>],
    width: u32,
    height: u32,
) {
    if points.len() < 6
        || points.len() != synthetic.len()
        || points.len() != players.len()
        || width < 64
        || height < 64
    {
        return;
    }
    let w = f64::from(width);
    let h = f64::from(height);
    let mut left = 0;
    while left + 2 < points.len() {
        let Some(begin) = points[left].filter(|p| valid_point(*p)) else {
            left += 1;
            continue;
        };
        let mut right = left + 1;
        while right < points.len() && points[right].is_none() {
            right += 1;
        }
        if right >= points.len() {
            break;
        }
        let missing = right - left - 1;
        if !(2..=MAX_MISSING_BALL_FRAMES).contains(&missing) {
            left = right;
            continue;
        }
        let Some(end) = points[right].filter(|p| valid_point(*p)) else {
            left = right;
            continue;
        };
        // Both velocities come from real VBallNet samples only, not from
        // earlier interpolation or a single questionable position.
        let previous = (left.saturating_sub(3)..left)
            .rev()
            .find(|&i| !synthetic[i] && points[i].is_some());
        let next = (right + 1..(right + 4).min(points.len()))
            .find(|&i| !synthetic[i] && points[i].is_some());
        let (Some(previous), Some(next)) = (previous, next) else {
            left = right;
            continue;
        };
        let Some(before) = points[previous] else {
            left = right;
            continue;
        };
        let Some(after) = points[next] else {
            left = right;
            continue;
        };
        let px = |p: [f64; 2]| [p[0] * w, p[1] * h];
        let (a, b) = (px(begin), px(end));
        let before = px(before);
        let after = px(after);
        let duration_before = (left - previous) as f64;
        let duration_after = (next - right) as f64;
        let incoming = [
            (a[0] - before[0]) / duration_before,
            (a[1] - before[1]) / duration_before,
        ];
        let outgoing = [
            (after[0] - b[0]) / duration_after,
            (after[1] - b[1]) / duration_after,
        ];
        let speed_before = incoming[0].hypot(incoming[1]);
        let speed_after = outgoing[0].hypot(outgoing[1]);
        if speed_before < 3.0 || speed_after < 3.0 {
            left = right;
            continue;
        }
        let cosine =
            (incoming[0] * outgoing[0] + incoming[1] * outgoing[1]) / (speed_before * speed_after);
        let turn = cosine.clamp(-1.0, 1.0).acos().to_degrees();
        let residual = (incoming[0] - outgoing[0]).hypot(incoming[1] - outgoing[1]);
        if turn < 70.0 || residual < 10.0 {
            left = right;
            continue;
        }
        let mut best: Option<(f64, usize, [f64; 2])> = None;
        for impact in left + 1..right {
            let da = (impact - left) as f64;
            let db = (impact as i64 - right as i64) as f64;
            let from_left = [a[0] + da * incoming[0], a[1] + da * incoming[1]];
            let from_right = [b[0] + db * outgoing[0], b[1] + db * outgoing[1]];
            let error = (from_left[0] - from_right[0]).hypot(from_left[1] - from_right[1]);
            if error > 32.0 {
                continue;
            }
            let joint = [
                (from_left[0] + from_right[0]) * 0.5,
                (from_left[1] + from_right[1]) * 0.5,
            ];
            if !valid_point([joint[0] / w, joint[1] / h]) {
                continue;
            }
            let near_athlete = players[impact].iter().any(|rect| {
                let [x1, y1, x2, y2] = *rect;
                if ![x1, y1, x2, y2].iter().all(|v| v.is_finite()) || x2 <= x1 || y2 <= y1 {
                    return false;
                }
                let athlete_height = (y2 - y1) * h;
                let reach = (athlete_height * 0.42).clamp(12.0, 140.0);
                joint[0] >= x1 * w - reach
                    && joint[0] <= x2 * w + reach
                    // Jump attacks may contact the ball well above the
                    // detected upper-body box while hands are extended.
                    && joint[1] >= y1 * h - athlete_height * 0.55
                    && joint[1] <= y2 * h + athlete_height * 0.15
            });
            if near_athlete && best.is_none_or(|value| error < value.0) {
                best = Some((error, impact, joint));
            }
        }
        let Some((_, impact, joint)) = best else {
            left = right;
            continue;
        };
        // A piecewise flight is valid only if the reconstructed samples
        // remain in frame and move at bounded, physically plausible speeds.
        let reconstructed = (left + 1..right)
            .map(|index| {
                let candidate = if index <= impact {
                    let t = (index - left) as f64 / (impact - left) as f64;
                    [a[0] + (joint[0] - a[0]) * t, a[1] + (joint[1] - a[1]) * t]
                } else {
                    let t = (index - impact) as f64 / (right - impact) as f64;
                    [
                        joint[0] + (b[0] - joint[0]) * t,
                        joint[1] + (b[1] - joint[1]) * t,
                    ]
                };
                [candidate[0] / w, candidate[1] / h]
            })
            .collect::<Vec<_>>();
        let mut previous_point = a;
        let mut plausible = true;
        for point in reconstructed.iter().copied().chain(std::iter::once(end)) {
            let value = px(point);
            if !valid_point(point)
                || (value[0] - previous_point[0]).hypot(value[1] - previous_point[1]) > 65.0
            {
                plausible = false;
                break;
            }
            previous_point = value;
        }
        if plausible {
            for (index, sample) in (left + 1..right).zip(reconstructed) {
                points[index] = Some(sample);
                synthetic[index] = true;
            }
        }
        left = right;
    }
}

fn valid_point(point: [f64; 2]) -> bool {
    point
        .iter()
        .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_only_interior_gaps_preserving_observations() {
        let mut points = [None, Some([0.2, 0.4]), None, None, Some([0.5, 0.4]), None];
        let flags = interpolate_short_gaps(&mut points);
        assert_eq!(flags, [false, false, true, true, false, false]);
        assert_eq!(points[0], None);
        assert_eq!(points[1], Some([0.2, 0.4]));
        assert!((points[2].unwrap()[0] - 0.3).abs() < 1e-12);
        assert!((points[3].unwrap()[0] - 0.4).abs() < 1e-12);
        assert_eq!(points[4], Some([0.5, 0.4]));
        assert_eq!(points[5], None);
    }

    #[test]
    fn does_not_bridge_long_missing_segments() {
        let mut points = vec![Some([0.2, 0.2])];
        points.extend([None; MAX_MISSING_BALL_FRAMES + 1]);
        points.push(Some([0.3, 0.2]));
        assert!(!interpolate_short_gaps(&mut points).iter().any(|flag| *flag));
        assert!(points[1..points.len() - 1].iter().all(Option::is_none));
    }

    #[test]
    fn does_not_join_implausible_switch_or_invalid_coordinates() {
        let mut points = [
            Some([0.05, 0.05]),
            None,
            Some([0.95, 0.95]),
            Some([f64::NAN, 0.3]),
            None,
            Some([0.6, 0.6]),
        ];
        assert!(!interpolate_short_gaps(&mut points).iter().any(|flag| *flag));
        assert_eq!(points[1], None);
        assert_eq!(points[4], None);
    }

    #[test]
    fn recovers_parabolic_flight_using_both_real_boundaries() {
        let flight = |i: usize| {
            let t = i as f64;
            [0.31 + 0.004 * t, 0.22 + 0.003 * t + 0.00045 * t * t]
        };
        let mut points = (0..18).map(|i| Some(flight(i))).collect::<Vec<_>>();
        for row in &mut points[5..=9] {
            *row = None;
        }
        let mask = interpolate_short_gaps(&mut points);
        for i in 5..=9 {
            assert!(mask[i]);
            let point = points[i].unwrap();
            let actual = flight(i);
            assert!((point[0] - actual[0]).abs() < 0.003);
            assert!((point[1] - actual[1]).abs() < 0.003);
        }
        assert!(!mask[4] && !mask[10]);
    }

    #[test]
    fn refuses_gap_with_hidden_velocity_reversal() {
        let mut points = [
            Some([0.20, 0.30]),
            Some([0.21, 0.30]),
            None,
            None,
            Some([0.24, 0.30]),
            Some([0.20, 0.30]),
        ];
        let mask = interpolate_short_gaps(&mut points);
        assert!(!mask.iter().any(|flag| *flag));
        assert!(points[2].is_none() && points[3].is_none());
    }

    #[test]
    fn refuses_ballistic_path_leaving_image_and_never_fills_tails() {
        let mut points = [
            None,
            Some([0.50, 0.016]),
            Some([0.50, 0.004]),
            None,
            None,
            None,
            Some([0.50, 0.004]),
            Some([0.50, 0.016]),
            None,
        ];
        let mask = interpolate_short_gaps(&mut points);
        assert!(!mask.iter().any(|flag| *flag));
        assert!(points[0].is_none() && points[8].is_none());
        assert!(points[3..6].iter().all(Option::is_none));
    }
}
