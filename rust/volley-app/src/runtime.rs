//! Rust-only process lifecycle: repository-scoped advisory lock and
//! truthful idle-job report. Never signals Python services or alters
//! historical project storage. A lock is held until process exit.
use fs2::FileExt;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub(crate) struct InstanceGuard {
    _file: File,
    #[cfg(test)]
    pub(crate) path: PathBuf,
}

pub(crate) fn lock_path(repository: &Path) -> io::Result<PathBuf> {
    let root = repository.canonicalize()?;
    let hash = Sha256::digest(root.to_string_lossy().as_bytes());
    Ok(std::env::temp_dir().join(format!("volley-rust-instance-{}.lock", format!("{hash:x}"))))
}
pub(crate) fn exclusive_instance(repository: &Path) -> io::Result<InstanceGuard> {
    let path = lock_path(repository)?;
    if let Ok(metadata) = fs::symlink_metadata(&path) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Rust instance lock must be a regular file",
            ));
        }
    }
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&path)?;
    file.try_lock_exclusive().map_err(|error| {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "Another Volleyball Studio Rust instance already holds this repository: {error}"
            ),
        )
    })?;
    Ok(InstanceGuard {
        _file: file,
        #[cfg(test)]
        path,
    })
}

/// The lifecycle launcher has a bounded status->signal gap. All incoming
/// requests pass this process-local gate: SIGTERM can atomically close it
/// against new requests BEFORE checking active native GPU/CPU/FFmpeg workers.
/// Existing requests hold a guard until handler completion/cancellation.
#[derive(Default)]
pub(crate) struct ShutdownGate {
    state: std::sync::Mutex<GateState>,
}
#[derive(Default)]
struct GateState {
    draining: bool,
    inflight: usize,
}
pub(crate) struct RequestHold {
    gate: std::sync::Arc<ShutdownGate>,
}
impl Drop for RequestHold {
    fn drop(&mut self) {
        let mut gate = self.gate.state.lock().unwrap_or_else(|e| e.into_inner());
        gate.inflight -= 1;
    }
}
impl ShutdownGate {
    pub(crate) fn enter(self: &std::sync::Arc<Self>) -> Option<RequestHold> {
        let mut gate = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if gate.draining {
            return None;
        }
        gate.inflight += 1;
        Some(RequestHold { gate: self.clone() })
    }
    fn begin_shutdown(&self) -> bool {
        let mut gate = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if gate.draining || gate.inflight != 0 {
            return false;
        }
        gate.draining = true;
        true
    }
    fn cancel_shutdown(&self) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .draining = false;
    }
    #[cfg(test)]
    fn is_draining(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .draining
    }
}
/// DNS-rebinding and cross-site navigation/CSRF protection on ALL routes,
/// including GET routes that may start preparation or expose local metadata.
/// CLI requests without Origin/Sec-Fetch-Site remain supported, but Host
/// must still be an explicit loopback hostname, never an attacker domain.
fn validate_local_request(headers: &axum::http::HeaderMap) -> Result<(), axum::http::StatusCode> {
    use axum::http::{header, StatusCode};
    let raw = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::FORBIDDEN)?;
    let host = raw.to_ascii_lowercase();
    let host_part = if let Some((prefix, port)) = host.rsplit_once(':') {
        if port.is_empty() || port.parse::<u16>().ok().filter(|p| *p > 0).is_none() {
            return Err(StatusCode::FORBIDDEN);
        }
        prefix
    } else {
        host.as_str()
    };
    if !matches!(host_part, "localhost" | "127.0.0.1" | "[::1]") {
        return Err(StatusCode::FORBIDDEN);
    }
    if let Some(origin) = headers.get(header::ORIGIN) {
        let trusted = format!("http://{raw}");
        if origin.to_str().ok() != Some(trusted.as_str()) {
            return Err(StatusCode::FORBIDDEN);
        }
    }
    if let Some(site) = headers.get("sec-fetch-site") {
        if !matches!(site.to_str().ok(), Some("same-origin" | "none")) {
            return Err(StatusCode::FORBIDDEN);
        }
    }
    Ok(())
}
pub(crate) async fn guard_request(
    axum::extract::State(gate): axum::extract::State<std::sync::Arc<ShutdownGate>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if let Err(status) = validate_local_request(request.headers()) {
        return status.into_response();
    }
    let Some(_hold) = gate.enter() else {
        return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    next.run(request).await
}
pub(crate) async fn try_shutdown(state: &AppState, gate: &ShutdownGate) -> bool {
    if !gate.begin_shutdown() {
        eprintln!("Rust shutdown denied: in-flight HTTP request or concurrent shutdown attempt");
        return false;
    }
    let busy = active_job_types(state).await;
    if !busy.is_empty() {
        gate.cancel_shutdown();
        eprintln!("Rust shutdown denied: active work: {}", busy.join(", "));
        return false;
    }
    // New HTTP work has been excluded since begin_shutdown was called.
    true
}
use crate::AppState;
use axum::{response::IntoResponse, Json};
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
pub(crate) struct RewriteStatus {
    name: &'static str,
    migration_complete: bool,
    production_ready: bool,
    isolated_writes_enabled: bool,
    pub(crate) shutdown_safe: bool,
    pub(crate) active_jobs: Vec<&'static str>,
    supported_api_endpoints: &'static [&'static str],
    pid: u32,
}

/// Status report is intentionally read-only. The launch script checks
/// shutdown_safe AND validates /proc/PID/exe and /proc/PID/cwd before SIGINT.
pub(crate) async fn active_job_types(state: &AppState) -> Vec<&'static str> {
    let mut busy = Vec::new();
    if state.full_inference.has_active().await {
        busy.push("gpu_inference");
    }
    if state.montage.has_active().await {
        busy.push("montage");
    }
    if state.quick_reel.has_active().await {
        busy.push("quick_reel");
    }
    if state.preparation.has_active().await {
        busy.push("video_preparation");
    }
    if state.inference_limit.available_permits() == 0 && !busy.contains(&"gpu_inference") {
        busy.push("gpu_inference");
    }
    if state.preview_limit.available_permits() == 0 {
        busy.push("preview_processing");
    }
    // Permit-based guards catch synchronous CPU mutations that have no
    // persistent job entry. Read-only browser preview is not an analysis.
    if state.touch_limit.available_permits() == 0 {
        busy.push("touch_processing");
    }
    if state.continuity_limit.available_permits() == 0 {
        busy.push("continuity_processing");
    }
    if state.reid_limit.available_permits() == 0 {
        busy.push("reid_processing");
    }
    if state.identity_limit.available_permits() == 0 {
        busy.push("identity_processing");
    }
    if state.statistics_limit.available_permits() == 0 {
        busy.push("statistics_processing");
    }
    if state.marks_limit.available_permits() == 0 {
        busy.push("marks_processing");
    }
    busy
}
pub(crate) async fn rewrite_status(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> Json<RewriteStatus> {
    let active_jobs = active_job_types(&state).await;
    Json(RewriteStatus {
        name: "Volleyball Studio - standalone Rust rewrite (development only)",
        migration_complete: false,
        production_ready: false,
        isolated_writes_enabled: state.writes_enabled,
        shutdown_safe:active_jobs.is_empty(),
        active_jobs,
        supported_api_endpoints: &[
            "GET /studio/rust/ui.js (WASM UI state bootstrap, RU/EN, preference controls)",
            "GET /api/projects",
            "GET /api/videos",
            "GET /api/video-info",
            "GET /api/video-view",
            "GET /api/video-media (Range bytes)",
            "GET /api/video-preview",
            "GET /api/project-preparation",
            "POST /api/project-preparation/retry (isolated root)",
            "POST /api/project-preparation/cancel (isolated root)",
            "GET /api/analysis-profiles",
            "GET /api/players/library",
            "POST /api/players/library/native (read, create, rename; isolated root)",
            "GET /api/analysis-resources",
            "POST /api/project-create (isolated data root only)",
            "POST /api/project-update (isolated data root only)",
            "GET /api/project-legacy/native/catalog (read-only historical registry)",
            "GET /api/project-legacy/native/inspect (media + run inventory, no automatic replay)",
            "POST /api/project-legacy/native/import (isolated metadata only, explicit consent)",
            "GET /api/project-archive/native (recoverable Rust-only archive list)",
            "POST /api/project-archive/native (recoverable isolated archive)",
            "POST /api/project-archive/native/restore (isolated rollback-safe restore)",
            "POST /api/montage/native/start (manual clips, isolated root)",
            "POST /api/montage/native/plan (CPU-based selection, read-only)",
            "POST /api/rallies/native/marks (read/set gold marks, isolated root)",
            "POST /api/quick-reel/native/select (explicit local video refs, isolated root)",
            "POST /api/quick-reel/native/pick (native Windows desktop dialog, original paths not copied)",
            "POST /api/quick-reel/native/upload (streamed browser drop, isolated 2 GiB/file)",
            "POST /api/quick-reel/native/release (safe selection/copy cleanup)",
            "POST /api/quick-reel/native/start (independent fresh CUDA GPU/CPU + original-source MP4; isolated root)",
            "GET /api/quick-reel/native/status",
            "POST /api/quick-reel/native/cancel",
            "GET /api/quick-reel/native/media (verified MP4, Range, attachment)",
            "POST /api/inference/native/preflight (read-only model + optional ONNX sessions)",
            "POST /api/inference/native/clip-preview (one actual 9-frame window, read-only)",
            "POST /api/inference/native/window-scan (bounded multi-window ONNX diagnostic, read-only)",
            "POST /api/inference/native/evidence/save (isolated raw candidate snapshot)",
            "GET /api/inference/native/evidence (read/list isolated snapshots)",
            "POST /api/inference/native/full/start (queued full prepared-video model scan)",
            "GET /api/inference/native/full/status (model scan progress)",
            "POST /api/inference/native/full/cancel (model scan cancellation)",
            "POST /api/inference/native/fuse (verify and merge two raw model runs)",
            "GET /api/inference/native/fusion (isolated fusion manifest)",
            "POST /api/cpu/native/retrack (CPU provisional Track ID snapshot)",
            "POST /api/cpu/native/touches (CPU-only motion-gated Touch candidates)",
            "POST /api/cpu/native/rallies (rebuild provisional rally candidates)",
            "GET /api/cpu/native/rallies (read isolated rally manifest)",
            "GET /api/cpu/native/rallies/preview (verified paginated motion evidence and safe manual clips)",
            "GET /api/cpu/native/rallies/runs (browse isolated provisional snapshots)",
            "GET/POST /api/cpu/native/rallies/marks (revisioned gold candidates)",
            "POST /api/cpu/native/actions/review (explicit human game-action labels)",
            "GET /studio/rust (standalone experimental preview/montage UI)",
            "GET /api/studio/rust/analysis-overview (metadata-only native GPU/CPU stage inventory)",
            "GET /api/inference/native/full/active (read-only reconnect to running GPU job)",
            "POST /api/montage/native/start-from-rally-candidates (explicit manual selection, original media)",
            "GET /api/cpu/native/touches (read isolated Touch snapshot manifest)",
            "GET /api/cpu/native/retrack (read isolated tracking manifest)",
            "GET/POST /api/players/native/tracks (manual provisional Track ID links for a selected CPU replay)",
            "GET /api/players/native/track-photo (on-demand isolated player crop preview)",
            "POST /api/players/native/reid-groups (cached, review-only cosine suggestions)",
            "POST /api/players/native/reid-review (manual revisioned group merge/split)",
            "POST /api/players/native/continuity (read-only gap suggestions)",
            "POST /api/players/native/statistics (verified per-athlete project-wide counts)",
            "POST /api/montage/native/player-highlights (reviewed attack ranked preview/start)",
            "POST /api/cpu/native/ball-extrema (native selected-ball motion only)",
            "POST /api/cpu/touch-preview (read-only RAVEL diagnostics)",
            "POST /api/players/continuity-preview (read-only geometry candidates)",
            "POST /api/players/reid-native/preview (saved embeddings, read-only)",
            "POST /api/players/reid-native/groups (read, merge, split; isolated root)",
            "POST /api/players/identity/native (manual review/edit, isolated root)",
            "POST /api/players/statistics/native (preview/recalculate, isolated root)",
            "POST /api/montage/native/start-from-run (CPU-backed, isolated root)",
            "GET /api/montage/native/status",
            "POST /api/montage/native/cancel (isolated root)",
            "GET /api/montage/native/library",
            "GET /api/montage/native/media (Range bytes)",
            "POST /api/montage/native/audio (multipart, isolated root)",
        ],
        pid: std::process::id(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;
    #[test]
    fn exclusive_same_repo_even_if_ports_or_data_dirs_differ() {
        let dir = std::env::temp_dir().join(format!("volley-runtime-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let first = exclusive_instance(&dir).unwrap();
        assert!(first
            .path
            .to_string_lossy()
            .contains("volley-rust-instance-"));
        assert_eq!(
            exclusive_instance(&dir).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        drop(first);
        let second = exclusive_instance(&dir).unwrap();
        drop(second);
        fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn idle_or_busy_server_report_guards_graceful_shutdown() {
        let root = std::env::temp_dir().join(format!("volley-busy-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let state = std::sync::Arc::new(crate::AppState {
            repository_root: root.clone(),
            data_root: root.clone(),
            writes_enabled: false,
            montage: crate::montage::MontageManager::new(),
            preparation: crate::preparation::PreparationManager::new(),
            preview_limit: tokio::sync::Semaphore::new(2),
            touch_limit: tokio::sync::Semaphore::new(1),
            continuity_limit: tokio::sync::Semaphore::new(1),
            reid_limit: tokio::sync::Semaphore::new(1),
            identity_limit: tokio::sync::Semaphore::new(1),
            statistics_limit: tokio::sync::Semaphore::new(1),
            marks_limit: tokio::sync::Semaphore::new(1),
            quick_reel: crate::quick_reel::QuickReelManager::new(),
            inference_limit: tokio::sync::Semaphore::new(1),
            full_inference: crate::inference_full::FullInferenceManager::new(),
        });
        assert!(super::active_job_types(&state).await.is_empty());
        let hold = state.statistics_limit.acquire().await.unwrap();
        assert_eq!(
            super::active_job_types(&state).await,
            vec!["statistics_processing"]
        );
        let axum::Json(status) = super::rewrite_status(axum::extract::State(state.clone())).await;
        assert!(!status.shutdown_safe);
        assert_eq!(status.active_jobs, vec!["statistics_processing"]);
        let gate = std::sync::Arc::new(ShutdownGate::default());
        let request = gate.enter().unwrap();
        assert!(
            !try_shutdown(&state, &gate).await,
            "In-flight HTTP request blocks simultaneous SIGTERM"
        );
        drop(request);
        assert!(
            !try_shutdown(&state, &gate).await,
            "Busy CPU worker blocks SIGTERM after HTTP releases"
        );
        drop(hold);
        assert!(try_shutdown(&state, &gate).await);
        assert!(gate.is_draining());
        assert!(
            gate.enter().is_none(),
            "Shutdown barrier rejects new requests after safe confirmation"
        );
        gate.cancel_shutdown();
        assert!(gate.enter().is_some());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn localhost_host_origin_and_fetch_site_gate_prevents_dns_rebinding_and_csrf() {
        use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("127.0.0.1:8875"));
        assert!(validate_local_request(&headers).is_ok());
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://127.0.0.1:8875"),
        );
        headers.insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
        assert!(validate_local_request(&headers).is_ok());
        headers.insert("sec-fetch-site", HeaderValue::from_static("cross-site"));
        assert_eq!(validate_local_request(&headers), Err(StatusCode::FORBIDDEN));
        headers.insert("sec-fetch-site", HeaderValue::from_static("same-site"));
        assert_eq!(validate_local_request(&headers), Err(StatusCode::FORBIDDEN));
        headers.insert("sec-fetch-site", HeaderValue::from_static("none"));
        assert!(validate_local_request(&headers).is_ok());
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://evil.example"),
        );
        assert_eq!(validate_local_request(&headers), Err(StatusCode::FORBIDDEN));
        headers.remove(header::ORIGIN);
        headers.insert(header::HOST, HeaderValue::from_static("evil.example:8875"));
        assert_eq!(validate_local_request(&headers), Err(StatusCode::FORBIDDEN));
        headers.insert(
            header::HOST,
            HeaderValue::from_static("localhost.evil.example:8875"),
        );
        assert_eq!(validate_local_request(&headers), Err(StatusCode::FORBIDDEN));
        headers.insert(header::HOST, HeaderValue::from_static("localhost:8875"));
        assert!(validate_local_request(&headers).is_ok());
        headers.insert(header::HOST, HeaderValue::from_static("127.0.0.1:0"));
        assert_eq!(validate_local_request(&headers), Err(StatusCode::FORBIDDEN));
        headers.remove(header::HOST);
        assert_eq!(validate_local_request(&headers), Err(StatusCode::FORBIDDEN));
    }
    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_instance_lock() {
        let dir = std::env::temp_dir().join(format!("volley-runtime-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join("target");
        fs::write(&target, b"DO NOT MODIFY").unwrap();
        let path = lock_path(&dir).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        let rejected = exclusive_instance(&dir);
        assert!(rejected.is_err());
        assert_eq!(fs::read(&target).unwrap(), b"DO NOT MODIFY");
        fs::remove_file(path).unwrap();
        fs::remove_dir_all(dir).unwrap();
    }
}
