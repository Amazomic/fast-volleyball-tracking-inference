//! Rust-native project management and compatibility with existing project JSON.
use crate::{
    storage::{self, ApiError, ApiResult},
    AppState,
};
use axum::{extract::State, http::HeaderMap, Json};
use fs2::FileExt;
use serde_json::{json, Map, Value};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

pub(crate) fn project_root(state: &AppState) -> PathBuf {
    state.data_root.join("projects")
}

fn registry_path(state: &AppState) -> PathBuf {
    project_root(state).join("projects.json")
}

fn load_projects(state: &AppState) -> Result<Vec<Value>, ApiError> {
    let Some(loaded) = storage::read_json(&registry_path(state))? else {
        return Ok(Vec::new());
    };
    let rows = loaded
        .get("projects")
        .and_then(Value::as_array)
        .ok_or_else(|| storage::internal("Project registry lacks projects array"))?;
    Ok(rows.clone())
}

fn find_project(state: &AppState, id: &str) -> Result<Value, ApiError> {
    storage::project_id(id)?;
    if project_root(state)
        .join(id)
        .join(".delete-requested")
        .is_file()
    {
        return Err(storage::conflict(
            "Проект удаляется: запуск новых задач запрещён",
        ));
    }
    load_projects(state)?
        .into_iter()
        .find(|row| row["id"].as_str() == Some(id))
        .ok_or_else(|| storage::not_found("Проект не найден"))
}

fn canonical_source(state: &AppState, name: &str) -> Result<PathBuf, ApiError> {
    storage::video_name(name)?;
    let source = state.repository_root.join("project").join(name);
    source
        .canonicalize()
        .ok()
        .filter(|source| source.is_file())
        .ok_or_else(|| storage::bad_request(format!("Видео не найдено: {name}")))
}

pub(crate) fn event_source_matches(
    state: &AppState,
    events: &Path,
    name: &str,
    resolve: bool,
) -> bool {
    let Ok(Some(data)) = storage::read_json(events) else {
        return false;
    };
    let Some(recorded) = data.get("source").and_then(Value::as_str) else {
        return false;
    };
    let normalized = recorded.replace('\\', "/");
    if !resolve {
        return Path::new(&normalized).file_name().and_then(|s| s.to_str()) == Some(name);
    }
    let path = Path::new(&normalized);
    let source = if path.is_absolute() {
        path.to_path_buf()
    } else {
        state.repository_root.join(path)
    };
    let expected = canonical_source(state, name);
    matches!((source.canonicalize(), expected), (Ok(a), Ok(b)) if a == b)
}

fn latest_cache(state: &AppState, name: &str, project_id: Option<&str>) -> Option<String> {
    let scoped = project_id.is_some();
    let dir = if let Some(id) = project_id {
        project_root(state).join(id).join("analysis")
    } else {
        state.data_root.join("analysis")
    };
    let mut latest: Option<(SystemTime, String)> = None;
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let events = path.join("analysis").join("events.json");
        if !event_source_matches(state, &events, name, scoped) {
            continue;
        }
        let stamp = events
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let Some(id) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if latest.as_ref().is_none_or(|(seen, _)| stamp > *seen) {
            latest = Some((stamp, id.to_owned()));
        }
    }
    latest.map(|(_, name)| name)
}

fn project_cache_available(state: &AppState, name: &str, id: &str) -> bool {
    if latest_cache(state, name, Some(id)).is_some() {
        return true;
    }
    // Legacy imported cache is only valid when explicitly associated with
    // the current project and the events source resolves to the original video.
    let manifest = project_root(state).join(id).join("legacy_analysis.json");
    let Ok(Some(legacy)) = storage::read_json(&manifest) else {
        return false;
    };
    let Some(cache) = legacy.get(name).and_then(Value::as_str) else {
        return false;
    };
    if cache.is_empty() || !cache.bytes().all(|c| c.is_ascii_alphanumeric()) {
        return false;
    }
    let events = state
        .data_root
        .join("analysis")
        .join(cache)
        .join("analysis/events.json");
    event_source_matches(state, &events, name, true)
}

fn safe_video_available(state: &AppState, project: &Value, video: &str) -> bool {
    if storage::video_name(video).is_err() {
        return false;
    }
    let Some(source) = state
        .repository_root
        .join("project")
        .join(video)
        .canonicalize()
        .ok()
    else {
        return false;
    };
    if !source.is_file() {
        return false;
    }
    if let Some(expected) = project
        .get("source_paths")
        .and_then(|s| s.get(video))
        .and_then(Value::as_str)
    {
        // Old imported absolute source paths are authoritative; Rust never
        // guesses a different source just because its filename matches.
        let expected = Path::new(expected);
        if expected.canonicalize().ok().as_ref() != Some(&source) {
            return false;
        }
    }
    true
}

fn project_summary(state: &AppState, project: &Value) -> Result<Value, ApiError> {
    let id = project
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| storage::internal("Project record has no ID"))?;
    storage::project_id(id)?;
    let videos = project
        .get("videos")
        .and_then(Value::as_array)
        .ok_or_else(|| storage::internal("Project record has no videos"))?;

    let mut analyses = Vec::with_capacity(videos.len());
    let mut missing = Vec::new();
    for video in videos {
        let Some(name) = video.as_str() else {
            continue;
        };
        let available = safe_video_available(state, project, name);
        if !available {
            missing.push(name.to_owned());
        }
        analyses.push(json!({
            "video": name,
            "available": available,
            "analyzed": available && project_cache_available(state, name, id)
        }));
    }

    let montage = project_root(state).join(id).join("highlights.mp4");
    let highlight_ready = montage.metadata().is_ok_and(|m| m.is_file() && m.len() > 0);
    let mut summary = project
        .as_object()
        .cloned()
        .ok_or_else(|| storage::internal("Project record is not object"))?;
    summary.entry("ranges").or_insert_with(|| json!({}));
    summary.insert("analyses".into(), json!(analyses));
    summary.insert("missing_videos".into(), json!(missing));
    summary.insert("highlight_ready".into(), json!(highlight_ready));
    Ok(Value::Object(summary))
}

pub async fn get_projects(State(state): State<Arc<AppState>>) -> ApiResult<Value> {
    let projects = tokio::task::spawn_blocking(move || {
        load_projects(&state)?
            .iter()
            .map(|project| project_summary(&state, project))
            .collect::<Result<Vec<_>, _>>()
    })
    .await
    .map_err(|error| storage::internal(format!("Project task failed: {error}")))??;

    // The Python UI reads {"projects": [summary...]}, never a bare list.
    Ok(Json(json!({"projects": projects})))
}

pub(crate) fn existing_project(state: &AppState, id: &str) -> Result<Value, ApiError> {
    find_project(state, id)
}

pub(crate) fn read_profiles(state: &AppState) -> Result<Vec<Value>, ApiError> {
    let path = state.data_root.join("analysis_profiles.json");
    let Some(contents) = storage::read_json(&path)? else {
        return Ok(Vec::new());
    };
    let Some(profiles) = contents.get("profiles").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    Ok(profiles
        .iter()
        .filter(|row| {
            row.is_object()
                && row.get("id").is_some_and(Value::is_string)
                && row.get("values").is_some_and(Value::is_object)
        })
        .cloned()
        .collect())
}

fn normalized_name(name: &str) -> Result<String, ApiError> {
    let clean: String = name
        .nfkc()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if (1..=100).contains(&clean.chars().count()) {
        Ok(clean)
    } else {
        Err(storage::bad_request(
            "Название проекта должно содержать от 1 до 100 символов",
        ))
    }
}

fn player_ids(state: &AppState, value: Option<&Value>) -> Result<Vec<String>, ApiError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let ids = value
        .as_array()
        .ok_or_else(|| storage::bad_request("Выберите не более 200 игроков"))?;
    if ids.len() > 200 {
        return Err(storage::bad_request("Выберите не более 200 игроков"));
    }
    let catalog = state.data_root.join("players/registry.json");
    let players = storage::read_json(&catalog)?
        .and_then(|doc| doc.get("players").and_then(Value::as_array).cloned())
        .unwrap_or_default();
    let known: HashSet<&str> = players.iter().filter_map(|p| p["id"].as_str()).collect();

    let mut unique = Vec::new();
    let mut selected = HashSet::new();
    for id in ids {
        let id = id
            .as_str()
            .ok_or_else(|| storage::bad_request("Некорректный идентификатор игрока"))?;
        if !known.contains(id) {
            return Err(storage::bad_request(
                "Один из выбранных игроков не найден в общей базе",
            ));
        }
        if selected.insert(id) {
            unique.push(id.to_owned());
        }
    }
    Ok(unique)
}

fn list_video_names(state: &AppState, value: &Value) -> Result<Vec<String>, ApiError> {
    let videos = value
        .as_array()
        .ok_or_else(|| storage::bad_request("Выберите видео для проекта"))?;
    if videos.is_empty() || videos.len() > 30 {
        return Err(storage::bad_request("Выберите от 1 до 30 видео"));
    }
    let mut names = Vec::new();
    let mut seen = HashSet::new();
    for item in videos {
        let name = item
            .as_str()
            .ok_or_else(|| storage::bad_request("Некорректное имя видео"))?;
        canonical_source(state, name)?;
        if seen.insert(name.to_owned()) {
            names.push(name.to_owned());
        }
    }
    Ok(names)
}

fn video_duration_secs(path: &Path) -> Result<f64, ApiError> {
    let duration = volley_media::probe(path)
        .map_err(storage::bad_request)?
        .duration_sec;
    if duration.is_finite() && duration > 0.0 {
        Ok(duration)
    } else {
        Err(storage::bad_request("Некорректная длительность видео"))
    }
}

fn rounded_secs(time: f64) -> f64 {
    // Python round precision matches for normal timestamps (ties unimportant
    // except at exact .0005; use same recorded three-decimal contract).
    (time * 1000.0).round() / 1000.0
}

fn validate_ranges(
    state: &AppState,
    videos: &[String],
    ranges: Option<&Value>,
) -> Result<Value, ApiError> {
    let explicit = match ranges {
        None | Some(Value::Null) => None,
        Some(Value::Object(object)) => Some(object),
        _ => return Err(storage::bad_request("Диапазоны должны быть объектом")),
    };
    if let Some(ranges) = explicit {
        if ranges.keys().any(|name| !videos.contains(name)) {
            return Err(storage::bad_request(
                "Диапазоны должны относиться к выбранным видео",
            ));
        }
    }
    let mut result = Map::new();
    for name in videos {
        let path = canonical_source(state, name)?;
        let duration = video_duration_secs(&path)?;
        let (start, end) = match explicit.and_then(|v| v.get(name)) {
            None | Some(Value::Null) => (0.0, duration),
            Some(Value::Object(selected)) => {
                if selected.len() != 2
                    || !selected.contains_key("start")
                    || !selected.contains_key("end")
                {
                    return Err(storage::bad_request("Неправильный диапазон видео"));
                }
                let start = selected["start"]
                    .as_f64()
                    .ok_or_else(|| storage::bad_request("Некорректная граница начала"))?;
                let end = selected["end"]
                    .as_f64()
                    .ok_or_else(|| storage::bad_request("Некорректная граница конца"))?;
                if !start.is_finite()
                    || !end.is_finite()
                    || start < 0.0
                    || start >= end
                    || end > duration + 0.01
                    || end - start < 1.0
                {
                    return Err(storage::bad_request(
                        "Диапазон должен быть в пределах видео (не менее 1 с)",
                    ));
                }
                (start, end.min(duration))
            }
            _ => return Err(storage::bad_request("Неправильный диапазон видео")),
        };
        result.insert(
            name.clone(),
            json!({"start": rounded_secs(start), "end": rounded_secs(end)}),
        );
    }
    Ok(Value::Object(result))
}

fn ensure_writable(state: &AppState) -> Result<(), ApiError> {
    if !state.writes_enabled {
        Err(storage::conflict(
            "Rust разработка работает только на чтение. Для записи используйте отдельный VOLLEY_RUST_DATA_ROOT",
        ))
    } else {
        Ok(())
    }
}

/// Registry lock is scoped to the isolated Rust data directory. This never
/// takes the Python app's real .registry.lock unless the sandbox guard fails.
fn with_registry_lock<T>(
    state: &AppState,
    operation: impl FnOnce() -> Result<T, ApiError>,
) -> Result<T, ApiError> {
    ensure_writable(state)?;
    let root = project_root(state);
    fs::create_dir_all(&root)
        .map_err(|err| storage::internal(format!("Create dev project root: {err}")))?;
    let file: File = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join(".registry.lock"))
        .map_err(|err| storage::internal(format!("Open dev registry lock: {err}")))?;
    file.lock_exclusive()
        .map_err(|err| storage::internal(format!("Lock dev registry: {err}")))?;
    let outcome = operation();
    // File lock automatically releases after this call. Explicitly drop it
    // after every operation rather than keep a lock in shared application state.
    drop(file);
    outcome
}

fn create_record(state: &AppState, request: Value) -> Result<Value, ApiError> {
    let raw_name = request["name"]
        .as_str()
        .ok_or_else(|| storage::bad_request("Укажите название проекта"))?;
    let name = normalized_name(raw_name)?;
    let videos = list_video_names(state, &request["videos"])?;
    let ranges = validate_ranges(state, &videos, request.get("ranges"))?;
    let ids = player_ids(state, request.get("player_ids"))?;
    let mut sources = Map::new();
    for video in &videos {
        sources.insert(
            video.to_owned(),
            json!(canonical_source(state, video)?
                .to_string_lossy()
                .to_string()),
        );
    }
    with_registry_lock(state, || {
        let mut projects = load_projects(state)?;
        if projects.iter().any(|p| {
            p["name"]
                .as_str()
                .is_some_and(|other| other.to_lowercase() == name.to_lowercase())
        }) {
            return Err(storage::bad_request(
                "Проект с таким названием уже существует",
            ));
        }
        let id = (0..20)
            .map(|_| Uuid::new_v4().simple().to_string()[..12].to_owned())
            .find(|id| !project_root(state).join(id).exists())
            .ok_or_else(|| storage::internal("Unable to allocate unique project ID"))?;
        let project = json!({
            "id": id,
            "name": name,
            "videos": videos,
            "ranges": ranges,
            "source_paths": sources,
            "player_ids": ids,
        });
        let directory = project_root(state).join(&id);
        fs::create_dir(&directory)
            .map_err(|err| storage::internal(format!("Create dev project: {err}")))?;
        if let Err(error) = storage::write_json_atomic(&directory.join("project.json"), &project) {
            let _ = fs::remove_dir(&directory);
            return Err(error);
        }
        projects.push(project.clone());
        if let Err(error) = storage::write_json_atomic(
            &registry_path(state),
            &json!({"schema":1,"projects":projects}),
        ) {
            let _ = fs::remove_file(directory.join("project.json"));
            let _ = fs::remove_dir(&directory);
            return Err(error);
        }
        Ok(project)
    })
}

fn update_record(state: &AppState, request: Value) -> Result<Value, ApiError> {
    let id = request["id"]
        .as_str()
        .ok_or_else(|| storage::bad_request("Укажите идентификатор проекта"))?
        .to_owned();
    storage::project_id(&id)?;
    with_registry_lock(state, || {
        let mut projects = load_projects(state)?;
        let index = projects
            .iter()
            .position(|row| row["id"].as_str() == Some(&id))
            .ok_or_else(|| storage::not_found("Проект не найден"))?;
        if project_root(state)
            .join(&id)
            .join(".delete-requested")
            .exists()
        {
            return Err(storage::conflict(
                "Проект архивируется; изменение запрещено",
            ));
        }
        let before = projects[index].clone();
        let mut item = before
            .as_object()
            .cloned()
            .ok_or_else(|| storage::internal("Invalid project record"))?;
        if let Some(name) = request.get("name").filter(|v| !v.is_null()) {
            let name = normalized_name(
                name.as_str()
                    .ok_or_else(|| storage::bad_request("Некорректное имя проекта"))?,
            )?;
            if projects.iter().enumerate().any(|(idx, p)| {
                idx != index
                    && p["name"]
                        .as_str()
                        .is_some_and(|other| other.to_lowercase() == name.to_lowercase())
            }) {
                return Err(storage::bad_request("Название проекта уже занято"));
            }
            item.insert("name".into(), json!(name));
        }
        if let Some(ids) = request.get("player_ids").filter(|v| !v.is_null()) {
            item.insert("player_ids".into(), json!(player_ids(state, Some(ids))?));
        }
        let changing_videos = request.get("videos").is_some_and(|v| !v.is_null());
        let changing_ranges = request.get("ranges").is_some_and(|v| !v.is_null());
        if changing_videos || changing_ranges {
            let source_videos = if changing_videos {
                list_video_names(state, &request["videos"])?
            } else {
                list_video_names(state, &item["videos"])?
            };
            let target_ranges = if changing_ranges {
                request.get("ranges")
            } else {
                Some(&item["ranges"])
            };
            let existing_ranges = if changing_videos && !changing_ranges {
                // Python drops range values for removed sources.
                let previous = target_ranges.and_then(Value::as_object);
                let mut reduced = Map::new();
                for name in &source_videos {
                    if let Some(old) = previous.and_then(|p| p.get(name)) {
                        reduced.insert(name.clone(), old.clone());
                    }
                }
                Some(Value::Object(reduced))
            } else {
                None
            };
            let range_ref = existing_ranges.as_ref().or(target_ranges);
            item.insert(
                "ranges".into(),
                validate_ranges(state, &source_videos, range_ref)?,
            );
            let mut sources = Map::new();
            for name in &source_videos {
                sources.insert(
                    name.to_owned(),
                    json!(canonical_source(state, name)?.to_string_lossy().to_string()),
                );
            }
            item.insert("videos".into(), json!(source_videos));
            item.insert("source_paths".into(), Value::Object(sources));
        }
        let updated = Value::Object(item);
        projects[index] = updated.clone();
        let detail = project_root(state).join(&id).join("project.json");
        // Both files are atomically swapped individually; if the registry
        // update fails, restore the old per-project document.
        storage::write_json_atomic(&detail, &updated)?;
        if let Err(error) = storage::write_json_atomic(
            &registry_path(state),
            &json!({"schema": 1, "projects": projects}),
        ) {
            let _ = storage::write_json_atomic(&detail, &before);
            return Err(error);
        }
        Ok(updated)
    })
}

pub async fn create_project(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    let writing = state.clone();
    let project = tokio::task::spawn_blocking(move || create_record(&writing, request))
        .await
        .map_err(|err| storage::internal(format!("Create task failed: {err}")))??;
    // The project record is already durably committed: a preparation queue
    // failure must not trigger an HTTP retry that creates another project.
    if let Some(id) = project["id"].as_str() {
        if let Err(error) = crate::preparation::enqueue_project(state, id, false).await {
            eprintln!("Rust project created, but preparation queue failed: {error:?}");
        }
    }
    Ok(Json(json!({"project": project})))
}

pub async fn update_project(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    let writing = state.clone();
    let project = tokio::task::spawn_blocking(move || update_record(&writing, request))
        .await
        .map_err(|err| storage::internal(format!("Update task failed: {err}")))??;
    if let Some(id) = project["id"].as_str() {
        if let Err(error) = crate::preparation::enqueue_project(state, id, false).await {
            eprintln!("Rust project updated, but preparation queue failed: {error:?}");
        }
    }
    Ok(Json(json!({"project": project})))
}

pub(crate) fn scan_videos(state: &AppState) -> Result<Vec<Value>, ApiError> {
    let video_root = state.repository_root.join("project");
    let Ok(folder) = fs::read_dir(video_root) else {
        return Ok(Vec::new());
    };
    let mut paths = folder
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|name| storage::video_name(name).is_ok())
        })
        .collect::<Vec<_>>();
    paths.sort();
    let mut out = Vec::new();
    for path in paths {
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let size = path.metadata().map(|m| m.len()).unwrap_or(0);
        let cache = latest_cache(state, name, None);
        out.push(json!({
            "name": name,
            "size_gb": ((size as f64 / 1e9)*100.0).round()/100.0,
            "analyzed": cache.is_some(),
            "cache_id": cache
        }));
    }
    Ok(out)
}

pub(crate) fn mark_project_for_archive(state: &AppState, id: &str) -> Result<(), ApiError> {
    storage::project_id(id)?;
    with_registry_lock(state, || {
        if !load_projects(state)?
            .iter()
            .any(|row| row["id"].as_str() == Some(id))
        {
            return Err(storage::not_found("Проект не найден"));
        }
        let directory = project_root(state).join(id);
        let metadata = fs::symlink_metadata(&directory)
            .map_err(|_| storage::not_found("Каталог проекта не найден"))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(storage::bad_request(
                "Каталог проекта не должен быть ссылкой",
            ));
        }
        let marker = directory.join(".delete-requested");
        if marker.exists() {
            return Err(storage::conflict("Проект уже архивируется"));
        }
        fs::write(&marker, b"rust_archive_pending")
            .map_err(|err| storage::internal(format!("Set project archive marker: {err}")))?;
        Ok(())
    })
}

pub(crate) fn clear_project_archive_marker(state: &AppState, id: &str) {
    if storage::project_id(id).is_ok() && state.writes_enabled {
        let _ = fs::remove_file(project_root(state).join(id).join(".delete-requested"));
    }
}

/// Move only project-owned data to the isolated recoverable archive.
/// Source videos, common prepared cache and global player photo albums are untouched.
pub(crate) fn archive_marked_project(state: &AppState, id: &str) -> Result<Value, ApiError> {
    storage::project_id(id)?;
    with_registry_lock(state, || {
        let mut registry = load_projects(state)?;
        let index = registry
            .iter()
            .position(|row| row["id"].as_str() == Some(id))
            .ok_or_else(|| storage::not_found("Проект не найден"))?;
        let source = project_root(state).join(id);
        let meta = fs::symlink_metadata(&source)
            .map_err(|_| storage::not_found("Каталог проекта не найден"))?;
        if meta.file_type().is_symlink()
            || !meta.is_dir()
            || !source.join(".delete-requested").is_file()
        {
            return Err(storage::conflict("Архивирование не было подготовлено"));
        }
        let trash = project_root(state).join(".trash");
        if let Ok(meta) = fs::symlink_metadata(&trash) {
            if meta.file_type().is_symlink() || !meta.is_dir() {
                return Err(storage::bad_request("Каталог архива недоступен"));
            }
        } else {
            fs::create_dir(&trash)
                .map_err(|err| storage::internal(format!("Create archive root: {err}")))?;
        }
        let archive_id = format!("{}-{}", id, Uuid::new_v4().simple());
        let destination = trash.join(&archive_id);
        fs::rename(&source, &destination)
            .map_err(|err| storage::internal(format!("Archive project directory: {err}")))?;
        registry.remove(index);
        if let Err(error) = storage::write_json_atomic(
            &registry_path(state),
            &json!({"schema":1,"projects":registry}),
        ) {
            // Archive rename is reversible; don't silently leave an absent
            // registry entry pointing to an unlisted project.
            if let Err(restore) = fs::rename(&destination, &source) {
                eprintln!("Rust project archive rollback failed: {restore}");
            }
            return Err(error);
        }
        let _ = fs::remove_file(destination.join(".delete-requested"));
        Ok(json!({"project_id":id,"archived":true,"deleted":false,
            "archive_id":archive_id,"shared_prepared_retained":true}))
    })
}

#[cfg(test)]
mod tests {
    use super::normalized_name;
    #[test]
    fn normalized_project_name_collapses_whitespace_and_nfkc() {
        assert_eq!(normalized_name("  A  B  ").unwrap(), "A B");
        assert_eq!(normalized_name("Ｆｏｏ").unwrap(), "Foo");
        assert!(normalized_name(" \t ").is_err());
    }
}
