//! Read-only legacy Python project discovery + explicit metadata-only import.
//! The old results/ tree is never modified, moved, deleted or reinterpreted
//! as native GPU/CPU authority. Missing media is visibly reported as missing.
use crate::{
    projects,
    storage::{self, ApiError, ApiResult},
    AppState,
};
use axum::{
    extract::{Query, State},
    http::HeaderMap,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
#[cfg(test)]
use uuid::Uuid;

const REGISTRY_MAX: u64 = 4 * 1024 * 1024;
const PROJECT_MAX: u64 = 512 * 1024;
const MAX_LEGACY_PROJECTS: usize = 1500;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyRequest {
    project_id: String,
}

fn safe_regular(path: &Path, max: u64) -> Result<Vec<u8>, ApiError> {
    let info =
        fs::symlink_metadata(path).map_err(|_| storage::not_found("Legacy source file missing"))?;
    if info.file_type().is_symlink() || !info.is_file() || info.len() > max {
        return Err(storage::bad_request(
            "Unsafe or oversized legacy source file",
        ));
    }
    fs::read(path).map_err(|e| storage::internal(format!("Read old project metadata: {e}")))
}
fn old_root(state: &AppState) -> Result<Option<PathBuf>, ApiError> {
    let results = state.repository_root.join("results");
    if !results.exists() {
        return Ok(None);
    }
    let root = results.join("projects");
    for folder in [&results, &root] {
        let info = fs::symlink_metadata(folder)
            .map_err(|_| storage::not_found("Old projects storage missing"))?;
        if info.file_type().is_symlink() || !info.is_dir() {
            return Err(storage::bad_request(
                "Legacy results path must not contain symlinks",
            ));
        }
    }
    let canonical_repo = state
        .repository_root
        .canonicalize()
        .map_err(|_| storage::bad_request("Repository path missing"))?;
    let canonical_root = root
        .canonicalize()
        .map_err(|_| storage::bad_request("Legacy projects root inaccessible"))?;
    if !canonical_root.starts_with(canonical_repo.join("results")) {
        return Err(storage::bad_request(
            "Legacy projects source outside repository",
        ));
    }
    Ok(Some(root))
}
fn validate_entry(row: &Value) -> Result<&str, ApiError> {
    let id = row["id"]
        .as_str()
        .ok_or_else(|| storage::bad_request("Legacy project has no ID"))?;
    storage::project_id(id)?;
    let name = row["name"]
        .as_str()
        .ok_or_else(|| storage::bad_request("Legacy project has no name"))?;
    if name.trim().is_empty() || name.len() > 320 {
        return Err(storage::bad_request("Legacy project name is invalid"));
    }
    let videos = row["videos"]
        .as_array()
        .ok_or_else(|| storage::bad_request("Legacy project has no videos array"))?;
    if videos.len() > 30 {
        return Err(storage::bad_request(
            "Legacy project contains too many videos",
        ));
    }
    let mut seen = HashSet::new();
    for item in videos {
        let name = item
            .as_str()
            .ok_or_else(|| storage::bad_request("Invalid old video name"))?;
        storage::video_name(name)?;
        if !seen.insert(name) {
            return Err(storage::bad_request("Duplicate old video name"));
        }
    }
    if !row["ranges"].is_null() && !row["ranges"].is_object() {
        return Err(storage::bad_request("Malformed legacy video ranges"));
    }
    if !row["source_paths"].is_null() && !row["source_paths"].is_object() {
        return Err(storage::bad_request(
            "Malformed original legacy source paths",
        ));
    }
    Ok(id)
}
fn legacy_registry(state: &AppState) -> Result<Vec<Value>, ApiError> {
    let Some(root) = old_root(state)? else {
        return Ok(Vec::new());
    };
    let path = root.join("projects.json");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let bytes = safe_regular(&path, REGISTRY_MAX)?;
    let decoded: Value = serde_json::from_slice(&bytes)
        .map_err(|_| storage::bad_request("Legacy project registry JSON invalid"))?;
    let rows = decoded["projects"]
        .as_array()
        .ok_or_else(|| storage::bad_request("Legacy project registry missing projects"))?;
    if rows.len() > MAX_LEGACY_PROJECTS {
        return Err(storage::bad_request(
            "Legacy registry exceeds project limit",
        ));
    }
    let mut seen = HashSet::new();
    for row in rows {
        let id = validate_entry(row)?;
        if !seen.insert(id) {
            return Err(storage::bad_request("Duplicate legacy project IDs"));
        }
    }
    Ok(rows.clone())
}
fn old_detail(state: &AppState, id: &str) -> Result<Value, ApiError> {
    storage::project_id(id)?;
    let root = old_root(state)?.ok_or_else(|| storage::not_found("Legacy results unavailable"))?;
    let project = root.join(id);
    let stat = fs::symlink_metadata(&project)
        .map_err(|_| storage::not_found("Legacy project directory missing"))?;
    if stat.file_type().is_symlink() || !stat.is_dir() {
        return Err(storage::bad_request("Legacy project directory is unsafe"));
    }
    let bytes = safe_regular(&project.join("project.json"), PROJECT_MAX)?;
    let record: Value = serde_json::from_slice(&bytes)
        .map_err(|_| storage::bad_request("Legacy project detail invalid"))?;
    validate_entry(&record)?;
    if record["id"].as_str() != Some(id) {
        return Err(storage::conflict("Legacy project ID mismatches directory"));
    }
    Ok(record)
}
fn verified_old_project(state: &AppState, id: &str) -> Result<Value, ApiError> {
    let registered = legacy_registry(state)?
        .into_iter()
        .find(|r| r["id"].as_str() == Some(id))
        .ok_or_else(|| storage::not_found("Project not found in historical registry"))?;
    let detail = old_detail(state, id)?;
    if registered["name"] != detail["name"] || registered["videos"] != detail["videos"] {
        return Err(storage::conflict(
            "Legacy project registry and detail disagree",
        ));
    }
    // Preserve legacy source_paths and all saved ranges verbatim. Never rebind
    // an old analysis to an unrelated source simply because names match.
    Ok(detail)
}
fn historical_runs(state: &AppState, id: &str) -> Vec<String> {
    let Some(root) = old_root(state).ok().flatten() else {
        return Vec::new();
    };
    let runroot = root.join(id).join("runs");
    let Ok(info) = fs::symlink_metadata(&runroot) else {
        return Vec::new();
    };
    if info.file_type().is_symlink() || !info.is_dir() {
        return Vec::new();
    }
    let Ok(entries) = fs::read_dir(runroot) else {
        return Vec::new();
    };
    let mut runs = Vec::new();
    for entry in entries.flatten().take(1000) {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.len() > 100 || name.starts_with('.') || name.contains(['/', '\\']) {
            continue;
        }
        let Ok(stat) = entry.metadata() else { continue };
        if stat.is_dir()
            && !entry.path().is_symlink()
            && (name.starts_with("run_") || name.as_bytes().first().is_some_and(u8::is_ascii_digit))
        {
            // Only inventory metadata; do not silently treat legacy inference
            // cache IDs as native GPU/CPU replay IDs.
            runs.push(name);
        }
    }
    runs.sort();
    runs.truncate(75);
    runs
}
fn describe(state: &AppState, record: &Value, imported: bool) -> Value {
    let videos = record["videos"].as_array().cloned().unwrap_or_default();
    let unavailable = videos
        .iter()
        .filter_map(|item| {
            let name = item.as_str()?;
            (!projects::safe_video_available(state, record, name)).then(|| name.to_owned())
        })
        .collect::<Vec<_>>();
    let id = record["id"].as_str().unwrap_or_default();
    let runs = historical_runs(state, id);
    json!({
        "id":id,"name":record["name"],"videos":videos,
        "missing_videos":unavailable,"media_ready":unavailable.is_empty()&&!videos.is_empty(),
        "historical_runs":runs,
        "historical_runs_are_native":false,
        "imported":imported,"source":"legacy_results_read_only",
        "analysis_replay_requires_verified_original":true
    })
}
pub async fn catalog(State(state): State<Arc<AppState>>) -> ApiResult<Value> {
    let response = tokio::task::spawn_blocking(move || {
        let existing = projects::load_projects(&state)?
            .into_iter()
            .filter_map(|p| p["id"].as_str().map(str::to_owned))
            .collect::<HashSet<_>>();
        let mut entries = Vec::new();
        for row in legacy_registry(&state)? {
            let id = row["id"].as_str().unwrap_or_default();
            let verified = verified_old_project(&state, id)?;
            entries.push(describe(&state, &verified, existing.contains(id)));
        }
        Ok::<_, ApiError>(json!({"projects":entries,"metadata_only":true,
            "original_results_immutable":true,"project_created":false}))
    })
    .await
    .map_err(|e| storage::internal(format!("Legacy catalog task: {e}")))??;
    Ok(Json(response))
}
pub async fn inspect(
    State(state): State<Arc<AppState>>,
    Query(req): Query<LegacyRequest>,
) -> ApiResult<Value> {
    storage::project_id(&req.project_id)?;
    let id = req.project_id;
    let value = tokio::task::spawn_blocking(move || {
        let detail = verified_old_project(&state, &id)?;
        let imported = projects::load_projects(&state)?
            .iter()
            .any(|r| r["id"].as_str() == Some(id.as_str()));
        Ok::<_, ApiError>(describe(&state, &detail, imported))
    })
    .await
    .map_err(|e| storage::internal(format!("Legacy inspection task: {e}")))??;
    Ok(Json(value))
}
pub async fn import(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<LegacyRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    storage::project_id(&req.project_id)?;
    if !state.writes_enabled {
        return Err(storage::conflict(
            "Legacy metadata import requires isolated Rust data root",
        ));
    }
    let imported = tokio::task::spawn_blocking(move || import_one(&state, &req.project_id))
        .await
        .map_err(|e| storage::internal(format!("Import old project task: {e}")))??;
    Ok(Json(imported))
}
fn import_one(state: &AppState, id: &str) -> Result<Value, ApiError> {
    let original = verified_old_project(state, id)?;
    let serialized = serde_json::to_vec(&original)
        .map_err(|_| storage::bad_request("Legacy metadata encoding failed"))?;
    let hash = format!("{:x}", Sha256::digest(&serialized));
    projects::with_registry_lock(state, || {
        let mut rows = projects::load_projects(state)?;
        if rows.iter().any(|r| r["id"].as_str() == Some(id)) {
            return Err(storage::conflict(
                "Legacy project already present in isolated Rust registry",
            ));
        }
        let archived = projects::project_root(state).join(".trash");
        if fs::symlink_metadata(&archived).is_ok_and(|m| m.file_type().is_symlink() || !m.is_dir())
        {
            return Err(storage::bad_request("Unsafe Rust archive namespace"));
        }
        if let Ok(folder) = fs::read_dir(&archived) {
            for item in folder.flatten().take(5000) {
                let name = item.file_name().to_string_lossy().to_string();
                if name.starts_with(&format!("{id}-")) && name.len() == 45 {
                    return Err(storage::conflict(
                        "Restore the archived Rust copy instead of importing again",
                    ));
                }
            }
        }
        let mut project = original
            .as_object()
            .cloned()
            .ok_or_else(|| storage::bad_request("Legacy project must be a JSON object"))?;
        project.insert(
            "legacy_import".into(),
            json!({
                "schema":"volley-rust-legacy-metadata/1","source":"results/projects",
                "original_project_id":id,"original_immutable":true,
                "native_inference_imported":false,"source_sha256":hash
            }),
        );
        let project = Value::Object(project);
        let directory = projects::project_root(state).join(id);
        fs::create_dir(&directory).map_err(|e| {
            storage::conflict(format!("Rust project directory already exists: {e}"))
        })?;
        let outcome = (|| {
            // No contents under repository_root/results/ are ever written:
            // only a small isolated project.json and registry entry.
            storage::write_json_atomic(&directory.join("project.json"), &project)?;
            rows.push(project.clone());
            storage::write_json_atomic(
                &projects::registry_path(state),
                &json!({"schema":1,"projects":rows}),
            )?;
            Ok::<_, ApiError>(project)
        })();
        if outcome.is_err() {
            let _ = fs::remove_file(directory.join("project.json"));
            let _ = fs::remove_dir(&directory);
        }
        outcome.map(|p| {
            json!({
                "imported":true,"project_id":id,"project":describe(state,&p,true),
                "copied_media":false,"copied_inference":false,
                "original_results_immutable":true
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sandbox() -> (PathBuf, AppState) {
        let root = std::env::temp_dir().join(format!("legacy-project-80-{}", Uuid::new_v4()));
        let repository_root = root.join("repo");
        let data_root = root.join("rust");
        fs::create_dir_all(&repository_root).unwrap();
        fs::create_dir_all(&data_root).unwrap();
        (
            root,
            AppState {
                repository_root,
                data_root,
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
                full_inference: crate::inference_full::FullInferenceManager::new(),
            },
        )
    }
    fn old_project(state: &AppState) -> (PathBuf, Vec<u8>) {
        let id = "391314fa0653";
        let old = state.repository_root.join("results/projects");
        fs::create_dir_all(old.join(id).join("runs/20261106_160721_394244_b9417c")).unwrap();
        fs::write(
            old.join(id)
                .join("runs/20261106_160721_394244_b9417c")
                .join("old-events.json"),
            b"historic analysis never modified",
        )
        .unwrap();
        let media = state.repository_root.join("project");
        fs::create_dir_all(&media).unwrap();
        // A real old project may have only a zero-length source placeholder:
        // the UI must flag that absence, never claim playable video.
        fs::write(media.join("match.mp4"), b"").unwrap();
        let project = json!({
            "id":id,"name":"06.11.2026 legacy","videos":["match.mp4"],
            "ranges":{"match.mp4":{"start":12.0,"end":300.0}},
            "source_paths":{"match.mp4":"/mnt/c/nonexistent/original.mp4"},
            "player_ids":["old-player"],"custom_python_key":{"keep":true}
        });
        let original_bytes = serde_json::to_vec_pretty(&json!({
            "schema":1,"projects":[project]
        }))
        .unwrap();
        fs::write(old.join("projects.json"), &original_bytes).unwrap();
        fs::write(
            old.join(id).join("project.json"),
            serde_json::to_vec(&project).unwrap(),
        )
        .unwrap();
        (old, original_bytes)
    }
    #[tokio::test]
    async fn import_true_old_project_preserves_history_and_does_not_mark_missing_video_available() {
        let (root, state) = sandbox();
        let (old, original_bytes) = old_project(&state);
        let entries = legacy_registry(&state).unwrap();
        assert_eq!(entries.len(), 1);
        let id = entries[0]["id"].as_str().unwrap();
        let report = describe(&state, &verified_old_project(&state, id).unwrap(), false);
        assert_eq!(report["media_ready"], false);
        assert_eq!(report["missing_videos"][0], "match.mp4");
        assert_eq!(report["historical_runs"].as_array().unwrap().len(), 1);
        assert_eq!(report["historical_runs_are_native"], false);
        let imported = import_one(&state, id).unwrap();
        assert_eq!(imported["imported"], true);
        assert_eq!(imported["copied_media"], false);
        assert_eq!(imported["copied_inference"], false);
        assert_eq!(
            import_one(&state, id).unwrap_err().0,
            axum::http::StatusCode::CONFLICT
        );
        let stored = projects::existing_project(&state, id).unwrap();
        assert_eq!(stored["id"], id);
        assert_eq!(stored["ranges"], entries[0]["ranges"]);
        assert_eq!(stored["source_paths"], entries[0]["source_paths"]);
        assert_eq!(stored["custom_python_key"]["keep"], true);
        assert_eq!(stored["legacy_import"]["original_immutable"], true);
        assert_eq!(stored["legacy_import"]["native_inference_imported"], false);
        let Json(listed) = projects::get_projects(State(Arc::new(state)))
            .await
            .unwrap();
        assert_eq!(listed["projects"][0]["missing_videos"][0], "match.mp4");
        assert_eq!(listed["projects"][0]["analyses"][0]["analyzed"], false);
        assert_eq!(fs::read(old.join("projects.json")).unwrap(), original_bytes);
        assert_eq!(
            fs::read(
                old.join(id)
                    .join("runs/20261106_160721_394244_b9417c/old-events.json")
            )
            .unwrap(),
            b"historic analysis never modified"
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn registry_and_details_mismatch_prevents_import_and_any_rust_registry_creation() {
        let (root, state) = sandbox();
        let (legacy, _) = old_project(&state);
        let id = "391314fa0653";
        let mut different = old_detail(&state, id).unwrap();
        different["videos"] = json!(["other.mp4"]);
        fs::write(
            legacy.join(id).join("project.json"),
            serde_json::to_vec(&different).unwrap(),
        )
        .unwrap();
        assert_eq!(
            import_one(&state, id).unwrap_err().0,
            axum::http::StatusCode::CONFLICT
        );
        assert!(!projects::project_root(&state).exists());
        fs::remove_dir_all(root).unwrap();
    }
    /// Opt-in real history acceptance: safe to run against a real historical
    /// project because ONLY a fresh temporary Rust sandbox is writable.
    #[tokio::test]
    async fn genuine_pre_rust_project_metadata_reopens_without_touching_old_results() {
        let Ok(repository) = std::env::var("VOLLEY_REAL_LEGACY_ROOT") else {
            return;
        };
        let (scratch, mut state) = sandbox();
        state.repository_root = PathBuf::from(repository);
        let registered = legacy_registry(&state).unwrap();
        assert!(
            !registered.is_empty(),
            "Expected real existing Python project metadata"
        );
        let id = registered[0]["id"].as_str().unwrap().to_owned();
        let original_registry = state.repository_root.join("results/projects/projects.json");
        let original_detail = state
            .repository_root
            .join("results/projects")
            .join(&id)
            .join("project.json");
        let before_registry = fs::read(&original_registry).unwrap();
        let before_detail = fs::read(&original_detail).unwrap();
        let original = verified_old_project(&state, &id).unwrap();
        let preview = describe(&state, &original, false);
        assert_eq!(preview["historical_runs_are_native"], false);
        let first = import_one(&state, &id).unwrap();
        assert_eq!(first["imported"], true);
        assert_eq!(first["copied_media"], false);
        let imported = projects::existing_project(&state, &id).unwrap();
        assert_eq!(imported["name"], original["name"]);
        assert_eq!(imported["videos"], original["videos"]);
        let lock = crate::project_archive::archive_guard(&state, &id).unwrap();
        projects::mark_project_for_archive(&state, &id).unwrap();
        let archived = projects::archive_marked_project(&state, &id).unwrap();
        drop(lock);
        assert_eq!(archived["archived"], true);
        let archive_id = archived["archive_id"].as_str().unwrap().to_owned();
        let state = Arc::new(state);
        let Json(restored) = crate::project_archive::restore(
            State(state.clone()),
            HeaderMap::new(),
            Json(serde_json::from_value(json!({"archive_id":archive_id})).unwrap()),
        )
        .await
        .unwrap();
        assert_eq!(restored["restored"], true);
        assert_eq!(
            projects::existing_project(&state, &id).unwrap()["name"],
            original["name"]
        );
        assert_eq!(fs::read(&original_registry).unwrap(), before_registry);
        assert_eq!(fs::read(&original_detail).unwrap(), before_detail);
        assert_eq!(
            fs::metadata(state.repository_root.join("results/projects"))
                .unwrap()
                .is_dir(),
            true
        );
        fs::remove_dir_all(scratch).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn legacy_registry_symlink_cannot_redirect_outside_repository() {
        let (root, state) = sandbox();
        let (legacy, _) = old_project(&state);
        fs::remove_file(legacy.join("projects.json")).unwrap();
        let outside = root.join("external.json");
        fs::write(&outside, b"secret").unwrap();
        std::os::unix::fs::symlink(&outside, legacy.join("projects.json")).unwrap();
        assert_eq!(
            legacy_registry(&state).unwrap_err().0,
            axum::http::StatusCode::BAD_REQUEST
        );
        assert!(!projects::project_root(&state).exists());
        fs::remove_dir_all(root).unwrap();
    }
}
