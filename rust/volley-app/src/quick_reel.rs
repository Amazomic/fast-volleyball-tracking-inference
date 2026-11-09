//! Independent Rust Quick Reel orchestration. NO project data and NO Python
//! inference reuse. Fresh CUDA/CPU stages run in a private per-reel namespace.
//! No project inference data, analysis proxy or Python worker may be reused.
//! All writes require VOLLEY_RUST_DATA_ROOT outside the original checkout.

use crate::{
    cpu_rallies, cpu_retrack, cpu_touches, inference_full, inference_fusion, montage,
    native_rally_preview, preparation,
    storage::{self, ApiError, ApiResult},
    AppState,
};
use axum::{
    extract::{Multipart, Query, State},
    http::{header, HeaderMap},
    response::Response,
    Json,
};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
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
use volley_media::montage::{SelectedClip, VideoLayout};
#[path = "quick_reel_pipeline.rs"]
mod pipeline;
use pipeline::run_native_pipeline;
#[cfg(test)]
use pipeline::{best_quick_rallies, create_run_workspace, RankedRally};
use volley_media::preparation::{PreparationPlan, PreparationReuse};

const MAX_VIDEOS: usize = 30;
const MAX_VIDEO_SIZE: u64 = 500 * 1024 * 1024 * 1024;
const MAX_SELECTION_BYTES: u64 = 16 * 1024;
const QUICK_ROOT: &str = "quick_reels";
const MAX_BROWSER_UPLOAD: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectRequest {
    pub paths: Vec<String>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseRequest {
    pub selection_ids: Vec<String>,
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
    #[serde(default)]
    display_name: Option<String>,
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
        display_name: None,
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
        selected.push(
            json!({"id":source.id,"name":source.display_name.as_deref().unwrap_or(&source.name),
            "size_gb":(source.size as f64/1e9*100.0).round()/100.0,
            "linked":true,"copied":false}),
        );
    }
    Ok(Json(json!({"videos":selected,"project_created":false})))
}

/// Native desktop explorer: original files are only linked by absolute path,
/// never uploaded or copied. The fixed PowerShell program has no user input.
const PICK_SCRIPT: &str = r#"
Add-Type -AssemblyName System.Windows.Forms
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$d = New-Object System.Windows.Forms.OpenFileDialog
$d.Title = 'Volleyball Studio — Quick Reel'
$d.Filter = 'Video files|*.mp4;*.mov;*.mkv;*.avi;*.webm;*.m4v;*.mts'
$d.Multiselect = $true
$d.CheckFileExists = $true
if ($d.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {
    [Console]::Out.WriteLine((ConvertTo-Json -InputObject @($d.FileNames) -Compress))
} else {
    [Console]::Out.WriteLine('[]')
}
"#;

/// PowerShell Unicode -EncodedCommand uses UTF-16LE base64 and avoids the
/// Windows/WSL shell quotation ambiguity for embedded GUI script strings.
/// This is a fixed checked-in command, NOT based on browser input.
fn powershell_command(script: &str) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = script
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let a = chunk[0];
        let b = *chunk.get(1).unwrap_or(&0);
        let c = *chunk.get(2).unwrap_or(&0);
        encoded.push(ALPHABET[(a >> 2) as usize] as char);
        encoded.push(ALPHABET[((a & 3) << 4 | b >> 4) as usize] as char);
        encoded.push(if chunk.len() > 1 {
            ALPHABET[((b & 15) << 2 | c >> 6) as usize] as char
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            ALPHABET[(c & 63) as usize] as char
        } else {
            '='
        });
    }
    encoded
}
async fn desktop_paths(raw: &[String]) -> Result<Vec<String>, ApiError> {
    if raw.len() > MAX_VIDEOS {
        return Err(storage::bad_request("Выберите не более 30 видео"));
    }
    let mut paths = Vec::with_capacity(raw.len());
    for win in raw {
        if win.len() > 4096 || !win.contains(':') || win.contains('\0') {
            return Err(storage::bad_request("Неправильный путь из проводника"));
        }
        if cfg!(windows) {
            paths.push(win.clone());
            continue;
        }
        // WSL starts the real Windows Explorer dialog, but Rust processes
        // the files through its Linux /mnt/<drive>/ pathname.
        let conversion = tokio::process::Command::new("wslpath")
            .args(["-u", win])
            .output()
            .await
            .map_err(|_| storage::bad_request("WSL path translation unavailable"))?;
        if !conversion.status.success() || conversion.stdout.len() > 4096 {
            return Err(storage::bad_request("Windows path cannot be used by WSL"));
        }
        let linux = String::from_utf8(conversion.stdout)
            .map_err(|_| storage::bad_request("Invalid WSL pathname encoding"))?;
        paths.push(linux.trim_end_matches(['\r', '\n']).to_owned());
    }
    Ok(paths)
}
pub async fn native_pick(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    ensure_write(&state)?;
    let executable = if cfg!(windows) {
        "powershell.exe"
    } else {
        "/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe"
    };
    let mut program = tokio::process::Command::new(executable);
    let command = powershell_command(PICK_SCRIPT);
    program
        .args(["-NoLogo", "-NoProfile", "-STA", "-EncodedCommand", &command])
        .kill_on_drop(true);
    let result = tokio::time::timeout(Duration::from_secs(240), program.output())
        .await
        .map_err(|_| storage::bad_request("Окно выбора файлов закрыто по тайм-ауту"))?
        .map_err(|_| {
            storage::bad_request(
                "Системный проводник недоступен. Используйте перетаскивание файлов.",
            )
        })?;
    if !result.status.success() || result.stdout.len() > 128 * 1024 {
        return Err(storage::bad_request(
            "Не удалось открыть системный проводник",
        ));
    }
    let files: Vec<String> = serde_json::from_slice(&result.stdout)
        .map_err(|_| storage::bad_request("Некорректный ответ проводника Windows"))?;
    if files.is_empty() {
        return Ok(Json(
            json!({"videos":[],"cancelled":true,"project_created":false}),
        ));
    }
    let paths = desktop_paths(&files).await?;
    select_sources(State(state), headers, Json(SelectRequest { paths })).await
}

/// Browser drag-and-drop has no privileged filesystem path. Copy only such
/// explicitly dropped videos, streaming to a size-bounded isolated root.
/// A desktop picker must be preferred for large recordings to avoid copies.
pub async fn upload_source(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> ApiResult<Value> {
    use tokio::io::AsyncWriteExt;
    storage::validate_origin(&headers)?;
    ensure_write(&state)?;
    let mut file = multipart
        .next_field()
        .await
        .map_err(|_| storage::bad_request("Повреждена загрузка видео"))?
        .ok_or_else(|| storage::bad_request("Нет загружаемого видео"))?;
    if file.name() != Some("file") {
        return Err(storage::bad_request("Ожидалось поле file"));
    }
    let display = file.file_name().unwrap_or("").to_owned();
    storage::video_name(&display)?;
    if display.len() > 255 {
        return Err(storage::bad_request("Слишком длинное имя видео"));
    }
    let ext = Path::new(&display)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let base = checked_work_root(&state, QUICK_ROOT)?;
    let dir = base.join("uploads");
    if let Ok(meta) = fs::symlink_metadata(&dir) {
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(storage::bad_request("Папка временных видео небезопасна"));
        }
    } else {
        fs::create_dir(&dir).map_err(|e| storage::internal(format!("Quick upload folder: {e}")))?;
    }
    let output_path = dir.join(format!("drop_{}.{}", Uuid::new_v4().simple(), ext));
    let mut output = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output_path)
        .await
        .map_err(|e| storage::internal(format!("Quick Reel temporary upload: {e}")))?;
    let result:Result<u64,ApiError>=async{
        let mut length=0u64;
        while let Some(bytes)=file.chunk().await
            .map_err(|_|storage::bad_request("Сбой получения видео"))?{
            length=length.checked_add(bytes.len() as u64)
                .ok_or_else(||storage::bad_request("Размер видео переполнен"))?;
            if length>MAX_BROWSER_UPLOAD{
                return Err(storage::bad_request(
                    "Перетаскивание ограничено 2 ГиБ. Для больших видео используйте проводник без копирования."));
            }
            output.write_all(&bytes).await
                .map_err(|e|storage::internal(format!("Video upload write: {e}")))?;
        }
        if length==0{return Err(storage::bad_request("Пустой видеофайл"));}
        output.flush().await.map_err(|e|storage::internal(format!("Video upload flush: {e}")))?;
        output.sync_all().await.map_err(|e|storage::internal(format!("Video upload sync: {e}")))?;
        Ok(length)
    }.await;
    drop(file);
    drop(output);
    if let Err(err) = result {
        let _ = tokio::fs::remove_file(&output_path).await;
        return Err(err);
    }
    if multipart
        .next_field()
        .await
        .map_err(|_| storage::bad_request("Ошибка окончания загрузки"))?
        .is_some()
    {
        let _ = tokio::fs::remove_file(&output_path).await;
        return Err(storage::bad_request("Загрузите один файл за запрос"));
    }
    let path = output_path.clone();
    let source = tokio::task::spawn_blocking(move || source_for(&path.to_string_lossy()))
        .await
        .map_err(|e| storage::internal(format!("Video probe task: {e}")))?;
    let mut source = match source {
        Ok(value) => value,
        Err(error) => {
            let _ = tokio::fs::remove_file(&output_path).await;
            return Err(error);
        }
    };
    source.display_name = Some(display.clone());
    let saved = (|| -> Result<Value, ApiError> {
        checked_selections(&state)?;
        storage::write_json_atomic(
            &selection_file(&state, &source.id)?,
            &serde_json::to_value(&source)
                .map_err(|e| storage::internal(format!("Quick selection encoding: {e}")))?,
        )?;
        Ok(json!({"videos":[{
            "id":source.id,"name":display,"size_gb":(source.size as f64/1e9*100.0).round()/100.0,
            "linked":false,"copied":true
        }],"project_created":false}))
    })();
    if saved.is_err() {
        let _ = tokio::fs::remove_file(&output_path).await;
    }
    Ok(Json(saved?))
}

/// Only the status-confirmed, original-media, AAC-verified MP4 for this UUID
/// is downloadable. No caller-provided filename/path is ever opened.
fn completed_quick_media(state: &AppState, id: &str) -> Result<PathBuf, ApiError> {
    if !is_id(id) {
        return Err(storage::bad_request("Invalid Quick Reel ID"));
    }
    let folder = root(state).join(id);
    let directory =
        fs::symlink_metadata(&folder).map_err(|_| storage::not_found("Quick Reel run missing"))?;
    if directory.file_type().is_symlink() || !directory.is_dir() {
        return Err(storage::bad_request("Quick Reel run folder is unsafe"));
    }
    let receipt_file = folder.join("result.json");
    let receipt_stat = fs::symlink_metadata(&receipt_file)
        .map_err(|_| storage::not_found("Quick Reel receipt missing"))?;
    if receipt_stat.file_type().is_symlink()
        || !receipt_stat.is_file()
        || receipt_stat.len() > 128 * 1024
    {
        return Err(storage::bad_request("Quick Reel receipt is unsafe"));
    }
    let status = status_for(state, id)?;
    let receipt = storage::read_json(&receipt_file)?
        .ok_or_else(|| storage::not_found("Quick Reel output not ready"))?;
    let expected = status["output"]["size_bytes"]
        .as_u64()
        .filter(|n| *n >= 1024)
        .ok_or_else(|| storage::conflict("Quick Reel MP4 not published"))?;
    if status["status"] != "complete"
        || status["id"] != id
        || status["fresh_gpu_cpu"] != true
        || status["inference_cache_reuse"] != false
        || receipt["schema"] != "volley-native-quick-reel/2"
        || receipt["id"] != id
        || receipt["project_created"] != false
        || receipt["source_type"] != "fresh_native_gpu_cpu"
        || receipt["output"]["file"] != "quick_reel.mp4"
        || receipt["output"]["size_bytes"].as_u64() != Some(expected)
        || receipt["output"]["native_original_clips_verified"] != true
        || receipt["output"]["mp4_quality"]["has_audio"] != true
    {
        return Err(storage::conflict(
            "Quick Reel completion evidence is inconsistent",
        ));
    }
    let file = folder.join("quick_reel.mp4");
    let meta =
        fs::symlink_metadata(&file).map_err(|_| storage::not_found("Quick Reel MP4 not found"))?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() != expected {
        return Err(storage::conflict(
            "Quick Reel MP4 file differs from validated receipt",
        ));
    }
    Ok(file)
}
pub async fn media(
    State(state): State<Arc<AppState>>,
    Query(query): Query<StatusQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    ensure_write(&state)?;
    let id = query
        .id
        .ok_or_else(|| storage::bad_request("Quick Reel ID required"))?;
    let reading = state.clone();
    let path = tokio::task::spawn_blocking(move || completed_quick_media(&reading, &id))
        .await
        .map_err(|e| storage::internal(format!("Quick MP4 lookup: {e}")))??;
    let mut response = crate::playback::serve_mp4(path, &headers).await?;
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        "attachment; filename=\"quick_reel.mp4\""
            .parse()
            .expect("Fixed filename header"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        "no-store".parse().expect("Fixed cache header"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        "nosniff".parse().expect("Fixed MIME header"),
    );
    Ok(response)
}
/// Clear only this browser's explicitly selected tokens. Linked originals
/// are NEVER deleted, even if the user clicks clear. Dropped copies are
/// removed only from the isolated Quick Reel uploads directory and only
/// while no server instance is processing a reel.
pub async fn release_sources(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<ReleaseRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    ensure_write(&state)?;
    if request.selection_ids.len() > MAX_VIDEOS
        || request.selection_ids.iter().any(|id| !is_id(id))
        || request.selection_ids.iter().collect::<HashSet<_>>().len() != request.selection_ids.len()
    {
        return Err(storage::bad_request("Invalid Quick Reel selection tokens"));
    }
    let current = state.quick_reel.current.lock().await;
    if current.is_some() {
        return Err(storage::conflict("Quick Reel is still active"));
    }
    let base = checked_work_root(&state, QUICK_ROOT)?;
    let lockpath = base.join(".active.lock");
    if fs::symlink_metadata(&lockpath).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return Err(storage::bad_request("Unsafe Quick Reel active lock"));
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(lockpath)
        .map_err(|e| storage::internal(format!("Quick lock: {e}")))?;
    file.try_lock_exclusive()
        .map_err(|_| storage::conflict("Quick Reel is active on another server"))?;
    let upload_root = base.join("uploads");
    let mut removed = 0usize;
    for id in &request.selection_ids {
        let item = read_source(&state, id)?;
        let selected = selection_file(&state, id)?;
        fs::remove_file(selected)
            .map_err(|e| storage::internal(format!("Clear Quick selection: {e}")))?;
        if item.display_name.is_some() {
            let path = Path::new(&item.source);
            if path.parent() == Some(upload_root.as_path())
                && path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.starts_with("drop_"))
            {
                fs::remove_file(path)
                    .map_err(|e| storage::internal(format!("Remove temporary video: {e}")))?;
            }
        }
        removed += 1;
    }
    Ok(Json(json!({"released":removed,"project_created":false})))
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
        "cpu_golden_profile":"native_physical_motion_touch_rally",
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
/// Only the worker's per-run directory may act as an internal transient
/// project namespace. It is NEVER entered into the real projects registry.
/// Shared preparation is the *only* cache that the internal stages can read.
pub(crate) fn prepared_plan_for_quick_reel(
    state: &AppState,
    project: &Value,
    video: &str,
) -> Result<PreparationPlan, ApiError> {
    storage::video_name(video)?;
    let scoped = state
        .data_root
        .canonicalize()
        .map_err(|_| storage::bad_request("Quick Reel pipeline root missing"))?;
    if scoped.file_name().and_then(|s| s.to_str()) != Some("pipeline") {
        return Err(storage::bad_request(
            "Not an internal Quick Reel pipeline namespace",
        ));
    }
    let run_dir = scoped
        .parent()
        .ok_or_else(|| storage::bad_request("Quick Reel has no run root"))?;
    let run_id = run_dir.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let quick_root = run_dir
        .parent()
        .ok_or_else(|| storage::bad_request("Quick Reel root missing"))?;
    if !is_id(run_id)
        || quick_root.file_name().and_then(|s| s.to_str()) != Some(QUICK_ROOT)
        || project["quick_reel_run_id"].as_str() != Some(run_id)
    {
        return Err(storage::bad_request(
            "Quick Reel virtual project does not belong to this run",
        ));
    }
    let global_root = quick_root
        .parent()
        .ok_or_else(|| storage::bad_request("Rust root missing"))?;
    let original = project["source_paths"][video]
        .as_str()
        .ok_or_else(|| storage::bad_request("Missing original Quick Reel source mapping"))?;
    let expected_id = project["source_ids"][video]
        .as_str()
        .ok_or_else(|| storage::bad_request("Missing Quick Reel source token"))?;
    // The original request was written by the server before this private
    // namespace was constructed. Re-verify source identity for every stage.
    let request = storage::read_json(&run_dir.join("request.json"))?
        .ok_or_else(|| storage::bad_request("Missing original Quick Reel request"))?;
    if request["id"].as_str() != Some(run_id)
        || request["inference_cache_reuse"] != false
        || request["schema"] != "volley-rust-quick-reel/1"
    {
        return Err(storage::conflict("Quick Reel request identity changed"));
    }
    let saved = request["videos"]
        .as_array()
        .and_then(|rows| {
            rows.iter().find(|row| {
                row["id"].as_str() == Some(expected_id) && row["source"].as_str() == Some(original)
            })
        })
        .ok_or_else(|| storage::conflict("Quick Reel input not in immutable run request"))?;
    let source: Source = serde_json::from_value(saved.clone())
        .map_err(|_| storage::bad_request("Damaged Quick Reel source identity"))?;
    let canonical = verify_source(&source)?;
    let plan = PreparationPlan::new(&canonical, global_root, None).map_err(storage::bad_request)?;
    if plan.ready_path().is_none() {
        return Err(storage::conflict(
            "Quick Reel prepared media not ready; no inference reuse allowed",
        ));
    }
    Ok(plan)
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
        "stage_total":4,
        "progress":(percent*100.0).round()/100.0,
        "videos":videos,
        "error":error,
        "music_duration":run.audio_duration,
        "native_inference_available":true,
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
        "note":"Preparation only may reuse media; fresh CUDA VballNet and RAVEL, CPU fusion, tracking, Touch, rally and montage follow per run."
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
            json!({"video":source.display_name.as_deref().unwrap_or(&source.name),"state":"queued","progress":0.0,
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
                "Остановлено после подготовки",
                1,
                status["progress"].as_f64().unwrap_or(0.0),
                &videos,
                None,
            );
        } else {
            match run_native_pipeline(&state, &run, videos.clone(), cancelled.clone()).await {
                Ok((output, video_status)) => {
                    videos = video_status;
                    status = running_json(
                        &run,
                        "complete",
                        "Quick Reel MP4 готов",
                        4,
                        100.0,
                        &videos,
                        None,
                    );
                    status["output"] = output;
                    status["inference_cache_reuse"] = json!(false);
                    status["fresh_gpu_cpu"] = json!(true);
                }
                Err(error) => {
                    // Keep last GPU/CPU stage and per-video provenance, even
                    // when an ONNX CUDA provider or model weight is missing.
                    let mut failed_stage = 2u8;
                    let mut failed_progress = 15.0;
                    if let Ok(Some(current)) = storage::read_json(&status_file(&run)) {
                        failed_stage = current["stage_index"]
                            .as_u64()
                            .and_then(|i| u8::try_from(i).ok())
                            .unwrap_or(2);
                        failed_progress = current["progress"].as_f64().unwrap_or(15.0);
                        if let Some(rows) = current["videos"].as_array() {
                            videos = rows.clone();
                        }
                    }
                    let was_cancelled = error == "cancelled" || *cancelled.borrow();
                    status = running_json(
                        &run,
                        if was_cancelled { "cancelled" } else { "failed" },
                        if was_cancelled {
                            "Quick Reel отменён"
                        } else {
                            "Не удалось выполнить нативный GPU/CPU/монтаж"
                        },
                        failed_stage,
                        failed_progress,
                        &videos,
                        (!was_cancelled).then_some(error.as_str()),
                    );
                    status["inference_cache_reuse"] = json!(false);
                    // An unsuccessful reel MUST NOT publish the MP4.
                    let _ = fs::remove_file(run.root.join("quick_reel.mp4"));
                }
            }
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
        .map(|row| json!({"video":row.display_name.as_deref().unwrap_or(&row.name),"state":"queued","progress":0}))
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
    Ok(Json(json!({"id":id,"status":"queued","stage_total":4,
        "default_analysis":true,"inference_cache_reuse":false,
        "project_created":false,"native_gpu_inference_available":true})))
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
        return Ok(Json(json!({"status":"idle","stage_total":4})));
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
    fn quick_rally_priority_is_motion_evidence_not_invented_technique() {
        let entry = |id: &str, touches: u64, coverage: f64, frames: u64| RankedRally {
            run_id: "a".repeat(32),
            candidate_id: id.into(),
            touches,
            coverage,
            start: 50,
            end: 50 + frames - 1,
        };
        let selected = best_quick_rallies(vec![
            entry("unknown", 0, 1.0, 130),
            entry("weak", 1, 0.2, 120),
            entry("ranked", 5, 0.75, 150),
            entry("short", 20, 0.8, 30),
            entry("bad_coverage", 18, 0.05, 150),
        ]);
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].candidate_id, "ranked");
        assert_eq!(selected[1].candidate_id, "weak");
        assert!(best_quick_rallies(vec![]).is_empty());
    }

    fn fixture_state(root: &Path) -> AppState {
        let repository_root = root.join("original_repo");
        let data_root = root.join("sandbox");
        fs::create_dir_all(&repository_root).unwrap();
        fs::create_dir_all(&data_root).unwrap();
        AppState {
            repository_root,
            data_root,
            writes_enabled: true,
            montage: montage::MontageManager::new(),
            preparation: preparation::PreparationManager::new(),
            preview_limit: tokio::sync::Semaphore::new(1),
            touch_limit: tokio::sync::Semaphore::new(1),
            continuity_limit: tokio::sync::Semaphore::new(1),
            reid_limit: tokio::sync::Semaphore::new(1),
            identity_limit: tokio::sync::Semaphore::new(1),
            statistics_limit: tokio::sync::Semaphore::new(1),
            marks_limit: tokio::sync::Semaphore::new(1),
            quick_reel: QuickReelManager::new(),
            inference_limit: tokio::sync::Semaphore::new(1),
            full_inference: inference_full::FullInferenceManager::new(),
        }
    }
    fn ffmpeg(cmd: &[String]) {
        let output = std::process::Command::new("ffmpeg")
            .args(cmd)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "FFmpeg fixture: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    #[tokio::test]
    async fn run_private_native_gpu_preflight_never_reuses_project_inference_or_fakes_mp4() {
        if !std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_ok_and(|o| o.status.success())
        {
            eprintln!("SKIP stage78 Quick Reel pipeline isolation: FFmpeg not installed");
            return;
        }
        let base = std::env::temp_dir().join(format!("quick-reel-78-{}", Uuid::new_v4()));
        fs::create_dir_all(&base).unwrap();
        let state = Arc::new(fixture_state(&base));
        let video = base.join("rally.mp4");
        ffmpeg(
            &[
                "-nostdin",
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=640x360:rate=30:duration=3",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p",
                "-an",
            ]
            .iter()
            .map(|s| s.to_string())
            .chain([video.to_string_lossy().into_owned()])
            .collect::<Vec<_>>(),
        );
        let music = base.join("sample.wav");
        ffmpeg(
            &[
                "-nostdin",
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000:duration=1",
                "-c:a",
                "pcm_s16le",
            ]
            .iter()
            .map(|s| s.to_string())
            .chain([music.to_string_lossy().into_owned()])
            .collect::<Vec<_>>(),
        );
        let original = source_for(video.to_str().unwrap()).unwrap();
        let plan = PreparationPlan::new(&video, &state.data_root, None).unwrap();
        if plan.ready_path().is_none() {
            fs::create_dir_all(plan.output.parent().unwrap()).unwrap();
            fs::copy(&plan.source, &plan.output).unwrap();
            storage::write_json_atomic(
                &plan.manifest,
                &json!({
                    "identity":plan.identity,"status":"ready","progress":100
                }),
            )
            .unwrap();
        }
        let id = Uuid::new_v4().simple().to_string();
        let root = state.data_root.join(QUICK_ROOT).join(&id);
        fs::create_dir_all(&root).unwrap();
        let run = PreparedRun {
            id: id.clone(),
            sources: vec![original],
            plans: vec![plan],
            audio: music.to_string_lossy().into_owned(),
            audio_duration: 1.0,
            root: root.clone(),
        };
        storage::write_json_atomic(&root.join("request.json"), &planned_request(&run)).unwrap();
        let work = create_run_workspace(&state, &run).unwrap();
        assert!(
            !state.data_root.join("projects").exists(),
            "Quick Reel must never register a real user project"
        );
        assert_eq!(work.project_id, run.id[..12]);
        let info = crate::projects::existing_project(&work.app, &work.project_id).unwrap();
        assert_eq!(info["quick_reel_internal"], true);
        let inspected = prepared_plan_for_quick_reel(&work.app, &info, &work.aliases[0]).unwrap();
        let mut wrong_run = info.clone();
        wrong_run["quick_reel_run_id"] = json!("f".repeat(32));
        assert!(
            prepared_plan_for_quick_reel(&work.app, &wrong_run, &work.aliases[0]).is_err(),
            "Cross-run transient project must be rejected"
        );
        let mut wrong_source = info.clone();
        wrong_source["source_paths"][&work.aliases[0]] = json!("/tmp/other-source.mp4");
        assert!(
            prepared_plan_for_quick_reel(&work.app, &wrong_source, &work.aliases[0]).is_err(),
            "Quick Reel cannot import another project source without run identity"
        );
        assert_eq!(inspected.source, video.canonicalize().unwrap());
        assert!(inspected.ready_path().is_some());
        assert!(work
            .app
            .data_root
            .join("projects")
            .join(&work.project_id)
            .exists());
        assert!(work
            .app
            .data_root
            .starts_with(state.data_root.join("quick_reels")));

        let private_path = work.app.data_root.clone();
        drop(work);
        // An actual start always allocates its own new private workspace;
        // discard the inspected fixture before re-entering the runner.
        fs::remove_dir_all(private_path).unwrap();
        // With no ONNX models in the fixture's original_repo, the real CUDA
        // model job must fail, never publish fabricated CPU/MP4 evidence.
        let (_, receiver) = watch::channel(false);
        let result = tokio::time::timeout(
            Duration::from_secs(25),
            run_native_pipeline(&state, &run, vec![json!({"video":"rally.mp4"})], receiver),
        )
        .await
        .expect("Fresh CUDA/ONNX model load never terminated");
        let err = result.unwrap_err();
        assert!(
            err.contains("GPU") && err.contains("models"),
            "Missing real model must give explicit GPU failure: {err}"
        );
        assert!(!root.join("quick_reel.mp4").exists());
        assert_eq!(video.metadata().unwrap().len(), run.sources[0].size);
        assert!(
            !state.data_root.join("projects").exists(),
            "GPU adapter must not touch real project registry"
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[tokio::test]
    async fn cancellation_before_gpu_does_not_create_private_project_or_mp4() {
        let root = std::env::temp_dir().join(format!("quick-cancel-78-{}", Uuid::new_v4()));
        let state = Arc::new(fixture_state(&root));
        let run = PreparedRun {
            id: Uuid::new_v4().simple().to_string(),
            sources: vec![],
            plans: vec![],
            audio: String::new(),
            audio_duration: 0.0,
            root: root.join("no-run"),
        };
        let (_, receiver) = watch::channel(true);
        assert_eq!(
            run_native_pipeline(&state, &run, vec![], receiver)
                .await
                .unwrap_err(),
            "cancelled"
        );
        assert!(!state.data_root.join("projects").exists());
        assert!(!run.root.exists());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn source_token_cannot_traverse() {
        assert!(is_id("0123456789abcdef0123456789abcdef"));
        assert!(!is_id("../another-file"));
        assert!(!is_id("0123456789ABCDEF0123456789ABCDEF"));
    }
    #[test]
    fn windows_picker_script_encoding_is_utf16le_base64() {
        assert_eq!(powershell_command("A"), "QQA=");
        assert!(PICK_SCRIPT.contains("System.Windows.Forms.OpenFileDialog"));
        assert!(PICK_SCRIPT.contains("ConvertTo-Json"));
        assert!(!PICK_SCRIPT.contains("Invoke-Expression"));
    }

    #[tokio::test]
    async fn http_browser_drop_and_linked_selection_without_creating_user_project() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        if !std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_ok_and(|o| o.status.success())
        {
            eprintln!("SKIP HTTP browser drop integration without FFmpeg");
            return;
        }
        let root = std::env::temp_dir().join(format!("quick-http-79-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let state = Arc::new(fixture_state(&root));
        let video = root.join("original.mp4");
        ffmpeg(
            &[
                "-nostdin",
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x180:rate=30:duration=1.2",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p",
                "-an",
            ]
            .iter()
            .map(|s| s.to_string())
            .chain([video.to_string_lossy().into_owned()])
            .collect::<Vec<_>>(),
        );
        let original_size = fs::metadata(&video).unwrap().len();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = state.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, crate::app(app)).await.unwrap();
        });
        async fn http(
            addr: std::net::SocketAddr,
            verb: &str,
            route: &str,
            kind: &str,
            content: &[u8],
        ) -> (String, Value) {
            let mut connection = tokio::net::TcpStream::connect(addr).await.unwrap();
            let head=format!(
                "{verb} {route} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                content.len()
            );
            connection.write_all(head.as_bytes()).await.unwrap();
            connection.write_all(content).await.unwrap();
            // Content-Length is authoritative; let Hyper close the connection.
            let mut bytes = Vec::new();
            connection.read_to_end(&mut bytes).await.unwrap();
            let split = bytes
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .unwrap_or_else(|| {
                    panic!(
                        "HTTP response has no header delimiter ({} bytes): {:?}",
                        bytes.len(),
                        String::from_utf8_lossy(&bytes)
                    )
                })
                + 4;
            let response = String::from_utf8_lossy(&bytes[..split]).to_string();
            let json = serde_json::from_slice::<Value>(&bytes[split..]).unwrap();
            (response, json)
        }
        let selection = json!({"paths":[video.to_string_lossy().to_string()]});
        let (status, direct) = http(
            addr,
            "POST",
            "/api/quick-reel/native/select",
            "application/json",
            &serde_json::to_vec(&selection).unwrap(),
        )
        .await;
        assert!(status.starts_with("HTTP/1.1 200"), "{status}");
        assert_eq!(direct["videos"][0]["linked"], true);
        assert_eq!(direct["videos"][0]["copied"], false);
        let direct_id = direct["videos"][0]["id"].as_str().unwrap().to_owned();
        let boundary = "----native-quick-79";
        let mut body=format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"dropped.mp4\"\r\nContent-Type: video/mp4\r\n\r\n"
        ).into_bytes();
        body.extend(fs::read(&video).unwrap());
        body.extend(format!("\r\n--{boundary}--\r\n").as_bytes());
        let (status, uploaded) = http(
            addr,
            "POST",
            "/api/quick-reel/native/upload",
            &format!("multipart/form-data; boundary={boundary}"),
            &body,
        )
        .await;
        assert!(status.starts_with("HTTP/1.1 200"), "{status} {uploaded}");
        assert_eq!(uploaded["videos"][0]["name"], "dropped.mp4");
        assert_eq!(uploaded["videos"][0]["copied"], true);
        let copied_id = uploaded["videos"][0]["id"].as_str().unwrap().to_owned();
        let linked = read_source(&state, &direct_id).unwrap();
        let copied = read_source(&state, &copied_id).unwrap();
        assert!(copied.source.contains("quick_reels"));
        assert_eq!(copied.display_name.as_deref(), Some("dropped.mp4"));
        assert_eq!(linked.display_name, None);
        assert!(
            !state.data_root.join("projects").exists(),
            "Quick Reel uploads must never create project analysis storage"
        );
        let request = json!({"selection_ids":[direct_id,copied_id]});
        let (status, cleared) = http(
            addr,
            "POST",
            "/api/quick-reel/native/release",
            "application/json",
            &serde_json::to_vec(&request).unwrap(),
        )
        .await;
        assert!(status.starts_with("HTTP/1.1 200"), "{status} {cleared}");
        assert_eq!(cleared["released"], 2);
        assert!(!Path::new(&copied.source).exists());
        assert!(video.exists());
        assert_eq!(video.metadata().unwrap().len(), original_size);
        task.abort();
        let _ = task.await;
        fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn quick_reel_mp4_download_is_receipt_scoped_and_byte_ranged() {
        let root = std::env::temp_dir().join(format!("quick-media-79-{}", Uuid::new_v4()));
        let state = Arc::new(fixture_state(&root));
        let id = Uuid::new_v4().simple().to_string();
        let folder = state.data_root.join(QUICK_ROOT).join(&id);
        fs::create_dir_all(&folder).unwrap();
        assert!(
            completed_quick_media(&state, &id).is_err(),
            "Absent status must never serve media"
        );
        let video = vec![0x41u8; 4096];
        fs::write(folder.join("quick_reel.mp4"), &video).unwrap();
        let status = json!({
            "id":id,"status":"running","fresh_gpu_cpu":true,
            "inference_cache_reuse":false,
            "output":{"file":"quick_reel.mp4","size_bytes":4096}
        });
        let receipt = json!({
            "schema":"volley-native-quick-reel/2","id":id,
            "project_created":false,"source_type":"fresh_native_gpu_cpu",
            "output":{"file":"quick_reel.mp4","size_bytes":4096,
                "native_original_clips_verified":true,
                "mp4_quality":{"has_audio":true}}
        });
        storage::write_json_atomic(&folder.join("status.json"), &status).unwrap();
        storage::write_json_atomic(&folder.join("result.json"), &receipt).unwrap();
        assert!(
            completed_quick_media(&state, &id).is_err(),
            "Running MP4 is never downloadable"
        );
        let mut ready = status.clone();
        ready["status"] = json!("complete");
        storage::write_json_atomic(&folder.join("status.json"), &ready).unwrap();
        let mut request_headers = HeaderMap::new();
        request_headers.insert(header::RANGE, "bytes=20-29".parse().unwrap());
        let response = media(
            State(state.clone()),
            Query(StatusQuery {
                id: Some(id.clone()),
            }),
            request_headers.clone(),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.headers()[header::CONTENT_LENGTH], "10");
        assert_eq!(
            response.headers()[header::CONTENT_RANGE],
            "bytes 20-29/4096"
        );
        assert_eq!(response.headers()[header::CONTENT_TYPE], "video/mp4");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            response.headers()[header::CONTENT_DISPOSITION],
            "attachment; filename=\"quick_reel.mp4\""
        );
        let payload = axum::body::to_bytes(response.into_body(), 64)
            .await
            .unwrap();
        assert_eq!(payload.as_ref(), &[0x41u8; 10]);
        let mut invalid = receipt.clone();
        invalid["output"]["mp4_quality"]["has_audio"] = json!(false);
        storage::write_json_atomic(&folder.join("result.json"), &invalid).unwrap();
        assert!(
            completed_quick_media(&state, &id).is_err(),
            "Cannot download MP4 without verified AAC"
        );
        storage::write_json_atomic(&folder.join("result.json"), &receipt).unwrap();
        fs::write(folder.join("quick_reel.mp4"), vec![0u8; 123]).unwrap();
        assert!(
            completed_quick_media(&state, &id).is_err(),
            "Size mismatch denies download"
        );
        assert!(
            completed_quick_media(&state, "../a").is_err(),
            "Traversal denied"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn release_selection_removes_only_browser_copy_not_linked_original() {
        let root = std::env::temp_dir().join(format!("quick-release-79-{}", Uuid::new_v4()));
        let state = Arc::new(fixture_state(&root));
        let base = checked_work_root(&state, QUICK_ROOT).unwrap();
        let upload = base.join("uploads");
        fs::create_dir_all(&upload).unwrap();
        let original = root.join("original.mp4");
        fs::write(&original, b"untouched original media").unwrap();
        let dropped = upload.join(format!("drop_{}.mp4", Uuid::new_v4().simple()));
        fs::copy(&original, &dropped).unwrap();
        let build = |path: &Path, copy: bool| {
            let absolute = path.canonicalize().unwrap();
            let metadata = fs::metadata(&absolute).unwrap();
            Source {
                id: Uuid::new_v4().simple().to_string(),
                name: absolute.file_name().unwrap().to_string_lossy().to_string(),
                source: absolute.to_string_lossy().to_string(),
                size: metadata.len(),
                mtime_ns: mtime(&metadata).unwrap(),
                display_name: copy.then(|| "uploaded.mp4".into()),
            }
        };
        let files = [build(&dropped, true), build(&original, false)];
        checked_selections(&state).unwrap();
        for row in &files {
            storage::write_json_atomic(
                &selection_file(&state, &row.id).unwrap(),
                &serde_json::to_value(row).unwrap(),
            )
            .unwrap();
        }
        let Json(done) = release_sources(
            State(state.clone()),
            HeaderMap::new(),
            Json(ReleaseRequest {
                selection_ids: files.iter().map(|s| s.id.clone()).collect(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(done["released"], 2);
        assert!(!dropped.exists(), "Browser copies are removed");
        assert!(original.exists(), "Linked original is NEVER deleted");
        for row in &files {
            assert!(!selection_file(&state, &row.id).unwrap().exists());
        }
        fs::remove_dir_all(root).unwrap();
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
    fn incomplete_quick_reel_never_claims_finished_output() {
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
            "failed",
            "GPU CUDA/ONNX evidence missing",
            2,
            15.0,
            &[],
            None,
        );
        assert!(status["output"].is_null());
        assert_eq!(status["native_inference_available"], true);
        assert_eq!(status["stage_total"], 4);
    }
}
