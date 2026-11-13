//! Remove short VballNet teleports from *temporary* postprocessing samples.
//! The immutable fusion/replay remains unchanged. A false stationary landmark
//! between two compatible ball observations must not erase nearby contacts.
use super::{center, valid_bbox, TouchFrame};

fn location(frame: &TouchFrame, width: f64, height: f64) -> Option<[f64; 2]> {
    let ball = frame
        .selected_ball
        .as_ref()
        .filter(|b| b.source_model == "vballnet" && valid_bbox(b.bbox_xyxy_norm))?;
    let [x, y] = center(ball.bbox_xyxy_norm);
    Some([x * width, y * height])
}

fn distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// A wrong VBallNet location can have *higher* confidence than the real
/// ball. Compare motion on both sides instead of deleting every low-score
/// sample. Only short out-and-back teleport islands are suppressed.
fn suppress_false_ball_islands(
    samples: &mut [Option<[f64; 2]>],
    width: u32,
    height: u32,
) -> Vec<bool> {
    let mut rejected = vec![false; samples.len()];
    if width < 64 || height < 64 || samples.len() < 3 {
        return rejected;
    }
    let (w, h) = (f64::from(width), f64::from(height));
    // At 30 FPS a one-frame move of >~100 px at 720p is suspect,
    // but it is *not* removed unless observations on both sides
    // independently agree that a short remote island is false.
    let min_teleport = (w * 0.09).max(90.0);
    const MAX_BRIDGE_FRAMES: usize = 11;
    for before in 0..samples.len() - 2 {
        let Some(a) = samples[before] else { continue };
        let Some(next) = samples[before + 1] else {
            continue;
        };
        let a_px = [a[0] * w, a[1] * h];
        let next_px = [next[0] * w, next[1] * h];
        if distance(a_px, next_px) < min_teleport {
            continue;
        }
        let end = (before + MAX_BRIDGE_FRAMES + 1).min(samples.len());
        for after in before + 2..end {
            let Some(b) = samples[after] else { continue };
            let b_px = [b[0] * w, b[1] * h];
            let gap = (after - before) as f64;
            if distance(a_px, b_px) > 60.0 + 22.0 * gap {
                continue;
            }
            let separated = (before + 1..after).all(|index| {
                let Some(p) = samples[index] else { return true };
                let t = (index - before) as f64 / gap;
                let bridge = [
                    a_px[0] + t * (b_px[0] - a_px[0]),
                    a_px[1] + t * (b_px[1] - a_px[1]),
                ];
                distance([p[0] * w, p[1] * h], bridge) > min_teleport
            });
            if separated {
                for index in before + 1..after {
                    if samples[index].is_some() {
                        rejected[index] = true;
                        samples[index] = None;
                    }
                }
                break;
            }
        }
    }
    rejected
}

/// Identify unreliable observed positions without inventing a ball location.
/// Streaming CPU stages can apply this boolean mask to their own source data.
pub fn flag_ball_teleports(samples: &[Option<[f64; 2]>], width: u32, height: u32) -> Vec<bool> {
    let mut working = samples.to_vec();
    suppress_false_ball_islands(&mut working, width, height)
}

/// Read-only reconstruction for *display*, never physical Touch evidence.
/// Correct detections stay anchored exactly. Physically plausible short gaps
/// are filled between VBallNet observations; ambiguous flight reversals,
/// out-of-frame arcs and long gaps intentionally stay empty.
pub fn repair_vballnet_trajectory(
    samples: &[Option<[f64; 2]>],
    width: u32,
    height: u32,
) -> (Vec<Option<[f64; 2]>>, Vec<bool>) {
    repair_vballnet_trajectory_with_players(
        samples,
        &vec![Vec::new(); samples.len()],
        width,
        height,
    )
}

/// The player-aware path also joins two genuinely different flights *only*
/// when their observed velocities meet inside a tracked athlete's reach.
/// This branch is exclusively for processed preview visualization.
pub fn repair_vballnet_trajectory_with_players(
    samples: &[Option<[f64; 2]>],
    players: &[Vec<[f64; 4]>],
    width: u32,
    height: u32,
) -> (Vec<Option<[f64; 2]>>, Vec<bool>) {
    let mut points = samples.to_vec();
    suppress_false_ball_islands(&mut points, width, height);
    let mut synthetic = vec![false; points.len()];
    crate::ball_interpolation::interpolate_player_rebound_gaps(
        &mut points,
        &mut synthetic,
        players,
        width,
        height,
    );
    let ballistic = crate::ball_interpolation::interpolate_short_gaps(&mut points);
    for (flag, fitted) in synthetic.iter_mut().zip(ballistic) {
        *flag |= fitted;
    }
    (points, synthetic)
}

/// Remove spurious ball locations from a temporary *observed-only* working
/// copy. Synthetic reconstruction must NEVER become player contact evidence.
#[must_use]
pub fn exclude_ball_teleports(frames: &[TouchFrame], width: u32, height: u32) -> Vec<TouchFrame> {
    let mut filtered = frames.to_vec();
    let mut samples = frames
        .iter()
        .map(|frame| location(frame, f64::from(width), f64::from(height)))
        .map(|point| point.map(|p| [p[0] / f64::from(width), p[1] / f64::from(height)]))
        .collect::<Vec<_>>();
    let rejected = suppress_false_ball_islands(&mut samples, width, height);
    for (frame, invalid) in filtered.iter_mut().zip(rejected) {
        if invalid {
            frame.selected_ball = None;
        }
    }
    filtered
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::touch_pipeline::TouchBall;

    fn sample(i: u64, x: f64) -> TouchFrame {
        TouchFrame {
            frame_index: i,
            players: Vec::new(),
            selected_ball: Some(TouchBall {
                source_model: "vballnet".into(),
                score: 0.7,
                bbox_xyxy_norm: [x, 0.35, x + 0.02, 0.37],
            }),
        }
    }

    #[test]
    fn remote_landmark_island_removed_without_changing_source() {
        let frames: Vec<_> = (0..30_u64)
            .map(|i| {
                let x = if (10..=17).contains(&i) {
                    0.93
                } else {
                    0.34 + (i as f64) * 0.001
                };
                sample(i, x)
            })
            .collect();
        let filtered = exclude_ball_teleports(&frames, 1280, 720);
        assert_eq!(
            filtered
                .iter()
                .filter(|f| f.selected_ball.is_none())
                .count(),
            8
        );
        assert!(filtered[9].selected_ball.is_some());
        assert!(filtered[18].selected_ball.is_some());
        assert!(frames.iter().all(|f| f.selected_ball.is_some()));
    }

    #[test]
    fn recovers_volley_23_4s_false_ball_island_with_high_attack() {
        // Real observations around frames 696..707; the GPU briefly chose
        // a stationary right-edge false ball at 697..700 and 703..706.
        let mut observations = vec![None; 17];
        let samples = [
            (0, [0.477, 0.372]), (1, [0.475, 0.382]),
            (2, [0.932, 0.587]), (3, [0.932, 0.583]),
            (4, [0.930, 0.583]), (5, [0.930, 0.587]),
            (6, [0.443, 0.354]), (7, [0.430, 0.347]),
            (8, [0.932, 0.587]), (9, [0.932, 0.587]),
            (10, [0.934, 0.583]), (11, [0.936, 0.580]),
            (12, [0.389, 0.337]), (13, [0.385, 0.337]),
        ];
        for (i, xy) in samples {
            observations[i] = Some(xy);
        }
        let mut players = vec![vec![]; observations.len()];
        for row in &mut players {
            row.push([0.448, 0.452, 0.493, 0.605]);
        }
        let (repaired, synthetic) = repair_vballnet_trajectory_with_players(
            &observations, &players, 1280, 720,
        );
        for i in 2..=5 {
            assert!(
                repaired[i].is_some_and(|p| p[0] < 0.55),
                "false ball island at frame {} must be reconstructed: {:?}",
                695 + i, repaired[i]
            );
            assert!(synthetic[i]);
        }
        for i in 8..=11 {
            assert!(
                repaired[i].is_some_and(|p| p[0] < 0.55),
                "second false island at frame {} must be reconstructed: {:?}",
                695 + i, repaired[i]
            );
            assert!(synthetic[i]);
        }
        assert_eq!(observations[2], Some([0.932, 0.587]));
        assert_eq!(repaired[1], observations[1]);
        assert_eq!(repaired[6], observations[6]);
    }

    #[test]
    fn repairs_high_confidence_false_island_without_changing_real_anchors() {
        let observed = (0..30)
            .map(|i| {
                Some(if (10..=17).contains(&i) {
                    [0.93, 0.59] // The false right-side stationary landmark.
                } else {
                    [0.34 + i as f64 * 0.001, 0.35]
                })
            })
            .collect::<Vec<_>>();
        let (restored, synthetic) = repair_vballnet_trajectory(&observed, 1280, 720);
        assert_eq!(observed[10], Some([0.93, 0.59]));
        for i in 10..=17 {
            assert!(restored[i].is_none_or(|point| point[0] < 0.5));
            if restored[i].is_some() {
                assert!(
                    synthetic[i],
                    "Inferred location must have a synthetic marker"
                );
            }
        }
        assert_eq!(restored[9], observed[9]);
        assert_eq!(restored[18], observed[18]);
        assert!(!synthetic[9] && !synthetic[18]);
    }

    #[test]
    fn does_not_invent_flight_across_a_real_player_reversal() {
        let observed = (463..=495)
            .map(|f| {
                let t = f as f64;
                let [x, y] = if f == 475 || (477..=484).contains(&f) {
                    [1192.0, 420.0]
                } else if f <= 476 {
                    [820.0 + 6.0 * (t - 476.0), 407.0 + 15.0 * (t - 476.0)]
                } else {
                    [808.0 - 10.0 * (t - 485.0), 400.0 - 29.0 * (t - 485.0)]
                };
                Some([x / 1280.0, y / 720.0])
            })
            .collect::<Vec<_>>();
        let (points, synthetic) = repair_vballnet_trajectory(&observed, 1280, 720);
        assert_eq!(points[476 - 463], observed[476 - 463]);
        assert!(!synthetic[476 - 463]);
        for frame in [475, 477, 478, 479, 480, 481, 482, 483, 484] {
            assert!(
                points[frame - 463].is_none_or(|p| p[0] < 0.83),
                "Bad ball at frame {frame} must not survive"
            );
        }
        // A hidden collision has incompatible velocities: do not bridge it
        // with a single straight line that would hide the actual Touch.
        assert!(points[(480 - 463) as usize].is_none());
    }

    #[test]
    fn player_supported_rebound_joins_two_flights_without_false_landmark() {
        let observed = (463..=495)
            .map(|f| {
                let t = f as f64;
                let [x, y] = if f == 475 || (477..=484).contains(&f) {
                    [1192.0, 420.0]
                } else if f <= 476 {
                    [820.0 + 6.0 * (t - 476.0), 407.0 + 15.0 * (t - 476.0)]
                } else {
                    [808.0 - 10.0 * (t - 485.0), 400.0 - 29.0 * (t - 485.0)]
                };
                Some([x / 1280.0, y / 720.0])
            })
            .collect::<Vec<_>>();
        let mut players = vec![Vec::new(); observed.len()];
        for frame in 479..=483 {
            players[frame - 463].push([
                817.0 / 1280.0,
                480.0 / 720.0,
                888.0 / 1280.0,
                710.0 / 720.0,
            ]);
        }
        let (filled, inferred) =
            repair_vballnet_trajectory_with_players(&observed, &players, 1280, 720);
        for frame in 477..=484 {
            let index = frame - 463;
            let pos = filled[index].expect("body-supported short rebound should be displayable");
            assert!(inferred[index]);
            assert!(
                pos[0] < 0.83,
                "Must never use the false right-side landmark"
            );
        }
        assert_eq!(filled[476 - 463], observed[476 - 463]);
        assert!(!inferred[476 - 463]);
        // A true flight reversal without any physically nearby player
        // remains unresolved, instead of manufacturing a new Touch.
        let (unconfirmed, _) = repair_vballnet_trajectory(&observed, 1280, 720);
        assert!(unconfirmed[480 - 463].is_none());
        assert!(observed[480 - 463].unwrap()[0] > 0.9);
    }

    #[test]
    fn realistic_rebound_not_suppressed() {
        let frames: Vec<_> = (0..25_u64)
            .map(|i| {
                sample(
                    i,
                    if i <= 12 {
                        0.3 + i as f64 * 0.004
                    } else {
                        0.348 - (i - 12) as f64 * 0.004
                    },
                )
            })
            .collect();
        let filtered = exclude_ball_teleports(&frames, 1280, 720);
        assert!(filtered.iter().all(|f| f.selected_ball.is_some()));
    }
}
