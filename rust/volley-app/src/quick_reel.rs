//! Independent Rust Quick Reel orchestration. NO project data and NO Python
//! inference reuse. Until native GPU/CPU stages exist, runs end honestly as
//! awaiting_native_inference, never with a fake finished MP4.
//! All writes require VOLLEY_RUST_DATA_ROOT outside the original checkout.

use crate::{
    preparation,
    storage::{self, ApiError, ApiResult},
    AppState,
};
use axum::{
    extract::{Query, State},
    http::HeaderMap,
    Json,
};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, UNIX_EPOCH},
};
use tokio::sync::{watch, Mutex};
use uuid::Uuid;
use volley_media::preparation::{PreparationPlan, PreparationReuse};

const MAX_VIDEOS: usize = 30;
const MAX_VIDEO_SIZE: u64 = 500 * 1024 * 1024 * 1024;
const MAX_SELECTION_BYTES: u64 = 16 * 1024;
const QUICK_ROOT: &str = "quick_reels";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectRequest {
    pub paths: Vec<String>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartRequest {
    pub selection_ids: Vec<String>,
    pub audio_file: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelRequest {
    pub id: String,
}
#[derive(Debug, Deserialize)]
pub struct StatusQuery {
    pub id: Option<String>,
}

struct Running {
    id: String,
    cancel: watch::Sender<bool>,
}
pub struct QuickReelManager {
    current: Mutex<Option<Running>>,
}
impl QuickReelManager {
    pub fn new() -> Self {
        Self {
            current: Mutex::new(None),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Source {
    id: String,
    name: String,
    source: String,
    size: u64,
    mtime_ns: u128,
}
#[derive(Clone)]
struct PreparedRun {
    id: String,
    sources: Vec<Source>,
    plans: Vec<PreparationPlan>,
    audio: String,
    audio_duration: f64,
    root: PathBuf,
}
fn root(state: &AppState) -> PathBuf {
    state.data_root.join(QUICK_ROOT)
}
/// Reject symlinks in writable quick-reel and prepared cache roots. This
/// preserves the separation from the original Python results even if the
/// isolated development directory is manually misconfigured.
fn checked_work_root(state: &AppState, subdir: &str) -> Result<PathBuf, ApiError> {
    ensure_write(state)?;
    let original = state
        .data_root
        .canonicalize()
        .map_err(|_| storage::bad_request("Недоступен Rust data root"))?;
    let target = original.join(subdir);
    if let Ok(meta) = fs::symlink_metadata(&target) {
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(storage::bad_request(
                "Рабочая папка не может быть символической ссылкой",
            ));
        }
    } else {
        fs::create_dir(&target)
            .map_err(|e| storage::internal(format!("Create Rust work directory: {e}")))?;
    }
    let actual = target
        .canonicalize()
        .map_err(|_| storage::bad_request("Не удалось проверить Rust-каталог"))?;
    if actual == original || !actual.starts_with(&original) {
        return Err(storage::bad_request(
            "Рабочая директория выходит из изолированного хранилища",
        ));
    }
    Ok(actual)
}
fn checked_selections(state: &AppState) -> Result<PathBuf, ApiError> {
    let folder = checked_work_root(state, QUICK_ROOT)?;
    let target = folder.join("selections");
    if let Ok(meta) = fs::symlink_metadata(&target) {
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(storage::bad_request("Папка выбора файлов является ссылкой"));
        }
    } else {
        fs::create_dir(&target)
            .map_err(|e| storage::internal(format!("Create selections: {e}")))?;
    }
    let real = target
        .canonicalize()
        .map_err(|_| storage::bad_request("Невозможно проверить папку файлов"))?;
    if !real.starts_with(&folder) || real == folder {
        return Err(storage::bad_request("Папка выбора файлов вне Quick Reel"));
    }
    Ok(real)
}
fn is_id(raw: &str) -> bool {
    raw.len() == 32
        && raw
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}
fn ensure_write(state: &AppState) -> Result<(), ApiError> {
    if !state.writes_enabled {
        Err(storage::conflict(
            "Quick Reel доступен для записи только в изолированном Rust-хранилище",
        ))
    } else {
        Ok(())
    }
}
fn safe_source(path: &str) -> Result<(PathBuf, fs::Metadata), ApiError> {
    if path.is_empty() || path.len() > 4096 || path.contains('\0') {
        return Err(storage::bad_request(
            "Выберите реальный локальный видеофайл",
        ));
    }
    let initial = Path::new(path);
    if !initial.is_absolute() {
        return Err(storage::bad_request(
            "Ожидается абсолютный путь, выбранный проводником",
        ));
    }
    let canonical = initial
        .canonicalize()
        .map_err(|_| storage::bad_request("Выбранный видеофайл не найден"))?;
    if !canonical.is_file() {
        return Err(storage::bad_request(
            "Указанный путь не является видеофайлом",
        ));
    }
    let name = canonical
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| storage::bad_request("Некорректное имя видео"))?;
    storage::video_name(name)?;
    let meta = fs::metadata(&canonical)
        .map_err(|_| storage::bad_request("Нет доступа к метаданным видео"))?;
    if meta.len() == 0 || meta.len() > MAX_VIDEO_SIZE {
        return Err(storage::bad_request(
            "Размер видео должен быть от 1 байта до 500 ГиБ",
        ));
    }
    Ok((canonical, meta))
}
fn mtime(meta: &fs::Metadata) -> Result<u128, ApiError> {
    meta.modified()
        .ok()
        .and_then(|v| v.duration_since(UNIX_EPOCH).ok())
        .map(|v| v.as_nanos())
        .ok_or_else(|| storage::bad_request("Не удалось прочитать время изменения файла"))
}
fn source_for(path: &str) -> Result<Source, ApiError> {
    let (canonical, meta) = safe_source(path)?;
    let probe = volley_media::probe(&canonical).map_err(storage::bad_request)?;
    if probe.duration_sec < 0.1 || !probe.duration_sec.is_finite() {
        return Err(storage::bad_request(
            "Видео не содержит воспроизводимых кадров",
        ));
    }
    Ok(Source {
        id: Uuid::new_v4().simple().to_string(),
        name: canonical
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        source: canonical.to_string_lossy().into_owned(),
        size: meta.len(),
        mtime_ns: mtime(&meta)?,
    })
}
fn verify_source(item: &Source) -> Result<PathBuf, ApiError> {
    if !is_id(&item.id) {
        return Err(storage::bad_request("Неправильный ID выбранного видео"));
    }
    let (canonical, meta) = safe_source(&item.source)?;
    if canonical.to_string_lossy() != item.source
        || meta.len() != item.size
        || mtime(&meta)? != item.mtime_ns
        || canonical.file_name().and_then(|s| s.to_str()) != Some(item.name.as_str())
    {
        return Err(storage::conflict("Исходное видео изменилось после выбора"));
    }
    Ok(canonical)
}
fn selection_file(state: &AppState, id: &str) -> Result<PathBuf, ApiError> {
    if !is_id(id) {
        return Err(storage::bad_request("Неправильный ID выбранного видео"));
    }
    Ok(root(state).join("selections").join(format!("{id}.json")))
}
fn read_source(state: &AppState, id: &str) -> Result<Source, ApiError> {
    // Validate the full isolated path before following selection metadata.
    checked_selections(state)?;
    let file = selection_file(state, id)?;
    let meta = fs::symlink_metadata(&file)
        .map_err(|_| storage::bad_request("Видео не выбрано текущим Rust-проводником"))?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > MAX_SELECTION_BYTES {
        return Err(storage::bad_request("Повреждены данные выбранного файла"));
    }
    let item: Source = serde_json::from_slice(
        &fs::read(file).map_err(|e| storage::internal(format!("Read selection: {e}")))?,
    )
    .map_err(|_| storage::bad_request("Повреждён файл выбора видео"))?;
    if item.id != id {
        return Err(storage::bad_request("ID выбранного видео не совпадает"));
    }
    verify_source(&item)?;
    Ok(item)
}
pub async fn select_sources(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<SelectRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    ensure_write(&state)?;
    if request.paths.is_empty() || request.paths.len() > MAX_VIDEOS {
        return Err(storage::bad_request("Выберите от 1 до 30 видео"));
    }
    let state2 = state.clone();
    let sources = tokio::task::spawn_blocking(move || {
        let mut sources = Vec::new();
        let mut unique = HashSet::new();
        for input in &request.paths {
            let candidate = source_for(input)?;
            if !unique.insert(candidate.source.clone()) {
                return Err(storage::bad_request("Одно видео выбрано несколько раз"));
            }
            sources.push(candidate);
        }
        Ok::<_, ApiError>(sources)
    })
    .await
    .map_err(|e| storage::internal(format!("Quick Reel file selection: {e}")))??;
    checked_selections(&state2)?;
    let mut selected = Vec::new();
    for source in &sources {
        let file = selection_file(&state2, &source.id)?;
        storage::write_json_atomic(
            &file,
            &serde_json::to_value(source)
                .map_err(|e| storage::internal(format!("Serialize selection: {e}")))?,
        )?;
        selected.push(json!({"id":source.id,"name":source.name,
            "size_gb":(source.size as f64/1e9*100.0).round()/100.0,
            "linked":true,"copied":false}));
    }
    Ok(Json(json!({"videos":selected,"project_created":false})))
}

fn audio_for(state: &AppState, filename: &str) -> Result<(String, f64), ApiError> {
    if filename.is_empty()
        || filename.len() > 200
        || filename.contains(['/', '\\', '\0'])
        || Path::new(filename).file_name().and_then(|s| s.to_str()) != Some(filename)
    {
        return Err(storage::bad_request(
            "Укажите музыкальный файл, загруженный в Rust",
        ));
    }
    let ext = Path::new(filename)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches!(
        ext.as_str(),
        "mp3" | "wav" | "m4a" | "flac" | "aac" | "ogg" | "opus"
    ) {
        return Err(storage::bad_request("Неподдерживаемый формат музыки"));
    }
    let parent = state
        .data_root
        .join("uploads/music")
        .canonicalize()
        .map_err(|_| storage::bad_request("Сначала загрузите музыкальный трек"))?;
    let audio = parent
        .join(filename)
        .canonicalize()
        .map_err(|_| storage::bad_request("Музыкальный трек не найден"))?;
    if !audio.starts_with(&parent) || !audio.is_file() {
        return Err(storage::bad_request("Музыкальный файл вне хранилища Rust"));
    }
    let metadata = audio
        .metadata()
        .map_err(|_| storage::bad_request("Музыкальный файл недоступен"))?;
    if metadata.len() == 0 || metadata.len() > 256 * 1024 * 1024 {
        return Err(storage::bad_request("Музыкальный трек слишком большой"));
    }
    let probe = volley_media::ffprobe(&audio).map_err(storage::bad_request)?;
    if !probe["streams"].as_array().is_some_and(|streams| {
        streams
            .iter()
            .any(|s| s["codec_type"].as_str() == Some("audio"))
    }) {
        return Err(storage::bad_request("В файле нет аудиодорожки"));
    }
    let duration = probe["format"]["duration"]
        .as_f64()
        .or_else(|| {
            probe["format"]["duration"]
                .as_str()
                .and_then(|s| s.parse().ok())
        })
        .ok_or_else(|| storage::bad_request("Не удалось прочитать длительность музыки"))?;
    if !duration.is_finite() || !(10.0..=600.0).contains(&duration) {
        return Err(storage::bad_request("Музыка должна длиться 10–600 секунд"));
    }
    Ok((
        audio.to_string_lossy().into_owned(),
        (duration * 1000.0).round() / 1000.0,
    ))
}
fn default_analysis() -> Value {
    json!({
        "schema":"volley-quick-reel-defaults/1",
        "inference_cache_reuse":false,
        "model_ball":"VballNetV4c",
        "model_players":"RAVEL-VB-013-9f",
        "ball_backend":"onnx",
        "player_backend":"onnx-cuda",
        "player_device":"GPU",
        "gpu_memory_limit_mb":0,
        "beach_volleyball":true,
        "no_court_fallback":true,
        "cpu_golden_profile":"pending_native_cpu_port",
        "ball_touch_extrema":true,
        "rally_selection_strategy":"highlights",
        "montage_mode":"montage",
        "follow_ball":true,
        "min_rally_sec":1.5,
        "min_confidence":0.5,
        "context_sec":1.5,
        "gap_sec":3.0,
        "min_rally_length_sec":3.0,
        "soundtrack_volume_percent":40
    })
}
fn checked_request(state: &AppState, input: StartRequest) -> Result<PreparedRun, ApiError> {
    ensure_write(state)?;
    if input.selection_ids.is_empty()
        || input.selection_ids.len() > MAX_VIDEOS
        || input.selection_ids.iter().collect::<HashSet<_>>().len() != input.selection_ids.len()
    {
        return Err(storage::bad_request(
            "Выберите от 1 до 30 разных видео для рилса",
        ));
    }
    checked_work_root(state, "prepared")?;
    checked_selections(state)?;
    let mut sources = Vec::new();
    let mut paths = HashSet::new();
    let mut plans = Vec::new();
    for id in &input.selection_ids {
        let src = read_source(state, id)?;
        let source = verify_source(&src)?;
        if !paths.insert(source.clone()) {
            return Err(storage::bad_request(
                "Один исходный видеофайл выбран дважды",
            ));
        }
        plans.push(
            PreparationPlan::new(&source, &state.data_root, None).map_err(storage::bad_request)?,
        );
        sources.push(src);
    }
    let (audio, audio_duration) = audio_for(state, &input.audio_file)?;
    let id = Uuid::new_v4().simple().to_string();
    let root = root(state).join(&id);
    Ok(PreparedRun {
        id,
        sources,
        plans,
        audio,
        audio_duration,
        root,
    })
}
fn status_file(run: &PreparedRun) -> PathBuf {
    run.root.join("status.json")
}
fn running_json(
    run: &PreparedRun,
    state: &str,
    stage: &str,
    index: u8,
    percent: f64,
    videos: &[Value],
    error: Option<&str>,
) -> Value {
    json!({
        "id":run.id,
        "status":state,
        "stage":stage,
        "stage_index":index,
        "stage_total":3,
        "progress":(percent*100.0).round()/100.0,
        "videos":videos,
        "error":error,
        "music_duration":run.audio_duration,
        "native_inference_available":false,
        "default_analysis":true,
        "output":Value::Null
    })
}
fn save_status(run: &PreparedRun, value: &Value) -> Result<(), ApiError> {
    storage::write_json_atomic(&status_file(run), value)
}
fn planned_request(run: &PreparedRun) -> Value {
    json!({
        "schema":"volley-rust-quick-reel/1",
        "id":run.id,
        "videos":run.sources,
        "audio_path":run.audio,
        "duration":run.audio_duration,
        "default_analysis":true,
        "defaults":default_analysis(),
        "inference_cache_reuse":false,
        "analysis_cache_salt":run.id,
        "state":"preparing",
        "note":"Preparation may reuse input MP4. GPU inference always must run fresh; not implemented in Rust yet."
    })
}
fn latest(state: &AppState) -> Result<Option<String>, ApiError> {
    let path = root(state).join("latest.json");
    let doc = storage::read_json(&path)?;
    let id = doc.as_ref().and_then(|v| v["id"].as_str());
    match id {
        None => Ok(None),
        Some(id) if is_id(id) => Ok(Some(id.to_owned())),
        Some(_) => Err(storage::bad_request("Повреждён latest.json быстрого рилса")),
    }
}
fn prune_scratch(state: &AppState, keep: &str) {
    // Only scratch dirs with exact generated UUIDs. Do not touch the
    // completed MP4 archive, prepared cache, media uploads or source files.
    let Ok(entries) = fs::read_dir(root(state)) else {
        return;
    };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if name == keep || !is_id(&name) {
            continue;
        }
        let Ok(meta) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if !meta.is_dir() || meta.file_type().is_symlink() {
            continue;
        }
        let _ = fs::remove_dir_all(entry.path());
    }
}

async fn perform(
    state: Arc<AppState>,
    run: PreparedRun,
    cancelled: watch::Receiver<bool>,
    _lock: File,
) {
    let mut videos = run
        .sources
        .iter()
        .map(|source| {
            json!({"video":source.name,"state":"queued","progress":0.0,
            "prepared_reused":Value::Null,"preparation":"pending"})
        })
        .collect::<Vec<_>>();
    let total = run.sources.len();
    let mut status = running_json(&run, "running", "Подготовка видео", 1, 0.0, &videos, None);
    let _ = save_status(&run, &status);
    for (index, plan) in run.plans.iter().enumerate() {
        if *cancelled.borrow() {
            status = running_json(
                &run,
                "cancelled",
                "Обработка остановлена",
                1,
                15.0 * index as f64 / total as f64,
                &videos,
                None,
            );
            let _ = save_status(&run, &status);
            break;
        }
        // Reusing a compatible video is allowed. Inference must always be
        // NEW per Quick Reel id; CPU outputs are never imported.
        let already = plan.reused();
        if already.is_some() {
            videos[index]["state"] = json!("ready");
            videos[index]["progress"] = json!(100.0);
            videos[index]["prepared_reused"] = json!(true);
            videos[index]["preparation"] = json!(if already == Some(PreparationReuse::Original) {
                "not_required"
            } else {
                "cached"
            });
        } else {
            videos[index]["state"] = json!("preparing");
            let progress = Arc::new(Mutex::new(preparation::Progress::queued()));
            let work = preparation::prepare_standalone(
                state.clone(),
                plan.clone(),
                cancelled.clone(),
                progress.clone(),
            );
            tokio::pin!(work);
            let outcome = loop {
                tokio::select! {
                    result=&mut work=>break result,
                    _=tokio::time::sleep(Duration::from_millis(600))=>{
                        let pct=progress.lock().await.progress;
                        videos[index]["progress"]=json!(pct);
                        let entire=15.0*(index as f64+pct/100.0)/total as f64;
                        status=running_json(&run,"running","Подготовка видео",1,
                            entire,&videos,None);
                        let _=save_status(&run,&status);
                    }
                }
            };
            if let Err(err) = outcome {
                let was_cancelled = err == "cancelled" || *cancelled.borrow();
                let label = if was_cancelled { "cancelled" } else { "failed" };
                videos[index]["state"] = json!(label);
                status = running_json(
                    &run,
                    label,
                    if was_cancelled {
                        "Обработка остановлена"
                    } else {
                        "Ошибка подготовки видео"
                    },
                    1,
                    15.0 * index as f64 / total as f64,
                    &videos,
                    (!was_cancelled).then_some(err.as_str()),
                );
                let _ = save_status(&run, &status);
                break;
            }
            videos[index]["state"] = json!("ready");
            videos[index]["progress"] = json!(100.0);
            videos[index]["prepared_reused"] = json!(false);
            videos[index]["preparation"] = json!("encoded");
        }
        // Never advance if the selected path was replaced while FFmpeg ran.
        if let Err(err) = verify_source(&run.sources[index]) {
            status = running_json(
                &run,
                "failed",
                "Исходное видео изменилось",
                1,
                15.0 * index as f64 / total as f64,
                &videos,
                Some(&format!("{err:?}")),
            );
            let _ = save_status(&run, &status);
            break;
        }
        let stage_progress = 15.0 * (index + 1) as f64 / total as f64;
        status = running_json(
            &run,
            "running",
            "Подготовка видео",
            1,
            stage_progress,
            &videos,
            None,
        );
        let _ = save_status(&run, &status);
    }
    if status["status"].as_str() == Some("running") {
        if *cancelled.borrow() {
            status = running_json(
                &run,
                "cancelled",
                "Обработка остановлена",
                1,
                status["progress"].as_f64().unwrap_or(0.0),
                &videos,
                None,
            );
        } else {
            // This is NOT success and no placeholder MP4 is published.
            // The full RAVEL/VballNet native inference and CPU reprocess
            // stage must be wired before the pipeline can continue.
            status=running_json(&run,"awaiting_native_inference",
                "Ожидается нативный GPU-инференс RAVEL-013 + VballNetV4c",2,
                15.0,&videos,Some(
                    "Рилс не создан: нативный GPU/CPU анализ ещё не перенесён на Rust. Подготовка завершена; анализ должен стартовать заново, без кеша готовых анализов."
                ));
        }
        let _ = save_status(&run, &status);
    }
    let mut current = state.quick_reel.current.lock().await;
    if current.as_ref().is_some_and(|entry| entry.id == run.id) {
        *current = None;
    }
}

pub async fn start(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<StartRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    ensure_write(&state)?;
    // In-process single flight. The separate file lock also protects
    // multiple Rust servers that happen to share the same dev data root.
    let mut slot = state.quick_reel.current.lock().await;
    if slot.is_some() {
        return Err(storage::conflict(
            "Предыдущий быстрый рилс ещё обрабатывается",
        ));
    }
    let local = state.clone();
    let run = tokio::task::spawn_blocking(move || checked_request(&local, request))
        .await
        .map_err(|e| storage::internal(format!("Quick Reel request: {e}")))??;
    let base = checked_work_root(&state, QUICK_ROOT)?;
    let lockfile = base.join(".active.lock");
    if fs::symlink_metadata(&lockfile)
        .ok()
        .is_some_and(|m| m.file_type().is_symlink())
    {
        return Err(storage::bad_request("Quick Reel lock cannot be a symlink"));
    }
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&lockfile)
        .map_err(|e| storage::internal(format!("Quick Reel lock file: {e}")))?;
    match lock.try_lock_exclusive() {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
            return Err(storage::conflict(
                "Другой Rust-процесс уже выполняет быстрый рилс",
            ))
        }
        Err(e) => return Err(storage::internal(format!("Quick Reel lock: {e}"))),
    }
    fs::create_dir(&run.root).map_err(|e| storage::internal(format!("Quick Reel folder: {e}")))?;
    storage::write_json_atomic(&run.root.join("request.json"), &planned_request(&run))?;
    let videos = run
        .sources
        .iter()
        .map(|row| json!({"video":row.name,"state":"queued","progress":0}))
        .collect::<Vec<_>>();
    save_status(
        &run,
        &running_json(&run, "queued", "Ожидание подготовки", 0, 0.0, &videos, None),
    )?;
    // Publish latest only after a valid durable request and queued status
    // exist. Never delete a previous latest on a failed start request.
    storage::write_json_atomic(&base.join("latest.json"), &json!({"id":run.id}))?;
    prune_scratch(&state, &run.id);
    let id = run.id.clone();
    let (tx, rx) = watch::channel(false);
    *slot = Some(Running {
        id: id.clone(),
        cancel: tx,
    });
    tokio::spawn(perform(state.clone(), run, rx, lock));
    Ok(Json(json!({"id":id,"status":"queued","stage_total":3,
        "default_analysis":true,"inference_cache_reuse":false,
        "project_created":false,"native_gpu_inference_available":false})))
}

fn status_for(state: &AppState, id: &str) -> Result<Value, ApiError> {
    if !is_id(id) {
        return Err(storage::bad_request("Некорректный идентификатор рилса"));
    }
    let path = root(state).join(id).join("status.json");
    let meta =
        fs::symlink_metadata(&path).map_err(|_| storage::not_found("Быстрый рилс не найден"))?;
    if meta.file_type().is_symlink() || !meta.is_file() || meta.len() > 128 * 1024 {
        return Err(storage::bad_request("Повреждён статус быстрого рилса"));
    }
    let status = storage::read_json(&path)?
        .ok_or_else(|| storage::not_found("Статус быстрого рилса недоступен"))?;
    if status["id"].as_str() != Some(id) {
        return Err(storage::bad_request("Статус относится к другому рилсу"));
    }
    Ok(status)
}
pub async fn status(
    State(state): State<Arc<AppState>>,
    Query(query): Query<StatusQuery>,
) -> ApiResult<Value> {
    ensure_write(&state)?;
    let id = match query.id {
        Some(id) if is_id(&id) => Some(id),
        Some(_) => return Err(storage::bad_request("Неправильный id быстрого рилса")),
        None => latest(&state)?,
    };
    let Some(id) = id else {
        return Ok(Json(json!({"status":"idle","stage_total":3})));
    };
    let mut result = status_for(&state, &id)?;
    if matches!(result["status"].as_str(), Some("queued" | "running")) {
        let active = state.quick_reel.current.lock().await;
        if active.as_ref().is_none_or(|job| job.id != id) {
            // A crashed Rust process must never leave a spinner forever.
            // Other processes sharing the isolated root are protected by
            // the active lock; do not falsely mark their run interrupted.
            let lockpath = root(&state).join(".active.lock");
            let cross_process_busy = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&lockpath)
                .ok()
                .is_some_and(|file| file.try_lock_exclusive().is_err());
            if !cross_process_busy {
                result["status"] = json!("interrupted");
                result["stage"] = json!("Запуск прерван остановкой Rust-сервера");
                result["error"] = json!(
                    "Подготовку нужно запустить заново; старые GPU-результаты использовать нельзя."
                );
            }
        }
    }
    Ok(Json(result))
}
pub async fn cancel(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<CancelRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    ensure_write(&state)?;
    if !is_id(&request.id) {
        return Err(storage::bad_request("Неправильный идентификатор рилса"));
    }
    let current = state.quick_reel.current.lock().await;
    let job = current
        .as_ref()
        .ok_or_else(|| storage::conflict("Сейчас нет активной обработки быстрого рилса"))?;
    if job.id != request.id {
        return Err(storage::not_found("Этот рилс не выполняется"));
    }
    let accepted = job.cancel.send(true).is_ok();
    Ok(Json(json!({"id":request.id,"cancel_requested":accepted})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_token_cannot_traverse() {
        assert!(is_id("0123456789abcdef0123456789abcdef"));
        assert!(!is_id("../another-file"));
        assert!(!is_id("0123456789ABCDEF0123456789ABCDEF"));
    }
    #[test]
    fn golden_config_forces_new_analysis() {
        let config = default_analysis();
        assert_eq!(config["inference_cache_reuse"], false);
        assert_eq!(config["beach_volleyball"], true);
        assert_eq!(config["player_backend"], "onnx-cuda");
        assert_eq!(config["gpu_memory_limit_mb"], 0);
        assert_eq!(config["follow_ball"], true);
    }
    #[test]
    fn no_quick_reel_can_claim_a_finished_output_without_inference() {
        let run = PreparedRun {
            id: "0123456789abcdef0123456789abcdef".into(),
            sources: vec![],
            plans: vec![],
            audio: String::new(),
            audio_duration: 120.0,
            root: PathBuf::new(),
        };
        let status = running_json(
            &run,
            "awaiting_native_inference",
            "GPU этап ещё не реализован",
            2,
            15.0,
            &[],
            None,
        );
        assert!(status["output"].is_null());
        assert_eq!(status["native_inference_available"], false);
    }
}
