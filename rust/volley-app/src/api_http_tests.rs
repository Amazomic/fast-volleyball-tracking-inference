//! In-process HTTP integration tests for the `/api/*` surface.
//!
//! Each test serves the real axum router on an ephemeral localhost port with
//! a disposable temporary data root, so the suite never touches the user's
//! live instance, repository projects or saved caches.

use super::*;
use std::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct TestServer {
    addr: SocketAddr,
    task: tokio::task::JoinHandle<()>,
    root: PathBuf,
}

impl TestServer {
    async fn spawn(writes_enabled: bool) -> Self {
        let root = std::env::temp_dir().join(format!("volley-api-http-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let state = Arc::new(AppState {
            repository_root: root.join("original"),
            data_root: root.join("sandbox"),
            writes_enabled,
            montage: montage::MontageManager::new(),
            preparation: preparation::PreparationManager::new(),
            preview_limit: tokio::sync::Semaphore::new(1),
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
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app(state)).await.unwrap();
        });
        Self { addr, task, root }
    }

    fn seed_project(&self, project_id: &str) {
        fs::create_dir_all(self.root.join("sandbox/projects").join(project_id)).unwrap();
        storage::write_json_atomic(
            &self.root.join("sandbox/projects/projects.json"),
            &serde_json::json!({"schema":1,"projects":[
                {"id":project_id,"name":"Match","videos":["match.mp4"]}]}),
        )
        .unwrap();
    }

    async fn request(&self, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
        let mut stream = tokio::net::TcpStream::connect(self.addr).await.unwrap();
        let body = body.unwrap_or("");
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut reply = Vec::new();
        stream.read_to_end(&mut reply).await.unwrap();
        let text = String::from_utf8_lossy(&reply).into_owned();
        let code = text
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse().ok())
            .unwrap_or(0);
        let body = text
            .split_once("\r\n\r\n")
            .map(|(_, body)| body.to_owned())
            .unwrap_or_default();
        (code, body)
    }

    async fn close(self) {
        self.task.abort();
        let _ = self.task.await;
        fs::remove_dir_all(self.root).unwrap();
    }
}

#[tokio::test]
async fn projects_api_returns_empty_list_on_fresh_root() {
    let server = TestServer::spawn(false).await;
    let (code, body) = server.request("GET", "/api/projects", None).await;
    assert_eq!(code, 200);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["projects"], serde_json::json!([]));
    server.close().await;
}

#[tokio::test]
async fn montage_cannot_start_while_project_deletion_holds_lock() {
    use fs2::FileExt;
    let server = TestServer::spawn(true).await;
    let id = "123456abcdef";
    server.seed_project(id);
    let locks = server.root.join("sandbox/projects/.project-locks");
    fs::create_dir_all(&locks).unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(locks.join(format!("{id}.lock")))
        .unwrap();
    lock.lock_exclusive().unwrap();
    let body = format!(r#"{{"project_id":"{id}","clips":[]}}"#);
    let (code, _) = server
        .request("POST", "/api/montage/native/start", Some(&body))
        .await;
    assert_eq!(code, 409);
    FileExt::unlock(&lock).unwrap();
    drop(lock);
    server.close().await;
}

#[tokio::test]
async fn analysis_api_renames_and_deletes_linked_results() {
    let server = TestServer::spawn(true).await;
    let project = "123456abcdef";
    let fusion = "11111111111111111111111111111111";
    let replay = "22222222222222222222222222222222";
    let rally = "33333333333333333333333333333333";
    server.seed_project(project);
    let root = server.root.join("sandbox/projects").join(project);
    for (folder, id, value) in [
        (
            "native_gpu_fusions",
            fusion,
            serde_json::json!({"schema":"volley-native-gpu-fusion/1","fusion_id":fusion}),
        ),
        (
            "native_cpu_replays",
            replay,
            serde_json::json!({"schema":"volley-native-cpu-tracking/1","replay_id":replay,"source_fusion_id":fusion}),
        ),
        (
            "native_cpu_rallies",
            rally,
            serde_json::json!({"schema":"volley-native-cpu-rally-proposals/1","rally_run_id":rally,"source_replay_id":replay}),
        ),
    ] {
        let dir = root.join(folder).join(id);
        fs::create_dir_all(&dir).unwrap();
        let mut value = value;
        value["project_id"] = serde_json::json!(project);
        value["video"] = serde_json::json!("match.mp4");
        storage::write_json_atomic(&dir.join("manifest.json"), &value).unwrap();
    }
    let rename = serde_json::json!({"project_id":project,"video":"match.mp4","kind":"fusion","id":fusion,"action":"rename","name":"Мой анализ"}).to_string();
    let (code, _) = server
        .request("POST", "/api/studio/analysis-rename", Some(&rename))
        .await;
    assert_eq!(code, 200);
    assert_eq!(
        analysis_management::display_name(&root, "fusion", fusion)
            .unwrap()
            .as_deref(),
        Some("Мой анализ")
    );
    let delete = serde_json::json!({"project_id":project,"video":"match.mp4","kind":"fusion","id":fusion,"action":"delete"}).to_string();
    let (code, _) = server
        .request("POST", "/api/studio/analysis-rename", Some(&delete))
        .await;
    assert_eq!(code, 400);
    assert!(root.join("native_gpu_fusions").join(fusion).exists());
    assert!(root.join("native_cpu_replays").join(replay).exists());
    assert!(root.join("native_cpu_rallies").join(rally).exists());
    let (code, _) = server
        .request("POST", "/api/studio/analysis-edit", Some(&delete))
        .await;
    assert_eq!(code, 200);
    assert!(!root.join("native_gpu_fusions").join(fusion).exists());
    assert!(!root.join("native_cpu_replays").join(replay).exists());
    assert!(!root.join("native_cpu_rallies").join(rally).exists());
    server.close().await;
}

#[tokio::test]
async fn preview_serves_compatible_original_instead_of_downscaled_analysis_copy() {
    check_preview_source(30, true).await;
}

#[tokio::test]
async fn preview_keeps_prepared_copy_for_nonmatching_frame_rate() {
    check_preview_source(25, false).await;
}

async fn check_preview_source(rate: u32, original: bool) {
    let server = TestServer::spawn(true).await;
    server.seed_project("123456abcdef");
    let source_dir = server.root.join("original/project");
    fs::create_dir_all(&source_dir).unwrap();
    let source = source_dir.join("match.mp4");
    let input = format!("testsrc2=size=1920x1080:rate={rate}");
    let status = std::process::Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            &input,
            "-frames:v",
            "3",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-y",
        ])
        .arg(&source)
        .status()
        .unwrap();
    assert!(status.success());
    let plan = volley_media::preparation::PreparationPlan::new(
        &source,
        &server.root.join("sandbox"),
        None,
    )
    .unwrap();
    assert_eq!(plan.width, 1280);
    fs::create_dir_all(plan.output.parent().unwrap()).unwrap();
    let status = std::process::Command::new("ffmpeg")
        .args(plan.ffmpeg_arguments(&plan.output))
        .status()
        .unwrap();
    assert!(status.success());
    let (size, sha) = volley_media::preparation::prepared_file_sha256(&plan.output).unwrap();
    storage::write_json_atomic(&plan.manifest, &serde_json::json!({"status":"ready","identity":plan.identity,"cache_size_bytes":size,"cache_sha256":sha})).unwrap();
    let (code, body) = server
        .request(
            "GET",
            "/api/video-media?project_id=123456abcdef&video=match.mp4",
            None,
        )
        .await;
    assert_eq!(code, 200);
    let expected = if original { &source } else { &plan.output };
    assert!(body == String::from_utf8_lossy(&fs::read(expected).unwrap()));
    server.close().await;
}

#[tokio::test]
async fn pipeline_status_is_idle_for_project_without_runs() {
    let server = TestServer::spawn(false).await;
    server.seed_project("123456abcdef");
    let (code, body) = server
        .request(
            "GET",
            "/api/studio/pipeline/status?project_id=123456abcdef",
            None,
        )
        .await;
    assert_eq!(code, 200);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["status"], "idle");
    server.close().await;
}

#[tokio::test]
async fn pipeline_status_rejects_unknown_project_with_404() {
    let server = TestServer::spawn(false).await;
    let (code, body) = server
        .request(
            "GET",
            "/api/studio/pipeline/status?project_id=ffffffffffff",
            None,
        )
        .await;
    assert_eq!(code, 404);
    assert!(body.contains("error"));
    server.close().await;
}

#[tokio::test]
async fn pipeline_status_rejects_malformed_project_id_with_400() {
    let server = TestServer::spawn(false).await;
    let (code, _) = server
        .request(
            "GET",
            "/api/studio/pipeline/status?project_id=..%2Fevil",
            None,
        )
        .await;
    assert_eq!(code, 400);
    server.close().await;
}

#[tokio::test]
async fn quick_reel_status_is_idle_on_writable_root() {
    let server = TestServer::spawn(true).await;
    let (code, body) = server
        .request("GET", "/api/quick-reel/native/status", None)
        .await;
    assert_eq!(code, 200);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["status"], "idle");
    assert_eq!(json["stage_total"], 4);
    server.close().await;
}

#[tokio::test]
async fn quick_reel_status_refuses_readonly_storage_with_409() {
    let server = TestServer::spawn(false).await;
    let (code, _) = server
        .request("GET", "/api/quick-reel/native/status", None)
        .await;
    assert_eq!(code, 409);
    server.close().await;
}

#[tokio::test]
async fn players_library_read_returns_empty_catalog_without_registry() {
    let server = TestServer::spawn(false).await;
    let (code, body) = server
        .request(
            "POST",
            "/api/players/library/native",
            Some(r#"{"action":"read"}"#),
        )
        .await;
    assert_eq!(code, 200);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["players"], serde_json::json!([]));
    assert_eq!(json["revision"], "absent");
    server.close().await;
}

#[tokio::test]
async fn pipeline_start_rejects_malformed_json_with_400() {
    let server = TestServer::spawn(false).await;
    let (code, _) = server
        .request("POST", "/api/studio/pipeline/start", Some("{not json"))
        .await;
    assert_eq!(code, 400);
    server.close().await;
}

#[tokio::test]
async fn unknown_api_route_returns_404() {
    let server = TestServer::spawn(false).await;
    let (code, _) = server
        .request("GET", "/api/definitely-not-a-route", None)
        .await;
    assert_eq!(code, 404);
    server.close().await;
}
