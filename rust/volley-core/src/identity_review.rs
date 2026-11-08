//! Canonical human-reviewed athlete/Track ID mutation policy.
//!
//! Adapters validate evidence, acquire locks, enforce compare-and-swap and
//! convert their on-disk formats. The decision algorithm runs once here.
use std::collections::{BTreeMap, BTreeSet};

use crate::evidence_source::VerifiedTrackSet;

const MAX_IDS_PER_ACTION: usize = 200;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReviewAction {
    Assign,
    Unassign,
    Exclude,
    Restore,
}
impl ReviewAction {
    pub fn parse(raw: &str) -> Result<Self, ReviewError> {
        match raw {
            "assign" => Ok(Self::Assign),
            "unassign" => Ok(Self::Unassign),
            "exclude" => Ok(Self::Exclude),
            "restore" => Ok(Self::Restore),
            _ => Err(ReviewError::invalid("Unsupported manual identity action")),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReviewErrorKind {
    Invalid,
    Conflict,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewError {
    pub kind: ReviewErrorKind,
    pub message: &'static str,
}
impl ReviewError {
    fn invalid(message: &'static str) -> Self {
        Self {
            kind: ReviewErrorKind::Invalid,
            message,
        }
    }
    fn conflict(message: &'static str) -> Self {
        Self {
            kind: ReviewErrorKind::Conflict,
            message,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ManualReviewState {
    /// A verified Track ID has at most one manually confirmed athlete owner.
    pub assignments: BTreeMap<u64, String>,
    pub excluded: BTreeSet<u64>,
    /// Legacy auto-suggestion denial preferences; restoring visibility retains these.
    pub rejected: BTreeSet<u64>,
}

impl ManualReviewState {
    /// Entire batch is validated before mutation. Never trusts numeric IDs
    /// outside the caller's source-scoped verified Track ID set.
    pub fn apply(
        &mut self,
        known: &VerifiedTrackSet,
        roster: &BTreeMap<String, String>,
        action: ReviewAction,
        ids: &[u64],
        owner: Option<&str>,
        allow_reassign: bool,
    ) -> Result<(), ReviewError> {
        if ids.is_empty() || ids.len() > MAX_IDS_PER_ACTION {
            return Err(ReviewError::invalid("Select 1–200 Track IDs"));
        }
        if ids.iter().any(|id| !known.contains_number(*id)) {
            return Err(ReviewError::invalid(
                "Track ID not found in selected verified source",
            ));
        }
        let selected = ids.iter().copied().collect::<BTreeSet<_>>();
        if selected.len() != ids.len() {
            return Err(ReviewError::invalid("Duplicate Track ID in request"));
        }
        let verified_owner = match action {
            ReviewAction::Assign => {
                let owner =
                    owner.ok_or_else(|| ReviewError::invalid("Select an existing player"))?;
                if !roster.contains_key(owner) {
                    return Err(ReviewError::invalid("Player is not in the project roster"));
                }
                Some(owner)
            }
            _ => {
                if owner.is_some() {
                    return Err(ReviewError::invalid(
                        "Player ID is only valid for assignment",
                    ));
                }
                None
            }
        };
        if selected.iter().any(|id| {
            self.assignments
                .get(id)
                .is_some_and(|player| !roster.contains_key(player))
                || (self.excluded.contains(id) && self.assignments.contains_key(id))
        }) {
            return Err(ReviewError::invalid(
                "Stored manual links are stale or conflicting",
            ));
        }
        for id in &selected {
            match action {
                ReviewAction::Assign => {
                    if self.excluded.contains(id) {
                        return Err(ReviewError::conflict(
                            "Restore excluded Track ID before assignment",
                        ));
                    }
                    if self
                        .assignments
                        .get(id)
                        .is_some_and(|current| Some(current.as_str()) != verified_owner)
                        && !allow_reassign
                    {
                        return Err(ReviewError::conflict(
                            "Track ID already assigned; explicit reassignment required",
                        ));
                    }
                }
                ReviewAction::Restore if !self.excluded.contains(id) => {
                    return Err(ReviewError::conflict(
                        "Only excluded Track IDs can be restored",
                    ));
                }
                _ => {}
            }
        }
        for id in selected {
            match action {
                ReviewAction::Assign => {
                    self.assignments
                        .insert(id, verified_owner.expect("validated owner").to_owned());
                    self.rejected.remove(&id);
                }
                ReviewAction::Unassign => {
                    self.assignments.remove(&id);
                    self.rejected.insert(id);
                }
                ReviewAction::Exclude => {
                    self.assignments.remove(&id);
                    self.excluded.insert(id);
                    self.rejected.insert(id);
                }
                ReviewAction::Restore => {
                    self.excluded.remove(&id);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence_source::{EvidenceSource, FrameRate, SourceStamp};
    fn source(native: bool) -> VerifiedTrackSet {
        let stamp = SourceStamp {
            bytes: 45,
            mtime_ns: 12,
        };
        let evidence = if native {
            EvidenceSource::native(
                "123456abcdef",
                "video.mp4",
                "0123456789abcdef0123456789abcdef",
                &"b".repeat(64),
                stamp,
                FrameRate::Exact {
                    numerator: 30,
                    denominator: 1,
                },
            )
            .unwrap()
        } else {
            EvidenceSource::legacy(
                "123456abcdef",
                "video.mp4",
                "run_20261107_123456",
                &"a".repeat(32),
                stamp,
                FrameRate::Unknown,
            )
            .unwrap()
        };
        VerifiedTrackSet::new(evidence, [5, 8]).unwrap()
    }
    fn roster() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("athlete1".to_owned(), "A".to_owned()),
            ("athlete2".to_owned(), "B".to_owned()),
        ])
    }
    #[test]
    fn both_evidence_formats_use_one_manual_policy() {
        for native in [true, false] {
            let known = source(native);
            let mut state = ManualReviewState::default();
            state
                .apply(
                    &known,
                    &roster(),
                    ReviewAction::Assign,
                    &[5],
                    Some("athlete1"),
                    false,
                )
                .unwrap();
            assert_eq!(
                state.assignments.get(&5).map(String::as_str),
                Some("athlete1")
            );
            let before = state.clone();
            let error = state
                .apply(
                    &known,
                    &roster(),
                    ReviewAction::Assign,
                    &[5],
                    Some("athlete2"),
                    false,
                )
                .unwrap_err();
            assert_eq!(error.kind, ReviewErrorKind::Conflict);
            assert_eq!(state, before);
            state
                .apply(
                    &known,
                    &roster(),
                    ReviewAction::Assign,
                    &[5],
                    Some("athlete2"),
                    true,
                )
                .unwrap();
            state
                .apply(&known, &roster(), ReviewAction::Exclude, &[5], None, false)
                .unwrap();
            assert!(!state.assignments.contains_key(&5));
            assert!(state.excluded.contains(&5));
            assert!(state.rejected.contains(&5));
            assert!(state
                .apply(
                    &known,
                    &roster(),
                    ReviewAction::Assign,
                    &[5],
                    Some("athlete1"),
                    true
                )
                .is_err());
            state
                .apply(&known, &roster(), ReviewAction::Restore, &[5], None, false)
                .unwrap();
            assert!(state.rejected.contains(&5));
            assert!(state
                .apply(&known, &roster(), ReviewAction::Restore, &[5], None, false)
                .is_err());
            state
                .apply(&known, &roster(), ReviewAction::Unassign, &[5], None, false)
                .unwrap();
            state
                .apply(
                    &known,
                    &roster(),
                    ReviewAction::Assign,
                    &[5],
                    Some("athlete1"),
                    false,
                )
                .unwrap();
            assert!(!state.rejected.contains(&5));
        }
    }
    #[test]
    fn malformed_batch_is_atomic() {
        let known = source(true);
        let mut state = ManualReviewState::default();
        for ids in [&[5, 5][..], &[5, 99], &[]] {
            assert!(state
                .apply(
                    &known,
                    &roster(),
                    ReviewAction::Assign,
                    ids,
                    Some("athlete1"),
                    false
                )
                .is_err());
            assert_eq!(state, ManualReviewState::default());
        }
        assert!(state
            .apply(
                &known,
                &roster(),
                ReviewAction::Assign,
                &[5],
                Some("unknown"),
                false
            )
            .is_err());
        assert!(state
            .apply(
                &known,
                &roster(),
                ReviewAction::Exclude,
                &[5],
                Some("athlete1"),
                false
            )
            .is_err());
        assert_eq!(state, ManualReviewState::default());
    }
    #[test]
    fn stale_owner_and_conflicting_sidecar_cannot_be_modified() {
        let known = source(false);
        let mut state = ManualReviewState::default();
        state.assignments.insert(5, "removed".into());
        assert!(state
            .apply(&known, &roster(), ReviewAction::Unassign, &[5], None, false)
            .is_err());
        state.assignments.insert(5, "athlete1".into());
        state.excluded.insert(5);
        assert!(state
            .apply(&known, &roster(), ReviewAction::Restore, &[5], None, false)
            .is_err());
    }
}
