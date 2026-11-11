//! Analysis execution history: read-only, original files are never rewritten.
use crate::{
    projects,
    storage::{self, ApiResult},
    AppState,
};
use axum::{
    extract::{Query, State},
    Json,
};
use chrono::DateTime;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
    sync::Arc,
};

fn seconds(value: Option<&Value>) -> Value {
    let seconds = value.and_then(|v| match v {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.parse::<f64>().ok(),
        _ => None,
    });
    match seconds {
        Some(n) if n.is_finite() && (0.0..365.0 * 24.0 * 3600.0).contains(&n) => {
            json!((n * 100.0).round() / 100.0)
        }
        _ => Value::Null,
    }
}

fn timestamp(value: Option<&Value>) -> i64 {
    value
        .and_then(Value::as_str)
        .and_then(|str_value| {
            DateTime::parse_from_rfc3339(str_value)
                .ok()
                .map(|parsed| parsed.timestamp_millis())
        })
        .unwrap_or(0)
}

fn valid_stages(stages: Option<&Value>) -> bool {
    stages.and_then(Value::as_array).is_some_and(|stages| {
        !stages.is_empty()
            && stages.iter().all(|stage| {
                stage.get("stage_index").is_some_and(Value::is_i64)
                    && stage
                        .get("stage")
                        .and_then(Value::as_str)
                        .is_some_and(|name| !name.trim().is_empty())
                    && !seconds(stage.get("duration_sec")).is_null()
            })
    })
}

fn mapped_stages(value: Option<&Value>) -> Vec<Value> {
    value
        .and_then(Value::as_array)
        .map(|stages| {
            stages
                .iter()
                .filter(|stage| stage.is_object())
                .map(|stage| {
                    json!({
                        "stage_index": stage.get("stage_index"),
                        "stage": stage.get("stage"),
                        "duration_sec": seconds(stage.get("duration_sec")),
                        "fps": stage.get("fps")
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn named_profile(data: &Value) -> Option<String> {
    data.get("profile_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|profile| !profile.is_empty())
        .map(str::to_owned)
}

fn json_files(path: &Path) -> Vec<std::path::PathBuf> {
    let Ok(directory) = fs::read_dir(path) else {
        return Vec::new();
    };
    directory
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.extension().is_some_and(|e| e == "json"))
        .collect()
}

fn initial_stages(mode: &str) -> Vec<&'static str> {
    match mode {
        "ball" => vec!["Подготовка фрагмента", "Обнаружение мяча"],
        "players" => vec!["Подготовка фрагмента", "Обнаружение игроков"],
        "full" => vec![
            "Подготовка фрагмента",
            "Повторное использование видео",
            "Обнаружение мяча",
            "Обнаружение игроков",
            "Трекинг и розыгрыши",
            "Игровые события",
            "Формирование результатов",
        ],
        _ => Vec::new(),
    }
}

fn mode_label(mode: &str) -> &'static str {
    match mode {
        "ball" => "Только мяч",
        "players" => "Только игроки",
        "full" => "Полный анализ фрагмента",
        _ => "Фрагмент",
    }
}

fn collect_history(state: &AppState, id: &str) -> Result<Vec<Value>, storage::ApiError> {
    let project = projects::existing_project(state, id)?;
    let Some(names) = project.get("videos").and_then(Value::as_array) else {
        return Err(storage::internal("Project videos are malformed"));
    };
    let allowed: HashSet<&str> = names.iter().filter_map(Value::as_str).collect();
    let root = state.data_root.join("projects").join(id);
    let timings = root.join("runs").join("analysis_timings");
    let mut all = Vec::new();

    for file in json_files(&timings) {
        let Ok(Some(entry)) = storage::read_json(&file) else {
            continue;
        };
        if entry["status"].as_str() != Some("complete") {
            continue;
        }
        let Some(profile) = named_profile(&entry) else {
            continue;
        };
        if !entry["settings"].is_object() {
            continue;
        }
        let Some(stage_names) = entry["stage_names"].as_array() else {
            continue;
        };
        if stage_names.is_empty() {
            continue;
        }
        let Some(videos) = entry["videos"].as_array() else {
            continue;
        };
        if videos.is_empty()
            || videos.len()
                != entry["videos_total"]
                    .as_u64()
                    .map(|number| number as usize)
                    .unwrap_or(videos.len())
        {
            continue;
        }
        if videos.iter().any(|row| {
            row["status"].as_str() != Some("complete")
                || !row["video"]
                    .as_str()
                    .is_some_and(|name| allowed.contains(name))
                || !valid_stages(row.get("stages"))
                || row["stages"]
                    .as_array()
                    .is_none_or(|v| v.len() != stage_names.len())
        }) {
            continue;
        }
        let Some(stem) = file.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let result_videos: Vec<Value> = videos
            .iter()
            .map(|row| {
                json!({
                    "video": row.get("video"),
                    "status": row.get("status"),
                    "duration_sec": seconds(row.get("duration_sec")),
                    "stages": mapped_stages(row.get("stages"))
                })
            })
            .collect();
        all.push(json!({
            "id": format!("run_{stem}"),
            "kind": "project",
            "profile": profile,
            "profile_id": entry.get("profile_id").filter(|v| v.as_str().is_some_and(|s| !s.is_empty())).unwrap_or(&json!("legacy")),
            "started_at": entry.get("started_at"),
            "finished_at": entry.get("finished_at"),
            "status": "complete",
            "duration_sec": seconds(entry.get("duration_sec")),
            "videos_total": entry.get("videos_total").cloned().unwrap_or(json!(result_videos.len())),
            "stage_names": entry.get("stage_names"),
            "videos": result_videos,
            "error": entry.get("error")
        }));
    }

    for file in json_files(&root.join("experiments/runs")) {
        let Ok(Some(entry)) = storage::read_json(&file) else {
            continue;
        };
        if entry["status"].as_str() != Some("complete")
            || !entry["video"].as_str().is_some_and(|v| allowed.contains(v))
            || !entry["settings"].is_object()
            || !valid_stages(entry.get("stages"))
        {
            continue;
        }
        let Some(profile) = named_profile(&entry) else {
            continue;
        };
        let Some(stem) = file.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let mode = entry["mode"].as_str().unwrap_or_default();
        let stages = mapped_stages(entry.get("stages"));
        let recorded_names: Vec<&str> = stages
            .iter()
            .filter_map(|stage| stage["stage"].as_str())
            .filter(|name| !name.is_empty())
            .collect();
        let stage_names: Vec<String> = if recorded_names.is_empty() {
            initial_stages(mode)
                .into_iter()
                .map(str::to_owned)
                .collect()
        } else {
            recorded_names.into_iter().map(str::to_owned).collect()
        };
        let duration = seconds(entry.get("elapsed_sec"));
        all.push(json!({
            "id": format!("experiment_{stem}"),
            "kind": "fragment",
            "mode": entry.get("mode"),
            "profile": profile,
            "started_at": entry.get("started_at"),
            "finished_at": entry.get("finished_at"),
            "status": "complete",
            "duration_sec": duration,
            "videos_total": 1,
            "stage_names": stage_names,
            "videos": [{
                "video": entry.get("video"),
                "status": "complete",
                "duration_sec": duration,
                "start": entry.get("start"),
                "seconds": entry.get("seconds"),
                "stages": stages
            }],
            "mode_label": mode_label(mode),
            "error": entry.get("error")
        }));
    }
    all.sort_by_key(|entry| std::cmp::Reverse(timestamp(entry.get("started_at"))));
    Ok(all)
}

pub async fn get_history(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult<Value> {
    let id = query.get("project_id").cloned().unwrap_or_default();
    let rows = tokio::task::spawn_blocking(move || collect_history(&state, &id))
        .await
        .map_err(|error| storage::internal(format!("History task failed: {error}")))??;
    Ok(Json(json!({"history": rows})))
}
