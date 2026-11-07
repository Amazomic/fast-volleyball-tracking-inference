//! Shared Rust-native JSON storage and origin/path safety.

use axum::{
    http::{header, HeaderMap, StatusCode},
    Json,
};
use serde_json::{json, Value};
use std::{
    fs, io,
    path::{Path, PathBuf},
};
use uuid::Uuid;

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
    eprintln!("Rust storage error: {}", message.as_ref());
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

pub fn read_json(path: &Path) -> Result<Option<Value>, ApiError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(internal(format!("{}: {error}", path.display()))),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| internal(format!("corrupted JSON {}: {error}", path.display())))
}

pub fn write_json_atomic(path: &Path, value: &Value) -> Result<(), ApiError> {
    let parent = path
        .parent()
        .ok_or_else(|| internal("JSON output has no parent"))?;
    fs::create_dir_all(parent)
        .map_err(|error| internal(format!("{}: {error}", parent.display())))?;
    let tmp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy(),
        Uuid::new_v4()
    ));
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| internal(format!("JSON serialization failed: {error}")))?;
    let saved = (|| -> io::Result<()> {
        fs::write(&tmp, bytes)?;
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
            .ok_or_else(|| internal("No existing parent for Rust data directory"))?;
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
            "Rust dev writes must stay outside the original repository",
        ));
    }
    fs::create_dir_all(&planned)
        .map_err(|error| internal(format!("Cannot create dev data root: {error}")))?;
    let data_root = planned
        .canonicalize()
        .map_err(|error| internal(format!("Cannot canonicalize dev data root: {error}")))?;
    if data_root.starts_with(repository_root) || repository_root.starts_with(&data_root) {
        return Err(internal("Rust data root redirected into original checkout"));
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
    fn video_names_are_basename_only() {
        assert!(video_name("rally.MP4").is_ok());
        assert!(video_name("../rally.mp4").is_err());
        assert!(video_name("foo\\bar.mp4").is_err());
        assert!(video_name("rally.exe").is_err());
    }
}
