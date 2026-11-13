//! Bounded source-video FFmpeg sampling; pure Rust CPU geometry detection.
//! Detection is advisory: a user confirms/adjusts it in the editor.
use crate::{
    storage::{self, ApiResult},
    AppState,
};
use axum::{extract::State, http::HeaderMap, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{process::Stdio, sync::Arc, time::Duration};
use volley_core::court_detection::{combine_frames, detect_frame};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DetectionRequest {
    project_id: String,
    video: String,
    #[serde(default)]
    start_sec: f64,
}

pub(crate) async fn detect(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<DetectionRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    if !req.start_sec.is_finite() || !(0.0..=172800.0).contains(&req.start_sec) {
        return Err(storage::bad_request("Invalid calibration sample time"));
    }
    // Existing source-ownership validation is shared with video edits.
    let source = crate::video_edits::calibration_source(&state, &req.project_id, &req.video)?;
    const W: usize = 320;
    const H: usize = 180;
    const N: usize = 5;
    let mut command = tokio::process::Command::new("ffmpeg");
    command
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-threads",
            "2",
        ])
        .args(["-ss", &format!("{:.3}", req.start_sec)])
        .arg("-i")
        .arg(source)
        .args([
            "-t",
            "10",
            "-vf",
            "fps=0.5,scale=320:180:flags=bilinear,format=rgb24",
        ])
        .args([
            "-frames:v",
            "5",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "pipe:1",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(35), command.output())
        .await
        .map_err(|_| storage::bad_request("Court frame extraction timed out"))?
        .map_err(|e| storage::bad_request(format!("FFmpeg unavailable: {e}")))?;
    if !output.status.success() {
        return Err(storage::bad_request("Unable to decode calibration frames"));
    }
    let frame_bytes = W * H * 3;
    if output.stdout.len() % frame_bytes != 0 || output.stdout.len() > N * frame_bytes {
        return Err(storage::bad_request(
            "Invalid decoded court frame dimensions",
        ));
    }
    let total = output.stdout.len() / frame_bytes;
    let detected: Vec<_> = output
        .stdout
        .chunks_exact(frame_bytes)
        .filter_map(|pixels| detect_frame(pixels, W, H))
        .collect();
    let result = if detected.len() >= 3 && detected.len() * 2 >= total {
        combine_frames(&detected)
    } else {
        None
    };
    Ok(Json(match result {
        Some(c) => json!({"detected":true,"court":c.court,"net":c.net,
            "confidence":c.confidence,"sampled_frames":total,
            "supporting_frames":detected.len()}),
        None => json!({"detected":false,"court":[],"net":null,
            "sampled_frames":total,"supporting_frames":detected.len(),
            "message":"No reliable court consensus; mark the four corners manually"}),
    }))
}
