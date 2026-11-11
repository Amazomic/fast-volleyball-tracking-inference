//! Conservative rally reconstruction and whole-rally selection.
//! No Python calls, GPU inference, media I/O, or saved-analysis mutation.
//! Mirrors evidence boundaries of scripts/rally_boundaries.py and scoring
//! of scripts/full_rallies.py + volleyball_ui/montage_project.py.

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};

pub const FPS: f64 = 30.0;
const ACTIONS: [&str; 6] = [
    "attack",
    "block",
    "dig",
    "overhead_pass",
    "serve",
    "low_touch",
];

#[derive(Debug, Clone)]
pub struct Track {
    pub id: i64,
    pub start: u64,
    pub end: u64,
    pub is_rally: bool,
    pub technical_return: bool,
    pub technical_return_confidence: Option<f64>,
    pub rally_confidence: f64,
    pub action_features: BTreeMap<String, u64>,
}

#[derive(Debug, Clone)]
pub struct Event {
    pub kind: String,
    pub frame: u64,
    pub confidence: f64,
    pub id: Option<String>,
    pub player_track_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Ranking {
    pub start: u64,
    pub end: u64,
    pub score: f64,
    pub confidence: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct RebuildOptions {
    pub max_gap_frames: u64,
    pub padding_frames: u64,
    pub min_frames: u64,
    pub duration_frames: Option<u64>,
}

impl Default for RebuildOptions {
    fn default() -> Self {
        Self {
            max_gap_frames: 90,
            padding_frames: 45,
            min_frames: 0,
            duration_frames: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Rally {
    pub detected_start_frame: u64,
    pub detected_end_frame: u64,
    pub source_start_frame: u64,
    pub source_end_frame: u64,
    pub serve_frame: Option<u64>,
    pub start_evidence: String,
    pub end_evidence: String,
    pub joined_parts: usize,
    pub track_ids: Vec<i64>,
    pub confidence: f64,
    pub duration_sec: f64,
    pub ranking_score: f64,
    pub base_ranking_score: f64,
    pub ranked: bool,
    pub action_counts: BTreeMap<String, u64>,
    pub actions_total: u64,
    pub boundary_method: String,
    pub context_before_frames: u64,
    pub context_after_frames: u64,
    pub selection_strategy: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rally_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub montage_start_sec: Option<f64>,
}

#[derive(Debug, Clone)]
struct InternalGroup {
    start: u64,
    end: u64,
    parts: Vec<Track>,
    confidence: f64,
}

fn finite_confidence(value: f64) -> f64 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        0.0
    }
}

fn round_decimals(value: f64, decimals: i32) -> f64 {
    let factor = 10_f64.powi(decimals);
    (value * factor).round() / factor
}

/// Stable ID for the Results/Montage shared selection. Python serializes the
/// array with spaces after commas and ensure_ascii=False; preserve this.
pub fn rally_selection_id(video: &str, cache: &str, rally: &Rally) -> String {
    let mut ids = rally.track_ids.clone();
    ids.sort_unstable();
    let joined = ids
        .into_iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let identity = format!(
        "[{}, {}, {}, {}, [{}]]",
        serde_json::to_string(video).expect("valid string"),
        serde_json::to_string(cache).expect("valid string"),
        rally.source_start_frame,
        rally.source_end_frame,
        joined
    );
    format!("{:x}", Sha256::digest(identity.as_bytes()))[..24].to_owned()
}

fn blocked(blockers: &[(u64, u64)], first: u64, last: u64) -> bool {
    first <= last && blockers.iter().any(|(a, b)| *a <= last && *b >= first)
}

fn serve_between(serves: &[u64], first_exclusive: u64, end_inclusive: u64) -> bool {
    serves
        .iter()
        .any(|f| *f > first_exclusive && *f <= end_inclusive)
}

/// Track fragments are joined before the minimum-duration filter. A strong
/// technical-return or a new documented serve is a hard boundary.
pub fn rebuild(
    tracks: &[Track],
    events: &[Event],
    ranking: &[Ranking],
    options: RebuildOptions,
) -> Vec<Rally> {
    let mut valid: Vec<Track> = tracks
        .iter()
        .filter(|t| t.is_rally && t.start <= t.end)
        .cloned()
        .collect();
    valid.sort_by_key(|t| (t.start, t.end));
    let mut blockers: Vec<(u64, u64)> = tracks
        .iter()
        .filter(|t| {
            !t.is_rally
                && t.technical_return
                && t.start <= t.end
                && t.technical_return_confidence
                    .is_none_or(|strength| strength >= 0.75)
        })
        .map(|t| (t.start, t.end))
        .collect();
    blockers.sort_unstable();
    let mut serves: Vec<u64> = events
        .iter()
        .filter(|e| e.kind == "serve" && finite_confidence(e.confidence) >= 0.7)
        .map(|e| e.frame)
        .collect();
    serves.sort_unstable();
    serves.dedup();
    let contacts: Vec<u64> = events
        .iter()
        .filter(|e| e.kind.contains("contact") && finite_confidence(e.confidence) >= 0.5)
        .map(|e| e.frame)
        .collect();

    let mut grouped: Vec<InternalGroup> = Vec::new();
    for track in valid {
        if let Some(group) = grouped.last_mut() {
            let gap = track.start as i128 - group.end as i128 - 1;
            let crossed = blocked(
                &blockers,
                group.end.saturating_add(1),
                track.start.saturating_sub(1),
            );
            if gap <= options.max_gap_frames as i128
                && !crossed
                && !serve_between(&serves, group.end, track.start)
            {
                group.end = group.end.max(track.end);
                group.confidence = group
                    .confidence
                    .max(finite_confidence(track.rally_confidence));
                group.parts.push(track);
                continue;
            }
        }
        grouped.push(InternalGroup {
            start: track.start,
            end: track.end,
            confidence: finite_confidence(track.rally_confidence),
            parts: vec![track],
        });
    }
    grouped.retain(|g| g.end - g.start + 1 >= options.min_frames);
    let duration = options
        .duration_frames
        .unwrap_or_else(|| {
            grouped
                .iter()
                .map(|g| g.end)
                .max()
                .unwrap_or(0)
                .saturating_add(1)
        })
        .max(1);
    let rank_map: HashMap<(u64, u64), &Ranking> =
        ranking.iter().map(|r| ((r.start, r.end), r)).collect();

    let mut out = Vec::with_capacity(grouped.len());
    for (index, group) in grouped.iter().enumerate() {
        let detected_start = group.start;
        let end = group.end;
        let prev = if index > 0 {
            Some(grouped[index - 1].end)
        } else {
            None
        };
        let next = if index + 1 < grouped.len() {
            grouped[index + 1].start
        } else {
            duration
        };
        let first_serve = serves
            .iter()
            .copied()
            .filter(|f| {
                *f <= detected_start
                    && *f >= detected_start.saturating_sub(120)
                    && prev.is_none_or(|before| *f > before)
                    && !blocked(&blockers, *f, detected_start.saturating_sub(1))
            })
            .min();
        let start = first_serve.unwrap_or(detected_start);
        // Python's floor division of (prev + start + 1)//2, prev==-1
        // for the first rally.
        let lower = prev.map_or(start / 2, |p| (p + start).div_ceil(2));
        let upper = (end.saturating_add(next).saturating_sub(1) / 2).min(duration - 1);
        let mut source_start = start
            .saturating_sub(options.padding_frames)
            .max(lower)
            .min(start);
        let mut source_end = end
            .saturating_add(options.padding_frames)
            .min(upper)
            .max(end);
        let last_touch = contacts
            .iter()
            .copied()
            .filter(|frame| *frame >= end.saturating_sub(15) && *frame <= end)
            .max();
        for (a, b) in &blockers {
            if *b >= source_start && *a < start {
                source_start = source_start.max(b.saturating_add(1));
            }
            if *a <= source_end && *a > end {
                let mut boundary = a.saturating_sub(1);
                if let Some(last_touch) = last_touch {
                    if a.saturating_sub(end) <= 5 {
                        boundary = boundary.max(last_touch.saturating_add(23));
                    }
                }
                source_end = source_end.min(boundary);
            }
        }
        if let Some(next_serve) = serves
            .iter()
            .copied()
            .filter(|f| *f > end && *f <= source_end)
            .min()
        {
            source_end = source_end.min(next_serve.saturating_sub(1));
        }
        source_start = source_start.min(start);
        source_end = source_end.max(end);

        let scores: Vec<&Ranking> = group
            .parts
            .iter()
            .filter_map(|p| rank_map.get(&(p.start, p.end)).copied())
            .collect();
        let max_score = scores
            .iter()
            .map(|r| r.score)
            .filter(|v| v.is_finite())
            .reduce(f64::max)
            .unwrap_or(0.0);
        let sum_score = scores
            .iter()
            .map(|r| r.score)
            .filter(|v| v.is_finite())
            .sum::<f64>();
        let rank_score = round_decimals(max_score + 0.3 * (sum_score - max_score), 4);
        let confidence = round_decimals(
            scores
                .iter()
                .map(|r| finite_confidence(r.confidence))
                .fold(group.confidence, f64::max),
            4,
        );

        let mut counts: BTreeMap<String, u64> =
            ACTIONS.iter().map(|name| (name.to_string(), 0)).collect();
        let mut seen = HashSet::new();
        for event in events {
            if !counts.contains_key(&event.kind) {
                continue;
            }
            if (event.frame < detected_start || event.frame > end)
                && !(event.kind == "serve" && Some(event.frame) == first_serve)
            {
                continue;
            }
            let unique = event
                .id
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| {
                    format!(
                        "{}:{}:{}",
                        event.frame,
                        event.kind,
                        event.player_track_id.as_deref().unwrap_or("")
                    )
                });
            if seen.insert(unique) {
                *counts.get_mut(&event.kind).expect("known action") += 1;
            }
        }
        if counts.values().all(|count| *count == 0) {
            for track in &group.parts {
                for name in ACTIONS {
                    let feature = match name {
                        "dig" => "dig_count",
                        "overhead_pass" => "overhead_count",
                        "low_touch" => "low_touch_count",
                        _ => "",
                    };
                    let constructed = if feature.is_empty() {
                        format!("{name}_count")
                    } else {
                        feature.to_string()
                    };
                    *counts.get_mut(name).expect("known") += track
                        .action_features
                        .get(&constructed)
                        .copied()
                        .unwrap_or(0);
                }
            }
        }
        let actions_total = counts.values().sum();
        out.push(Rally {
            detected_start_frame: detected_start,
            detected_end_frame: end,
            source_start_frame: source_start,
            source_end_frame: source_end,
            serve_frame: first_serve,
            start_evidence: if first_serve.is_some() {
                "detected_serve"
            } else {
                "first_rally_ball_track"
            }
            .into(),
            end_evidence: "last_rally_ball_track".into(),
            joined_parts: group.parts.len(),
            track_ids: group.parts.iter().map(|t| t.id).collect(),
            confidence,
            duration_sec: round_decimals((source_end - source_start + 1) as f64 / FPS, 4),
            ranking_score: rank_score,
            base_ranking_score: rank_score,
            ranked: !scores.is_empty(),
            action_counts: counts,
            actions_total,
            boundary_method: "reconstructed_ball_tracks; serve/point not verified".into(),
            context_before_frames: start - source_start,
            context_after_frames: source_end - end,
            selection_strategy: "highlights".into(),
            rally_id: None,
            sequence: None,
            montage_start_sec: None,
        });
    }
    out
}

/// Same ranking orders as Python preview. Automatic strategy also enforces
/// action filters; manual selection does NOT silently filter chosen scenes.
pub fn selection_score(rally: &Rally, strategy: &str) -> Result<f64, String> {
    let base = rally.base_ranking_score;
    let duration = (rally.detected_end_frame - rally.detected_start_frame + 1) as f64 / FPS;
    if strategy == "highlights" {
        return Ok(base + 0.45 * duration.min(16.0));
    }
    let primary = match strategy {
        "longest" => duration,
        "most_actions" => rally.actions_total as f64,
        "attacks" => *rally.action_counts.get("attack").unwrap_or(&0) as f64,
        "blocks" => *rally.action_counts.get("block").unwrap_or(&0) as f64,
        "digs" => *rally.action_counts.get("dig").unwrap_or(&0) as f64,
        "passes" => *rally.action_counts.get("overhead_pass").unwrap_or(&0) as f64,
        "serves" => *rally.action_counts.get("serve").unwrap_or(&0) as f64,
        _ => return Err("Неизвестный критерий отбора розыгрышей".into()),
    };
    Ok(primary * 10000.0 + base.clamp(0.0, 9999.0) / 10000.0)
}

pub fn ranked_eligible(
    rally: &Rally,
    min_sec: f64,
    min_confidence: f64,
    thresholds: &BTreeMap<String, u64>,
    strategy: &str,
) -> bool {
    let detected_sec = (rally.detected_end_frame - rally.detected_start_frame + 1) as f64 / FPS;
    if !rally.ranked || detected_sec < min_sec || rally.confidence < min_confidence {
        return false;
    }
    let focus = match strategy {
        "attacks" => "attack",
        "blocks" => "block",
        "digs" => "dig",
        "passes" => "overhead_pass",
        "serves" => "serve",
        _ => "",
    };
    if !focus.is_empty() && rally.action_counts.get(focus).copied().unwrap_or(0) == 0 {
        return false;
    }
    thresholds.iter().all(|(name, threshold)| {
        if name == "actions" {
            rally.actions_total >= *threshold
        } else {
            rally
                .action_counts
                .get(name)
                .is_some_and(|n| n >= threshold)
        }
    })
}

pub fn choose_whole_groups(
    mut candidates: Vec<Rally>,
    budget_seconds: f64,
) -> Result<Vec<Rally>, String> {
    if !budget_seconds.is_finite() || budget_seconds <= 0.0 || budget_seconds > 600.0 {
        return Err("Недопустимая длительность ролика".into());
    }
    let max_frames = (budget_seconds * FPS).round() as u64;
    candidates.retain(|r| r.ranked);
    candidates.sort_by(|a, b| {
        b.ranking_score
            .total_cmp(&a.ranking_score)
            .then_with(|| b.duration_sec.total_cmp(&a.duration_sec))
    });
    let mut selected = Vec::new();
    let mut used = 0_u64;
    for rally in candidates {
        let size = rally.source_end_frame - rally.source_start_frame + 1;
        if used.saturating_add(size) <= max_frames {
            used += size;
            selected.push(rally);
        }
    }
    if selected.is_empty() {
        return Err("Ни один полный розыгрыш не помещается в заданное время".into());
    }
    selected.sort_by_key(|r| (r.source_start_frame, r.source_end_frame));
    let mut timeline = 0_u64;
    for (i, rally) in selected.iter_mut().enumerate() {
        rally.sequence = Some(i + 1);
        rally.montage_start_sec = Some(round_decimals(timeline as f64 / FPS, 4));
        timeline += rally.source_end_frame - rally.source_start_frame + 1;
    }
    Ok(selected)
}
