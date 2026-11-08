//! Motion evidence classification and non-overlapping clip boundaries.
//! These are measurable kinematic categories, never inferred serve/attack,
//! dig/block, player identity, or point outcome.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MotionInput {
    pub probability: f64,
    pub direction_change_deg: f64,
    pub speed_before_px_frame: f64,
    pub speed_after_px_frame: f64,
    pub velocity_residual_px_frame: f64,
    pub provisional_track_id: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct EventEvidence {
    pub event_type: &'static str,
    pub motion_pattern: &'static str,
    pub confidence: f64,
    pub direction_change_deg: f64,
    pub speed_before_px_frame: f64,
    pub speed_after_px_frame: f64,
    pub velocity_residual_px_frame: f64,
    pub provisional_track_id: Option<u64>,
    pub athlete_verified: bool,
    pub game_action_verified: bool,
}
pub fn interpret_motion(input: MotionInput) -> Result<EventEvidence, String> {
    if !input.probability.is_finite()
        || !(0.0..=1.0).contains(&input.probability)
        || !input.direction_change_deg.is_finite()
        || !(0.0..=180.0).contains(&input.direction_change_deg)
        || ![
            input.speed_before_px_frame,
            input.speed_after_px_frame,
            input.velocity_residual_px_frame,
        ]
        .iter()
        .all(|v| v.is_finite() && *v >= 0.0 && *v <= 20000.0)
        || input
            .provisional_track_id
            .is_some_and(|v| v == 0 || v > i64::MAX as u64)
    {
        return Err("Invalid motion evidence".into());
    }
    // Ratios are unreliable when one side is effectively stationary.
    // An explicitly measured relative or absolute change must still be
    // distinguished from a physical action technique such as "attack".
    let before = input.speed_before_px_frame;
    let after = input.speed_after_px_frame;
    let delta = after - before;
    let strong_accel = delta >= 3.0 && (before < 2.0 || after >= before * 1.5);
    let strong_decel = delta <= -3.0 && (after < 2.0 || before >= after * 1.5);
    let direction = input.direction_change_deg >= 35.0;
    let pattern = if input.direction_change_deg >= 110.0 {
        "direction_reversal"
    } else if direction && (strong_accel || strong_decel) {
        "direction_and_speed_change"
    } else if direction {
        "direction_change"
    } else if strong_accel {
        "measured_acceleration"
    } else if strong_decel {
        "measured_deceleration"
    } else {
        "motion_residual_only"
    };
    Ok(EventEvidence {
        event_type: "unknown_touch",
        motion_pattern: pattern,
        confidence: input.probability,
        direction_change_deg: input.direction_change_deg,
        speed_before_px_frame: before,
        speed_after_px_frame: after,
        velocity_residual_px_frame: input.velocity_residual_px_frame,
        provisional_track_id: input.provisional_track_id,
        athlete_verified: false,
        game_action_verified: false,
    })
}
#[derive(Clone, Debug, Serialize)]
pub struct ClipBounds {
    pub start_frame: u64,
    pub end_frame: u64,
    pub source_fps: u32,
    pub includes_complete_detected_interval: bool,
    pub boundaries_provisional: bool,
}
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Interval {
    pub start: u64,
    pub end: u64,
}
/// Extend candidates with context up to the midpoints between neighboring
/// detected intervals. Never crop a detected rally or accidentally include a
/// portion of another candidate. Caller must supply the *entire* run, not
/// only its current preview page, to prevent pagination-dependent cuts.
pub fn bounded_clips(
    intervals: &[Interval],
    video_frames: u64,
    context: u64,
) -> Result<Vec<ClipBounds>, String> {
    if video_frames == 0
        || video_frames > 48 * 3600 * 30
        || context > 180
        || intervals.len() > 200_000
    {
        return Err("Invalid video length, context or number of rally intervals".into());
    }
    for (i, interval) in intervals.iter().enumerate() {
        if interval.start > interval.end
            || interval.end >= video_frames
            || (i > 0 && intervals[i - 1].end >= interval.start)
        {
            return Err("Invalid, overlapping, unsorted or out-of-bounds rally intervals".into());
        }
    }
    let mut result = Vec::with_capacity(intervals.len());
    for (i, interval) in intervals.iter().enumerate() {
        let mut start = interval.start.saturating_sub(context);
        let mut end = interval.end.saturating_add(context).min(video_frames - 1);
        if i > 0 {
            let prev = intervals[i - 1].end;
            // The first half of the gap belongs to the earlier episode;
            // the second half belongs to the later episode.
            let minimum = prev + 1 + (interval.start - prev - 1) / 2;
            start = start.max(minimum);
        }
        if let Some(next) = intervals.get(i + 1) {
            let maximum = interval.end + (next.start - interval.end - 1) / 2;
            end = end.min(maximum);
        }
        if start > interval.start || end < interval.end {
            return Err("Context trim cannot cut detected gameplay".into());
        }
        result.push(ClipBounds {
            start_frame: start,
            end_frame: end,
            source_fps: 30,
            includes_complete_detected_interval: true,
            boundaries_provisional: true,
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event(angle: f64, before: f64, after: f64) -> EventEvidence {
        interpret_motion(MotionInput {
            probability: 0.94,
            direction_change_deg: angle,
            speed_before_px_frame: before,
            speed_after_px_frame: after,
            velocity_residual_px_frame: 8.0,
            provisional_track_id: Some(12),
        })
        .unwrap()
    }
    #[test]
    fn motion_categories_are_kinematic_not_game_actions() {
        assert_eq!(event(150.0, 5.0, 4.0).motion_pattern, "direction_reversal");
        assert_eq!(
            event(50.0, 4.0, 12.0).motion_pattern,
            "direction_and_speed_change"
        );
        assert_eq!(event(4.0, 2.0, 9.0).motion_pattern, "measured_acceleration");
        assert_eq!(
            event(2.0, 11.0, 2.0).motion_pattern,
            "measured_deceleration"
        );
        assert_eq!(event(3.0, 9.0, 8.0).motion_pattern, "motion_residual_only");
        let classified = event(150.0, 5.0, 4.0);
        assert_eq!(classified.event_type, "unknown_touch");
        assert!(!classified.athlete_verified);
        assert!(!classified.game_action_verified);
    }
    #[test]
    fn invalid_untrusted_motion_does_not_assign_athlete() {
        assert!(interpret_motion(MotionInput {
            probability: 0.6,
            direction_change_deg: 190.0,
            speed_before_px_frame: 2.0,
            speed_after_px_frame: 5.0,
            velocity_residual_px_frame: 7.0,
            provisional_track_id: None
        })
        .is_err());
        let mut row = MotionInput {
            probability: 0.6,
            direction_change_deg: 20.0,
            speed_before_px_frame: 2.0,
            speed_after_px_frame: 5.0,
            velocity_residual_px_frame: 7.0,
            provisional_track_id: Some(0),
        };
        assert!(interpret_motion(row).is_err());
        row.provisional_track_id = None;
        let valid = interpret_motion(row).unwrap();
        assert!(valid.provisional_track_id.is_none());
        assert!(!valid.athlete_verified);
    }
    #[test]
    fn midpoint_context_never_overlaps_or_crops_detected_play() {
        let items = [
            Interval { start: 30, end: 80 },
            Interval {
                start: 91,
                end: 130,
            },
            Interval {
                start: 230,
                end: 310,
            },
        ];
        let result = bounded_clips(&items, 400, 45).unwrap();
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].start_frame, 0);
        assert_eq!(result[0].end_frame, 85);
        assert_eq!(result[1].start_frame, 86);
        assert_eq!(result[1].end_frame, 175);
        assert_eq!(result[2].start_frame, 185);
        assert_eq!(result[2].end_frame, 355);
        for i in 0..items.len() {
            assert!(result[i].start_frame <= items[i].start);
            assert!(result[i].end_frame >= items[i].end);
            if i > 0 {
                assert!(result[i - 1].end_frame < result[i].start_frame);
            }
        }
    }
    #[test]
    fn rejects_overlap_reverse_order_or_out_of_range() {
        assert!(bounded_clips(
            &[
                Interval { start: 1, end: 10 },
                Interval { start: 10, end: 12 }
            ],
            30,
            2
        )
        .is_err());
        assert!(bounded_clips(
            &[
                Interval { start: 21, end: 25 },
                Interval { start: 2, end: 6 }
            ],
            30,
            2
        )
        .is_err());
        assert!(bounded_clips(&[Interval { start: 1, end: 40 }], 30, 2).is_err());
        assert!(bounded_clips(&[], 30, 181).is_err());
        assert!(bounded_clips(&[], 30, 30).is_ok());
    }
}
