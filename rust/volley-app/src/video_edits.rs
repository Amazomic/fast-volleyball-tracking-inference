//! Non-destructive source trim and normalized four-corner court marking.
use crate::{
    project_guard, projects,
    storage::{self, ApiResult},
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
use std::{collections::HashMap, fs, path::PathBuf, sync::Arc};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VideoEdit {
    project_id: String,
    video: String,
    trim_start_sec: f64,
    trim_end_sec: Option<f64>,
    court: Vec<[f64; 2]>,
}

fn edit_path(
    state: &AppState,
    project_id: &str,
    video: &str,
) -> Result<PathBuf, storage::ApiError> {
    storage::project_id(project_id)?;
    storage::video_name(video)?;
    let project = projects::existing_project(state, project_id)?;
    if !project["videos"]
        .as_array()
        .is_some_and(|rows| rows.iter().any(|v| v.as_str() == Some(video)))
    {
        return Err(storage::bad_request("Video does not belong to project"));
    }
    projects::canonical_source(state, video)?;
    let hash = format!("{:x}", Sha256::digest(video.as_bytes()));
    Ok(projects::project_root(state)
        .join(project_id)
        .join("video_edits")
        .join(format!("{hash}.json")))
}

fn with_revision(state: &AppState, record: Value) -> Result<Value, storage::ApiError> {
    let video = record["video"]
        .as_str()
        .ok_or_else(|| storage::bad_request("Missing edit video"))?;
    let source = projects::canonical_source(state, video)?;
    revision_for_source(record, &source)
}
fn revision_for_source(
    mut record: Value,
    source: &std::path::Path,
) -> Result<Value, storage::ApiError> {
    let meta = fs::metadata(source)
        .map_err(|e| storage::internal(format!("Edit source metadata: {e}")))?;
    let modified = meta
        .modified()
        .map_err(|e| storage::internal(format!("Edit source timestamp: {e}")))?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| storage::internal(format!("Edit source time: {e}")))?
        .as_nanos();
    let source_revision = format!(
        "{:x}",
        Sha256::digest(format!("{}:{}:{modified}", source.display(), meta.len()).as_bytes())
    );
    record["source_revision"] = json!(source_revision);
    record["edit_revision"] = json!(crate::versioned_store::revision(Some(
        &serde_json::to_vec(&record)
            .map_err(|e| storage::internal(format!("Edit revision: {e}")))?
    )));
    Ok(record)
}

pub async fn get(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult<Value> {
    let project_id = query
        .get("project_id")
        .map(String::as_str)
        .unwrap_or_default();
    let video = query.get("video").map(String::as_str).unwrap_or_default();
    let path = edit_path(&state, project_id, video)?;
    if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink() || !m.is_file()) {
        return Err(storage::bad_request("Unsafe video edit"));
    }
    if let Some(record) = storage::read_json(&path)? {
        if record["project_id"] != project_id
            || record["video"] != video
            || record["schema"] != "volley-video-edit/1"
        {
            return Err(storage::bad_request("Video edit belongs to another source"));
        }
        return Ok(Json(with_revision(&state, record)?));
    }
    Ok(Json(with_revision(
        &state,
        json!({"schema":"volley-video-edit/1","project_id":project_id,"video":video,
        "trim_start_sec":0.0,"trim_end_sec":null,"court":[]}),
    )?))
}

// Coordinates are normalized against the *image*, not the editing stage.
// The extra margin permits estimated court corners outside the camera view.
fn court_is_valid(court: &[[f64; 2]]) -> bool {
    (court.is_empty() || court.len() == 4)
        && court
            .iter()
            .flatten()
            .all(|v| v.is_finite() && (-0.5..=1.5).contains(v))
}

pub async fn set(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(edit): Json<VideoEdit>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    if !state.writes_enabled {
        return Err(storage::conflict("Studio data directory is read only"));
    }
    if !edit.trim_start_sec.is_finite()
        || edit.trim_start_sec < 0.0
        || edit
            .trim_end_sec
            .is_some_and(|end| !end.is_finite() || end <= edit.trim_start_sec)
        || !court_is_valid(&edit.court)
    {
        return Err(storage::bad_request(
            "Invalid trim interval or court corners",
        ));
    }
    let _guard = project_guard::write_guard(&state, &edit.project_id)?;
    let path = edit_path(&state, &edit.project_id, &edit.video)?;
    let dir = path
        .parent()
        .ok_or_else(|| storage::bad_request("Invalid video edit path"))?;
    if fs::symlink_metadata(dir).is_ok_and(|m| m.file_type().is_symlink() || !m.is_dir()) {
        return Err(storage::bad_request("Unsafe video edits directory"));
    }
    fs::create_dir_all(dir).map_err(|e| storage::internal(format!("Create video edits: {e}")))?;
    if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink() || !m.is_file()) {
        return Err(storage::bad_request("Unsafe video edit file"));
    }
    let record = json!({"schema":"volley-video-edit/1","project_id":edit.project_id,
        "video":edit.video,"trim_start_sec":edit.trim_start_sec,
        "trim_end_sec":edit.trim_end_sec,"court":edit.court});
    storage::write_json_atomic(&path, &record)?;
    Ok(Json(with_revision(&state, record)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn value_is_constrained() {
        let edit: VideoEdit = serde_json::from_value(json!({
            "project_id":"p","video":"a.mp4","trim_start_sec":0.0,
            "trim_end_sec":10.0,"court":[[0.1,0.1],[0.9,0.1],[0.9,0.9],[0.1,0.9]]
        }))
        .unwrap();
        assert_eq!(edit.court.len(), 4);
        assert!(court_is_valid(&edit.court));
    }

    #[test]
    fn off_frame_corners_are_bounded_but_supported() {
        assert!(court_is_valid(&[]));
        assert!(court_is_valid(&[
            [-0.21, 0.13],
            [1.18, -0.12],
            [1.09, 1.32],
            [0.12, 1.07],
        ]));
        assert!(!court_is_valid(&[[0.0, 0.0]]));
        assert!(!court_is_valid(&[
            [-0.51, 0.0],
            [0.8, 0.0],
            [0.8, 0.8],
            [0.1, 0.8],
        ]));
        assert!(!court_is_valid(&[
            [f64::NAN, 0.0],
            [0.8, 0.0],
            [0.8, 0.8],
            [0.1, 0.8],
        ]));
    }
    #[test]
    fn draft_revision_changes_with_source_or_persisted_edit() {
        let path =
            std::env::temp_dir().join(format!("video-edit-revision-{}", uuid::Uuid::new_v4()));
        fs::write(&path, b"one").unwrap();
        let base = json!({"project_id":"project","video":"a.mp4","trim_start_sec":0,"trim_end_sec":null,"court":[]});
        let first = revision_for_source(base.clone(), &path).unwrap();
        assert_eq!(first, revision_for_source(base.clone(), &path).unwrap());
        let mut changed = base.clone();
        changed["trim_start_sec"] = json!(1);
        assert_ne!(
            first["edit_revision"],
            revision_for_source(changed, &path).unwrap()["edit_revision"]
        );
        fs::write(&path, b"longer").unwrap();
        assert_ne!(
            first["source_revision"],
            revision_for_source(base, &path).unwrap()["source_revision"]
        );
        fs::remove_file(path).unwrap();
    }
}
