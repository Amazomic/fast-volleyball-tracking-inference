//! Personalized whole-rally montage evidence for one reviewed project athlete.
//! This only reads saved CPU actions and human-reviewed identity assignments.
//! A candidate ReID similarity never automatically becomes a real player.

use crate::{
    player_statistics, projects, rallies, reid_native,
    storage::{self, ApiError},
    AppState,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::Path,
};

use volley_core::statistics::{self, Action, Player};

const EVENTS_LIMIT: u64 = 160 * 1024 * 1024;
const METADATA_LIMIT: u64 = 16 * 1024 * 1024;

fn matches_range(event: &Action, scene: &rallies::Scene) -> bool {
    let Some(frame) = event.frame else {
        return false;
    };
    let start = scene.rally.detected_start_frame;
    let end = scene.rally.detected_end_frame;
    (frame >= start && frame <= end)
        || (event.kind == "serve"
            && scene.rally.start_evidence == "detected_serve"
            && scene.rally.serve_frame == Some(frame)
            && frame < start)
}

fn verified_group_source(
    state: &AppState,
    project_id: &str,
    video: &str,
    base: &str,
    source: &Path,
) -> Result<(Value, String), ApiError> {
    let group_path = projects::project_root(state)
        .join(project_id)
        .join("player_identity_groups")
        .join(format!("{}.json", reid_native::path_key(video, base)));
    let (group, revision) = player_statistics::read_document(&group_path, METADATA_LIMIT, false)?;
    if group.is_null() {
        return Ok((group, revision));
    }
    let stamp = fs::metadata(source)
        .map_err(|_| storage::bad_request("Saved GPU player source missing"))?;
    let time_ns = stamp
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|t| t.as_nanos());
    if group["schema"].as_u64() != Some(1)
        || group["stamp"]["size"].as_u64() != Some(stamp.len())
        || group["stamp"]["mtime_ns"].as_u64().map(u128::from) != time_ns
    {
        return Err(storage::conflict(
            "ReID-группы устарели, нельзя использовать их для монтажа игрока",
        ));
    }
    Ok((group, revision))
}

fn action_count_for_strategy(counts: &BTreeMap<String, u64>, strategy: &str, total: u64) -> u64 {
    let kind = match strategy {
        "attacks" => "attack",
        "blocks" => "block",
        "serves" => "serve",
        "digs" => "dig",
        "passes" => "overhead_pass",
        _ => return total,
    };
    *counts.get(kind).unwrap_or(&0)
}

/// Annotate only rallies for one selected video. The caller has already
/// confirmed this is a saved run and correctly matched video/cache source.
/// The requested athlete must be in this project's roster.
pub(crate) fn annotate_rallies(
    state: &AppState,
    project_id: &str,
    project: &Value,
    video: &str,
    cache_id: &str,
    player_id: &str,
    player_action: &str,
    strategy: &str,
    scenes: &mut [rallies::Scene],
) -> Result<String, ApiError> {
    let roster_file = state.data_root.join("players/registry.json");
    let (_, roster_before) = player_statistics::read_document(&roster_file, METADATA_LIMIT, false)?;
    let roster = player_statistics::roster(state, project)?;
    let player_name = roster
        .get(player_id)
        .ok_or_else(|| {
            storage::bad_request("Игрок не входит в подтверждённый состав выбранного проекта")
        })?
        .clone();

    let cpu_cache = rallies::cache_root(state, project_id, video, cache_id)?;
    let base = cache_id.split('.').next().unwrap_or(cache_id);
    let gpu_cache = if cache_id.contains('.') {
        cpu_cache
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| storage::bad_request("Недоступен базовый GPU-кэш"))?
            .to_path_buf()
    } else {
        cpu_cache.clone()
    };
    let player_source = gpu_cache.join("players.json");
    let gpu_before = fs::metadata(&player_source)
        .map_err(|_| storage::bad_request("Отсутствуют исходные обнаружения игроков"))?;
    let gpu_stamp = (gpu_before.len(), gpu_before.modified().ok());
    let known = player_statistics::known_tracks(&player_source)?;
    let (groups, group_revision) =
        verified_group_source(state, project_id, video, base, &player_source)?;
    let identity_file = projects::project_root(state)
        .join(project_id)
        .join("player_identities.json");
    let (identity, identity_revision) =
        player_statistics::read_document(&identity_file, METADATA_LIMIT, false)?;
    let (owners, excluded, conflicts, _) =
        player_statistics::known_owners(&identity, &groups, video, base, &roster, &known);
    if conflicts != 0 {
        // Don't produce a flattering player reel from an ambiguous identity
        // registry. Ask for manual repair rather than dropping conflicts.
        return Err(storage::conflict(
            "В ручных привязках игроков есть противоречия; устраните их перед монтажом",
        ));
    }
    let selected = owners
        .into_iter()
        .filter_map(|(tid, owner)| (owner == player_id).then_some(tid))
        .collect::<HashSet<_>>();
    if selected.is_empty() {
        // Other project videos may contain reviewed IDs; a player with
        // no confirmed detections in this particular video is not an error.
        for scene in scenes {
            scene.player_action_counts = Some(BTreeMap::new());
            scene.player_actions_total = Some(0);
            scene.player_focus_matches = Some(0);
            scene.rally.ranking_score = 0.0;
        }
        return Ok(player_name);
    }
    let replay_path = cpu_cache.join("players.replay.json");
    let replay_before = fs::metadata(&replay_path)
        .ok()
        .map(|m| (m.len(), m.modified().ok()));
    let visible = if replay_path.is_file() {
        Some(
            player_statistics::known_tracks(&replay_path)?
                .into_keys()
                .collect::<HashSet<_>>(),
        )
    } else {
        None
    };
    let events_path = cpu_cache.join("analysis/events.json");
    let (raw, evidence_revision) =
        player_statistics::read_document(&events_path, EVENTS_LIMIT, true)?;
    let (actions, _) = player_statistics::actions_from_events(&raw, None)?;
    let mut actions = actions
        .into_iter()
        .filter(|event| {
            event.track_id.as_ref().is_some_and(|tid| {
                selected.contains(tid)
                    && !excluded.contains(tid)
                    && visible.as_ref().is_none_or(|ids| ids.contains(tid))
            })
        })
        .filter(|event| event.frame.is_some())
        .collect::<Vec<_>>();
    actions.sort_by_key(|event| event.frame.unwrap_or(u64::MAX));
    let roster = vec![Player {
        id: player_id.to_owned(),
        name: player_name.clone(),
        track_ids: selected.into_iter().collect(),
    }];
    for scene in scenes {
        let start = scene.rally.detected_start_frame;
        let end = scene.rally.detected_end_frame;
        let lower = actions.partition_point(|event| event.frame.unwrap_or(u64::MAX) < start);
        let upper = actions.partition_point(|event| event.frame.unwrap_or(u64::MAX) <= end);
        let mut events = actions[lower..upper].to_vec();
        // The detected serve may lead the first ball segment; include only
        // this rally's verified serve anchor rather than previous rallies.
        if scene.rally.start_evidence == "detected_serve" {
            if let Some(serve_frame) = scene.rally.serve_frame {
                if serve_frame < start {
                    let first =
                        actions.partition_point(|e| e.frame.unwrap_or(u64::MAX) < serve_frame);
                    let last =
                        actions.partition_point(|e| e.frame.unwrap_or(u64::MAX) <= serve_frame);
                    events.extend(
                        actions[first..last]
                            .iter()
                            .filter(|e| e.kind == "serve" && matches_range(e, scene))
                            .cloned(),
                    );
                }
            }
        }
        let result =
            statistics::summarize(&events, &roster, &excluded).map_err(storage::bad_request)?;
        let owned = &result.players[0];
        let focused = if player_action == "all" {
            owned.touches
        } else {
            *owned.actions.get(player_action).unwrap_or(&0)
        };
        let kind_score = action_count_for_strategy(&owned.actions, strategy, owned.touches);
        // The selected person's actions, not other players' actions, are the
        // primary score for personalized attack/block/serve/dig montages.
        let ranking = if strategy == "longest" {
            scene.rally.duration_sec * 10000.0
                + (focused as f64).min(1000.0)
                + scene.rally.base_ranking_score.clamp(0.0, 9999.0) / 10000.0
        } else {
            let primary = if strategy == "highlights" {
                focused
            } else {
                kind_score
            };
            primary as f64 * 10000.0
                + scene.rally.base_ranking_score.clamp(0.0, 9999.0) / 10000.0
                + 0.01 * ((end - start + 1) as f64 / 30.0).min(16.0)
        };
        scene.rally.ranking_score = (ranking * 10000.0).round() / 10000.0;
        scene.player_action_counts = Some(owned.actions.clone());
        scene.player_actions_total = Some(owned.touches);
        scene.player_focus_matches = Some(focused);
    }
    let gpu_after = fs::metadata(&player_source)
        .map_err(|_| storage::conflict("GPU-player source disappeared"))?;
    if (gpu_after.len(), gpu_after.modified().ok()) != gpu_stamp {
        return Err(storage::conflict(
            "GPU-player source changed during montage planning",
        ));
    }
    let (_, identity_after) =
        player_statistics::read_document(&identity_file, METADATA_LIMIT, false)?;
    if identity_after != identity_revision {
        return Err(storage::conflict(
            "Ручные привязки изменились при планировании монтажа",
        ));
    }
    let (_, evidence_after) = player_statistics::read_document(&events_path, EVENTS_LIMIT, true)?;
    if evidence_after != evidence_revision {
        return Err(storage::conflict(
            "CPU события изменились при планировании монтажа",
        ));
    }
    let (_, roster_after) = player_statistics::read_document(&roster_file, METADATA_LIMIT, false)?;
    if roster_after != roster_before {
        return Err(storage::conflict(
            "Состав игроков изменился при планировании монтажа",
        ));
    }
    let (_, groups_after) = verified_group_source(state, project_id, video, base, &player_source)?;
    if groups_after != group_revision {
        return Err(storage::conflict(
            "Ручные ReID-группы изменились при выборе розыгрышей",
        ));
    }
    let replay_after = fs::metadata(&replay_path)
        .ok()
        .map(|m| (m.len(), m.modified().ok()));
    if replay_after != replay_before {
        return Err(storage::conflict(
            "Результаты CPU-трекинга игроков изменились",
        ));
    }
    Ok(player_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_strategy_to_athlete_action() {
        let counts = BTreeMap::from([("attack".into(), 2), ("block".into(), 5)]);
        assert_eq!(action_count_for_strategy(&counts, "attacks", 7), 2);
        assert_eq!(action_count_for_strategy(&counts, "blocks", 7), 5);
        assert_eq!(action_count_for_strategy(&counts, "highlights", 7), 7);
    }

    #[test]
    fn detected_action_window_does_not_include_padding() {
        let scene = rallies::Scene {
            video: "match.mp4".into(),
            cache_id: "gpu.post".into(),
            analysis_offset_sec: 0.0,
            rally: volley_core::rallies::Rally {
                detected_start_frame: 100,
                detected_end_frame: 130,
                source_start_frame: 75,
                source_end_frame: 150,
                serve_frame: Some(90),
                start_evidence: "detected_serve".into(),
                end_evidence: "last_rally_ball_track".into(),
                joined_parts: 1,
                track_ids: vec![1],
                confidence: 0.9,
                duration_sec: 2.53,
                ranking_score: 4.0,
                base_ranking_score: 4.0,
                ranked: true,
                action_counts: BTreeMap::new(),
                actions_total: 0,
                boundary_method: "test".into(),
                context_before_frames: 25,
                context_after_frames: 20,
                selection_strategy: "highlights".into(),
                rally_id: None,
                sequence: None,
                montage_start_sec: None,
            },
            player_action_counts: None,
            player_actions_total: None,
            player_focus_matches: None,
        };
        let action = |frame, kind: &str| Action {
            frame: Some(frame),
            kind: kind.into(),
            track_id: Some("9".into()),
            id: String::new(),
            contact_id: None,
            time_ms: (frame * 33) as i64,
            confidence: 1.0,
        };
        assert!(matches_range(&action(90, "serve"), &scene));
        assert!(!matches_range(&action(90, "attack"), &scene));
        assert!(matches_range(&action(110, "attack"), &scene));
        assert!(!matches_range(&action(140, "attack"), &scene));
    }
}
