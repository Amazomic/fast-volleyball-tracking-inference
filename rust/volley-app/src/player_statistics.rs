//! Saved-CPU player statistics, using only explicitly reviewed identities.
//! Preview is read-only; recalculate persists versioned snapshots exclusively
//! under an isolated VOLLEY_RUST_DATA_ROOT outside the original checkout.

use crate::{
    analysis_runs, projects, rallies, reid_native,
    storage::{self, ApiError, ApiResult},
    AppState,
};
use axum::{extract::State, http::HeaderMap, Json};
use fs2::FileExt;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};
use volley_core::{
    rallies::{self as rally_core, Event as RallyEvent, RebuildOptions, Track as RallyTrack},
    statistics::{self, Action, Player, PlayerTotals, Stats},
};

const STATS_ALGORITHM: &str = "volley-native-player-statistics-v1-rally-filter-dedup";
const MAX_EVENTS: u64 = 160 * 1024 * 1024;
const MAX_TRACKS: u64 = 128 * 1024 * 1024;
const MAX_IDS: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatisticsRequest {
    pub project_id: String,
    pub run_id: String,
    #[serde(default = "preview_mode")]
    pub mode: String,
    #[serde(default)]
    pub selected_videos: Option<Vec<String>>,
}
fn preview_mode() -> String {
    "preview".into()
}

fn read_raw(path: &Path, limit: u64) -> Result<Option<Vec<u8>>, ApiError> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > limit {
            return Err(storage::bad_request(
                "Файл CPU-статистики слишком большой или небезопасен",
            ));
        }
    }
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(storage::internal(format!("Cannot open CPU stats: {e}"))),
    };
    let mut data = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut data)
        .map_err(|e| storage::internal(format!("Cannot read CPU stats: {e}")))?;
    if data.len() as u64 > limit {
        return Err(storage::bad_request("CPU-данные превышают лимит"));
    }
    Ok(Some(data))
}
fn digest(bytes: Option<&[u8]>) -> String {
    bytes
        .map(|b| format!("{:x}", Sha256::digest(b)))
        .unwrap_or_else(|| "absent".into())
}
pub(crate) fn read_document(
    path: &Path,
    limit: u64,
    required: bool,
) -> Result<(Value, String), ApiError> {
    let data = read_raw(path, limit)?;
    if data.is_none() && required {
        return Err(storage::bad_request(format!(
            "Отсутствует {}",
            path.display()
        )));
    }
    let revision = digest(data.as_deref());
    let value = data
        .as_deref()
        .map(|data| {
            serde_json::from_slice::<Value>(data)
                .map_err(|e| storage::bad_request(format!("Некорректный CPU JSON: {e}")))
        })
        .transpose()?
        .unwrap_or(Value::Null);
    Ok((value, revision))
}
fn num(x: &Value) -> Option<f64> {
    x.as_f64()
        .or_else(|| x.as_str().and_then(|v| v.parse::<f64>().ok()))
        .filter(|v| v.is_finite())
}
fn integer(x: &Value) -> Option<u64> {
    x.as_u64()
        .or_else(|| x.as_str().and_then(|v| v.parse::<u64>().ok()))
}
fn tid(x: &Value) -> Option<String> {
    integer(x)
        .filter(|n| n.to_string().len() <= 12)
        .map(|n| n.to_string())
}
fn marker(path: &Path) -> Result<String, ApiError> {
    let meta = fs::metadata(path).map_err(|_| storage::bad_request("Отсутствуют GPU-детекции"))?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|v| v.duration_since(UNIX_EPOCH).ok())
        .ok_or_else(|| storage::bad_request("GPU-детекции без временной метки"))?;
    Ok(format!("{}:{}", modified.as_nanos(), meta.len()))
}
fn optional_marker(path: &Path) -> Result<String, ApiError> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok("absent".into()),
        Err(e) => Err(storage::internal(format!("Input marker: {e}"))),
        Ok(m) if m.file_type().is_symlink() || !m.is_file() => Err(storage::bad_request(
            "Не поддерживаются ссылки на CPU/identity данные",
        )),
        Ok(_) => marker(path),
    }
}
pub(crate) fn known_tracks(path: &Path) -> Result<HashMap<String, (i64, i64)>, ApiError> {
    let ids = reid_native::track_statistics(path)?
        .into_iter()
        .map(|t| (t.id.to_string(), (t.first_frame, t.last_frame)))
        .collect::<HashMap<_, _>>();
    if ids.len() > 20_000 {
        return Err(storage::bad_request("Слишком много GPU Track ID"));
    }
    Ok(ids)
}
pub(crate) fn roster(
    state: &AppState,
    project: &Value,
) -> Result<BTreeMap<String, String>, ApiError> {
    let selected = project["player_ids"]
        .as_array()
        .ok_or_else(|| storage::bad_request("Не настроен список игроков проекта"))?
        .iter()
        .filter_map(Value::as_str)
        .collect::<HashSet<_>>();
    let path = state.data_root.join("players/registry.json");
    let (catalog, _) = read_document(&path, MAX_IDS, !selected.is_empty())?;
    let mut players = BTreeMap::new();
    for item in catalog["players"].as_array().into_iter().flatten() {
        if let (Some(id), Some(name)) = (item["id"].as_str(), item["name"].as_str()) {
            if selected.contains(id) && id.len() <= 128 && name.len() <= 200 {
                players.insert(id.to_owned(), name.to_owned());
            }
        }
    }
    if players.len() != selected.len() {
        return Err(storage::bad_request(
            "В общей базе отсутствуют игроки этого проекта",
        ));
    }
    Ok(players)
}
pub(crate) fn known_owners(
    identity: &Value,
    groups: &Value,
    video: &str,
    base: &str,
    roster: &BTreeMap<String, String>,
    known: &HashMap<String, (i64, i64)>,
) -> (HashMap<String, String>, HashSet<String>, usize, usize) {
    let key = format!("{video}::{base}");
    let mut excluded = HashSet::new();
    for v in identity["excluded_ids"][&key]
        .as_array()
        .into_iter()
        .flatten()
    {
        if let Some(id) = tid(v) {
            excluded.insert(id);
        }
    }
    let mut owners = HashMap::new();
    let mut conflicted = HashSet::new();
    for group in identity["videos"][&key].as_array().into_iter().flatten() {
        let Some(owner) = group["id"].as_str() else {
            continue;
        };
        if !roster.contains_key(owner) {
            continue;
        }
        for value in group["track_ids"].as_array().into_iter().flatten() {
            let Some(track) = tid(value) else {
                continue;
            };
            if excluded.contains(&track) || !known.contains_key(&track) {
                continue;
            }
            let reviewed = group["link_meta"][&track]["reviewed"]
                .as_bool()
                .unwrap_or(true);
            if !reviewed {
                continue;
            }
            if owners
                .insert(track.clone(), owner.to_owned())
                .is_some_and(|old| old != owner)
            {
                conflicted.insert(track);
            }
        }
    }
    for track in &conflicted {
        owners.remove(track);
    }
    let mut inherited = 0_usize;
    for group in groups["groups"].as_array().into_iter().flatten() {
        if group["source"].as_str() != Some("manual") {
            continue;
        }
        let ids = group["track_ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(tid)
            .filter(|t| known.contains_key(t) && !excluded.contains(t) && !conflicted.contains(t))
            .collect::<Vec<_>>();
        // Imported manual group catalogues may predate overlap guards.
        // Never credit an athlete from a group with co-visible Track IDs.
        let overlaps = ids.iter().enumerate().any(|(i, id)| {
            let (from, to) = known[id];
            ids.iter().skip(i + 1).any(|other| {
                let (b_from, b_to) = known[other];
                from <= b_to && b_from <= to
            })
        });
        if overlaps {
            continue;
        }
        let candidates = ids
            .iter()
            .filter_map(|id| owners.get(id))
            .collect::<HashSet<_>>();
        if candidates.len() != 1 {
            continue;
        }
        let owner = (*candidates.into_iter().next().expect("one owner")).clone();
        for track in ids {
            if owners.insert(track, owner.clone()).is_none() {
                inherited += 1;
            }
        }
    }
    (owners, excluded, conflicted.len(), inherited)
}
fn rally_ranges(
    tracks: &Value,
    events: &Value,
) -> Result<(Vec<(u64, u64, Option<u64>, bool)>, usize), ApiError> {
    let rows = tracks
        .as_array()
        .ok_or_else(|| storage::bad_request("Некорректные game-tracks.json"))?;
    if rows.len() > 100_000 {
        return Err(storage::bad_request("Слишком много игровых траекторий"));
    }
    let flights = rows
        .iter()
        .filter_map(|r| {
            let (start, end, id) = (
                integer(&r["start_frame"])?,
                integer(&r["last_frame"])?,
                integer(&r["track_id"])?,
            );
            if start > end || id > i64::MAX as u64 {
                return None;
            }
            let class = &r["rally_classification"];
            let accepted = class["is_rally"].as_bool() == Some(true)
                || class["label"].as_str() == Some("rally");
            Some(RallyTrack {
                id: id as i64,
                start,
                end,
                is_rally: accepted,
                technical_return: !accepted
                    && class["not_rally_reason"].as_str() == Some("technical_return"),
                technical_return_confidence: num(&class["technical_return_confidence"]),
                rally_confidence: num(&class["rally_confidence"]).unwrap_or(0.0),
                action_features: BTreeMap::new(),
            })
        })
        .collect::<Vec<_>>();
    let events = events["events"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            Some(RallyEvent {
                kind: e["type"].as_str()?.to_owned(),
                frame: integer(&e["frame"])?,
                confidence: num(&e["confidence"]).unwrap_or(0.0),
                id: None,
                player_track_id: None,
            })
        })
        .collect::<Vec<_>>();
    let groups = rally_core::rebuild(
        &flights,
        &events,
        &[],
        RebuildOptions {
            max_gap_frames: 90,
            padding_frames: 45,
            min_frames: 90,
            duration_frames: None,
        },
    );
    let ranges = groups
        .iter()
        .map(|g| {
            (
                g.detected_start_frame,
                g.detected_end_frame,
                g.serve_frame,
                g.start_evidence == "detected_serve",
            )
        })
        .collect();
    Ok((ranges, groups.len()))
}
pub(crate) fn actions_from_events(
    events: &Value,
    ranges: Option<&[(u64, u64, Option<u64>, bool)]>,
) -> Result<(Vec<Action>, u64), ApiError> {
    let fps = num(&events["fps"]).unwrap_or(30.0);
    let offset = num(&events["analysis_offset_sec"]).unwrap_or(0.0);
    if !(1.0..=240.0).contains(&fps) || !(0.0..=86400.0).contains(&offset) {
        return Err(storage::bad_request(
            "Неправильный FPS или сдвиг CPU анализа",
        ));
    }
    let absolute = events["schema"].as_str() == Some("volleyball-events/2");
    let rows = events["events"]
        .as_array()
        .ok_or_else(|| storage::bad_request("CPU-анализ не содержит массив events"))?;
    if rows.len() > 1_000_000 {
        return Err(storage::bad_request("Слишком много CPU событий"));
    }
    let mut actions = Vec::new();
    let mut outside = 0_u64;
    for (index, event) in rows.iter().enumerate() {
        let Some(kind) = event["type"].as_str() else {
            continue;
        };
        if !statistics::ACTION_TYPES.contains(&kind) {
            continue;
        }
        let frame = integer(&event["frame"]);
        if let Some(windows) = ranges {
            let inside = frame.is_some_and(|f| {
                windows.iter().any(|(start, end, serve, anchored)| {
                    (f >= *start && f <= *end)
                        || (kind == "serve" && *anchored && f < *start && *serve == Some(f))
                })
            });
            if !inside {
                outside += 1;
                continue;
            }
        }
        let when = num(&event["time_sec"])
            .map(|time| time + if absolute { 0.0 } else { offset })
            .or_else(|| frame.map(|f| f as f64 / fps + offset));
        let Some(when) = when else {
            continue;
        };
        let ms = (when * 1000.0).round();
        if !ms.is_finite() || !(0.0..=172800000.0).contains(&ms) {
            continue;
        }
        let contact_id = event["contact_id"].as_str().map(str::to_owned).or_else(|| {
            let segment = event["track_id"]
                .as_str()
                .map(str::to_owned)
                .or_else(|| integer(&event["track_id"]).map(|n| n.to_string()))?;
            Some(format!("{segment}:{}", frame?))
        });
        let id = event["id"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| integer(&event["id"]).map(|n| n.to_string()))
            .unwrap_or_else(|| format!("event-{index}"));
        actions.push(Action {
            frame,
            id,
            contact_id,
            kind: kind.to_owned(),
            time_ms: ms as i64,
            track_id: tid(&event["player_track_id"]),
            confidence: num(&event["confidence"]).unwrap_or(0.0).clamp(0.0, 1.0),
        });
    }
    Ok((actions, outside))
}

fn combine(videos: &BTreeMap<String, Stats>, roster: &BTreeMap<String, String>) -> Stats {
    let mut all=Stats{
        total_touches:0,unassigned_touches:0,recognized_touches:0,
        counts:BTreeMap::new(),
        players:roster.iter().map(|(id,name)|PlayerTotals{
            id:id.clone(),name:name.clone(),..PlayerTotals::default()
        }).collect(),
        raw:vec![],ignored_non_actions:0,ignored_excluded:0,
        duplicates_collapsed:0,median_player_touches:0.0,
        notes:"All videos counted separately. Track IDs are scoped to video and appear in per_video raw entries.",
    };
    let indexes = all
        .players
        .iter()
        .enumerate()
        .map(|(i, p)| (p.id.clone(), i))
        .collect::<HashMap<_, _>>();
    for stats in videos.values() {
        all.total_touches += stats.total_touches;
        all.unassigned_touches += stats.unassigned_touches;
        all.recognized_touches += stats.recognized_touches;
        all.ignored_non_actions += stats.ignored_non_actions;
        all.ignored_excluded += stats.ignored_excluded;
        all.duplicates_collapsed += stats.duplicates_collapsed;
        for (kind, count) in &stats.counts {
            *all.counts.entry(kind.clone()).or_insert(0) += count;
        }
        for player in &stats.players {
            if let Some(&i) = indexes.get(&player.id) {
                let target = &mut all.players[i];
                target.touches += player.touches;
                target.recognized_touches += player.recognized_touches;
                for (kind, count) in &player.actions {
                    *target.actions.entry(kind.clone()).or_insert(0) += count;
                }
            }
        }
    }
    let mut sorted = all.players.iter().map(|p| p.touches).collect::<Vec<_>>();
    sorted.sort_unstable();
    let len = sorted.len();
    let median = if len == 0 {
        0.0
    } else if len % 2 == 0 {
        (sorted[len / 2 - 1] as f64 + sorted[len / 2] as f64) / 2.0
    } else {
        sorted[len / 2] as f64
    };
    all.median_player_touches = median;
    for p in &mut all.players {
        p.activity = (median > 0.0).then_some(p.touches as f64 / median);
    }
    all.players
        .sort_by(|a, b| b.touches.cmp(&a.touches).then_with(|| a.name.cmp(&b.name)));
    all
}

fn choose_videos(
    project: &Value,
    run: &Value,
    chosen: Option<&Vec<String>>,
) -> Result<Vec<String>, ApiError> {
    let allowed = project["videos"]
        .as_array()
        .ok_or_else(|| storage::bad_request("У проекта нет списка видео"))?
        .iter()
        .filter_map(Value::as_str)
        .collect::<HashSet<_>>();
    let available = run["videos"]
        .as_object()
        .ok_or_else(|| storage::bad_request("Запуск не содержит обработанных видео"))?;
    let names = match chosen {
        Some(videos) => videos.clone(),
        None => available
            .keys()
            .filter(|name| allowed.contains(name.as_str()))
            .cloned()
            .collect(),
    };
    if names.is_empty()
        || names.len() > 30
        || names.iter().collect::<HashSet<_>>().len() != names.len()
        || names
            .iter()
            .any(|v| !allowed.contains(v.as_str()) || !available.contains_key(v))
    {
        return Err(storage::bad_request(
            "Выберите видео, принадлежащие CPU-запуску",
        ));
    }
    Ok(names)
}

#[derive(Clone)]
struct Marker {
    path: PathBuf,
    value: String,
}

fn persist(
    state: &AppState,
    id: &str,
    run: &str,
    hash: &str,
    result: &Value,
    identity_revision: &str,
    inputs: &[Marker],
) -> Result<(String, bool), ApiError> {
    let root = projects::project_root(state)
        .canonicalize()
        .map_err(|_| storage::bad_request("Изолированный реестр проектов недоступен"))?;
    let project = root
        .join(id)
        .canonicalize()
        .map_err(|_| storage::bad_request("Изолированный проект недоступен"))?;
    if project == root || !project.starts_with(&root) {
        return Err(storage::bad_request("Некорректный путь записи статистики"));
    }
    let lockfile = project.join(".player_identities.lock");
    if fs::symlink_metadata(&lockfile)
        .ok()
        .is_some_and(|m| m.file_type().is_symlink())
    {
        return Err(storage::bad_request(
            "Файл блокировки не может быть ссылкой",
        ));
    }
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(lockfile)
        .map_err(|e| storage::internal(format!("Stats identity lock: {e}")))?;
    lock.lock_exclusive()
        .map_err(|e| storage::internal(format!("Stats identity lock: {e}")))?;
    let bytes = read_raw(&project.join("player_identities.json"), MAX_IDS)?;
    if digest(bytes.as_deref()) != identity_revision {
        return Err(storage::conflict(
            "Ручные привязки изменились во время CPU-пересчёта",
        ));
    }
    for input in inputs {
        if optional_marker(&input.path)? != input.value {
            return Err(storage::conflict(
                "CPU/ReID/roster источник изменился перед сохранением статистики",
            ));
        }
    }
    let folder = project.join("reports/native-player-statistics");
    fs::create_dir_all(&folder)
        .map_err(|e| storage::internal(format!("Cannot create stats folder: {e}")))?;
    if !folder
        .canonicalize()
        .ok()
        .is_some_and(|p| p.starts_with(&project))
    {
        return Err(storage::bad_request(
            "Папка статистики выходит за пределы проекта",
        ));
    }
    let name = format!("{run}-{}.json", &hash[..24]);
    let target = folder.join(&name);
    if fs::symlink_metadata(&target)
        .ok()
        .is_some_and(|m| m.file_type().is_symlink())
    {
        return Err(storage::bad_request(
            "Файл статистики не может быть ссылкой",
        ));
    }
    if let Some(raw) = read_raw(&target, MAX_EVENTS)? {
        let existing: Value = serde_json::from_slice(&raw)
            .map_err(|_| storage::conflict("Существующий snapshot повреждён"))?;
        if existing["source_fingerprint"].as_str() != Some(hash) {
            return Err(storage::conflict(
                "Snapshot уже занят другими исходными данными",
            ));
        }
        return Ok((name, false));
    }
    storage::write_json_atomic(&target, result)?;
    Ok((name, true))
}

fn calculate(state: &AppState, req: StatisticsRequest) -> Result<Value, ApiError> {
    storage::project_id(&req.project_id)?;
    if req.run_id.len() > 104
        || !req.run_id.starts_with("run_")
        || !req
            .run_id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
    {
        return Err(storage::bad_request(
            "Выберите существующий запуск CPU-анализа",
        ));
    }
    if !matches!(req.mode.as_str(), "preview" | "recalculate") {
        return Err(storage::bad_request(
            "Режим должен быть preview или recalculate",
        ));
    }
    if req.mode == "recalculate" && !state.writes_enabled {
        return Err(storage::conflict(
            "Сохранение статистики возможно только в изолированном Rust-хранилище",
        ));
    }
    let project = projects::existing_project(state, &req.project_id)?;
    let run = analysis_runs::list_runs(state, &req.project_id)?
        .into_iter()
        .find(|row| row["id"].as_str() == Some(&req.run_id))
        .ok_or_else(|| storage::bad_request("Указанный CPU-запуск не найден"))?;
    if run["inference_only"].as_bool() == Some(true) {
        return Err(storage::bad_request(
            "Для статистики нужен CPU-результат, а не только GPU",
        ));
    }
    let videos = choose_videos(&project, &run, req.selected_videos.as_ref())?;
    let athletes = roster(state, &project)?;
    let registry_file = state.data_root.join("players/registry.json");
    let (_, roster_hash) = read_document(&registry_file, MAX_IDS, !athletes.is_empty())?;
    let identity_path = projects::project_root(state)
        .join(&req.project_id)
        .join("player_identities.json");
    let (identity, identity_hash) = read_document(&identity_path, MAX_IDS, false)?;
    let mut per_video = BTreeMap::new();
    let mut per_video_diagnostics = BTreeMap::new();
    let mut cache_ids = BTreeMap::new();
    let mut input_markers = vec![
        Marker {
            path: registry_file.clone(),
            value: optional_marker(&registry_file)?,
        },
        Marker {
            path: identity_path.clone(),
            value: optional_marker(&identity_path)?,
        },
    ];
    let mut fingerprints = vec![
        format!("algorithm:{STATS_ALGORITHM}"),
        format!("project:{}", req.project_id),
        format!("run:{}", req.run_id),
        format!("roster:{roster_hash}"),
        format!("identity:{identity_hash}"),
    ];
    for video in &videos {
        let cache_id = run["videos"][video]
            .as_str()
            .ok_or_else(|| storage::bad_request("В выбранном запуске нет видео"))?;
        let folder = rallies::cache_root(state, &req.project_id, video, cache_id)?;
        let base = if cache_id.contains('.') {
            folder
                .parent()
                .and_then(Path::parent)
                .ok_or_else(|| storage::bad_request("Неправильная CPU версия"))?
                .to_path_buf()
        } else {
            folder.clone()
        };
        let base_id = cache_id.split('.').next().unwrap_or(cache_id);
        let player_file = base.join("players.json");
        let gpu_marker = marker(&player_file)?;
        input_markers.push(Marker {
            path: player_file.clone(),
            value: gpu_marker.clone(),
        });
        let known = known_tracks(&player_file)?;
        let replay = folder.join("players.replay.json");
        input_markers.push(Marker {
            path: replay.clone(),
            value: optional_marker(&replay)?,
        });
        let available = if replay.is_file() {
            input_markers.push(Marker {
                path: replay.clone(),
                value: marker(&replay)?,
            });
            Some(known_tracks(&replay)?)
        } else {
            None
        };
        let events_file = folder.join("analysis/events.json");
        let (events, events_hash) = read_document(&events_file, MAX_EVENTS, true)?;
        input_markers.push(Marker {
            path: events_file,
            value: marker(&folder.join("analysis/events.json"))?,
        });
        let tracks_file = folder.join("analysis/game-tracks.json");
        let (tracks, tracks_hash) = read_document(&tracks_file, MAX_TRACKS, false)?;
        input_markers.push(Marker {
            path: tracks_file.clone(),
            value: optional_marker(&tracks_file)?,
        });
        let (ranges, rally_count) = if tracks.is_null() {
            (None, None)
        } else {
            let (windows, count) = rally_ranges(&tracks, &events)?;
            (Some(windows), Some(count))
        };
        let (mut actions, filtered) = actions_from_events(&events, ranges.as_deref())?;
        if let Some(visible) = &available {
            actions.retain(|a| a.track_id.as_ref().is_none_or(|t| visible.contains_key(t)));
        }
        let group_file = projects::project_root(state)
            .join(&req.project_id)
            .join("player_identity_groups")
            .join(format!("{}.json", reid_native::path_key(video, base_id)));
        let (groups, group_hash) = read_document(&group_file, MAX_IDS, false)?;
        input_markers.push(Marker {
            path: group_file.clone(),
            value: optional_marker(&group_file)?,
        });
        if !groups.is_null() {
            let stat = fs::metadata(&player_file)
                .map_err(|_| storage::bad_request("GPU players.json пропал"))?;
            let timestamp = stat
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|v| v.as_nanos());
            if groups["schema"].as_u64() != Some(1)
                || groups["stamp"]["size"].as_u64() != Some(stat.len())
                || timestamp != groups["stamp"]["mtime_ns"].as_u64().map(u128::from)
            {
                return Err(storage::conflict(
                    "ReID-группы устарели; нельзя наследовать по ним игрока",
                ));
            }
        }
        let (owners, excluded, conflicts, inherited) =
            known_owners(&identity, &groups, video, base_id, &athletes, &known);
        let participants = athletes
            .iter()
            .map(|(id, name)| Player {
                id: id.clone(),
                name: name.clone(),
                track_ids: owners
                    .iter()
                    .filter_map(|(tid, owner)| (owner == id).then_some(tid.clone()))
                    .collect(),
            })
            .collect::<Vec<_>>();
        let stats = statistics::summarize(&actions, &participants, &excluded)
            .map_err(storage::bad_request)?;
        per_video_diagnostics.insert(
            video.clone(),
            json!({
                "rallies":rally_count,
                "events_outside_rallies":filtered,
                "confirmed_tracks":owners.len().saturating_sub(inherited),
                "manual_group_inherited_tracks":inherited,
                "conflicting_confirmations":conflicts,
                "excluded_tracks":excluded.len(),
                "gpu_track_ids":known.len(),
                "replay_track_ids":available.as_ref().map(HashMap::len),
            }),
        );
        per_video.insert(video.clone(), stats);
        cache_ids.insert(video.clone(), cache_id.to_owned());
        fingerprints.push(format!("video:{video};cpu:{cache_id};events:{events_hash};tracks:{tracks_hash};groups:{group_hash};gpu:{gpu_marker}"));
    }
    let overall = combine(&per_video, &athletes);
    let fingerprint = format!("{:x}", Sha256::digest(fingerprints.join("\n").as_bytes()));
    let mut result = json!({
        "schema":"volley-native-player-statistics/1",
        "algorithm":STATS_ALGORITHM,
        "project_id":req.project_id,
        "run_id":req.run_id,
        "mode":req.mode,
        "selected_videos":videos,
        "source_fingerprint":fingerprint,
        "video_cache_ids":cache_ids,
        "input_identity_revision":identity_hash,
        "per_video":per_video,
        "overall":overall,
        "diagnostics":per_video_diagnostics,
        "saved":false,
        "disclaimer":"Counts are CPU estimates. Only reviewed manual IDs and unambiguous explicit manual group links are credited to known athletes.",
    });
    // Reject a snapshot if any saved CPU/GPU input changed while calculating.
    for input in &input_markers {
        if optional_marker(&input.path)? != input.value {
            return Err(storage::conflict(
                "CPU/GPU-источник изменился при подсчёте статистики",
            ));
        }
    }
    if req.mode == "recalculate" {
        result["saved"] = json!(true);
        let (file, created) = persist(
            state,
            &req.project_id,
            &req.run_id,
            &fingerprint,
            &result,
            &identity_hash,
            &input_markers,
        )?;
        result["snapshot_file"] = json!(file);
        result["snapshot_created"] = json!(created);
    }
    Ok(result)
}

pub async fn statistics(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<StatisticsRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    let _permit = state
        .statistics_limit
        .acquire()
        .await
        .map_err(|e| storage::internal(format!("Statistics gate: {e}")))?;
    let local = state.clone();
    let report = tokio::task::spawn_blocking(move || calculate(&local, req))
        .await
        .map_err(|e| storage::internal(format!("Native stats calculation: {e}")))??;
    Ok(Json(report))
}
