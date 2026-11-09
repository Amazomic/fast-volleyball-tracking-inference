//! Rust-native Studio HTTP application. The replacement is still incomplete.
mod analysis_runs;
mod catalog;
mod cpu_rallies;
mod cpu_retrack;
mod cpu_touches;
mod history;
mod inference_clip;
mod inference_evidence;
mod inference_full;
mod inference_fusion;
mod inference_native;
mod inference_stream;
mod montage;
mod native_player_tracks;
mod native_rally_marks;
mod native_rally_preview;
mod native_reid_cache;
mod playback;
mod player_catalog;
mod player_continuity;
mod player_identity;
mod player_montage;
mod player_statistics;
mod preparation;
mod project_archive;
mod projects;
mod quick_reel;
mod rallies;
mod rally_marks;
mod reid_group_edits;
mod reid_native;
mod reid_npz;
mod storage;
mod studio_analysis;
mod studio_web;
mod touch_preview;
mod versioned_store;

use axum::{
    response::Redirect,
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use std::{env, error::Error, io, net::SocketAddr, path::PathBuf, sync::Arc};

struct AppState {
    repository_root: PathBuf,
    data_root: PathBuf,
    writes_enabled: bool,
    montage: montage::MontageManager,
    preparation: preparation::PreparationManager,
    preview_limit: tokio::sync::Semaphore,
    touch_limit: tokio::sync::Semaphore,
    continuity_limit: tokio::sync::Semaphore,
    reid_limit: tokio::sync::Semaphore,
    identity_limit: tokio::sync::Semaphore,
    statistics_limit: tokio::sync::Semaphore,
    marks_limit: tokio::sync::Semaphore,
    quick_reel: quick_reel::QuickReelManager,
    inference_limit: tokio::sync::Semaphore,
    full_inference: inference_full::FullInferenceManager,
}

#[derive(Serialize)]
struct RewriteStatus {
    name: &'static str,
    migration_complete: bool,
    production_ready: bool,
    isolated_writes_enabled: bool,
    supported_api_endpoints: &'static [&'static str],
    pid: u32,
}

async fn rewrite_status(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> Json<RewriteStatus> {
    Json(RewriteStatus {
        name: "Volleyball Studio - standalone Rust rewrite (development only)",
        migration_complete: false,
        production_ready: false,
        isolated_writes_enabled: state.writes_enabled,
        supported_api_endpoints: &[
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
            "POST /api/project-archive/native (recoverable isolated archive)",
            "POST /api/montage/native/start (manual clips, isolated root)",
            "POST /api/montage/native/plan (CPU-based selection, read-only)",
            "POST /api/rallies/native/marks (read/set gold marks, isolated root)",
            "POST /api/quick-reel/native/select (explicit local video refs, isolated root)",
            "POST /api/quick-reel/native/start (preparation only; inference pending)",
            "GET /api/quick-reel/native/status",
            "POST /api/quick-reel/native/cancel",
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

fn app(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(|| async { Redirect::temporary("/studio/rust") }))
        // This is not /api/health: do not fool the production UI's API gate
        // into believing this partial Rust rewrite is ready.
        .route("/internal/rewrite-status", get(rewrite_status))
        .route("/studio/rust", get(studio_web::index))
        .route("/studio/rust/app.js", get(studio_web::js))
        .route("/studio/rust/tabs.js", get(studio_web::tabs_js))
        .route("/studio/rust/tracks.js", get(studio_web::tracks_js))
        .route(
            "/studio/rust/selection.wasm",
            get(studio_web::selection_wasm),
        )
        .route("/studio/rust/app.css", get(studio_web::css))
        .route("/api/cpu/native/rallies/runs", get(studio_web::runs))
        .route(
            "/api/studio/rust/analysis-overview",
            get(studio_analysis::get),
        )
        .route(
            "/api/cpu/native/rallies/marks",
            get(native_rally_marks::get).post(native_rally_marks::set),
        )
        .route("/api/projects", get(projects::get_projects))
        .route("/api/videos", get(catalog::get_videos))
        .route("/api/video-info", get(catalog::get_video_info))
        .route("/api/video-view", get(playback::video_view))
        .route("/api/video-media", get(playback::video_media))
        .route("/api/video-preview", get(playback::video_preview))
        .route("/api/analysis-resources", get(catalog::get_resources))
        .route("/api/analysis-profiles", get(catalog::get_profiles))
        .route("/api/analysis-history", get(history::get_history))
        .route("/api/analysis-runs", get(analysis_runs::get_runs))
        .route("/api/players/library", get(catalog::get_library))
        .route("/api/players/library/native", post(player_catalog::edit))
        .route(
            "/api/players/native/tracks",
            get(native_player_tracks::read).post(native_player_tracks::edit),
        )
        .route(
            "/api/players/native/track-photo",
            get(native_player_tracks::photo),
        )
        .route(
            "/api/cpu/native/ball-extrema",
            post(native_player_tracks::ball_trajectory),
        )
        .route(
            "/api/players/native/continuity",
            post(native_player_tracks::native_continuity),
        )
        .route(
            "/api/players/native/reid-review",
            post(native_player_tracks::reid_review),
        )
        .route(
            "/api/players/native/reid-groups",
            post(native_player_tracks::reid_preview),
        )
        .route(
            "/api/players/native/embeddings",
            post(native_player_tracks::embeddings),
        )
        .route(
            "/api/players/native/references",
            post(native_player_tracks::references),
        )
        .route(
            "/api/players/native/reference-photo",
            get(native_player_tracks::reference_photo),
        )
        .route("/api/project-create", post(projects::create_project))
        .route("/api/project-update", post(projects::update_project))
        .route(
            "/api/project-archive/native",
            post(project_archive::archive),
        )
        .route(
            "/api/montage/native/start",
            post(montage::start_native_montage),
        )
        .route("/api/montage/native/status", get(montage::native_status))
        .route("/api/montage/native/plan", post(rallies::preview_selection))
        .route(
            "/api/montage/native/start-from-rally-candidates",
            post(native_rally_preview::start_manual_montage),
        )
        .route("/api/rallies/native/marks", post(rally_marks::marks))
        .route(
            "/api/quick-reel/native/select",
            post(quick_reel::select_sources),
        )
        .route("/api/quick-reel/native/start", post(quick_reel::start))
        .route("/api/quick-reel/native/status", get(quick_reel::status))
        .route("/api/quick-reel/native/cancel", post(quick_reel::cancel))
        .route(
            "/api/inference/native/preflight",
            post(inference_native::preflight),
        )
        .route(
            "/api/inference/native/clip-preview",
            post(inference_clip::clip_preview),
        )
        .route(
            "/api/inference/native/window-scan",
            post(inference_stream::scan),
        )
        .route(
            "/api/inference/native/evidence/save",
            post(inference_evidence::save),
        )
        .route(
            "/api/inference/native/evidence",
            get(inference_evidence::read),
        )
        .route(
            "/api/inference/native/full/start",
            post(inference_full::start),
        )
        .route(
            "/api/inference/native/full/active",
            get(inference_full::active),
        )
        .route(
            "/api/inference/native/full/status",
            get(inference_full::status),
        )
        .route(
            "/api/inference/native/full/cancel",
            post(inference_full::cancel),
        )
        .route("/api/inference/native/fuse", post(inference_fusion::fuse))
        .route(
            "/api/inference/native/fusion",
            get(inference_fusion::manifest),
        )
        .route(
            "/api/cpu/native/retrack",
            post(cpu_retrack::replay).get(cpu_retrack::status),
        )
        .route(
            "/api/cpu/native/touches",
            post(cpu_touches::run).get(cpu_touches::status),
        )
        .route(
            "/api/cpu/native/rallies",
            post(cpu_rallies::run).get(cpu_rallies::status),
        )
        .route(
            "/api/cpu/native/rallies/preview",
            get(native_rally_preview::preview),
        )
        .route("/api/cpu/touch-preview", post(touch_preview::touch_preview))
        .route(
            "/api/players/statistics/native",
            post(player_statistics::statistics),
        )
        .route(
            "/api/players/identity/native",
            post(player_identity::read_or_edit),
        )
        .route(
            "/api/players/reid-native/preview",
            post(reid_native::preview),
        )
        .route(
            "/api/players/reid-native/groups",
            post(reid_group_edits::group_edits),
        )
        .route(
            "/api/players/continuity-preview",
            post(player_continuity::preview),
        )
        .route(
            "/api/montage/native/start-from-run",
            post(montage::start_native_from_run),
        )
        .route("/api/montage/native/cancel", post(montage::native_cancel))
        .route("/api/montage/native/media", get(montage::native_media))
        .route("/api/montage/native/library", get(montage::native_library))
        .route(
            "/api/montage/native/audio",
            post(montage::native_upload_audio)
                .layer(axum::extract::DefaultBodyLimit::max(91 * 1024 * 1024)),
        )
        .route(
            "/api/project-preparation",
            get(preparation::get_preparation),
        )
        .route(
            "/api/project-preparation/retry",
            post(preparation::retry_preparation),
        )
        .route(
            "/api/project-preparation/cancel",
            post(preparation::cancel_preparation),
        )
        .with_state(state)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let root = match env::var_os("VOLLEY_REPOSITORY_ROOT") {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
    };
    let root = root.canonicalize()?;
    let isolated = storage::writable_data_root(&root).map_err(|(_, message)| {
        io::Error::other(format!("Invalid Rust data root: {}", message.0))
    })?;
    let (data_root, writes_enabled) = match isolated {
        Some(root) => (root, true),
        None => (root.join("results"), false),
    };
    let addr: SocketAddr = env::var("VOLLEY_RUST_BIND")
        .unwrap_or_else(|_| "127.0.0.1:8875".to_owned())
        .parse()?;
    if !addr.ip().is_loopback() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the incomplete Rust development server must bind only to loopback",
        )
        .into());
    }

    // Exclusive port listener: cannot create a second instance on the same port.
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let state = Arc::new(AppState {
        repository_root: root,
        data_root,
        writes_enabled,
        montage: montage::MontageManager::new(),
        preparation: preparation::PreparationManager::new(),
        preview_limit: tokio::sync::Semaphore::new(2),
        touch_limit: tokio::sync::Semaphore::new(1),
        continuity_limit: tokio::sync::Semaphore::new(1),
        reid_limit: tokio::sync::Semaphore::new(1),
        identity_limit: tokio::sync::Semaphore::new(1),
        statistics_limit: tokio::sync::Semaphore::new(1),
        marks_limit: tokio::sync::Semaphore::new(1),
        quick_reel: quick_reel::QuickReelManager::new(),
        inference_limit: tokio::sync::Semaphore::new(1),
        full_inference: inference_full::FullInferenceManager::new(),
    });
    println!(
        "Volleyball Studio Rust rewrite (INCOMPLETE): http://{addr}/internal/rewrite-status (isolated writes={writes_enabled})"
    );
    axum::serve(listener, app(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
