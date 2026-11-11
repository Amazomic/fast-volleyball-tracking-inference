//! Game actions are not derivable from a ball-velocity impulse alone.
//! A verified Touch stays unknown until an explicit human-reviewed action.
//! This is independent of verified athlete identity.
use serde::Serialize;

pub const REVIEWABLE_ACTIONS: [&str; 6] = [
    "serve",
    "attack",
    "block",
    "dig",
    "overhead_pass",
    "low_touch",
];
#[derive(Clone, Copy, Debug)]
pub struct ContactEvidence {
    pub frame_index: u64,
    pub provisional_track_id: Option<u64>,
    pub probability: f64,
    pub physical_motion_supported: bool,
    pub sha_verified: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct ReviewedGameAction {
    pub frame_index: u64,
    pub provisional_track_id: Option<u64>,
    pub action_kind: String,
    pub motion_supported: bool,
    pub action_verified: bool,
    pub action_source: &'static str,
    pub athlete_identity_verified: bool,
}
/// The only route to a named serve/attack/block/dig/pass is explicit manual
/// event review against SHA-bound physical contact evidence.
pub fn classify(
    evidence: ContactEvidence,
    manually_reviewed: Option<&str>,
) -> Result<ReviewedGameAction, &'static str> {
    if evidence.frame_index > 48 * 3600 * 30
        || evidence
            .provisional_track_id
            .is_some_and(|v| v == 0 || v > i64::MAX as u64)
        || !evidence.probability.is_finite()
        || !(0.0..=1.0).contains(&evidence.probability)
        || !evidence.sha_verified
    {
        return Err("Action classification requires a verified source contact");
    }
    let named = match manually_reviewed {
        Some(action)
            if REVIEWABLE_ACTIONS.contains(&action) && evidence.physical_motion_supported =>
        {
            Some(action)
        }
        Some(action) if !REVIEWABLE_ACTIONS.contains(&action) => {
            return Err("Unknown or speculative volleyball action")
        }
        Some(_) => return Err("Named action requires motion-supported Touch"),
        None => None,
    };
    Ok(ReviewedGameAction {
        frame_index: evidence.frame_index,
        provisional_track_id: evidence.provisional_track_id,
        action_kind: named.unwrap_or("unknown_touch").to_owned(),
        motion_supported: evidence.physical_motion_supported,
        action_verified: named.is_some(),
        action_source: if named.is_some() {
            "explicit_human_review"
        } else {
            "unclassified_motion"
        },
        athlete_identity_verified: false,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn source() -> ContactEvidence {
        ContactEvidence {
            frame_index: 65,
            provisional_track_id: Some(8),
            probability: 0.96,
            physical_motion_supported: true,
            sha_verified: true,
        }
    }
    #[test]
    fn motion_alone_does_not_fabricate_attack_serve_or_block() {
        let item = classify(source(), None).unwrap();
        assert_eq!(item.action_kind, "unknown_touch");
        assert!(!item.action_verified);
        assert!(!item.athlete_identity_verified);
        for kind in [
            "attack",
            "serve",
            "block",
            "dig",
            "overhead_pass",
            "low_touch",
        ] {
            let checked = classify(source(), Some(kind)).unwrap();
            assert_eq!(checked.action_kind, kind);
            assert!(checked.action_verified);
            assert_eq!(checked.action_source, "explicit_human_review");
            assert!(!checked.athlete_identity_verified);
        }
    }
    #[test]
    fn unverified_source_or_motion_does_not_claim_named_action() {
        let mut input = source();
        input.sha_verified = false;
        assert!(classify(input, Some("serve")).is_err());
        input = source();
        input.physical_motion_supported = false;
        assert!(classify(input, Some("block")).is_err());
        assert_eq!(classify(input, None).unwrap().action_kind, "unknown_touch");
        input = source();
        assert!(classify(input, Some("attack_if_fast")).is_err());
        input.probability = f64::NAN;
        assert!(classify(input, Some("attack")).is_err());
        input = source();
        input.provisional_track_id = Some(0);
        assert!(classify(input, Some("attack")).is_err());
    }
}
