//! Read-only whole-rally selection from an explicitly chosen CPU snapshot.
//! Names and ownership are verified against the project's saved run registry.
//! Never use a "latest" cache or silently promote a GPU-only run.

use crate::{
    analysis_runs, projects, rally_marks,
    storage::{self, ApiError, ApiResult},
    AppState,
};
use axum::{extract::State, http::HeaderMap, Json};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};
use volley_core::rallies::{self, Event, Rally, Ranking, RebuildOptions, Track, FPS};

#[derive(Clone, Debug, Deserialize)]
pub struct SelectionRequest {
    pub project_id: String,
    pub run_id: String,
    #[serde(default = "default_duration")]
    pub duration: f64,
    #[serde(default = "default_min_rally")]
    pub min_rally: f64,
    #[serde(default = "default_confidence")]
    pub min_confidence: f64,
    #[serde(default = "default_gap")]
    pub gap_sec: f64,
    #[serde(default = "default_context")]
    pub context_sec: f64,
    #[serde(default)]
    pub rally_min_sec: f64,
    #[serde(default = "default_strategy")]
    pub selection_strategy: String,
    #[serde(default)]
    pub min_action_counts: BTreeMap<String, u64>,
    #[serde(default = "default_mode")]
    pub rally_selection_mode: String,
    #[serde(default)]
    pub selected_rally_ids: Vec<String>,
    #[serde(default)]
    pub selected_videos: Option<Vec<String>>,
    /// Only previously verified athlete IDs, never raw or predicted Track IDs.
    #[serde(default)]
    pub player_id: Option<String>,
    #[serde(default = "default_player_action")]
    pub player_action: String,
    #[serde(default = "default_min_player_actions")]
    pub min_player_actions: u64,
    /// all: no gold filter; marked: gold only; unmarked: non-gold only.
    #[serde(default = "default_gold_filter")]
    pub gold_filter: String,
}

fn default_gold_filter() -> String {
    "all".into()
}
fn default_player_action() -> String {
    "all".into()
}
fn default_min_player_actions() -> u64 {
    1
}
fn default_duration() -> f64 {
    120.0
}
fn default_min_rally() -> f64 {
    1.5
}
fn default_confidence() -> f64 {
    0.5
}
fn default_gap() -> f64 {
    3.0
}
fn default_context() -> f64 {
    1.5
}
fn default_strategy() -> String {
    "highlights".into()
}
fn default_mode() -> String {
    "auto".into()
}

#[derive(Clone, Debug, Serialize)]
pub struct Scene {
    pub video: String,
    pub cache_id: String,
    pub analysis_offset_sec: f64,
    #[serde(flatten)]
    pub rally: Rally,
    /// UI renders the entire card gold when true.
    pub gold_marked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub player_action_counts: Option<BTreeMap<String, u64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub player_actions_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub player_focus_matches: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Selection {
    pub project_id: String,
    pub run_id: String,
    pub selection_strategy: String,
    pub rally_selection_mode: String,
    pub candidates: Vec<Scene>,
    pub selected: Vec<Scene>,
    pub requested_duration: f64,
    pub selected_duration: f64,
    pub gold_filter: String,
    pub gold_marked_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub player_filter: Option<PlayerFocus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matching_player_rallies: Option<usize>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PlayerFocus {
    pub player_id: String,
    pub player_name: String,
    pub player_action: String,
    pub min_player_actions: u64,
}

pub(crate) fn cache_root(
    state: &AppState,
    project_id: &str,
    video: &str,
    cache_id: &str,
) -> Result<PathBuf, ApiError> {
    if !analysis_runs::source_matches(state, project_id, video, cache_id) {
        return Err(storage::bad_request(
            "Версия CPU-анализа недоступна или не принадлежит этому видео",
        ));
    }
    let (base, revision) = cache_id
        .split_once('.')
        .map_or((cache_id, None), |(base, rev)| (base, Some(rev)));
    let base = projects::project_root(state)
        .join(project_id)
        .join("analysis")
        .join(base);
    Ok(match revision {
        Some(rev) => base.join("postprocessing").join(rev),
        None => base,
    })
}

fn json_frames(value: &Value) -> Option<u64> {
    match value {
        Value::Number(n) => n
            .as_u64()
            .or_else(|| n.as_i64().and_then(|n| u64::try_from(n).ok())),
        Value::String(s) => s.parse::<u64>().ok(),
        _ => None,
    }
}
fn json_number(value: &Value) -> Option<f64> {
    let n = match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse::<f64>().ok(),
        _ => None,
    }?;
    n.is_finite().then_some(n)
}
fn read_document(path: &Path, required: bool) -> Result<Value, ApiError> {
    match storage::read_json(path)? {
        Some(value) => Ok(value),
        None if !required => Ok(Value::Null),
        None => Err(storage::bad_request(format!(
            "Для выбранного CPU-запуска отсутствует {}",
            path.file_name().unwrap_or_default().to_string_lossy()
        ))),
    }
}

fn decode_tracks(raw: &Value) -> Result<Vec<Track>, ApiError> {
    let rows = raw
        .as_array()
        .ok_or_else(|| storage::bad_request("Некорректный game-tracks.json"))?;
    let mut tracks = Vec::new();
    for item in rows {
        let Some(start) = json_frames(&item["start_frame"]) else {
            continue;
        };
        let Some(end) = json_frames(&item["last_frame"]) else {
            continue;
        };
        let Some(id) = item["track_id"].as_i64().or_else(|| {
            item["track_id"]
                .as_str()
                .and_then(|s| s.parse::<i64>().ok())
        }) else {
            continue;
        };
        if end < start {
            continue;
        }
        let label = &item["rally_classification"];
        let is_rally =
            label["label"].as_str() == Some("rally") || label["is_rally"].as_bool() == Some(true);
        let technical_return =
            !is_rally && label["not_rally_reason"].as_str() == Some("technical_return");
        let mut features = BTreeMap::new();
        if let Some(map) = item["rally_features"].as_object() {
            for (name, value) in map {
                if let Some(count) = json_frames(value) {
                    features.insert(name.clone(), count);
                }
            }
        }
        tracks.push(Track {
            id,
            start,
            end,
            is_rally,
            technical_return,
            technical_return_confidence: json_number(&label["technical_return_confidence"]),
            rally_confidence: json_number(&label["rally_confidence"]).unwrap_or(0.0),
            action_features: features,
        });
    }
    Ok(tracks)
}

fn decode_events(raw: &Value) -> Vec<Event> {
    raw["events"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let frame = json_frames(&row["frame"])?;
                    let kind = row["type"].as_str()?.to_owned();
                    let id = row["id"]
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| row["id"].as_i64().map(|id| id.to_string()));
                    let player_track_id = row["player_track_id"]
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| row["player_track_id"].as_i64().map(|id| id.to_string()));
                    Some(Event {
                        kind,
                        frame,
                        confidence: json_number(&row["confidence"]).unwrap_or(0.0),
                        id,
                        player_track_id,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn decode_ranking(raw: &Value) -> Result<Vec<Ranking>, ApiError> {
    let rows = raw
        .as_array()
        .ok_or_else(|| storage::bad_request("Некорректный ranking.json"))?;
    Ok(rows
        .iter()
        .filter_map(|item| {
            Some(Ranking {
                start: json_frames(&item["start"])?,
                end: json_frames(&item["end"])?,
                score: json_number(&item["score"])?,
                confidence: json_number(&item["confidence"]).unwrap_or(0.0),
            })
        })
        .collect())
}

fn validate_request(request: &SelectionRequest) -> Result<(), ApiError> {
    storage::project_id(&request.project_id)?;
    if request.run_id.is_empty()
        || request.run_id.len() > 104
        || !request.run_id.starts_with("run_")
        || !request
            .run_id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
    {
        return Err(storage::bad_request(
            "Укажите конкретный запуск CPU-обработки",
        ));
    }
    let valid_seconds = [
        (request.duration, 1.0, 600.0),
        (request.min_rally, 1.5, 30.0),
        (request.min_confidence, 0.05, 1.0),
        (request.gap_sec, 0.0, 12.0),
        (request.context_sec, 0.0, 10.0),
        (request.rally_min_sec, 0.0, 40.0),
    ];
    if valid_seconds
        .iter()
        .any(|(value, min, max)| !value.is_finite() || value < min || value > max)
    {
        return Err(storage::bad_request(
            "Числовые фильтры розыгрышей вне допустимого диапазона",
        ));
    }
    if !matches!(
        request.selection_strategy.as_str(),
        "highlights"
            | "longest"
            | "most_actions"
            | "attacks"
            | "blocks"
            | "digs"
            | "passes"
            | "serves"
    ) {
        return Err(storage::bad_request("Неизвестный критерий отбора"));
    }
    if !matches!(request.rally_selection_mode.as_str(), "auto" | "manual") {
        return Err(storage::bad_request("Некорректный режим выбора розыгрышей"));
    }
    if request.selected_rally_ids.len() > 500
        || request
            .selected_rally_ids
            .iter()
            .any(|id| id.len() != 24 || !id.bytes().all(|b| b.is_ascii_hexdigit()))
        || request
            .selected_rally_ids
            .iter()
            .collect::<HashSet<_>>()
            .len()
            != request.selected_rally_ids.len()
    {
        return Err(storage::bad_request("Некорректный список ID розыгрышей"));
    }
    if request.player_id.as_ref().is_some_and(|id| {
        id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
    }) || !matches!(
        request.player_action.as_str(),
        "all"
            | "attack"
            | "block"
            | "serve"
            | "dig"
            | "overhead_pass"
            | "low_touch"
            | "unknown_touch"
    ) || request.min_player_actions == 0
        || request.min_player_actions > 100
        || (request.player_id.is_none()
            && (request.player_action != "all" || request.min_player_actions != 1))
    {
        return Err(storage::bad_request(
            "Укажите игрока проекта и допустимый фильтр его игровых действий",
        ));
    }
    if !matches!(request.gold_filter.as_str(), "all" | "marked" | "unmarked") {
        return Err(storage::bad_request(
            "Фильтр золотых розыгрышей должен быть all, marked или unmarked",
        ));
    }
    if request.min_action_counts.iter().any(|(key, value)| {
        !matches!(
            key.as_str(),
            "actions" | "attack" | "block" | "dig" | "overhead_pass" | "serve"
        ) || *value > 100
    }) {
        return Err(storage::bad_request(
            "Некорректные фильтры игровых действий",
        ));
    }
    Ok(())
}

fn load_video_rallies(
    state: &AppState,
    request: &SelectionRequest,
    run: &Value,
    video: &str,
    cache_id: &str,
) -> Result<Vec<Scene>, ApiError> {
    let cache = cache_root(state, &request.project_id, video, cache_id)?;
    let tracks = decode_tracks(&read_document(
        &cache.join("analysis/game-tracks.json"),
        true,
    )?)?;
    let events_doc = read_document(&cache.join("analysis/events.json"), true)?;
    let events = decode_events(&events_doc);
    let rank_doc = read_document(&cache.join("ranking.json"), false)?;
    if rank_doc.is_null() && request.rally_selection_mode == "auto" {
        return Err(storage::bad_request(
            "Для автоматического монтажа отсутствует рейтинг выбранного CPU-анализа",
        ));
    }
    let ranking = if rank_doc.is_null() {
        Vec::new()
    } else {
        decode_ranking(&rank_doc)?
    };
    let offset = json_number(&events_doc["analysis_offset_sec"])
        .or_else(|| json_number(&run["video_ranges"][video]["start"]))
        .unwrap_or(0.0);
    if offset < 0.0 || offset > 86400.0 {
        return Err(storage::bad_request("Неправильное смещение CPU-анализа"));
    }
    let analysis_mp4 = if cache_id.contains('.') {
        cache
            .parent()
            .and_then(Path::parent)
            .map(|p| p.join("analysis.mp4"))
    } else {
        Some(cache.join("analysis.mp4"))
    };
    let duration_frames = analysis_mp4
        .filter(|p| p.is_file())
        .and_then(|p| volley_media::probe(&p).ok())
        .map(|v| (v.duration_sec * FPS).round().max(1.0) as u64);
    let cfg = RebuildOptions {
        max_gap_frames: (request.gap_sec * FPS).round() as u64,
        padding_frames: (request.context_sec * FPS).round() as u64,
        min_frames: (request.rally_min_sec * FPS).round() as u64,
        duration_frames,
    };
    let mut groups = rallies::rebuild(&tracks, &events, &ranking, cfg);
    for group in &mut groups {
        group.rally_id = Some(rallies::rally_selection_id(video, cache_id, group));
        group.ranking_score = rallies::selection_score(group, &request.selection_strategy)
            .map_err(storage::bad_request)?;
        group.ranking_score = (group.ranking_score * 10000.0).round() / 10000.0;
        group.selection_strategy = request.selection_strategy.clone();
    }
    Ok(groups
        .into_iter()
        .filter(|r| !r.track_ids.is_empty())
        .map(|r| Scene {
            video: video.to_owned(),
            cache_id: cache_id.to_owned(),
            analysis_offset_sec: offset,
            rally: r,
            gold_marked: false,
            player_action_counts: None,
            player_actions_total: None,
            player_focus_matches: None,
        })
        .collect())
}

fn select(state: &AppState, request: &SelectionRequest) -> Result<Selection, ApiError> {
    validate_request(request)?;
    let project = projects::existing_project(state, &request.project_id)?;
    let videos = project["videos"]
        .as_array()
        .ok_or_else(|| storage::bad_request("Некорректный список видео проекта"))?;
    let listed: Vec<&str> = videos.iter().filter_map(Value::as_str).collect();
    let chosen: Vec<&str> = if let Some(selected) = &request.selected_videos {
        if selected.is_empty()
            || selected.len() > listed.len()
            || selected.iter().collect::<HashSet<_>>().len() != selected.len()
            || selected.iter().any(|v| !listed.contains(&v.as_str()))
        {
            return Err(storage::bad_request("Выбраны видео не из проекта"));
        }
        listed
            .into_iter()
            .filter(|name| selected.iter().any(|chosen| chosen == name))
            .collect()
    } else {
        listed
    };
    let runs = analysis_runs::list_runs(state, &request.project_id)?;
    let run = runs
        .iter()
        .find(|v| v["id"].as_str() == Some(&request.run_id))
        .ok_or_else(|| storage::bad_request("Выбранный запуск анализа недоступен"))?;
    if run["inference_only"].as_bool() == Some(true) {
        return Err(storage::bad_request(
            "Нужны результаты CPU-обработки, а не только GPU-инференс",
        ));
    }
    let marks = rally_marks::marked_anchors(state, &request.project_id, &request.run_id)?;
    let mut all = Vec::new();
    let mut focused_name = None;
    for video in chosen {
        let cache = run["videos"][video]
            .as_str()
            .ok_or_else(|| storage::bad_request(format!("Нет CPU-результата для {video}")))?;
        let mut scenes = load_video_rallies(state, request, run, video, cache)?;
        if let Some(player_id) = request.player_id.as_deref() {
            let player_name = crate::player_montage::annotate_rallies(
                state,
                &request.project_id,
                &project,
                video,
                cache,
                player_id,
                &request.player_action,
                &request.selection_strategy,
                &mut scenes,
            )?;
            focused_name = Some(player_name);
        }
        for scene in &mut scenes {
            scene.gold_marked = marks.contains(&rally_marks::anchor(scene));
        }
        all.extend(scenes);
    }
    let gold_marked_count = all.iter().filter(|s| s.gold_marked).count();
    // A player focus only admits rallies containing confirmed action evidence.
    // ReID appearance and continuity suggestions are never accepted as proof.
    // Stable rally IDs still refer to intact original source-frame intervals.
    let candidates: Vec<Scene> = all
        .into_iter()
        .filter(|scene| {
            request.player_id.is_none()
                || scene.player_focus_matches.unwrap_or(0) >= request.min_player_actions
        })
        .filter(|scene| match request.gold_filter.as_str() {
            "marked" => scene.gold_marked,
            "unmarked" => !scene.gold_marked,
            _ => true,
        })
        .collect();
    let matching_player_rallies = request.player_id.as_ref().map(|_| candidates.len());
    // Manual mode trusts only known stable IDs from the selected run, never
    // arbitrary client-provided offsets or frame ranges.
    let mut selected: Vec<Scene> = if request.rally_selection_mode == "manual" {
        let by_id: HashMap<&str, &Scene> = candidates
            .iter()
            .filter_map(|scene| Some((scene.rally.rally_id.as_deref()?, scene)))
            .collect();
        let mut chosen = Vec::new();
        for id in &request.selected_rally_ids {
            chosen.push(
                (*by_id.get(id.as_str()).ok_or_else(|| {
                    storage::bad_request("ID розыгрыша устарел — обновите список")
                })?)
                .clone(),
            );
        }
        chosen
    } else {
        candidates
            .iter()
            .filter(|scene| {
                // Explicit manual gold marking is sufficient eligibility
                // evidence for an unranked rally. Duration/confidence/action
                // thresholds still apply; no fragment is ever created.
                let mut eligibility = scene.rally.clone();
                if request.gold_filter == "marked" && scene.gold_marked {
                    eligibility.ranked = true;
                }
                rallies::ranked_eligible(
                    &eligibility,
                    request.min_rally,
                    request.min_confidence,
                    &request.min_action_counts,
                    if request.player_id.is_some() {
                        "highlights" // focus-specific event evidence is checked above.
                    } else {
                        &request.selection_strategy
                    },
                )
            })
            .cloned()
            .collect()
    };
    if request.rally_selection_mode == "auto" {
        selected.sort_by(|a, b| {
            b.rally
                .ranking_score
                .total_cmp(&a.rally.ranking_score)
                .then_with(|| b.rally.duration_sec.total_cmp(&a.rally.duration_sec))
        });
        let cap = (request.duration * FPS).round() as u64;
        let mut used = 0_u64;
        selected.retain(|scene| {
            let frames = scene.rally.source_end_frame - scene.rally.source_start_frame + 1;
            if used.saturating_add(frames) <= cap {
                used += frames;
                true
            } else {
                false
            }
        });
    }
    let order = project["videos"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .enumerate()
        .map(|(i, s)| (s.to_owned(), i))
        .collect::<HashMap<_, _>>();
    selected.sort_by(|a, b| {
        order
            .get(&a.video)
            .cmp(&order.get(&b.video))
            .then_with(|| a.rally.source_start_frame.cmp(&b.rally.source_start_frame))
    });
    let mut timeline = 0_u64;
    for (i, scene) in selected.iter_mut().enumerate() {
        scene.rally.sequence = Some(i + 1);
        scene.rally.montage_start_sec = Some(((timeline as f64 / FPS) * 10000.0).round() / 10000.0);
        timeline += scene.rally.source_end_frame - scene.rally.source_start_frame + 1;
    }
    if timeline > 600 * 30 {
        return Err(storage::bad_request(
            "Выбранные полные розыгрыши превышают 600 секунд",
        ));
    }
    Ok(Selection {
        project_id: request.project_id.clone(),
        run_id: request.run_id.clone(),
        selection_strategy: request.selection_strategy.clone(),
        rally_selection_mode: request.rally_selection_mode.clone(),
        candidates,
        selected,
        requested_duration: request.duration,
        selected_duration: ((timeline as f64 / FPS) * 1000.0).round() / 1000.0,
        gold_filter: request.gold_filter.clone(),
        gold_marked_count,
        player_filter: request.player_id.as_ref().map(|id| PlayerFocus {
            player_id: id.clone(),
            player_name: focused_name.unwrap_or_default(),
            player_action: request.player_action.clone(),
            min_player_actions: request.min_player_actions,
        }),
        matching_player_rallies,
    })
}

pub(crate) fn select_for_montage(
    state: &AppState,
    request: &SelectionRequest,
) -> Result<Selection, ApiError> {
    select(state, request)
}

pub async fn preview_selection(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<SelectionRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    let output = tokio::task::spawn_blocking(move || select(&state, &request))
        .await
        .map_err(|error| storage::internal(format!("Rally planning failed: {error}")))??;
    Ok(Json(serde_json::to_value(output).map_err(|error| {
        storage::internal(format!("Rally JSON failed: {error}"))
    })?))
}
