//! Deterministic CPU-only RAVEL touch validation: the ball must change
//! velocity after subtracting gravity.  This mirrors _ball_motion_change
//! in scripts/volleyball_studio/reprocess.py and never performs inference.

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BallObservation {
    pub frame: i64,
    pub x: f64,
    pub y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MotionConfig {
    pub velocity_window: usize,
    pub min_samples: usize,
    pub max_gap_frames: i64,
    pub min_speed: f64,
    pub min_residual: f64,
    pub relative_residual: f64,
    pub gravity: f64,
}

/// Shared physics thresholds for Touch detection and trajectory diagnostics.
pub const TOUCH_MOTION_CONFIG: MotionConfig = MotionConfig {
    velocity_window: 5,
    min_samples: 2,
    max_gap_frames: 4,
    min_speed: 1.0,
    min_residual: 6.0,
    relative_residual: 0.30,
    gravity: 1.0,
};
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct MotionEvidence {
    pub motion_frame: i64,
    pub speed_before: f64,
    pub speed_after: f64,
    pub angle: f64,
    pub residual: f64,
}

fn fit(
    observations: &[BallObservation],
    index: usize,
    step: isize,
    config: MotionConfig,
) -> Option<(f64, f64, f64)> {
    let mut segment = Vec::with_capacity(config.velocity_window);
    let mut cursor = index as isize;
    let mut last_frame = observations[index].frame;
    while cursor >= 0
        && (cursor as usize) < observations.len()
        && segment.len() < config.velocity_window
    {
        let item = observations[cursor as usize];
        if item.frame.abs_diff(last_frame) > config.max_gap_frames as u64 {
            break;
        }
        segment.push(item);
        last_frame = item.frame;
        cursor += step;
    }
    if segment.len() < config.min_samples {
        return None;
    }
    let min_frame = segment.iter().map(|p| p.frame).min()?;
    let max_frame = segment.iter().map(|p| p.frame).max()?;
    if min_frame == max_frame {
        return None;
    }
    let n = segment.len() as f64;
    let center_time = segment.iter().map(|p| p.frame as f64).sum::<f64>() / n;
    let variance = segment
        .iter()
        .map(|p| (p.frame as f64 - center_time).powi(2))
        .sum::<f64>();
    if variance == 0.0 {
        return None;
    }
    let vx = segment
        .iter()
        .map(|p| (p.frame as f64 - center_time) * p.x)
        .sum::<f64>()
        / variance;
    let vy = segment
        .iter()
        .map(|p| (p.frame as f64 - center_time) * p.y)
        .sum::<f64>()
        / variance;
    Some((vx, vy, center_time))
}

pub fn ball_motion_change(
    observations: &[BallObservation],
    target_frame: i64,
    window_frames: i64,
    config: MotionConfig,
) -> Option<MotionEvidence> {
    if observations.is_empty() || window_frames < 0 || config.max_gap_frames < 0 {
        return None;
    }
    let start = target_frame.saturating_sub(window_frames);
    let end = target_frame.saturating_add(window_frames);
    let begin = observations.partition_point(|p| p.frame < start);
    let mut strongest = None;
    for index in begin..observations.len() {
        let observed_frame = observations[index].frame;
        if observed_frame > end {
            break;
        }
        let (bx, by, bt) = match fit(observations, index, -1, config) {
            Some(v) => v,
            None => continue,
        };
        let (ax, ay, at) = match fit(observations, index, 1, config) {
            Some(v) => v,
            None => continue,
        };
        let speed_before = bx.hypot(by);
        let speed_after = ax.hypot(ay);
        let top_speed = speed_before.max(speed_after);
        if top_speed < config.min_speed {
            continue;
        }
        let residual = (ax - bx).hypot(ay - by - config.gravity * (at - bt).max(0.0));
        let threshold = config
            .min_residual
            .max(config.relative_residual * top_speed);
        if residual < threshold {
            continue;
        }
        let cosine = (bx * ax + by * ay) / (speed_before * speed_after).max(1e-9);
        let angle = cosine.clamp(-1.0, 1.0).acos().to_degrees();
        let speed_delta = (speed_after - speed_before).abs();
        if angle < 25.0 && speed_delta < 4.0_f64.max(0.30 * top_speed) {
            continue;
        }
        if strongest
            .as_ref()
            .is_none_or(|e: &MotionEvidence| residual > e.residual)
        {
            strongest = Some(MotionEvidence {
                motion_frame: observed_frame,
                speed_before,
                speed_after,
                angle,
                residual,
            });
        }
    }
    strongest
}

/// Local ball motion discontinuity is physical evidence, not an athlete touch.
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct BallMotionExtremum {
    pub frame_index: u64,
    pub x_px: f64,
    pub y_px: f64,
    pub direction_change_deg: f64,
    pub speed_before_px_frame: f64,
    pub speed_after_px_frame: f64,
    pub velocity_residual_px_frame: f64,
    pub physical_motion_supported: bool,
    pub athlete_identity_verified: bool,
}

/// Bounded read-only scan of one chosen ball-source trajectory. No GPU.
pub fn ball_motion_extrema(
    observations: &[BallObservation],
    min_distance_frames: u64,
) -> Result<Vec<BallMotionExtremum>, &'static str> {
    if observations.len() > 1200
        || !(3..=60).contains(&min_distance_frames)
        || observations
            .iter()
            .any(|p| p.frame < 0 || !p.x.is_finite() || !p.y.is_finite())
        || observations.windows(2).any(|v| v[0].frame >= v[1].frame)
    {
        return Err("Invalid, duplicate or oversized ball trajectory");
    }
    if observations.len() < 5 {
        return Ok(Vec::new());
    }
    let mut ranked = Vec::new();
    for item in observations {
        if let Some(e) = ball_motion_change(observations, item.frame, 0, TOUCH_MOTION_CONFIG) {
            ranked.push((item.frame, item.x, item.y, e));
        }
    }
    ranked.sort_by(|a, b| {
        b.3.residual
            .total_cmp(&a.3.residual)
            .then_with(|| a.0.cmp(&b.0))
    });
    let mut kept: Vec<(i64, f64, f64, MotionEvidence)> = Vec::new();
    for event in ranked {
        if kept
            .iter()
            .all(|old| old.0.abs_diff(event.0) >= min_distance_frames)
        {
            kept.push(event);
        }
        if kept.len() == 120 {
            break;
        }
    }
    kept.sort_by_key(|e| e.0);
    Ok(kept
        .into_iter()
        .map(|(frame, x, y, e)| BallMotionExtremum {
            frame_index: frame as u64,
            x_px: x,
            y_px: y,
            direction_change_deg: e.angle,
            speed_before_px_frame: e.speed_before,
            speed_after_px_frame: e.speed_after,
            velocity_residual_px_frame: e.residual,
            physical_motion_supported: true,
            athlete_identity_verified: false,
        })
        .collect())
}
fn config_valid(config: MotionConfig) -> bool {
    config.velocity_window >= 2
        && config.velocity_window <= 128
        && config.min_samples >= 2
        && config.min_samples <= config.velocity_window
        && config.max_gap_frames >= 0
        && [
            config.min_speed,
            config.min_residual,
            config.relative_residual,
            config.gravity,
        ]
        .iter()
        .all(|v| v.is_finite() && *v >= 0.0)
}

/// Evaluate a series of candidate Touch frames against the same ball evidence.
///
/// C ABI contract:
/// - input observations sorted by nondecreasing frame; all coordinates finite;
/// - output arrays each have at least target_count elements;
/// - statuses[i]: 1 = evidence, 0 = no break, negative = invalid input;
/// - returns 0 on valid processing, -1 for invalid pointer/shape/config.
///
/// # Safety
/// The caller must provide valid, non-overlapping allocations covering all
/// lengths. FFI pointers cannot be validated for allocation size in Rust.
#[no_mangle]
pub unsafe extern "C" fn volley_ball_motion_batch_v1(
    observations: *const BallObservation,
    count: usize,
    target_frames: *const i64,
    target_count: usize,
    window_frames: i64,
    config: *const MotionConfig,
    outputs: *mut MotionEvidence,
    statuses: *mut i32,
) -> i32 {
    if observations.is_null()
        || target_frames.is_null()
        || config.is_null()
        || outputs.is_null()
        || statuses.is_null()
        || count == 0
        || count > 10_000_000
        || target_count == 0
        || target_count > 1_000_000
        || window_frames < 0
    {
        return -1;
    }
    // SAFETY: pointers and lengths follow the documented C ABI contract.
    let (samples, frames, results, status, parameters) = unsafe {
        (
            std::slice::from_raw_parts(observations, count),
            std::slice::from_raw_parts(target_frames, target_count),
            std::slice::from_raw_parts_mut(outputs, target_count),
            std::slice::from_raw_parts_mut(statuses, target_count),
            *config,
        )
    };
    if !config_valid(parameters)
        || samples.iter().any(|s| !s.x.is_finite() || !s.y.is_finite())
        || samples.windows(2).any(|pair| pair[0].frame > pair[1].frame)
    {
        return -1;
    }
    for (idx, frame) in frames.iter().enumerate() {
        match ball_motion_change(samples, *frame, window_frames, parameters) {
            Some(found) => {
                results[idx] = found;
                status[idx] = 1;
            }
            None => {
                results[idx] = MotionEvidence::default();
                status[idx] = 0;
            }
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> MotionConfig {
        MotionConfig {
            velocity_window: 5,
            min_samples: 2,
            max_gap_frames: 4,
            min_speed: 1.0,
            min_residual: 6.0,
            relative_residual: 0.30,
            gravity: 1.0,
        }
    }

    fn trajectory(f: impl Fn(i64) -> (f64, f64)) -> Vec<BallObservation> {
        (0..31)
            .map(|frame| {
                let (x, y) = f(frame);
                BallObservation { frame, x, y }
            })
            .collect()
    }

    #[test]
    fn straight_and_gravity_only_flight_have_no_break() {
        let straight = trajectory(|frame| (100.0 + 5.0 * frame as f64, 180.0));
        assert!(ball_motion_change(&straight, 15, 4, config()).is_none());
        let apex = trajectory(|frame| {
            let t = frame as f64;
            (180.0, 300.0 - 15.0 * t + t * t / 2.0)
        });
        assert!(ball_motion_change(&apex, 15, 4, config()).is_none());
    }

    #[test]
    fn reversal_and_speed_jump_produce_evidence() {
        let reverse = trajectory(|frame| {
            let t = (frame - 15) as f64;
            (
                if frame <= 15 {
                    200.0 + 7.0 * t
                } else {
                    200.0 - 6.0 * t
                },
                180.0,
            )
        });
        let e = ball_motion_change(&reverse, 15, 4, config()).unwrap();
        assert!(e.angle > 120.0);
        assert!(e.residual > 6.0);
        let gain = trajectory(|frame| {
            let t = (frame - 15) as f64;
            (
                if frame <= 15 {
                    200.0 + 2.0 * t
                } else {
                    200.0 + 12.0 * t
                },
                180.0,
            )
        });
        let e = ball_motion_change(&gain, 15, 4, config()).unwrap();
        assert!(e.speed_after > e.speed_before * 1.5);
        assert!(e.angle < 10.0);
    }

    #[test]
    fn extrema_find_reversal_but_not_ballistic_gravity() {
        let reverse = trajectory(|frame| {
            let t = (frame - 15) as f64;
            (
                if frame <= 15 {
                    200.0 + 7.0 * t
                } else {
                    200.0 - 6.0 * t
                },
                180.0,
            )
        });
        let impacts = ball_motion_extrema(&reverse, 8).unwrap();
        assert_eq!(impacts.len(), 1);
        assert!(impacts[0].frame_index.abs_diff(15) <= 3);
        assert!(impacts[0].direction_change_deg > 110.0);
        assert!(impacts[0].physical_motion_supported && !impacts[0].athlete_identity_verified);
        let gravity = trajectory(|frame| {
            let t = frame as f64;
            (180.0, 300.0 - 15.0 * t + t * t / 2.0)
        });
        assert!(ball_motion_extrema(&gravity, 8).unwrap().is_empty());
        assert!(ball_motion_extrema(
            &trajectory(|frame| { (100.0 + 5.0 * frame as f64, 180.0) }),
            8
        )
        .unwrap()
        .is_empty());
    }
    #[test]
    fn extrema_reject_duplicate_nonfinite_and_unsorted() {
        let a = [
            BallObservation {
                frame: 1,
                x: 1.0,
                y: 1.0,
            },
            BallObservation {
                frame: 1,
                x: 2.0,
                y: 1.0,
            },
        ];
        assert!(ball_motion_extrema(&a, 8).is_err());
        let a = [
            BallObservation {
                frame: 2,
                x: 1.0,
                y: 1.0,
            },
            BallObservation {
                frame: 1,
                x: 2.0,
                y: 1.0,
            },
        ];
        assert!(ball_motion_extrema(&a, 8).is_err());
        let a = [BallObservation {
            frame: 1,
            x: f64::NAN,
            y: 1.0,
        }];
        assert!(ball_motion_extrema(&a, 8).is_err());
        assert!(ball_motion_extrema(&[], 2).is_err());
    }
    #[test]
    fn sparse_frames_do_not_establish_touch() {
        let short = [BallObservation {
            frame: 15,
            x: 100.0,
            y: 180.0,
        }];
        assert!(ball_motion_change(&short, 15, 4, config()).is_none());
    }
}
