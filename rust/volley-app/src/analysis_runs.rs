//! Versioned analysis sessions and CPU snapshots loaded natively in Rust.
//! This module never materializes or modifies a Python-generated run branch.
use crate::{
    projects,
    storage::{self, ApiResult},
    AppState,
};
use axum::{
    extract::{Query, State},
    Json,
};
use chrono::{DateTime, Local};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
    sync::Arc,
    time::SystemTime,
};

fn valid_hex(value: &str, min_len: usize, max_len: usize) -> bool {
    (min_len..=max_len).contains(&value.len())
        && value
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

fn valid_cache_id(value: &str) -> bool {
    match value.split_once('.') {
        Some((base, version)) => valid_hex(base, 16, 64) && valid_hex(version, 16, 64),
        None => valid_hex(value, 16, 64),
    }
}

fn valid_run_id(value: &str) -> bool {
    value.len() >= 10
        && value.len() <= 104
        && value.starts_with("run_")
        && value[4..]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

/// Python's json.dumps(..., ensure_ascii=False, sort_keys=True) representation.
/// This string is hashed for legacy CPU version IDs. Preserve object key order
/// and Python's spaces after colon/comma (serde_json's default omits them).
fn python_sorted_json(value: &Value) -> String {
    match value {
        Value::Object(object) => {
            let mut keys = object.keys().collect::<Vec<_>>();
            keys.sort();
            let inner = keys
                .into_iter()
                .map(|key| {
                    let name = serde_json::to_string(key).expect("JSON key");
                    format!("{name}: {}", python_sorted_json(&object[key]))
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{{inner}}}")
        }
        Value::Array(rows) => format!(
            "[{}]",
            rows.iter()
                .map(python_sorted_json)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => serde_json::to_string(value).expect("Serializable JSON"),
    }
}

fn settings_hash(settings: &Value) -> String {
    let bytes = Sha256::digest(python_sorted_json(settings).as_bytes());
    let hex = format!("{bytes:x}");
    hex[..12].to_owned()
}

pub(crate) fn source_matches(state: &AppState, project_id: &str, name: &str, cache: &str) -> bool {
    if !valid_cache_id(cache) {
        return false;
    }
    let cache_root = projects::project_root(state)
        .join(project_id)
        .join("analysis");
    let (base, version) = cache
        .split_once('.')
        .map_or((cache, None), |(base, version)| (base, Some(version)));
    let raw_base = cache_root.join(base);
    let path = if let Some(version) = version {
        raw_base.join("postprocessing").join(version)
    } else {
        raw_base.clone()
    };

    if let Some(version) = version {
        let Ok(Some(manifest)) = storage::read_json(&path.join("postprocess.manifest.json")) else {
            return false;
        };
        let Some(inference_cache) = manifest.get("inference_cache").and_then(Value::as_str) else {
            return false;
        };
        let recorded_base = Path::new(inference_cache);
        if manifest["schema"].as_str() != Some("volley-postprocessing/1")
            || manifest["version_id"].as_str() != Some(version)
            || recorded_base.canonicalize().ok() != raw_base.canonicalize().ok()
            || !path.join("ranking.json").is_file()
        {
            return false;
        }
    }

    if projects::event_source_matches(
        state,
        &path.join("analysis").join("events.json"),
        name,
        true,
    ) {
        return true;
    }
    if version.is_some() {
        return false;
    }
    let Ok(Some(manifest)) = storage::read_json(&path.join("inference.manifest.json")) else {
        return false;
    };
    if manifest["schema"].as_str() != Some("volley-inference/1")
        || !path.join("analysis.mp4").is_file()
        || !path.join("players.json").is_file()
        || !path.join("ball/analysis/ball.csv").is_file()
    {
        return false;
    }
    let Some(source) = manifest["source"].as_str() else {
        return false;
    };
    let source = Path::new(source);
    let raw_source = if source.is_absolute() {
        source.to_path_buf()
    } else {
        state.repository_root.join(source)
    };
    raw_source.canonicalize().ok() == projects::canonical_source(state, name).ok()
}

fn optional_record(path: &Path) -> Value {
    storage::read_json(path)
        .ok()
        .flatten()
        .unwrap_or_else(|| json!({}))
}

fn snapshot_runs(state: &AppState, project_id: &str, run: &Value) -> (Vec<Value>, HashSet<String>) {
    let mut snapshots = Vec::new();
    let mut suppressed = HashSet::new();
    let Some(parent) = run["id"].as_str() else {
        return (snapshots, suppressed);
    };
    let Some(versions) = run["videos"].as_object() else {
        return (snapshots, suppressed);
    };
    let dir = projects::project_root(state)
        .join(project_id)
        .join("runs/cpu_snapshots")
        .join(parent);
    let Ok(entries) = fs::read_dir(dir) else {
        return (snapshots, suppressed);
    };
    for entry in entries.flatten() {
        let file = entry.path();
        if file.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let snap = optional_record(&file);
        let Some(snap_id) = snap["id"].as_str() else {
            continue;
        };
        if snap["schema"].as_str() != Some("volley-cpu-snapshot/1")
            || snap["parent_run_id"].as_str() != Some(parent)
            || !snap_id.starts_with(&format!("{parent}_pp_"))
            || !valid_run_id(snap_id)
            || !snap["postprocess_settings"].is_object()
        {
            continue;
        }
        let Some(saved) = snap["videos"].as_object() else {
            continue;
        };
        if saved.len() != versions.len() || !versions.keys().all(|k| saved.contains_key(k)) {
            continue;
        }
        if saved.iter().any(|(video, value)| {
            let Some(cache_id) = value.as_str() else {
                return true;
            };
            let Some(base_id) = versions[video].as_str() else {
                return true;
            };
            !cache_id.starts_with(&format!("{base_id}."))
                || !source_matches(state, project_id, video, cache_id)
        }) {
            continue;
        }
        let mut result = run.as_object().cloned().unwrap_or_default();
        result.insert("id".into(), json!(snap_id));
        result.insert(
            "profile".into(),
            json!(format!(
                "{} / {}",
                run["profile"].as_str().unwrap_or_default(),
                snap["postprocess_settings"]["mode"]
                    .as_str()
                    .unwrap_or("CPU")
            )),
        );
        result.insert("videos".into(), snap["videos"].clone());
        result.insert("parent_run_id".into(), json!(parent));
        result.insert(
            "postprocess_settings".into(),
            snap["postprocess_settings"].clone(),
        );
        result.insert("started_at".into(), snap["started_at"].clone());
        result.insert("cpu_snapshot".into(), json!(true));
        snapshots.push(Value::Object(result));

        if snap["suppress_legacy"].as_bool() == Some(true) {
            suppressed.insert(format!(
                "{parent}_pp_{}",
                settings_hash(&snap["postprocess_settings"])
            ));
        }
    }
    (snapshots, suppressed)
}

fn legacy_postprocessing_runs(state: &AppState, id: &str, run: &Value) -> Vec<Value> {
    let (mut results, suppressed) = snapshot_runs(state, id, run);
    let Some(base_versions) = run["videos"].as_object() else {
        return results;
    };
    // Each source video must have a complete postprocessed version with the
    // same saved settings; intersection of configs is used, never just one.
    let mut available: HashMap<&str, HashMap<String, (String, SystemTime)>> = HashMap::new();
    for (video, base_id) in base_versions {
        let Some(base_id) = base_id.as_str() else {
            return results;
        };
        if !valid_cache_id(base_id) || base_id.contains('.') {
            return results;
        }
        let path = projects::project_root(state)
            .join(id)
            .join("analysis")
            .join(base_id)
            .join("postprocessing");
        let Ok(versions) = fs::read_dir(path) else {
            return results;
        };
        let mut matched = HashMap::new();
        for entry in versions.flatten() {
            let path = entry.path();
            let Some(version) = path.file_name().and_then(|v| v.to_str()) else {
                continue;
            };
            if !path.is_dir() || !valid_hex(version, 16, 64) {
                continue;
            }
            let manifest = path.join("postprocess.manifest.json");
            let config = optional_record(&manifest);
            if !config["settings"].is_object() {
                continue;
            }
            let cache = format!("{base_id}.{version}");
            if !source_matches(state, id, video, &cache) {
                continue;
            }
            let key = python_sorted_json(&config["settings"]);
            let timestamp = manifest
                .metadata()
                .and_then(|file| file.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            matched.insert(key, (cache, timestamp));
        }
        if matched.is_empty() {
            return results;
        }
        available.insert(video, matched);
    }

    let Some((_, first)) = available.iter().next() else {
        return results;
    };
    let mut common: Vec<String> = first
        .keys()
        .filter(|key| available.values().all(|m| m.contains_key(*key)))
        .cloned()
        .collect();
    common.sort();
    let Some(parent) = run["id"].as_str() else {
        return results;
    };
    for settings_json in common {
        let hash =
            settings_hash(&serde_json::from_str::<Value>(&settings_json).unwrap_or(Value::Null));
        let id_value = format!("{parent}_pp_{hash}");
        if suppressed.contains(&id_value) {
            continue;
        }
        let mut mapping = Map::new();
        let mut newest = SystemTime::UNIX_EPOCH;
        for (video, variants) in &available {
            let (cache, stamp) = &variants[&settings_json];
            mapping.insert((*video).to_owned(), json!(cache));
            newest = newest.max(*stamp);
        }
        let configuration = serde_json::from_str::<Value>(&settings_json).unwrap_or(json!({}));
        let mut result = run.as_object().cloned().unwrap_or_default();
        result.insert("id".into(), json!(id_value));
        result.insert(
            "profile".into(),
            json!(format!(
                "{} / {}",
                run["profile"].as_str().unwrap_or_default(),
                configuration["mode"].as_str().unwrap_or("CPU")
            )),
        );
        result.insert("videos".into(), Value::Object(mapping));
        result.insert("parent_run_id".into(), json!(parent));
        result.insert("postprocess_settings".into(), configuration);
        let millis = newest
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let started = DateTime::from_timestamp_millis(millis as i64)
            .map(|date| date.with_timezone(&Local).to_rfc3339())
            .unwrap_or_default();
        result.insert("started_at".into(), json!(started));
        results.push(Value::Object(result));
    }
    results
}

fn started_timestamp(value: &Value) -> i64 {
    value["started_at"]
        .as_str()
        .and_then(|value| {
            DateTime::parse_from_rfc3339(value)
                .ok()
                .map(|dt| dt.timestamp_millis())
        })
        .unwrap_or(0)
}

pub(crate) fn list_runs(state: &AppState, id: &str) -> Result<Vec<Value>, storage::ApiError> {
    let project = projects::existing_project(state, id)?;
    let Some(videos) = project["videos"].as_array() else {
        return Err(storage::internal("Project videos are invalid"));
    };
    let allowed = videos
        .iter()
        .filter_map(Value::as_str)
        .collect::<HashSet<_>>();
    let root = projects::project_root(state).join(id);
    let timing_dir = root.join("runs/analysis_timings");
    let Ok(iter) = fs::read_dir(&timing_dir) else {
        return Ok(Vec::new());
    };
    let mut files = iter
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    files.sort_by(|a, b| b.cmp(a));

    let mut runs = Vec::new();
    for file in files {
        let entry = optional_record(&file);
        if entry["status"].as_str() != Some("complete") {
            continue;
        }
        let Some(profile) = entry["profile_name"].as_str().map(str::trim) else {
            continue;
        };
        if profile.is_empty() {
            continue;
        }
        let Some(rows) = entry["videos"].as_array() else {
            continue;
        };
        if rows.len() != allowed.len()
            || rows
                .iter()
                .any(|row| row["status"].as_str() != Some("complete"))
        {
            continue;
        }
        let stages = entry.get("stage_names").and_then(Value::as_array);
        if let Some(expected) = stages.filter(|s| !s.is_empty()) {
            if rows.iter().any(|row| {
                row.get("stages")
                    .and_then(Value::as_array)
                    .is_none_or(|actual| {
                        actual.len() != expected.len()
                            || actual.iter().enumerate().any(|(idx, stage)| {
                                stage["stage_index"].as_u64() != Some((idx + 1) as u64)
                            })
                    })
            }) {
                continue;
            }
        }
        let mut mappings = Map::new();
        let mut ranges = Map::new();
        let mut processed = Vec::new();
        for row in rows {
            let Some(video) = row["video"].as_str() else {
                continue;
            };
            let Some(cache) = row["cache_id"].as_str() else {
                continue;
            };
            if !allowed.contains(video) || !source_matches(state, id, video, cache) {
                continue;
            }
            mappings.insert(video.to_owned(), json!(cache));
            if let Some(start) = row.get("range_start").filter(|v| !v.is_null()) {
                ranges.insert(
                    video.to_owned(),
                    json!({"start": start, "end": row.get("range_end").cloned().unwrap_or(json!(0))}),
                );
            } else {
                let manifest = projects::project_root(state)
                    .join(id)
                    .join("analysis")
                    .join(cache)
                    .join("inference.manifest.json");
                if let Some(offset) = optional_record(&manifest)
                    .get("analysis_offset_sec")
                    .filter(|v| !v.is_null())
                {
                    ranges.insert(video.to_owned(), json!({"start": offset, "end": null}));
                }
            }
            if projects::project_root(state)
                .join(id)
                .join("analysis")
                .join(cache)
                .join("analysis/events.json")
                .is_file()
            {
                processed.push(video.to_owned());
            }
        }
        if mappings.len() != allowed.len() {
            continue;
        }
        let Some(stem) = file.file_stem().and_then(|name| name.to_str()) else {
            continue;
        };
        let id_value = format!("run_{stem}");
        if !valid_run_id(&id_value) {
            continue;
        }
        let run = json!({
            "id": id_value,
            "profile": profile,
            "profile_id": entry.get("profile_id").filter(|v| v.as_str().is_some_and(|s| !s.is_empty())).cloned().unwrap_or(json!("legacy")),
            "started_at": entry.get("started_at"),
            "videos": mappings,
            "video_ranges": ranges,
            "postprocessed_videos": processed,
            "status": "complete",
            "inference_only": entry.get("inference_only").and_then(Value::as_bool).unwrap_or(false)
                || entry["settings"]["inference_only"].as_bool().unwrap_or(false)
        });
        runs.push(run);
    }
    for run in runs.clone() {
        runs.extend(legacy_postprocessing_runs(state, id, &run));
    }
    let entries = optional_record(&root.join("runs/header_entries.json"));
    let deleted: HashSet<&str> = entries["deleted"]
        .as_array()
        .map(|deleted| deleted.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let names = entries["names"].as_object();
    runs.retain(|run| {
        run["id"]
            .as_str()
            .is_some_and(|name| !deleted.contains(name))
            && run
                .get("parent_run_id")
                .and_then(Value::as_str)
                .is_none_or(|parent| !deleted.contains(parent))
    });
    for run in &mut runs {
        let Some(id_value) = run["id"].as_str().map(str::to_owned) else {
            continue;
        };
        if run.get("parent_run_id").is_some() {
            if let Some(when) = run["started_at"]
                .as_str()
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            {
                run["auto_name"] = json!(when.format("%d.%m.%y %H:%M").to_string());
            }
        }
        if let Some(label) = names
            .and_then(|names| names.get(&id_value))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|label| !label.is_empty())
        {
            run["display_name"] = json!(label);
        }
    }
    runs.sort_by_key(|run| std::cmp::Reverse(started_timestamp(run)));
    Ok(runs)
}

pub async fn get_runs(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult<Value> {
    let id = query.get("project_id").cloned().unwrap_or_default();
    let rows = tokio::task::spawn_blocking(move || list_runs(&state, &id))
        .await
        .map_err(|error| storage::internal(format!("Runs task failed: {error}")))??;
    Ok(Json(json!({"runs": rows})))
}

#[cfg(test)]
mod tests {
    use super::{python_sorted_json, settings_hash, valid_cache_id, valid_run_id};
    use serde_json::json;

    #[test]
    fn saved_cpu_version_hash_uses_python_sorted_json() {
        assert_eq!(
            python_sorted_json(&json!({"b":2,"a":{"z":1,"c":[3,4]}})),
            r#"{"a": {"c": [3, 4], "z": 1}, "b": 2}"#
        );
        assert_eq!(settings_hash(&json!({"mode":"RAVEL"})).len(), 12);
    }
    #[test]
    fn ids_reject_traversal() {
        assert!(valid_cache_id("abcdef1234567890"));
        assert!(valid_cache_id("abcdef1234567890.abcdef1234567890"));
        assert!(!valid_cache_id("../something"));
        assert!(valid_run_id("run_abcdef123456"));
        assert!(!valid_run_id("run_../evil"));
    }
}
