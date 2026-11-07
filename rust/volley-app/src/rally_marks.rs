//! Explicit gold bookmarks for *complete* rallies, scoped to selected CPU runs.
//! Persist only in isolated Rust project storage; do not alter CPU/GPU caches.
//! The bookmark identity excludes padded source frames, unlike rally_id.

use crate::{
    analysis_runs, projects, rallies,
    storage::{self, ApiError, ApiResult},
    AppState,
};
use axum::{extract::State, http::HeaderMap, Json};
use fs2::FileExt;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::Read,
    path::Path,
    sync::Arc,
};

const MAX_MARKS_BYTES: u64 = 2 * 1024 * 1024;
const MAX_MARKS: usize = 5000;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarksRequest {
    pub project_id: String,
    pub run_id: String,
    #[serde(default = "read_action")]
    pub action: String,
    #[serde(default)]
    pub video: Option<String>,
    #[serde(default)]
    pub rally_id: Option<String>,
    /// Used to re-validate the particular rally_id the user clicked.
    #[serde(default = "default_context")]
    pub context_sec: f64,
    #[serde(default = "default_gap")]
    pub gap_sec: f64,
    #[serde(default)]
    pub rally_min_sec: f64,
    /// Required for action=set. True marks, false unmarks.
    #[serde(default)]
    pub marked: Option<bool>,
    /// SHA256 returned by read or previous set ("absent" if not created).
    #[serde(default)]
    pub expected_revision: Option<String>,
}
fn read_action() -> String {
    "read".into()
}
fn default_context() -> f64 {
    1.5
}
fn default_gap() -> f64 {
    3.0
}

pub(crate) fn anchor(scene: &rallies::Scene) -> String {
    let mut ids = scene.rally.track_ids.clone();
    ids.sort_unstable();
    let payload = json!([
        scene.video,
        scene.cache_id,
        scene.rally.detected_start_frame,
        scene.rally.detected_end_frame,
        ids,
    ]);
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&payload).expect("finite rally anchor"))
    )[..24]
        .to_owned()
}
fn document_path(state: &AppState, project_id: &str) -> std::path::PathBuf {
    projects::project_root(state)
        .join(project_id)
        .join("rally_marks.json")
}
fn read_bytes(path: &Path) -> Result<Option<Vec<u8>>, ApiError> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > MAX_MARKS_BYTES {
            return Err(storage::bad_request(
                "Файл золотых отметок повреждён или слишком большой",
            ));
        }
    }
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(storage::internal(format!("Open rally marks: {e}"))),
    };
    let mut data = Vec::new();
    file.take(MAX_MARKS_BYTES + 1)
        .read_to_end(&mut data)
        .map_err(|e| storage::internal(format!("Read rally marks: {e}")))?;
    if data.len() as u64 > MAX_MARKS_BYTES {
        return Err(storage::bad_request(
            "Превышен размер списка золотых отметок",
        ));
    }
    Ok(Some(data))
}
fn revision(bytes: Option<&[u8]>) -> String {
    bytes
        .map(|data| format!("{:x}", Sha256::digest(data)))
        .unwrap_or_else(|| "absent".into())
}
fn parse(bytes: Option<&[u8]>) -> Result<Value, ApiError> {
    let Some(bytes) = bytes else {
        return Ok(json!({"schema":"volley-rally-marks/1","marks":[]}));
    };
    let doc: Value = serde_json::from_slice(bytes)
        .map_err(|_| storage::bad_request("Некорректный JSON золотых отметок"))?;
    if doc["schema"].as_str() != Some("volley-rally-marks/1")
        || doc["marks"].as_array().is_none()
        || doc["marks"].as_array().is_some_and(|m| m.len() > MAX_MARKS)
    {
        return Err(storage::bad_request(
            "Неподдерживаемая версия золотых отметок",
        ));
    }
    for mark in doc["marks"].as_array().expect("schema validated") {
        if mark["run_id"].as_str().is_none()
            || mark["video"].as_str().is_none()
            || mark["cache_id"].as_str().is_none()
            || mark["anchor_id"]
                .as_str()
                .is_none_or(|s| s.len() != 24 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(storage::bad_request("В файле отметок повреждена запись"));
        }
    }
    Ok(doc)
}
fn record_ids(doc: &Value, run_id: &str) -> HashSet<String> {
    doc["marks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|mark| mark["run_id"].as_str() == Some(run_id))
        .filter_map(|mark| mark["anchor_id"].as_str().map(str::to_owned))
        .collect()
}
pub(crate) fn marked_anchors(
    state: &AppState,
    project_id: &str,
    run_id: &str,
) -> Result<HashSet<String>, ApiError> {
    let path = document_path(state, project_id);
    let bytes = read_bytes(&path)?;
    let doc = parse(bytes.as_deref())?;
    Ok(record_ids(&doc, run_id))
}
fn status(req: &MarksRequest, doc: &Value, rev: String, writable: bool) -> Value {
    let chosen = doc["marks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|mark| mark["run_id"].as_str() == Some(req.run_id.as_str()))
        .filter(|mark| {
            req.video
                .as_deref()
                .is_none_or(|video| mark["video"].as_str() == Some(video))
        })
        .cloned()
        .collect::<Vec<_>>();
    json!({
        "schema":"volley-rally-marks/1",
        "project_id":req.project_id,
        "run_id":req.run_id,
        "video":req.video,
        "revision":rev,
        "writable":writable,
        "marks":chosen,
        "marked_count":chosen.len(),
        "instruction":"Use the returned revision when marking/unmarking. The whole rally, not a single touch, is marked."
    })
}
fn validate_run(state: &AppState, req: &MarksRequest) -> Result<(), ApiError> {
    storage::project_id(&req.project_id)?;
    let project = projects::existing_project(state, &req.project_id)?;
    if let Some(video) = &req.video {
        storage::video_name(video)?;
        if !project["videos"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|v| v.as_str() == Some(video)))
        {
            return Err(storage::bad_request("Видео не относится к этому проекту"));
        }
    }
    if req.run_id.len() > 104
        || !req.run_id.starts_with("run_")
        || !req
            .run_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return Err(storage::bad_request("Некорректная версия CPU-анализа"));
    }
    let run = analysis_runs::list_runs(state, &req.project_id)?
        .into_iter()
        .find(|row| row["id"].as_str() == Some(req.run_id.as_str()))
        .ok_or_else(|| storage::bad_request("Не найден выбранный запуск CPU-анализа"))?;
    if run["inference_only"].as_bool() == Some(true) {
        return Err(storage::bad_request("Для отметок нужны CPU-розыгрыши"));
    }
    if let Some(video) = &req.video {
        if run["videos"][video].as_str().is_none() {
            return Err(storage::bad_request("В указанном запуске нет этого видео"));
        }
    }
    Ok(())
}

fn mutate(state: &AppState, req: &MarksRequest) -> Result<Value, ApiError> {
    if !state.writes_enabled {
        return Err(storage::conflict(
            "Изменять золотые отметки можно только в изолированном Rust-хранилище",
        ));
    }
    let video = req
        .video
        .as_ref()
        .ok_or_else(|| storage::bad_request("Для отметки укажите видео"))?;
    let rally_id = req
        .rally_id
        .as_deref()
        .ok_or_else(|| storage::bad_request("Для отметки укажите ID розыгрыша"))?;
    if rally_id.len() != 24 || !rally_id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(storage::bad_request("Недопустимый ID розыгрыша"));
    }
    let marked = req
        .marked
        .ok_or_else(|| storage::bad_request("Нужно явно указать marked:true/false"))?;
    let expected = req.expected_revision.as_deref().ok_or_else(|| {
        storage::bad_request("Сначала прочитайте отметки и передайте expected_revision")
    })?;
    if expected != "absent"
        && (expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(storage::bad_request("Неверный SHA256 revision отметок"));
    }
    let canonical_root = projects::project_root(state)
        .canonicalize()
        .map_err(|_| storage::bad_request("Изолированное хранилище не существует"))?;
    let project = canonical_root
        .join(&req.project_id)
        .canonicalize()
        .map_err(|_| storage::bad_request("Изолированный проект не найден"))?;
    if project == canonical_root || !project.starts_with(&canonical_root) {
        return Err(storage::bad_request("Запрещён выход из каталога проекта"));
    }
    let path = project.join("rally_marks.json");
    if fs::symlink_metadata(&path)
        .ok()
        .is_some_and(|m| m.file_type().is_symlink())
    {
        return Err(storage::bad_request("Файл отметок не может быть ссылкой"));
    }
    let lock_path = project.join(".rally_marks.lock");
    if fs::symlink_metadata(&lock_path)
        .ok()
        .is_some_and(|m| m.file_type().is_symlink())
    {
        return Err(storage::bad_request(
            "Файл блокировки не может быть ссылкой",
        ));
    }
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&lock_path)
        .map_err(|e| storage::internal(format!("Cannot open rally marks lock: {e}")))?;
    lock.lock_exclusive()
        .map_err(|e| storage::internal(format!("Cannot lock rally marks: {e}")))?;
    let old = read_bytes(&path)?;
    if revision(old.as_deref()) != expected {
        return Err(storage::conflict(
            "Отметки изменились в другой вкладке; обновите карточки",
        ));
    }
    let mut doc = parse(old.as_deref())?;

    // Resolve exactly one real rally from the selected run, no caller-provided
    // offsets. In particular, unranked rallies are valid gold candidates.
    let lookup: rallies::SelectionRequest = serde_json::from_value(json!({
        "project_id":req.project_id,
        "run_id":req.run_id,
        "rally_selection_mode":"manual",
        "selected_videos":[video],
        "context_sec":req.context_sec,
        "gap_sec":req.gap_sec,
        "rally_min_sec":req.rally_min_sec,
    }))
    .map_err(|e| storage::bad_request(format!("Cannot resolve rally selection: {e}")))?;
    let selection = rallies::select_for_montage(state, &lookup)?;
    let scene = selection
        .candidates
        .iter()
        .find(|scene| scene.rally.rally_id.as_deref() == Some(rally_id) && scene.video == *video)
        .ok_or_else(|| {
            storage::conflict(
                "Этот ID розыгрыша устарел; обновите предпросмотр с текущими CPU настройками",
            )
        })?;
    let key = anchor(scene);
    let rows = doc["marks"].as_array_mut().expect("validated marks");
    let existing = rows.iter().position(|row| {
        row["run_id"].as_str() == Some(req.run_id.as_str())
            && row["anchor_id"].as_str() == Some(key.as_str())
    });
    let changed = match (marked, existing) {
        (true, Some(_)) | (false, None) => false,
        (false, Some(index)) => {
            rows.remove(index);
            true
        }
        (true, None) => {
            if rows.len() >= MAX_MARKS {
                return Err(storage::bad_request("Достигнут лимит золотых отметок"));
            }
            rows.push(json!({
                "run_id":req.run_id,
                "video":video,
                "cache_id":scene.cache_id,
                "rally_id":rally_id,
                "anchor_id":key,
                "detected_start_frame":scene.rally.detected_start_frame,
                "detected_end_frame":scene.rally.detected_end_frame,
                "track_ids":scene.rally.track_ids,
                "gold":true
            }));
            true
        }
    };
    let revision = if changed {
        storage::write_json_atomic(&path, &doc)?;
        revision(read_bytes(&path)?.as_deref())
    } else {
        expected.to_owned()
    };
    let mut result = status(req, &doc, revision, true);
    result["marked"] = json!(marked);
    result["changed"] = json!(changed);
    result["rally_id"] = json!(rally_id);
    result["anchor_id"] = json!(key);
    Ok(result)
}

fn compute(state: &AppState, req: MarksRequest) -> Result<Value, ApiError> {
    if !matches!(req.action.as_str(), "read" | "set") {
        return Err(storage::bad_request("Допустимые действия: read и set"));
    }
    validate_run(state, &req)?;
    if req.action == "set" {
        return mutate(state, &req);
    }
    if req.marked.is_some() || req.rally_id.is_some() || req.expected_revision.is_some() {
        return Err(storage::bad_request(
            "read не изменяет отметки; уберите параметры записи",
        ));
    }
    let path = document_path(state, &req.project_id);
    let bytes = read_bytes(&path)?;
    let doc = parse(bytes.as_deref())?;
    Ok(status(
        &req,
        &doc,
        revision(bytes.as_deref()),
        state.writes_enabled,
    ))
}

pub async fn marks(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<MarksRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    let _permit = state
        .marks_limit
        .acquire()
        .await
        .map_err(|e| storage::internal(format!("Rally mark concurrency: {e}")))?;
    let local = state.clone();
    let result = tokio::task::spawn_blocking(move || compute(&local, req))
        .await
        .map_err(|e| storage::internal(format!("Rally mark task failed: {e}")))??;
    Ok(Json(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use volley_core::rallies::Rally;

    fn sample(start: u64, end: u64, context: u64, cache: &str) -> rallies::Scene {
        rallies::Scene {
            video: "match.mp4".into(),
            cache_id: cache.into(),
            analysis_offset_sec: 0.0,
            rally: Rally {
                detected_start_frame: start,
                detected_end_frame: end,
                source_start_frame: start.saturating_sub(context),
                source_end_frame: end + context,
                serve_frame: None,
                start_evidence: "first_rally_ball_track".into(),
                end_evidence: "last_rally_ball_track".into(),
                joined_parts: 1,
                track_ids: vec![5, 3],
                confidence: 0.9,
                duration_sec: 4.0,
                ranking_score: 1.0,
                base_ranking_score: 1.0,
                ranked: false,
                action_counts: BTreeMap::new(),
                actions_total: 0,
                boundary_method: "test".into(),
                context_before_frames: context,
                context_after_frames: context,
                selection_strategy: "highlights".into(),
                rally_id: None,
                sequence: None,
                montage_start_sec: None,
            },
            gold_marked: false,
            player_action_counts: None,
            player_actions_total: None,
            player_focus_matches: None,
        }
    }
    #[test]
    fn bookmark_survives_context_change() {
        let short = sample(100, 200, 10, "base.rev");
        let long = sample(100, 200, 40, "base.rev");
        assert_eq!(anchor(&short), anchor(&long));
        assert_ne!(
            volley_core::rallies::rally_selection_id("match.mp4", "base.rev", &short.rally),
            volley_core::rallies::rally_selection_id("match.mp4", "base.rev", &long.rally)
        );
    }
    #[test]
    fn different_source_or_detected_window_is_distinct() {
        assert_ne!(
            anchor(&sample(100, 200, 10, "base.rev")),
            anchor(&sample(100, 201, 10, "base.rev"))
        );
        assert_ne!(
            anchor(&sample(100, 200, 10, "base.rev")),
            anchor(&sample(100, 200, 10, "base.next"))
        );
    }
    #[test]
    fn stale_revision_never_matches_different_file() {
        assert_eq!(revision(None), "absent");
        assert_ne!(revision(Some(b"{}")), revision(Some(b"{ }")));
    }
}
