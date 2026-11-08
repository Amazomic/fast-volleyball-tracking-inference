//! Experimental full prepared-video ONNX processing with bounded memory.
//! One model/video per job, immutable raw-detections JSONL, durable status.
//! This is *not* a complete Studio GPU/CPU analysis or player Track ID result.
use crate::{
    inference_stream, preparation, project_archive, projects,
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
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex as StdMutex,
    },
    time::Duration,
};
use tokio::sync::Mutex;
use uuid::Uuid;
use volley_inference::{
    decode::{decode_ravel, decode_vball, DecodeConfig, RawDetection},
    Config, Device, Engine, ModelKind,
};
use volley_media::{
    frame_windows::normalized_model_window,
    raw_decoder::{decode_full_video, DecodeSummary, ModelPixels},
};

const STATUS_FILE: &str = "status.json";
const RESULT_FILE: &str = "detections.jsonl";
const MANIFEST_FILE: &str = "manifest.json";
const MIN_STRIDE: usize = 1;
const MAX_STRIDE: usize = 9;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct JobStatus {
    job_id: String,
    project_id: String,
    video: String,
    model: ModelKind,
    phase: String,
    windows: usize,
    decoded_frames: u64,
    progress_percent: f64,
    error: Option<String>,
    #[serde(default)]
    output_manifest: Option<String>,
}

struct Active {
    id: String,
    project_id: String,
    cancel: Arc<AtomicBool>,
    status: Arc<StdMutex<JobStatus>>,
}

pub struct FullInferenceManager {
    current: Mutex<Option<Active>>,
}
impl FullInferenceManager {
    pub fn new() -> Self {
        Self {
            current: Mutex::new(None),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartRequest {
    project_id: String,
    video: String,
    model: ModelKind,
    #[serde(default = "default_stride")]
    stride: usize,
    #[serde(default = "default_device")]
    device: Device,
    #[serde(default)]
    gpu_device_id: i32,
    #[serde(default)]
    gpu_memory_limit_mb: u64,
    #[serde(default = "default_threads")]
    cpu_threads: usize,
}
fn default_stride() -> usize {
    9
}
fn default_device() -> Device {
    Device::Cuda
}
fn default_threads() -> usize {
    8
}

#[derive(Deserialize)]
pub struct JobQuery {
    project_id: String,
    job_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelRequest {
    project_id: String,
    job_id: String,
}

fn valid_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
fn terminal(phase: &str) -> bool {
    matches!(phase, "complete" | "failed" | "cancelled" | "interrupted")
}

fn job_root(state: &AppState, project_id: &str, create: bool) -> Result<PathBuf, ApiError> {
    storage::project_id(project_id)?;
    let path = projects::project_root(state).join(project_id);
    let info = fs::symlink_metadata(&path)
        .map_err(|_| storage::not_found("Каталог проекта отсутствует"))?;
    if info.file_type().is_symlink() || !info.is_dir() {
        return Err(storage::bad_request("Каталог проекта небезопасен"));
    }
    let root = state
        .data_root
        .canonicalize()
        .map_err(|e| storage::internal(format!("Resolve sandbox: {e}")))?;
    if !path
        .canonicalize()
        .map_err(|e| storage::internal(format!("Resolve project: {e}")))?
        .starts_with(&root)
    {
        return Err(storage::bad_request(
            "Путь проекта находится за пределами Rust-хранилища",
        ));
    }
    let jobs = path.join("native_gpu_runs");
    match fs::symlink_metadata(&jobs) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
            return Err(storage::bad_request("Папка GPU-запусков небезопасна"))
        }
        Ok(_) => (),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && create => {
            fs::create_dir(&jobs)
                .map_err(|e| storage::internal(format!("Create GPU job root: {e}")))?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(storage::internal(format!("Check GPU run folder: {e}"))),
    }
    Ok(jobs)
}

fn checked_run_folder(root: &Path, id: &str) -> Result<PathBuf, ApiError> {
    if !valid_id(id) {
        return Err(storage::bad_request("Неверный ID GPU-задачи"));
    }
    let path = root.join(id);
    let meta =
        fs::symlink_metadata(&path).map_err(|_| storage::not_found("GPU-задача не найдена"))?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(storage::bad_request("Недопустимый каталог GPU-задачи"));
    }
    Ok(path)
}

fn persist(folder: &Path, status: &JobStatus) -> Result<(), String> {
    storage::write_json_atomic(
        &folder.join(STATUS_FILE),
        &serde_json::to_value(status).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("Save GPU job status: {e:?}"))
}
fn snapshot(status: &Arc<StdMutex<JobStatus>>) -> JobStatus {
    status
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}
fn update_progress(
    status: &Arc<StdMutex<JobStatus>>,
    windows: usize,
    decoded_frames: u64,
    estimated_frames: f64,
) {
    let mut item = status
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    item.phase = "running".into();
    item.windows = windows;
    item.decoded_frames = decoded_frames;
    item.progress_percent = (decoded_frames as f64 * 100.0 / estimated_frames).clamp(0.0, 99.0);
}

struct EvidenceWriter {
    writer: BufWriter<File>,
    digest: Sha256,
    count: usize,
}
impl EvidenceWriter {
    fn create(folder: &Path) -> Result<Self, String> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(folder.join("detections.jsonl.partial"))
            .map_err(|e| format!("Create evidence output: {e}"))?;
        Ok(Self {
            writer: BufWriter::new(file),
            digest: Sha256::new(),
            count: 0,
        })
    }
    fn push(&mut self, frame: u64, records: &[RawDetection]) -> Result<(), String> {
        let mut line = serde_json::to_vec(&json!({
            "first_frame":frame,
            "raw_detections":records,
            "confirmed_tracks":false,
            "confirmed_touches":false,
        }))
        .map_err(|e| format!("Serialize raw detections: {e}"))?;
        line.push(b'\n');
        self.writer
            .write_all(&line)
            .map_err(|e| format!("Write model evidence: {e}"))?;
        self.digest.update(&line);
        self.count += 1;
        Ok(())
    }
    fn finish(mut self) -> Result<(String, usize), String> {
        self.writer
            .flush()
            .map_err(|e| format!("Flush model evidence: {e}"))?;
        self.writer
            .get_ref()
            .sync_all()
            .map_err(|e| format!("Sync model evidence: {e}"))?;
        Ok((format!("{:x}", self.digest.finalize()), self.count))
    }
}

#[derive(Clone)]
struct JobContext {
    id: String,
    project_id: String,
    video: String,
    source: PathBuf,
    duration_sec: f64,
    config: Config,
    stride: usize,
    folder: PathBuf,
    repo: PathBuf,
    status: Arc<StdMutex<JobStatus>>,
    cancelled: Arc<AtomicBool>,
}

fn run_model(context: &JobContext) -> Result<DecodeSummary, String> {
    if context.cancelled.load(Ordering::Acquire) {
        return Err("cancelled".into());
    }
    let before = inference_stream::source_stamp(&context.source)?;
    let mut engine = Engine::load(&context.repo, &context.config)?;
    let pixels = match context.config.model {
        ModelKind::Ball => ModelPixels::BallGray,
        ModelKind::Players => ModelPixels::PlayersRgb,
    };
    let (w, h, channels, _) = pixels.dimensions();
    let mut sink = EvidenceWriter::create(&context.folder)?;
    let estimated_frames = (context.duration_sec * 30.0).max(9.0);
    let summary = decode_full_video(
        &context.source,
        pixels,
        context.stride,
        &context.cancelled,
        |window| {
            if context.cancelled.load(Ordering::Acquire) {
                return Err("cancelled".into());
            }
            let input = normalized_model_window(&window.data, w, h, channels)?;
            let tensors = engine.infer_clip(input)?;
            let records = if context.config.model == ModelKind::Players {
                decode_ravel(&tensors, window.first_frame, DecodeConfig::default())?
            } else {
                decode_vball(&tensors, window.first_frame, 0.05)?
            };
            sink.push(window.first_frame, &records)?;
            let windows = sink.count;
            if windows % 20 == 0 {
                update_progress(
                    &context.status,
                    windows,
                    window.first_frame + 9,
                    estimated_frames,
                );
                persist(&context.folder, &snapshot(&context.status))?;
            }
            Ok(())
        },
    )?;
    if context.cancelled.load(Ordering::Acquire) {
        return Err("cancelled".into());
    }
    if inference_stream::source_stamp(&context.source)? != before {
        return Err("Prepared video changed during full ONNX scan".into());
    }
    let (digest, count) = sink.finish()?;
    if count != summary.windows {
        return Err("Model output window count disagrees with decoder".into());
    }
    let final_file = context.folder.join(RESULT_FILE);
    fs::rename(context.folder.join("detections.jsonl.partial"), &final_file)
        .map_err(|e| format!("Publish raw detections: {e}"))?;
    let manifest = json!({
        "schema":"volley-native-gpu-model-detections/1",
        "status":"model_raw_detections_complete",
        "job_id":context.id,
        "project_id":context.project_id,
        "video":context.video,
        "model":context.config.model,
        "provider":context.config.device,
        "source_identity":{"bytes":before.0,"mtime_ns":before.1},
        "fps":30,
        "stride":context.stride,
        "decoded_frames":summary.decoded_frames,
        "windows":summary.windows,
        "file":RESULT_FILE,
        "sha256":digest,
        "raw_candidates_only":true,
        "stable_track_ids_present":false,
        "touches_confirmed":false,
        "studio_gpu_run_complete":false,
        "cpu_processing_complete":false,
    });
    storage::write_json_atomic(&context.folder.join(MANIFEST_FILE), &manifest)
        .map_err(|e| format!("Write immutable model manifest: {e:?}"))?;
    Ok(summary)
}

async fn worker(state: Arc<AppState>, context: JobContext) {
    let permit = loop {
        if context.cancelled.load(Ordering::Acquire) {
            break None;
        }
        match tokio::time::timeout(Duration::from_millis(200), state.inference_limit.acquire())
            .await
        {
            Ok(Ok(permit)) => break Some(permit),
            Ok(Err(_)) => break None,
            Err(_) => {}
        }
    };
    let outcome = if let Some(_permit) = permit {
        {
            let mut info = context.status.lock().unwrap_or_else(|e| e.into_inner());
            info.phase = "loading_model".into();
        }
        let _ = persist(&context.folder, &snapshot(&context.status));
        let task = context.clone();
        tokio::task::spawn_blocking(move || run_model(&task))
            .await
            .map_err(|e| format!("ONNX model worker panicked: {e}"))
            .and_then(|r| r)
    } else {
        Err("cancelled".into())
    };
    let mut finished = snapshot(&context.status);
    match outcome {
        Ok(summary) => {
            finished.phase = "complete".into();
            finished.progress_percent = 100.0;
            finished.windows = summary.windows;
            finished.decoded_frames = summary.decoded_frames;
            finished.output_manifest = Some(MANIFEST_FILE.into());
        }
        Err(error) => {
            finished.phase = if error.contains("cancelled") {
                "cancelled"
            } else {
                "failed"
            }
            .into();
            finished.error = Some(error.chars().take(700).collect());
            let _ = fs::remove_file(context.folder.join("detections.jsonl.partial"));
        }
    }
    if let Err(e) = persist(&context.folder, &finished) {
        finished.phase = "failed".into();
        finished.error = Some(e);
        finished.output_manifest = None;
    }
    let mut info = context.status.lock().unwrap_or_else(|e| e.into_inner());
    *info = finished;
}

/// Queue one model of a prepared video. A Studio-level GPU run needs BOTH
/// model outputs and further cross-window/CPU reconstruction; this route
/// deliberately advertises model-level status only.
pub async fn start(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<StartRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    if !state.writes_enabled {
        return Err(storage::conflict(
            "GPU-запуск возможен только в отдельной Rust-песочнице",
        ));
    }
    storage::project_id(&request.project_id)?;
    storage::video_name(&request.video)?;
    if !(MIN_STRIDE..=MAX_STRIDE).contains(&request.stride) {
        return Err(storage::bad_request("Шаг кадров должен быть от 1 до 9"));
    }
    let config = Config {
        model: request.model,
        device: request.device,
        gpu_device_id: request.gpu_device_id,
        gpu_memory_limit_mb: request.gpu_memory_limit_mb,
        cpu_threads: request.cpu_threads,
    };
    config.verify().map_err(storage::bad_request)?;
    let mut current = state.full_inference.current.lock().await;
    if let Some(old) = current.as_ref() {
        if !terminal(&snapshot(&old.status).phase) {
            return Err(storage::conflict(
                "Другая полная GPU-задача уже выполняется",
            ));
        }
    }
    let project_id = request.project_id.clone();
    let _guard = project_archive::archive_guard(&state, &project_id)?;
    projects::existing_project(&state, &project_id)?;
    let plan = preparation::plan_for(&state, &project_id, &request.video)?;
    let source = plan
        .ready_path()
        .ok_or_else(|| storage::bad_request("Сначала подготовьте видео через Rust"))?
        .to_path_buf();
    if !(0.3..=48.0 * 3600.0).contains(&plan.duration_sec) {
        return Err(storage::bad_request(
            "Длительность видео для инференса некорректна",
        ));
    }
    let folder_root = job_root(&state, &project_id, true)?;
    let job_id = Uuid::new_v4().simple().to_string();
    let folder = folder_root.join(&job_id);
    fs::create_dir(&folder)
        .map_err(|e| storage::internal(format!("Create GPU job directory: {e}")))?;
    let status = Arc::new(StdMutex::new(JobStatus {
        job_id: job_id.clone(),
        project_id: project_id.clone(),
        video: request.video.clone(),
        model: request.model,
        phase: "queued".into(),
        windows: 0,
        decoded_frames: 0,
        progress_percent: 0.0,
        error: None,
        output_manifest: None,
    }));
    persist(&folder, &snapshot(&status)).map_err(storage::internal)?;
    let cancel = Arc::new(AtomicBool::new(false));
    let context = JobContext {
        id: job_id.clone(),
        project_id: project_id.clone(),
        video: request.video.clone(),
        source,
        duration_sec: plan.duration_sec,
        config,
        stride: request.stride,
        folder,
        repo: state.repository_root.clone(),
        status: status.clone(),
        cancelled: cancel.clone(),
    };
    *current = Some(Active {
        id: job_id.clone(),
        project_id,
        cancel,
        status,
    });
    drop(_guard);
    drop(current);
    tokio::spawn(worker(state, context));
    Ok(Json(json!({"job_id":job_id,"state":"queued",
        "studio_gpu_run_complete":false,
        "note":"Raw single-model scan. Full RAVEL/VballNet+CPU Studio pipeline remains pending."})))
}

pub async fn status(
    State(state): State<Arc<AppState>>,
    Query(query): Query<JobQuery>,
) -> ApiResult<Value> {
    storage::project_id(&query.project_id)?;
    projects::existing_project(&state, &query.project_id)?;
    if let Some(active) = state.full_inference.current.lock().await.as_ref() {
        if active.project_id == query.project_id
            && query.job_id.as_deref().is_none_or(|id| id == active.id)
        {
            return Ok(Json(
                serde_json::to_value(snapshot(&active.status))
                    .map_err(|e| storage::internal(format!("Serialize GPU status: {e}")))?,
            ));
        }
    }
    let id = query
        .job_id
        .ok_or_else(|| storage::not_found("Нет активной GPU-задачи"))?;
    let root = job_root(&state, &query.project_id, false)?;
    let folder = checked_run_folder(&root, &id)?;
    let file = folder.join(STATUS_FILE);
    let metadata =
        fs::symlink_metadata(&file).map_err(|_| storage::not_found("Нет статуса GPU-задачи"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 16384 {
        return Err(storage::bad_request("Статус GPU-задачи повреждён"));
    }
    let mut data =
        storage::read_json(&file)?.ok_or_else(|| storage::not_found("GPU status missing"))?;
    if data["project_id"].as_str() != Some(&query.project_id)
        || data["job_id"].as_str() != Some(&id)
    {
        return Err(storage::bad_request(
            "GPU job status belongs to another project",
        ));
    }
    if data["phase"] == "running" || data["phase"] == "queued" || data["phase"] == "loading_model" {
        data["phase"] = json!("interrupted");
        data["error"] = json!("Сервер был перезапущен до завершения GPU-задачи");
    }
    Ok(Json(data))
}

pub async fn cancel(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<CancelRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    storage::project_id(&request.project_id)?;
    if !valid_id(&request.job_id) {
        return Err(storage::bad_request("Некорректный GPU job ID"));
    }
    let manager = state.full_inference.current.lock().await;
    let job = manager
        .as_ref()
        .filter(|item| item.project_id == request.project_id && item.id == request.job_id)
        .ok_or_else(|| storage::not_found("Активная GPU-задача не найдена"))?;
    if terminal(&snapshot(&job.status).phase) {
        return Ok(Json(
            json!({"cancel_requested":false,"state":"already_finished"}),
        ));
    }
    job.cancel.store(true, Ordering::Release);
    Ok(Json(json!({"cancel_requested":true,"job_id":job.id})))
}

pub(crate) async fn stop_for_project_archive(
    state: &AppState,
    project_id: &str,
) -> Result<bool, ApiError> {
    let target = {
        let manager = state.full_inference.current.lock().await;
        manager
            .as_ref()
            .filter(|active| {
                active.project_id == project_id && !terminal(&snapshot(&active.status).phase)
            })
            .map(|active| {
                active.cancel.store(true, Ordering::Release);
                active.status.clone()
            })
    };
    let Some(status) = target else {
        return Ok(false);
    };
    for _ in 0..120 {
        if terminal(&snapshot(&status).phase) {
            return Ok(true);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(storage::conflict(
        "GPU-задача не остановилась; архивирование отклонено",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn raw_jsonl_is_append_only_and_hashable() {
        let root = std::env::temp_dir().join(format!("volley-full-gpu-writer-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let mut writer = EvidenceWriter::create(&root).unwrap();
        let record = RawDetection {
            frame_index: 10,
            class_name: "player",
            score: 0.9,
            bbox_xyxy_norm: [0.1, 0.2, 0.3, 0.4],
            query_index: Some(1),
            touch_probability: Some(0.9),
            head_point_norm: None,
        };
        writer.push(10, &[record]).unwrap();
        writer.push(19, &[]).unwrap();
        let (sha, count) = writer.finish().unwrap();
        assert_eq!(count, 2);
        let bytes = fs::read(root.join("detections.jsonl.partial")).unwrap();
        assert_eq!(sha, format!("{:x}", Sha256::digest(&bytes)));
        let lines = bytes
            .split(|b| *b == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["raw_detections"][0]["query_index"], 1);
        assert!(lines[1]["raw_detections"].as_array().unwrap().is_empty());
        assert!(EvidenceWriter::create(&root).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn full_job_rejects_missing_runtime_without_publishing_fake_gpu_result() {
        use std::process::Command;
        if Command::new("ffmpeg").arg("-version").output().is_err() {
            return;
        }
        let root = std::env::temp_dir().join(format!("volley-gpu-full-fixture-{}", Uuid::new_v4()));
        let repo = root.join("original");
        let data = root.join("sandbox");
        let project_id = "1234abcdef01";
        let video = "fixture.mp4";
        fs::create_dir_all(repo.join("project")).unwrap();
        fs::create_dir_all(data.join("projects").join(project_id)).unwrap();
        let original = repo.join("project").join(video);
        let result = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=64x64:rate=30:duration=0.5",
                "-an",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-y",
            ])
            .arg(&original)
            .status()
            .unwrap();
        assert!(result.success());
        let original_size = fs::metadata(&original).unwrap().len();
        storage::write_json_atomic(
            &data.join("projects").join("projects.json"),
            &json!({"schema":1,"projects":[{
                "id":project_id,"name":"fixture","videos":[video]
            }]}),
        )
        .unwrap();
        let state = Arc::new(AppState {
            repository_root: repo.clone(),
            data_root: data.clone(),
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
            full_inference: FullInferenceManager::new(),
        });
        let request = StartRequest {
            project_id: project_id.into(),
            video: video.into(),
            model: ModelKind::Players,
            stride: 9,
            device: Device::Cpu,
            gpu_device_id: 0,
            gpu_memory_limit_mb: 0,
            cpu_threads: 2,
        };
        let Json(receipt) = start(State(state.clone()), HeaderMap::new(), Json(request))
            .await
            .unwrap();
        let job_id = receipt["job_id"].as_str().unwrap().to_string();
        assert!(valid_id(&job_id));
        let mut last = Value::Null;
        for _ in 0..60 {
            last = status(
                State(state.clone()),
                Query(JobQuery {
                    project_id: project_id.into(),
                    job_id: Some(job_id.clone()),
                }),
            )
            .await
            .unwrap()
            .0;
            if terminal(last["phase"].as_str().unwrap_or("")) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        // This fixture has no repository/models/ directory. Model loading
        // must fail instead of a successful fabricated full GPU run.
        assert_eq!(last["phase"], "failed", "state: {last}");
        assert!(last["error"].is_string());
        assert!(last["output_manifest"].is_null());
        let folder = data
            .join("projects")
            .join(project_id)
            .join("native_gpu_runs")
            .join(&job_id);
        assert!(!folder.join(MANIFEST_FILE).exists());
        assert!(!folder.join(RESULT_FILE).exists());
        assert_eq!(fs::metadata(&original).unwrap().len(), original_size);
        assert!(!repo.join("results").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    #[ignore = "Needs native ORT_DYLIB_PATH and source ONNX models; run explicitly"]
    async fn native_full_model_fixture_end_to_end() {
        use std::process::Command;
        let root = std::env::temp_dir().join(format!("volley-native-gpu-e2e-{}", Uuid::new_v4()));
        let repo = root.join("original");
        let data = root.join("sandbox");
        let id = "1234abcdef01";
        let video = "fixture.mp4";
        let source_models = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("models");
        fs::create_dir_all(repo.join("project")).unwrap();
        fs::create_dir_all(repo.join("models")).unwrap();
        fs::create_dir_all(data.join("projects").join(id)).unwrap();
        let mut completed_jobs = Vec::new();
        for model in [ModelKind::Ball, ModelKind::Players] {
            let onnx = model.filename();
            let source = source_models.join(onnx);
            let dest = repo.join("models").join(onnx);
            fs::copy(&source, &dest).unwrap();
            fs::copy(source.with_extension("json"), dest.with_extension("json")).unwrap();
        }
        let original = repo.join("project").join(video);
        let media = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=64x64:rate=30:duration=0.6",
                "-an",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-y",
            ])
            .arg(&original)
            .status()
            .unwrap();
        assert!(media.success());
        let source_size = fs::metadata(&original).unwrap().len();
        storage::write_json_atomic(
            &data.join("projects").join("projects.json"),
            &json!({"schema":1,"projects":[{
                "id":id,"name":"fixture","videos":[video]
            }]}),
        )
        .unwrap();
        let state = Arc::new(AppState {
            repository_root: repo.clone(),
            data_root: data.clone(),
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
            full_inference: FullInferenceManager::new(),
        });
        let device = match std::env::var("VOLLEY_TEST_ORT_DEVICE").as_deref() {
            Ok("cuda") => Device::Cuda,
            _ => Device::Cpu,
        };
        for model in [ModelKind::Ball, ModelKind::Players] {
            let request = StartRequest {
                project_id: id.into(),
                video: video.into(),
                model,
                stride: 9,
                device,
                gpu_device_id: 0,
                gpu_memory_limit_mb: 0,
                cpu_threads: 2,
            };
            let Json(receipt) = start(State(state.clone()), HeaderMap::new(), Json(request))
                .await
                .unwrap();
            let job_id = receipt["job_id"].as_str().unwrap().to_owned();
            let mut last = Value::Null;
            for _ in 0..160 {
                last = status(
                    State(state.clone()),
                    Query(JobQuery {
                        project_id: id.into(),
                        job_id: Some(job_id.clone()),
                    }),
                )
                .await
                .unwrap()
                .0;
                if terminal(last["phase"].as_str().unwrap_or("")) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            assert_eq!(last["phase"], "complete", "GPU result: {last}");
            assert_eq!(last["output_manifest"], MANIFEST_FILE);
            let folder = data
                .join("projects")
                .join(id)
                .join("native_gpu_runs")
                .join(&job_id);
            let manifest = storage::read_json(&folder.join(MANIFEST_FILE))
                .unwrap()
                .unwrap();
            assert_eq!(manifest["schema"], "volley-native-gpu-model-detections/1");
            assert_eq!(manifest["status"], "model_raw_detections_complete");
            assert_eq!(manifest["windows"], 2);
            assert_eq!(manifest["decoded_frames"], 18);
            assert_eq!(manifest["raw_candidates_only"], true);
            assert_eq!(manifest["studio_gpu_run_complete"], false);
            let evidence = fs::read(folder.join(RESULT_FILE)).unwrap();
            let hash = format!("{:x}", Sha256::digest(&evidence));
            assert_eq!(manifest["sha256"].as_str(), Some(hash.as_str()));
            let records = evidence
                .split(|b| *b == b'\n')
                .filter(|line| !line.is_empty())
                .map(|line| serde_json::from_slice::<Value>(line).unwrap())
                .collect::<Vec<_>>();
            assert_eq!(records.len(), 2);
            assert_eq!(records[0]["first_frame"], 0);
            assert_eq!(records[1]["first_frame"], 9);
            assert!(records.iter().all(|row| row["raw_detections"].is_array()));
            println!(
                "Complete {model:?} {device:?} native JSONL job: {} bytes SHA256 verified",
                evidence.len()
            );
            completed_jobs.push(job_id);
        }
        // The end-to-end native CPU entry point must verify both real CUDA
        // model hashes before publishing the paired raw frame stream.
        let merged = crate::inference_fusion::do_fusion(
            &state,
            &crate::inference_fusion::FuseRequest {
                project_id: id.into(),
                video: video.into(),
                vball_job_id: completed_jobs[0].clone(),
                ravel_job_id: completed_jobs[1].clone(),
            },
        )
        .unwrap();
        assert_eq!(merged["schema"], "volley-native-gpu-fusion/1");
        assert_eq!(merged["emitted_frames"], 18);
        assert_eq!(merged["windows"], 2);
        assert_eq!(merged["player_track_ids_confirmed"], false);
        assert_eq!(merged["cpu_processing_complete"], false);
        let output = data
            .join("projects")
            .join(id)
            .join("native_gpu_fusions")
            .join(merged["fusion_id"].as_str().unwrap())
            .join("frames.jsonl");
        let fused = fs::read(&output).unwrap();
        let digest = format!("{:x}", Sha256::digest(&fused));
        assert_eq!(merged["sha256"].as_str(), Some(digest.as_str()));
        assert_eq!(fused.iter().filter(|&&byte| byte == 10u8).count(), 18);
        println!(
            "Real {device:?} RAVEL/VballNet fusion: 18 verified frame rows, {} bytes",
            fused.len()
        );
        let cpu_replay = crate::cpu_retrack::process(
            &state,
            &crate::cpu_retrack::ReplayRequest {
                project_id: id.into(),
                video: video.into(),
                fusion_id: merged["fusion_id"].as_str().unwrap().into(),
                settings: None,
            },
        )
        .unwrap();
        assert_eq!(cpu_replay["schema"], "volley-native-cpu-tracking/1");
        assert_eq!(cpu_replay["frames"], 18);
        assert_eq!(cpu_replay["track_ids_provisional"], true);
        assert_eq!(cpu_replay["touches_confirmed"], false);
        assert_eq!(cpu_replay["cpu_processing_complete"], false);
        let replay_path = data
            .join("projects")
            .join(id)
            .join("native_cpu_replays")
            .join(cpu_replay["replay_id"].as_str().unwrap())
            .join("tracked_frames.jsonl");
        let replay = fs::read(&replay_path).unwrap();
        assert_eq!(replay.iter().filter(|&&byte| byte == 10u8).count(), 18);
        let replay_hash = format!("{:x}", Sha256::digest(&replay));
        assert_eq!(cpu_replay["sha256"].as_str(), Some(replay_hash.as_str()));
        println!(
            "Native {device:?} models -> fusion -> CPU tracking: {} bytes SHA256 verified",
            replay.len()
        );
        let touch_run = crate::cpu_touches::process(
            &state,
            &crate::cpu_touches::TouchRequest {
                project_id: id.into(),
                video: video.into(),
                replay_id: cpu_replay["replay_id"].as_str().unwrap().into(),
                settings: None,
            },
        )
        .unwrap();
        assert_eq!(touch_run["schema"], "volley-native-cpu-touch-proposals/1");
        assert_eq!(touch_run["frames"], 18);
        assert_eq!(touch_run["contacts_authoritatively_confirmed"], false);
        assert_eq!(touch_run["athlete_identities_verified"], false);
        assert_eq!(touch_run["cpu_processing_complete"], false);
        let touch_path = data
            .join("projects")
            .join(id)
            .join("native_cpu_touch_runs")
            .join(touch_run["touch_id"].as_str().unwrap())
            .join("touch_candidates.jsonl");
        let touch_bytes = fs::read(touch_path).unwrap();
        let touch_hash = format!("{:x}", Sha256::digest(&touch_bytes));
        assert_eq!(touch_run["sha256"].as_str(), Some(touch_hash.as_str()));
        println!("Native {device:?} complete GPU -> fusion -> tracklet -> motion Touch replay: {} events",
            touch_run["touch_candidates"]);
        let rally_run = crate::cpu_rallies::process(
            &state,
            &crate::cpu_rallies::RallyRequest {
                project_id: id.into(),
                video: video.into(),
                replay_id: cpu_replay["replay_id"].as_str().unwrap().into(),
                touch_id: touch_run["touch_id"].as_str().unwrap().into(),
                settings: None,
            },
        )
        .unwrap();
        assert_eq!(rally_run["schema"], "volley-native-cpu-rally-proposals/1");
        assert_eq!(rally_run["frames"], 18);
        assert_eq!(rally_run["touch_candidates_ingested"], 0);
        assert_eq!(rally_run["rally_candidates"], 0);
        assert_eq!(rally_run["rallies_authoritatively_confirmed"], false);
        assert_eq!(rally_run["montage_ready"], false);
        assert_eq!(rally_run["cpu_processing_complete"], false);
        let rallies_path = data
            .join("projects")
            .join(id)
            .join("native_cpu_rallies")
            .join(rally_run["rally_run_id"].as_str().unwrap())
            .join("rally_candidates.jsonl");
        let rallies = fs::read(rallies_path).unwrap();
        assert!(rallies.is_empty());
        let rallies_sha = format!("{:x}", Sha256::digest(&rallies));
        assert_eq!(rally_run["sha256"].as_str(), Some(rallies_sha.as_str()));
        println!(
            "Native {device:?} full GPU -> fusion -> tracks -> Touch -> rally proposals: {}",
            rally_run["rally_candidates"]
        );
        let read_only_preview = crate::native_rally_preview::preview(
            axum::extract::State(state.clone()),
            axum::extract::Query(crate::native_rally_preview::PreviewQuery {
                project_id: id.into(),
                rally_run_id: rally_run["rally_run_id"].as_str().unwrap().into(),
                offset: 0,
                limit: 24,
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(read_only_preview["schema"], "volley-native-rally-preview/1");
        assert_eq!(read_only_preview["total_candidates"], 0);
        assert_eq!(read_only_preview["automatic_montage_eligible"], false);
        assert_eq!(read_only_preview["authoritative_game_events"], false);
        assert_eq!(read_only_preview["items"].as_array().unwrap().len(), 0);
        println!("Native {device:?} CPU rally preview loaded and SHA-verified");
        assert_eq!(fs::metadata(&original).unwrap().len(), source_size);
        assert!(!repo.join("results").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn api_job_id_must_be_opaque_and_terminal_states_explicit() {
        assert!(valid_id("0123456789abcdef0123456789abcdef"));
        assert!(!valid_id("../project"));
        assert!(terminal("complete"));
        assert!(terminal("failed"));
        assert!(!terminal("running"));
    }
}
