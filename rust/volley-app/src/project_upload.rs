//! Browser-owned source upload for native Rust projects.
//! Streaming writes stay inside the isolated Rust data root. Originals are not modified.
use crate::{
    projects,
    storage::{self, ApiResult},
    AppState,
};
use axum::{
    extract::{Multipart, Query, State},
    http::HeaderMap,
    Json,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, fs, io::Read, path::Path, sync::Arc};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

const MAX_PROJECT_UPLOAD: u64 = crate::storage::MAX_BROWSER_VIDEO_BYTES;
// Serialize the receipt check + rename so simultaneous identical uploads
// cannot create two stored sources inside one Studio process.
static UPLOAD_FINALIZE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

// Receipts are an index, never a client-supplied path. Legacy receipts may
// lack SHA-256: hash only same-size files so they can also be reused.
fn stored_for_hash(root: &Path, hash: &str, size: u64) -> Result<Option<Value>, storage::ApiError> {
    if fs::symlink_metadata(root)
        .map_or(true, |meta| meta.file_type().is_symlink() || !meta.is_dir())
    {
        return Ok(None);
    }
    let entries =
        fs::read_dir(root).map_err(|e| storage::internal(format!("Read upload receipts: {e}")))?;
    for entry in entries {
        let entry = entry.map_err(|e| storage::internal(format!("Read receipt entry: {e}")))?;
        let path = entry.path();
        if path.extension().is_none_or(|x| x != "json")
            || !entry.file_type().is_ok_and(|kind| kind.is_file())
        {
            continue;
        }
        let Some(receipt) = storage::read_json(&path)? else {
            continue;
        };
        if receipt["schema"] != "volley-project-upload/1" || receipt["size"].as_u64() != Some(size)
        {
            continue;
        }
        let Some(name) = receipt["name"].as_str() else {
            continue;
        };
        if storage::video_name(name).is_err()
            || path.file_name().and_then(|n| n.to_str()) != Some(format!("{name}.json").as_str())
        {
            continue;
        }
        let source = root.join(name);
        if !fs::symlink_metadata(&source)
            .is_ok_and(|m| m.is_file() && !m.file_type().is_symlink() && m.len() == size)
        {
            continue;
        }
        let matched = if let Some(saved) = receipt["sha256"].as_str() {
            saved == hash
        } else {
            let mut reader = fs::File::open(&source)
                .map_err(|e| storage::internal(format!("Open legacy upload: {e}")))?;
            let mut digest = Sha256::new();
            let mut buffer = [0u8; 1024 * 1024];
            loop {
                let n = reader
                    .read(&mut buffer)
                    .map_err(|e| storage::internal(format!("Hash legacy upload: {e}")))?;
                if n == 0 {
                    break;
                }
                digest.update(&buffer[..n]);
            }
            format!("{:x}", digest.finalize()) == hash
        };
        if matched {
            return Ok(Some(json!({
                "name":name,
                "display_name":receipt["display_name"].as_str().unwrap_or(name),
                "size":size,
                "sha256":hash,
                "uploaded":false,
                "duplicate":true
            })));
        }
    }
    Ok(None)
}

pub async fn lookup(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult<Value> {
    let hash = query.get("sha256").map(String::as_str).unwrap_or_default();
    let size = query
        .get("size")
        .and_then(|n| n.parse::<u64>().ok())
        .unwrap_or(0);
    if !valid_hash(hash) || !(1024..=MAX_PROJECT_UPLOAD).contains(&size) {
        return Err(storage::bad_request("Invalid SHA-256 or video size"));
    }
    let root = projects::uploaded_video_root(&state);
    let hash = hash.to_string();
    let result = tokio::task::spawn_blocking(move || stored_for_hash(&root, &hash, size))
        .await
        .map_err(|e| storage::internal(format!("Check upload hash: {e}")))??;
    Ok(Json(result.unwrap_or_else(|| json!({"duplicate":false}))))
}

use crate::storage::PartialUpload;

pub async fn upload(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> ApiResult<serde_json::Value> {
    storage::validate_origin(&headers)?;
    if !state.writes_enabled {
        return Err(storage::conflict(
            "Загрузка требует отдельного хранилища Rust Studio",
        ));
    }
    if headers
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .is_some_and(|n| n > MAX_PROJECT_UPLOAD + 16 * 1024 * 1024)
    {
        return Err(storage::bad_request(
            "Максимальный размер одного видео — 32 ГиБ",
        ));
    }
    let claimed_hash = headers
        .get("x-content-sha256")
        .and_then(|h| h.to_str().ok())
        .map(str::to_string);
    if claimed_hash.as_deref().is_some_and(|h| !valid_hash(h)) {
        return Err(storage::bad_request("Invalid SHA-256"));
    }
    let claimed_size = headers
        .get("x-content-size")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.parse::<u64>().ok());
    if claimed_size.is_some_and(|n| !(1024..=MAX_PROJECT_UPLOAD).contains(&n)) {
        return Err(storage::bad_request("Invalid upload size"));
    }
    let mut digest = Sha256::new();
    let mut field = multipart
        .next_field()
        .await
        .map_err(|_| storage::bad_request("Не удалось прочитать загружаемое видео"))?
        .ok_or_else(|| storage::bad_request("Выберите видео"))?;
    if field.name() != Some("file") {
        return Err(storage::bad_request("Ожидалось поле file"));
    }
    let original = field.file_name().unwrap_or("").to_owned();
    storage::video_name(&original)?;
    if original.len() > 180 {
        return Err(storage::bad_request(
            "Имя видео слишком длинное (максимум 180 байт)",
        ));
    }
    let extension = Path::new(&original)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let stem = Path::new(&original)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("video");
    if stem.is_empty() {
        return Err(storage::bad_request("Пустое имя видео"));
    }

    // Never touch the historical repository/project directory or original desktop videos.
    let root = projects::uploaded_video_root(&state);
    // Check ancestors before creating a directory, not after following a
    // symlink into a potentially unrelated user path.
    for check in [state.data_root.join("uploads"), root.clone()] {
        if fs::symlink_metadata(&check).is_ok_and(|m| m.file_type().is_symlink() || !m.is_dir()) {
            return Err(storage::bad_request("Небезопасный каталог загрузки"));
        }
    }
    fs::create_dir_all(&root)
        .map_err(|e| storage::internal(format!("Create isolated uploads: {e}")))?;
    let canonical_root = root
        .canonicalize()
        .map_err(|e| storage::internal(format!("Resolve isolated uploads: {e}")))?;
    let canonical_data = state
        .data_root
        .canonicalize()
        .map_err(|e| storage::internal(format!("Resolve isolated data root: {e}")))?;
    if !canonical_root.starts_with(&canonical_data)
        || canonical_root == canonical_data
        || fs::symlink_metadata(&root).is_ok_and(|m| m.file_type().is_symlink() || !m.is_dir())
    {
        return Err(storage::bad_request(
            "Каталог загрузок находится вне изолированного хранилища",
        ));
    }
    let uuid = Uuid::new_v4().simple();
    let stored_name = format!("{stem}--{uuid}.{extension}");
    storage::video_name(&stored_name)?;
    let destination = root.join(&stored_name);
    // A private partial file is unlisted and cleaned even if the HTTP upload is cancelled.
    let temp = root.join(format!(".{uuid}.uploading"));
    let mut cleanup = PartialUpload::new(temp.clone());
    let mut output = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)
        .await
        .map_err(|e| storage::internal(format!("Create video upload: {e}")))?;
    let mut total = 0u64;
    while let Some(chunk) = field
        .chunk()
        .await
        .map_err(|_| storage::bad_request("Передача видео прервана"))?
    {
        total = total
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| storage::bad_request("Размер видео переполнен"))?;
        if total > MAX_PROJECT_UPLOAD {
            return Err(storage::bad_request(
                "Максимальный размер одного видео — 32 ГиБ",
            ));
        }
        digest.update(&chunk);
        output
            .write_all(&chunk)
            .await
            .map_err(|e| storage::internal(format!("Write video upload: {e}")))?;
    }
    if total < 1024 {
        return Err(storage::bad_request("Видео слишком маленькое"));
    }
    if claimed_size.is_some_and(|expected| expected != total) {
        return Err(storage::bad_request("Upload size mismatch"));
    }
    let sha256 = format!("{:x}", digest.finalize());
    if claimed_hash
        .as_deref()
        .is_some_and(|expected| expected != sha256)
    {
        return Err(storage::bad_request("Upload SHA-256 mismatch"));
    }
    output
        .flush()
        .await
        .map_err(|e| storage::internal(format!("Flush upload: {e}")))?;
    output
        .sync_all()
        .await
        .map_err(|e| storage::internal(format!("Sync upload: {e}")))?;
    drop(output);
    drop(field);
    if multipart
        .next_field()
        .await
        .map_err(|_| storage::bad_request("Некорректное завершение загрузки"))?
        .is_some()
    {
        return Err(storage::bad_request("Один видеофайл на запрос"));
    }

    let _finalize = UPLOAD_FINALIZE.lock().await;
    // Check again after streaming: two clients can upload the same source at once.
    let match_root = root.clone();
    let match_hash = sha256.clone();
    if let Some(reused) =
        tokio::task::spawn_blocking(move || stored_for_hash(&match_root, &match_hash, total))
            .await
            .map_err(|e| storage::internal(format!("Deduplicate upload: {e}")))??
    {
        return Ok(Json(reused));
    }

    // Real ffprobe verifies the uploaded file before adding it to the catalog.
    let to_probe = temp.clone();
    let info = tokio::task::spawn_blocking(move || volley_media::probe(&to_probe))
        .await
        .map_err(|e| storage::internal(format!("Probe upload task: {e}")))?
        .map_err(storage::bad_request)?;
    if !(info.duration_sec.is_finite() && info.duration_sec >= 1.0) {
        return Err(storage::bad_request(
            "Невозможно определить длительность видео",
        ));
    }
    if destination.exists() {
        return Err(storage::conflict("Повторный идентификатор загрузки"));
    }
    tokio::fs::rename(&temp, &destination)
        .await
        .map_err(|e| storage::internal(format!("Finish upload: {e}")))?;
    // The renamed file is also cleaned if metadata cannot be committed.
    cleanup.disarm();
    let mut saved = PartialUpload::new(destination.clone());
    let receipt = root.join(format!("{stored_name}.json"));
    storage::write_json_atomic(
        &receipt,
        &json!({
            "schema":"volley-project-upload/1",
            "name":stored_name,
            "display_name":original,
            "size":total,
            "sha256":sha256
        }),
    )?;
    saved.disarm();
    Ok(Json(json!({
        "name":stored_name, "display_name":original, "size":total,
        "duration_sec":info.duration_sec, "sha256":sha256, "uploaded":true, "duplicate":false
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn supports_more_than_fourteen_gib() {
        // Compile-time contract: the upload ceiling must exceed 14 GiB.
        const _: () = assert!(14u64 * 1024 * 1024 * 1024 < MAX_PROJECT_UPLOAD);
        assert!(valid_hash(&"ab".repeat(32)));
        assert!(!valid_hash(&"AB".repeat(32)));
    }
    #[test]
    fn receipt_lookup_reuses_identical_bytes_and_rejects_same_size_other_hash() {
        let root = std::env::temp_dir().join(format!("volley-upload-hash-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let name = "uploaded-test.mp4";
        let bytes = vec![0x51; 2048];
        fs::write(root.join(name), &bytes).unwrap();
        let sha = format!("{:x}", Sha256::digest(&bytes));
        storage::write_json_atomic(
            &root.join(format!("{name}.json")),
            &json!({
                "schema":"volley-project-upload/1","name":name,"display_name":"source.mp4",
                "size":bytes.len() as u64,"sha256":sha
            }),
        )
        .unwrap();
        let hit = stored_for_hash(&root, &sha, bytes.len() as u64)
            .unwrap()
            .unwrap();
        assert_eq!(hit["duplicate"], true);
        assert_eq!(hit["name"], name);
        assert!(stored_for_hash(&root, &"0".repeat(64), bytes.len() as u64)
            .unwrap()
            .is_none());
        fs::remove_dir_all(&root).unwrap();
    }
    #[test]
    fn legacy_receipt_is_deduplicated_by_streamed_hash() {
        let root = std::env::temp_dir().join(format!("volley-upload-legacy-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let name = "legacy.mp4";
        let bytes = vec![0x11; 3072];
        fs::write(root.join(name), &bytes).unwrap();
        let sha = format!("{:x}", Sha256::digest(&bytes));
        storage::write_json_atomic(
            &root.join(format!("{name}.json")),
            &json!({
                "schema":"volley-project-upload/1","name":name,"display_name":"legacy.mp4",
                "size":bytes.len() as u64
            }),
        )
        .unwrap();
        assert!(stored_for_hash(&root, &sha, bytes.len() as u64)
            .unwrap()
            .is_some());
        fs::remove_dir_all(&root).unwrap();
    }
}
