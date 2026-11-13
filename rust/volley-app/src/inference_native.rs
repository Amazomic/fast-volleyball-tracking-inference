//! Isolated, read-only ONNX model and GPU provider preflight.
//! Never runs Python, never touches production inference caches, never
//! invents model inference success when ORT/CUDA cannot start.

use crate::{
    storage::{self, ApiResult},
    AppState,
};
use axum::{extract::State, http::HeaderMap, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use volley_inference::{person::PersonEngine, Config, Device, Engine, ModelKind};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreflightRequest {
    #[serde(default)]
    pub probe_runtime: bool,
    #[serde(default = "default_device")]
    pub device: Device,
    #[serde(default)]
    pub gpu_device_id: i32,
    #[serde(default)]
    pub gpu_memory_limit_mb: u64,
    #[serde(default = "default_threads")]
    pub cpu_threads: usize,
}
fn default_device() -> Device {
    Device::Cuda
}
fn default_threads() -> usize {
    8
}

pub async fn preflight(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<PreflightRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    let config = Config {
        model: ModelKind::Ball,
        device: request.device,
        gpu_device_id: request.gpu_device_id,
        gpu_memory_limit_mb: request.gpu_memory_limit_mb,
        cpu_threads: request.cpu_threads,
    };
    config.verify().map_err(storage::bad_request)?;
    let _permit = state
        .inference_limit
        .acquire()
        .await
        .map_err(|e| storage::internal(format!("ONNX preflight concurrency: {e}")))?;
    let path = state.repository_root.clone();
    let result=tokio::task::spawn_blocking(move ||{
        let ball=volley_inference::inspect(&path,ModelKind::Ball);
        let players=volley_inference::inspect(&path,ModelKind::Players);
        let manifest_valid=ball.is_ok()&&players.is_ok();
        let mut diagnostics=json!({
            "schema":"volley-native-inference-preflight/1",
            "device":request.device,
            "gpu_device_id":request.gpu_device_id,
            "gpu_memory_limit_mb":request.gpu_memory_limit_mb,
            "cpu_threads":request.cpu_threads,
            "vballnet_v4c":ball.as_ref().map(|m|serde_json::to_value(m).unwrap_or(Value::Null)).ok(),
            "person_pose":players.as_ref().map(|m|serde_json::to_value(m).unwrap_or(Value::Null)).ok(),
            "model_checks_passed":manifest_valid,
            "model_errors":[ball.err(),players.err()],
            "runtime_probed":request.probe_runtime,
            "native_clip_kernel_compiled":true,
            "native_video_pipeline_complete":false,
            "ready_for_quick_reel":false,
            "python_fallback":false,
        });
        if request.probe_runtime && manifest_valid {
            let mut sessions=Vec::new();
            for model in [ModelKind::Ball,ModelKind::Players]{
                let mut choice=config.clone();
                choice.model=model;
                if model == ModelKind::Players {
                    match PersonEngine::load(&path, choice.device, choice.cpu_threads, choice.gpu_memory_limit_mb) {
                        Ok(_) => sessions.push(json!({"model":"person_pose","session_opened":true,
                            "input":"RTMDet + RTMPose", "note":"Two ONNX sessions; no inference performed"})),
                        Err(e) => sessions.push(json!({"model":"person_pose","session_opened":false,"error":e})),
                    }
                    continue;
                }
                match Engine::load(&path,&choice) {
                    Ok(engine)=>sessions.push(json!({
                        "model":model,"session_opened":true,
                        "input":engine.spec().input_name,
                        "outputs":engine.spec().outputs,
                        "note":"No inference was executed by this preflight"
                    })),
                    Err(e)=>sessions.push(json!({
                        "model":model,"session_opened":false,
                        "error":e.chars().take(800).collect::<String>(),
                    })),
                }
            }
            diagnostics["runtime_sessions"]=json!(sessions);
        }
        diagnostics
    }).await.map_err(|e|storage::internal(format!("Native ONNX preflight: {e}")))?;
    Ok(Json(result))
}
