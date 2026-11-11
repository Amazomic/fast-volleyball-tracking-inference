//! Read-only catalogs: source videos, global presets and player library.

use crate::{
    projects,
    storage::{self, ApiResult},
    AppState,
};
use axum::{
    extract::{Query, State},
    Json,
};
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Arc};

pub async fn get_profiles(State(state): State<Arc<AppState>>) -> ApiResult<Value> {
    let profiles = tokio::task::spawn_blocking(move || projects::read_profiles(&state))
        .await
        .map_err(|e| storage::internal(format!("Profiles task failed: {e}")))??;
    Ok(Json(json!({"profiles": profiles})))
}

pub async fn get_videos(State(state): State<Arc<AppState>>) -> ApiResult<Value> {
    let videos = tokio::task::spawn_blocking(move || projects::scan_videos(&state))
        .await
        .map_err(|e| storage::internal(format!("Videos task failed: {e}")))??;
    Ok(Json(json!({"videos": videos})))
}

pub async fn get_library(State(state): State<Arc<AppState>>) -> ApiResult<Value> {
    let players = tokio::task::spawn_blocking(move || {
        let registry = state.data_root.join("players").join("registry.json");
        let players = storage::read_json(&registry)?
            .and_then(|record| record.get("players").and_then(Value::as_array).cloned())
            .unwrap_or_default();
        Ok::<_, storage::ApiError>(players)
    })
    .await
    .map_err(|e| storage::internal(format!("Library task failed: {e}")))??;
    Ok(Json(json!({"players": players})))
}

pub async fn get_resources() -> Json<Value> {
    let cores = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1);
    let threads = ((cores as f64) * 0.8).round() as usize;
    Json(json!({
        "logical_cpu_count": cores,
        "default_threads": threads.clamp(1, 128)
    }))
}

pub async fn get_video_info(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult<Value> {
    let name = query.get("video").cloned().unwrap_or_default();
    storage::video_name(&name)?;
    let source = projects::canonical_source(&state, &name)?;
    let row = tokio::task::spawn_blocking(move || {
        let path = source
            .canonicalize()
            .map_err(|_| storage::not_found("Исходное видео не найдено"))?;
        if !path.is_file() {
            return Err(storage::not_found("Исходное видео не найдено"));
        }
        let video = volley_media::probe(&path).map_err(storage::bad_request)?;
        let mut data = serde_json::to_value(video)
            .map_err(|e| storage::internal(format!("Serialize video info: {e}")))?;
        data["video"] = json!(name);
        Ok::<_, storage::ApiError>(data)
    })
    .await
    .map_err(|e| storage::internal(format!("Video metadata task failed: {e}")))??;
    Ok(Json(row))
}
