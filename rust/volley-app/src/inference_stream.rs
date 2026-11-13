//! Bounded native FFmpeg -> persistent ONNX Runtime session diagnostic.
//! Reads a verified project/prepared-video source. Never writes GPU cache,
//! claims confirmed detections, or replaces a full inference pipeline.
use crate::{
    inference_clip::{ball_peaks, pixel_params},
    preparation, projects,
    storage::{self, ApiResult},
    AppState,
};
use axum::{extract::State, http::HeaderMap, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    fs,
    sync::{atomic::AtomicBool, Arc},
    time::UNIX_EPOCH,
};
use volley_inference::{
    decode::{decode_vball, RawDetection},
    person::PersonEngine,
    Config, Device, Engine, ModelKind,
};
use volley_media::{
    frame_windows::normalized_model_window,
    raw_decoder::{decode_windows, ModelPixels},
};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScanRequest {
    pub(crate) project_id: String,
    pub(crate) video: String,
    model: ModelKind,
    #[serde(default)]
    first_frame: u64,
    #[serde(default = "default_windows")]
    windows: usize,
    #[serde(default = "default_stride")]
    stride: usize,
    #[serde(default = "default_device")]
    device: Device,
    #[serde(default)]
    gpu_device_id: i32,
    #[serde(default)]
    gpu_memory_limit_mb: u64,
    #[serde(default = "default_threads")]
    cpu_threads: usize,
}
fn default_windows() -> usize {
    8
}
fn default_stride() -> usize {
    9
}
fn default_device() -> Device {
    Device::Cuda
}
fn default_threads() -> usize {
    8
}

fn checked_range(request: &ScanRequest, duration: f64) -> Result<(u64, u64), String> {
    if !(1..=120).contains(&request.windows) || !(1..=9).contains(&request.stride) {
        return Err("Окна: 1–120; шаг: 1–9 кадров".into());
    }
    let frames = 9_usize
        .checked_add(
            (request.windows - 1)
                .checked_mul(request.stride)
                .ok_or("Window overflow")?,
        )
        .ok_or("Window overflow")? as u64;
    let end = request
        .first_frame
        .checked_add(frames)
        .ok_or("Frame overflow")?;
    if end > 48 * 3600 * 30
        || !duration.is_finite()
        || duration <= 0.0
        || end as f64 / 30.0 > duration + 0.0005
    {
        return Err("Запрошенные окна выходят за пределы подготовленного видео".into());
    }
    Ok((request.first_frame, end))
}

pub(crate) fn source_stamp(path: &std::path::Path) -> Result<(u64, u128), String> {
    let meta = fs::metadata(path).map_err(|e| format!("Source file changed: {e}"))?;
    let ns = meta
        .modified()
        .map_err(|e| format!("Source mtime: {e}"))?
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("Source timestamp: {e}"))?
        .as_nanos();
    Ok((meta.len(), ns))
}

pub async fn scan(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<ScanRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    storage::project_id(&request.project_id)?;
    storage::video_name(&request.video)?;
    let config = Config {
        model: request.model,
        device: request.device,
        gpu_device_id: request.gpu_device_id,
        gpu_memory_limit_mb: request.gpu_memory_limit_mb,
        cpu_threads: request.cpu_threads,
    };
    config.verify().map_err(storage::bad_request)?;
    let project = projects::existing_project(&state, &request.project_id)?;
    if !project["videos"].as_array().is_some_and(|rows| {
        rows.iter()
            .any(|video| video.as_str() == Some(&request.video))
    }) {
        return Err(storage::bad_request("Видео не принадлежит проекту"));
    }
    let plan =
        preparation::plan_for_async(state.clone(), &request.project_id, &request.video).await?;
    checked_range(&request, plan.duration_sec).map_err(storage::bad_request)?;
    let file = preparation::ready_path_async(&plan)
        .await
        .map_err(storage::bad_request)?
        .ok_or_else(|| {
            storage::bad_request("Подготовленное видео не найдено; сначала запустите подготовку")
        })?;
    // One concurrent ONNX session + FFmpeg producer on this development server.
    let _permit = state
        .inference_limit
        .acquire()
        .await
        .map_err(|e| storage::internal(format!("Inference concurrency: {e}")))?;
    let before = source_stamp(&file).map_err(storage::bad_request)?;
    let repo = state.repository_root.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<Value, String> {
        let mut ball_engine = if request.model == ModelKind::Ball {
            Some(Engine::load(&repo, &config)?)
        } else {
            None
        };
        let mut person_engine = if request.model == ModelKind::Players {
            Some(PersonEngine::load(&repo, config.device, config.cpu_threads, config.gpu_memory_limit_mb)?)
        } else {
            None
        };
        let (w, h, channels, _) = pixel_params(request.model);
        let pixels = if request.model == ModelKind::Ball {
            ModelPixels::BallGray
        } else {
            ModelPixels::PlayersRgb
        };
        let mut proposals = Vec::new();
        let mut layout: Option<Vec<Value>> = None;
        let cancellation = AtomicBool::new(false);
        let summary = decode_windows(
            &file, pixels, request.first_frame, request.windows,
            request.stride, &cancellation, |window| {
                if let Some(engine) = person_engine.as_mut() {
                    let rgb_size = w * h * 3;
                    let mut raw = Vec::new();
                    let mut frames = Vec::new();
                    for (offset, rgb) in window.data.chunks_exact(rgb_size).enumerate() {
                        let people = engine.detect(rgb)?;
                        for (index, pose) in people.iter().enumerate() {
                            raw.push(RawDetection {
                                frame_index: window.first_frame + offset as u64,
                                class_name: "player",
                                score: pose.score,
                                bbox_xyxy_norm: pose.bbox.map(|v| v as f32),
                                query_index: Some(index),
                                touch_probability: None,
                                head_point_norm: pose.head.map(|p| p.map(|v| v as f32)),
                            });
                        }
                        frames.push(json!({
                            "frame": window.first_frame + offset as u64,
                            "people_with_skeletons": people.len(),
                        }));
                    }
                    if layout.is_none() {
                        layout = Some(vec![json!({"name":"RTMDet + RTMPose skeleton observations"})]);
                    }
                    proposals.push(json!({
                        "first_frame":window.first_frame,
                        "candidate_frames":frames,
                        "raw_detections":raw,
                    }));
                } else {
                    let inputs = normalized_model_window(&window.data, w, h, channels)?;
                    let engine = ball_engine.as_mut().ok_or("Missing VBallNet session")?;
                    let tensors = engine.infer_clip(inputs)?;
                    let current = tensors.iter().map(|tensor| json!({
                        "name":tensor.name,
                        "dimensions":tensor.dimensions,
                        "values":tensor.data.len(),
                    })).collect::<Vec<_>>();
                    match &layout {
                        Some(expected) if expected != &current =>
                            return Err("ONNX tensor layout changed during streaming".into()),
                        None => layout = Some(current),
                        _ => {}
                    }
                    let proposal = tensors.iter().find(|t| t.name == "heatmaps")
                        .ok_or("VBallNet output tensor missing")?;
                    proposals.push(json!({
                        "first_frame":window.first_frame,
                        "candidate_frames":ball_peaks(proposal, window.first_frame)?,
                        "raw_detections":decode_vball(&tensors, window.first_frame, 0.05)?,
                    }));
                }

                Ok(())
            },
        )?;
        if source_stamp(&file)? != before {
            return Err("Prepared video changed during inference scan".into());
        }
        Ok(json!({
            "schema":"volley-native-inference-stream/1",
            "project_id":request.project_id,
            "video":request.video,
            "model":request.model,
            "device":request.device,
            "gpu_device_id":request.gpu_device_id,
            "windows":summary.windows,
            "first_frame":summary.first_frame,
            "decoded_frames":summary.decoded_frames,
            "stride":summary.stride,
            "prepared_source":{"size_bytes":before.0,"mtime_ns":before.1},
            "tensor_layout":layout,
            "proposals":proposals,
            "raw_model_candidates_only":true,
            "identity_confirmed":false,
            "full_gpu_run_complete":false,
            "cpu_processing_complete":false,
            "saved":false,
            "note":"Bounded direct ONNX model scan; detections, tracks and run cache are not yet published.",
        }))
    }).await.map_err(|e| storage::internal(format!("Inference scan task failed: {e}")))?
    .map_err(storage::bad_request)?;
    Ok(Json(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(windows: usize, stride: usize, first_frame: u64) -> ScanRequest {
        ScanRequest {
            project_id: "1234abcdef01".into(),
            video: "game.mp4".into(),
            model: ModelKind::Ball,
            first_frame,
            windows,
            stride,
            device: Device::Cuda,
            gpu_device_id: 0,
            gpu_memory_limit_mb: 0,
            cpu_threads: 8,
        }
    }
    #[test]
    fn bounded_schedule_enforces_video_duration() {
        assert_eq!(checked_range(&fixture(3, 3, 0), 0.5).unwrap(), (0, 15));
        assert_eq!(checked_range(&fixture(1, 9, 30), 2.0).unwrap(), (30, 39));
        assert!(checked_range(&fixture(4, 9, 0), 1.0).is_err());
        assert!(checked_range(&fixture(121, 9, 0), 100.0).is_err());
        assert!(checked_range(&fixture(1, 0, 0), 100.0).is_err());
        assert!(checked_range(&fixture(1, 1, u64::MAX), 100.0).is_err());
    }
    #[test]
    fn strict_request_deserialization_rejects_bad_options() {
        let bad = serde_json::from_value::<ScanRequest>(
            json!({"project_id":"1234abcdef01","video":"game.mp4","model":"ball","escape":"../"}),
        );
        assert!(bad.is_err());
    }
}
