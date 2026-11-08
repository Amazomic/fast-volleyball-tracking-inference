//! Initial native Rust Studio preview shell, scoped to the isolated server.
//! Static browser bridge is a temporary frontend until wasm32 toolchain is
//! available; all business decisions and authority checks live in Rust.
use crate::{
    cpu_retrack::{opaque_id, safe_file, safe_folder},
    projects,
    storage::{self, ApiResult},
    AppState,
};
use axum::{
    extract::{Query, State},
    http::{header, HeaderValue},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{fs, sync::Arc, time::SystemTime};

const CSS: &str = include_str!("../static/studio.css");
const SCRIPT: &str = include_str!("../static/studio.js");
const TABS_SCRIPT: &str = include_str!("../static/studio_tabs.js");
const SELECTION_WASM: &[u8] = include_bytes!("../static/volley_web.wasm");
const HTML: &str = include_str!("../static/studio.html");

fn web_asset(mime: &'static str, contents: &'static str) -> Response {
    let mut response = contents.into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; connect-src 'self'; img-src 'self' data:; media-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'"
        ),
    );
    response
}
pub async fn index() -> Response {
    web_asset("text/html; charset=utf-8", HTML)
}
pub async fn css() -> Response {
    web_asset("text/css; charset=utf-8", CSS)
}
pub async fn js() -> Response {
    web_asset("text/javascript; charset=utf-8", SCRIPT)
}
pub async fn tabs_js() -> Response {
    web_asset("text/javascript; charset=utf-8", TABS_SCRIPT)
}
pub async fn selection_wasm() -> Response {
    let mut response = SELECTION_WASM.to_vec().into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/wasm"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunQuery {
    project_id: String,
}
fn valid_manifest(value: &Value, project_id: &str, run_id: &str, project_videos: &[&str]) -> bool {
    value["schema"] == "volley-native-cpu-rally-proposals/1"
        && value["state"] == "provisional_rally_segments"
        && value["project_id"] == project_id
        && value["rally_run_id"] == run_id
        && value["video"]
            .as_str()
            .is_some_and(|v| project_videos.contains(&v))
        && value["file"] == "rally_candidates.jsonl"
        && value["fps"] == 30
        && value["rallies_authoritatively_confirmed"] == false
        && value["montage_ready"] == false
        && value["rally_candidates"]
            .as_u64()
            .is_some_and(|n| n <= 200_000)
        && value["sha256"].as_str().is_some_and(|s| {
            s.len() == 64
                && s.bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        })
}
fn list_runs(state: &AppState, project_id: &str) -> Result<Value, crate::storage::ApiError> {
    storage::project_id(project_id)?;
    let project = projects::existing_project(state, project_id)?;
    let videos = project["videos"]
        .as_array()
        .map(|arr| arr.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_default();
    let project_root = projects::project_root(state).join(project_id);
    safe_folder(&project_root).map_err(storage::bad_request)?;
    let root = project_root.join("native_cpu_rallies");
    if !root.exists() {
        return Ok(json!({"project_id":project_id,"runs":[],"total_seen":0,
            "listing_truncated":false,"provisional_only":true}));
    }
    safe_folder(&root).map_err(storage::bad_request)?;
    let mut rows = Vec::new();
    let mut total = 0usize;
    for item in
        fs::read_dir(root).map_err(|e| storage::internal(format!("List native rally runs: {e}")))?
    {
        let item = item.map_err(|e| storage::internal(format!("Read native run entry: {e}")))?;
        let id = item.file_name().to_string_lossy().to_string();
        if !opaque_id(&id) {
            continue;
        }
        total += 1;
        if total > 1500 {
            return Err(storage::bad_request("Too many native rally runs to list"));
        }
        let folder = item.path();
        if safe_folder(&folder).is_err() {
            continue;
        }
        let manifest_file = match safe_file(&folder, "manifest.json", 16 * 1024) {
            Ok(file) => file,
            Err(_) => continue,
        };
        let Ok(data) = fs::read(manifest_file) else {
            continue;
        };
        let Ok(manifest) = serde_json::from_slice::<Value>(&data) else {
            continue;
        };
        if !valid_manifest(&manifest, project_id, &id, &videos) {
            continue;
        }
        let modified = fs::metadata(folder)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());
        rows.push((
            modified,
            json!({
                "rally_run_id":id,
                "video":manifest["video"],
                "candidate_count":manifest["rally_candidates"],
                "touch_candidate_count":manifest["touch_candidates_ingested"],
                "source_replay_id":manifest["source_replay_id"],
                "source_touch_id":manifest["source_touch_id"],
                "provisional":true,
            }),
        ));
    }
    rows.sort_by(|a, b| {
        b.0.cmp(&a.0).then_with(|| {
            b.1["rally_run_id"]
                .as_str()
                .cmp(&a.1["rally_run_id"].as_str())
        })
    });
    let truncated = rows.len() > 100;
    let entries = rows
        .into_iter()
        .take(100)
        .map(|(_, v)| v)
        .collect::<Vec<_>>();
    Ok(json!({
        "project_id":project_id,"runs":entries,"total_seen":total,
        "listing_truncated":truncated,"provisional_only":true
    }))
}
pub async fn runs(
    State(state): State<Arc<AppState>>,
    Query(query): Query<RunQuery>,
) -> ApiResult<Value> {
    let result = tokio::task::spawn_blocking(move || list_runs(&state, &query.project_id))
        .await
        .map_err(|e| storage::internal(format!("Native run listing: {e}")))??;
    Ok(Json(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn safe_rally_run_listing_accepts_only_named_provisional_scopes() {
        let v = json!({
            "schema":"volley-native-cpu-rally-proposals/1",
            "state":"provisional_rally_segments",
            "project_id":"1234abcdef01",
            "rally_run_id":"0123456789abcdef0123456789abcdef",
            "video":"volley.mp4",
            "file":"rally_candidates.jsonl","fps":30,
            "rallies_authoritatively_confirmed":false,
            "montage_ready":false,"rally_candidates":3,
            "sha256":"a".repeat(64)
        });
        assert!(valid_manifest(
            &v,
            "1234abcdef01",
            "0123456789abcdef0123456789abcdef",
            &["volley.mp4"]
        ));
        assert!(!valid_manifest(
            &v,
            "different",
            "0123456789abcdef0123456789abcdef",
            &["volley.mp4"]
        ));
        assert!(!valid_manifest(
            &v,
            "1234abcdef01",
            "0123456789abcdef0123456789abcdef",
            &["other.mp4"]
        ));
        let mut invalid = v.clone();
        invalid["montage_ready"] = json!(true);
        assert!(!valid_manifest(
            &invalid,
            "1234abcdef01",
            "0123456789abcdef0123456789abcdef",
            &["volley.mp4"]
        ));
    }
    #[tokio::test]
    async fn axum_serves_standalone_rust_ui_assets_over_http() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        async fn get(addr: std::net::SocketAddr, path: &str) -> String {
            let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            let request =
                format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
            stream.write_all(request.as_bytes()).await.unwrap();
            let mut reply = Vec::new();
            stream.read_to_end(&mut reply).await.unwrap();
            String::from_utf8_lossy(&reply).into_owned()
        }
        let root =
            std::env::temp_dir().join(format!("volley-studio-http-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let state = Arc::new(AppState {
            repository_root: root.clone(),
            data_root: root.join("sandbox"),
            writes_enabled: false,
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
            full_inference: crate::inference_full::FullInferenceManager::new(),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, crate::app(state)).await.unwrap();
        });
        let html = get(addr, "/studio/rust").await;
        assert!(html.starts_with("HTTP/1.1 200 OK"));
        assert!(html.to_lowercase().contains("content-security-policy:"));
        assert!(html.contains("id=\"studio-rallies\""));
        let css = get(addr, "/studio/rust/app.css").await;
        assert!(css.starts_with("HTTP/1.1 200 OK"));
        assert!(css.contains("text/css; charset=utf-8"));
        assert!(css.contains("rally-card.gold"));
        let js = get(addr, "/studio/rust/app.js").await;
        assert!(js.starts_with("HTTP/1.1 200 OK"));
        assert!(js.contains("rallies/marks"));
        let tabs = get(addr, "/studio/rust/tabs.js").await;
        assert!(tabs.starts_with("HTTP/1.1 200 OK"));
        assert!(tabs.contains("/api/studio/rust/analysis-overview"));
        assert!(tabs.contains("/api/players/library/native"));
        let wasm = get(addr, "/studio/rust/selection.wasm").await;
        assert!(wasm.starts_with("HTTP/1.1 200 OK"));
        assert!(wasm.contains("application/wasm"));
        assert!(SELECTION_WASM.len() > 1000);
        assert_eq!(&SELECTION_WASM[..4], b"\0asm");
        assert!(tabs.to_lowercase().contains("content-security-policy:"));
        let status = get(addr, "/internal/rewrite-status").await;
        assert!(status.contains("\"production_ready\":false"));
        let no_health = get(addr, "/api/health").await;
        assert!(no_health.starts_with("HTTP/1.1 404 Not Found"));
        task.abort();
        let _ = task.await;
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_ui_has_strict_csp_and_no_legacy_health_gate() {
        let page = web_asset("text/html; charset=utf-8", HTML);
        assert!(page.headers().contains_key(header::CONTENT_SECURITY_POLICY));
        assert!(HTML.contains("id=\"studio-rallies\""));
        assert!(HTML.contains("id=\"studio-project\""));
        assert!(HTML.contains("id=\"studio-analysis-view\""));
        assert!(HTML.contains("id=\"studio-players-view\""));
        assert!(HTML.contains("/studio/rust/tabs.js"));
        assert!(!TABS_SCRIPT.contains("innerHTML"));
        assert!(!HTML.contains("http://127.0.0.1:8765"));
        assert!(SCRIPT.contains("textContent"));
        assert!(!SCRIPT.contains("innerHTML"));
    }
}
