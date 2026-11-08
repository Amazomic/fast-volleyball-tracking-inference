//! Conservative CPU Track ID reconstruction from RAVEL per-frame boxes.
//! IDs are provisional tracklets, NOT verified athlete identities.
//! Pairing is one-to-one, reciprocal, and deliberately refuses ambiguous
//! player crossings rather than collapsing simultaneous players.
use crate::{bbox_iou, fusion::Candidate};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TrackingOptions {
    /// Score threshold for RAVEL raw candidate boxes, replayable on CPU.
    pub min_player_score: f64,
    pub nms_iou: f64,
    /// Gaps longer than this always create new unconfirmed tracklets.
    pub max_gap_frames: u64,
    pub min_assignment_score: f64,
    /// Reciprocal best-vs-second-best confidence margin on *both* sides.
    pub ambiguity_margin: f64,
}

impl Default for TrackingOptions {
    fn default() -> Self {
        Self {
            min_player_score: 0.30,
            nms_iou: 0.88,
            max_gap_frames: 12,
            min_assignment_score: 0.48,
            ambiguity_margin: 0.11,
        }
    }
}
impl TrackingOptions {
    pub fn validate(self) -> Result<Self, String> {
        if !self.min_player_score.is_finite()
            || !(0.05..=1.0).contains(&self.min_player_score)
            || !self.nms_iou.is_finite()
            || !(0.6..=0.99).contains(&self.nms_iou)
            || !(1..=30).contains(&self.max_gap_frames)
            || !self.min_assignment_score.is_finite()
            || !(0.2..=1.0).contains(&self.min_assignment_score)
            || !self.ambiguity_margin.is_finite()
            || !(0.0..=0.5).contains(&self.ambiguity_margin)
        {
            return Err("Invalid CPU tracker options".into());
        }
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
#[derive(Clone, Copy, Debug)]
struct Edge {
    track_idx: usize,
    candidate_idx: usize,
    score: f64,
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
/// Short, bounded, cautious velocity extrapolation is used only for gating.
/// It does not invent new detections or extend an identity beyond max_gap.
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
    if ratio > 1.48 {
        return None;
    }
    let base = center(t.bbox);
    let target = center(c.bbox_xyxy_norm);
    let predicted = if let Some((last_frame, b)) = t.prev {
        let elapsed = (t.last_frame - last_frame).max(1) as f64;
        let prev = center(b);
        let delta = (gap as f64 / elapsed).min(2.0);
        // Never trust unbounded extrapolation for a long absence.
        [
            base[0] + (base[0] - prev[0]) * delta,
            base[1] + (base[1] - prev[1]) * delta,
        ]
    } else {
        base
    };
    let gap_f = gap as f64;
    let max_movement = 0.034 + 0.007 * gap_f.min(12.0);
    let d = distance(predicted, target);
    if d > max_movement {
        return None;
    }
    let overlap = bbox_iou(t.bbox, c.bbox_xyxy_norm);
    // IoU + predicted-centre distance + height similarity. Actual overlap
    // is the strongest cue; predicted position is a modest tie-breaker.
    let spatial = (1.0 - d / max_movement).clamp(0.0, 1.0);
    let size = (1.0 - (ratio - 1.0) / 0.48).clamp(0.0, 1.0);
    let raw = 0.49 * overlap + 0.34 * spatial + 0.17 * size;
    // Do not silently bridge a long occlusion on geometry alone.
    let penalty = ((gap.saturating_sub(1)) as f64 * 0.018).min(0.25);
    let score = raw - penalty;
    (score >= config.min_assignment_score).then_some(score)
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

        let mut edges: Vec<Edge> = Vec::new();
        for (ti, track) in self.active.iter().enumerate() {
            for (ci, candidate) in selected.iter().enumerate() {
                if let Some(score) = score_match(track, candidate, frame, self.config) {
                    edges.push(Edge {
                        track_idx: ti,
                        candidate_idx: ci,
                        score,
                    });
                }
            }
        }
        // Require a reciprocal, unique best pairing. Equal-score crossing
        // creates new tracklets rather than inventing confident identity.
        let mut by_track: BTreeMap<usize, Vec<Edge>> = BTreeMap::new();
        let mut by_candidate: BTreeMap<usize, Vec<Edge>> = BTreeMap::new();
        for e in edges {
            by_track.entry(e.track_idx).or_default().push(e);
            by_candidate.entry(e.candidate_idx).or_default().push(e);
        }
        for choices in by_track.values_mut() {
            choices.sort_by(|a, b| {
                b.score
                    .total_cmp(&a.score)
                    .then_with(|| a.candidate_idx.cmp(&b.candidate_idx))
            });
        }
        for choices in by_candidate.values_mut() {
            choices.sort_by(|a, b| {
                b.score
                    .total_cmp(&a.score)
                    .then_with(|| a.track_idx.cmp(&b.track_idx))
            });
        }
        let mut assignments: BTreeMap<usize, usize> = BTreeMap::new();
        let mut ambiguous = HashSet::new();
        for (ci, candidates) in &by_candidate {
            let strongest = candidates[0];
            let track_choices = &by_track[&strongest.track_idx];
            let margin_ok = candidates.get(1).is_none_or(|second| {
                strongest.score - second.score >= self.config.ambiguity_margin
            }) && track_choices.get(1).is_none_or(|second| {
                strongest.score - second.score >= self.config.ambiguity_margin
            });
            if margin_ok && track_choices[0].candidate_idx == *ci {
                assignments.insert(*ci, strongest.track_idx);
            } else {
                ambiguous.insert(*ci);
            }
        }
        let mut out = Vec::with_capacity(selected.len());
        for (ci, c) in selected.into_iter().enumerate() {
            let id = if let Some(&ti) = assignments.get(&ci) {
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
        self.total_ambiguous += ambiguous.len() as u64;
        Ok(TrackedFrame {
            frame_index: frame,
            tracks: out,
            retained_boxes: raw.len() - skipped,
            rejected_boxes: skipped,
            ambiguous_detections: ambiguous.len(),
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
    fn ambiguous_crossing_never_forces_id_reassignment() {
        let mut tracker = ProvisionalTracker::new(TrackingOptions::default()).unwrap();
        tracker
            .process_frame(0, &[detection(0, 0, 0.40, 0.9), detection(0, 1, 0.46, 0.9)])
            .unwrap();
        let out = tracker
            .process_frame(1, &[detection(1, 0, 0.43, 0.9)])
            .unwrap();
        assert_eq!(out.tracks.len(), 1);
        assert!(out.ambiguous_detections > 0);
        assert_eq!(out.tracks[0].track_id, 3);
    }
    #[test]
    fn long_gap_creates_new_id_not_fake_four_second_identity() {
        let mut tracker = ProvisionalTracker::new(TrackingOptions::default()).unwrap();
        let a = tracker
            .process_frame(0, &[detection(0, 0, 0.1, 0.9)])
            .unwrap();
        let b = tracker
            .process_frame(50, &[detection(50, 0, 0.1, 0.9)])
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
    fn refuses_non_finite_and_overly_permissive_thresholds() {
        assert!(ProvisionalTracker::new(TrackingOptions {
            max_gap_frames: 120,
            ..TrackingOptions::default()
        })
        .is_err());
        let mut tracker = ProvisionalTracker::new(TrackingOptions::default()).unwrap();
        let mut invalid = detection(0, 0, 0.1, 0.9);
        invalid.score = f64::NAN;
        assert!(tracker.process_frame(0, &[invalid]).is_err());
    }
}
