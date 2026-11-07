//! Rust-native appearance grouping from saved ReID embedding vectors.
//! Cosine is a similarity, not an identification probability. Grouping is
//! provisional; only explicitly reviewed human assignments carry identity.
//! No Python or OpenVINO runtime is needed to compare existing embeddings.

use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct TrackInfo {
    pub id: i64,
    pub first_frame: i64,
    pub last_frame: i64,
    pub detected_frames: u32,
    pub start_center: [f64; 2],
    pub end_center: [f64; 2],
    pub fps: f64,
}

#[derive(Debug, Clone)]
pub struct Feature {
    pub id: i64,
    pub vector: Vec<f32>,
}

#[derive(Debug, Serialize)]
pub struct Nearest {
    pub track_id: String,
    pub similarity: f64,
}

#[derive(Debug, Serialize)]
pub struct ReidGroup {
    pub id: String,
    pub track_ids: Vec<String>,
    pub similarity: Option<f64>,
    pub source: &'static str,
}

#[derive(Debug, Serialize)]
pub struct ReidSuggestion {
    pub a: String,
    pub b: String,
    pub similarity: f64,
}

#[derive(Debug, Serialize)]
pub struct ReidReport {
    pub groups: Vec<ReidGroup>,
    pub unmatched_ids: Vec<String>,
    pub suggestions: Vec<ReidSuggestion>,
    pub nearest_tracks: BTreeMap<String, Vec<Nearest>>,
    pub missing_visual_ids: Vec<String>,
    pub embedded_count: usize,
    pub total_tracks: usize,
    pub threshold_percent: u8,
    pub method: &'static str,
    pub similarity_type: &'static str,
    pub confirmed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FeatureMode {
    OpenVino,
    ColorFallback,
}

impl FeatureMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenVino => "openvino_reid",
            Self::ColorFallback => "color_fallback",
        }
    }
}

#[derive(Debug, Clone)]
struct Candidate {
    i: usize,
    j: usize,
    sim: f32,
}

fn overlap(a: &TrackInfo, b: &TrackInfo) -> bool {
    a.first_frame <= b.last_frame && b.first_frame <= a.last_frame
}

fn compatible(a: &TrackInfo, b: &TrackInfo, owners: &HashMap<i64, String>) -> bool {
    if let (Some(a_owner), Some(b_owner)) = (owners.get(&a.id), owners.get(&b.id)) {
        if a_owner != b_owner {
            return false;
        }
    }
    !overlap(a, b)
}

fn normalized(vector: &[f32]) -> Option<Vec<f32>> {
    if vector.is_empty() || vector.len() > 4096 || vector.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let len = vector
        .iter()
        .map(|x| (*x as f64).powi(2))
        .sum::<f64>()
        .sqrt();
    if len < 1e-6 {
        return None;
    }
    Some(vector.iter().map(|x| (*x as f64 / len) as f32).collect())
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (*x as f64) * (*y as f64))
        .sum::<f64>()
        .clamp(-1.0, 1.0) as f32
}

fn rounded(x: f32) -> f64 {
    ((x as f64) * 10000.0).round() / 10000.0
}

fn ambiguous(i: usize, j: usize, sim: f32, scores: &[f32], tracks: &[&TrackInfo]) -> bool {
    let n = tracks.len();
    (0..n).any(|competitor| {
        competitor != i
            && competitor != j
            && scores[i * n + competitor] >= sim - 0.018
            && overlap(tracks[j], tracks[competitor])
    })
}

fn find(parents: &[usize], mut index: usize) -> usize {
    while parents[index] != index {
        index = parents[index];
    }
    index
}

/// Compare all saved Track IDs (not only the top 80) by L2-normalized
/// appearance cosine similarity. Preserve co-visible persons and confirmed
/// different owners as distinct identities; require complete-link similarity
/// for each candidate group to prevent A-B-C bridging.
pub fn build_groups(
    features: &[Feature],
    known_tracks: &[TrackInfo],
    owners: &HashMap<i64, String>,
    excluded: &HashSet<i64>,
    mode: FeatureMode,
    threshold_percent: u8,
) -> Result<ReidReport, &'static str> {
    if !(55..=99).contains(&threshold_percent) {
        return Err("ReID cosine threshold must be 55..99");
    }
    if known_tracks.len() > 10_000 || features.len() > 2_500 {
        return Err("Too many ReID observations");
    }
    let mut known = HashMap::new();
    for track in known_tracks {
        if track.id < 0
            || track.first_frame > track.last_frame
            || !track.fps.is_finite()
            || track.fps < 1.0
            || [
                track.start_center[0],
                track.start_center[1],
                track.end_center[0],
                track.end_center[1],
            ]
            .iter()
            .any(|x| !x.is_finite())
            || known.insert(track.id, track).is_some()
        {
            return Err("Invalid or duplicate player track statistics");
        }
    }
    let mut ids = known
        .keys()
        .copied()
        .filter(|id| !excluded.contains(id))
        .collect::<Vec<_>>();
    ids.sort_unstable();
    let mut vectors = HashMap::new();
    let mut dimension = None;
    for feature in features {
        if !known.contains_key(&feature.id) || excluded.contains(&feature.id) {
            continue;
        }
        let Some(vec) = normalized(&feature.vector) else {
            continue;
        };
        if dimension.is_some_and(|d| d != vec.len()) {
            return Err("Mixed embedding dimensions/model spaces");
        }
        dimension = Some(vec.len());
        if vectors.insert(feature.id, vec).is_some() {
            return Err("Duplicate embedding IDs");
        }
    }
    let mut visual_ids = ids
        .iter()
        .copied()
        .filter(|id| vectors.contains_key(id))
        .collect::<Vec<_>>();
    visual_ids.sort_unstable();
    let n = visual_ids.len();
    let tracks = visual_ids
        .iter()
        .map(|id| *known.get(id).expect("known track"))
        .collect::<Vec<_>>();
    let dense = visual_ids
        .iter()
        .map(|id| vectors.get(id).expect("known feature"))
        .collect::<Vec<_>>();
    let mut matrix = vec![-1_f32; n * n];
    for i in 0..n {
        for j in i + 1..n {
            let score = cosine(dense[i], dense[j]);
            matrix[i * n + j] = score;
            matrix[j * n + i] = score;
        }
    }

    let mut nearest = BTreeMap::new();
    for id in &ids {
        nearest.insert(id.to_string(), Vec::new());
    }
    for i in 0..n {
        let mut order = (0..n)
            .filter(|&j| j != i && compatible(tracks[i], tracks[j], owners))
            .collect::<Vec<_>>();
        order.sort_by(|a, b| {
            matrix[i * n + *b]
                .total_cmp(&matrix[i * n + *a])
                .then_with(|| visual_ids[*a].cmp(&visual_ids[*b]))
        });
        nearest.insert(
            visual_ids[i].to_string(),
            order
                .iter()
                .take(3)
                .map(|j| Nearest {
                    track_id: visual_ids[*j].to_string(),
                    similarity: rounded(matrix[i * n + *j]),
                })
                .collect(),
        );
    }

    let threshold = threshold_percent as f32 / 100.0;
    let mut candidates = Vec::new();
    for i in 0..n {
        for j in i + 1..n {
            let sim = matrix[i * n + j];
            if sim >= threshold {
                candidates.push(Candidate { i, j, sim });
            }
        }
    }
    candidates.sort_by(|a, b| {
        b.sim
            .total_cmp(&a.sim)
            .then_with(|| b.i.cmp(&a.i))
            .then_with(|| b.j.cmp(&a.j))
    });

    let mut parents = (0..n).collect::<Vec<_>>();
    let mut members = (0..n)
        .map(|i| (i, BTreeSet::from([i])))
        .collect::<HashMap<_, _>>();
    let mut accepted = Vec::new();
    for candidate in candidates {
        let Candidate { i, j, sim } = candidate;
        if !compatible(tracks[i], tracks[j], owners)
            || ambiguous(i, j, sim, &matrix, &tracks)
            || ambiguous(j, i, sim, &matrix, &tracks)
        {
            continue;
        }
        if mode == FeatureMode::ColorFallback {
            let (before, after) = if tracks[i].last_frame < tracks[j].first_frame {
                (tracks[i], tracks[j])
            } else {
                (tracks[j], tracks[i])
            };
            let gap = after.first_frame - before.last_frame;
            let distance = (before.end_center[0] - after.start_center[0])
                .hypot(before.end_center[1] - after.start_center[1]);
            if gap as f64 > 4.0 * before.fps.max(1.0)
                || distance > 0.11
                || sim < threshold.max(0.985)
            {
                continue;
            }
        }
        if tracks[i].detected_frames.min(tracks[j].detected_frames) < 3
            && sim < (threshold + 0.07).min(0.999)
        {
            continue;
        }
        let ra = find(&parents, i);
        let rb = find(&parents, j);
        if ra == rb {
            continue;
        }
        let compatible_all = members[&ra].iter().all(|a| {
            members[&rb].iter().all(|b| {
                compatible(tracks[*a], tracks[*b], owners)
                    && matrix[*a * n + *b]
                        >= if mode == FeatureMode::ColorFallback {
                            threshold.max(0.985)
                        } else {
                            threshold
                        }
            })
        });
        if !compatible_all {
            continue;
        }
        parents[rb] = ra;
        let merged = members.remove(&rb).expect("valid root");
        members.get_mut(&ra).expect("valid root").extend(merged);
        accepted.push(ReidSuggestion {
            a: visual_ids[i].to_string(),
            b: visual_ids[j].to_string(),
            similarity: rounded(sim),
        });
    }
    let mut groups = Vec::new();
    for entries in members.values() {
        if entries.len() < 2 {
            continue;
        }
        let mut member_ids = entries.iter().map(|i| visual_ids[*i]).collect::<Vec<_>>();
        member_ids.sort_unstable();
        let mut minimum = 1_f32;
        for (index, a) in entries.iter().enumerate() {
            for b in entries.iter().skip(index + 1) {
                minimum = minimum.min(matrix[*a * n + *b]);
            }
        }
        groups.push(ReidGroup {
            id: format!("group-{}", member_ids[0]),
            track_ids: member_ids.iter().map(ToString::to_string).collect(),
            similarity: Some(rounded(minimum)),
            source: "automatic",
        });
    }
    groups.sort_by(|a, b| {
        b.track_ids.len().cmp(&a.track_ids.len()).then_with(|| {
            a.track_ids[0]
                .parse::<i64>()
                .ok()
                .cmp(&b.track_ids[0].parse::<i64>().ok())
        })
    });
    let used = groups
        .iter()
        .flat_map(|g| g.track_ids.iter().cloned())
        .collect::<HashSet<_>>();
    let unmatched = ids
        .iter()
        .map(ToString::to_string)
        .filter(|id| !used.contains(id))
        .collect();
    let missing = ids
        .iter()
        .filter(|id| !vectors.contains_key(id))
        .map(ToString::to_string)
        .collect();
    Ok(ReidReport {
        groups,
        unmatched_ids: unmatched,
        suggestions: accepted.into_iter().take(80).collect(),
        nearest_tracks: nearest,
        missing_visual_ids: missing,
        embedded_count: n,
        total_tracks: ids.len(),
        threshold_percent,
        method: mode.as_str(),
        similarity_type: "cosine_similarity_not_probability",
        confirmed: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: i64, first: i64, last: i64) -> TrackInfo {
        TrackInfo {
            id,
            first_frame: first,
            last_frame: last,
            detected_frames: 12,
            start_center: [0.2, 0.2],
            end_center: [0.2, 0.2],
            fps: 30.0,
        }
    }
    fn feat(id: i64, v: &[f32]) -> Feature {
        Feature {
            id,
            vector: v.to_vec(),
        }
    }
    #[test]
    fn overlapping_players_never_become_same_person() {
        let tracks = vec![track(1, 0, 30), track(2, 10, 20)];
        let embedded = vec![feat(1, &[1., 0.]), feat(2, &[1., 0.])];
        let result = build_groups(
            &embedded,
            &tracks,
            &HashMap::new(),
            &HashSet::new(),
            FeatureMode::OpenVino,
            85,
        )
        .unwrap();
        assert!(result.groups.is_empty());
    }
    #[test]
    fn distinct_confirmed_owners_block_visual_merges() {
        let tracks = vec![track(1, 0, 10), track(2, 30, 40)];
        let embedded = vec![feat(1, &[1., 0.]), feat(2, &[1., 0.])];
        let owners = HashMap::from([(1, "player-a".to_string()), (2, "player-b".to_string())]);
        let result = build_groups(
            &embedded,
            &tracks,
            &owners,
            &HashSet::new(),
            FeatureMode::OpenVino,
            85,
        )
        .unwrap();
        assert!(result.groups.is_empty());
    }
    #[test]
    fn appearance_merge_is_a_provisional_group() {
        let tracks = vec![track(1, 0, 10), track(2, 30, 40)];
        let embedded = vec![feat(1, &[1., 0.]), feat(2, &[1., 0.01])];
        let result = build_groups(
            &embedded,
            &tracks,
            &HashMap::new(),
            &HashSet::new(),
            FeatureMode::OpenVino,
            85,
        )
        .unwrap();
        assert_eq!(result.groups.len(), 1);
        assert!(!result.confirmed);
        assert_eq!(result.groups[0].track_ids, ["1", "2"]);
    }
    #[test]
    fn complete_link_blocks_transitive_bridge() {
        // A-B and B-C plausible, A-C dissimilar: no 3-person cluster.
        let tracks = vec![track(1, 0, 10), track(2, 20, 30), track(3, 40, 50)];
        let embedded = vec![
            feat(1, &[1., 0.]),
            feat(2, &[0.8, 0.6]),
            feat(3, &[0.28, 0.96]),
        ];
        let result = build_groups(
            &embedded,
            &tracks,
            &HashMap::new(),
            &HashSet::new(),
            FeatureMode::OpenVino,
            79,
        )
        .unwrap();
        assert!(result.groups.iter().all(|g| g.track_ids.len() < 3));
    }
}

/// A manually reviewed track or photo, in the SAME embedding model space.
pub struct Reference {
    pub player_id: String,
    pub vector: Vec<f32>,
    pub origin: [String; 4], // project, video, base inference cache, track ID
}

#[derive(Debug, Serialize)]
pub struct PlayerMatch {
    pub player_id: String,
    pub name: String,
    pub similarity: Option<f64>,
    pub above_threshold: bool,
    pub confirmed: bool,
    pub ambiguous: bool,
    pub reference_count: usize,
    pub runner_up_similarity: Option<f64>,
    pub source: &'static str,
}

/// Ranking for the current project's roster. The caller must supply
/// only human-reviewed reference tracks and vetted cached album embeddings.
/// Nothing here assigns an athlete or modifies player identity storage.
pub fn match_references(
    features: &[Feature],
    roster: &BTreeMap<String, String>,
    manually_reviewed: &HashMap<i64, String>,
    references: &[Reference],
    project_id: &str,
    video: &str,
    base_cache: &str,
    threshold_percent: u8,
) -> Result<(BTreeMap<String, PlayerMatch>, BTreeMap<String, usize>), &'static str> {
    if !(55..=99).contains(&threshold_percent) {
        return Err("Invalid cosine threshold");
    }
    if references.len() > 10_000 || roster.len() > 200 || features.len() > 2_500 {
        return Err("Too many reference embeddings");
    }
    let mut refs: BTreeMap<&str, Vec<(Vec<f32>, &[String; 4])>> = BTreeMap::new();
    let mut counts = BTreeMap::new();
    for (pid, _) in roster {
        refs.insert(pid, Vec::new());
    }
    for item in references {
        if !roster.contains_key(&item.player_id) {
            continue;
        }
        let Some(vec) = normalized(&item.vector) else {
            continue;
        };
        if let Some(target) = refs.get_mut(item.player_id.as_str()) {
            target.push((vec, &item.origin));
        }
    }
    for (pid, values) in &refs {
        counts.insert((*pid).to_owned(), values.len());
    }
    let mut matches = BTreeMap::new();
    for feature in features {
        let tid = feature.id;
        if let Some(pid) = manually_reviewed.get(&tid) {
            if let Some(name) = roster.get(pid) {
                matches.insert(
                    tid.to_string(),
                    PlayerMatch {
                        player_id: pid.clone(),
                        name: name.clone(),
                        similarity: None,
                        above_threshold: true,
                        confirmed: true,
                        ambiguous: false,
                        reference_count: *counts.get(pid).unwrap_or(&0),
                        runner_up_similarity: None,
                        source: "manual_verified",
                    },
                );
            }
            continue;
        }
        let Some(embedded) = normalized(&feature.vector) else {
            continue;
        };
        let mut candidates = Vec::new();
        for (pid, items) in &refs {
            let mut highest = None::<f32>;
            let mut evidence = 0_usize;
            for (vector, origin) in items {
                if origin[0] == project_id
                    && origin[1] == video
                    && origin[2] == base_cache
                    && origin[3] == tid.to_string()
                {
                    continue; // no tautological 100%-self-match.
                }
                if embedded.len() != vector.len() {
                    continue;
                }
                let cosine = cosine(&embedded, vector);
                if cosine.is_finite() {
                    highest = Some(highest.map_or(cosine, |old| old.max(cosine)));
                    evidence += 1;
                }
            }
            if let Some(highest) = highest {
                candidates.push((highest, (*pid).to_owned(), evidence));
            }
        }
        candidates.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
        if let Some((best, pid, evidence)) = candidates.first() {
            let second = candidates.get(1).map(|other| other.0);
            matches.insert(
                tid.to_string(),
                PlayerMatch {
                    player_id: pid.clone(),
                    name: roster[pid].clone(),
                    similarity: Some(rounded(*best)),
                    above_threshold: *best >= threshold_percent as f32 / 100.0,
                    confirmed: false,
                    ambiguous: second.is_some_and(|v| v == *best),
                    reference_count: *evidence,
                    runner_up_similarity: second.map(rounded),
                    source: "confirmed_track_and_album",
                },
            );
        }
    }
    Ok((matches, counts))
}
