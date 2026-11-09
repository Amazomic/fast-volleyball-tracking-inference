//! Restore ONLY a Rust-sandbox project soft archive. Legacy Python results,
//! source recordings and global caches are never renamed or removed here.
use super::*;
use std::path::PathBuf;

fn archive_root(state: &AppState) -> Result<Option<PathBuf>, ApiError> {
    let projects = projects::project_root(state);
    if !projects.exists() {
        return Ok(None);
    }
    let trash = projects.join(".trash");
    if !trash.exists() {
        return Ok(None);
    }
    for folder in [&projects, &trash] {
        let meta = fs::symlink_metadata(folder)
            .map_err(|_| storage::bad_request("Rust project archive path inaccessible"))?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(storage::bad_request("Rust project archive path is unsafe"));
        }
    }
    let canonical = trash
        .canonicalize()
        .map_err(|_| storage::bad_request("Archive cannot resolve"))?;
    let root = state
        .data_root
        .canonicalize()
        .map_err(|_| storage::bad_request("Rust sandbox cannot resolve"))?;
    if !canonical.starts_with(root) {
        return Err(storage::bad_request("Archive escaped isolated Rust root"));
    }
    Ok(Some(trash))
}
fn archived_id(value: &str) -> Result<&str, ApiError> {
    let Some((id, nonce)) = value.split_once('-') else {
        return Err(storage::bad_request("Invalid archived project ID"));
    };
    storage::project_id(id)?;
    if nonce.len() != 32
        || !nonce
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(storage::bad_request("Invalid archive nonce"));
    }
    Ok(id)
}
fn archive_record(state: &AppState, archive_id: &str) -> Result<(PathBuf, Value), ApiError> {
    let id = archived_id(archive_id)?;
    let parent =
        archive_root(state)?.ok_or_else(|| storage::not_found("No isolated Rust archives"))?;
    let dir = parent.join(archive_id);
    let meta =
        fs::symlink_metadata(&dir).map_err(|_| storage::not_found("Archived project missing"))?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(storage::bad_request("Archived Rust directory is unsafe"));
    }
    let manifest = dir.join("project.json");
    let meta = fs::symlink_metadata(&manifest)
        .map_err(|_| storage::not_found("Archived project manifest missing"))?;
    if meta.file_type().is_symlink() || !meta.is_file() || meta.len() > 512 * 1024 {
        return Err(storage::bad_request("Archived project metadata unsafe"));
    }
    let original = storage::read_json(&manifest)?
        .ok_or_else(|| storage::not_found("Archived project record missing"))?;
    if original["id"].as_str() != Some(id) || !original["videos"].is_array() {
        return Err(storage::conflict("Archive project ID or videos mismatch"));
    }
    if dir.join(".delete-requested").exists() {
        return Err(storage::conflict("Archive transaction not complete"));
    }
    Ok((dir, original))
}
pub async fn list(State(state): State<Arc<AppState>>) -> ApiResult<Value> {
    let rows = tokio::task::spawn_blocking(move || {
        let Some(path) = archive_root(&state)? else {
            return Ok::<_, ApiError>(Vec::<Value>::new());
        };
        let mut output = Vec::new();
        for entry in fs::read_dir(path)
            .map_err(|e| storage::internal(format!("Read Rust archives: {e}")))?
            .take(2500)
            .flatten()
        {
            let id = entry.file_name().to_string_lossy().to_string();
            if archived_id(&id).is_err() {
                continue;
            }
            let Ok((_, project)) = archive_record(&state, &id) else {
                continue;
            };
            output.push(json!({
                "archive_id":id,"project_id":project["id"],"name":project["name"],
                "video_count":project["videos"].as_array().map(Vec::len).unwrap_or(0),
                "legacy_imported":project["legacy_import"].is_object()
            }));
        }
        output.sort_by(|a, b| a["archive_id"].as_str().cmp(&b["archive_id"].as_str()));
        Ok(output)
    })
    .await
    .map_err(|e| storage::internal(format!("Archive catalogue task: {e}")))??;
    Ok(Json(
        json!({"archives":rows,"recoverable":true,"isolated_rust_only":true}),
    ))
}

pub(super) fn restore_one(state: &AppState, archive_id: &str) -> Result<Value, ApiError> {
    let id = archived_id(archive_id)?.to_owned();
    let _guard = archive_guard(state, &id)?;
    projects::with_registry_lock(state, || {
        let (source, record) = archive_record(state, archive_id)?;
        let mut registry = projects::load_projects(state)?;
        let target = projects::project_root(state).join(&id);
        if registry
            .iter()
            .any(|p| p["id"].as_str() == Some(id.as_str()))
            || fs::symlink_metadata(&target).is_ok()
        {
            return Err(storage::conflict(
                "Project already active: cannot overwrite",
            ));
        }
        fs::rename(&source, &target)
            .map_err(|e| storage::internal(format!("Restore Rust project: {e}")))?;
        registry.push(record.clone());
        if let Err(err) = storage::write_json_atomic(
            &projects::registry_path(state),
            &json!({"schema":1,"projects":registry}),
        ) {
            if let Err(rollback) = fs::rename(&target, &source) {
                eprintln!("CRITICAL: Rust archive restore rollback: {rollback}");
            }
            return Err(err);
        }
        Ok(json!({
            "restored":true,"project_id":id,"archive_id":archive_id,
            "legacy_results_unchanged":true,"shared_prepared_retained":true,
            "project":record
        }))
    })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreRequest {
    archive_id: String,
}

pub async fn restore(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<RestoreRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    archived_id(&request.archive_id)?;
    let worker = state.clone();
    let id = request.archive_id;
    let result = tokio::task::spawn_blocking(move || restore_one(&worker, &id))
        .await
        .map_err(|e| storage::internal(format!("Restore Rust archive: {e}")))??;
    Ok(Json(result))
}
