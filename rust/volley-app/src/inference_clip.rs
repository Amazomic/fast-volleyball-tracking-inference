//! One real 9-frame ONNX inference from a validated prepared project video.
//! This diagnostic endpoint is read-only and never writes the original CPU
//! analysis result, creates auto-identities, or reports a completed inference.
//! It intentionally cannot run without the native ONNX Runtime shared library.

use crate::{
    preparation, projects,
    storage::{self, ApiError, ApiResult},
    AppState,
};
use axum::{extract::State, http::HeaderMap, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{path::PathBuf, process::Stdio, sync::Arc, time::Duration};
use tokio::{io::AsyncReadExt, process::Command};
use volley_inference::{Config, Device, Engine, ModelKind, Output};
const CLIP: usize = 9;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipRequest {
    pub project_id: String,
    pub video: String,
    pub first_frame: u64,
    pub model: ModelKind,
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

pub(crate) fn pixel_params(kind: ModelKind) -> (usize, usize, usize, &'static str) {
    match kind {
        ModelKind::Ball => (512, 288, 1, "gray"),
        ModelKind::Players => (640, 360, 3, "rgb24"),
    }
}
fn normalize_pixels(bytes: &[u8], kind: ModelKind) -> Result<Vec<f32>, String> {
    let (width, height, channels, _) = pixel_params(kind);
    volley_media::frame_windows::normalized_model_window(bytes, width, height, channels)
}
async fn decode_nine(prepared: PathBuf, request: &ClipRequest) -> Result<Vec<f32>, ApiError> {
    let (w, h, _, format) = pixel_params(request.model);
    let seek = request.first_frame as f64 / 30.0;
    // FFmpeg 30-fps frame indices refer to this exact prepared proxy. Output
    // conversion is bilinear as a conservative OpenCV approximation; final
    // pixel-level Python/Rust parity is deferred to complete migration tests.
    if !seek.is_finite() || seek > 48.0 * 3600.0 {
        return Err(storage::bad_request("Некорректный номер кадра инференса"));
    }
    let mut ff = Command::new("ffmpeg");
    ff.arg("-hide_banner")
        .args(["-loglevel", "error", "-nostdin", "-threads", "4"])
        .args(["-ss", &format!("{seek:.6}")])
        .arg("-i")
        .arg(&prepared)
        .args([
            "-map",
            "0:v:0",
            "-an",
            "-sn",
            "-dn",
            "-vf",
            &format!("scale={w}:{h}:flags=bilinear,format={format}"),
            "-frames:v",
            "9",
            "-pix_fmt",
            format,
            "-f",
            "rawvideo",
            "pipe:1",
        ]);
    ff.stdin(Stdio::null())
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true);
    let mut process = ff
        .spawn()
        .map_err(|e| storage::bad_request(format!("Native FFmpeg decode unavailable: {e}")))?;
    // Output of 9 bounded frames only; no arbitrary file/URL selection.
    let (w, h, ch, _) = pixel_params(request.model);
    let max = w * h * ch * CLIP;
    let mut stdout = process
        .stdout
        .take()
        .ok_or_else(|| storage::internal("Missing FFmpeg video stream"))?;
    let mut stderr = process
        .stderr
        .take()
        .ok_or_else(|| storage::internal("Missing FFmpeg error stream"))?;
    let result = tokio::time::timeout(Duration::from_secs(45), async {
        let mut frames = Vec::with_capacity(max);
        let mut chunk = vec![0u8; 1024 * 1024];
        loop {
            let count = stdout
                .read(&mut chunk)
                .await
                .map_err(|e| format!("FFmpeg read video: {e}"))?;
            if count == 0 {
                break;
            }
            if frames.len() + count > max {
                return Err("FFmpeg emitted more than 9 expected model frames".to_owned());
            }
            frames.extend_from_slice(&chunk[..count]);
        }
        let mut errors = vec![0u8; 8192];
        let reported = stderr.read(&mut errors).await.unwrap_or(0);
        let status = process
            .wait()
            .await
            .map_err(|e| format!("FFmpeg wait: {e}"))?;
        if !status.success() {
            return Err(format!(
                "FFmpeg decode failed: {}",
                String::from_utf8_lossy(&errors[..reported])
                    .chars()
                    .take(250)
                    .collect::<String>()
            ));
        }
        normalize_pixels(&frames, request.model)
    })
    .await;
    match result {
        Ok(Ok(values)) => Ok(values),
        Ok(Err(error)) => Err(storage::bad_request(error)),
        Err(_) => Err(storage::bad_request("FFmpeg decode exceeded 45s timeout")),
    }
}
pub(crate) fn ball_peaks(output: &Output, start: u64) -> Result<Value, String> {
    if output.name != "heatmaps" || output.dimensions != [1, 18, 288, 512] {
        return Err("Unexpected VballNetV4c tensor layout".into());
    }
    let each = 288 * 512;
    if output.data.len() != 18 * each {
        return Err("Incomplete VballNetV4c result".into());
    }
    let mut peaks = Vec::new();
    for i in 0..CLIP {
        let map = &output.data[i * each..(i + 1) * each];
        let (index, &score) = map
            .iter()
            .enumerate()
            .filter(|(_, v)| v.is_finite())
            .max_by(|a, b| a.1.total_cmp(b.1))
            .ok_or("No finite ball confidence")?;
        let radius = output.data[(i + 9) * each + index];
        peaks.push(json!({
            "frame":start+i as u64,
            "score":score,
            "x":index%512,
            "y":index/512,
            "radius_fraction":if radius.is_finite(){Some(radius)}else{None},
            "status":"raw_model_candidate_not_confirmed_ball",
        }));
    }
    Ok(json!(peaks))
}
pub(crate) fn touch_peaks(output: &Output, start: u64) -> Result<Value, String> {
    if output.name != "action_logits" {
        return Ok(Value::Null);
    }
    let dimensions = &output.dimensions;
    if dimensions.len() != 4
        || dimensions[0] != 1
        || dimensions[1] != 9
        || dimensions[3] != 2
        || !(1..=1024).contains(&dimensions[2])
    {
        return Err(format!(
            "Unexpected RAVEL Touch logits dimensions: {dimensions:?}"
        ));
    }
    let queries = usize::try_from(dimensions[2]).map_err(|_| "Invalid RAVEL Touch query count")?;
    if output.data.len() != 9 * queries * 2 {
        return Err("RAVEL Touch logits incomplete".into());
    }
    let mut per_frame = Vec::new();
    for frame in 0..CLIP {
        let mut best = 0.0_f32;
        let mut query = 0_usize;
        for i in 0..queries {
            let index = (frame * queries + i) * 2;
            let touch = output.data[index];
            let none = output.data[index + 1];
            if !touch.is_finite() || !none.is_finite() {
                continue;
            }
            let delta = (none - touch).clamp(-80.0, 80.0);
            let p = 1.0 / (1.0 + delta.exp());
            if p > best {
                best = p;
                query = i;
            }
        }
        per_frame.push(json!({
            "frame":start+frame as u64,
            "max_touch_probability":best,
            "query_index":query,
            "status":"raw_ravel_touch_proposal_not_confirmed_contact",
        }));
    }
    Ok(json!(per_frame))
}
pub async fn clip_preview(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<ClipRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    storage::project_id(&request.project_id)?;
    storage::video_name(&request.video)?;
    let cfg = Config {
        model: request.model,
        device: request.device,
        gpu_device_id: request.gpu_device_id,
        gpu_memory_limit_mb: request.gpu_memory_limit_mb,
        cpu_threads: request.cpu_threads,
    };
    cfg.verify().map_err(storage::bad_request)?;
    let project = projects::existing_project(&state, &request.project_id)?;
    if !project["videos"]
        .as_array()
        .is_some_and(|rows| rows.iter().any(|v| v.as_str() == Some(&request.video)))
    {
        return Err(storage::bad_request(
            "Видео отсутствует в выбранном проекте",
        ));
    }
    let plan = preparation::plan_for(&state, &request.project_id, &request.video)?;
    let file = plan
        .ready_path()
        .ok_or_else(|| {
            storage::bad_request("Сначала подготовьте видео в Rust, не используйте чужой CPU-кэш")
        })?
        .to_path_buf();
    let last = request
        .first_frame
        .checked_add(CLIP as u64)
        .filter(|frame| *frame <= 48 * 3600 * 30)
        .ok_or_else(|| storage::bad_request("Номер кадра превышает размер анализа"))?;
    if plan.duration_sec < last as f64 / 30.0 {
        return Err(storage::bad_request(
            "Для 9-кадрового окна недостаточно видео",
        ));
    }
    let _permit = state
        .inference_limit
        .acquire()
        .await
        .map_err(|e| storage::internal(format!("Native inference concurrency: {e}")))?;
    let inputs = decode_nine(file, &request).await?;
    let model = request.model;
    let root = state.repository_root.clone();
    let result=tokio::task::spawn_blocking(move ||{
        let mut engine=Engine::load(&root,&cfg)?;
        let tensors=engine.infer_clip(inputs)?;
        let mut layouts=Vec::new();
        let mut proposals=Value::Null;
        for tensor in tensors{
            if model==ModelKind::Ball&&tensor.name=="heatmaps"{
                proposals=ball_peaks(&tensor,request.first_frame)?;
            }else if model==ModelKind::Players&&tensor.name=="action_logits"{
                proposals=touch_peaks(&tensor,request.first_frame)?;
            }
            layouts.push(json!({
                "name":tensor.name,
                "dimensions":tensor.dimensions,
                "values":tensor.data.len(),
            }));
        }
        Ok::<_,String>(json!({
            "schema":"volley-native-inference-clip/1",
            "project_id":request.project_id,"video":request.video,
            "model":model,"first_frame":request.first_frame,
            "inferred_frames":9,
            "raw_outputs":layouts,
            "proposals":proposals,
            "identity_confirmed":false,
            "full_gpu_run_complete":false,
            "cpu_processing_complete":false,
            "notes":"One real ONNX window, not the full inference stream. FFmpeg bilinear resize may differ from Python OpenCV; final frame parity testing is pending.",
        }))
    }).await.map_err(|e|storage::internal(format!("Native model task failed: {e}")))?
        .map_err(storage::bad_request)?;
    Ok(Json(result))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rgb_frame_pixels_use_chw_layout() {
        let frame_bytes = vec![3_u8; 9 * 360 * 640 * 3];
        let floats = normalize_pixels(&frame_bytes, ModelKind::Players).unwrap();
        assert_eq!(floats.len(), 9 * 360 * 640 * 3);
        assert!((floats[0] - 3.0 / 255.0).abs() < 1e-6);
    }
}
