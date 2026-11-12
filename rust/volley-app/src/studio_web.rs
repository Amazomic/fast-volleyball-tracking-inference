//! Native Rust Studio shell with compiled Rust/WASM navigation, preferences
//! and bilingual UI text plus browser-only DOM/network adapters.
//! All business decisions and evidence checks remain in native Rust services;
//! browser JS only owns DOM, media, file picker and same-origin transport.
use crate::{
    cpu_retrack::{opaque_id, safe_file, safe_folder},
    projects,
    storage::{self, ApiResult},
    AppState,
};
use axum::{
    extract::{Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{fs, sync::Arc, time::SystemTime};

const UI_CONTRACT: &str = include_str!("../static/ui-contract.txt");
const CSS: &str = include_str!("../static/studio.css");
const VOLLEYBALL_SVG: &str = include_str!("../static/volleyball.svg");
const VOLLEYBALL_PHOTO_PNG: &[u8] = include_bytes!("../static/volleyball-photo.png");
const SCRIPT: &str = include_str!("../static/studio.js");
const UPLOAD_SCRIPT: &str = include_str!("../static/studio_upload.js");
const HASH_SCRIPT: &str = include_str!("../static/studio_hash.js");
const EDITOR_SCRIPT: &str = include_str!("../static/studio_editor.js");
const TASKS_SCRIPT: &str = include_str!("../static/studio_tasks.js");
const OVERVIEW_SCRIPT: &str = include_str!("../static/studio_overview.js");
const CHAIN_SCRIPT: &str = include_str!("../static/studio_chain.js");
const UI_SCRIPT: &str = include_str!("../static/studio_ui.js");
const TABS_SCRIPT: &str = include_str!("../static/studio_tabs.js");
const TRACKS_SCRIPT: &str = include_str!("../static/studio_tracks.js");
const STATS_SCRIPT: &str = include_str!("../static/studio_stats.js");
const HIGHLIGHTS_SCRIPT: &str = include_str!("../static/studio_highlights.js");
const QUICK_REEL_SCRIPT: &str = include_str!("../static/studio_quick_reel.js");
const SELECTION_WASM: &[u8] = include_bytes!("../static/volley_web.wasm");
const HTML: &str = include_str!("../static/studio.html");

fn web_asset(mime: &'static str, contents: &str) -> Response {
    let mut response = contents.to_owned().into_response();
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
fn development_asset(root: &std::path::Path, filename: &str) -> Result<String, ()> {
    let contract = fs::read_to_string(root.join("ui-contract.txt")).map_err(|_| ())?;
    if contract != UI_CONTRACT {
        return Err(());
    }
    fs::read_to_string(root.join(filename)).map_err(|_| ())
}
fn studio_asset_from(
    root: Option<&std::path::Path>,
    mime: &'static str,
    filename: &'static str,
    embedded: &'static str,
) -> Response {
    match root {
        Some(root) => match development_asset(root, filename) {
            Ok(contents) => web_asset(mime, &contents),
            Err(()) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "Development UI unavailable or incompatible; rebuild and restart Studio",
            )
                .into_response(),
        },
        None => web_asset(mime, embedded),
    }
}
fn studio_asset(mime: &'static str, filename: &'static str, embedded: &'static str) -> Response {
    let root = if cfg!(debug_assertions) {
        std::env::var_os("VOLLEY_DEV_UI_ROOT")
    } else {
        None
    };
    studio_asset_from(
        root.as_deref().map(std::path::Path::new),
        mime,
        filename,
        embedded,
    )
}
pub async fn index() -> Response {
    studio_asset("text/html; charset=utf-8", "studio.html", HTML)
}
pub async fn volleyball_svg() -> Response {
    studio_asset(
        "image/svg+xml; charset=utf-8",
        "volleyball.svg",
        VOLLEYBALL_SVG,
    )
}

pub async fn volleyball_photo_png() -> Response {
    let mut response = VOLLEYBALL_PHOTO_PNG.to_vec().into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
pub async fn css() -> Response {
    studio_asset("text/css; charset=utf-8", "studio.css", CSS)
}
pub async fn hash_js() -> Response {
    studio_asset(
        "text/javascript; charset=utf-8",
        "studio_hash.js",
        HASH_SCRIPT,
    )
}
pub async fn upload_js() -> Response {
    studio_asset(
        "text/javascript; charset=utf-8",
        "studio_upload.js",
        UPLOAD_SCRIPT,
    )
}
pub async fn editor_js() -> Response {
    studio_asset(
        "text/javascript; charset=utf-8",
        "studio_editor.js",
        EDITOR_SCRIPT,
    )
}
pub async fn tasks_js() -> Response {
    studio_asset(
        "text/javascript; charset=utf-8",
        "studio_tasks.js",
        TASKS_SCRIPT,
    )
}
pub async fn overview_js() -> Response {
    studio_asset(
        "text/javascript; charset=utf-8",
        "studio_overview.js",
        OVERVIEW_SCRIPT,
    )
}
pub async fn js() -> Response {
    studio_asset("text/javascript; charset=utf-8", "studio.js", SCRIPT)
}
pub async fn chain_js() -> Response {
    studio_asset(
        "text/javascript; charset=utf-8",
        "studio_chain.js",
        CHAIN_SCRIPT,
    )
}
pub async fn ui_js() -> Response {
    studio_asset("text/javascript; charset=utf-8", "studio_ui.js", UI_SCRIPT)
}
pub async fn tabs_js() -> Response {
    studio_asset(
        "text/javascript; charset=utf-8",
        "studio_tabs.js",
        TABS_SCRIPT,
    )
}
pub async fn highlights_js() -> Response {
    studio_asset(
        "text/javascript; charset=utf-8",
        "studio_highlights.js",
        HIGHLIGHTS_SCRIPT,
    )
}
pub async fn quick_reel_js() -> Response {
    studio_asset(
        "text/javascript; charset=utf-8",
        "studio_quick_reel.js",
        QUICK_REEL_SCRIPT,
    )
}
pub async fn stats_js() -> Response {
    studio_asset(
        "text/javascript; charset=utf-8",
        "studio_stats.js",
        STATS_SCRIPT,
    )
}
pub async fn tracks_js() -> Response {
    studio_asset(
        "text/javascript; charset=utf-8",
        "studio_tracks.js",
        TRACKS_SCRIPT,
    )
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
                "created_at":modified,
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
    #[test]
    fn development_assets_reload_and_reject_incompatible_contracts() {
        let dir = std::env::temp_dir().join(format!("volley-ui-assets-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("ui-contract.txt"), UI_CONTRACT).unwrap();
        fs::write(dir.join("studio.css"), "first").unwrap();
        assert_eq!(development_asset(&dir, "studio.css").unwrap(), "first");
        fs::write(dir.join("studio.css"), "second").unwrap();
        assert_eq!(development_asset(&dir, "studio.css").unwrap(), "second");
        fs::write(dir.join("ui-contract.txt"), "incompatible").unwrap();
        assert!(development_asset(&dir, "studio.css").is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn development_responses_refresh_and_fail_closed() {
        let dir = std::env::temp_dir().join(format!("volley-ui-http-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("ui-contract.txt"), UI_CONTRACT).unwrap();
        fs::write(dir.join("studio.js"), "first").unwrap();
        let response = studio_asset_from(Some(&dir), "text/javascript", "studio.js", "embedded");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            axum::body::to_bytes(response.into_body(), 100)
                .await
                .unwrap(),
            "first"
        );
        fs::write(dir.join("studio.js"), "second").unwrap();
        let response = studio_asset_from(Some(&dir), "text/javascript", "studio.js", "embedded");
        assert_eq!(
            axum::body::to_bytes(response.into_body(), 100)
                .await
                .unwrap(),
            "second"
        );
        fs::write(dir.join("ui-contract.txt"), "2").unwrap();
        assert_eq!(
            studio_asset_from(Some(&dir), "text/javascript", "studio.js", "embedded").status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        let response = studio_asset_from(None, "text/javascript", "studio.js", "embedded");
        assert_eq!(
            axum::body::to_bytes(response.into_body(), 100)
                .await
                .unwrap(),
            "embedded"
        );
        fs::remove_dir_all(dir).unwrap();
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
        let html = get(addr, "/studio").await;
        assert!(html.starts_with("HTTP/1.1 200 OK"));
        let home = get(addr, "/").await;
        assert!(home.starts_with("HTTP/1.1 307 Temporary Redirect"));
        assert!(home.to_lowercase().contains("location: /studio"));
        assert!(html.to_lowercase().contains("content-security-policy:"));
        assert!(html.contains("id=\"studio-rallies\""));
        assert!(html.contains("id=\"studio-layout\""));
        assert!(html.contains("id=\"studio-volume\""));
        assert!(html.contains("id=\"studio-highlights-layout\""));
        assert!(html.contains("id=\"studio-highlights-music\""));
        assert!(!html.contains("studio-nav-legacy"));
        assert!(!html.contains("id=\"studio-ui-locale\""));
        assert!(html.contains("id=\"studio-inference\""));
        assert!(html.contains("id=\"studio-run\""));
        assert!(html.contains("/studio/chain.js"));
        assert!(!html.contains("id=\"studio-ui-theme\""));
        assert!(!html.contains("id=\"studio-ui-density\""));
        assert!(!html.contains("id=\"studio-ui-reduced-motion\""));
        assert!(!html.contains("id=\"studio-ui-reset\""));
        assert!(html.contains("id=\"studio-gpu-stride\""));
        assert!(html.contains("id=\"studio-gpu-memory\""));
        assert!(html.contains("id=\"studio-gpu-threads\""));
        assert!(html.contains("id=\"studio-quick-open\""));
        assert!(!html.contains("id=\"studio-nav-quick\""));
        assert!(!html.contains("id=\"studio-quick-summary\""));
        assert!(html.contains("id=\"studio-quick-drop\""));
        assert!(html.contains("id=\"studio-quick-progress\""));
        assert!(html.contains("id=\"studio-quick-cancel\""));
        assert!(html.contains("id=\"studio-quick-download\""));
        let css = get(addr, "/studio/app.css").await;
        assert!(css.starts_with("HTTP/1.1 200 OK"));
        assert!(css.contains("text/css; charset=utf-8"));
        assert!(css.contains("rally-card.gold"));
        assert!(css.contains("data-ui-theme"));
        assert!(css.contains("data-ui-density"));
        assert!(css.contains("data-ui-motion"));
        let chain = get(addr, "/studio/chain.js").await;
        assert!(chain.starts_with("HTTP/1.1 200 OK"));
        assert!(chain.contains("text/javascript; charset=utf-8"));
        assert!(chain.contains("volley_domain_call"));
        assert!(!chain.contains("source_fusion_id"));
        let js = get(addr, "/studio/app.js").await;
        assert!(js.starts_with("HTTP/1.1 200 OK"));
        assert!(js.contains("rallies/marks"));
        assert!(js.contains("layout:") || js.contains("layout,"));
        assert!(js.contains("audio_volume:"));
        let runtime = get(addr, "/studio/ui.js").await;
        assert!(runtime.starts_with("HTTP/1.1 200 OK"));
        assert!(runtime.contains("volley_ui_api_version"));
        assert!(runtime.contains("volley_ui_begin_load"));
        assert!(runtime.contains("volley_ui_text_count"));
        assert!(!runtime.contains("innerHTML"));
        assert!(html.contains("/studio/ui.js"));
        let tabs = get(addr, "/studio/tabs.js").await;
        assert!(tabs.starts_with("HTTP/1.1 200 OK"));
        assert!(tabs.contains("/api/studio/pipeline/start"));
        assert!(tabs.contains("/api/studio/pipeline/status"));
        assert!(tabs.contains("/api/studio/pipeline/log"));
        assert!(tabs.contains("/api/studio/pipeline/cancel"));
        assert!(!tabs.contains("studio-stage-list"));
        assert!(tabs.contains("/api/players/library/native"));
        assert!(tabs.contains("runtime.navigate("));
        assert!(tabs.contains("gpu_memory_limit_mb"));
        assert!(tabs.contains("runtime.currentLoad("));
        let tracks = get(addr, "/studio/tracks.js").await;
        assert!(tracks.starts_with("HTTP/1.1 200 OK"));
        assert!(tracks.contains("/api/players/native/tracks"));
        assert!(tracks.contains("/api/players/native/track-photo"));
        assert!(tracks.contains("/api/players/native/reid-review"));
        assert!(tracks.contains("/api/players/native/embeddings"));
        let stats = get(addr, "/studio/stats.js").await;
        assert!(stats.starts_with("HTTP/1.1 200 OK"));
        assert!(stats.contains("/api/players/native/statistics"));
        assert!(stats.contains("source_fingerprint"));
        let highlights = get(addr, "/studio/highlights.js").await;
        assert!(highlights.starts_with("HTTP/1.1 200 OK"));
        assert!(highlights.contains("/api/montage/native/player-highlights"));
        assert!(highlights.contains("/api/montage/native/cancel"));
        assert!(highlights.contains("/api/montage/native/audio"));
        assert!(highlights.contains("expected_source_fingerprint"));
        let quick = get(addr, "/studio/quick-reel.js").await;
        assert!(quick.starts_with("HTTP/1.1 200 OK"));
        assert!(quick.contains("/api/quick-reel/native/pick"));
        for action in [
            "native/upload",
            "native/release",
            "native/start",
            "native/status",
            "native/cancel",
            "native/media",
        ] {
            assert!(
                quick.contains(action),
                "Missing Quick Reel client action {action}"
            );
        }
        assert!(!quick.contains("innerHTML"));
        assert!(get(addr, "/studio/legacy.js")
            .await
            .starts_with("HTTP/1.1 404 Not Found"));
        assert!(!tracks.contains("innerHTML"));
        let wasm = get(addr, "/studio/selection.wasm").await;
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
        assert!(HTML.contains("/studio/tabs.js"));
        assert!(HTML.contains("/studio/tracks.js"));
        assert!(HTML.contains("id=\"studio-native-track-panel\""));
        assert!(HTML.contains("id=\"studio-track-list\""));
        assert!(HTML.contains("id=\"studio-player-list\""));
        assert!(!TRACKS_SCRIPT.contains("innerHTML"));
        assert!(!TABS_SCRIPT.contains("innerHTML"));
        assert!(!HTML.contains("http://127.0.0.1:8765"));
        assert!(SCRIPT.contains("textContent"));
        assert!(!SCRIPT.contains("innerHTML"));
    }
}
