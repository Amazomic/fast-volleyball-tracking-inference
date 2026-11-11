//! The CPU replay publishes only tracklets supported by enough real frames.
//! Tentative IDs remain usable for matching during occlusion, then the final
//! evidence pass removes unconfirmed trajectories from derived snapshots.

use super::TrackedPlayer;
use std::collections::BTreeMap;

/// Defends the offline confirmation pass from pathological GPU source volume.
const MAX_TRACKLETS: usize = 1_000_000;

#[derive(Default, Clone, Debug)]
struct Evidence {
    observed_frames: u64,
    last_frame: Option<u64>,
}

#[derive(Default)]
pub struct TrackConfirmation {
    candidates: BTreeMap<u64, Evidence>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrackConfirmationSummary {
    pub confirmed_tracks: u64,
    pub discarded_tracks: u64,
    pub confirmed_detections: u64,
    pub discarded_detections: u64,
}

impl TrackConfirmation {
    /// Observations are accepted only for uniquely identified, real detections.
    /// Occlusion frames are not counted as new evidence.
    pub fn observe(&mut self, frame: u64, players: &[TrackedPlayer]) -> Result<(), String> {
        for player in players {
            if player.track_id == 0 {
                return Err("Track ID must be positive".into());
            }
            if !self.candidates.contains_key(&player.track_id)
                && self.candidates.len() >= MAX_TRACKLETS
            {
                return Err("Too many provisional tracklets for bounded confirmation".into());
            }
            let evidence = self.candidates.entry(player.track_id).or_default();
            if evidence.last_frame.is_some_and(|last| frame <= last) {
                return Err("Repeated or out-of-order detection for Track ID".into());
            }
            evidence.observed_frames += 1;
            evidence.last_frame = Some(frame);
        }
        Ok(())
    }

    pub fn confirmed(&self, id: u64, min_observations: u64) -> bool {
        self.candidates
            .get(&id)
            .is_some_and(|e| e.observed_frames >= min_observations)
    }

    pub fn summary(&self, min_observations: u64) -> TrackConfirmationSummary {
        let mut result = TrackConfirmationSummary::default();
        for item in self.candidates.values() {
            if item.observed_frames >= min_observations {
                result.confirmed_tracks += 1;
                result.confirmed_detections += item.observed_frames;
            } else {
                result.discarded_tracks += 1;
                result.discarded_detections += item.observed_frames;
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fusion::Candidate;

    fn track(frame: u64, id: u64) -> TrackedPlayer {
        TrackedPlayer {
            track_id: id,
            detection: Candidate {
                frame_index: frame,
                class_name: "player".into(),
                score: 0.88,
                bbox_xyxy_norm: [0.1, 0.1, 0.2, 0.8],
                query_index: Some(0),
                touch_probability: None,
                head_point_norm: None,
            },
            is_verified_athlete: false,
        }
    }

    #[test]
    fn brief_ghost_is_removed_but_observed_athlete_survives() {
        let mut evidence = TrackConfirmation::default();
        for frame in 0..20 {
            let mut players = vec![track(frame, 1)];
            if (7..=9).contains(&frame) {
                players.push(track(frame, 2));
            }
            evidence.observe(frame, &players).unwrap();
        }
        assert!(evidence.confirmed(1, 12));
        assert!(!evidence.confirmed(2, 12));
        assert_eq!(
            evidence.summary(12),
            TrackConfirmationSummary {
                confirmed_tracks: 1,
                discarded_tracks: 1,
                confirmed_detections: 20,
                discarded_detections: 3,
            }
        );
    }

    #[test]
    fn intermittent_athlete_can_confirm_without_counting_missing_frames() {
        let mut evidence = TrackConfirmation::default();
        for frame in (0..24).step_by(2) {
            evidence.observe(frame, &[track(frame, 3)]).unwrap();
        }
        assert!(evidence.confirmed(3, 12));
        assert!(!evidence.confirmed(3, 13));
        assert_eq!(evidence.summary(13).discarded_detections, 12);
    }

    #[test]
    fn duplicate_or_non_monotone_observations_are_rejected() {
        let mut evidence = TrackConfirmation::default();
        assert!(evidence.observe(0, &[track(0, 7), track(0, 7)]).is_err());
        assert!(evidence.observe(0, &[track(0, 7)]).is_err());
        assert_eq!(evidence.summary(12).discarded_tracks, 1);
    }
}
