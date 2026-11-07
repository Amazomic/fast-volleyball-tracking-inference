//! Rust-native Studio HTTP application. The replacement is still incomplete.
mod analysis_runs;
mod catalog;
mod history;
mod playback;
mod preparation;
mod projects;
mod storage;

use axum::{
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use std::{env, error::Error, io, net::SocketAddr, path::PathBuf, sync::Arc};

struct AppState {
    repository_root: PathBuf,
    data_root: PathBuf,
    writes_enabled: bool,
    preparation: preparation::PreparationManager,
    preview_limit: tokio::sync::Semaphore,
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
            "GET /api/analysis-resources",
            "POST /api/project-create (isolated data root only)",
            "POST /api/project-update (isolated data root only)",
        ],
        pid: std::process::id(),
    })
}

fn app(state: Arc<AppState>) -> Router {
    Router::new()
        // This is not /api/health: do not fool the production UI's API gate
        // into believing this partial Rust rewrite is ready.
        .route("/internal/rewrite-status", get(rewrite_status))
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
        .route("/api/project-create", post(projects::create_project))
        .route("/api/project-update", post(projects::update_project))
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
        preparation: preparation::PreparationManager::new(),
        preview_limit: tokio::sync::Semaphore::new(2),
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
