//! Isolated Rust-native global athlete registry editor.
//! Existing albums and project links are never rewritten by a rename.
use crate::{
    storage::{self, ApiError, ApiResult},
    AppState,
};
use axum::{extract::State, http::HeaderMap, Json};
use fs2::FileExt;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io,
    sync::Arc,
};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

const MAX_PLAYERS: usize = 2000;
const MAX_REGISTRY_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogRequest {
    action: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    player_id: Option<String>,
    #[serde(default)]
    expected_revision: Option<String>,
}

fn registry_path(state: &AppState) -> std::path::PathBuf {
    state.data_root.join("players").join("registry.json")
}

fn checked_registry_path(
    state: &AppState,
    for_write: bool,
) -> Result<std::path::PathBuf, ApiError> {
    let folder = state.data_root.join("players");
    if for_write {
        if !state.writes_enabled {
            return Err(storage::conflict(
                "Изменение игроков требует отдельного VOLLEY_RUST_DATA_ROOT",
            ));
        }
        fs::create_dir_all(&folder)
            .map_err(|e| storage::internal(format!("Player registry directory: {e}")))?;
    }
    if let Ok(meta) = fs::symlink_metadata(&folder) {
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(storage::bad_request(
                "Каталог игроков должен быть обычным каталогом",
            ));
        }
        if for_write {
            let resolved = folder
                .canonicalize()
                .map_err(|e| storage::internal(format!("Resolve player folder: {e}")))?;
            if !resolved.starts_with(&state.data_root) {
                return Err(storage::bad_request("Каталог игроков вне Rust-хранилища"));
            }
        }
    }
    let path = registry_path(state);
    if let Ok(meta) = fs::symlink_metadata(&path) {
        if meta.file_type().is_symlink() || !meta.is_file() || meta.len() > MAX_REGISTRY_BYTES {
            return Err(storage::bad_request("Недопустимый файл реестра игроков"));
        }
    }
    Ok(path)
}

fn load(state: &AppState, for_write: bool) -> Result<(Value, String), ApiError> {
    let path = checked_registry_path(state, for_write)?;
    let bytes = match fs::read(&path) {
        Ok(value) => value,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok((json!({"schema":1,"players":[]}), "absent".into()));
        }
        Err(e) => return Err(storage::internal(format!("Read player registry: {e}"))),
    };
    if bytes.len() as u64 > MAX_REGISTRY_BYTES {
        return Err(storage::bad_request("Реестр игроков слишком большой"));
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| storage::bad_request("Повреждён реестр игроков"))?;
    let players = value
        .get("players")
        .and_then(Value::as_array)
        .ok_or_else(|| storage::bad_request("Реестр игроков не содержит список игроков"))?;
    if !value.is_object() || players.len() > MAX_PLAYERS {
        return Err(storage::bad_request("Некорректный реестр игроков"));
    }
    let revision = format!("{:x}", Sha256::digest(&bytes));
    Ok((value, revision))
}

fn normalized_name(name: &str) -> Result<String, ApiError> {
    let name = name.nfkc().collect::<String>();
    let trimmed = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if (1..=80).contains(&trimmed.chars().count()) && !trimmed.chars().any(|ch| ch.is_control()) {
        Ok(trimmed)
    } else {
        Err(storage::bad_request(
            "Имя игрока должно содержать от 1 до 80 символов",
        ))
    }
}

fn apply_edit(state: &AppState, req: CatalogRequest) -> Result<Value, ApiError> {
    if !matches!(req.action.as_str(), "create" | "rename") {
        return Err(storage::bad_request("Неизвестное изменение списка игроков"));
    }
    let name = normalized_name(req.name.as_deref().unwrap_or_default())?;
    let expected = req
        .expected_revision
        .as_deref()
        .ok_or_else(|| storage::bad_request("Нужна версия реестра expected_revision"))?;
    if expected != "absent"
        && (expected.len() != 64
            || !expected
                .bytes()
                .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase()))
    {
        return Err(storage::bad_request("Неправильная версия реестра"));
    }
    let path = checked_registry_path(state, true)?;
    let lock_path = path.with_file_name(".registry.edit.lock");
    if fs::symlink_metadata(&lock_path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(storage::bad_request("Недопустимый файл блокировки игроков"));
    }
    let lock: File = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|e| storage::internal(format!("Open player edit lock: {e}")))?;
    lock.lock_exclusive()
        .map_err(|e| storage::internal(format!("Lock player registry: {e}")))?;
    let (mut document, revision) = load(state, true)?;
    if revision != expected {
        return Err(storage::conflict(
            "Реестр игроков изменён в другом окне. Обновите список",
        ));
    }
    let entries = document["players"]
        .as_array_mut()
        .ok_or_else(|| storage::bad_request("Повреждён реестр игроков"))?;
    let duplicate = |id: Option<&str>| {
        entries.iter().any(|row| {
            row["name"]
                .as_str()
                .is_some_and(|old| old.to_lowercase() == name.to_lowercase())
                && row["id"].as_str() != id
        })
    };
    let player_id = if req.action == "create" {
        if entries.len() >= MAX_PLAYERS {
            return Err(storage::bad_request("Достигнут лимит игроков"));
        }
        if duplicate(None) {
            return Err(storage::conflict("Игрок с таким именем уже существует"));
        }
        let id = (0..20)
            .map(|_| Uuid::new_v4().simple().to_string()[..12].to_string())
            .find(|id| !entries.iter().any(|row| row["id"].as_str() == Some(id)))
            .ok_or_else(|| storage::internal("Cannot allocate player ID"))?;
        entries.push(json!({"id":id,"name":name,"created_by":"manual"}));
        id
    } else {
        let id = req
            .player_id
            .as_deref()
            .ok_or_else(|| storage::bad_request("Укажите ID игрока"))?;
        if id.len() != 12
            || !id
                .bytes()
                .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase())
        {
            return Err(storage::bad_request("Недопустимый ID игрока"));
        }
        if duplicate(Some(id)) {
            return Err(storage::conflict("Игрок с таким именем уже существует"));
        }
        let player = entries
            .iter_mut()
            .find(|row| row["id"].as_str() == Some(id))
            .ok_or_else(|| storage::not_found("Игрок не найден"))?;
        player["name"] = json!(name);
        id.to_string()
    };
    storage::write_json_atomic(&path, &document)?;
    let (_, new_revision) = load(state, true)?;
    Ok(json!({"players":document["players"],"player_id":player_id,"revision":new_revision}))
}

pub async fn edit(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CatalogRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    if req.action == "read" {
        let (document, revision) = tokio::task::spawn_blocking(move || load(&state, false))
            .await
            .map_err(|e| storage::internal(format!("Load registry task: {e}")))??;
        return Ok(Json(
            json!({"players":document["players"],"revision":revision}),
        ));
    }
    let edited = tokio::task::spawn_blocking(move || apply_edit(&state, req))
        .await
        .map_err(|e| storage::internal(format!("Edit player task: {e}")))??;
    Ok(Json(edited))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_normalize_and_reject_controls() {
        assert_eq!(normalized_name("  Ильнур    А  ").unwrap(), "Ильнур А");
        assert_eq!(normalized_name("Ｆｏｏ").unwrap(), "Foo");
        assert!(normalized_name("  ").is_err());
        assert!(normalized_name(&"x".repeat(81)).is_err());
        assert!(normalized_name("A\u{0000}B").is_err());
    }
}

#[cfg(test)]
mod registry_tests {
    use super::*;

    fn test_state() -> (std::path::PathBuf, AppState) {
        let dir = std::env::temp_dir().join(format!("volley-rust-catalog-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let state = AppState {
            repository_root: dir.join("original-readonly"),
            data_root: dir.join("sandbox"),
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
        };
        (dir, state)
    }

    #[test]
    fn lifecycle_uses_revision_and_preserves_player_identity() {
        let (dir, state) = test_state();
        let (initial, version) = load(&state, false).unwrap();
        assert_eq!(version, "absent");
        assert!(initial["players"].as_array().unwrap().is_empty());
        let created = apply_edit(
            &state,
            CatalogRequest {
                action: "create".into(),
                name: Some("  Ильнур  ".into()),
                player_id: None,
                expected_revision: Some(version),
            },
        )
        .unwrap();
        assert_eq!(created["players"][0]["name"], "Ильнур");
        let id = created["player_id"].as_str().unwrap().to_owned();
        let current = created["revision"].as_str().unwrap().to_owned();
        let stale = apply_edit(
            &state,
            CatalogRequest {
                action: "create".into(),
                name: Some("Ирек".into()),
                player_id: None,
                expected_revision: Some("absent".into()),
            },
        );
        assert_eq!(stale.unwrap_err().0, axum::http::StatusCode::CONFLICT);
        let renamed = apply_edit(
            &state,
            CatalogRequest {
                action: "rename".into(),
                name: Some("Ирек".into()),
                player_id: Some(id.clone()),
                expected_revision: Some(current),
            },
        )
        .unwrap();
        assert_eq!(renamed["players"][0]["id"], id);
        assert_eq!(renamed["players"][0]["created_by"], "manual");
        assert_eq!(renamed["players"][0]["name"], "Ирек");
        assert_ne!(created["revision"], renamed["revision"]);
        let another = apply_edit(
            &state,
            CatalogRequest {
                action: "create".into(),
                name: Some("ирек".into()),
                player_id: None,
                expected_revision: Some(renamed["revision"].as_str().unwrap().into()),
            },
        );
        assert_eq!(another.unwrap_err().0, axum::http::StatusCode::CONFLICT);
        assert!(!state.repository_root.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_nonisolated_edits_and_symlinks() {
        let (dir, mut state) = test_state();
        state.writes_enabled = false;
        let read_only = apply_edit(
            &state,
            CatalogRequest {
                action: "create".into(),
                name: Some("Ирек".into()),
                player_id: None,
                expected_revision: Some("absent".into()),
            },
        );
        assert_eq!(read_only.unwrap_err().0, axum::http::StatusCode::CONFLICT);
        assert!(!state.data_root.exists());
        state.writes_enabled = true;
        #[cfg(unix)]
        {
            let external = dir.join("external.json");
            fs::write(&external, r#"{"schema":1,"players":[]}"#).unwrap();
            let players = state.data_root.join("players");
            fs::create_dir_all(&players).unwrap();
            std::os::unix::fs::symlink(&external, players.join("registry.json")).unwrap();
            assert_eq!(
                load(&state, true).unwrap_err().0,
                axum::http::StatusCode::BAD_REQUEST
            );
            assert_eq!(
                fs::read_to_string(external).unwrap(),
                r#"{"schema":1,"players":[]}"#
            );
        }
        fs::remove_dir_all(dir).unwrap();
    }
}
