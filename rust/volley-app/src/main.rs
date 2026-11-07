//! Rust-native Studio entrypoint. Incomplete: never replace the running Python UI.
mod projects;

use axum::{routing::get, Json, Router};
use serde::Serialize;
use std::{env, error::Error, io, net::SocketAddr, path::PathBuf, sync::Arc};

#[derive(Clone)]
struct AppState {
    repository_root: PathBuf,
}

#[derive(Serialize)]
struct RewriteStatus {
    name: &'static str,
    migration_complete: bool,
    production_ready: bool,
    supported_api_endpoints: &'static [&'static str],
    pid: u32,
}

async fn rewrite_status() -> Json<RewriteStatus> {
    Json(RewriteStatus {
        name: "Volleyball Studio - standalone Rust rewrite (development only)",
        migration_complete: false,
        production_ready: false,
        supported_api_endpoints: &["GET /api/projects"],
        pid: std::process::id(),
    })
}

fn app(state: Arc<AppState>) -> Router {
    Router::new()
        // Deliberately no /api/health: the existing UI must NOT mistake this
        // partial service for a fully compatible replacement of API v28.
        .route("/internal/rewrite-status", get(rewrite_status))
        .route("/api/projects", get(projects::get_projects))
        .with_state(state)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let root = match env::var_os("VOLLEY_REPOSITORY_ROOT") {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
    };
    let root = root.canonicalize()?;
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

    // Exclusive listener: refuses to create a duplicate if a port is in use.
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let state = Arc::new(AppState {
        repository_root: root,
    });
    println!("Volleyball Studio Rust rewrite (INCOMPLETE): http://{addr}/internal/rewrite-status");
    axum::serve(listener, app(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
