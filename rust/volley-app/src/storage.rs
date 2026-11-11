//! Shared Rust-native JSON storage and origin/path safety.

use axum::{
    http::{header, HeaderMap, StatusCode},
    Json,
};
use serde_json::{json, Value};
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;

pub(crate) const MAX_BROWSER_VIDEO_BYTES: u64 = 32 * 1024 * 1024 * 1024;

pub(crate) struct PartialUpload(Option<PathBuf>);
impl PartialUpload {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self(Some(path))
    }
    pub(crate) fn disarm(&mut self) {
        self.0 = None;
    }
}
impl Drop for PartialUpload {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = fs::remove_file(path);
        }
    }
}

pub type ApiResult<T> = Result<Json<T>, ApiError>;
pub type ApiError = (StatusCode, Json<Value>);

pub fn bad_request(message: impl AsRef<str>) -> ApiError {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error": message.as_ref()})),
    )
}

pub fn conflict(message: impl AsRef<str>) -> ApiError {
    (
        StatusCode::CONFLICT,
        Json(json!({"error": message.as_ref()})),
    )
}

pub fn internal(message: impl AsRef<str>) -> ApiError {
    eprintln!("Storage error: {}", message.as_ref());
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "Ошибка чтения или записи данных проекта"})),
    )
}

pub fn not_found(message: impl AsRef<str>) -> ApiError {
    (
        StatusCode::NOT_FOUND,
        Json(json!({"error": message.as_ref()})),
    )
}

/// Never open files using a query/body supplied path. Existing project IDs
/// are 12 hex characters; this also prevents traversal into other projects.
pub fn project_id(value: &str) -> Result<&str, ApiError> {
    if value.len() == 12 && value.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(value)
    } else {
        Err(bad_request("Некорректный идентификатор проекта"))
    }
}

pub fn video_name(value: &str) -> Result<&str, ApiError> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\\')
        || value.contains('\0')
        || Path::new(value).file_name().and_then(|s| s.to_str()) != Some(value)
    {
        return Err(bad_request("Недопустимое имя видео"));
    }
    let extension = Path::new(value)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !matches!(
        extension.as_str(),
        "mp4" | "mov" | "mkv" | "avi" | "webm" | "m4v" | "mts"
    ) {
        return Err(bad_request("Неподдерживаемое расширение видео"));
    }
    Ok(value)
}

const MAX_JSON_BYTES: u64 = 32 * 1024 * 1024;

/// Source-scoped sidecars can be user-owned in the isolated root. Reject
/// existing symlink components (not just the final basename) before reading
/// or writing a JSON file. This does not substitute for OS-level sandboxing.
fn reject_symlink_ancestors(path: &Path) -> Result<(), ApiError> {
    for component in path.ancestors() {
        match fs::symlink_metadata(component) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(bad_request("JSON storage path contains a symlink"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(internal(format!("Check storage ancestor: {error}"))),
        }
    }
    Ok(())
}

pub fn read_json(path: &Path) -> Result<Option<Value>, ApiError> {
    reject_symlink_ancestors(path)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(internal(format!(
                "Cannot stat JSON {}: {error}",
                path.display()
            )))
        }
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > MAX_JSON_BYTES {
        return Err(bad_request("Unsafe or oversized JSON storage file"));
    }
    let bytes = fs::read(path).map_err(|error| internal(format!("{}: {error}", path.display())))?;
    if bytes.len() as u64 > MAX_JSON_BYTES {
        return Err(bad_request("JSON storage read exceeded size limit"));
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| internal(format!("Corrupted JSON {}: {error}", path.display())))
}

pub fn write_json_atomic(path: &Path, value: &Value) -> Result<(), ApiError> {
    let parent = path
        .parent()
        .ok_or_else(|| internal("JSON output has no parent"))?;
    reject_symlink_ancestors(parent)?;
    fs::create_dir_all(parent)
        .map_err(|error| internal(format!("{}: {error}", parent.display())))?;
    reject_symlink_ancestors(path)?;
    if let Ok(meta) = fs::symlink_metadata(path) {
        if !meta.is_file() || meta.file_type().is_symlink() {
            return Err(bad_request("Cannot replace unsafe JSON storage target"));
        }
    }
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| internal(format!("JSON serialization failed: {error}")))?;
    if bytes.len() as u64 > MAX_JSON_BYTES {
        return Err(bad_request("JSON storage write exceeded size limit"));
    }
    let tmp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy(),
        Uuid::new_v4()
    ));
    let saved = (|| -> io::Result<()> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();
    if let Err(error) = saved {
        let _ = fs::remove_file(&tmp);
        return Err(internal(format!("{}: {error}", path.display())));
    }
    Ok(())
}
pub fn validate_origin(headers: &HeaderMap) -> Result<(), ApiError> {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return Ok(());
    };
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| bad_request("Отсутствует Host"))?;
    if origin.to_str().ok() != Some(format!("http://{host}").as_str()) {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({"error":"Запрос с другого сайта запрещён"})),
        ));
    }
    Ok(())
}

pub fn writable_data_root(repository_root: &Path) -> Result<Option<PathBuf>, ApiError> {
    let Some(requested) = std::env::var_os("VOLLEY_RUST_DATA_ROOT") else {
        return Ok(None);
    };
    let requested = PathBuf::from(requested);
    if !requested.is_absolute() {
        return Err(internal("VOLLEY_RUST_DATA_ROOT must be absolute"));
    }
    // Validate the resolved destination BEFORE mkdir: creating even an empty
    // directory within the original project would violate migration isolation.
    if requested
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(internal(
            "VOLLEY_RUST_DATA_ROOT cannot contain parent traversal",
        ));
    }
    let mut existing = requested.as_path();
    while !existing.exists() {
        existing = existing
            .parent()
            .ok_or_else(|| internal("No existing parent for project storage directory"))?;
    }
    let parent = existing
        .canonicalize()
        .map_err(|error| internal(format!("Cannot resolve dev data parent: {error}")))?;
    let planned = parent.join(
        requested
            .strip_prefix(existing)
            .map_err(|error| internal(format!("Invalid dev data root: {error}")))?,
    );
    // Never write inside the original checkout (project/, results/, models/,
    // or .git), or an ancestor that contains the original checkout.
    if planned == repository_root
        || planned.starts_with(repository_root)
        || repository_root.starts_with(&planned)
    {
        return Err(internal(
            "Development writes must stay outside the original repository",
        ));
    }
    fs::create_dir_all(&planned)
        .map_err(|error| internal(format!("Cannot create dev data root: {error}")))?;
    let data_root = planned
        .canonicalize()
        .map_err(|error| internal(format!("Cannot canonicalize dev data root: {error}")))?;
    if data_root.starts_with(repository_root) || repository_root.starts_with(&data_root) {
        return Err(internal("Data root redirected into original checkout"));
    }
    Ok(Some(data_root))
}

#[cfg(test)]
mod tests {
    use super::{project_id, video_name};
    #[test]
    fn project_identifiers_are_non_traversable() {
        assert!(project_id("1234abcdef01").is_ok());
        assert!(project_id("../../unsafe").is_err());
        assert!(project_id("a/aaaaaaaaaa").is_err());
    }
    #[test]
    fn atomic_json_rejects_symlink_traversal_corrupt_payload_and_oversized_files() {
        use super::*;
        let root =
            std::env::temp_dir().join(format!("volley-json-fault83-{}", Uuid::new_v4().simple()));
        fs::create_dir_all(&root).unwrap();
        let safe = root.join("safe.json");
        write_json_atomic(&safe, &json!({"id":"a","revision":1})).unwrap();
        assert_eq!(read_json(&safe).unwrap().unwrap()["revision"], 1);
        write_json_atomic(&safe, &json!({"id":"a","revision":2})).unwrap();
        assert_eq!(read_json(&safe).unwrap().unwrap()["revision"], 2);
        fs::write(&safe, b"{broken").unwrap();
        assert!(
            read_json(&safe).is_err(),
            "Corrupted JSON cannot be silently defaulted"
        );
        let big = root.join("oversized.json");
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&big)
            .unwrap();
        file.set_len(MAX_JSON_BYTES + 1).unwrap();
        assert_eq!(read_json(&big).unwrap_err().0, StatusCode::BAD_REQUEST);
        #[cfg(unix)]
        {
            let protected = root.join("protected.json");
            fs::write(&protected, b"{\"keep\":true}").unwrap();
            let linked = root.join("external.json");
            std::os::unix::fs::symlink(&protected, &linked).unwrap();
            assert_eq!(read_json(&linked).unwrap_err().0, StatusCode::BAD_REQUEST);
            assert_eq!(
                write_json_atomic(&linked, &json!({"keep":false}))
                    .unwrap_err()
                    .0,
                StatusCode::BAD_REQUEST
            );
            let folder = root.join("linkdir");
            std::os::unix::fs::symlink(&root, &folder).unwrap();
            assert_eq!(
                write_json_atomic(&folder.join("unsafe.json"), &json!(true))
                    .unwrap_err()
                    .0,
                StatusCode::BAD_REQUEST
            );
            assert_eq!(fs::read(&protected).unwrap(), b"{\"keep\":true}");
        }
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn video_names_are_basename_only() {
        assert!(video_name("rally.MP4").is_ok());
        assert!(video_name("../rally.mp4").is_err());
        assert!(video_name("foo\\bar.mp4").is_err());
        assert!(video_name("rally.exe").is_err());
    }
}

#[cfg(test)]
mod upload_cleanup_tests {
    use super::*;
    #[test]
    fn incomplete_upload_is_removed_but_published_file_survives() {
        let path = std::env::temp_dir().join(format!("upload-guard-{}", Uuid::new_v4()));
        fs::write(&path, b"partial").unwrap();
        drop(PartialUpload::new(path.clone()));
        assert!(!path.exists());
        fs::write(&path, b"complete").unwrap();
        let mut guard = PartialUpload::new(path.clone());
        guard.disarm();
        drop(guard);
        assert_eq!(fs::read(&path).unwrap(), b"complete");
        fs::remove_file(path).unwrap();
    }
}
