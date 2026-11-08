//! Immutable raw ONNX proposal snapshots in an isolated Rust sandbox.
//! These are NOT confirmed ball/player tracks, a CPU result, or a GPU run.
use crate::{
    inference_stream::{self, ScanRequest},
    preparation, project_archive, projects,
    storage::{self, ApiError, ApiResult},
    AppState,
};
use axum::{
    extract::{Query, State},
    http::HeaderMap,
    Json,
};
use serde_json::{json, Value};
use std::{collections::HashMap, fs, path::PathBuf, sync::Arc};
use uuid::Uuid;

const MAX_EVIDENCE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_LIST: usize = 100;

fn valid_evidence_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
fn evidence_folder(state: &AppState, project_id: &str, create: bool) -> Result<PathBuf, ApiError> {
    storage::project_id(project_id)?;
    let folder = projects::project_root(state)
        .join(project_id)
        .join("native_inference_evidence");
    if create && !folder.exists() {
        fs::create_dir(&folder)
            .map_err(|e| storage::internal(format!("Create inference evidence dir: {e}")))?;
    }
    if let Ok(meta) = fs::symlink_metadata(&folder) {
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(storage::bad_request("Каталог GPU evidence небезопасен"));
        }
        let data_root = state
            .data_root
            .canonicalize()
            .map_err(|e| storage::internal(format!("Resolve Rust storage root: {e}")))?;
        if !folder
            .canonicalize()
            .map_err(|e| storage::internal(format!("Resolve evidence dir: {e}")))?
            .starts_with(&data_root)
        {
            return Err(storage::bad_request("GPU evidence вне Rust-хранилища"));
        }
    }
    Ok(folder)
}
fn inspected_file(folder: &std::path::Path, id: &str) -> Result<PathBuf, ApiError> {
    if !valid_evidence_id(id) {
        return Err(storage::bad_request(
            "Неверный ID сохранённого GPU evidence",
        ));
    }
    let path = folder.join(format!("{id}.json"));
    let meta = fs::symlink_metadata(&path)
        .map_err(|_| storage::not_found("Сохранённые кандидаты модели не найдены"))?;
    if meta.file_type().is_symlink() || !meta.is_file() || meta.len() > MAX_EVIDENCE_BYTES {
        return Err(storage::bad_request("Недопустимый файл GPU evidence"));
    }
    Ok(path)
}
fn store_record(
    state: &AppState,
    project_id: &str,
    video: &str,
    scan: &Value,
) -> Result<Value, ApiError> {
    if !state.writes_enabled {
        return Err(storage::conflict(
            "GPU evidence можно сохранять только в отдельной Rust-песочнице",
        ));
    }
    // Lock against concurrent archive/rename of this project's directory,
    // then revalidate its membership and prepared source identity.
    let _archive_lock = project_archive::archive_guard(state, project_id)?;
    let project = projects::existing_project(state, project_id)?;
    if !project["videos"]
        .as_array()
        .is_some_and(|v| v.iter().any(|name| name.as_str() == Some(video)))
    {
        return Err(storage::bad_request("Видео не принадлежит проекту"));
    }
    if scan["project_id"].as_str() != Some(project_id)
        || scan["video"].as_str() != Some(video)
        || scan["schema"] != "volley-native-inference-stream/1"
        || scan["full_gpu_run_complete"] != false
        || scan["raw_model_candidates_only"] != true
        || !scan["proposals"].is_array()
    {
        return Err(storage::bad_request(
            "Недопустимый черновой результат инференса",
        ));
    }
    let plan = preparation::plan_for(state, project_id, video)?;
    let file = plan
        .ready_path()
        .ok_or_else(|| storage::conflict("Подготовленное видео исчезло"))?;
    let (size, modified) = inference_stream::source_stamp(file).map_err(storage::bad_request)?;
    if scan["prepared_source"]["size_bytes"].as_u64() != Some(size)
        || scan["prepared_source"]["mtime_ns"].as_u64() != u64::try_from(modified).ok()
    {
        return Err(storage::conflict(
            "Исходное видео изменилось после GPU-сканирования",
        ));
    }
    let folder = evidence_folder(state, project_id, true)?;
    let id = Uuid::new_v4().simple().to_string();
    let record = json!({
        "schema":"volley-native-inference-evidence/1",
        "id":id,
        "project_id":project_id,
        "video":video,
        "status":"raw_unconfirmed_candidates_only",
        "scan":scan,
    });
    let bytes = serde_json::to_vec(&record)
        .map_err(|e| storage::internal(format!("Serialize raw GPU evidence: {e}")))?;
    if bytes.len() as u64 > MAX_EVIDENCE_BYTES {
        return Err(storage::bad_request(
            "GPU evidence превышает ограничение 2 МиБ",
        ));
    }
    storage::write_json_atomic(&folder.join(format!("{id}.json")), &record)?;
    Ok(
        json!({"project_id":project_id,"video":video,"evidence_id":id,
        "saved":true,"status":"raw_unconfirmed_candidates_only",
        "windows":scan["windows"],"full_gpu_run_complete":false}),
    )
}

/// Performs the same bounded diagnostic as window-scan, but stores its
/// unconfirmed proposals with unique immutable IDs under isolated Rust data.
pub async fn save(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<ScanRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    if !state.writes_enabled {
        return Err(storage::conflict(
            "Для сохранения нужен отдельный VOLLEY_RUST_DATA_ROOT",
        ));
    }
    let project_id = request.project_id.clone();
    let video = request.video.clone();
    let Json(scan) = inference_stream::scan(State(state.clone()), headers, Json(request)).await?;
    let receipt =
        tokio::task::spawn_blocking(move || store_record(&state, &project_id, &video, &scan))
            .await
            .map_err(|e| storage::internal(format!("Save native evidence task failed: {e}")))??;
    Ok(Json(receipt))
}

pub async fn read(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult<Value> {
    let project_id = query
        .get("project_id")
        .ok_or_else(|| storage::bad_request("Требуется project_id"))?
        .to_owned();
    storage::project_id(&project_id)?;
    let evidence_id = query.get("evidence_id").cloned();
    let data = tokio::task::spawn_blocking(move || -> Result<Value, ApiError> {
        projects::existing_project(&state, &project_id)?;
        let folder = evidence_folder(&state, &project_id, false)?;
        if let Some(id) = evidence_id {
            let record = storage::read_json(&inspected_file(&folder, &id)?)?
                .ok_or_else(|| storage::not_found("GPU evidence не найдено"))?;
            if record["id"].as_str() != Some(&id)
                || record["project_id"].as_str() != Some(&project_id)
                || record["schema"] != "volley-native-inference-evidence/1"
            {
                return Err(storage::bad_request(
                    "GPU evidence не соответствует проекту",
                ));
            }
            return Ok(record);
        }
        let mut rows = Vec::new();
        if let Ok(entries) = fs::read_dir(folder) {
            for entry in entries.flatten() {
                let Some(id) = entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.strip_suffix(".json"))
                    .filter(|id| valid_evidence_id(id))
                    .map(str::to_owned)
                else {
                    continue;
                };
                let Ok(meta) = fs::symlink_metadata(entry.path()) else {
                    continue;
                };
                if meta.file_type().is_symlink() {
                    continue;
                }
                if !meta.is_file() || meta.len() > MAX_EVIDENCE_BYTES {
                    continue;
                }
                rows.push((meta.modified().ok(), id));
            }
        }
        rows.sort_by(|a, b| b.0.cmp(&a.0));
        rows.truncate(MAX_LIST);
        Ok(json!({"project_id":project_id,"evidence_ids":
            rows.into_iter().map(|(_,id)|id).collect::<Vec<_>>()}))
    })
    .await
    .map_err(|e| storage::internal(format!("Read GPU evidence task: {e}")))??;
    Ok(Json(data))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opaque_evidence_ids_reject_path_traversal() {
        assert!(valid_evidence_id("0123456789abcdef0123456789abcdef"));
        assert!(!valid_evidence_id("../../etc/passwd"));
        assert!(!valid_evidence_id("0123456789ABCDEF0123456789ABCDEF"));
        assert!(!valid_evidence_id("0123456789abcdef0123456789abcde/"));
    }
}

#[cfg(test)]
mod isolated_persistence_tests {
    use super::*;
    use std::process::Command;

    fn state(root: &std::path::Path) -> AppState {
        AppState {
            repository_root: root.join("original"),
            data_root: root.join("rust-sandbox"),
            writes_enabled: true,
            montage: crate::montage::MontageManager::new(),
            preparation: crate::preparation::PreparationManager::new(),
            preview_limit: tokio::sync::Semaphore::new(1),
            touch_limit: tokio::sync::Semaphore::new(1),
            continuity_limit: tokio::sync::Semaphore::new(1),
            reid_limit: tokio::sync::Semaphore::new(1),
            identity_limit: tokio::sync::Semaphore::new(1),
            statistics_limit: tokio::sync::Semaphore::new(1),
            marks_limit: tokio::sync::Semaphore::new(1),
            quick_reel: crate::quick_reel::QuickReelManager::new(),
            inference_limit: tokio::sync::Semaphore::new(1),
        }
    }

    #[test]
    fn snapshot_isolated_from_original_project_and_prepared_data() {
        if Command::new("ffmpeg").arg("-version").output().is_err() {
            return;
        }
        let root = std::env::temp_dir().join(format!("volley-evidence-{}", Uuid::new_v4()));
        let app = state(&root);
        let id = "1234abcdef01";
        let source = app.repository_root.join("project").join("test.mp4");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        let status = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=64x64:rate=30:duration=1",
                "-an",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-y",
            ])
            .arg(&source)
            .status()
            .unwrap();
        assert!(status.success());

        let projects_root = projects::project_root(&app);
        fs::create_dir_all(projects_root.join(id)).unwrap();
        storage::write_json_atomic(
            &projects_root.join("projects.json"),
            &json!({
                "schema":1,"projects":[{"id":id,"name":"Test","videos":["test.mp4"]}]
            }),
        )
        .unwrap();
        let plan = preparation::plan_for(&app, id, "test.mp4").unwrap();
        assert!(
            plan.ready_path().is_some(),
            "fixture should already be analysis ready"
        );
        let (size, mtime) = inference_stream::source_stamp(&source).unwrap();
        let result = json!({
            "schema":"volley-native-inference-stream/1",
            "project_id":id,"video":"test.mp4","full_gpu_run_complete":false,
            "raw_model_candidates_only":true,"windows":1,
            "prepared_source":{"size_bytes":size,"mtime_ns":mtime},
            "proposals":[{"first_frame":0,"candidate_frames":[]}]
        });
        let saved = store_record(&app, id, "test.mp4", &result).unwrap();
        assert_eq!(saved["saved"], true);
        let evidence_id = saved["evidence_id"].as_str().unwrap();
        assert!(valid_evidence_id(evidence_id));
        let file = inspected_file(
            &projects_root.join(id).join("native_inference_evidence"),
            evidence_id,
        )
        .unwrap();
        let record = storage::read_json(&file).unwrap().unwrap();
        assert_eq!(record["scan"]["proposals"], result["proposals"]);
        assert_eq!(record["status"], "raw_unconfirmed_candidates_only");
        assert!(source.is_file());
        assert!(!app.repository_root.join("results").exists());
        let mut stale = result.clone();
        stale["prepared_source"]["size_bytes"] = json!(size + 1);
        assert_eq!(
            store_record(&app, id, "test.mp4", &stale).unwrap_err().0,
            axum::http::StatusCode::CONFLICT
        );
        fs::remove_dir_all(root).unwrap();
    }
}
