//! One isolated Rust montage job runner for both native and legacy CPU rally
//! selection. FFmpeg export is owned by volley-media; pixels are always read
//! from the original project recording, never from analysis proxies.
use crate::{
    preparation, projects,
    storage::{self, ApiError, ApiResult},
    AppState,
};
use axum::{
    extract::{Query, State},
    http::HeaderMap,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Instant,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::{watch, Mutex},
};
use uuid::Uuid;
use volley_media::{
    montage::{
        can_stream_copy, compose_args_for_layout, concat_playlist, native_clip_format,
        output_profile_for_layout, soundtrack_args, soundtrack_is_audio, verify_mp4_quality,
        verify_original_segment, OriginalClip, SelectedClip, VideoLayout,
    },
    probe,
};

const MAX_CLIPS: usize = 80;
const MAX_TOTAL_SECONDS: f64 = 600.0;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MontageRequest {
    pub project_id: String,
    pub clips: Vec<SelectedClip>,
    #[serde(default)]
    pub audio_file: Option<String>,
    #[serde(default = "default_volume")]
    pub audio_volume: f64,
    #[serde(default)]
    pub encoder: Option<String>,
    #[serde(default)]
    pub layout: VideoLayout,
    #[serde(skip)]
    pub(crate) verified_highlights: Option<Value>,
}

fn default_volume() -> f64 {
    40.0
}

#[derive(Clone)]
struct JobStatus {
    id: String,
    project_id: String,
    state: &'static str,
    stage: String,
    completed: usize,
    total: usize,
    overall_percent: f64,
    error: String,
    output: Option<PathBuf>,
}

impl JobStatus {
    fn as_json(&self) -> Value {
        json!({
            "id":self.id,
            "project_id":self.project_id,
            "state":self.state,
            "stage":self.stage,
            "completed_stages":self.completed,
            "total_stages":self.total,
            "overall_percent":self.overall_percent,
            "error":self.error,
            "media_url": self.output.as_ref().map(|_| {
                format!("/api/montage/native/media?project_id={}&job_id={}", self.project_id, self.id)
            }),
        })
    }
}

struct MontageJob {
    status: Arc<Mutex<JobStatus>>,
    cancel: watch::Sender<bool>,
}

pub struct MontageManager {
    current: Mutex<Option<MontageJob>>,
}

impl MontageManager {
    pub fn new() -> Self {
        Self {
            current: Mutex::new(None),
        }
    }
}

struct Planned {
    project_id: String,
    job_id: String,
    clips: Vec<OriginalClip>,
    output_dir: PathBuf,
    audio: Option<PathBuf>,
    audio_volume: f64,
    encoder: String,
    verified_highlights: Option<Value>,
    layout: VideoLayout,
}

fn validate_audio(state: &AppState, request: &MontageRequest) -> Result<Option<PathBuf>, ApiError> {
    let Some(filename) = request.audio_file.as_deref() else {
        return Ok(None);
    };
    // Music must first be added by a trusted upload/import implementation to
    // the isolated Rust music directory. Never accept arbitrary local paths.
    if filename.len() > 240
        || filename.is_empty()
        || filename.contains('/')
        || filename.contains('\\')
        || filename.contains('\0')
    {
        return Err(storage::bad_request("Неправильное имя аудиофайла"));
    }
    let extension = Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !matches!(
        extension.as_str(),
        "mp3" | "m4a" | "wav" | "flac" | "aac" | "ogg" | "opus"
    ) {
        return Err(storage::bad_request("Неподдерживаемый формат музыки"));
    }
    let root = state.data_root.join("uploads").join("music");
    let path = root
        .join(filename)
        .canonicalize()
        .map_err(|_| storage::bad_request("Аудиотрек не найден в Rust-хранилище"))?;
    let root = root
        .canonicalize()
        .map_err(|_| storage::bad_request("Каталог загруженной музыки не найден"))?;
    if !path.starts_with(&root) || !path.is_file() {
        return Err(storage::bad_request("Недоступный аудиотрек"));
    }
    if !soundtrack_is_audio(&path).map_err(storage::bad_request)? {
        return Err(storage::bad_request("Аудиотрек не содержит звук"));
    }
    Ok(Some(path))
}

fn prepare_request(state: &AppState, request: MontageRequest) -> Result<Planned, ApiError> {
    if !state.writes_enabled {
        return Err(storage::conflict(
            "Выполнение Rust-монтажа разрешено только в отдельном хранилище разработки",
        ));
    }
    let project = projects::existing_project(state, &request.project_id)?;
    if request.clips.is_empty() || request.clips.len() > MAX_CLIPS {
        return Err(storage::bad_request("Выберите от 1 до 80 эпизодов"));
    }
    if !request.audio_volume.is_finite() || !(0.0..=100.0).contains(&request.audio_volume) {
        return Err(storage::bad_request("Громкость музыки должна быть 0–100"));
    }
    let encoder = request.encoder.as_deref().unwrap_or("auto");
    if !matches!(encoder, "auto" | "cpu" | "gpu") {
        return Err(storage::bad_request("Кодирование: auto, cpu или gpu"));
    }
    let allowed = project["videos"]
        .as_array()
        .ok_or_else(|| storage::bad_request("В проекте нет списка исходников"))?
        .iter()
        .filter_map(Value::as_str)
        .collect::<HashSet<_>>();
    let mut plans: HashMap<String, PathBuf> = HashMap::new();
    let mut clips = Vec::new();
    let mut total = 0.0;
    for clip in &request.clips {
        if !allowed.contains(clip.video.as_str()) {
            return Err(storage::bad_request(
                "Видео не принадлежит выбранному проекту",
            ));
        }
        if clip.end_frame < clip.start_frame || clip.end_frame - clip.start_frame > 30 * 600 {
            return Err(storage::bad_request("Неправильные границы эпизода"));
        }
        let start = clip.analysis_offset_sec.unwrap_or(0.0);
        if !start.is_finite() || start < 0.0 {
            return Err(storage::bad_request("Некорректное смещение анализа"));
        }
        let source = if let Some(path) = plans.get(&clip.video) {
            path.clone()
        } else {
            let plan = preparation::plan_for(state, &request.project_id, &clip.video)?;
            let path = plan.source;
            plans.insert(clip.video.clone(), path.clone());
            path
        };
        let original =
            OriginalClip::from_analysis(&source, clip.start_frame, clip.end_frame, start)
                .map_err(storage::bad_request)?;
        total += original.duration_sec();
        if total > MAX_TOTAL_SECONDS {
            return Err(storage::bad_request(
                "Общая длительность выбранных эпизодов превышает 600 секунд",
            ));
        }
        clips.push(original);
    }
    if total < 1.0 {
        return Err(storage::bad_request(
            "Слишком короткий монтаж (менее одной секунды)",
        ));
    }
    let audio = validate_audio(state, &request)?;
    let job_id = format!("montage_{}", Uuid::new_v4().simple());
    let output_dir = state
        .data_root
        .join("projects")
        .join(&request.project_id)
        .join("montages")
        .join(&job_id);
    if output_dir.exists() {
        return Err(storage::conflict("Папка монтажа уже существует"));
    }
    Ok(Planned {
        project_id: request.project_id,
        job_id,
        clips,
        output_dir,
        audio,
        audio_volume: request.audio_volume,
        encoder: encoder.to_owned(),
        verified_highlights: request.verified_highlights,
        layout: request.layout,
    })
}

fn progress_line(line: &str, expected_sec: f64) -> Option<f64> {
    let value = line.strip_prefix("out_time=")?;
    let parts = value.split(':').collect::<Vec<_>>();
    if parts.len() != 3 || expected_sec <= 0.0 {
        return None;
    }
    let hours = parts[0].parse::<u32>().ok()?;
    let mins = parts[1].parse::<u32>().ok()?;
    let secs = parts[2].parse::<f64>().ok()?;
    if mins > 59 || !(0.0..60.0).contains(&secs) {
        return None;
    }
    let done = hours as f64 * 3600.0 + mins as f64 * 60.0 + secs;
    Some((done / expected_sec).clamp(0.0, 1.0))
}

async fn advance(status: &Arc<Mutex<JobStatus>>, index: usize, stage: &str, percent: f64) {
    let mut info = status.lock().await;
    info.completed = index;
    info.stage = stage.into();
    let incoming =
        (((index as f64 + percent.clamp(0.0, 1.0)) / info.total.max(1) as f64) * 100.0 * 100.0)
            .round()
            / 100.0;
    // Encoder fallback must not make UI progress jump backwards.
    info.overall_percent = info.overall_percent.max(incoming);
}

async fn run_ffmpeg(
    mut args: Vec<String>,
    seconds: f64,
    status: &Arc<Mutex<JobStatus>>,
    index: usize,
    stage: &str,
    cancel: &mut watch::Receiver<bool>,
) -> Result<(), String> {
    if *cancel.borrow() {
        return Err("cancelled".into());
    }
    let output = args.pop().ok_or("FFmpeg args missing output")?;
    args.extend(["-progress".into(), "pipe:1".into(), "-nostats".into()]);
    args.push(output);
    advance(status, index, stage, 0.0).await;
    let mut child = Command::new("ffmpeg");
    child
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // FFmpeg can produce a long error; stderr is independently drained to
    // prevent stdout reader blocking on full OS pipe buffers.
    let mut child = child
        .spawn()
        .map_err(|e| format!("FFmpeg недоступен: {e}"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or("FFmpeg stderr pipe unavailable")?;
    let stderr_task = tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut input = stderr;
        let mut tail = Vec::with_capacity(8192);
        let mut chunk = [0_u8; 4096];
        // Always drain stderr, but never retain more than 16 KiB.
        loop {
            match input.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    tail.extend_from_slice(&chunk[..count]);
                    if tail.len() > 16 * 1024 {
                        tail.drain(..tail.len() - 16 * 1024);
                    }
                }
            }
        }
        let utf = String::from_utf8_lossy(&tail);
        utf.chars()
            .rev()
            .take(700)
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>()
    });
    let stdout = child
        .stdout
        .take()
        .ok_or("FFmpeg stdout pipe unavailable")?;
    let mut lines = BufReader::new(stdout).lines();
    let started = Instant::now();
    let result: Result<(), String> = async {
        loop {
            tokio::select! {
                line = lines.next_line() => match line {
                    Ok(Some(line)) => {
                        if let Some(frac) = progress_line(&line, seconds) {
                            advance(status, index, stage, frac).await;
                        }
                    }
                    Ok(None) => break,
                    Err(e) => return Err(format!("FFmpeg progress stream: {e}"))
                },
                changed = cancel.changed() => {
                    if changed.is_ok() && *cancel.borrow() {
                        return Err("cancelled".into());
                    }
                },
            }
        }
        let code = child
            .wait()
            .await
            .map_err(|e| format!("FFmpeg wait: {e}"))?;
        if code.success() {
            Ok(())
        } else {
            Err(format!("FFmpeg не завершился успешно ({code})"))
        }
    }
    .await;
    if result.is_err() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    let detail = stderr_task.await.unwrap_or_default();
    if result.is_ok() {
        advance(status, index, stage, 1.0).await;
    }
    result.map_err(|e| {
        if e == "cancelled" {
            e
        } else {
            format!(
                "{e}; {detail}; elapsed={:.1}s",
                started.elapsed().as_secs_f64()
            )
        }
    })
}

async fn encode_with_fallback(
    primary: impl Fn(bool) -> Vec<String>,
    duration: f64,
    encoder: &str,
    status: &Arc<Mutex<JobStatus>>,
    index: usize,
    stage: &str,
    cancel: &mut watch::Receiver<bool>,
) -> Result<(), String> {
    let use_gpu = encoder != "cpu" && (encoder == "gpu" || duration >= 7.0);
    if use_gpu {
        let result = run_ffmpeg(primary(true), duration, status, index, stage, cancel).await;
        match result {
            Ok(()) => return Ok(()),
            Err(reason) if reason == "cancelled" => return Err(reason),
            Err(_) => {
                // Same as existing Python runner: fail over to high-quality
                // CPU encoder if NVENC is unavailable or rejects this input.
            }
        }
    }
    run_ffmpeg(primary(false), duration, status, index, stage, cancel).await
}

fn validate_rendered_profile(path: &Path, profile: (u32, u32, f64)) -> Result<(), String> {
    let (width, height, codec, fps) = native_clip_format(path)?;
    if (width, height) != (profile.0, profile.1) || codec != "h264" || (fps - profile.2).abs() > 0.1
    {
        return Err(format!(
            "FFmpeg output lost original resolution or FPS: got {width}x{height}/{fps:.5}, expected {}x{}/{:.5}",
            profile.0,profile.1,profile.2,
        ));
    }
    Ok(())
}
fn validate_rendered(path: &Path, expected_duration: f64) -> Result<(), String> {
    let stat = fs::metadata(path).map_err(|e| format!("Ролик не создан: {e}"))?;
    if !stat.is_file() || stat.len() < 1024 {
        return Err("Пустой файл результата FFmpeg".into());
    }
    let actual = probe(path)?.duration_sec;
    if !actual.is_finite()
        || actual <= 0.0
        || actual < (expected_duration - 0.35).max(0.01)
        || actual > expected_duration + 0.75
    {
        return Err(format!(
            "Неправильная длительность результата: {actual:.3} c, план {expected_duration:.3} c"
        ));
    }
    Ok(())
}

async fn render(plan: Planned, status: Arc<Mutex<JobStatus>>, mut cancel: watch::Receiver<bool>) {
    let receipt_path = plan.output_dir.join("selection.json");
    let output = plan.output_dir.join("montage.mp4");
    let segments = plan.output_dir.join("_render_temp");
    let result: Result<(), String> = async {
        fs::create_dir_all(&segments).map_err(|e| format!("Нельзя создать папку монтажа: {e}"))?;
        let mut paths = Vec::new();
        let mut formats = Vec::new();
        let total_duration: f64 = plan.clips.iter().map(OriginalClip::duration_sec).sum();
        for (index, clip) in plan.clips.iter().enumerate() {
            if *cancel.borrow() {
                return Err("cancelled".into());
            }
            let target = segments.join(format!("segment_{index:04}.mp4"));
            let seconds = clip.duration_sec();
            encode_with_fallback(
                |hardware| clip.extract_args(&target, hardware),
                seconds,
                &plan.encoder,
                &status,
                index,
                &format!("Исходный фрагмент {} из {}", index + 1, plan.clips.len()),
                &mut cancel,
            )
            .await?;
            validate_rendered(&target, seconds)?;
            let actual_format = native_clip_format(&target)?;
            verify_original_segment(clip, &actual_format)?;
            formats.push(actual_format);
            paths.push(target);
        }

        let final_profile = output_profile_for_layout(&formats, plan.layout)?;
        let join_index = paths.len();
        let joined = segments.join("joined.mp4");
        let playlist = segments.join("sources.concat.txt");
        fs::write(&playlist, concat_playlist(&paths)?)
            .map_err(|e| format!("Нельзя сохранить список клипов: {e}"))?;
        let mut used_copy = false;
        if can_stream_copy(&formats)
            && (plan.layout == VideoLayout::Original
                || formats[0].0 == final_profile.0 && formats[0].1 == final_profile.1)
        {
            let args = compose_args_for_layout(
                &paths,
                &formats,
                &joined,
                Some(&playlist),
                false,
                plan.layout,
            )?;
            match run_ffmpeg(
                args,
                total_duration,
                &status,
                join_index,
                "Сборка исходных фрагментов",
                &mut cancel,
            )
            .await
            {
                Ok(()) => used_copy = true,
                Err(e) if e == "cancelled" => return Err(e),
                Err(_) => {
                    let _ = fs::remove_file(&joined);
                }
            }
        }
        if !used_copy {
            // Construct and validate both encoder plans BEFORE starting
            // FFmpeg. Never silently run an empty command on plan failure.
            let gpu_args =
                compose_args_for_layout(&paths, &formats, &joined, None, true, plan.layout)?;
            let cpu_args =
                compose_args_for_layout(&paths, &formats, &joined, None, false, plan.layout)?;
            encode_with_fallback(
                |gpu| {
                    if gpu {
                        gpu_args.clone()
                    } else {
                        cpu_args.clone()
                    }
                },
                total_duration,
                &plan.encoder,
                &status,
                join_index,
                "Сборка исходных фрагментов",
                &mut cancel,
            )
            .await?;
        }
        validate_rendered(&joined, total_duration)?;
        validate_rendered_profile(&joined, final_profile)?;
        verify_mp4_quality(&joined, final_profile, total_duration, false)?;
        let final_path = if let Some(audio) = &plan.audio {
            let with_music = segments.join("with_music.mp4");
            let args = soundtrack_args(
                &joined,
                audio,
                &with_music,
                total_duration,
                plan.audio_volume,
            )?;
            run_ffmpeg(
                args,
                total_duration,
                &status,
                join_index + 1,
                "Наложение музыки",
                &mut cancel,
            )
            .await?;
            validate_rendered(&with_music, total_duration)?;
            validate_rendered_profile(&with_music, final_profile)?;
            verify_mp4_quality(&with_music, final_profile, total_duration, true)?;
            with_music
        } else {
            joined
        };
        let quality = verify_mp4_quality(
            &final_path,
            final_profile,
            total_duration,
            plan.audio.is_some(),
        )?;
        if *cancel.borrow() {
            return Err("cancelled".into());
        }
        fs::rename(final_path, &output)
            .map_err(|e| format!("Не удалось сохранить готовый монтаж: {e}"))?;
        let report = json!({
            "schema":"volley-rust-montage/1",
            "id":plan.job_id,
            "project_id":plan.project_id,
            "render_source":"original_recording",
            "original_resolution_verified":plan.layout==VideoLayout::Original,
            "source_clips_native_resolution_verified":true,
            "layout":plan.layout,
            "entire_frame_visible_without_cropping":true,
            "mp4_quality":quality,
            "output_dimensions":{"width":final_profile.0,"height":final_profile.1},
            "output_fps":final_profile.2,
            "quality_policy":if plan.layout==VideoLayout::Vertical {
                "portrait_9x16_fit_letterbox_lanczos_h264_crf15_nvenc_cq17"
            }else{
                "native_resolution_no_downsampling_h264_crf15_nvenc_cq17"
            },
            "verified_highlights":plan.verified_highlights,
            "duration_sec":total_duration,
            "selected":plan.clips.iter().map(OriginalClip::receipt).collect::<Vec<_>>(),
            "music":plan.audio.as_ref().map(|path| json!({
                "source":path.file_name().and_then(|name|name.to_str()),
                "volume":plan.audio_volume
            })),
            "file":"montage.mp4"
        });
        crate::storage::write_json_atomic(&receipt_path, &report)
            .map_err(|e| format!("Cannot save montage manifest: {e:?}"))?;
        // Internal transcoding files can be reclaimed without touching
        // the final output or the input videos.
        let _ = fs::remove_dir_all(&segments);
        Ok(())
    }
    .await;

    let mut data = status.lock().await;
    match result {
        Ok(()) => {
            data.state = "complete";
            data.overall_percent = 100.0;
            data.completed = data.total;
            data.stage = "Монтаж готов".into();
            data.output = Some(output);
        }
        Err(error) if error == "cancelled" => {
            data.state = "cancelled";
            data.stage = "Отменено".into();
            // Only the unique job-owned isolated directory is cleaned; never original media.
            let _ = fs::remove_dir_all(&plan.output_dir);
        }
        Err(error) => {
            data.state = "failed";
            data.stage = "Ошибка монтажа".into();
            data.error = error;
        }
    }
}

pub async fn start_native_montage(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<MontageRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    // Check active jobs first; do not block status/cancel while FFprobe
    // reads source metadata. Recheck the slot before publishing the job.
    let current = state.montage.current.lock().await;
    if let Some(existing) = current.as_ref() {
        let snapshot = existing.status.lock().await;
        if matches!(snapshot.state, "queued" | "running") {
            return Err(storage::conflict("Другой монтаж ещё выполняется"));
        }
    }
    drop(current);
    let selected = tokio::task::spawn_blocking({
        let state = state.clone();
        move || prepare_request(&state, request)
    })
    .await
    .map_err(|e| storage::internal(format!("Montage planning task failed: {e}")))??;
    let mut current = state.montage.current.lock().await;
    if let Some(existing) = current.as_ref() {
        if matches!(existing.status.lock().await.state, "queued" | "running") {
            return Err(storage::conflict("Другой монтаж уже начал работу"));
        }
    }
    // Archive may have started while FFprobe planned this job; reject it
    // before publishing a new worker in the montage slot.
    projects::existing_project(&state, &selected.project_id)?;
    let total = selected.clips.len() + 1 + usize::from(selected.audio.is_some());
    let job_id = selected.job_id.clone();
    let status = Arc::new(Mutex::new(JobStatus {
        id: job_id.clone(),
        project_id: selected.project_id.clone(),
        state: "queued",
        stage: "Ожидание запуска".into(),
        completed: 0,
        total,
        overall_percent: 0.0,
        error: String::new(),
        output: None,
    }));
    let (tx, rx) = watch::channel(false);
    *current = Some(MontageJob {
        status: status.clone(),
        cancel: tx,
    });
    tokio::spawn(async move {
        status.lock().await.state = "running";
        render(selected, status, rx).await;
    });
    Ok(Json(json!({"job_id":job_id, "status":"queued"})))
}

pub async fn native_status(State(state): State<Arc<AppState>>) -> Json<Value> {
    let current = state.montage.current.lock().await;
    if let Some(job) = current.as_ref() {
        Json(job.status.lock().await.as_json())
    } else {
        Json(json!({"state":"idle"}))
    }
}

pub async fn native_cancel(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    if !state.writes_enabled {
        return Err(storage::conflict(
            "Rust-монтаж не запущен в изолированном режиме",
        ));
    }
    let id = request["job_id"].as_str().unwrap_or_default();
    let current = state.montage.current.lock().await;
    let job = current
        .as_ref()
        .ok_or_else(|| storage::not_found("Задача монтажа не найдена"))?;
    if job.status.lock().await.id != id {
        return Err(storage::not_found("Задача монтажа не найдена"));
    }
    let active = matches!(job.status.lock().await.state, "queued" | "running");
    if active {
        let _ = job.cancel.send(true);
    }
    Ok(Json(json!({"cancel_requested":active,"job_id":id})))
}

/// Cancel an active montage for exactly this project before archiving its
/// owned directory. Unrelated montages must not be interrupted.
pub(crate) async fn stop_for_project_archive(
    state: &AppState,
    project_id: &str,
) -> Result<bool, ApiError> {
    let status = {
        let current = state.montage.current.lock().await;
        if let Some(job) = current.as_ref() {
            let snapshot = job.status.lock().await;
            if snapshot.project_id == project_id && matches!(snapshot.state, "queued" | "running") {
                let _ = job.cancel.send(true);
                Some(job.status.clone())
            } else {
                None
            }
        } else {
            None
        }
    };
    let Some(status) = status else {
        return Ok(false);
    };
    for _ in 0..120 {
        if !matches!(status.lock().await.state, "queued" | "running") {
            return Ok(true);
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    Err(storage::conflict(
        "Монтаж проекта ещё останавливается; архивирование отменено",
    ))
}

fn completed_montage(
    state: &AppState,
    project_id: &str,
    id: &str,
) -> Result<(PathBuf, Value), ApiError> {
    projects::existing_project(state, project_id)?;
    if id.len() != 40
        || !id.starts_with("montage_")
        || !id.as_bytes()[8..].iter().all(u8::is_ascii_hexdigit)
    {
        return Err(storage::bad_request("Некорректный идентификатор монтажа"));
    }
    let folder = state
        .data_root
        .join("projects")
        .join(project_id)
        .join("montages")
        .join(id);
    let receipt = storage::read_json(&folder.join("selection.json"))?
        .ok_or_else(|| storage::not_found("Монтаж не найден"))?;
    if receipt["schema"].as_str() != Some("volley-rust-montage/1")
        || receipt["id"].as_str() != Some(id)
        || receipt["project_id"].as_str() != Some(project_id)
        || receipt["render_source"].as_str() != Some("original_recording")
        || receipt["file"].as_str() != Some("montage.mp4")
    {
        return Err(storage::not_found("Данные монтажа повреждены"));
    }
    let folder = folder
        .canonicalize()
        .map_err(|_| storage::not_found("Каталог монтажа не найден"))?;
    let output = folder
        .join("montage.mp4")
        .canonicalize()
        .map_err(|_| storage::not_found("MP4 не найден"))?;
    // Never follow an exported symlink outside the validated montage folder.
    if !output.starts_with(&folder) {
        return Err(storage::not_found("MP4 вне каталога монтажа"));
    }
    let meta = output
        .metadata()
        .map_err(|_| storage::not_found("MP4 не найден"))?;
    if !meta.is_file() || meta.len() < 1024 {
        return Err(storage::not_found("MP4 не найден"));
    }
    Ok((output, receipt))
}

fn list_montages(state: &AppState, project_id: &str) -> Result<Vec<Value>, ApiError> {
    projects::existing_project(state, project_id)?;
    let dir = state
        .data_root
        .join("projects")
        .join(project_id)
        .join("montages");
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(Vec::new());
    };
    let mut results = Vec::new();
    for entry in entries.flatten() {
        let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Ok((output, receipt)) = completed_montage(state, project_id, &id) else {
            continue;
        };
        let Ok(stat) = output.metadata() else {
            continue;
        };
        let url = format!("/api/montage/native/media?project_id={project_id}&job_id={id}");
        results.push(json!({
            "id": id,
            "name": "montage.mp4",
            "project_id": project_id,
            "duration_sec": receipt["duration_sec"],
            "verified_highlights":receipt["verified_highlights"],
            "render_source": "original_recording",
            "layout":receipt["layout"],
            "mp4_quality":receipt["mp4_quality"],
            "clips": receipt["selected"].as_array().map(Vec::len).unwrap_or(0),
            "music": receipt.get("music").filter(|value| !value.is_null()).is_some(),
            "size_mb": ((stat.len() as f64 / 1024.0 / 1024.0)*10.0).round()/10.0,
            "url": url,
            "download_url": url,
        }));
    }
    results.sort_by(|a, b| b["id"].as_str().cmp(&a["id"].as_str()));
    Ok(results)
}

pub async fn native_library(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult<Value> {
    let project_id = query
        .get("project_id")
        .ok_or_else(|| storage::bad_request("Укажите проект"))?
        .to_owned();
    let results = tokio::task::spawn_blocking(move || list_montages(&state, &project_id))
        .await
        .map_err(|error| storage::internal(format!("Montage listing failed: {error}")))??;
    Ok(Json(json!({"videos": results})))
}

pub async fn native_media(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<axum::http::Response<axum::body::Body>, ApiError> {
    let project_id = query
        .get("project_id")
        .ok_or_else(|| storage::bad_request("Укажите проект"))?
        .to_owned();
    let job_id = query
        .get("job_id")
        .ok_or_else(|| storage::bad_request("Укажите монтаж"))?
        .to_owned();
    let reading = state.clone();
    let path = tokio::task::spawn_blocking(move || {
        completed_montage(&reading, &project_id, &job_id).map(|(path, _)| path)
    })
    .await
    .map_err(|error| storage::internal(format!("Montage media lookup failed: {error}")))??;
    crate::playback::serve_mp4(path, &headers).await
}

/// Upload a soundtrack exclusively to the isolated Rust montage sandbox.
/// The submitted filename is never used as a filesystem path.
pub async fn native_upload_audio(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> ApiResult<Value> {
    use tokio::io::AsyncWriteExt;

    storage::validate_origin(&headers)?;
    if !state.writes_enabled {
        return Err(storage::conflict(
            "Загрузка музыки требует изолированного Rust-хранилища",
        ));
    }
    const LIMIT: usize = 90 * 1024 * 1024;
    let mut file = multipart
        .next_field()
        .await
        .map_err(|error| storage::bad_request(format!("Ошибка загрузки музыки: {error}")))?
        .ok_or_else(|| storage::bad_request("Аудиофайл не передан"))?;

    if file.name() != Some("file") {
        return Err(storage::bad_request("Ожидалось поле file"));
    }
    let origin = file.file_name().unwrap_or_default().to_owned();
    let extension = Path::new(&origin)
        .extension()
        .and_then(|text| text.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !matches!(
        extension.as_str(),
        "mp3" | "m4a" | "wav" | "flac" | "aac" | "ogg" | "opus"
    ) {
        return Err(storage::bad_request("Формат музыки не поддерживается"));
    }

    let directory = state.data_root.join("uploads").join("music");
    tokio::fs::create_dir_all(&directory)
        .await
        .map_err(|error| {
            storage::internal(format!("Cannot create Rust music directory: {error}"))
        })?;
    let filename = format!("track_{}.{}", Uuid::new_v4().simple(), extension);
    let target = directory.join(&filename);
    let temp = directory.join(format!(".{filename}.upload"));
    let mut output = tokio::fs::File::create(&temp)
        .await
        .map_err(|error| storage::internal(format!("Cannot create audio upload: {error}")))?;
    let mut uploaded = 0usize;
    let result: Result<(), ApiError> = async {
        while let Some(chunk) = file
            .chunk()
            .await
            .map_err(|error| storage::bad_request(format!("Audio stream failed: {error}")))?
        {
            uploaded = uploaded
                .checked_add(chunk.len())
                .ok_or_else(|| storage::bad_request("Аудиофайл слишком большой"))?;
            if uploaded > LIMIT {
                return Err(storage::bad_request("Музыка не должна превышать 90 МиБ"));
            }
            output.write_all(&chunk).await.map_err(|error| {
                storage::internal(format!("Cannot write uploaded audio: {error}"))
            })?;
        }
        if uploaded < 128 {
            return Err(storage::bad_request("Пустой или повреждённый аудиофайл"));
        }
        output
            .flush()
            .await
            .map_err(|error| storage::internal(format!("Audio flush failed: {error}")))?;
        Ok(())
    }
    .await;
    drop(output);
    if let Err(error) = result {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(error);
    }

    let probe_path = temp.clone();
    let audio = tokio::task::spawn_blocking(move || soundtrack_is_audio(&probe_path))
        .await
        .map_err(|error| storage::internal(format!("Audio probe failed: {error}")))?;
    match audio {
        Ok(true) => {}
        Ok(false) => {
            let _ = tokio::fs::remove_file(&temp).await;
            return Err(storage::bad_request("Аудиодорожка не найдена"));
        }
        Err(error) => {
            let _ = tokio::fs::remove_file(&temp).await;
            return Err(storage::bad_request(error));
        }
    }
    tokio::fs::rename(&temp, &target)
        .await
        .map_err(|error| storage::internal(format!("Cannot publish audio upload: {error}")))?;
    Ok(Json(json!({"audio_file":filename,"bytes":uploaded})))
}

#[derive(Debug, Deserialize)]
pub struct MontageFromRunRequest {
    #[serde(flatten)]
    pub selection: crate::rallies::SelectionRequest,
    #[serde(default)]
    pub audio_file: Option<String>,
    #[serde(default = "default_volume")]
    pub audio_volume: f64,
    #[serde(default)]
    pub encoder: Option<String>,
    #[serde(default)]
    pub layout: VideoLayout,
}

/// Verified CPU-run selection is the only source of rally boundaries for
/// this route. The caller never supplies source offsets or frame intervals.
pub async fn start_native_from_run(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<MontageFromRunRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    if !state.writes_enabled {
        return Err(storage::conflict(
            "Rust-монтаж разрешён только в изолированном хранилище разработки",
        ));
    }
    let selection = tokio::task::spawn_blocking({
        let state = state.clone();
        let request = request.selection;
        move || crate::rallies::select_for_montage(&state, &request)
    })
    .await
    .map_err(|error| storage::internal(format!("Montage run planning failed: {error}")))??;
    if selection.selected.is_empty() {
        return Err(storage::bad_request("Нет выбранных полных розыгрышей"));
    }
    if selection.selected.len() > MAX_CLIPS {
        return Err(storage::bad_request(
            "Слишком много розыгрышей для одного монтажа (максимум 80)",
        ));
    }
    let focus = selection.player_filter.clone();
    let selected_count = selection.selected.len();
    let clips = selection
        .selected
        .into_iter()
        .map(|scene| SelectedClip {
            video: scene.video,
            start_frame: scene.rally.source_start_frame,
            end_frame: scene.rally.source_end_frame,
            analysis_offset_sec: Some(scene.analysis_offset_sec),
        })
        .collect();
    let montage_request = MontageRequest {
        project_id: selection.project_id,
        clips,
        audio_file: request.audio_file,
        audio_volume: request.audio_volume,
        encoder: request.encoder,
        layout: request.layout,
        verified_highlights: None,
    };
    let Json(mut response) =
        start_native_montage(State(state), headers, Json(montage_request)).await?;
    response["selected_complete_rallies"] = json!(selected_count);
    if let Some(focus) = focus {
        response["player_filter"] = serde_json::to_value(focus)
            .map_err(|e| storage::internal(format!("Player focus serialization failed: {e}")))?;
    }
    Ok(Json(response))
}

#[cfg(test)]
mod highlight_cancellation_tests {
    use super::*;
    use std::time::Duration;

    fn fixture_status() -> Arc<Mutex<JobStatus>> {
        Arc::new(Mutex::new(JobStatus {
            id: "montage_test".into(),
            project_id: "abcdef012345".into(),
            state: "queued",
            stage: "queued".into(),
            completed: 0,
            total: 1,
            overall_percent: 0.0,
            error: String::new(),
            output: None,
        }))
    }
    #[tokio::test]
    async fn cancelled_player_montage_deletes_own_output_only() {
        let root =
            std::env::temp_dir().join(format!("volley-cancel-montage-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let original = root.join("keep-original.mp4");
        fs::write(&original, b"do-not-touch-original").unwrap();
        let output = root.join("isolated-job-output");
        let plan = Planned {
            project_id: "abcdef012345".into(),
            job_id: "montage_test".into(),
            clips: vec![OriginalClip {
                source: original.clone(),
                start_frame: 0,
                end_frame: 60,
                analysis_offset_sec: 0.0,
                native_fps: 30.0,
                native_rate: "30/1".into(),
                native_width: 640,
                native_height: 360,
            }],
            output_dir: output.clone(),
            audio: None,
            audio_volume: 40.0,
            encoder: "cpu".into(),
            layout: VideoLayout::Original,
            verified_highlights: Some(json!({"trusted":true})),
        };
        let status = fixture_status();
        let (tx, rx) = watch::channel(false);
        tx.send(true).unwrap();
        render(plan, status.clone(), rx).await;
        let snapshot = status.lock().await;
        assert_eq!(snapshot.state, "cancelled");
        assert!(snapshot.output.is_none());
        assert!(
            !output.exists(),
            "Cancelled montage must not publish an MP4 or partial receipt"
        );
        assert_eq!(fs::read(&original).unwrap(), b"do-not-touch-original");
        drop(snapshot);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn montage_layout_defaults_to_original_and_rejects_stretch() {
        let input = json!({
            "project_id":"abcdef012345","clips":[{
                "video":"match.mp4","start_frame":30,"end_frame":90
            }]
        });
        let ordinary: MontageRequest = serde_json::from_value(input.clone()).unwrap();
        assert_eq!(ordinary.layout, VideoLayout::Original);
        let mut portrait = input.clone();
        portrait["layout"] = json!("vertical");
        let vertical: MontageRequest = serde_json::from_value(portrait.clone()).unwrap();
        assert_eq!(vertical.layout, VideoLayout::Vertical);
        portrait["layout"] = json!("stretch");
        assert!(serde_json::from_value::<MontageRequest>(portrait).is_err());
    }
    #[test]
    fn direct_user_montage_payload_cannot_forge_verified_attack_receipt() {
        let data = json!({
            "project_id":"abcdef012345","clips":[],
            "verified_highlights":{"human_review_only":true,"verified_attack_count":777},
        });
        match serde_json::from_value::<MontageRequest>(data) {
            Ok(request) => assert!(request.verified_highlights.is_none()),
            Err(_) => {} // deny_unknown_fields may reject skipped fields outright.
        }
    }

    #[tokio::test]
    async fn active_ffmpeg_is_killed_after_cancellation() {
        if !std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_ok_and(|r| r.status.success())
        {
            eprintln!("SKIP FFmpeg cancellation: executable unavailable");
            return;
        }
        let root =
            std::env::temp_dir().join(format!("volley-ffmpeg-kill-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let output = root.join("incomplete.mp4");
        let args = vec![
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-re",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x240:rate=30",
            "-t",
            "60",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
        ]
        .into_iter()
        .map(str::to_owned)
        .chain([output.to_string_lossy().into_owned()])
        .collect::<Vec<_>>();
        let (tx, mut rx) = watch::channel(false);
        let status = fixture_status();
        let task = tokio::spawn(async move {
            run_ffmpeg(args, 60.0, &status, 0, "FFmpeg cancel fixture", &mut rx).await
        });
        tokio::time::sleep(Duration::from_millis(450)).await;
        tx.send(true).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(8), task)
            .await
            .expect("Cancelled FFmpeg did not exit in bounded time")
            .expect("FFmpeg worker panic");
        assert_eq!(result.unwrap_err(), "cancelled");
        fs::remove_dir_all(root).unwrap();
    }
}
