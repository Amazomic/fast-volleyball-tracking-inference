//! RTMPose-M Body7 top-down 17-point ONNX GPU inference.
//! Pose outputs are observations, never inferred athlete identities or touches.
use crate::Device;
use ort::{
    execution_providers::{CPUExecutionProvider, CUDAExecutionProvider, ExecutionProvider},
    session::Session,
    value::Tensor,
};
use std::path::{Path, PathBuf};

pub const MODEL_FILE: &str = "RTMPose-M-Body7.onnx";
pub const MODEL_SHA256: &str = "5c0a4bf67953e6d2ac43ce15e77dc9d5d354ae18430a47d2c5963a7bc5683e3c";
pub const KPT_COUNT: usize = 17;
const INPUT_WIDTH: usize = 192;
const INPUT_HEIGHT: usize = 256;
const IMAGE_WIDTH: usize = 640;
const IMAGE_HEIGHT: usize = 360;
const MEAN: [f32; 3] = [123.675, 116.28, 103.53];
const STD: [f32; 3] = [58.395, 57.12, 57.375];

#[derive(Clone, Copy, Debug)]
pub struct Keypoint {
    pub x: f32,
    pub y: f32,
    pub score: f32,
}

/// Source is a 640x360 RGB24 frame from the prepared video.
pub struct PoseEngine {
    session: Session,
}

impl PoseEngine {
    pub fn load(
        repo: &Path,
        device: Device,
        threads: usize,
        memory_mb: u64,
    ) -> Result<Self, String> {
        if !(1..=128).contains(&threads) || memory_mb > 131072 {
            return Err("Invalid pose engine thread or memory setting".into());
        }
        let models = repo
            .join("models")
            .canonicalize()
            .map_err(|e| format!("Missing pose models directory: {e}"))?;
        let path: PathBuf = models
            .join(MODEL_FILE)
            .canonicalize()
            .map_err(|e| format!("RTMPose-M ONNX not installed in models/: {e}"))?;
        if !path.starts_with(&models) || !path.is_file() {
            return Err("RTMPose model path is outside approved model directory".into());
        }
        let len = path
            .metadata()
            .map_err(|e| format!("RTMPose model: {e}"))?
            .len();
        if !(1024..=256 * 1024 * 1024).contains(&len) {
            return Err("RTMPose model size outside validated range".into());
        }
        #[cfg(target_os = "linux")]
        let _ = crate::runtime_library(device)?;
        ort::init()
            .commit()
            .map_err(|e| format!("ONNX runtime: {e}"))?;
        let mut builder = Session::builder()
            .map_err(|e| format!("RTMPose session builder: {e}"))?
            .with_intra_threads(threads)
            .map_err(|e| format!("RTMPose threads: {e}"))?;
        if device == Device::Cuda {
            let mut cuda = CUDAExecutionProvider::default().with_device_id(0);
            if memory_mb > 0 {
                cuda = cuda.with_memory_limit((memory_mb * 1024 * 1024) as usize);
            }
            if !cuda
                .is_available()
                .map_err(|e| format!("RTMPose CUDA: {e}"))?
            {
                return Err("RTMPose CUDA provider unavailable; no implicit CPU fallback".into());
            }
            builder = builder
                .with_execution_providers([cuda.build().error_on_failure()])
                .map_err(|e| format!("RTMPose CUDA provider: {e}"))?;
        } else {
            builder = builder
                .with_execution_providers([CPUExecutionProvider::default().build()])
                .map_err(|e| format!("RTMPose CPU provider: {e}"))?;
        }
        let session = builder
            .commit_from_file(&path)
            .map_err(|e| format!("Load RTMPose ONNX: {e}"))?;
        let input = session.inputs.iter().any(|i| i.name == "input");
        let sx = session.outputs.iter().any(|i| i.name == "simcc_x");
        let sy = session.outputs.iter().any(|i| i.name == "simcc_y");
        if !input || !sx || !sy {
            return Err("Invalid RTMPose-M Body7 ONNX tensor names".into());
        }
        Ok(Self { session })
    }

    pub fn infer(&mut self, rgb: &[u8], bbox: [f64; 4]) -> Result<[Keypoint; KPT_COUNT], String> {
        let crop = crop_and_normalize(rgb, bbox)?;
        let tensor = Tensor::from_array((vec![1_i64, 3, 256, 192], crop))
            .map_err(|e| format!("RTMPose input tensor: {e}"))?;
        let result = self
            .session
            .run(ort::inputs!["input" => tensor])
            .map_err(|e| format!("RTMPose inference: {e}"))?;
        let (dx, sx) = result
            .get("simcc_x")
            .ok_or("RTMPose X output missing")?
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("RTMPose X tensor: {e}"))?;
        let (dy, sy) = result
            .get("simcc_y")
            .ok_or("RTMPose Y output missing")?
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("RTMPose Y tensor: {e}"))?;
        if dx.iter().copied().collect::<Vec<_>>() != [1, 17, 384]
            || dy.iter().copied().collect::<Vec<_>>() != [1, 17, 512]
        {
            return Err(format!("Unexpected RTMPose SimCC shapes: {dx:?}, {dy:?}"));
        }
        decode_simcc(sx, sy, bbox)
    }
}

fn crop_geometry(bbox: [f64; 4]) -> Result<(f64, f64, f64, f64), String> {
    let [left, top, right, bottom] = bbox;
    if !bbox
        .iter()
        .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
        || right <= left
        || bottom <= top
    {
        return Err("Invalid RTMPose player bbox".into());
    }
    let box_w = (right - left) * IMAGE_WIDTH as f64;
    let box_h = (bottom - top) * IMAGE_HEIGHT as f64;
    if box_w < 8.0 || box_h < 20.0 {
        return Err("Player bbox too small for reliable RTMPose crop".into());
    }
    let crop_w = box_w.max(box_h * 0.75) * 1.25;
    let crop_h = crop_w / 0.75;
    let cx = (left + right) * IMAGE_WIDTH as f64 * 0.5;
    let cy = (top + bottom) * IMAGE_HEIGHT as f64 * 0.5;
    Ok((cx, cy, crop_w, crop_h))
}

fn pixel(rgb: &[u8], x: isize, y: isize, c: usize) -> f32 {
    if x < 0 || y < 0 || x >= IMAGE_WIDTH as isize || y >= IMAGE_HEIGHT as isize {
        return 0.0;
    }
    rgb[(y as usize * IMAGE_WIDTH + x as usize) * 3 + c] as f32
}

fn crop_and_normalize(rgb: &[u8], bbox: [f64; 4]) -> Result<Vec<f32>, String> {
    if rgb.len() != IMAGE_WIDTH * IMAGE_HEIGHT * 3 {
        return Err("RTMPose RGB input is not a 640x360 frame".into());
    }
    let (cx, cy, crop_w, crop_h) = crop_geometry(bbox)?;
    let mut data = vec![0f32; 3 * INPUT_HEIGHT * INPUT_WIDTH];
    for y in 0..INPUT_HEIGHT {
        let fy = cy + ((y as f64 + 0.5) / INPUT_HEIGHT as f64 - 0.5) * crop_h - 0.5;
        let y0 = fy.floor() as isize;
        let wy = (fy - y0 as f64) as f32;
        for x in 0..INPUT_WIDTH {
            let fx = cx + ((x as f64 + 0.5) / INPUT_WIDTH as f64 - 0.5) * crop_w - 0.5;
            let x0 = fx.floor() as isize;
            let wx = (fx - x0 as f64) as f32;
            for c in 0..3 {
                let a = pixel(rgb, x0, y0, c) * (1.0 - wx) + pixel(rgb, x0 + 1, y0, c) * wx;
                let b = pixel(rgb, x0, y0 + 1, c) * (1.0 - wx) + pixel(rgb, x0 + 1, y0 + 1, c) * wx;
                data[c * INPUT_HEIGHT * INPUT_WIDTH + y * INPUT_WIDTH + x] =
                    ((1.0 - wy) * a + wy * b - MEAN[c]) / STD[c];
            }
        }
    }
    Ok(data)
}

fn argmax_finite(values: &[f32]) -> Result<(usize, f32), String> {
    let mut best = 0usize;
    let mut max = f32::NEG_INFINITY;
    for (i, &v) in values.iter().enumerate() {
        if !v.is_finite() {
            return Err("Non-finite RTMPose SimCC output".into());
        }
        if v > max {
            max = v;
            best = i;
        }
    }
    Ok((best, max))
}

fn decode_simcc(x: &[f32], y: &[f32], bbox: [f64; 4]) -> Result<[Keypoint; KPT_COUNT], String> {
    if x.len() != KPT_COUNT * 384 || y.len() != KPT_COUNT * 512 {
        return Err("Unexpected RTMPose SimCC lengths".into());
    }
    let (cx, cy, crop_w, crop_h) = crop_geometry(bbox)?;
    let mut points = [Keypoint {
        x: 0.0,
        y: 0.0,
        score: 0.0,
    }; KPT_COUNT];
    for (i, point) in points.iter_mut().enumerate() {
        let (ix, sx) = argmax_finite(&x[i * 384..(i + 1) * 384])?;
        let (iy, sy) = argmax_finite(&y[i * 512..(i + 1) * 512])?;
        let px = cx + (ix as f64 / 2.0 / INPUT_WIDTH as f64 - 0.5) * crop_w;
        let py = cy + (iy as f64 / 2.0 / INPUT_HEIGHT as f64 - 0.5) * crop_h;
        let x_norm = (px / IMAGE_WIDTH as f64) as f32;
        let y_norm = (py / IMAGE_HEIGHT as f64) as f32;
        point.score = if !(0.0..=1.0).contains(&x_norm)
            || !(0.0..=1.0).contains(&y_norm) {
            0.0
        } else {
            sx.min(sy).clamp(0.0, 1.0)
        };
        // Preserve valid normalized schema for an out-of-frame limb with zero score.
        point.x = x_norm.clamp(0.0, 1.0);
        point.y = y_norm.clamp(0.0, 1.0);
    }
    Ok(points)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pose_crop_guards_invalid_box_and_rgb_size() {
        let box_ok = [0.2, 0.1, 0.4, 0.8];
        assert!(crop_and_normalize(&[], box_ok).is_err());
        assert!(crop_geometry([0.5, 0.1, 0.2, 0.3]).is_err());
        assert!(crop_geometry([0.1, 0.1, 0.11, 0.12]).is_err());
    }
    #[test]
    fn simcc_decodes_center_of_crop() {
        let mut x = vec![0f32; 17 * 384];
        let mut y = vec![0f32; 17 * 512];
        for i in 0..17 {
            x[i * 384 + 192] = 0.8;
            y[i * 512 + 256] = 0.9;
        }
        let points = decode_simcc(&x, &y, [0.2, 0.1, 0.4, 0.8]).unwrap();
        assert!((points[0].x - 0.3).abs() < 1e-4);
        assert!((points[0].y - 0.45).abs() < 1e-4);
        assert!((points[0].score - 0.8).abs() < 1e-4);
    }
}
