//! Read-only compatibility with the original projects.json registry.
//! This is a genuine Rust-native file reader, never a Python subprocess.
//! Mutations and remaining API routes are intentionally not implemented yet.

use crate::AppState;
use axum::{extract::State, http::StatusCode, Json};
use serde_json::{json, Value};
use std::{io::ErrorKind, sync::Arc};

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<Value>)>;

pub async fn get_projects(State(state): State<Arc<AppState>>) -> ApiResult<Vec<Value>> {
    let register = state
        .repository_root
        .join("results")
        .join("projects")
        .join("projects.json");

    let raw = match tokio::fs::read(register).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Json(Vec::new())),
        Err(error) => {
            eprintln!("Rust project registry read failed: {error}");
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Project registry cannot be read" })),
            ));
        }
    };

    let parsed: Value = serde_json::from_slice(&raw).map_err(|error| {
        eprintln!("Rust project registry JSON is invalid: {error}");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "Project registry JSON is invalid" })),
        )
    })?;

    // Keep all project fields verbatim: existing projects contain additional
    // per-video ranges, global player links and source-path metadata.
    let projects = parsed
        .get("projects")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Unsupported project registry structure" })),
            )
        })?;
    Ok(Json(projects.clone()))
}
