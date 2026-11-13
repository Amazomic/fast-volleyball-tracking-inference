//! Conservative CPU Track ID reconstruction from RTMDet/RTMPose skeleton boxes.
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
    /// Score threshold for skeletal RTMDet/RTMPose boxes, replayable on CPU.
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
            min_player_score: 0.28,
            nms_iou: 0.72,
            max_gap_frames: 90,
            min_assignment_score: 0.43,
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
    head: Option<[f64; 2]>,
    observations: u64,
    prev: Option<(u64, [f64; 4])>,
}
fn center(b: [f64; 4]) -> [f64; 2] {
    [(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0]
}
fn height(b: [f64; 4]) -> f64 {
    b[3] - b[1]
}
/// Net occlusion often produces a second, torso-only pose for the same
/// distant head. Suppress it only when two independent head anchors nearly
/// coincide and the horizontal skeleton regions overlap substantially.
fn same_distant_skeleton(a: &Candidate, b: &Candidate) -> bool {
    let (Some(head_a), Some(head_b)) = (a.head_point_norm, b.head_point_norm) else {
        return false;
    };
    let box_a = a.bbox_xyxy_norm;
    let box_b = b.bbox_xyxy_norm;
    let short = height(box_a).min(height(box_b));
    if short > 0.18 || distance(head_a, head_b) > 0.012 {
        return false;
    }
    let shared_width = (box_a[2].min(box_b[2]) - box_a[0].max(box_b[0])).max(0.0);
    let narrow_width = (box_a[2] - box_a[0]).min(box_b[2] - box_b[0]);
    shared_width >= 0.55 * narrow_width
        && bbox_iou(box_a, box_b) > 0.12
        && (box_a[1] - box_b[1]).abs() < 0.045
}
/// Two differently cropped poses can share one body even when their box IoU
/// is only ~0.3: a foreground skeleton may lose its lower or upper joints.
/// Require aligned heads, a common horizontal body axis and real area overlap;
/// adjacent players and players aligned only by their x coordinate survive.
fn same_shifted_skeleton(a: &Candidate, b: &Candidate) -> bool {
    let (Some(ha), Some(hb)) = (a.head_point_norm, b.head_point_norm) else {
        return false;
    };
    let ba = a.bbox_xyxy_norm;
    let bb = b.bbox_xyxy_norm;
    let width_a = ba[2] - ba[0];
    let width_b = bb[2] - bb[0];
    let width = width_a.min(width_b);
    let min_height = height(ba).min(height(bb));
    if width < 0.025
        || min_height < 0.16
        || (height(ba) / height(bb)).max(height(bb) / height(ba)) > 1.55
    {
        return false;
    }
    let shared_width = (ba[2].min(bb[2]) - ba[0].max(bb[0])).max(0.0);
    let center_dx = (center(ba)[0] - center(bb)[0]).abs();
    let head_dx = (ha[0] - hb[0]).abs();
    let head_dy = (ha[1] - hb[1]).abs();
    shared_width > width * 0.82
        && center_dx < width * 0.20
        && head_dx < width * 0.24
        && head_dy < 0.085
        && bbox_iou(ba, bb) > 0.30
}
/// A weak top-only crop without a nose can be nested inside an established
/// torso pose. This is a duplicate, not evidence for a new athlete identity.
fn same_nested_upper_body(a: &Candidate, b: &Candidate) -> bool {
    if a.head_point_norm.is_some() && b.head_point_norm.is_some() {
        return false;
    }
    let ba = a.bbox_xyxy_norm;
    let bb = b.bbox_xyxy_norm;
    let ha = height(ba);
    let hb = height(bb);
    let short = ha.min(hb);
    let long = ha.max(hb);
    if short > 0.085 || long < 1.4 * short || (ba[1] - bb[1]).abs() > 0.018 {
        return false;
    }
    let shared_width = (ba[2].min(bb[2]) - ba[0].max(bb[0])).max(0.0);
    let shared_height = (ba[3].min(bb[3]) - ba[1].max(bb[1])).max(0.0);
    let small_width = (ba[2] - ba[0]).min(bb[2] - bb[0]);
    shared_width * shared_height > 0.80 * small_width * short
        && (center(ba)[0] - center(bb)[0]).abs() < 0.012
}
/// Same head and overlapping body mean two crops of one person, even if the
/// full/partial bounding boxes differ enough to evade ordinary box NMS.
/// Requiring a head on both proposals prevents merging close teammates
/// whose upper-body locations are merely adjacent.
pub fn same_head_body_duplicate(a: &Candidate, b: &Candidate) -> bool {
    let (Some(ha), Some(hb)) = (a.head_point_norm, b.head_point_norm) else {
        return false;
    };
    let (ba, bb) = (a.bbox_xyxy_norm, b.bbox_xyxy_norm);
    let head_dx = (ha[0] - hb[0]).abs();
    let head_dy = (ha[1] - hb[1]).abs();
    let shared_width = (ba[2].min(bb[2]) - ba[0].max(bb[0])).max(0.0);
    let narrow = (ba[2] - ba[0]).min(bb[2] - bb[0]);
    narrow > 0.012
        && head_dx < 0.012
        && head_dy < 0.018
        && shared_width > narrow * 0.55
        && bbox_iou(ba, bb) > 0.30
}
/// A lower-confidence partial skeleton can sit inside a fuller same-player
/// detection while its inferred nose drifts due to net occlusion. Require
/// substantial *small-box containment*, compatible heads, and a clear
/// score advantage; do not suppress neighboring complete players.
pub fn same_partial_body_duplicate(strong: &Candidate, weak: &Candidate) -> bool {
    let (Some(a_head), Some(b_head)) = (strong.head_point_norm, weak.head_point_norm) else {
        return false;
    };
    if strong.score - weak.score < 0.14 {
        return false;
    }
    let (a, b) = (strong.bbox_xyxy_norm, weak.bbox_xyxy_norm);
    let (aw, ah) = (a[2] - a[0], height(a));
    let (bw, bh) = (b[2] - b[0], height(b));
    if aw < 0.025 || bw < 0.025 || ah < 0.12 || bh < 0.12
        || ah.max(bh) / ah.min(bh) > 1.7
        || (a_head[0] - b_head[0]).abs() > 0.045
        || (a_head[1] - b_head[1]).abs() > 0.065
    {
        return false;
    }
    let width = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let vertical = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let smaller_area = (aw * ah).min(bw * bh);
    width * vertical >= 0.60 * smaller_area
        && (center(a)[0] - center(b)[0]).abs() < 0.035
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
    // A small far-court player can lose legs behind the net: RTMPose then
    // publishes a torso-only box, which should not force a fresh ID after a
    // short occlusion. Prolonged gaps still need matching position and head.
    let size_ratio_limit = if gap > 8 && ha.min(hb) < 0.15 {
        2.30
    } else {
        1.8
    };
    if ratio > size_ratio_limit {
        return None;
    }
    let observed = center(c.bbox_xyxy_norm);
    let base = center(t.bbox);
    // Extrapolate a few frames, not the entire 3-second absence: velocity
    // over longer periods is unreliable when an athlete changes direction.
    let predicted = if let Some((previous_frame, previous_bbox)) = t.prev {
        let elapsed = t.last_frame.saturating_sub(previous_frame).max(1) as f64;
        let previous = center(previous_bbox);
        // Predict motion only while the velocity estimate is trustworthy.
        // After a long occlusion, the last actual court location dominates.
        let velocity_weight = ((24.0 - gap as f64) / 24.0).clamp(0.0, 1.0);
        let observed_displacement = distance(base, previous) / elapsed;
        // Sudden RTMPose crop height changes look like motion, but are not
        // athlete velocity. They must not make a stable ID vanish on frame+1.
        let velocity_scale = if observed_displacement > 0.038 {
            0.0
        } else {
            1.0
        };
        let scale = (gap as f64 / elapsed).min(8.0) * velocity_weight * velocity_scale;
        [
            base[0] + (base[0] - previous[0]) * scale,
            base[1] + (base[1] - previous[1]) * scale,
        ]
    } else {
        base
    };
    // A player hidden behind the net stays in a bounded region of the court.
    // Keep the Track ID alive for 90 frames without matching an arbitrary
    // person half a screen away when the player reappears.
    let max_movement = (0.052 + 0.0032 * gap as f64).min(0.34);
    let distance = distance(predicted, observed);
    if distance > max_movement {
        return None;
    }
    let overlap = bbox_iou(t.bbox, c.bbox_xyxy_norm);
    let spatial = (1.0 - distance / max_movement).clamp(0.0, 1.0);
    let size = (1.0 - (ratio - 1.0) / (size_ratio_limit - 1.0)).clamp(0.0, 1.0);
    // Fresh tracks favor IoU; after an occlusion a missing IoU must not
    // automatically generate a new ID for the same moving player.
    let near = 0.25 * overlap + 0.58 * spatial + 0.17 * size;
    let distant = 0.10 * overlap + 0.65 * spatial + 0.25 * size;
    let mix = ((gap.saturating_sub(1)) as f64 / 20.0).min(1.0);
    let freshness = (gap.saturating_sub(1)) as f64 / config.max_gap_frames as f64;
    let mut score = near * (1.0 - mix) + distant * mix - 0.08 * freshness;
    // RTMPose's observed nose is an independent skeletal anchor. It helps
    // disambiguate overlapping skeleton-derived boxes, but missing/occluded
    // head points never invalidate a geometrically consistent track.
    if let (Some(previous), Some(head)) = (t.head, c.head_point_norm) {
        let threshold = (0.06 + gap as f64 * 0.007).min(0.50);
        let similarity = (1.0 - (previous[0] - head[0]).hypot(previous[1] - head[1]) / threshold)
            .clamp(0.0, 1.0);
        score = 0.82 * score + 0.18 * similarity;
    }
    let threshold = config.min_assignment_score - 0.08 * freshness;
    // Prefer a stable, repeatedly observed skeleton over a transient duplicate,
    // but only after the spatial gate and minimum similarity are satisfied.
    (score >= threshold.max(0.2)).then_some(score + 0.035 * (t.observations as f64 / 90.0).min(1.0))
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
            return Err("Too many player skeleton candidates in a single frame".into());
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
            if selected.iter().any(|used| {
                bbox_iou(used.bbox_xyxy_norm, c.bbox_xyxy_norm) > self.config.nms_iou
                    || same_distant_skeleton(used, &c)
                    || same_head_body_duplicate(used, &c)
                    || same_shifted_skeleton(used, &c)
                    || same_nested_upper_body(used, &c)
                    || same_partial_body_duplicate(used, &c)
            }) {
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
                t.head = c.head_point_norm;
                t.observations += 1;
                id
            } else {
                let id = self.next_id;
                self.next_id = self.next_id.checked_add(1).ok_or("Track ID overflow")?;
                self.active.push(ActiveTrack {
                    id,
                    last_frame: frame,
                    bbox: c.bbox_xyxy_norm,
                    head: c.head_point_norm,
                    observations: 1,
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
    fn rejects_shorter_duplicate_near_timur_at_10_7s() {
        let mut tracker = ProvisionalTracker::new(TrackingOptions::default()).unwrap();
        let mut full = detection(321, 0, 0.227, 0.74);
        full.bbox_xyxy_norm = [0.227, 0.700, 0.336, 0.997];
        full.head_point_norm = Some([0.282, 0.728]);
        let mut crop = detection(321, 1, 0.271, 0.54);
        crop.bbox_xyxy_norm = [0.271, 0.641, 0.343, 0.862];
        crop.head_point_norm = Some([0.314, 0.686]);
        assert!(same_partial_body_duplicate(&full, &crop));
        let result = tracker.process_frame(321, &[crop.clone(), full.clone()]).unwrap();
        assert_eq!(result.tracks.len(), 1, "crop cannot mint second ID");
        assert_eq!(result.tracks[0].detection.query_index, full.query_index);
        crop.head_point_norm = Some([0.385, 0.686]);
        assert!(!same_partial_body_duplicate(&full, &crop));
        let mut next = full.clone();
        let mut other = crop.clone();
        next.frame_index = 322;
        other.frame_index = 322;
        let separate = tracker.process_frame(322, &[next, other]).unwrap();
        assert_eq!(separate.tracks.len(), 2);
    }
    #[test]
    fn reconnects_after_occlusion_or_pose_crop_size_change() {
        let options = TrackingOptions::default();
        let make = |frame_index, bbox_xyxy_norm| Candidate {
            frame_index,
            class_name: "player".into(),
            score: 0.61,
            bbox_xyxy_norm,
            query_index: Some(0),
            touch_probability: None,
            head_point_norm: None,
        };
        let before = ActiveTrack {
            id: 6,
            last_frame: 482,
            bbox: [0.639, 0.686, 0.684, 0.925],
            head: None,
            observations: 12,
            prev: Some((481, [0.633, 0.724, 0.682, 0.996])),
        };
        assert!(
            score_match(
                &before,
                &make(483, [0.642, 0.704, 0.684, 0.965]),
                483,
                options,
            )
            .is_some(),
            "Cropped athlete at 482/483 must keep same ID"
        );
        let torso = ActiveTrack {
            id: 4,
            last_frame: 368,
            bbox: [0.493, 0.437, 0.529, 0.499],
            head: None,
            observations: 12,
            prev: None,
        };
        assert!(
            score_match(
                &torso,
                &make(408, [0.482, 0.438, 0.507, 0.551]),
                408,
                options,
            )
            .is_some(),
            "Missing legs behind net must not force another ID"
        );
    }

    #[test]
    fn reacquires_nearby_same_player_after_three_second_net_occlusion() {
        let mut tracker = ProvisionalTracker::new(TrackingOptions::default()).unwrap();
        let before = tracker
            .process_frame(0, &[detection(0, 0, 0.42, 0.7)])
            .unwrap();
        let id = before.tracks[0].track_id;
        for frame in 1..90 {
            assert!(tracker.process_frame(frame, &[]).unwrap().tracks.is_empty());
        }
        let reacquired = tracker
            .process_frame(90, &[detection(90, 0, 0.46, 0.7)])
            .unwrap();
        assert_eq!(reacquired.tracks[0].track_id, id);
        let too_far = tracker
            .process_frame(91, &[detection(91, 0, 0.86, 0.7)])
            .unwrap();
        assert_ne!(too_far.tracks[0].track_id, id);
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
    fn missing_head_cropped_upper_body_does_not_create_an_id() {
        let mut primary = detection(355, 0, 0.445, 0.51);
        primary.bbox_xyxy_norm = [0.445, 0.450, 0.480, 0.550];
        primary.head_point_norm = Some([0.460, 0.462]);
        let mut partial = detection(355, 1, 0.441, 0.29);
        partial.bbox_xyxy_norm = [0.441, 0.450, 0.480, 0.516];
        partial.head_point_norm = None;
        let mut tracker = ProvisionalTracker::new(TrackingOptions::default()).unwrap();
        let output = tracker.process_frame(355, &[primary, partial]).unwrap();
        assert_eq!(output.tracks.len(), 1);
        assert_eq!(output.rejected_boxes, 1);
    }

    #[test]
    fn frame_360_same_foreground_athlete_keeps_single_track_id() {
        let mut tracker = ProvisionalTracker::new(TrackingOptions::default()).unwrap();
        let mut primary = detection(360, 0, 0.24, 0.71);
        primary.bbox_xyxy_norm = [0.236, 0.671, 0.330, 0.946];
        primary.head_point_norm = Some([0.272616, 0.731814]);
        let mut duplicate = detection(360, 1, 0.26, 0.54);
        duplicate.bbox_xyxy_norm = [0.258, 0.596, 0.320, 0.852];
        duplicate.head_point_norm = Some([0.284140, 0.671675]);
        let first = tracker
            .process_frame(360, &[primary.clone(), duplicate.clone()])
            .unwrap();
        assert_eq!(
            first.tracks.len(),
            1,
            "two poses must not create two athlete IDs"
        );
        assert_eq!(first.rejected_boxes, 1);
        primary.frame_index = 361;
        duplicate.frame_index = 361;
        let next = tracker.process_frame(361, &[primary, duplicate]).unwrap();
        assert_eq!(next.tracks.len(), 1);
        assert_eq!(next.tracks[0].track_id, first.tracks[0].track_id);
    }

    #[test]
    fn overlapping_far_pose_crops_with_same_head_are_one_player() {
        let mut first = detection(0, 0, 0.42, 0.87);
        first.bbox_xyxy_norm = [0.40, 0.44, 0.46, 0.64];
        first.head_point_norm = Some([0.43, 0.45]);
        let mut duplicate = detection(0, 1, 0.42, 0.68);
        duplicate.bbox_xyxy_norm = [0.415, 0.445, 0.455, 0.555];
        duplicate.head_point_norm = Some([0.431, 0.451]);
        let mut distinct = detection(0, 2, 0.55, 0.8);
        distinct.bbox_xyxy_norm = [0.53, 0.44, 0.59, 0.64];
        distinct.head_point_norm = Some([0.55, 0.45]);
        let mut tracker = ProvisionalTracker::new(TrackingOptions::default()).unwrap();
        let frame = tracker
            .process_frame(0, &[first, duplicate, distinct])
            .unwrap();
        assert_eq!(frame.tracks.len(), 2);
        assert_eq!(frame.rejected_boxes, 1);
    }

    #[test]
    fn duplicate_proposals_keep_existing_track_id_across_frames() {
        let mut tracker = ProvisionalTracker::new(TrackingOptions::default()).unwrap();
        let mut full = detection(0, 0, 0.50, 0.83);
        full.bbox_xyxy_norm = [0.50, 0.40, 0.54, 0.55];
        full.head_point_norm = Some([0.52, 0.41]);
        let old = tracker.process_frame(0, &[full.clone()]).unwrap().tracks[0].track_id;
        full.frame_index = 1;
        let mut part = detection(1, 1, 0.502, 0.68);
        part.bbox_xyxy_norm = [0.502, 0.40, 0.538, 0.48];
        part.head_point_norm = Some([0.521, 0.41]);
        let next = tracker.process_frame(1, &[full, part]).unwrap();
        assert_eq!(next.tracks.len(), 1);
        assert_eq!(next.tracks[0].track_id, old);
        assert_eq!(tracker.next_id(), 2);
    }
    #[test]
    fn partial_body_with_same_head_does_not_create_another_track() {
        let mut full = detection(0, 0, 0.50, 0.85);
        full.bbox_xyxy_norm = [0.50, 0.40, 0.54, 0.55];
        full.head_point_norm = Some([0.52, 0.41]);
        let mut upper = detection(0, 1, 0.502, 0.72);
        upper.bbox_xyxy_norm = [0.502, 0.40, 0.538, 0.48];
        upper.head_point_norm = Some([0.521, 0.41]);
        assert!(same_head_body_duplicate(&full, &upper));
        assert!(bbox_iou(full.bbox_xyxy_norm, upper.bbox_xyxy_norm) < 0.65);
        let mut tracker = ProvisionalTracker::new(TrackingOptions::default()).unwrap();
        let out = tracker.process_frame(0, &[full.clone(), upper.clone()]).unwrap();
        assert_eq!(out.tracks.len(), 1);
        assert_eq!(out.rejected_boxes, 1);
        upper.head_point_norm = Some([0.555, 0.42]);
        assert!(!same_head_body_duplicate(&full, &upper));
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
