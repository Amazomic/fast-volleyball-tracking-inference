//! Provisional serve/toss and side-possession context for a single rally.
//! Ported from Python player_contacts._merge_serve_toss,
//! _settle_serve and RallyContactStateMachine._advance.
//!
//! Never edits source Track IDs, hand labels or source Touch JSONL. A missing
//! side remains unknown, and neither team rules nor ball flight can fabricate
//! an athlete/contact not observed on its original frame.
use crate::{action_prediction::ActionKind, court_geometry::CourtSide};
use serde::Serialize;

const TOSS_MAX_FRAMES: u64 = 75; // Python TOSS_MAX_SECONDS * 30 fps.
const TOSS_MIN_RISE_DIAMETERS: f64 = 2.0;
const SERVER_REPEAT_FRAMES: u64 = 60; // Python SERVER_REPEAT_SECONDS * 30 fps.
const SERVE_MIN_SPEED_GAIN: f64 = 1.2;
const MAX_TOUCHES_PER_SIDE: u32 = 3;

#[derive(Clone, Copy, Debug)]
pub struct ContactEvidence {
    pub frame: u64,
    pub track_id: Option<u64>,
    pub side: Option<CourtSide>,
    pub in_serve_zone: bool,
    pub kind: Option<ActionKind>,
    pub ball_y_px: f64,
    pub speed_before_px_frame: f64,
    pub speed_after_px_frame: f64,
    pub velocity_residual_px_frame: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct BallSample {
    pub frame: u64,
    pub y_px: f64,
    pub radius_px: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct ResolvedTouch {
    pub frame_index: u64,
    pub provisional_track_id: Option<u64>,
    pub side: Option<CourtSide>,
    pub estimated_kind: Option<ActionKind>,
    pub ignored_serve_toss: bool,
    pub possession_touches: u32,
    pub phase: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct RallyContext {
    pub method: &'static str,
    pub serve_frame: Option<u64>,
    pub serve_track_id: Option<u64>,
    pub serving_side: Option<CourtSide>,
    pub side_switches: u32,
    pub three_touch_possessions: u32,
    pub over_three_touch_possessions: u32,
    pub max_same_side_touches: u32,
    pub ignored_serve_toss_count: u32,
    /// Positional votes cannot be promoted to an authoritative player ID.
    pub authoritative: bool,
    pub events: Vec<ResolvedTouch>,
}

fn speed_gain(contact: ContactEvidence) -> f64 {
    contact.speed_after_px_frame / contact.speed_before_px_frame.max(0.1)
}

fn plausible_toss(first: ContactEvidence, second: ContactEvidence, ball: &[BallSample]) -> bool {
    if first.track_id.is_none()
        || first.track_id != second.track_id
        || !first.in_serve_zone
        || !second.in_serve_zone
        || second.frame <= first.frame
        || second.frame - first.frame > TOSS_MAX_FRAMES
        || speed_gain(second) < SERVE_MIN_SPEED_GAIN
    {
        return false;
    }
    let mut between = ball
        .iter()
        .filter(|sample| sample.frame > first.frame && sample.frame < second.frame)
        .copied()
        .collect::<Vec<_>>();
    // A one-frame ball ID or source-model glitch must not be treated as
    // the apex of a toss. Require at least three measured flight frames.
    if between.len() < 3 {
        return false;
    }
    let mut radius = between
        .iter()
        .filter_map(|sample| {
            (sample.radius_px.is_finite() && sample.radius_px > 0.0).then_some(sample.radius_px)
        })
        .collect::<Vec<_>>();
    radius.sort_by(f64::total_cmp);
    let median_radius = radius.get(radius.len() / 2).copied().unwrap_or(10.0);
    let min_rise = TOSS_MIN_RISE_DIAMETERS * 2.0 * median_radius;
    between.sort_by(|a, b| a.y_px.total_cmp(&b.y_px));
    // Compared with Python's single minimum, use the middle of the three
    // highest observed positions to reject one-frame outliers.
    let apex = between[1].y_px;
    first.ball_y_px - apex >= min_rise && second.ball_y_px - apex >= min_rise
}

/// Resolve nonauthoritative contact actions and team possessions per rally.
///
/// The caller provides temporally sorted, same-frame-validated contacts and
/// ball samples from one *accepted* CPU rally. Empty or uncalibrated input
/// never turns into a fabricated serve. The returned event list retains
/// every raw event; toss suppression is only a context flag for the preview.
#[must_use]
pub fn analyze(contacts: &[ContactEvidence], ball: &[BallSample]) -> RallyContext {
    let mut serve = contacts.iter().position(|contact| {
        contact.kind == Some(ActionKind::Serve)
            && contact.track_id.is_some()
            && contact.in_serve_zone
            && contact.side.is_some()
    });
    let mut suppressed = vec![false; contacts.len()];

    // Python merge_serve_toss applies to the first two physical contacts in
    // a rally only. Do not merge an arbitrary later back-row pass into serve.
    if contacts.len() >= 2 && plausible_toss(contacts[0], contacts[1], ball) {
        serve = Some(1);
        suppressed[0] = true;
    }
    if let Some(index) = serve {
        let server = contacts[index].track_id;
        let start = contacts[index].frame;
        let mut group = vec![index];
        for (next, contact) in contacts.iter().enumerate().skip(index + 1) {
            if contact.track_id != server || contact.frame - start > SERVER_REPEAT_FRAMES {
                break;
            }
            group.push(next);
        }
        let strike = group
            .iter()
            .copied()
            .max_by(|&a, &b| {
                contacts[a]
                    .velocity_residual_px_frame
                    .total_cmp(&contacts[b].velocity_residual_px_frame)
            })
            .unwrap_or(index);
        for old in group {
            if old != strike {
                suppressed[old] = true;
            }
        }
        serve = Some(strike);
    }
    let serve_frame = serve.map(|index| contacts[index].frame);
    let serve_track_id = serve.and_then(|index| contacts[index].track_id);
    let serving_side = serve.and_then(|index| contacts[index].side);

    let mut side = None;
    let mut consecutive = 0u32;
    let mut side_switches = 0u32;
    let mut max_same_side_touches = 0u32;
    let mut three_touch_possessions = 0u32;
    let mut over_three_touch_possessions = 0u32;
    let mut reception_touches = 0u32;
    let mut phase = "pre_serve";
    let mut events = Vec::with_capacity(contacts.len());

    for (index, contact) in contacts.iter().copied().enumerate() {
        if suppressed[index] {
            events.push(ResolvedTouch {
                frame_index: contact.frame,
                provisional_track_id: contact.track_id,
                side: contact.side,
                estimated_kind: contact.kind,
                ignored_serve_toss: true,
                possession_touches: 0,
                phase: "serve_preparation",
            });
            continue;
        }
        let estimated_kind = if serve == Some(index) {
            Some(ActionKind::Serve)
        } else if contact.kind == Some(ActionKind::Serve) {
            // A serving area alone does not create a second legal serve.
            None
        } else {
            contact.kind
        };
        if serve == Some(index) {
            phase = "serve_flight";
            reception_touches = 0;
            consecutive = 0;
            side = None;
        } else if matches!(phase, "serve_flight" | "reception")
            && serving_side.is_some()
            && contact.side.is_some()
            && contact.side != serving_side
        {
            reception_touches += 1;
            phase = if reception_touches < 2 {
                "reception"
            } else {
                "possession"
            };
        } else if phase == "pre_serve" {
            phase = "possession";
        }
        if let Some(observed_side) = contact.side {
            if side == Some(observed_side) {
                consecutive += 1;
            } else {
                if side.is_some() {
                    side_switches += 1;
                }
                side = Some(observed_side);
                consecutive = 1;
            }
            max_same_side_touches = max_same_side_touches.max(consecutive);
            if consecutive == MAX_TOUCHES_PER_SIDE {
                three_touch_possessions += 1;
            } else if consecutive == MAX_TOUCHES_PER_SIDE + 1 {
                over_three_touch_possessions += 1;
            }
        } else {
            // Unknown team cannot advance a team's confirmed touch count.
            side = None;
            consecutive = 0;
        }
        events.push(ResolvedTouch {
            frame_index: contact.frame,
            provisional_track_id: contact.track_id,
            side: contact.side,
            estimated_kind,
            ignored_serve_toss: false,
            possession_touches: consecutive,
            phase,
        });
    }
    RallyContext {
        method: "python_serve_toss_possession_v1",
        serve_frame,
        serve_track_id,
        serving_side,
        side_switches,
        three_touch_possessions,
        over_three_touch_possessions,
        max_same_side_touches,
        ignored_serve_toss_count: suppressed.iter().filter(|&&ignore| ignore).count() as u32,
        authoritative: false,
        events,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contact(frame: u64, track_id: u64, side: CourtSide) -> ContactEvidence {
        ContactEvidence {
            frame,
            track_id: Some(track_id),
            side: Some(side),
            in_serve_zone: true,
            kind: Some(ActionKind::Attack),
            ball_y_px: 250.0,
            speed_before_px_frame: 2.0,
            speed_after_px_frame: 8.0,
            velocity_residual_px_frame: 7.0,
        }
    }

    #[test]
    fn merges_observed_serve_toss_and_selects_second_strike() {
        let contacts = [
            contact(100, 5, CourtSide::Near),
            ContactEvidence {
                velocity_residual_px_frame: 18.0,
                ..contact(144, 5, CourtSide::Near)
            },
            contact(190, 9, CourtSide::Far),
        ];
        let flight = (101..144)
            .map(|frame| BallSample {
                frame,
                y_px: if frame > 115 && frame < 130 {
                    160.0
                } else {
                    235.0
                },
                radius_px: 10.0,
            })
            .collect::<Vec<_>>();
        let resolved = analyze(&contacts, &flight);
        assert_eq!(resolved.serve_frame, Some(144));
        assert_eq!(resolved.serve_track_id, Some(5));
        assert_eq!(resolved.ignored_serve_toss_count, 1);
        assert!(resolved.events[0].ignored_serve_toss);
        assert_eq!(resolved.events[1].estimated_kind, Some(ActionKind::Serve));
        assert_eq!(resolved.events[2].phase, "reception");
        assert_eq!(resolved.side_switches, 1);
    }

    #[test]
    fn does_not_merge_without_apex_or_matching_id_or_zone() {
        let first = contact(100, 5, CourtSide::Near);
        let other = contact(140, 9, CourtSide::Far);
        let flat = (101..140)
            .map(|frame| BallSample {
                frame,
                y_px: 240.0,
                radius_px: 10.0,
            })
            .collect::<Vec<_>>();
        assert_eq!(analyze(&[first, other], &flat).serve_frame, None);
        let mut next = contact(140, 5, CourtSide::Near);
        next.in_serve_zone = false;
        assert_eq!(analyze(&[first, next], &flat).serve_frame, None);
        next.in_serve_zone = true;
        assert_eq!(analyze(&[first, next], &flat).serve_frame, None);
    }

    #[test]
    fn collapses_server_repeat_within_two_seconds_preserving_strongest_impact() {
        let mut serve = contact(100, 5, CourtSide::Near);
        serve.kind = Some(ActionKind::Serve);
        let mut stronger = contact(127, 5, CourtSide::Near);
        stronger.velocity_residual_px_frame = 25.0;
        let resolved = analyze(&[serve, stronger, contact(190, 8, CourtSide::Far)], &[]);
        assert_eq!(resolved.serve_frame, Some(127));
        assert!(resolved.events[0].ignored_serve_toss);
        assert_eq!(resolved.events[1].estimated_kind, Some(ActionKind::Serve));
    }

    #[test]
    fn possession_counts_three_then_flags_fourth_without_inventing_id() {
        let c = [
            contact(10, 1, CourtSide::Near),
            contact(20, 2, CourtSide::Near),
            contact(30, 3, CourtSide::Near),
            contact(40, 4, CourtSide::Near),
            contact(50, 5, CourtSide::Far),
        ];
        let summary = analyze(&c, &[]);
        assert_eq!(summary.max_same_side_touches, 4);
        assert_eq!(summary.three_touch_possessions, 1);
        assert_eq!(summary.over_three_touch_possessions, 1);
        assert_eq!(summary.side_switches, 1);
        assert_eq!(summary.events[4].possession_touches, 1);
        assert!(!summary.authoritative);
    }

    #[test]
    fn unknown_track_and_side_do_not_advance_possession_or_imply_serve() {
        let mut unknown = contact(15, 3, CourtSide::Near);
        unknown.track_id = None;
        unknown.side = None;
        unknown.kind = Some(ActionKind::Serve);
        let summary = analyze(&[unknown], &[]);
        assert_eq!(summary.serve_frame, None);
        assert_eq!(summary.max_same_side_touches, 0);
        assert_eq!(summary.events[0].estimated_kind, None);
    }
}
