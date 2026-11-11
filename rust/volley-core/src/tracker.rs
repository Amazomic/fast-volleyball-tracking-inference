//! Conservative CPU Track ID reconstruction from RAVEL per-frame boxes.
//! IDs are provisional tracklets, NOT verified athlete identities.
//! Pairing is globally optimal and one-to-one; crossings retain living IDs
//! while ambiguous geometry is reported, not misrepresented as confirmed identity.
use crate::{bbox_iou, fusion::Candidate};
use serde::{Deserialize, Serialize};
mod association;
mod confirmation;
pub use confirmation::{TrackConfirmation, TrackConfirmationSummary};

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TrackingOptions {
    /// Score threshold for RAVEL raw candidate boxes, replayable on CPU.
    pub min_player_score: f64,
    pub nms_iou: f64,
    /// Maximum disappearance in frames (90 frames = 3 seconds at the 30 FPS CPU replay rate).
    pub max_gap_frames: u64,
    pub min_assignment_score: f64,
    /// Mark close competing matches as ambiguous without creating extra IDs.
    pub ambiguity_margin: f64,
    /// Minimum number of frames with actual observations before publishing an ID.
    /// Short unconfirmed tracklets remain only in immutable GPU evidence.
    #[serde(default = "default_min_track_observations")]
    pub min_track_observations: u64,
}
const fn default_min_track_observations() -> u64 {
    12
}

impl Default for TrackingOptions {
    fn default() -> Self {
        Self {
            min_player_score: 0.30,
            nms_iou: 0.88,
            max_gap_frames: 90,
            min_assignment_score: 0.48,
            ambiguity_margin: 0.11,
            min_track_observations: default_min_track_observations(),
        }
    }
}
impl TrackingOptions {
    pub fn validate(mut self) -> Result<Self, String> {
        if !self.min_player_score.is_finite()
            || !(0.05..=1.0).contains(&self.min_player_score)
            || !self.nms_iou.is_finite()
            || !(0.6..=0.99).contains(&self.nms_iou)
            || !(1..=180).contains(&self.max_gap_frames)
            || !self.min_assignment_score.is_finite()
            || !(0.2..=1.0).contains(&self.min_assignment_score)
            || !self.ambiguity_margin.is_finite()
            || !(0.0..=0.5).contains(&self.ambiguity_margin)
            || !(6..=90).contains(&self.min_track_observations)
        {
            return Err("Invalid CPU tracker options".into());
        }
        // Accept older saved profiles, but never expire a provisional track
        // before the minimum three-second grace period after its last detection.
        self.max_gap_frames = self.max_gap_frames.max(90);
        Ok(self)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct TrackedPlayer {
    pub track_id: u64,
    pub detection: Candidate,
    pub is_verified_athlete: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct TrackedFrame {
    pub frame_index: u64,
    pub tracks: Vec<TrackedPlayer>,
    pub retained_boxes: usize,
    pub rejected_boxes: usize,
    pub ambiguous_detections: usize,
}
#[derive(Clone, Debug)]
struct ActiveTrack {
    id: u64,
    last_frame: u64,
    bbox: [f64; 4],
    prev: Option<(u64, [f64; 4])>,
}
fn center(b: [f64; 4]) -> [f64; 2] {
    [(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0]
}
fn height(b: [f64; 4]) -> f64 {
    b[3] - b[1]
}
fn distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}
/// Match a detection to a living track with velocity, size and position cues.
/// A vanished track remains eligible for the configured grace period. Matching
/// is still provisional: geometry cannot prove an athlete's real-world identity.
fn score_match(t: &ActiveTrack, c: &Candidate, frame: u64, config: TrackingOptions) -> Option<f64> {
    let gap = frame.checked_sub(t.last_frame)?;
    if gap == 0 || gap > config.max_gap_frames {
        return None;
    }
    let hb = height(c.bbox_xyxy_norm);
    let ha = height(t.bbox);
    if hb < 0.015 || ha < 0.015 {
        return None;
    }
    let ratio = ha.max(hb) / ha.min(hb);
    if ratio > 1.8 {
        return None;
    }
    let observed = center(c.bbox_xyxy_norm);
    let base = center(t.bbox);
    // Extrapolate a few frames, not the entire 3-second absence: velocity
    // over longer periods is unreliable when an athlete changes direction.
    let predicted = if let Some((previous_frame, previous_bbox)) = t.prev {
        let elapsed = t.last_frame.saturating_sub(previous_frame).max(1) as f64;
        let previous = center(previous_bbox);
        let scale = (gap as f64 / elapsed).min(8.0);
        [
            base[0] + (base[0] - previous[0]) * scale,
            base[1] + (base[1] - previous[1]) * scale,
        ]
    } else {
        base
    };
    let max_movement = (0.055 + 0.007 * gap as f64).min(0.55);
    let distance = distance(predicted, observed);
    if distance > max_movement {
        return None;
    }
    let overlap = bbox_iou(t.bbox, c.bbox_xyxy_norm);
    let spatial = (1.0 - distance / max_movement).clamp(0.0, 1.0);
    let size = (1.0 - (ratio - 1.0) / 0.8).clamp(0.0, 1.0);
    // Fresh tracks favor IoU; after an occlusion a missing IoU must not
    // automatically generate a new ID for the same moving player.
    let near = 0.25 * overlap + 0.58 * spatial + 0.17 * size;
    let distant = 0.10 * overlap + 0.65 * spatial + 0.25 * size;
    let mix = ((gap.saturating_sub(1)) as f64 / 20.0).min(1.0);
    let freshness = (gap.saturating_sub(1)) as f64 / config.max_gap_frames as f64;
    let score = near * (1.0 - mix) + distant * mix - 0.08 * freshness;
    let threshold = config.min_assignment_score - 0.12 * freshness;
    (score >= threshold.max(0.2)).then_some(score)
}

/// Streaming tracker with bounded active state. IDs start at 1; query_index
/// is NOT a global Track ID and is never used to cross nine-frame windows.
pub struct ProvisionalTracker {
    config: TrackingOptions,
    active: Vec<ActiveTrack>,
    next_id: u64,
    last_frame: Option<u64>,
    total_seen: u64,
    total_skipped: u64,
    total_ambiguous: u64,
}
impl ProvisionalTracker {
    pub fn new(config: TrackingOptions) -> Result<Self, String> {
        Ok(Self {
            config: config.validate()?,
            active: Vec::new(),
            next_id: 1,
            last_frame: None,
            total_seen: 0,
            total_skipped: 0,
            total_ambiguous: 0,
        })
    }
    pub fn process_frame(&mut self, frame: u64, raw: &[Candidate]) -> Result<TrackedFrame, String> {
        if self.last_frame.is_some_and(|last| frame <= last) {
            return Err("CPU tracking frames must be strictly increasing".into());
        }
        if raw.len() > 128 {
            return Err("Too many RAVEL candidates in a single frame".into());
        }
        if raw.iter().any(|c| {
            c.frame_index != frame
                || c.class_name != "player"
                || c.query_index.is_none()
                || c.bbox_xyxy_norm.iter().any(|v| !v.is_finite())
                || !c.score.is_finite()
                || c.score < 0.0
                || c.score > 1.0
        }) {
            return Err("Invalid player detection input frame".into());
        }
        self.active
            .retain(|t| frame.saturating_sub(t.last_frame) <= self.config.max_gap_frames);
        // NMS only suppresses near-identical raw proposals, preserving
        // genuinely overlapping but distinct contemporaneous players.
        let mut valid = raw
            .iter()
            .filter(|c| {
                c.score >= self.config.min_player_score
                    && c.bbox_xyxy_norm[0] >= 0.0
                    && c.bbox_xyxy_norm[1] >= 0.0
                    && c.bbox_xyxy_norm[2] <= 1.0
                    && c.bbox_xyxy_norm[3] <= 1.0
                    && c.bbox_xyxy_norm[2] > c.bbox_xyxy_norm[0]
                    && c.bbox_xyxy_norm[3] > c.bbox_xyxy_norm[1]
            })
            .cloned()
            .collect::<Vec<_>>();
        valid.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.query_index.cmp(&b.query_index))
        });
        let mut selected: Vec<Candidate> = Vec::new();
        for c in valid {
            if selected
                .iter()
                .any(|used| bbox_iou(used.bbox_xyxy_norm, c.bbox_xyxy_norm) > self.config.nms_iou)
            {
                continue;
            }
            selected.push(c);
        }
        let skipped = raw.len() - selected.len();
        self.total_skipped += skipped as u64;
        self.total_seen += selected.len() as u64;

        // A *global* one-to-one assignment avoids fragmentation at crossings:
        // uncertain proposals are matched to a plausible existing ID rather
        // than minting another ID whenever reciprocal best choices disagree.
        let scores: Vec<Vec<Option<f64>>> = selected
            .iter()
            .map(|candidate| {
                self.active
                    .iter()
                    .map(|track| score_match(track, candidate, frame, self.config))
                    .collect()
            })
            .collect();
        let assignments = association::assign(&scores);
        let ambiguous = scores
            .iter()
            .filter(|row| {
                let mut values = row.iter().filter_map(|score| *score).collect::<Vec<_>>();
                values.sort_by(|a, b| b.total_cmp(a));
                values.len() > 1 && values[0] - values[1] < self.config.ambiguity_margin
            })
            .count();
        let mut out = Vec::with_capacity(selected.len());
        for (ci, c) in selected.into_iter().enumerate() {
            let id = if let Some(ti) = assignments[ci] {
                let t = &mut self.active[ti];
                let id = t.id;
                t.prev = Some((t.last_frame, t.bbox));
                t.last_frame = frame;
                t.bbox = c.bbox_xyxy_norm;
                id
            } else {
                let id = self.next_id;
                self.next_id = self.next_id.checked_add(1).ok_or("Track ID overflow")?;
                self.active.push(ActiveTrack {
                    id,
                    last_frame: frame,
                    bbox: c.bbox_xyxy_norm,
                    prev: None,
                });
                id
            };
            out.push(TrackedPlayer {
                track_id: id,
                detection: c,
                is_verified_athlete: false,
            });
        }
        self.last_frame = Some(frame);
        self.total_ambiguous += ambiguous as u64;
        Ok(TrackedFrame {
            frame_index: frame,
            tracks: out,
            retained_boxes: raw.len() - skipped,
            rejected_boxes: skipped,
            ambiguous_detections: ambiguous,
        })
    }
    pub fn next_id(&self) -> u64 {
        self.next_id
    }
    pub fn totals(&self) -> (u64, u64, u64) {
        (self.total_seen, self.total_skipped, self.total_ambiguous)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn detection(frame: u64, query: usize, x: f64, score: f64) -> Candidate {
        Candidate {
            frame_index: frame,
            class_name: "player".into(),
            score,
            bbox_xyxy_norm: [x, 0.2, x + 0.10, 0.7],
            query_index: Some(query),
            touch_probability: Some(0.9),
            head_point_norm: Some([x + 0.05, 0.2]),
        }
    }
    #[test]
    fn stable_ids_despite_query_index_permutation() {
        let mut tracker = ProvisionalTracker::new(TrackingOptions::default()).unwrap();
        let a = tracker
            .process_frame(0, &[detection(0, 0, 0.1, 0.9), detection(0, 1, 0.7, 0.9)])
            .unwrap();
        let b = tracker
            .process_frame(
                1,
                &[detection(1, 1, 0.105, 0.9), detection(1, 0, 0.705, 0.9)],
            )
            .unwrap();
        let a_left = a
            .tracks
            .iter()
            .find(|t| t.detection.bbox_xyxy_norm[0] < 0.5)
            .unwrap()
            .track_id;
        let a_right = a
            .tracks
            .iter()
            .find(|t| t.detection.bbox_xyxy_norm[0] > 0.5)
            .unwrap()
            .track_id;
        let b_left = b
            .tracks
            .iter()
            .find(|t| t.detection.bbox_xyxy_norm[0] < 0.5)
            .unwrap()
            .track_id;
        let b_right = b
            .tracks
            .iter()
            .find(|t| t.detection.bbox_xyxy_norm[0] > 0.5)
            .unwrap()
            .track_id;
        assert_eq!(a_left, b_left);
        assert_eq!(a_right, b_right);
        assert_ne!(b_left, b_right);
        assert!(!b.tracks[0].is_verified_athlete);
        assert_eq!(b.tracks[0].detection.touch_probability, Some(0.9));
    }
    #[test]
    fn suppresses_duplicates_but_does_not_merge_distinct_simultaneous_boxes() {
        let mut tracker = ProvisionalTracker::new(TrackingOptions::default()).unwrap();
        let out = tracker
            .process_frame(
                0,
                &[
                    detection(0, 0, 0.11, 0.99),
                    detection(0, 1, 0.11, 0.6),
                    detection(0, 2, 0.145, 0.88),
                ],
            )
            .unwrap();
        assert_eq!(out.tracks.len(), 2);
        assert_ne!(out.tracks[0].track_id, out.tracks[1].track_id);
        assert_eq!(out.rejected_boxes, 1);
    }
    #[test]
    fn ambiguous_crossing_does_not_create_a_third_id() {
        let mut tracker = ProvisionalTracker::new(TrackingOptions::default()).unwrap();
        tracker
            .process_frame(0, &[detection(0, 0, 0.40, 0.9), detection(0, 1, 0.46, 0.9)])
            .unwrap();
        let out = tracker
            .process_frame(1, &[detection(1, 0, 0.43, 0.9)])
            .unwrap();
        assert_eq!(out.tracks.len(), 1);
        assert!(out.ambiguous_detections > 0);
        assert!(matches!(out.tracks[0].track_id, 1 | 2));
        assert_eq!(tracker.next_id(), 3);
    }
    #[test]
    fn absence_longer_than_three_seconds_creates_new_id() {
        let mut tracker = ProvisionalTracker::new(TrackingOptions::default()).unwrap();
        let a = tracker
            .process_frame(0, &[detection(0, 0, 0.1, 0.9)])
            .unwrap();
        let b = tracker
            .process_frame(91, &[detection(91, 0, 0.1, 0.9)])
            .unwrap();
        assert_ne!(a.tracks[0].track_id, b.tracks[0].track_id);
        assert_eq!(tracker.next_id(), 3);
    }
    #[test]
    fn preserves_distinct_players_through_short_occlusion_and_empty_frames() {
        let mut tracker = ProvisionalTracker::new(TrackingOptions::default()).unwrap();
        let a = tracker
            .process_frame(0, &[detection(0, 0, 0.1, 0.8)])
            .unwrap();
        tracker.process_frame(1, &[]).unwrap();
        let b = tracker
            .process_frame(3, &[detection(3, 0, 0.101, 0.8)])
            .unwrap();
        assert_eq!(a.tracks[0].track_id, b.tracks[0].track_id);
        assert!(tracker.process_frame(3, &[]).is_err());
    }
    #[test]
    fn crossing_players_keep_ids_through_detection_occlusion() {
        let mut tracker = ProvisionalTracker::new(TrackingOptions::default()).unwrap();
        let mut ids = [None; 2];
        for frame in 0..40 {
            let left_to_right = detection(frame, 0, 0.18 + frame as f64 * 0.012, 0.90);
            let right_to_left = detection(frame, 1, 0.68 - frame as f64 * 0.012, 0.90);
            let detections = if frame % 2 == 0 {
                vec![right_to_left, left_to_right]
            } else {
                vec![left_to_right, right_to_left]
            };
            let result = tracker.process_frame(frame, &detections).unwrap();
            for player in result.tracks {
                let query = player.detection.query_index.unwrap();
                if let Some(existing) = ids[query] {
                    assert_eq!(player.track_id, existing, "ID switch on frame {frame}");
                } else {
                    ids[query] = Some(player.track_id);
                }
            }
            assert_eq!(tracker.next_id(), 3, "fragmented at crossing frame {frame}");
        }
        assert_ne!(ids[0], ids[1]);
    }

    #[test]
    fn older_saved_tracking_profile_gets_default_confirmation() {
        let legacy = serde_json::json!({
            "min_player_score":0.3,"nms_iou":0.88,"max_gap_frames":12,
            "min_assignment_score":0.48,"ambiguity_margin":0.11
        });
        let settings: TrackingOptions = serde_json::from_value(legacy).unwrap();
        let settings = settings.validate().unwrap();
        assert_eq!(settings.max_gap_frames, 90);
        assert_eq!(settings.min_track_observations, 12);
        let bad = TrackingOptions {
            min_track_observations: 5,
            ..settings
        };
        assert!(bad.validate().is_err());
    }

    #[test]
    fn saved_legacy_twelve_frame_profile_is_upgraded_to_three_seconds() {
        let settings = TrackingOptions {
            max_gap_frames: 12,
            ..TrackingOptions::default()
        };
        assert_eq!(settings.validate().unwrap().max_gap_frames, 90);
        let tracker = ProvisionalTracker::new(settings).unwrap();
        assert_eq!(tracker.config.max_gap_frames, 90);
    }

    #[test]
    fn three_second_dropout_keeps_the_same_id() {
        let mut tracker = ProvisionalTracker::new(TrackingOptions::default()).unwrap();
        let initial = tracker
            .process_frame(0, &[detection(0, 0, 0.1, 0.95)])
            .unwrap();
        for frame in 1..90 {
            assert!(tracker.process_frame(frame, &[]).unwrap().tracks.is_empty());
        }
        let returned = tracker
            .process_frame(90, &[detection(90, 8, 0.30, 0.85)])
            .unwrap();
        assert_eq!(initial.tracks[0].track_id, returned.tracks[0].track_id);
        assert_eq!(tracker.next_id(), 2);
    }

    #[test]
    fn long_sequence_with_shuffled_proposals_does_not_fragment_players() {
        let mut tracker = ProvisionalTracker::new(TrackingOptions::default()).unwrap();
        for frame in 0..900 {
            let mut proposals = (0..6)
                .map(|i| {
                    let x =
                        0.06 + (i as f64) * 0.145 + (frame as f64 * 0.017 + i as f64).sin() * 0.012;
                    detection(frame, (i * 11 + frame as usize) % 6, x, 0.9)
                })
                .collect::<Vec<_>>();
            if frame % 2 == 0 {
                proposals.reverse();
            }
            let result = tracker.process_frame(frame, &proposals).unwrap();
            assert_eq!(result.tracks.len(), 6);
            assert_eq!(tracker.next_id(), 7, "fragmentation on frame {frame}");
        }
    }

    #[test]
    fn overlapping_tracks_remain_distinct_after_partial_occlusion() {
        let mut tracker = ProvisionalTracker::new(TrackingOptions::default()).unwrap();
        let start = tracker
            .process_frame(0, &[detection(0, 0, 0.3, 0.9), detection(0, 1, 0.5, 0.9)])
            .unwrap();
        tracker
            .process_frame(1, &[detection(1, 0, 0.31, 0.9)])
            .unwrap();
        let end = tracker
            .process_frame(2, &[detection(2, 1, 0.32, 0.9), detection(2, 0, 0.5, 0.9)])
            .unwrap();
        assert_eq!(tracker.next_id(), 3);
        let mut ids = end.tracks.iter().map(|p| p.track_id).collect::<Vec<_>>();
        ids.sort_unstable();
        assert_eq!(
            ids,
            start.tracks.iter().map(|p| p.track_id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn refuses_non_finite_and_overly_permissive_thresholds() {
        assert!(ProvisionalTracker::new(TrackingOptions {
            max_gap_frames: 181,
            ..TrackingOptions::default()
        })
        .is_err());
        let mut tracker = ProvisionalTracker::new(TrackingOptions::default()).unwrap();
        let mut invalid = detection(0, 0, 0.1, 0.9);
        invalid.score = f64::NAN;
        assert!(tracker.process_frame(0, &[invalid]).is_err());
    }
}
