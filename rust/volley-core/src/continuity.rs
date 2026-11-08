//! CPU-only candidate continuity between tracked RAVEL player segments.
//!
//! Short-gap geometry replicates the conservative Python
//! player_continuity.group_tracklets policy, without writing identity labels.
//! Gaps up to four seconds are scored as *suggestions only*, never merged.
//! The real player identity is maintained separately by explicit confirmation.

use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Clone, Copy, Debug)]
pub struct PlayerDetection {
    pub frame: u32,
    pub track_id: i64,
    pub score: f64,
    pub bbox: [f64; 4],
    pub interpolated: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct ContinuityConfig {
    pub width: f64,
    pub height: f64,
    pub fps: f64,
    pub extended_gap_sec: f64,
}

impl ContinuityConfig {
    pub fn validate(self) -> Result<(), &'static str> {
        if !self.width.is_finite()
            || !self.height.is_finite()
            || self.width < 1.0
            || self.height < 1.0
            || !self.fps.is_finite()
            || !(10.0..=120.0).contains(&self.fps)
            || !self.extended_gap_sec.is_finite()
            || !(0.7..=4.0).contains(&self.extended_gap_sec)
        {
            return Err("Invalid video geometry, FPS or extended gap");
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct Tracklet {
    id: i64,
    first: u32,
    last: u32,
    first_center: [f64; 2],
    last_center: [f64; 2],
    first_height: f64,
    last_height: f64,
    frames: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct ContinuityLink {
    pub from_track_id: String,
    pub to_track_id: String,
    pub score: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct CandidateGroup {
    pub id: String,
    pub track_ids: Vec<String>,
    pub confirmed: bool,
    pub evidence: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct ContinuityReport {
    pub schema: &'static str,
    pub confirmed: bool,
    pub method: &'static str,
    pub raw_track_ids: usize,
    pub candidate_groups: usize,
    pub proposed_links: Vec<ContinuityLink>,
    pub track_to_candidate: BTreeMap<String, String>,
    pub groups: Vec<CandidateGroup>,
    pub extended_gap_sec: f64,
    /// Stronger-than-default geometry, without identity assignment.
    pub extended_suggestions: Vec<ContinuityLink>,
    pub discarded_detections: usize,
    pub warning: &'static str,
}

#[derive(Clone, Copy, Debug)]
struct Edge {
    from: i64,
    to: i64,
    score: f64,
}

fn valid_bbox(b: &[f64; 4]) -> bool {
    b.iter().all(|v| v.is_finite()) && b[2] > b[0] && b[3] > b[1]
}

fn distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn height_ratio(a: &Tracklet, b: &Tracklet) -> Option<f64> {
    let smaller = a.last_height.min(b.first_height);
    if smaller <= 0.0 {
        return None;
    }
    Some(a.last_height.max(b.first_height) / smaller)
}

fn edge(
    gap: u32,
    a: &Tracklet,
    b: &Tracklet,
    short_max: u32,
    extended: u32,
) -> Option<(f64, bool)> {
    if gap == 0 || gap > extended {
        return None;
    }
    let ratio = height_ratio(a, b)?;
    let dist = distance(a.last_center, b.first_center);
    if gap <= short_max {
        if ratio > 1.18 || dist > 0.040 {
            return None;
        }
        // Exact original Python coefficients: a score is a heuristic,
        // not an appearance embedding or a calibrated probability.
        let score = 0.70 * (1.0 - dist / 0.040)
            + 0.20 * (1.0 - gap as f64 / 21.0)
            + 0.10 * (1.0 - (ratio - 1.0) / 0.18);
        (score >= 0.75).then_some((score, false))
    } else {
        // For 0.7–4 seconds there is no high-confidence identity evidence.
        // Only surface near-stationary, similarly sized *manual suggestions*.
        if ratio > 1.09 || dist > 0.018 || a.frames < 10 || b.frames < 10 {
            return None;
        }
        let gap_fraction = (gap - short_max) as f64 / (extended - short_max).max(1) as f64;
        let score = 0.78 * (1.0 - dist / 0.018)
            + 0.16 * (1.0 - (ratio - 1.0) / 0.09)
            + 0.06 * (1.0 - gap_fraction);
        (score >= 0.82).then_some((score, true))
    }
}

/// Reciprocal best match with >=.12 margin to the second-best candidate.
/// A fast approach to a dense, changing player lineup must remain unassigned.
fn reciprocal_unique(edges: &[Edge], margin: f64) -> Vec<Edge> {
    let mut outgoing: HashMap<i64, Vec<Edge>> = HashMap::new();
    let mut incoming: HashMap<i64, Vec<Edge>> = HashMap::new();
    for e in edges {
        outgoing.entry(e.from).or_default().push(*e);
        incoming.entry(e.to).or_default().push(*e);
    }
    let mut accepted = Vec::new();
    for (from, mut options) in outgoing {
        options.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.to.cmp(&b.to)));
        let strongest = options[0];
        if options.len() > 1 && strongest.score - options[1].score < margin {
            continue;
        }
        let Some(targets) = incoming.get(&strongest.to) else {
            continue;
        };
        let mut targets = targets.clone();
        targets.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.from.cmp(&b.from))
        });
        if targets[0].from != from
            || (targets.len() > 1 && strongest.score - targets[1].score < margin)
        {
            continue;
        }
        accepted.push(strongest);
    }
    accepted.sort_by_key(|e| (e.from, e.to));
    accepted
}

fn serializable_edge(edge: Edge) -> ContinuityLink {
    ContinuityLink {
        from_track_id: edge.from.to_string(),
        to_track_id: edge.to.to_string(),
        score: (edge.score * 1000.0).round() / 1000.0,
    }
}

/// Enforce a temporal chain: two simultaneous visible IDs are distinct.
/// Even a manually requested gap extension must not collapse overlapping
/// or competing candidates into one confirmed player.
pub fn group_tracklets(
    detections: &[PlayerDetection],
    config: ContinuityConfig,
    excluded: &HashSet<i64>,
) -> Result<ContinuityReport, &'static str> {
    config.validate()?;
    if detections.len() > 5_000_000 || excluded.len() > 100_000 {
        return Err("Too many player track detections");
    }

    let mut tracks: HashMap<i64, Tracklet> = HashMap::new();
    let mut discarded = 0usize;
    for point in detections {
        if point.track_id < 0
            || excluded.contains(&point.track_id)
            || point.interpolated
            || point.score < 0.3
            || !point.score.is_finite()
            || point.frame > 10_000_000
            || !valid_bbox(&point.bbox)
        {
            discarded += 1;
            continue;
        }
        let center = [
            (point.bbox[0] + point.bbox[2]) / (2.0 * config.width),
            (point.bbox[1] + point.bbox[3]) / (2.0 * config.height),
        ];
        let height = (point.bbox[3] - point.bbox[1]) / config.height;
        match tracks.get_mut(&point.track_id) {
            Some(track) => {
                track.frames += 1;
                if point.frame < track.first {
                    track.first = point.frame;
                    track.first_center = center;
                    track.first_height = height;
                }
                if point.frame > track.last {
                    track.last = point.frame;
                    track.last_center = center;
                    track.last_height = height;
                }
            }
            None => {
                tracks.insert(
                    point.track_id,
                    Tracklet {
                        id: point.track_id,
                        first: point.frame,
                        last: point.frame,
                        first_center: center,
                        last_center: center,
                        first_height: height,
                        last_height: height,
                        frames: 1,
                    },
                );
            }
        };
    }
    let mut usable = tracks
        .values()
        .filter(|track| track.frames >= 5)
        .cloned()
        .collect::<Vec<_>>();
    if usable.len() > 12_000 {
        return Err("Too many tracklets");
    }
    usable.sort_by_key(|t| (t.first, t.last, t.id));

    let short_max = (config.fps * 0.7).round() as u32;
    let short_max = short_max.min(20);
    let extended = (config.fps * config.extended_gap_sec).round() as u32;
    let mut short = Vec::new();
    let mut distant = Vec::new();
    for (index, first) in usable.iter().enumerate() {
        for second in usable.iter().skip(index + 1) {
            if second.first <= first.last {
                continue;
            }
            let gap = second.first - first.last;
            if gap > extended {
                // Usable tracklets are sorted by first frame; no subsequent
                // candidate can fall back inside this short matching window.
                break;
            }
            if let Some((score, is_extended)) = edge(gap, first, second, short_max, extended) {
                let candidate = Edge {
                    from: first.id,
                    to: second.id,
                    score,
                };
                if is_extended {
                    distant.push(candidate);
                } else {
                    short.push(candidate);
                }
            }
        }
    }
    let verified_short = reciprocal_unique(&short, 0.12);
    let previous = verified_short
        .iter()
        .map(|e| (e.to, e.from))
        .collect::<HashMap<_, _>>();
    let next = verified_short
        .iter()
        .map(|e| (e.from, e.to))
        .collect::<HashMap<_, _>>();

    let mut memberships = BTreeMap::new();
    let mut groups = Vec::new();
    let mut seen = HashSet::new();
    for track in &usable {
        if seen.contains(&track.id) {
            continue;
        }
        let mut ancestor = track.id;
        let mut walk = HashSet::new();
        while let Some(before) = previous.get(&ancestor) {
            if !walk.insert(ancestor) {
                break;
            }
            ancestor = *before;
        }
        if seen.contains(&ancestor) {
            continue;
        }
        let mut members = vec![ancestor];
        while let Some(after) = next.get(members.last().expect("group is nonempty")) {
            if members.contains(after) {
                break;
            }
            members.push(*after);
        }
        // Guard against a chain accidentally joining overlapping intervals.
        let conflict = members.windows(2).any(|pair| {
            let a = tracks.get(&pair[0]).expect("known id");
            let b = tracks.get(&pair[1]).expect("known id");
            a.last >= b.first
        });
        if conflict {
            continue;
        }
        let gid = format!("candidate-{ancestor}");
        for member in &members {
            seen.insert(*member);
            memberships.insert(member.to_string(), gid.clone());
        }
        groups.push(CandidateGroup {
            id: gid,
            track_ids: members.into_iter().map(|n| n.to_string()).collect(),
            confirmed: false,
            evidence: "temporal_geometry_only",
        });
    }
    let mut all_ids = tracks.keys().copied().collect::<Vec<_>>();
    all_ids.sort_unstable();
    for id in all_ids {
        if memberships.contains_key(&id.to_string()) {
            continue;
        }
        let key = format!("candidate-{id}");
        memberships.insert(id.to_string(), key.clone());
        groups.push(CandidateGroup {
            id: key,
            track_ids: vec![id.to_string()],
            confirmed: false,
            evidence: "single_tracklet",
        });
    }

    // Suggestions never alter group memberships or linked player identities.
    // Only choose terminal->head matches across existing candidate groups.
    let terminal = usable
        .iter()
        .filter(|t| !next.contains_key(&t.id))
        .map(|t| t.id)
        .collect::<HashSet<_>>();
    let heads = usable
        .iter()
        .filter(|t| !previous.contains_key(&t.id))
        .map(|t| t.id)
        .collect::<HashSet<_>>();
    let distant = distant
        .into_iter()
        .filter(|e| {
            terminal.contains(&e.from)
                && heads.contains(&e.to)
                && memberships.get(&e.from.to_string()) != memberships.get(&e.to.to_string())
        })
        .collect::<Vec<_>>();
    let distant = reciprocal_unique(&distant, 0.12);

    Ok(ContinuityReport {
        schema:"volley-player-continuity/1",
        confirmed:false,
        method:"short_gap_position_size_reciprocal_unambiguous",
        raw_track_ids:tracks.len(),
        candidate_groups:groups.len(),
        proposed_links:verified_short.into_iter().map(serializable_edge).collect(),
        track_to_candidate:memberships,
        groups,
        extended_gap_sec:config.extended_gap_sec,
        extended_suggestions:distant.into_iter().map(serializable_edge).collect(),
        discarded_detections:discarded,
        warning:"Only provisional geometric continuity. Extended gaps are review suggestions, never confirmed player identities.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(frame: u32, id: i64, x: f64) -> PlayerDetection {
        PlayerDetection {
            frame,
            track_id: id,
            score: 0.9,
            bbox: [x, 100.0, x + 30.0, 300.0],
            interpolated: false,
        }
    }
    fn config() -> ContinuityConfig {
        ContinuityConfig {
            width: 1000.0,
            height: 500.0,
            fps: 30.0,
            extended_gap_sec: 4.0,
        }
    }
    #[test]
    fn short_unambiguous_links_but_no_confirmed_players() {
        let mut rows = (1..=12).map(|f| point(f, 1, 100.0)).collect::<Vec<_>>();
        rows.extend((18..=35).map(|f| point(f, 2, 102.0)));
        let result = group_tracklets(&rows, config(), &HashSet::new()).unwrap();
        assert_eq!(result.proposed_links.len(), 1);
        assert_eq!(
            result.track_to_candidate["1"],
            result.track_to_candidate["2"]
        );
        assert!(!result.confirmed);
    }
    #[test]
    fn three_second_gap_is_review_only_and_configurable() {
        let mut rows = (1..=30).map(|f| point(f, 1, 100.0)).collect::<Vec<_>>();
        rows.extend((120..=151).map(|f| point(f, 2, 100.0)));
        let full = group_tracklets(&rows, config(), &HashSet::new()).unwrap();
        assert!(full.proposed_links.is_empty());
        assert_eq!(full.extended_suggestions.len(), 1);
        assert_ne!(full.track_to_candidate["1"], full.track_to_candidate["2"]);
        let mut strict = config();
        strict.extended_gap_sec = 2.5;
        let short = group_tracklets(&rows, strict, &HashSet::new()).unwrap();
        assert!(short.extended_suggestions.is_empty());
        assert_eq!(short.candidate_groups, 2);
    }
    #[test]
    fn crossing_players_prevent_four_second_identity_swaps() {
        let mut rows = (1..=30)
            .flat_map(|f| {
                [
                    point(f, 1, 100.0 + f as f64 * 0.5),
                    point(f, 2, 110.0 - f as f64 * 0.5),
                ]
            })
            .collect::<Vec<_>>();
        rows.extend((120..=160).flat_map(|f| [point(f, 3, 102.0), point(f, 4, 103.0)]));
        let report = group_tracklets(&rows, config(), &HashSet::new()).unwrap();
        assert!(report.proposed_links.is_empty());
        assert!(report.extended_suggestions.is_empty());
        assert_eq!(report.candidate_groups, 4);
        assert!(!report.confirmed);
    }
    #[test]
    fn four_second_extension_never_automatically_merges_ids() {
        let mut rows = (1..=25).map(|f| point(f, 1, 100.0)).collect::<Vec<_>>();
        rows.extend((125..=160).map(|f| point(f, 2, 100.0)));
        let result = group_tracklets(&rows, config(), &HashSet::new()).unwrap();
        assert!(result.proposed_links.is_empty());
        assert_eq!(result.extended_suggestions.len(), 1);
        assert_ne!(
            result.track_to_candidate["1"],
            result.track_to_candidate["2"]
        );
    }
    #[test]
    fn overlap_and_excluded_ids_stay_disconnected() {
        let mut rows = (1..=50).map(|f| point(f, 1, 100.0)).collect::<Vec<_>>();
        rows.extend((40..=70).map(|f| point(f, 2, 100.0)));
        let result = group_tracklets(&rows, config(), &HashSet::new()).unwrap();
        assert!(result.proposed_links.is_empty());
        assert!(result.extended_suggestions.is_empty());
        let excluded = HashSet::from([2_i64]);
        let result = group_tracklets(&rows, config(), &excluded).unwrap();
        assert!(!result.track_to_candidate.contains_key("2"));
    }
    #[test]
    fn ambiguous_close_players_do_not_get_merged() {
        let mut rows = (1..=10).map(|f| point(f, 1, 100.0)).collect::<Vec<_>>();
        rows.extend((16..=30).map(|f| point(f, 2, 102.0)));
        rows.extend((16..=30).map(|f| point(f, 3, 103.0)));
        let result = group_tracklets(&rows, config(), &HashSet::new()).unwrap();
        assert!(result.proposed_links.is_empty());
    }
}
