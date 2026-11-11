//! Read-only continuity and 4-second gap suggestions for a specific saved run.
//! Never interprets geometric groups as confirmed people or writes a GPU cache.
use crate::{
    analysis_runs, projects, rallies,
    storage::{self, ApiError, ApiResult},
    AppState,
};
use axum::{extract::State, http::HeaderMap, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
    sync::Arc,
};
use volley_core::continuity::{group_tracklets, ContinuityConfig, PlayerDetection};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContinuityRequest {
    pub project_id: String,
    pub run_id: String,
    pub video: String,
    #[serde(default = "default_gap_sec")]
    pub extended_gap_sec: f64,
}

fn default_gap_sec() -> f64 {
    4.0
}

fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|text| text.parse::<f64>().ok()))
        .filter(|value| value.is_finite())
}

fn as_id(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|text| text.parse::<i64>().ok()))
        .filter(|id| *id >= 0)
}

fn frame(value: &Value) -> Option<u32> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|text| text.parse::<u64>().ok()))
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value <= 10_000_000)
}

fn bbox(value: &Value) -> Option<[f64; 4]> {
    let items = value.as_array()?;
    if items.len() != 4 {
        return None;
    }
    let mut result = [0_f64; 4];
    for (index, item) in items.iter().enumerate() {
        result[index] = number(item)?;
    }
    (result[2] > result[0] && result[3] > result[1]).then_some(result)
}

fn read_document(path: &Path, limit: u64) -> Result<Value, ApiError> {
    let file =
        File::open(path).map_err(|_| storage::bad_request("Сохранённые игроки недоступны"))?;
    let meta = file
        .metadata()
        .map_err(|error| storage::internal(format!("Cannot stat player data: {error}")))?;
    if meta.len() > limit {
        return Err(storage::bad_request("Файл обнаружений слишком большой"));
    }
    serde_json::from_reader(BufReader::new(file))
        .map_err(|error| storage::bad_request(format!("Повреждённые обнаружения: {error}")))
}

pub(crate) fn excluded_ids(
    state: &AppState,
    project_id: &str,
    video: &str,
    base_cache: &str,
) -> Result<HashSet<i64>, ApiError> {
    let source = projects::project_root(state)
        .join(project_id)
        .join("player_identities.json");
    if !source.exists() {
        return Ok(HashSet::new());
    }
    let document = read_document(&source, 16 * 1024 * 1024)?;
    let key = format!("{video}::{base_cache}");
    let mut excluded = HashSet::new();
    if let Some(rows) = document["excluded_ids"][&key].as_array() {
        for row in rows {
            if let Some(id) = as_id(row) {
                excluded.insert(id);
            }
        }
    }
    Ok(excluded)
}

fn select_source(
    state: &AppState,
    request: &ContinuityRequest,
) -> Result<(PathBuf, String, String), ApiError> {
    storage::project_id(&request.project_id)?;
    storage::video_name(&request.video)?;
    ContinuityConfig {
        width: 1.0,
        height: 1.0,
        fps: 30.0,
        extended_gap_sec: request.extended_gap_sec,
    }
    .validate()
    .map_err(storage::bad_request)?;

    projects::existing_project(state, &request.project_id)?;
    let run = analysis_runs::list_runs(state, &request.project_id)?
        .into_iter()
        .find(|row| row["id"].as_str() == Some(&request.run_id))
        .ok_or_else(|| storage::bad_request("Не найден указанный запуск анализа"))?;
    let cache_id = run["videos"][&request.video]
        .as_str()
        .ok_or_else(|| storage::bad_request("Видео не входит в выбранный запуск"))?;
    let selected_cache = rallies::cache_root(state, &request.project_id, &request.video, cache_id)?;
    let base_cache = cache_id.split('.').next().unwrap_or(cache_id).to_owned();
    let player_file = selected_cache.join("players.replay.json");
    let source = if player_file.is_file() {
        player_file
    } else {
        let parent = if cache_id.contains('.') {
            selected_cache
                .parent()
                .and_then(Path::parent)
                .ok_or_else(|| storage::bad_request("Некорректный CPU-кэш"))?
                .to_path_buf()
        } else {
            selected_cache
        };
        parent.join("players.json")
    };
    Ok((source, cache_id.to_owned(), base_cache))
}

fn reconstruct(state: &AppState, request: ContinuityRequest) -> Result<Value, ApiError> {
    let (source, cache_id, base_cache) = select_source(state, &request)?;
    let saved = read_document(&source, 512 * 1024 * 1024)?;
    let records = saved["predictions"]
        .as_array()
        .ok_or_else(|| storage::bad_request("У players JSON нет predictions"))?;
    if records.len() > 5_000_000 {
        return Err(storage::bad_request(
            "Слишком много записей детектора игроков",
        ));
    }
    let sizes = &saved["source_size"];
    let width = number(&sizes["width"])
        .ok_or_else(|| storage::bad_request("Неизвестная ширина детекций"))?;
    let height = number(&sizes["height"])
        .ok_or_else(|| storage::bad_request("Неизвестная высота детекций"))?;
    let fps =
        number(&saved["fps"]).ok_or_else(|| storage::bad_request("Неизвестная FPS анализа"))?;
    let config = ContinuityConfig {
        width,
        height,
        fps,
        extended_gap_sec: request.extended_gap_sec,
    };
    config.validate().map_err(storage::bad_request)?;
    let excluded = excluded_ids(state, &request.project_id, &request.video, &base_cache)?;

    let mut players = Vec::new();
    for record in records {
        if record["class_name"].as_str() != Some("player") {
            continue;
        }
        let Some(track_id) = as_id(&record["track_id"]) else {
            continue;
        };
        let Some(frame) = frame(&record["frame_index"]) else {
            continue;
        };
        let Some(score) = number(&record["score"]) else {
            continue;
        };
        let Some(bbox) = bbox(&record["bbox_xyxy"]) else {
            continue;
        };
        players.push(PlayerDetection {
            track_id,
            frame,
            score,
            bbox,
            interpolated: record["interpolated"].as_bool() == Some(true),
        });
    }
    let report = group_tracklets(&players, config, &excluded).map_err(storage::bad_request)?;
    Ok(json!({
        "project_id":request.project_id,
        "run_id":request.run_id,
        "video":request.video,
        "cache_id":cache_id,
        "input_file":source.file_name().and_then(|name|name.to_str()),
        "source_predictions":records.len(),
        "player_detections":players.len(),
        "excluded_track_ids":excluded.len(),
        "read_only":true,
        "continuity":report,
    }))
}

pub async fn preview(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<ContinuityRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    let _permit = state
        .continuity_limit
        .acquire()
        .await
        .map_err(|e| storage::internal(format!("Continuity concurrency: {e}")))?;
    let reading = state.clone();
    let value = tokio::task::spawn_blocking(move || reconstruct(&reading, request))
        .await
        .map_err(|e| storage::internal(format!("Continuity task failed: {e}")))??;
    Ok(Json(value))
}
