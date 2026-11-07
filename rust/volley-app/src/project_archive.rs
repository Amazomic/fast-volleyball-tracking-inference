//! Recoverable isolated Rust project archive with cooperative cancellation.
//! This never removes original source recordings, global player photos or
//! the shared prepared-video cache.
use crate::{
    montage, preparation, projects,
    storage::{self, ApiError, ApiResult},
    AppState,
};
use axum::{extract::State, http::HeaderMap, Json};
use fs2::FileExt;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    fs::{self, File, OpenOptions},
    io,
    sync::Arc,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveRequest {
    project_id: String,
}

/// This per-project lock is advisory across Rust Studio instances. It is
/// intentionally separate from the short-lived registry mutex and held for
/// the entire archive/cancellation transaction.
fn archive_guard(state: &AppState, id: &str) -> Result<File, ApiError> {
    if !state.writes_enabled {
        return Err(storage::conflict(
            "Архивирование Rust-проекта требует отдельного VOLLEY_RUST_DATA_ROOT",
        ));
    }
    storage::project_id(id)?;
    let root = projects::project_root(state);
    fs::create_dir_all(&root)
        .map_err(|e| storage::internal(format!("Archive project root: {e}")))?;
    let root_metadata = fs::symlink_metadata(&root)
        .map_err(|e| storage::internal(format!("Check project root: {e}")))?;
    let canonical = root
        .canonicalize()
        .map_err(|e| storage::internal(format!("Resolve project root: {e}")))?;
    if root_metadata.file_type().is_symlink()
        || !root_metadata.is_dir()
        || !canonical.starts_with(&state.data_root)
    {
        return Err(storage::bad_request("Каталог проектов вне Rust-хранилища"));
    }
    let locks = root.join(".archive-locks");
    if let Ok(meta) = fs::symlink_metadata(&locks) {
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(storage::bad_request("Папка блокировок проекта небезопасна"));
        }
    } else {
        fs::create_dir(&locks)
            .map_err(|e| storage::internal(format!("Archive lock directory: {e}")))?;
    }
    let path = locks.join(format!("{id}.lock"));
    if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(storage::bad_request("Файл блокировки проекта небезопасен"));
    }
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|e| storage::internal(format!("Open archive lock: {e}")))?;
    match lock.try_lock_exclusive() {
        Ok(()) => Ok(lock),
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => Err(storage::conflict(
            "Проект уже архивируется в другом Rust-процессе",
        )),
        Err(e) => Err(storage::internal(format!("Lock project archive: {e}"))),
    }
}

pub async fn archive(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<ArchiveRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    storage::project_id(&request.project_id)?;
    let id = request.project_id;
    let preparing = state.clone();
    let archive_id = id.clone();
    let guard = tokio::task::spawn_blocking(move || archive_guard(&preparing, &archive_id))
        .await
        .map_err(|e| storage::internal(format!("Archive lock task: {e}")))??;
    let marking = state.clone();
    let marker_id = id.clone();
    tokio::task::spawn_blocking(move || projects::mark_project_for_archive(&marking, &marker_id))
        .await
        .map_err(|e| storage::internal(format!("Archive marker task: {e}")))??;
    let outcome = async {
        let detached = preparation::detach_archiving_project(&state, &id).await;
        let stopped_montage = montage::stop_for_project_archive(&state, &id).await?;
        let publishing = state.clone();
        let project_id = id.clone();
        let mut archive = tokio::task::spawn_blocking(move || {
            projects::archive_marked_project(&publishing, &project_id)
        })
        .await
        .map_err(|e| storage::internal(format!("Archive commit task: {e}")))??;
        archive["cancelled_preparation_subscriptions"] = json!(detached);
        archive["stopped_montage"] = json!(stopped_montage);
        Ok::<_, ApiError>(archive)
    }
    .await;
    if outcome.is_err() {
        let clearing = state.clone();
        let marker_id = id.clone();
        let _ = tokio::task::spawn_blocking(move || {
            projects::clear_project_archive_marker(&clearing, &marker_id)
        })
        .await;
        preparation::resume_after_failed_archive(&state, &id).await;
    }
    drop(guard);
    outcome.map(Json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn sandbox() -> (std::path::PathBuf, AppState) {
        let root = std::env::temp_dir().join(format!("volley-rust-archive-{}", Uuid::new_v4()));
        let data_root = root.join("rust-sandbox");
        fs::create_dir_all(&data_root).unwrap();
        let state = AppState {
            repository_root: root.join("original"),
            data_root,
            writes_enabled: true,
            montage: montage::MontageManager::new(),
            preparation: preparation::PreparationManager::new(),
            preview_limit: tokio::sync::Semaphore::new(1),
            touch_limit: tokio::sync::Semaphore::new(1),
            continuity_limit: tokio::sync::Semaphore::new(1),
            reid_limit: tokio::sync::Semaphore::new(1),
            identity_limit: tokio::sync::Semaphore::new(1),
            statistics_limit: tokio::sync::Semaphore::new(1),
            marks_limit: tokio::sync::Semaphore::new(1),
            quick_reel: crate::quick_reel::QuickReelManager::new(),
            inference_limit: tokio::sync::Semaphore::new(1),
        };
        (root, state)
    }

    #[test]
    fn archiving_removes_only_isolated_project_and_keeps_shared_data() {
        let (root, state) = sandbox();
        let id = "1234abcdef01";
        let projects_root = projects::project_root(&state);
        let project = projects_root.join(id);
        fs::create_dir_all(project.join("analysis")).unwrap();
        fs::write(
            project.join("analysis").join("saved-events.json"),
            b"preserve me",
        )
        .unwrap();
        storage::write_json_atomic(
            &projects_root.join("projects.json"),
            &json!({"schema":1,"projects":[{"id":id,"name":"fixture","videos":[]}]}),
        )
        .unwrap();
        let common_prepared = state.data_root.join("prepared");
        fs::create_dir_all(&common_prepared).unwrap();
        fs::write(common_prepared.join("shared.mp4"), b"source never touched").unwrap();
        let photos = state
            .data_root
            .join("players")
            .join("1234abcdef01")
            .join("photos");
        fs::create_dir_all(&photos).unwrap();
        fs::write(photos.join("photo.jpg"), b"photo never touched").unwrap();

        let lock = archive_guard(&state, id).unwrap();
        projects::mark_project_for_archive(&state, id).unwrap();
        assert_eq!(
            projects::existing_project(&state, id).unwrap_err().0,
            axum::http::StatusCode::CONFLICT
        );
        let result = projects::archive_marked_project(&state, id).unwrap();
        assert_eq!(result["archived"], true);
        assert_eq!(result["deleted"], false);
        assert!(!project.exists());
        let archived = projects_root
            .join(".trash")
            .join(result["archive_id"].as_str().unwrap());
        assert_eq!(
            fs::read(archived.join("analysis/saved-events.json")).unwrap(),
            b"preserve me"
        );
        assert!(!archived.join(".delete-requested").exists());
        let registry = storage::read_json(&projects_root.join("projects.json"))
            .unwrap()
            .unwrap();
        assert!(registry["projects"].as_array().unwrap().is_empty());
        assert_eq!(
            fs::read(common_prepared.join("shared.mp4")).unwrap(),
            b"source never touched"
        );
        assert_eq!(
            fs::read(photos.join("photo.jpg")).unwrap(),
            b"photo never touched"
        );
        drop(lock);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn read_only_sandbox_never_archives() {
        let (root, mut state) = sandbox();
        state.writes_enabled = false;
        let error = archive_guard(&state, "1234abcdef01").unwrap_err();
        assert_eq!(error.0, axum::http::StatusCode::CONFLICT);
        assert!(!projects::project_root(&state).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn archive_guard_rejects_symlinked_project_root() {
        let (root, state) = sandbox();
        let external = root.join("external");
        fs::create_dir_all(&external).unwrap();
        std::os::unix::fs::symlink(&external, projects::project_root(&state)).unwrap();
        let error = archive_guard(&state, "1234abcdef01").unwrap_err();
        assert_eq!(error.0, axum::http::StatusCode::BAD_REQUEST);
        assert!(external.read_dir().unwrap().next().is_none());
        fs::remove_dir_all(root).unwrap();
    }
}
