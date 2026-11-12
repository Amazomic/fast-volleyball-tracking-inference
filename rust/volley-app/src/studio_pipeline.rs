//! Complete Studio analysis: one fresh pair of GPU runs followed by native CPU stages.
//! The profile and immutable output IDs are retained for each launch. No fake results.
use crate::{
    cpu_rallies, cpu_retrack, cpu_touches, inference_full, inference_fusion, preparation,
    project_guard, projects,
    storage::{self, ApiError, ApiResult},
    AppState,
};
use axum::{
    extract::{Query, State},
    http::HeaderMap,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, OnceLock,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;
use uuid::Uuid;
use volley_core::{
    rally_reconstruction::RallySettings, touch_pipeline::TouchSettings, tracker::TrackingOptions,
};
use volley_inference::ModelKind;

const SCHEMA: &str = "volley-studio-pipeline/1";
const MAX_STATUS_BYTES: u64 = 32 * 1024;
const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;
const MAX_LOG_ROW: usize = 8192;
const MAX_LOG_PAGE: usize = 150;
const MAX_LOG_ENTRIES: usize = 4000;

/// Append-only structured diagnostics. Function calls are not shell commands;
/// external processes are explicitly labelled with their real executable/argv.
fn log_entry(
    path: &Path,
    level: &str,
    kind: &str,
    stage: &str,
    message: &str,
    call: Option<Value>,
) -> Result<(), String> {
    let file_path = path.with_file_name("events.jsonl");
    let before = match fs::symlink_metadata(&file_path) {
        Ok(meta) => {
            if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > MAX_LOG_BYTES {
                return Err("Unsafe or oversized pipeline log".into());
            }
            meta.len()
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
        Err(e) => return Err(format!("Read pipeline log metadata: {e}")),
    };
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("Clock: {e}"))?
        .as_millis();
    let record = json!({
        "timestamp_ms":timestamp_ms,
        "level":level,"kind":kind,"stage":stage,"message":message,
        "call":call
    });
    let bytes = serde_json::to_vec(&record).map_err(|e| format!("Serialize log entry: {e}"))?;
    if bytes.len() > MAX_LOG_ROW || before + bytes.len() as u64 + 1 > MAX_LOG_BYTES {
        return Err("Pipeline log capacity exceeded".into());
    }
    let mut output = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file_path)
        .map_err(|e| format!("Open pipeline log: {e}"))?;
    let mut line = bytes;
    line.push(b'\n');
    output
        .write_all(&line)
        .map_err(|e| format!("Write pipeline log: {e}"))
}
fn log_native(path: &Path, stage: &str, function: &str, arguments: Value) -> Result<(), String> {
    log_entry(
        path,
        "info",
        "invocation",
        stage,
        "Нативный вызов Rust",
        Some(json!({"type":"native_rust","name":function,"arguments":arguments})),
    )
}
fn log_event(path: &Path, stage: &str, level: &str, message: &str) -> Result<(), String> {
    log_entry(path, level, "event", stage, message, None)
}
#[derive(Deserialize)]
pub struct LogQuery {
    project_id: String,
    id: String,
    #[serde(default)]
    after: usize,
}
pub async fn log(
    State(state): State<Arc<AppState>>,
    Query(query): Query<LogQuery>,
) -> ApiResult<Value> {
    if query.after > MAX_LOG_ENTRIES {
        return Err(storage::bad_request("Недопустимый курсор журнала"));
    }
    let root = folder(&state, &query.project_id, false)?;
    let status_file = status_path(&root, &query.id)?;
    let status = read(&status_file)?;
    if status["schema"] != SCHEMA
        || status["project_id"] != query.project_id
        || status["id"] != query.id
    {
        return Err(storage::bad_request("Журнал принадлежит другому запуску"));
    }
    let log_file = status_file.with_file_name("events.jsonl");
    let bytes = match fs::symlink_metadata(&log_file) {
        Ok(info) => {
            if info.file_type().is_symlink() || !info.is_file() || info.len() > MAX_LOG_BYTES {
                return Err(storage::bad_request("Журнал анализа повреждён"));
            }
            fs::read(&log_file).map_err(|e| storage::internal(format!("Read pipeline log: {e}")))?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(storage::internal(format!("Check pipeline log: {e}"))),
    };
    let mut entries = Vec::new();
    let mut total = 0usize;
    // An observer may read while the writer is flushing a final row.
    let complete_len = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    for row in bytes[..complete_len].split(|byte| *byte == b'\n') {
        if row.is_empty() {
            continue;
        }
        if total >= MAX_LOG_ENTRIES {
            return Err(storage::bad_request("Слишком длинный журнал"));
        }
        if total >= query.after && entries.len() < MAX_LOG_PAGE {
            let value = serde_json::from_slice::<Value>(row)
                .map_err(|_| storage::bad_request("Нарушена структура журнала"))?;
            entries.push(value);
        }
        total += 1;
    }
    let offset = query.after.min(total) + entries.len();
    Ok(Json(json!({
        "schema":"volley-studio-pipeline-log/1","project_id":query.project_id,
        "id":query.id,"entries":entries,"next_offset":offset,
        "has_more":offset<total
    })))
}

#[derive(Deserialize)]
pub struct HistoryQuery {
    project_id: String,
    #[serde(default)]
    offset: usize,
}

fn history_rows(state: &AppState, project_id: &str, offset: usize) -> Result<Value, ApiError> {
    if offset > 1500 {
        return Err(storage::bad_request("Недопустимая страница истории"));
    }
    let root = folder(state, project_id, false)?;
    if !root.exists() {
        return Ok(
            json!({"schema":"volley-studio-pipeline-history/1","project_id":project_id,"runs":[],"next_offset":0,"has_more":false}),
        );
    }
    let mut rows = Vec::new();
    let mut candidates = Vec::new();
    for entry in
        fs::read_dir(&root).map_err(|e| storage::internal(format!("List pipeline history: {e}")))?
    {
        let entry = entry.map_err(|e| storage::internal(format!("Read pipeline history: {e}")))?;
        let id = entry.file_name().to_string_lossy().into_owned();
        if !valid_id(&id) {
            continue;
        }
        let Ok(meta) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if meta.file_type().is_symlink() || !meta.is_dir() {
            continue;
        }
        let created = meta.created().or_else(|_| meta.modified()).ok();
        candidates.push((created, id));
        if candidates.len() > 1500 {
            return Err(storage::bad_request("Слишком много запусков анализа"));
        }
    }
    candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    let total = candidates.len();
    let next_offset = (offset + 25).min(total);
    for (_, id) in candidates.into_iter().skip(offset).take(25) {
        let Ok(path) = status_path(&root, &id) else {
            continue;
        };
        let Ok(status) = read(&path) else { continue };
        if status["schema"] != SCHEMA || status["project_id"] != project_id || status["id"] != id {
            continue;
        }
        let log_path = path.with_file_name("events.jsonl");
        let records: Vec<Value> = match fs::symlink_metadata(&log_path) {
            Ok(meta)
                if meta.is_file()
                    && !meta.file_type().is_symlink()
                    && meta.len() <= MAX_LOG_BYTES =>
            {
                fs::read(&log_path)
                    .ok()
                    .map(|bytes| {
                        let end = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
                        bytes[..end]
                            .split(|b| *b == b'\n')
                            .take(MAX_LOG_ENTRIES)
                            .filter_map(|line| serde_json::from_slice(line).ok())
                            .collect()
                    })
                    .unwrap_or_default()
            }
            _ => Vec::new(),
        };
        let started = records.first().and_then(|row| row["timestamp_ms"].as_u64());
        let finished = if ["complete", "failed", "cancelled"]
            .contains(&status["status"].as_str().unwrap_or(""))
        {
            records.last().and_then(|row| row["timestamp_ms"].as_u64())
        } else {
            None
        };
        let starts: Vec<_> = records
            .iter()
            .filter(|row| row["kind"] == "stage_start")
            .filter_map(|row| Some((row["stage"].as_str()?, row["timestamp_ms"].as_u64()?)))
            .collect();
        let stages: Vec<Value> = starts
            .iter()
            .enumerate()
            .map(|(index, (name, start))| {
                let end = starts.get(index + 1).map(|(_, time)| *time).or(finished);
                json!({"name":name,"duration_ms":end.and_then(|time| time.checked_sub(*start))})
            })
            .collect();
        rows.push(json!({"id":id,"video":status["video"],"mode":status["mode"],
            "status":status["status"],"stage":status["stage"],"error":status["error"],
            "started_ms":started,"duration_ms":started.zip(finished).and_then(|(a,b)| b.checked_sub(a)),
            "gpu_reused":status["gpu_reused"],"result_reused":status["result_reused"],
            "stages":stages}));
    }
    rows.sort_by(|a, b| {
        b["started_ms"]
            .as_u64()
            .cmp(&a["started_ms"].as_u64())
            .then_with(|| b["id"].as_str().cmp(&a["id"].as_str()))
    });
    let has_more = next_offset < total;
    Ok(
        json!({"schema":"volley-studio-pipeline-history/1","project_id":project_id,"runs":rows,
        "next_offset":next_offset,"has_more":has_more}),
    )
}

pub async fn history(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HistoryQuery>,
) -> ApiResult<Value> {
    let result =
        tokio::task::spawn_blocking(move || history_rows(&state, &query.project_id, query.offset))
            .await
            .map_err(|e| storage::internal(format!("Pipeline history worker: {e}")))??;
    let mut result = result;
    let project_id = result["project_id"].as_str().unwrap_or("").to_owned();
    let slot = manager().lock().await;
    if let Some(runs) = result["runs"].as_array_mut() {
        for run in runs {
            if ["queued", "running", "cancelling"].contains(&run["status"].as_str().unwrap_or(""))
                && !slot.as_ref().is_some_and(|active| {
                    Some(active.id.as_str()) == run["id"].as_str()
                        && active.project_id == project_id
                })
            {
                run["status"] = json!("interrupted");
            }
        }
    }
    Ok(Json(result))
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct GpuProfile {
    stride: usize,
    gpu_memory_limit_mb: u64,
    cpu_threads: usize,
    /// Run independent RTMPose GPU pose evidence for this fusion.
    pose_enabled: bool,
    /// Inter-frame stride for pose, independent from nine-frame Vball/RAVEL windows.
    pose_stride: usize,
}
impl Default for GpuProfile {
    fn default() -> Self {
        Self {
            stride: 9,
            gpu_memory_limit_mb: 0,
            cpu_threads: 8,
            pose_enabled: true,
            pose_stride: 6,
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct PostProfile {
    tracking: TrackingOptions,
    touches: TouchSettings,
    rallies: RallySettings,
    #[serde(default = "default_pose_classification")]
    pose_classification: bool,
}
fn default_pose_classification() -> bool {
    true
}
impl Default for PostProfile {
    fn default() -> Self {
        Self {
            tracking: TrackingOptions::default(),
            touches: TouchSettings::default(),
            rallies: RallySettings::default(),
            pose_classification: true,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum PipelineMode {
    #[default]
    Full,
    Gpu,
    Post,
}
impl PipelineMode {
    /// CPU-only runs are allowed to consume saved GPU evidence, never to start a model.
    const fn allows_gpu_execution(self) -> bool {
        matches!(self, Self::Full | Self::Gpu)
    }

    fn label(self) -> &'static str {
        match self {
            Self::Full => "Полный анализ",
            Self::Gpu => "Первичный GPU-анализ",
            Self::Post => "CPU-постобработка",
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StartRequest {
    project_id: String,
    video: String,
    #[serde(default)]
    gpu: GpuProfile,
    #[serde(default)]
    post: PostProfile,
    #[serde(default)]
    mode: PipelineMode,
    #[serde(default)]
    source_analysis_id: Option<String>,
    #[serde(default)]
    client_request_id: Option<String>,
}
#[derive(Deserialize)]
pub struct StatusQuery {
    project_id: String,
    id: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelRequest {
    project_id: String,
    id: String,
}
struct Running {
    project_id: String,
    id: String,
    cancel: Arc<AtomicBool>,
}
fn manager() -> &'static Mutex<Option<Running>> {
    static MANAGER: OnceLock<Mutex<Option<Running>>> = OnceLock::new();
    MANAGER.get_or_init(|| Mutex::new(None))
}
pub(crate) async fn has_active() -> bool {
    manager().lock().await.is_some()
}
fn reason(error: ApiError) -> String {
    error.1 .0["error"]
        .as_str()
        .unwrap_or("Ошибка нативной обработки")
        .to_owned()
}
pub(crate) use volley_core::contracts::job_id as valid_id;
fn folder(state: &AppState, project_id: &str, create: bool) -> Result<PathBuf, ApiError> {
    storage::project_id(project_id)?;
    projects::existing_project(state, project_id)?;
    let project = projects::project_root(state).join(project_id);
    cpu_retrack::safe_folder(&project).map_err(storage::bad_request)?;
    let root = project.join("native_studio_pipelines");
    match fs::symlink_metadata(&root) {
        Ok(info) => {
            if info.file_type().is_symlink() || !info.is_dir() {
                return Err(storage::bad_request("Каталог запусков небезопасен"));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && create => {
            fs::create_dir(&root)
                .map_err(|e| storage::internal(format!("Создание каталога запусков: {e}")))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(storage::internal(format!(
                "Проверка каталога запусков: {error}"
            )))
        }
    }
    Ok(root)
}
fn save(path: &Path, status: &Value) -> Result<(), String> {
    storage::write_json_atomic(path, status).map_err(|e| format!("Сохранение статуса: {e:?}"))
}
fn read(path: &Path) -> Result<Value, ApiError> {
    let meta =
        fs::symlink_metadata(path).map_err(|_| storage::not_found("Статус анализа не найден"))?;
    if meta.file_type().is_symlink() || !meta.is_file() || meta.len() > MAX_STATUS_BYTES {
        return Err(storage::bad_request("Файл статуса анализа повреждён"));
    }
    storage::read_json(path)?.ok_or_else(|| storage::not_found("Пустой статус анализа"))
}
fn status_path(root: &Path, id: &str) -> Result<PathBuf, ApiError> {
    if !valid_id(id) {
        return Err(storage::bad_request("Неверный идентификатор запуска"));
    }
    let run = root.join(id);
    cpu_retrack::safe_folder(&run).map_err(storage::bad_request)?;
    Ok(run.join("status.json"))
}
fn progress(path: &Path, status: &mut Value, stage: &str, percent: f64) -> Result<(), String> {
    let stage_name = stage.split_once(": ").map_or(stage, |(name, _)| name);
    let previous = status["stage"].as_str().unwrap_or("");
    let previous_name = previous.split_once(": ").map_or(previous, |(name, _)| name);
    if stage_name != previous_name {
        log_entry(
            path,
            "info",
            "stage_start",
            stage_name,
            "Начало этапа",
            None,
        )?;
    }
    status["status"] = json!("running");
    status["stage"] = json!(stage);
    status["progress_percent"] = json!(percent.clamp(0.0, 99.9));
    save(path, status)
}
fn require_id(response: &Value, field: &str) -> Result<String, String> {
    response[field]
        .as_str()
        .filter(|value| valid_id(value))
        .map(str::to_owned)
        .ok_or_else(|| format!("Этап не вернул корректный {field}"))
}
const MAX_SOURCE_MANIFEST: u64 = 32 * 1024;
const MAX_SOURCE_EVIDENCE: u64 = 64 * 1024 * 1024 * 1024;
const MAX_PIPELINES: usize = 1000;

/// Version branching is based on the entire GPU profile AND the immutable
/// prepared-media identity. A change confined to CPU settings only branches
/// under the existing fused GPU source. Metadata is never sufficient for
/// claims of valid raw evidence: CPU processors verify SHA-256 when reading.
#[derive(Default)]
struct ExistingVersions {
    gpu: Option<Value>,
    full: Option<Value>,
}
fn saved_stage(project: &Path, directory: &str, id: &str, file: &str) -> Option<Value> {
    if !valid_id(id) {
        return None;
    }
    let parent = project.join(directory);
    cpu_retrack::safe_folder(&parent).ok()?;
    let dir = parent.join(id);
    cpu_retrack::safe_folder(&dir).ok()?;
    let manifest_file = cpu_retrack::safe_file(&dir, "manifest.json", MAX_SOURCE_MANIFEST).ok()?;
    let manifest: Value = serde_json::from_slice(&fs::read(manifest_file).ok()?).ok()?;
    let evidence = cpu_retrack::safe_file(&dir, file, MAX_SOURCE_EVIDENCE).ok()?;
    if fs::metadata(evidence).ok()?.len() == 0 || !valid_sha(&manifest["sha256"]) {
        return None;
    }
    Some(manifest)
}
fn valid_sha(value: &Value) -> bool {
    value.as_str().is_some_and(volley_core::contracts::sha256)
}

fn gpu_lineage_valid(
    project: &Path,
    request: &StartRequest,
    identity: &Value,
    saved: &Value,
) -> bool {
    let Some(fusion_id) = saved["analysis_id"].as_str().filter(|id| valid_id(id)) else {
        return false;
    };
    let Some(ball_id) = saved["gpu_jobs"]["ball"].as_str().filter(|id| valid_id(id)) else {
        return false;
    };
    let Some(players_id) = saved["gpu_jobs"]["players"]
        .as_str()
        .filter(|id| valid_id(id))
    else {
        return false;
    };
    let Some(fusion) = saved_stage(project, "native_gpu_fusions", fusion_id, "frames.jsonl") else {
        return false;
    };
    if fusion["schema"] != "volley-native-gpu-fusion/1"
        || fusion["state"] != "raw_candidates_fused"
        || fusion["fusion_id"] != fusion_id
        || fusion["project_id"] != request.project_id
        || fusion["video"] != request.video
        || fusion["source_identity"] != *identity
        || fusion["sources"]["vballnet"]["job_id"] != ball_id
        || fusion["sources"]["ravel"]["job_id"] != players_id
    {
        return false;
    }
    for (id, model, source) in [
        (ball_id, "ball", "vballnet"),
        (players_id, "players", "ravel"),
    ] {
        let Some(raw) = saved_stage(project, "native_gpu_runs", id, "detections.jsonl") else {
            return false;
        };
        if !volley_core::contracts::gpu_manifest(
            &raw,
            &request.project_id,
            &request.video,
            id,
            model,
        ) || raw["source_identity"] != *identity
            || raw["stride"] != request.gpu.stride
            || raw["sha256"] != fusion["sources"][source]["sha256"]
        {
            return false;
        }
    }
    if request.gpu.pose_enabled {
        let Some(pose_id) = saved["gpu_jobs"]["pose"].as_str().filter(|id| valid_id(id)) else {
            return false;
        };
        let Some(pose) = saved_stage(project, "native_gpu_pose_runs", pose_id, "poses.jsonl")
        else {
            return false;
        };
        if pose["schema"] != "volley-native-gpu-pose/1"
            || pose["state"] != "pose_evidence_complete"
            || pose["pose_run_id"] != pose_id
            || pose["fusion_id"] != fusion_id
            || pose["model_sha256"] != volley_inference::pose::MODEL_SHA256
            || pose["stride"] != request.gpu.pose_stride
            || pose["project_id"] != request.project_id
            || pose["video"] != request.video
            || pose["source_identity"] != *identity
            || pose["source_fusion_sha256"] != fusion["sha256"]
        {
            return false;
        }
    }
    true
}
fn cpu_lineage_valid(project: &Path, request: &StartRequest, saved: &Value) -> bool {
    let (Some(fusion), Some(replay), Some(touch), Some(rally)) = (
        saved["analysis_id"].as_str().filter(|id| valid_id(id)),
        saved["cpu_runs"]["tracking"]
            .as_str()
            .filter(|id| valid_id(id)),
        saved["cpu_runs"]["touches"]
            .as_str()
            .filter(|id| valid_id(id)),
        saved["postprocessing_id"]
            .as_str()
            .filter(|id| valid_id(id)),
    ) else {
        return false;
    };
    if saved["cpu_runs"]["rallies"] != rally {
        return false;
    }
    let Some(track) = saved_stage(
        project,
        "native_cpu_replays",
        replay,
        "tracked_frames.jsonl",
    ) else {
        return false;
    };
    let Some(touches) = saved_stage(
        project,
        "native_cpu_touch_runs",
        touch,
        "touch_candidates.jsonl",
    ) else {
        return false;
    };
    let Some(rallies) = saved_stage(
        project,
        "native_cpu_rallies",
        rally,
        "rally_candidates.jsonl",
    ) else {
        return false;
    };
    track["schema"] == "volley-native-cpu-tracking/1"
        && track["state"] == "provisional_tracklets_rebuilt"
        && track["replay_id"] == replay
        && track["source_fusion_id"] == fusion
        && track["project_id"] == request.project_id
        && track["video"] == request.video
        && touches["schema"] == "volley-native-cpu-touch-proposals/1"
        && touches["state"] == "motion_gated_touch_proposals"
        && touches["touch_id"] == touch
        && touches["source_replay_id"] == replay
        && touches["project_id"] == request.project_id
        && touches["video"] == request.video
        && touches["court_revision"] == saved["court_revision"]
        && rallies["schema"] == "volley-native-cpu-rally-proposals/1"
        && rallies["state"] == "provisional_rally_segments"
        && rallies["court_contact_context_version"] == "python_serve_toss_possession_v1"
        && rallies["rally_run_id"] == rally
        && rallies["source_replay_id"] == replay
        && rallies["source_touch_id"] == touch
        && rallies["court_revision"] == touches["court_revision"]
        && rallies["project_id"] == request.project_id
        && rallies["video"] == request.video
}
fn compatible_version(saved: &Value, request: &StartRequest, identity: &Value) -> bool {
    saved["schema"] == SCHEMA && saved["project_id"] == request.project_id
        && saved["video"] == request.video
        && saved["profile"]["gpu"] == serde_json::to_value(&request.gpu).unwrap_or(Value::Null)
        // Older pipeline status files can predate the source_identity field;
        // the fusion and both GPU manifests are always validated separately.
        && (saved["source_identity"].is_null() || saved["source_identity"] == *identity)
}
fn find_reusable(
    state: &AppState,
    request: &StartRequest,
    identity: &Value,
    current_id: &str,
) -> ExistingVersions {
    let mut matches = ExistingVersions::default();
    let Ok(root) = folder(state, &request.project_id, false) else {
        return matches;
    };
    let Ok(directory) = fs::read_dir(&root) else {
        return matches;
    };
    let mut paths = Vec::new();
    for item in directory {
        let Ok(item) = item else {
            return ExistingVersions::default();
        };
        let id = item.file_name().to_string_lossy().into_owned();
        if !valid_id(&id) || id == current_id {
            continue;
        }
        if paths.len() >= MAX_PIPELINES {
            return ExistingVersions::default();
        }
        let modified = item.metadata().and_then(|m| m.modified()).ok();
        paths.push((modified, id));
    }
    paths.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    let project = projects::project_root(state).join(&request.project_id);
    // Real execution validates the court snapshot before attempting reuse.
    // In synthetic version tests, the optional source store is absent.
    let court_revision =
        crate::video_edits::court_snapshot(state, &request.project_id, &request.video)
            .ok()
            .flatten()
            .map(|court| court.revision);
    for (_, id) in paths {
        let Ok(file) = status_path(&root, &id) else {
            continue;
        };
        let Ok(saved) = read(&file) else {
            continue;
        };
        if saved["id"] != id
            || !compatible_version(&saved, request, identity)
            || !gpu_lineage_valid(&project, request, identity, &saved)
        {
            continue;
        }
        if saved["status"] == "complete"
            && saved["pipeline_complete"] == true
            && saved["profile"]["post"]
                == serde_json::to_value(&request.post).unwrap_or(Value::Null)
            && saved["court_revision"] == json!(court_revision)
            && cpu_lineage_valid(&project, request, &saved)
        {
            matches.full = Some(saved);
            return matches;
        }
        if matches.gpu.is_none() {
            matches.gpu = Some(saved);
        }
    }
    matches
}

/// Resolve a user-selected fusion against the valid GPU pipeline that
/// created it. CPU runs must never silently choose a different GPU source.
fn selected_gpu(
    state: &AppState,
    request: &StartRequest,
    identity: &Value,
    analysis_id: &str,
) -> Result<Value, String> {
    let root = folder(state, &request.project_id, false).map_err(reason)?;
    let project = projects::project_root(state).join(&request.project_id);
    if let Ok(entries) = fs::read_dir(&root) {
        let mut checked = 0_usize;
        for entry in entries {
            let entry = entry.map_err(|e| format!("Чтение GPU-версии: {e}"))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !valid_id(&name) {
                continue;
            }
            checked += 1;
            if checked > MAX_PIPELINES {
                return Err("Слишком много сохранённых GPU-версий".into());
            }
            let Ok(file) = status_path(&root, &name) else {
                continue;
            };
            let Ok(saved) = read(&file) else {
                continue;
            };
            if saved["schema"] != SCHEMA
                || saved["project_id"] != request.project_id
                || saved["video"] != request.video
                || saved["analysis_id"] != analysis_id
                || saved["source_identity"] != *identity
            {
                continue;
            }
            let Ok(gpu) = serde_json::from_value::<GpuProfile>(saved["profile"]["gpu"].clone())
            else {
                continue;
            };
            let mut scoped = request.clone();
            scoped.gpu = gpu;
            if gpu_lineage_valid(&project, &scoped, identity, &saved) {
                return Ok(saved);
            }
        }
    }
    // Fusion snapshots can also originate from the standalone native GPU API,
    // without a Studio pipeline status. Accept that exact source only after
    // validating every immutable GPU manifest and its SHA-256 references.
    let fusion = saved_stage(&project, "native_gpu_fusions", analysis_id, "frames.jsonl")
        .ok_or("Выбранный GPU-анализ не найден")?;
    if fusion["schema"] != "volley-native-gpu-fusion/1"
        || fusion["state"] != "raw_candidates_fused"
        || fusion["fusion_id"] != analysis_id
        || fusion["project_id"] != request.project_id
        || fusion["video"] != request.video
        || fusion["source_identity"] != *identity
        || fusion["fps"] != 30
    {
        return Err("Fusion не принадлежит выбранному видео или исходник изменился".into());
    }
    let ball = require_id(&fusion["sources"]["vballnet"], "job_id")?;
    let players = require_id(&fusion["sources"]["ravel"], "job_id")?;
    let mut stride = None;
    for (model, id, source) in [
        ("ball", ball.as_str(), "vballnet"),
        ("players", players.as_str(), "ravel"),
    ] {
        let raw = saved_stage(&project, "native_gpu_runs", id, "detections.jsonl")
            .ok_or("Не найден исходный GPU-результат для Fusion")?;
        let current_stride = raw["stride"]
            .as_u64()
            .filter(|s| (1..=9).contains(s))
            .ok_or("Неверный шаг исходного GPU-анализа")?;
        if !volley_core::contracts::gpu_manifest(
            &raw,
            &request.project_id,
            &request.video,
            id,
            model,
        ) || raw["source_identity"] != *identity
            || raw["sha256"] != fusion["sources"][source]["sha256"]
            || stride.is_some_and(|s| s != current_stride)
        {
            return Err("GPU-источник выбранной версии повреждён или не совпадает".into());
        }
        stride = Some(current_stride);
    }
    Ok(json!({
        "analysis_id": analysis_id,
        "gpu_jobs": {"ball":ball,"players":players},
        "id": analysis_id,
        // Standalone GPU manifests preserve stride, not thread/VRAM settings.
        "profile": {"gpu": {"stride":stride.unwrap_or(9),
            "gpu_memory_limit_mb":0,"cpu_threads":8}}
    }))
}

// GPU stage coordinates (progress window, status document, cancellation)
// stay explicit; a stage-context struct is planned for stage E.
#[expect(clippy::too_many_arguments)]
async fn gpu(
    app: &Arc<AppState>,
    request: &StartRequest,
    model: ModelKind,
    cancel: &AtomicBool,
    path: &Path,
    status: &mut Value,
    from: f64,
    share: f64,
) -> Result<String, String> {
    // Defense in depth: a CPU-post request must never reach native GPU model
    // startup, even if the caller's branching is later changed.
    if !request.mode.allows_gpu_execution() {
        return Err("CPU-постобработка не имеет права запускать GPU-модели".into());
    }
    if cancel.load(Ordering::Acquire) {
        return Err("cancelled".into());
    }
    let model_name = if model == ModelKind::Ball {
        "VballNet"
    } else {
        "RAVEL"
    };
    progress(path, status, &format!("GPU · {model_name}"), from)?;
    log_event(
        path,
        &format!("GPU · {model_name}"),
        "info",
        "Подготовка вызова нативной модели",
    )?;
    let payload = json!({
        "project_id": request.project_id,
        "video": request.video,
        "model": model,
        "device": "cuda",
        "gpu_device_id": 0,
        "stride": request.gpu.stride,
        "gpu_memory_limit_mb": request.gpu.gpu_memory_limit_mb,
        "cpu_threads": request.gpu.cpu_threads
    });
    let config: inference_full::StartRequest = serde_json::from_value(payload.clone())
        .map_err(|e| format!("Неверный GPU-профиль: {e}"))?;
    log_native(
        path,
        &format!("GPU · {model_name}"),
        "inference_full::start",
        payload,
    )?;
    let Json(start) = inference_full::start(State(app.clone()), HeaderMap::new(), Json(config))
        .await
        .map_err(reason)?;
    let id = require_id(&start, "job_id")?;
    let mut last_phase = String::new();
    let mut last_bucket = -1i32;
    crate::gpu_poll::wait(
        app,
        &request.project_id,
        &id,
        || cancel.load(Ordering::Acquire),
        |gpu_status| {
            status["current_gpu_job_id"] = json!(id);
            let phase = gpu_status["phase"].as_str().unwrap_or("unknown");
            let pct = gpu_status["progress_percent"].as_f64().unwrap_or(0.0);
            let bucket = (pct.clamp(0.0, 100.0) / 10.0).floor() as i32;
            if phase != last_phase || bucket > last_bucket {
                log_event(
                    path,
                    &format!("GPU · {model_name}"),
                    "info",
                    &format!("job_id={id} · phase={phase} · прогресс={pct:.1}%"),
                )?;
                last_phase = phase.to_owned();
                last_bucket = bucket;
            }
            if phase == "complete" {
                status["current_gpu_job_id"] = Value::Null;
            }
            progress(
                path,
                status,
                &format!("GPU · {model_name}: {phase}"),
                from + share * pct.clamp(0.0, 100.0) / 100.0,
            )
        },
    )
    .await
}

async fn cpu<T: Send + 'static>(
    app: Arc<AppState>,
    request: T,
    handler: fn(&AppState, &T) -> Result<Value, ApiError>,
) -> Result<Value, String> {
    tokio::task::spawn_blocking(move || handler(&app, &request))
        .await
        .map_err(|e| format!("CPU worker: {e}"))?
        .map_err(reason)
}
async fn pipeline(
    app: &Arc<AppState>,
    request: &StartRequest,
    cancel: &AtomicBool,
    path: &Path,
    status: &mut Value,
) -> Result<(), String> {
    // Preparation is part of the one-click workflow. Reuses only the canonical
    // content-addressed prepared video, never old inference or CPU snapshots.
    log_native(
        path,
        "Подготовка видео",
        "preparation::plan_for",
        json!({"project_id":request.project_id,"video":request.video}),
    )?;
    let plan = preparation::plan_for_async(app.clone(), &request.project_id, &request.video)
        .await
        .map_err(reason)?;
    progress(path, status, "Подготовка исходного видео", 0.0)?;
    let cached = preparation::ready_path_async(&plan).await?;
    log_event(
        path,
        "Подготовка видео",
        "info",
        if cached.is_some() {
            "Подготовленное видео уже в кэше; FFmpeg не запускается"
        } else {
            "Необходима подготовка исходного видео"
        },
    )?;
    if cached.is_none() {
        log_native(
            path,
            "Подготовка видео",
            "preparation::prepare_standalone",
            json!({"project_id":request.project_id,"video":request.video,"output":plan.output.display().to_string()}),
        )?;
        let (tx, rx) = tokio::sync::watch::channel(false);
        let preparation_progress = Arc::new(Mutex::new(preparation::Progress::queued()));
        let mut ffmpeg_logged = false;
        let mut last_preparation_bucket = -1i32;
        let worker =
            preparation::prepare_standalone(app.clone(), plan, rx, preparation_progress.clone());
        tokio::pin!(worker);
        loop {
            tokio::select! {
                result = &mut worker => {
                    let completed = preparation_progress.lock().await;
                    if !ffmpeg_logged {
                        if let Some(argv)=&completed.process_argv {
                            log_entry(path,"info","invocation","Подготовка видео","Вызов Command::spawn (FFmpeg)",
                                Some(json!({"type":"process","name":"ffmpeg","argv":argv})))?;
                        }
                    }
                    drop(completed);
                    result?;
                    log_event(path,"Подготовка видео","success","FFmpeg завершён; кэш подготовлен")?;
                    break;
                }
                _ = tokio::time::sleep(Duration::from_millis(600)) => {
                    if cancel.load(Ordering::Acquire) {
                        let _ = tx.send(true);
                    }
                    let current = preparation_progress.lock().await;
                    if !ffmpeg_logged {
                        if let Some(argv)=&current.process_argv {
                            log_entry(path,"info","invocation","Подготовка видео","Вызов Command::spawn (FFmpeg)",
                                Some(json!({"type":"process","name":"ffmpeg","argv":argv})))?;
                            ffmpeg_logged=true;
                        }
                    }
                    let bucket=(current.progress.clamp(0.0,100.0)/10.0).floor() as i32;
                    if bucket>last_preparation_bucket {
                        last_preparation_bucket=bucket;
                        log_event(path,"Подготовка видео","info",&format!("FFmpeg: {} · {:.1}%",current.state,current.progress))?;
                    }
                    progress(path, status, "Подготовка исходного видео",
                        (current.progress * 0.08).clamp(0.0, 8.0))?;
                }
            }
        }
    }
    if cancel.load(Ordering::Acquire) {
        return Err("cancelled".into());
    }
    progress(path, status, "Видео подготовлено", 8.0)?;
    let ready = preparation::plan_for_async(app.clone(), &request.project_id, &request.video)
        .await
        .map_err(reason)?;
    let prepared = preparation::ready_path_async(&ready)
        .await?
        .ok_or("Подготовленное видео недоступно")?;
    let (bytes, mtime_ns) = crate::inference_stream::source_stamp(&prepared)?;
    let identity = json!({"bytes":bytes,"mtime_ns":mtime_ns});
    status["source_identity"] = identity.clone();
    let court_revision =
        crate::video_edits::court_snapshot(app, &request.project_id, &request.video)
            .map_err(reason)?
            .map(|court| court.revision);
    status["court_revision"] = json!(court_revision);
    save(path, status)?;
    let current_id = status["id"].as_str().unwrap_or_default();
    log_event(
        path,
        "Выбор версии",
        "info",
        "Проверка GPU/CPU профилей, подготовленного видео и подписанных идентификаторов источника",
    )?;
    let reusable = if request.mode == PipelineMode::Full {
        find_reusable(app, request, &identity, current_id)
    } else {
        ExistingVersions::default()
    };
    if request.mode == PipelineMode::Post {
        let source_id = request
            .source_analysis_id
            .as_deref()
            .ok_or("Не выбран первичный GPU-анализ в верхней панели")?;
        let source = selected_gpu(app, request, &identity, source_id)?;
        status["analysis_id"] = source["analysis_id"].clone();
        status["gpu_jobs"] = source["gpu_jobs"].clone();
        status["profile"]["gpu"] = source["profile"]["gpu"].clone();
        status["gpu_reused"] = json!(true);
        status["reused_from_id"] = source["id"].clone();
        save(path, status)?;
        log_event(
            path,
            "CPU",
            "info",
            &format!("Создаётся новая CPU-версия для выбранного fusion_id={source_id}"),
        )?;
        progress(
            path,
            status,
            "Выбран сохранённый GPU-анализ · новый CPU-пересчёт",
            82.0,
        )?;
    } else if let Some(existing) = reusable.full {
        status["analysis_id"] = existing["analysis_id"].clone();
        status["gpu_jobs"] = existing["gpu_jobs"].clone();
        status["cpu_runs"] = existing["cpu_runs"].clone();
        status["postprocessing_id"] = existing["postprocessing_id"].clone();
        status["gpu_reused"] = json!(true);
        status["result_reused"] = json!(true);
        status["reused_from_id"] = existing["id"].clone();
        log_event(
            path,
            "Повторное использование",
            "success",
            &format!(
                "Настройки совпали: GPU и CPU результатов повторно использованы из запуска {}",
                existing["id"].as_str().unwrap_or("?")
            ),
        )?;
        progress(
            path,
            status,
            "Настройки совпали: выбраны сохранённые версии GPU и CPU",
            99.9,
        )?;
        return Ok(());
    }
    if request.mode.allows_gpu_execution() {
        if let Some(existing) = reusable.gpu {
            status["analysis_id"] = existing["analysis_id"].clone();
            status["gpu_jobs"] = existing["gpu_jobs"].clone();
            status["gpu_reused"] = json!(true);
            status["reused_from_id"] = existing["id"].clone();
            log_event(
                path,
                "Повторное использование GPU",
                "info",
                &format!(
                    "GPU источник из запуска {} сохранён; запускаются только новые стадии CPU",
                    existing["id"].as_str().unwrap_or("?")
                ),
            )?;
            progress(
                path,
                status,
                "Сохранённый GPU-анализ найден · новая CPU-постобработка",
                82.0,
            )?;
        } else {
            log_event(
                path,
                "Первичный анализ",
                "info",
                "Совпадающего GPU-профиля нет, запуск двух нативных моделей",
            )?;
            let ball = gpu(
                app,
                request,
                ModelKind::Ball,
                cancel,
                path,
                status,
                8.0,
                36.0,
            )
            .await?;
            status["gpu_jobs"]["ball"] = json!(ball);
            save(path, status)?;
            let players = gpu(
                app,
                request,
                ModelKind::Players,
                cancel,
                path,
                status,
                44.0,
                36.0,
            )
            .await?;
            status["gpu_jobs"]["players"] = json!(players);
            save(path, status)?;
            if cancel.load(Ordering::Acquire) {
                return Err("cancelled".into());
            }

            progress(
                path,
                status,
                "Постобработка · объединение VballNet и RAVEL",
                82.0,
            )?;
            log_native(
                path,
                "Объединение GPU",
                "inference_fusion::do_fusion",
                json!({"project_id":request.project_id,"video":request.video,"vball_job_id":ball,"ravel_job_id":players}),
            )?;
            let fusion = cpu(
                app.clone(),
                inference_fusion::FuseRequest {
                    project_id: request.project_id.clone(),
                    video: request.video.clone(),
                    vball_job_id: ball,
                    ravel_job_id: players,
                },
                inference_fusion::do_fusion,
            )
            .await?;
            let fusion_id = require_id(&fusion, "fusion_id")?;
            status["analysis_id"] = json!(fusion_id);
            save(path, status)?;
            log_event(
                path,
                "Объединение GPU",
                "success",
                &format!("GPU объединён: fusion_id={fusion_id}"),
            )?;
            if cancel.load(Ordering::Acquire) {
                return Err("cancelled".into());
            }
        }
    }

    // RTMPose is a separate GPU evidence stage. It uses the immutable fusion
    // source, never Track IDs, and is intentionally never executed in Post mode.
    if request.mode.allows_gpu_execution()
        && request.gpu.pose_enabled
        && status["gpu_jobs"]["pose"].as_str().is_none()
    {
        if cancel.load(Ordering::Acquire) {
            return Err("cancelled".into());
        }
        let fusion_id = require_id(status, "analysis_id")?;
        progress(path, status, "GPU · RTMPose-M: позы игроков", 82.0)?;
        log_native(
            path,
            "GPU · RTMPose-M",
            "pose_inference::process",
            json!({"fusion_id":fusion_id,"model":"RTMPose-M-Body7","stride":request.gpu.pose_stride,
                    "threads":request.gpu.cpu_threads,"gpu_memory_limit_mb":request.gpu.gpu_memory_limit_mb}),
        )?;
        let _permit = app
            .inference_limit
            .acquire()
            .await
            .map_err(|e| format!("RTMPose inference limit: {e}"))?;
        let pose = tokio::task::block_in_place(|| {
            crate::pose_inference::process(
                app,
                &request.project_id,
                &request.video,
                &fusion_id,
                request.gpu.pose_stride,
                request.gpu.gpu_memory_limit_mb,
                request.gpu.cpu_threads,
                cancel,
                |p| {
                    progress(
                        path,
                        status,
                        "GPU · RTMPose-M: позы игроков",
                        82.0 + p * 2.5,
                    )
                },
            )
        })?;
        status["gpu_jobs"]["pose"] = pose["pose_run_id"].clone();
        save(path, status)?;
        log_event(
            path,
            "GPU · RTMPose-M",
            "success",
            &format!(
                "Позы сохранены: samples={}, players={}",
                pose["sampled_frames"], pose["player_poses"]
            ),
        )?;
    }
    if request.mode == PipelineMode::Gpu {
        progress(path, status, "Первичный GPU-анализ завершён", 99.9)?;
        return Ok(());
    }
    let fusion_id = require_id(status, "analysis_id")?;
    progress(path, status, "Постобработка · Track ID", 86.0)?;
    log_native(
        path,
        "Track ID",
        "cpu_retrack::process",
        json!({"project_id":request.project_id,"video":request.video,"fusion_id":fusion_id,"settings":request.post.tracking}),
    )?;
    let replay = cpu(
        app.clone(),
        cpu_retrack::ReplayRequest {
            project_id: request.project_id.clone(),
            video: request.video.clone(),
            fusion_id,
            settings: Some(request.post.tracking),
        },
        cpu_retrack::process,
    )
    .await?;
    let replay_id = require_id(&replay, "replay_id")?;
    status["cpu_runs"]["tracking"] = json!(replay_id);
    save(path, status)?;
    log_event(
        path,
        "Track ID",
        "success",
        &format!(
            "Трекинг завершён: replay_id={replay_id} · ID подтверждено={} ·              коротких ID отброшено={} · обнаружений отброшено={}",
            replay["provisional_track_count"].as_u64().unwrap_or(0),
            replay["discarded_short_track_ids"].as_u64().unwrap_or(0),
            replay["discarded_short_track_detections"].as_u64().unwrap_or(0),
        ),
    )?;
    if cancel.load(Ordering::Acquire) {
        return Err("cancelled".into());
    }

    progress(path, status, "Постобработка · физические касания", 91.0)?;
    log_native(
        path,
        "Касания",
        "cpu_touches::process",
        json!({"project_id":request.project_id,"video":request.video,"replay_id":replay_id,"settings":request.post.touches}),
    )?;
    let touches = cpu(
        app.clone(),
        cpu_touches::TouchRequest {
            project_id: request.project_id.clone(),
            video: request.video.clone(),
            replay_id: replay_id.clone(),
            settings: Some(request.post.touches),
            pose_classification: request.post.pose_classification,
        },
        cpu_touches::process,
    )
    .await?;
    let touch_id = require_id(&touches, "touch_id")?;
    if touches["court_revision"] != status["court_revision"] {
        return Err("Court calibration changed during CPU processing".into());
    }
    status["cpu_runs"]["touches"] = json!(touch_id);
    save(path, status)?;
    log_event(
        path,
        "Касания",
        "success",
        &format!("Касания сохранены: touch_id={touch_id}"),
    )?;
    if cancel.load(Ordering::Acquire) {
        return Err("cancelled".into());
    }

    progress(path, status, "Постобработка · розыгрыши", 96.0)?;
    log_native(
        path,
        "Розыгрыши",
        "cpu_rallies::process",
        json!({"project_id":request.project_id,"video":request.video,"replay_id":replay_id,"touch_id":touch_id,"settings":request.post.rallies}),
    )?;
    let rallies = cpu(
        app.clone(),
        cpu_rallies::RallyRequest {
            project_id: request.project_id.clone(),
            video: request.video.clone(),
            replay_id,
            touch_id,
            settings: Some(request.post.rallies),
        },
        cpu_rallies::process,
    )
    .await?;
    let rally_id = require_id(&rallies, "rally_run_id")?;
    status["cpu_runs"]["rallies"] = json!(rally_id);
    status["postprocessing_id"] = json!(rally_id);
    save(path, status)?;
    log_event(
        path,
        "Розыгрыши",
        "success",
        &format!("CPU-постобработка завершена: rally_run_id={rally_id}"),
    )?;
    if cancel.load(Ordering::Acquire) {
        return Err("cancelled".into());
    }
    Ok(())
}
async fn perform(
    state: Arc<AppState>,
    request: StartRequest,
    id: String,
    cancel: Arc<AtomicBool>,
    path: PathBuf,
) {
    let mut status = match read(&path) {
        Ok(v) => v,
        Err(_) => return,
    };
    let _ = log_event(
        &path,
        request.mode.label(),
        "info",
        "Запуск создан, настройки и выбранный источник сохранены.",
    );
    let outcome = pipeline(&state, &request, &cancel, &path, &mut status).await;
    match outcome {
        Ok(()) => {
            status["status"] = json!("complete");
            status["stage"] = json!(match request.mode {
                PipelineMode::Full => "GPU и CPU завершены",
                PipelineMode::Gpu => "Первичный GPU-анализ завершён",
                PipelineMode::Post => "Новая CPU-постобработка завершена",
            });
            status["progress_percent"] = json!(100.0);
            status["pipeline_complete"] = json!(request.mode != PipelineMode::Gpu);
            let _ = log_event(
                &path,
                request.mode.label(),
                "success",
                if status["result_reused"] == true {
                    "Готовый GPU+CPU результат повторно использован"
                } else {
                    match request.mode {
                        PipelineMode::Full => "Все стадии полного анализа успешно завершены",
                        PipelineMode::Gpu => "Новый GPU-анализ и слияние сохранены",
                        PipelineMode::Post => "Новая CPU-версия сохранена без запуска GPU",
                    }
                },
            );
        }
        Err(error) => {
            let cancelled = error == "cancelled" || cancel.load(Ordering::Acquire);
            status["status"] = json!(if cancelled { "cancelled" } else { "failed" });
            status["stage"] = json!(if cancelled {
                "Обработка отменена"
            } else {
                "Ошибка анализа"
            });
            status["error"] = if cancelled { Value::Null } else { json!(error) };
            status["pipeline_complete"] = json!(false);
            let _ = log_event(
                &path,
                request.mode.label(),
                if cancelled { "warn" } else { "error" },
                if cancelled {
                    "Анализ отменён пользователем"
                } else {
                    &error
                },
            );
        }
    }
    status["current_gpu_job_id"] = Value::Null;
    let _ = save(&path, &status);
    let mut slot = manager().lock().await;
    if slot.as_ref().is_some_and(|current| current.id == id) {
        *slot = None;
    }
}

pub async fn start(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<StartRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    if !state.writes_enabled {
        return Err(storage::conflict(
            "Запуск анализа возможен только в рабочем хранилище",
        ));
    }
    storage::project_id(&request.project_id)?;
    storage::video_name(&request.video)?;
    if request.mode == PipelineMode::Post
        && request
            .source_analysis_id
            .as_deref()
            .is_none_or(|id| !valid_id(id))
    {
        return Err(storage::bad_request(
            "Для CPU-постобработки выберите GPU-анализ в шапке",
        ));
    }
    if request.mode != PipelineMode::Post && request.source_analysis_id.is_some() {
        return Err(storage::bad_request(
            "GPU-источник указывается только при CPU-пересчёте",
        ));
    }
    if request
        .client_request_id
        .as_deref()
        .is_some_and(|id| !valid_id(id))
    {
        return Err(storage::bad_request("Некорректный идентификатор запроса"));
    }
    if !(1..=9).contains(&request.gpu.stride)
        || !(1..=128).contains(&request.gpu.cpu_threads)
        || request.gpu.gpu_memory_limit_mb > 131072
        || !(1..=9).contains(&request.gpu.pose_stride)
    {
        return Err(storage::bad_request(
            "GPU-профиль вне допустимых диапазонов",
        ));
    }
    request
        .post
        .tracking
        .validate()
        .map_err(storage::bad_request)?;
    request
        .post
        .touches
        .validate()
        .map_err(storage::bad_request)?;
    request
        .post
        .rallies
        .validate()
        .map_err(storage::bad_request)?;
    projects::existing_project(&state, &request.project_id)?;
    let _plan =
        preparation::plan_for_async(state.clone(), &request.project_id, &request.video).await?;
    let mut slot = manager().lock().await;
    // If the HTTP response was lost after scheduling, replaying the same
    // client token must recover the saved run rather than launch a duplicate.
    if let Some(token) = request.client_request_id.as_deref() {
        let existing_root = folder(&state, &request.project_id, false)?;
        let latest_path = existing_root.join("latest.json");
        if latest_path.exists() {
            let latest = read(&latest_path)?;
            if let Some(previous_id) = latest["id"].as_str().filter(|id| valid_id(id)) {
                let previous = read(&status_path(&existing_root, previous_id)?)?;
                if previous["client_request_id"] == token {
                    if previous["schema"] != SCHEMA
                        || previous["project_id"] != request.project_id
                        || previous["video"] != request.video
                        || previous["mode"] != json!(request.mode)
                        || previous["source_analysis_id"] != json!(request.source_analysis_id)
                        || previous["profile"]["gpu"] != json!(request.gpu)
                        || previous["profile"]["post"] != json!(request.post)
                    {
                        return Err(storage::conflict("Идентификатор запроса уже использован"));
                    }
                    return Ok(Json(json!({"schema":SCHEMA,"id":previous_id,
                        "status":previous["status"],"recovered":true})));
                }
            }
        }
    }
    if slot.is_some() {
        return Err(storage::conflict("Другой запуск анализа уже выполняется"));
    }
    if request.mode != PipelineMode::Post
        && (state.full_inference.has_active().await || state.quick_reel.has_active().await)
    {
        return Err(storage::conflict(
            "GPU занят отдельным запуском или быстрым рилсом",
        ));
    }
    let _guard = project_guard::write_guard(&state, &request.project_id)?;
    let root = folder(&state, &request.project_id, true)?;
    let id = Uuid::new_v4().simple().to_string();
    let dir = root.join(&id);
    fs::create_dir(&dir).map_err(|e| storage::internal(format!("Создание запуска: {e}")))?;
    let path = dir.join("status.json");
    let status = json!({
        "schema":SCHEMA,"id":id,"project_id":request.project_id,"video":request.video,
        "status":"queued","stage":"В очереди","progress_percent":0.0,
        "profile":{"gpu":request.gpu,"post":request.post},
        "mode":request.mode,"source_analysis_id":request.source_analysis_id,
        "client_request_id":request.client_request_id,
        "gpu_jobs":{},"cpu_runs":{},"analysis_id":null,
        "postprocessing_id":null,"current_gpu_job_id":null,
        "source_identity":null,"gpu_reused":false,"result_reused":false,
        "reused_from_id":null,"pipeline_complete":false,"error":null
    });
    save(&path, &status).map_err(storage::internal)?;
    log_native(
        &path,
        "Запуск",
        "studio_pipeline::start",
        json!({
            "project_id":request.project_id,"video":request.video,
            "gpu":request.gpu,"post":request.post,"mode":request.mode,
            "source_analysis_id":request.source_analysis_id,
            "client_request_id":request.client_request_id
        }),
    )
    .map_err(storage::internal)?;
    save(&root.join("latest.json"), &json!({"id":id})).map_err(storage::internal)?;
    let cancelled = Arc::new(AtomicBool::new(false));
    *slot = Some(Running {
        project_id: request.project_id.clone(),
        id: id.clone(),
        cancel: cancelled.clone(),
    });
    drop(_guard);
    tokio::spawn(perform(state, request, id.clone(), cancelled, path));
    Ok(Json(json!({"schema":SCHEMA,"id":id,"status":"queued"})))
}
pub async fn status(
    State(state): State<Arc<AppState>>,
    Query(query): Query<StatusQuery>,
) -> ApiResult<Value> {
    let root = folder(&state, &query.project_id, false)?;
    let id = match query.id {
        Some(id) => id,
        None => {
            if !root.join("latest.json").exists() {
                return Ok(Json(json!({"schema":SCHEMA,"status":"idle",
                    "project_id":query.project_id})));
            }
            let latest = read(&root.join("latest.json"))?;
            latest["id"]
                .as_str()
                .filter(|id| valid_id(id))
                .ok_or_else(|| storage::bad_request("Неверный указатель на последний запуск"))?
                .to_owned()
        }
    };
    let mut result = read(&status_path(&root, &id)?)?;
    if result["schema"] != SCHEMA || result["project_id"] != query.project_id || result["id"] != id
    {
        return Err(storage::bad_request("Статус принадлежит другому проекту"));
    }
    if result["status"] == "running" || result["status"] == "queued" {
        let slot = manager().lock().await;
        match slot
            .as_ref()
            .filter(|entry| entry.id == id && entry.project_id == query.project_id)
        {
            Some(entry) if entry.cancel.load(Ordering::Acquire) => {
                result["status"] = json!("cancelling");
                result["stage"] = json!("Отмена запрошена · остановка текущей операции");
                result["cancel_requested"] = json!(true);
            }
            Some(_) => {}
            None => {
                result["status"] = json!("interrupted");
                result["stage"] = json!("Сервер перезапущен");
                result["error"] = json!("Анализ не завершён до перезапуска сервера");
            }
        }
    }
    Ok(Json(result))
}
pub async fn cancel(
    State(_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<CancelRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    storage::project_id(&request.project_id)?;
    if !valid_id(&request.id) {
        return Err(storage::bad_request("Неверный ID анализа"));
    }
    let slot = manager().lock().await;
    let running = slot
        .as_ref()
        .filter(|entry| entry.project_id == request.project_id && entry.id == request.id)
        .ok_or_else(|| storage::not_found("Нет активного запуска для отмены"))?;
    running.cancel.store(true, Ordering::Release);
    drop(slot);
    // Promptly signal the active GPU job; never wait for the pipeline's next
    // poll cycle before forwarding a user cancellation.
    let path = folder(&_state, &request.project_id, false)?
        .join(&request.id)
        .join("status.json");
    let gpu_job_id = read(&path).ok().and_then(|snapshot| {
        snapshot["current_gpu_job_id"]
            .as_str()
            .filter(|id| valid_id(id))
            .map(str::to_owned)
    });
    let mut gpu_notified = false;
    if let Some(job_id) = gpu_job_id {
        let payload = json!({"project_id":request.project_id,"job_id":job_id});
        if let Ok(config) = serde_json::from_value::<inference_full::CancelRequest>(payload) {
            gpu_notified =
                inference_full::cancel(State(_state.clone()), HeaderMap::new(), Json(config))
                    .await
                    .is_ok();
        }
    }
    // The acknowledgement and UI must not depend on journal writes.
    let _ = log_event(&path, "Анализ", "warn", "Получен запрос на отмену");
    Ok(Json(json!({"id":request.id,"cancel_requested":true,
        "gpu_cancel_forwarded":gpu_notified})))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn progress_records_stage_transitions_for_history() {
        let dir = std::env::temp_dir().join(format!("studio-history-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("status.json");
        let mut status = json!({"stage":"В очереди"});
        progress(&path, &mut status, "GPU", 1.0).unwrap();
        let log = fs::read_to_string(dir.join("events.jsonl")).unwrap_or_default();
        let records: Vec<Value> = log
            .lines()
            .map(|row| serde_json::from_str(row).unwrap())
            .collect();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["kind"], "stage_start");
        assert_eq!(records[0]["stage"], "GPU");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stage_modes_reject_unknown_values_and_default_to_full() {
        let request: StartRequest = serde_json::from_value(json!({
            "project_id":"abc123def456","video":"match.mp4"
        }))
        .unwrap();
        assert_eq!(request.mode, PipelineMode::Full);
        assert!(request.mode.allows_gpu_execution());
        for mode in ["gpu", "post"] {
            let request: StartRequest = serde_json::from_value(json!({
                "project_id":"abc123def456", "video":"match.mp4", "mode":mode
            }))
            .unwrap();
            assert_eq!(request.mode.allows_gpu_execution(), mode == "gpu");
            assert_eq!(
                request.mode.label(),
                if mode == "gpu" {
                    "Первичный GPU-анализ"
                } else {
                    "CPU-постобработка"
                }
            );
        }
        assert!(serde_json::from_value::<StartRequest>(json!({
            "project_id":"abc123def456","video":"match.mp4","mode":"unknown"
        }))
        .is_err());
    }

    #[test]
    fn profiles_have_golden_defaults_and_reject_invalid_cpu_thresholds() {
        let profile: StartRequest = serde_json::from_value(json!({
            "project_id":"abc123def456","video":"match.mp4"
        }))
        .unwrap();
        assert_eq!(profile.gpu.stride, 9);
        assert_eq!(profile.gpu.cpu_threads, 8);
        assert_eq!(profile.post.tracking.max_gap_frames, 90);
        assert_eq!(profile.post.touches.threshold, 0.5);
        assert_eq!(profile.post.rallies.min_rally_frames, 45);
        assert!(profile.post.tracking.validate().is_ok());
        assert!(profile.post.touches.validate().is_ok());
        assert!(profile.post.rallies.validate().is_ok());
        let bad: StartRequest = serde_json::from_value(json!({
            "project_id":"abc123def456","video":"match.mp4",
            "post":{"tracking":{
                "min_player_score":0.30,"nms_iou":0.88,"max_gap_frames":500,
                "min_assignment_score":0.48,"ambiguity_margin":0.11
            }}
        }))
        .unwrap();
        assert!(bad.post.tracking.validate().is_err());
    }
}

#[cfg(test)]
mod branching_tests {
    use super::*;

    fn emit(project: &Path, directory: &str, id: &str, filename: &str, data: &Value) {
        let folder = project.join(directory).join(id);
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join(filename), b"example evidence").unwrap();
        storage::write_json_atomic(&folder.join("manifest.json"), data).unwrap();
    }

    #[tokio::test]
    async fn gpu_settings_and_source_are_branch_keys_while_cpu_settings_are_subbranches() {
        let base = std::env::temp_dir().join(format!("studio-branches-{}", Uuid::new_v4()));
        fs::create_dir_all(&base).unwrap();
        let project_dir = base.join("projects").join("abc123def456");
        fs::create_dir_all(&project_dir).unwrap();
        storage::write_json_atomic(
            &base.join("projects").join("projects.json"),
            &json!({"schema":1,"projects":[{"id":"abc123def456","name":"Test",
                "videos":["match.mp4"]}]}),
        )
        .unwrap();
        let gpu_ball = "a".repeat(32);
        let gpu_players = "b".repeat(32);
        let analysis = "c".repeat(32);
        let replay = "d".repeat(32);
        let touch = "e".repeat(32);
        let rally = "f".repeat(32);
        let identity = json!({"bytes":2048,"mtime_ns":123456789u64});
        let mut request: StartRequest = serde_json::from_value(json!({
            "project_id":"abc123def456","video":"match.mp4"
        }))
        .unwrap();
        // The historic fixture contains no RTMPose run, so its GPU profile
        // must explicitly disable the newly introduced pose stage.
        request.gpu.pose_enabled = false;
        let sha = "a".repeat(64);
        for (model, id) in [("ball", &gpu_ball), ("players", &gpu_players)] {
            emit(
                &project_dir,
                "native_gpu_runs",
                id,
                "detections.jsonl",
                &json!({
                    "schema":"volley-native-gpu-model-detections/1",
                    "status":"model_raw_detections_complete","job_id":id,
                    "project_id":request.project_id,"video":request.video,
                    "model":model,"stride":9,"source_identity":identity,
                    "raw_candidates_only":true,"studio_gpu_run_complete":false,
                    "file":"detections.jsonl","fps":30,
                    "sha256":sha
                }),
            );
        }
        emit(
            &project_dir,
            "native_gpu_fusions",
            &analysis,
            "frames.jsonl",
            &json!({
                "schema":"volley-native-gpu-fusion/1",
                "state":"raw_candidates_fused","fusion_id":analysis,
                "project_id":request.project_id,"video":request.video,
                "sources":{"vballnet":{"job_id":gpu_ball,"sha256":sha},
                           "ravel":{"job_id":gpu_players,"sha256":sha}},
                "source_identity":identity,"sha256":sha,"fps":30
            }),
        );
        emit(
            &project_dir,
            "native_cpu_replays",
            &replay,
            "tracked_frames.jsonl",
            &json!({
                "schema":"volley-native-cpu-tracking/1","state":"provisional_tracklets_rebuilt","replay_id":replay,
                "project_id":request.project_id,"video":request.video,
                "source_fusion_id":analysis,"sha256":sha
            }),
        );
        emit(
            &project_dir,
            "native_cpu_touch_runs",
            &touch,
            "touch_candidates.jsonl",
            &json!({
                "schema":"volley-native-cpu-touch-proposals/1","state":"motion_gated_touch_proposals","touch_id":touch,
                "project_id":request.project_id,"video":request.video,
                "source_replay_id":replay,"sha256":sha
            }),
        );
        emit(
            &project_dir,
            "native_cpu_rallies",
            &rally,
            "rally_candidates.jsonl",
            &json!({
                "schema":"volley-native-cpu-rally-proposals/1","state":"provisional_rally_segments","rally_run_id":rally,
                "court_contact_context_version":"python_serve_toss_possession_v1",
                "project_id":request.project_id,"video":request.video,
                "source_replay_id":replay,"source_touch_id":touch,"sha256":sha
            }),
        );
        let saved = json!({
            "schema":SCHEMA,"project_id":request.project_id,"video":request.video,
            "status":"complete","pipeline_complete":true,"source_identity":identity,
            "profile":{"gpu":request.gpu,"post":request.post},
            "gpu_jobs":{"ball":gpu_ball,"players":gpu_players},
            "analysis_id":analysis,"cpu_runs":{"tracking":replay,"touches":touch,"rallies":rally},
            "postprocessing_id":rally
        });
        let mut saved = saved;
        let run_id = "1".repeat(32);
        saved["id"] = json!(run_id);
        let directory = project_dir.join("native_studio_pipelines").join(&run_id);
        fs::create_dir_all(&directory).unwrap();
        storage::write_json_atomic(&directory.join("status.json"), &saved).unwrap();
        let state = AppState {
            repository_root: base.clone(),
            data_root: base.clone(),
            writes_enabled: false,
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
        };
        let matching = find_reusable(&state, &request, &identity, &"2".repeat(32));
        assert!(
            matching.full.is_some(),
            "unchanged settings must reuse both versions"
        );
        assert!(compatible_version(&saved, &request, &identity));
        assert!(gpu_lineage_valid(&project_dir, &request, &identity, &saved));
        assert!(cpu_lineage_valid(&project_dir, &request, &saved));

        // Reconstructing touches differently reuses the same GPU analysis.
        request.post.touches.threshold = 0.61;
        let changed_cpu = find_reusable(&state, &request, &identity, &"2".repeat(32));
        assert!(changed_cpu.full.is_none());
        assert!(
            changed_cpu.gpu.is_some(),
            "CPU change must reuse the GPU fusion"
        );
        assert!(compatible_version(&saved, &request, &identity));
        assert!(gpu_lineage_valid(&project_dir, &request, &identity, &saved));
        assert_ne!(
            saved["profile"]["post"],
            serde_json::to_value(&request.post).unwrap()
        );

        // Changing inference parameters or video identity cannot reuse the GPU branch.
        request.gpu.stride = 8;
        let changed_gpu = find_reusable(&state, &request, &identity, &"2".repeat(32));
        assert!(changed_gpu.full.is_none() && changed_gpu.gpu.is_none());
        assert!(!compatible_version(&saved, &request, &identity));
        request.gpu.stride = 9;
        let other_source = json!({"bytes":2048,"mtime_ns":123456790u64});
        let replaced_source = find_reusable(&state, &request, &other_source, &"2".repeat(32));
        assert!(replaced_source.gpu.is_none() && replaced_source.full.is_none());
        assert!(!compatible_version(&saved, &request, &other_source));
        assert!(!gpu_lineage_valid(
            &project_dir,
            &request,
            &other_source,
            &saved
        ));

        // Forged lineage cannot be selected even when a profile matches.
        let mut tampered = saved.clone();
        tampered["gpu_jobs"]["ball"] = json!("7".repeat(32));
        assert!(!gpu_lineage_valid(
            &project_dir,
            &request,
            &identity,
            &tampered
        ));
        tampered = saved.clone();
        tampered["cpu_runs"]["tracking"] = json!("8".repeat(32));
        assert!(!cpu_lineage_valid(&project_dir, &request, &tampered));
        // Diagnostics persist the exact native arguments and process argv.
        let logfile = directory.join("status.json");
        log_native(
            &logfile,
            "GPU",
            "inference_full::start",
            json!({"model":"ball","stride":9,"cpu_threads":8}),
        )
        .unwrap();
        log_entry(
            &logfile,
            "info",
            "invocation",
            "Подготовка",
            "FFmpeg Command::spawn",
            Some(json!({"type":"process","name":"ffmpeg",
                "argv":["ffmpeg","-i","match.mp4","-y","output.mp4"]})),
        )
        .unwrap();
        let gpu_selected = selected_gpu(&state, &request, &identity, &analysis).unwrap();
        assert_eq!(gpu_selected["analysis_id"], analysis);
        assert_eq!(gpu_selected["gpu_jobs"]["ball"], gpu_ball);
        assert!(selected_gpu(&state, &request, &identity, &"7".repeat(32)).is_err());
        assert!(selected_gpu(
            &state,
            &request,
            &json!({"bytes":42,"mtime_ns":123456789u64}),
            &analysis
        )
        .is_err());
        let handler_state = Arc::new(state);
        let Json(first) = log(
            State(handler_state.clone()),
            Query(LogQuery {
                project_id: "abc123def456".into(),
                id: run_id.clone(),
                after: 0,
            }),
        )
        .await
        .unwrap();
        assert_eq!(first["schema"], "volley-studio-pipeline-log/1");
        assert_eq!(first["entries"].as_array().unwrap().len(), 2);
        assert_eq!(first["entries"][0]["call"]["name"], "inference_full::start");
        assert_eq!(first["entries"][0]["call"]["arguments"]["stride"], 9);
        assert_eq!(first["entries"][1]["call"]["argv"][1], "-i");
        assert_eq!(first["next_offset"], 2);
        let Json(second) = log(
            State(handler_state.clone()),
            Query(LogQuery {
                project_id: "abc123def456".into(),
                id: run_id.clone(),
                after: 1,
            }),
        )
        .await
        .unwrap();
        assert_eq!(second["entries"].as_array().unwrap().len(), 1);
        assert_eq!(second["entries"][0]["call"]["type"], "process");
        assert!(log(
            State(handler_state.clone()),
            Query(LogQuery {
                project_id: "abc123def456".into(),
                id: "7".repeat(32),
                after: 0
            })
        )
        .await
        .is_err());
        fs::write(
            directory.join("events.jsonl"),
            concat!(
                "{\"timestamp_ms\":1000,\"kind\":\"stage_start\",\"stage\":\"GPU\"}\n",
                "{\"timestamp_ms\":3000,\"kind\":\"stage_start\",\"stage\":\"CPU\"}\n",
                "{\"timestamp_ms\":4500,\"kind\":\"event\",\"stage\":\"Анализ\"}\n"
            ),
        )
        .unwrap();
        let history = history_rows(&handler_state, "abc123def456", 0).unwrap();
        assert_eq!(history["runs"][0]["id"], run_id);
        assert_eq!(history["runs"][0]["duration_ms"], 3500);
        assert_eq!(history["runs"][0]["stages"][0]["duration_ms"], 2000);
        assert_eq!(history["runs"][0]["stages"][1]["duration_ms"], 1500);
        // CPU can use a standalone GPU fusion with no Studio pipeline record.
        fs::remove_file(directory.join("status.json")).unwrap();
        let source = selected_gpu(&handler_state, &request, &identity, &analysis).unwrap();
        assert_eq!(source["gpu_jobs"]["ball"], gpu_ball);
        assert_eq!(source["gpu_jobs"]["players"], gpu_players);
        assert_eq!(source["profile"]["gpu"]["stride"], 9);
        assert!(selected_gpu(
            &handler_state,
            &request,
            &json!({"bytes":42,"mtime_ns":123456789u64}),
            &analysis
        )
        .is_err());
        // Regression: the user-selected GPU source is valid even when the
        // browser sends a different/default GPU profile for CPU-only runs.
        // Such a run must never invoke a model or write a GPU stage status.
        let mut post = request.clone();
        post.mode = PipelineMode::Post;
        post.source_analysis_id = Some(analysis.clone());
        post.gpu.stride = 3;
        let selected = selected_gpu(&handler_state, &post, &identity, &analysis).unwrap();
        assert_eq!(selected["analysis_id"], analysis);
        assert_eq!(selected["gpu_jobs"]["ball"], gpu_ball);
        assert!(!post.mode.allows_gpu_execution());
        let untouched = directory.join("post-must-not-write-gpu.json");
        let mut post_status = json!({"stage":"CPU-only"});
        let refusal = gpu(
            &handler_state,
            &post,
            ModelKind::Ball,
            &AtomicBool::new(false),
            &untouched,
            &mut post_status,
            8.0,
            36.0,
        )
        .await
        .unwrap_err();
        assert!(refusal.contains("не имеет права запускать GPU"));
        assert!(!untouched.exists());
        assert_eq!(post_status["stage"], "CPU-only");
        for index in 0..26 {
            let id = format!("{index:032x}");
            let run = project_dir.join("native_studio_pipelines").join(&id);
            fs::create_dir_all(&run).unwrap();
            fs::write(
                run.join("status.json"),
                serde_json::to_vec(&json!({
                    "schema":SCHEMA,"id":id,"project_id":"abc123def456","status":"complete"
                }))
                .unwrap(),
            )
            .unwrap();
        }
        let page = history_rows(&handler_state, "abc123def456", 0).unwrap();
        assert_eq!(page["runs"].as_array().unwrap().len(), 25);
        assert_eq!(page["next_offset"], 25);
        assert_eq!(page["has_more"], true);
        let page2 = history_rows(&handler_state, "abc123def456", 25).unwrap();
        assert_eq!(page2["runs"].as_array().unwrap().len(), 1);
        assert_eq!(page2["has_more"], false);
        fs::remove_dir_all(base).unwrap();
    }
}
