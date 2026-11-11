//! Video preparation jobs owned entirely by Rust.
//! Only isolated VOLLEY_RUST_DATA_ROOT may be written during migration.

use crate::{
    projects,
    storage::{self, ApiError, ApiResult},
    AppState,
};
use axum::{
    extract::{Query, State},
    http::HeaderMap,
    Json,
};
use fs2::FileExt;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex as StdMutex},
    time::{Duration, Instant, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::{watch, Mutex, Semaphore},
};
use uuid::Uuid;
use volley_media::preparation::{
    cache_id, prepared_file_sha256, PreparationPlan, PreparationReuse,
};

#[derive(Clone, Debug)]
pub(crate) struct Progress {
    pub(crate) state: String,
    pub(crate) progress: f64,
    pub(crate) error: String,
    /// Exact executable and argv captured from the real spawned FFmpeg process.
    pub(crate) process_argv: Option<Vec<String>>,
}

impl Progress {
    pub(crate) fn queued() -> Self {
        Self {
            state: "queued".into(),
            progress: 0.0,
            error: String::new(),
            process_argv: None,
        }
    }
}

struct ActiveJob {
    subscribed_projects: HashSet<String>,
    cancel: watch::Sender<bool>,
    progress: Arc<Mutex<Progress>>,
}

pub struct PreparationManager {
    jobs: Mutex<HashMap<String, ActiveJob>>,
    cancelled_projects: Mutex<HashSet<String>>,
    plan_cache: StdMutex<HashMap<PathBuf, PreparationPlan>>,
    concurrency: Arc<Semaphore>,
}

impl PreparationManager {
    pub(crate) async fn has_active(&self) -> bool {
        !self.jobs.lock().await.is_empty()
    }
    pub fn new() -> Self {
        Self {
            jobs: Mutex::new(HashMap::new()),
            cancelled_projects: Mutex::new(HashSet::new()),
            plan_cache: StdMutex::new(HashMap::new()),
            concurrency: Arc::new(Semaphore::new(1)),
        }
    }
}

pub(crate) fn plan_for(
    state: &AppState,
    id: &str,
    video: &str,
) -> Result<PreparationPlan, ApiError> {
    let project = projects::existing_project(state, id)?;
    storage::video_name(video)?;
    if project["quick_reel_internal"] == true {
        return crate::quick_reel::prepared_plan_for_quick_reel(state, &project, video);
    }
    if !project["videos"]
        .as_array()
        .is_some_and(|videos| videos.iter().any(|name| name.as_str() == Some(video)))
    {
        return Err(storage::bad_request(
            "Видео не принадлежит выбранному проекту",
        ));
    }
    let source = projects::canonical_source(state, video)?;
    if let Some(recorded) = project["source_paths"][video].as_str() {
        let recorded = Path::new(recorded)
            .canonicalize()
            .map_err(|_| storage::bad_request("Записанный источник видео больше не доступен"))?;
        if recorded != source {
            return Err(storage::bad_request("Исходный файл видео изменился"));
        }
    }
    // FFprobe is expensive on long videos. Cache a probe plan while the
    // original file identity (path, size, nanosecond mtime) remains unchanged.
    let metadata = source
        .metadata()
        .map_err(|error| storage::internal(format!("Video metadata: {error}")))?;
    let stamp = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|t| t.as_nanos());
    if let Some(stamp) = stamp {
        if let Ok(lookup) = state.preparation.plan_cache.lock() {
            if let Some(cached) = lookup.get(&source) {
                if cached.identity.size == metadata.len() && cached.identity.mtime_ns == stamp {
                    return Ok(cached.clone());
                }
            }
        }
    }
    let plan =
        PreparationPlan::new(&source, &state.data_root, None).map_err(storage::bad_request)?;
    if let Ok(mut lookup) = state.preparation.plan_cache.lock() {
        lookup.insert(source, plan.clone());
    }
    Ok(plan)
}

/// ffprobe on a plan-cache miss and the first full SHA verification are
/// blocking operations; async handlers must never run them on a tokio worker.
pub(crate) async fn plan_for_async(
    state: Arc<AppState>,
    id: &str,
    video: &str,
) -> Result<PreparationPlan, ApiError> {
    let id = id.to_owned();
    let video = video.to_owned();
    tokio::task::spawn_blocking(move || plan_for(&state, &id, &video))
        .await
        .map_err(|error| storage::internal(format!("Preparation plan task: {error}")))?
}

pub(crate) async fn reused_async(
    plan: &PreparationPlan,
) -> Result<Option<PreparationReuse>, String> {
    let plan = plan.clone();
    tokio::task::spawn_blocking(move || plan.reused())
        .await
        .map_err(|error| format!("Preparation reuse task: {error}"))
}

pub(crate) async fn ready_path_async(plan: &PreparationPlan) -> Result<Option<PathBuf>, String> {
    let plan = plan.clone();
    tokio::task::spawn_blocking(move || plan.ready_path().map(|path| path.to_path_buf()))
        .await
        .map_err(|error| format!("Prepared path task: {error}"))
}

fn manifest_for(plan: &PreparationPlan) -> Option<Value> {
    storage::read_json(&plan.manifest)
        .ok()
        .flatten()
        .filter(|value| value.get("identity") == serde_json::to_value(&plan.identity).ok().as_ref())
}

fn manifest_write(
    plan: &PreparationPlan,
    status: &str,
    progress: f64,
    error: &str,
) -> Result<(), ApiError> {
    let mut value = json!({
        "identity": plan.identity,
        "status": status,
        "progress": progress.clamp(0.0, 100.0),
    });
    if !error.is_empty() {
        value["error"] = json!(error.chars().take(500).collect::<String>());
    }
    storage::write_json_atomic(&plan.manifest, &value)
}

fn manifest_write_verified_ready(
    plan: &PreparationPlan,
    size: u64,
    sha256: &str,
) -> Result<(), ApiError> {
    storage::write_json_atomic(
        &plan.manifest,
        &json!({
            "identity":plan.identity,
            "status":"ready",
            "progress":100.0,
            "cache_size_bytes":size,
            "cache_sha256":sha256,
            "cache_verification":"sha256-stream-v1"
        }),
    )
}
async fn update(progress: &Arc<Mutex<Progress>>, state: &str, value: f64, error: &str) {
    let mut current = progress.lock().await;
    current.state = state.to_owned();
    current.progress = value.clamp(0.0, 100.0);
    current.error = error.chars().take(500).collect();
}

fn percent_from_ffmpeg(line: &str, duration: f64) -> Option<f64> {
    let value = line.trim().strip_prefix("out_time=")?;
    let mut fields = value.split(':');
    let h = fields.next()?.parse::<u64>().ok()?;
    let m = fields.next()?.parse::<u64>().ok()?;
    let s = fields.next()?.parse::<f64>().ok()?;
    if fields.next().is_some() || m >= 60 || !s.is_finite() || s < 0.0 || duration <= 0.0 {
        return None;
    }
    let elapsed = (h * 3600 + m * 60) as f64 + s;
    Some((elapsed / duration * 100.0).clamp(0.0, 99.5))
}

fn validate_source_unchanged(plan: &PreparationPlan) -> Result<(), String> {
    let changed = plan
        .source
        .metadata()
        .map_err(|error| format!("Source metadata unavailable: {error}"))?;
    let mtime_ns = changed
        .modified()
        .map_err(|error| format!("Source timestamp unreadable: {error}"))?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| format!("Source timestamp out of range: {error}"))?
        .as_nanos();
    if changed.len() != plan.identity.size || mtime_ns != plan.identity.mtime_ns {
        Err("Исходное видео изменилось во время подготовки".into())
    } else {
        Ok(())
    }
}

async fn wait_or_cancel(cancel: &mut watch::Receiver<bool>, duration: Duration) -> bool {
    if *cancel.borrow() {
        return true;
    }
    tokio::select! {
        _ = tokio::time::sleep(duration) => *cancel.borrow(),
        _ = cancel.changed() => *cancel.borrow()
    }
}

/// A file lock is held for the whole FFmpeg run. Other Rust instances cannot
/// publish the same content-addressed output concurrently.
async fn run_one(
    plan: &PreparationPlan,
    mut cancel: watch::Receiver<bool>,
    progress: &Arc<Mutex<Progress>>,
) -> Result<(), String> {
    let parent = plan.output.parent().ok_or("Prepared video has no parent")?;
    fs::create_dir_all(parent).map_err(|error| format!("Prepare directory: {error}"))?;

    let lockfile = plan.output.with_extension("prepare.lock");
    let lock: File = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lockfile)
        .map_err(|error| format!("Open preparation lock: {error}"))?;

    loop {
        if *cancel.borrow() {
            return Err("cancelled".into());
        }
        match lock.try_lock_exclusive() {
            Ok(()) => break,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if wait_or_cancel(&mut cancel, Duration::from_millis(500)).await {
                    return Err("cancelled".into());
                }
            }
            Err(error) => return Err(format!("Lock preparation: {error}")),
        }
    }

    // A different server may have published the exact same complete content
    // while we were waiting for the lock.
    if reused_async(plan).await?.is_some() {
        update(progress, "ready", 100.0, "").await;
        return Ok(());
    }
    if *cancel.borrow() {
        return Err("cancelled".into());
    }

    manifest_write(plan, "preparing", 0.0, "").map_err(|error| format!("{error:?}"))?;
    update(progress, "preparing", 0.0, "").await;
    let partial = plan.output.with_file_name(format!(
        "{}.{}.partial.mp4",
        cache_id(&plan.identity),
        Uuid::new_v4().simple()
    ));
    let logfile = plan.output.with_extension("ffmpeg.log");
    let stderr = File::create(&logfile).map_err(|error| format!("FFmpeg log: {error}"))?;
    let args = plan.ffmpeg_arguments(&partial);
    {
        let mut current = progress.lock().await;
        current.process_argv = Some(
            std::iter::once("ffmpeg".to_owned())
                .chain(args.iter().cloned())
                .collect(),
        );
    }
    let mut cmd = Command::new("ffmpeg");
    cmd.args(args);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(stderr))
        .kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|error| format!("Failed to start FFmpeg: {error}"))?;

    let result: Result<(), String> = async {
        let stdout = child.stdout.take().ok_or("FFmpeg stdout is unavailable")?;
        let mut lines = BufReader::new(stdout).lines();
        let mut last_write = Instant::now() - Duration::from_secs(1);
        loop {
            tokio::select! {
                line = lines.next_line() => {
                    match line {
                        Ok(Some(line)) => {
                            if let Some(percent) = percent_from_ffmpeg(&line, plan.duration_sec) {
                                if last_write.elapsed() >= Duration::from_millis(600) {
                                    let rounded = (percent * 10.0).round()/10.0;
                                    update(progress, "preparing", rounded, "").await;
                                    let _ = manifest_write(plan, "preparing", rounded, "");
                                    last_write = Instant::now();
                                }
                            }
                        }
                        Ok(None) => break,
                        Err(error) => return Err(format!("FFmpeg progress: {error}")),
                    }
                }
                _ = cancel.changed() => {
                    if *cancel.borrow() {
                        return Err("cancelled".into());
                    }
                }
            }
        }
        let status = child
            .wait()
            .await
            .map_err(|error| format!("FFmpeg wait: {error}"))?;
        if !status.success() {
            let detail = fs::read_to_string(&logfile).unwrap_or_default();
            let tail = detail
                .chars()
                .rev()
                .take(400)
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>();
            return Err(format!("FFmpeg завершился с ошибкой ({status}): {tail}"));
        }
        if *cancel.borrow() {
            return Err("cancelled".into());
        }
        validate_source_unchanged(plan)?;
        if !partial
            .metadata()
            .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
        {
            return Err("FFmpeg не создал подготовленный MP4".into());
        }
        // Produce a durable SHA receipt before publishing a cache entry.
        // Partial MP4s and cancelled jobs never become reusable.
        let hash_partial = partial.clone();
        let (cache_size, cache_sha256) =
            tokio::task::spawn_blocking(move || prepared_file_sha256(&hash_partial))
                .await
                .map_err(|error| format!("Hash finished FFmpeg output: {error}"))?
                .map_err(|error| format!("Hash finished FFmpeg output: {error}"))?;
        if *cancel.borrow() {
            return Err("cancelled".into());
        }
        validate_source_unchanged(plan)?;
        fs::rename(&partial, &plan.output)
            .map_err(|error| format!("Publish prepared video: {error}"))?;
        manifest_write_verified_ready(plan, cache_size, &cache_sha256)
            .map_err(|error| format!("Publish SHA-verified preparation: {error:?}"))?;
        update(progress, "ready", 100.0, "").await;
        Ok(())
    }
    .await;

    if result.is_err() {
        // A running FFmpeg must be terminated and reaped before publishing
        // cancellation. kill_on_drop is a final fallback, not normal cleanup.
        let _ = child.kill().await;
        let _ = child.wait().await;
        let _ = fs::remove_file(&partial);
    }
    result
}

fn publish_failure_if_uncontended(plan: &PreparationPlan, state: &str, error: &str) {
    // Failure after an interrupted transcode must not overwrite a successful
    // output or another process's active preparation manifest.
    if plan.reused().is_some() {
        return;
    }
    let file = plan.output.with_extension("prepare.lock");
    let Ok(lock) = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(file)
    else {
        return;
    };
    if lock.try_lock_exclusive().is_ok() && plan.reused().is_none() {
        let _ = manifest_write(plan, state, 0.0, error);
    }
}

async fn finished(
    state: Arc<AppState>,
    key: String,
    plan: PreparationPlan,
    mut cancel: watch::Receiver<bool>,
    progress: Arc<Mutex<Progress>>,
) {
    let permit = tokio::select! {
        result = state.preparation.concurrency.clone().acquire_owned() => {
            match result {
                Ok(permit) => permit,
                Err(_) => return,
            }
        }
        _ = cancel.changed() => {
            if *cancel.borrow() {
                update(&progress, "cancelled", 0.0, "").await;
                state.preparation.jobs.lock().await.remove(&key);
                return;
            }
            match state.preparation.concurrency.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(_) => return,
            }
        }
    };

    let outcome = run_one(&plan, cancel, &progress).await;
    match outcome {
        Ok(()) => {}
        Err(error) if error == "cancelled" => {
            update(&progress, "cancelled", 0.0, "").await;
            let failed_plan = plan.clone();
            let _ = tokio::task::spawn_blocking(move || {
                publish_failure_if_uncontended(&failed_plan, "cancelled", "");
            })
            .await;
        }
        Err(error) => {
            update(&progress, "failed", 0.0, &error).await;
            let failed_plan = plan.clone();
            let _ = tokio::task::spawn_blocking(move || {
                publish_failure_if_uncontended(&failed_plan, "failed", &error);
            })
            .await;
        }
    }
    drop(permit);
    state.preparation.jobs.lock().await.remove(&key);
}

/// Only called on the isolated Rust sandbox. Never writes Python registry,
/// original prepared cache, project histories, or live Python job statuses.
pub async fn enqueue_project(
    state: Arc<AppState>,
    id: &str,
    retry_failed: bool,
) -> Result<(), ApiError> {
    if !state.writes_enabled {
        return Err(storage::conflict(
            "Подготовка видео разрешена только в рабочем каталоге",
        ));
    }
    // Validate before constructing any disk path: retry removes a marker.
    let project = projects::existing_project(&state, id)?;
    let pause_marker = projects::project_root(&state)
        .join(id)
        .join(".preparation-paused");
    if retry_failed {
        state.preparation.cancelled_projects.lock().await.remove(id);
        if pause_marker.exists() {
            fs::remove_file(&pause_marker).map_err(|error| {
                storage::internal(format!("Cannot resume preparation: {error}"))
            })?;
        }
    } else if pause_marker.is_file()
        || state
            .preparation
            .cancelled_projects
            .lock()
            .await
            .contains(id)
    {
        // A status poll must never start a job after explicit cancellation,
        // even when the development server was restarted.
        return Ok(());
    }
    let videos = project["videos"]
        .as_array()
        .ok_or_else(|| storage::internal("Invalid project videos"))?;
    for name in videos.iter().filter_map(Value::as_str) {
        let plan = plan_for_async(state.clone(), id, name).await?;
        if reused_async(&plan)
            .await
            .map_err(storage::internal)?
            .is_some()
        {
            continue;
        }
        if !retry_failed
            && manifest_for(&plan).is_some_and(|m| m["status"].as_str() == Some("failed"))
        {
            continue;
        }
        let key = cache_id(&plan.identity);
        let mut jobs = state.preparation.jobs.lock().await;
        if let Some(existing) = jobs.get_mut(&key) {
            existing.subscribed_projects.insert(id.to_owned());
            continue;
        }
        let (tx, rx) = watch::channel(false);
        let progress = Arc::new(Mutex::new(Progress::queued()));
        jobs.insert(
            key.clone(),
            ActiveJob {
                subscribed_projects: HashSet::from([id.to_owned()]),
                cancel: tx,
                progress: progress.clone(),
            },
        );
        // Queued status stays in memory until an exclusive output lock is
        // acquired. Publishing a queued manifest without a lock could corrupt
        // another Rust process's actively running preparation progress.
        tokio::spawn(finished(state.clone(), key, plan, rx, progress));
    }
    Ok(())
}

pub async fn cancel_project(state: Arc<AppState>, id: &str) -> Result<usize, ApiError> {
    projects::existing_project(&state, id)?;
    if !state.writes_enabled {
        return Err(storage::conflict(
            "Отмена доступна только для проектов текущей студии",
        ));
    }
    // Persist this user pause only in the isolated Rust sandbox.
    let pause_marker = projects::project_root(&state)
        .join(id)
        .join(".preparation-paused");
    fs::write(&pause_marker, b"paused")
        .map_err(|error| storage::internal(format!("Cannot save preparation pause: {error}")))?;
    state
        .preparation
        .cancelled_projects
        .lock()
        .await
        .insert(id.to_owned());
    let mut jobs = state.preparation.jobs.lock().await;
    let mut affected = 0;
    for job in jobs.values_mut() {
        if job.subscribed_projects.remove(id) {
            affected += 1;
            if job.subscribed_projects.is_empty() {
                let _ = job.cancel.send(true);
            }
        }
    }
    Ok(affected)
}

pub(crate) async fn collect_status(state: Arc<AppState>, id: &str) -> Result<Value, ApiError> {
    let project = projects::existing_project(&state, id)?;
    let videos = project["videos"]
        .as_array()
        .ok_or_else(|| storage::internal("Project videos malformed"))?;
    let mut rows = Vec::new();
    let mut ready = 0usize;
    for video in videos {
        let Some(name) = video.as_str() else {
            continue;
        };
        let plan = plan_for_async(state.clone(), id, name).await?;
        let (mut state_name, mut progress, mut error, mut size_mb, mut preparation) =
            ("missing".to_owned(), 0.0, String::new(), 0.0, "pending");

        match reused_async(&plan).await.map_err(storage::internal)? {
            Some(PreparationReuse::Original) => {
                state_name = "ready".into();
                progress = 100.0;
                preparation = "not_required";
                size_mb = plan
                    .source
                    .metadata()
                    .map(|m| m.len() as f64 / 1048576.0)
                    .unwrap_or(0.0);
            }
            Some(PreparationReuse::Cached) => {
                state_name = "ready".into();
                progress = 100.0;
                preparation = "cached";
                size_mb = plan
                    .output
                    .metadata()
                    .map(|m| m.len() as f64 / 1048576.0)
                    .unwrap_or(0.0);
            }
            None => {
                let key = cache_id(&plan.identity);
                let active = {
                    let jobs = state.preparation.jobs.lock().await;
                    jobs.get(&key).map(|job| job.progress.clone())
                };
                if let Some(progress_arc) = active {
                    let value = progress_arc.lock().await.clone();
                    state_name = value.state;
                    progress = value.progress;
                    error = value.error;
                } else if let Some(manifest) = manifest_for(&plan) {
                    // A crash must not leave "preparing" visible indefinitely.
                    if manifest["status"].as_str() == Some("failed") {
                        state_name = "failed".into();
                        error = manifest["error"].as_str().unwrap_or_default().to_owned();
                    }
                }
            }
        }

        if state_name == "ready" {
            ready += 1;
        }
        rows.push(json!({
            "video": name,
            "state": state_name,
            "progress": (progress * 10.0).round()/10.0,
            "width": plan.width,
            "fps": 30,
            "error": error,
            "size_mb": (size_mb * 10.0).round()/10.0,
            "preparation": preparation,
        }));
    }
    Ok(json!({
        "project_id": id,
        "videos": rows,
        "ready": ready,
        "total": videos.len(),
    }))
}

pub async fn get_preparation(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult<Value> {
    let id = query.get("project_id").cloned().unwrap_or_default();
    if state.writes_enabled {
        enqueue_project(state.clone(), &id, false).await?;
    }
    Ok(Json(collect_status(state.clone(), &id).await?))
}

pub async fn retry_preparation(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    let id = request["project_id"]
        .as_str()
        .ok_or_else(|| storage::bad_request("Укажите проект"))?;
    enqueue_project(state.clone(), id, true).await?;
    Ok(Json(collect_status(state.clone(), id).await?))
}

pub async fn cancel_preparation(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    if !state.writes_enabled {
        return Err(storage::conflict(
            "Нет разрешения на отмену задач этой студии",
        ));
    }
    let id = request["project_id"]
        .as_str()
        .ok_or_else(|| storage::bad_request("Укажите проект"))?;
    let cancelled = cancel_project(state, id).await?;
    Ok(Json(json!({"cancelled": cancelled, "project_id": id})))
}

/// Run project-independent Quick Reel video preparation using the SAME
/// content-addressed cache, exclusive file lock, FFmpeg encoder and global
/// concurrency limit as ordinary Rust project preparation.
///
/// Quick Reel owns the orchestration and status, not the project's registry.
/// The caller must verify the source file identity again before inference.
pub(crate) async fn prepare_standalone(
    state: Arc<AppState>,
    plan: PreparationPlan,
    mut cancel: watch::Receiver<bool>,
    progress: Arc<Mutex<Progress>>,
) -> Result<(), String> {
    if !state.writes_enabled {
        return Err("Video preparation requires an isolated project directory".into());
    }
    if reused_async(&plan).await?.is_some() {
        update(&progress, "ready", 100.0, "").await;
        return Ok(());
    }
    let permit = tokio::select! {
        acquired=state.preparation.concurrency.clone().acquire_owned()=>{
            acquired.map_err(|_|"Preparation queue is stopped".to_owned())?
        },
        result=cancel.changed()=>{
            if result.is_err() || *cancel.borrow() {
                return Err("cancelled".into());
            }
            state.preparation.concurrency.clone().acquire_owned().await
                .map_err(|_|"Preparation queue is stopped".to_owned())?
        }
    };
    let result = run_one(&plan, cancel, &progress).await;
    if let Err(error) = &result {
        let failed_plan = plan.clone();
        let state_name = if error == "cancelled" {
            "cancelled"
        } else {
            "failed"
        };
        let message = if error == "cancelled" {
            String::new()
        } else {
            error.clone()
        };
        let _ = tokio::task::spawn_blocking(move || {
            publish_failure_if_uncontended(&failed_plan, state_name, &message);
        })
        .await;
    }
    drop(permit);
    result
}

#[cfg(test)]
mod tests {
    use super::percent_from_ffmpeg;

    #[tokio::test]
    async fn real_ffmpeg_preparation_sha_receipt_bitflip_and_cancel_fail_closed() {
        use super::*;
        use sha2::{Digest, Sha256};
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            eprintln!("SKIP real FFmpeg preparation SHA test without FFmpeg");
            return;
        }
        let sandbox =
            std::env::temp_dir().join(format!("volley-prep-fault83-{}", Uuid::new_v4().simple()));
        fs::create_dir_all(&sandbox).unwrap();
        let source = sandbox.join("source_25fps.mp4");
        let result = std::process::Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x180:rate=25:duration=4",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p",
                "-an",
            ])
            .arg(&source)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{:?}",
            String::from_utf8_lossy(&result.stderr)
        );
        let original = fs::read(&source).unwrap();
        let original_sha = format!("{:x}", Sha256::digest(&original));
        let plan = PreparationPlan::new(&source, &sandbox, Some(256)).unwrap();
        assert!(
            !plan.already_analysis_ready,
            "25fps must require preparation"
        );
        let (_tx, rx) = watch::channel(false);
        let progress = Arc::new(Mutex::new(Progress::queued()));
        run_one(&plan, rx, &progress).await.unwrap();
        assert_eq!(plan.reused(), Some(PreparationReuse::Cached));
        let receipt = storage::read_json(&plan.manifest).unwrap().unwrap();
        let output = fs::read(&plan.output).unwrap();
        assert_eq!(
            receipt["cache_size_bytes"].as_u64(),
            Some(output.len() as u64)
        );
        assert_eq!(
            receipt["cache_sha256"],
            format!("{:x}", Sha256::digest(&output))
        );
        let mut altered = output.clone();
        altered[12] ^= 0x40;
        fs::write(&plan.output, &altered).unwrap();
        assert_eq!(
            plan.reused(),
            None,
            "Same-size corrupted MP4 must not be reused"
        );
        let (_tx, rx) = watch::channel(false);
        run_one(&plan, rx, &progress).await.unwrap();
        assert_eq!(
            plan.reused(),
            Some(PreparationReuse::Cached),
            "Corrupt entry must be rebuilt and rehashed"
        );
        assert_eq!(
            fs::read(&plan.output).unwrap(),
            output,
            "Same original has deterministic FFmpeg output"
        );
        fs::write(&plan.output, &altered).unwrap();
        let (tx, rx) = watch::channel(false);
        tx.send(true).unwrap();
        assert_eq!(
            run_one(&plan, rx, &progress).await.unwrap_err(),
            "cancelled"
        );
        assert_eq!(
            plan.reused(),
            None,
            "Cancellation never blesses the corrupted cached output"
        );
        assert_eq!(
            format!("{:x}", Sha256::digest(fs::read(&source).unwrap())),
            original_sha,
            "Original user video must never be changed"
        );
        fs::remove_dir_all(sandbox).unwrap();
    }
    #[tokio::test]
    async fn cancelling_running_ffmpeg_reaps_process_without_partial_or_cache_receipt() {
        use super::*;
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }
        let root = std::env::temp_dir().join(format!("volley-ffmpeg-cancel83-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("long_source.mp4");
        let result = std::process::Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x180:rate=25:duration=8",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p",
                "-an",
            ])
            .arg(&source)
            .output()
            .unwrap();
        assert!(result.status.success());
        let plan = PreparationPlan::new(&source, &root, Some(1280)).unwrap();
        assert!(!plan.already_analysis_ready);
        let size_before = fs::metadata(&source).unwrap().len();
        let (tx, rx) = watch::channel(false);
        let progress = Arc::new(Mutex::new(Progress::queued()));
        let cloned = plan.clone();
        let progress2 = progress.clone();
        let job = tokio::spawn(async move { run_one(&cloned, rx, &progress2).await });
        let mut active = false;
        for _ in 0..250 {
            if progress.lock().await.state == "preparing" {
                active = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(8)).await;
        }
        assert!(active, "FFmpeg did not enter active preparation stage");
        tokio::time::sleep(Duration::from_millis(40)).await;
        tx.send(true).unwrap();
        let outcome = tokio::time::timeout(Duration::from_secs(12), job)
            .await
            .expect("Cancelled FFmpeg failed to terminate promptly")
            .unwrap()
            .unwrap_err();
        assert_eq!(outcome, "cancelled");
        assert!(
            !plan.output.exists(),
            "Cancelled transcode published cache media"
        );
        assert_ne!(plan.reused(), Some(PreparationReuse::Cached));
        let partials = fs::read_dir(plan.output.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".partial.mp4"))
            .count();
        assert_eq!(partials, 0, "Cancelled FFmpeg leaked a partial MP4");
        assert_eq!(fs::metadata(&source).unwrap().len(), size_before);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn ffmpeg_progress_is_monotone_bounded() {
        assert_eq!(
            percent_from_ffmpeg("out_time=00:00:15.000000", 30.0),
            Some(50.0)
        );
        assert_eq!(
            percent_from_ffmpeg("out_time=00:00:44.000000", 30.0),
            Some(99.5)
        );
        assert_eq!(percent_from_ffmpeg("out_time=nope", 30.0), None);
    }
}
