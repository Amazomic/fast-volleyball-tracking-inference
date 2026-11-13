//! RAVEL-free person observations: RTMDet-nano proposals confirmed by RTMPose.
//! Tracking uses a box reconstructed from observed COCO17 joints, not the
//! detector's preliminary bounding rectangle.
use crate::{
    pose::{Keypoint, PoseEngine, KPT_COUNT},
    Device,
};
use ort::{
    execution_providers::{CPUExecutionProvider, CUDAExecutionProvider, ExecutionProvider},
    session::Session,
    value::Tensor,
};
use sha2::{Digest, Sha256};
use std::path::Path;

pub const DETECTOR_FILE: &str = "RTMDet-Nano-Person.onnx";
pub const DETECTOR_SHA256: &str =
    "b8585adb6d1352796f6e86337e09c3459b830645a7e7192d9381670bea2facbd";
const W: usize = 640;
const H: usize = 360;
const SIDE: usize = 320;
const MEAN_BGR: [f32; 3] = [103.53, 116.28, 123.675];
const STD_BGR: [f32; 3] = [57.375, 57.12, 58.395];
const MAX_PERSONS: usize = 12;

/// The global view is downsampled 2x, but far-court tiles preserve the
/// original 640x360 pixels for small athletes behind the net.
#[derive(Clone, Copy, Debug)]
struct DetectorView {
    left: usize,
    top: usize,
    width: usize,
    height: usize,
}
const VIEWS: [DetectorView; 4] = [
    DetectorView {
        left: 0,
        top: 0,
        width: W,
        height: H,
    },
    DetectorView {
        left: 0,
        top: 55,
        width: W / 2,
        height: 245,
    },
    DetectorView {
        left: W / 2,
        top: 55,
        width: W / 2,
        height: 245,
    },
    // The central view crosses the former left/right seam. It preserves
    // small, occluded far-court players without cutting the torso in half.
    DetectorView {
        left: W / 4,
        top: 40,
        width: W / 2,
        height: 245,
    },
];

#[derive(Clone, Debug)]
pub struct PersonObservation {
    pub bbox: [f64; 4],
    pub score: f32,
    pub head: Option<[f64; 2]>,
    pub keypoints: [[f64; 3]; KPT_COUNT],
}

/// Detector rectangles are coarse crop proposals only. At least eight
/// confident joint positions, including torso evidence, are mandatory.
pub fn skeleton_box(points: &[Keypoint; KPT_COUNT]) -> Option<([f64; 4], f32)> {
    skeleton_box_with_threshold(points, 0.36, 8)
}

/// A small athlete beyond the net may have joints at 0.3–0.35 even when
/// the detector finds a coherent person. Keep the strict path for normal
/// players, and relax only the *small crop* path with more joints required.
fn skeleton_box_with_threshold(
    points: &[Keypoint; KPT_COUNT],
    min_score: f32,
    min_joints: usize,
) -> Option<([f64; 4], f32)> {
    let valid = |p: &Keypoint| {
        p.score.is_finite()
            && p.score >= min_score
            && p.x.is_finite()
            && p.y.is_finite()
            && (0.0..=1.0).contains(&p.x)
            && (0.0..=1.0).contains(&p.y)
    };
    if !(valid(&points[5]) && valid(&points[6]) && (valid(&points[11]) || valid(&points[12]))) {
        return None;
    }
    let selected: Vec<_> = points.iter().filter(|p| valid(p)).collect();
    if selected.len() < min_joints {
        return None;
    }
    let mut bounds = [1.0_f64, 1.0_f64, 0.0_f64, 0.0_f64];
    let mut confidence = 0.0f32;
    for point in selected.iter().copied() {
        bounds[0] = bounds[0].min(point.x as f64);
        bounds[1] = bounds[1].min(point.y as f64);
        bounds[2] = bounds[2].max(point.x as f64);
        bounds[3] = bounds[3].max(point.y as f64);
        confidence += point.score;
    }
    let bw = bounds[2] - bounds[0];
    let bh = bounds[3] - bounds[1];
    if bw < 0.01 || bh < 0.035 || bw > 0.70 || bh > 0.96 {
        return None;
    }
    // Use the skeleton for all four sides. Extents include a small
    // perspective-independent margin for the torso and moving limbs.
    bounds[0] = (bounds[0] - bw * 0.13 - 0.003).max(0.0);
    bounds[1] = (bounds[1] - bh * 0.10).max(0.0);
    bounds[2] = (bounds[2] + bw * 0.13 + 0.003).min(1.0);
    bounds[3] = (bounds[3] + bh * 0.10).min(1.0);
    Some((bounds, confidence / selected.len() as f32))
}

fn overlap(a: [f64; 4], b: [f64; 4]) -> f64 {
    let w = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let h = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let area = |b: [f64; 4]| (b[2] - b[0]) * (b[3] - b[1]);
    (w * h) / (area(a) + area(b) - w * h).max(1e-10)
}

/// Overlapping viewports can predict the same athlete with a truncated
/// lower body. Suppress nested skeletal boxes even if their IoU is low.
fn same_person_box(a: [f64; 4], b: [f64; 4]) -> bool {
    if overlap(a, b) > 0.58 {
        return true;
    }
    let iw = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let ih = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let area = |r: [f64; 4]| (r[2] - r[0]) * (r[3] - r[1]);
    let contained = iw * ih / area(a).min(area(b)).max(1e-9);
    let horizontal = ((a[0] + a[2]) - (b[0] + b[2])).abs() * 0.5;
    contained > 0.89 && horizontal < 0.022 && (a[1] - b[1]).abs() < 0.05
}

pub struct PersonEngine {
    detector: Session,
    pose: PoseEngine,
}
impl PersonEngine {
    pub fn load(
        repo: &Path,
        device: Device,
        threads: usize,
        memory_mb: u64,
    ) -> Result<Self, String> {
        if !(1..=128).contains(&threads) || memory_mb > 131072 {
            return Err("Invalid person inference resource settings".into());
        }
        let models = repo
            .join("models")
            .canonicalize()
            .map_err(|e| format!("Models root: {e}"))?;
        let path = models
            .join(DETECTOR_FILE)
            .canonicalize()
            .map_err(|e| format!("RTMDet person model not installed: {e}"))?;
        if !path.starts_with(&models)
            || !path.is_file()
            || path.metadata().map_err(|e| e.to_string())?.len() > 32 * 1024 * 1024
        {
            return Err("Invalid RTMDet person model path or size".into());
        }
        let fingerprint = format!(
            "{:x}",
            Sha256::digest(std::fs::read(&path).map_err(|e| format!("RTMDet model read: {e}"))?)
        );
        if fingerprint != DETECTOR_SHA256 {
            return Err("RTMDet ONNX SHA-256 does not match the vetted artifact".into());
        }
        // Both GPU sessions explicitly require CUDA; never silently fall back.
        #[cfg(target_os = "linux")]
        let _ = crate::runtime_library(device)?;
        ort::init()
            .commit()
            .map_err(|e| format!("ONNX Runtime init: {e}"))?;
        let mut builder = Session::builder()
            .map_err(|e| format!("RTMDet session builder: {e}"))?
            .with_intra_threads(threads)
            .map_err(|e| e.to_string())?;
        if device == Device::Cuda {
            let mut cuda = CUDAExecutionProvider::default().with_device_id(0);
            if memory_mb > 0 {
                cuda = cuda.with_memory_limit((memory_mb * 1024 * 1024) as usize);
            }
            if !cuda.is_available().map_err(|e| e.to_string())? {
                return Err("RTMDet CUDA unavailable".into());
            }
            builder = builder
                .with_execution_providers([cuda.build().error_on_failure()])
                .map_err(|e| format!("RTMDet CUDA: {e}"))?;
        } else {
            builder = builder
                .with_execution_providers([CPUExecutionProvider::default().build()])
                .map_err(|e| e.to_string())?;
        }
        let detector = builder
            .commit_from_file(path)
            .map_err(|e| format!("Open RTMDet: {e}"))?;
        if !detector.inputs.iter().any(|i| i.name == "input")
            || !detector.outputs.iter().any(|o| o.name == "dets")
            || !detector.outputs.iter().any(|o| o.name == "labels")
        {
            return Err("Unexpected RTMDet ONNX tensor ABI".into());
        }
        let pose = PoseEngine::load(repo, device, threads, memory_mb)?;
        Ok(Self { detector, pose })
    }

    pub fn detect(&mut self, rgb: &[u8]) -> Result<Vec<PersonObservation>, String> {
        if rgb.len() != W * H * 3 {
            return Err("RTMDet expects one 640x360 RGB24 frame".into());
        }
        let mut proposals = Vec::new();
        for view in VIEWS {
            let ratio = SIDE as f64 / view.width as f64;
            let input = preprocess_view(rgb, view)?;
            let tensor = Tensor::from_array((vec![1_i64, 3, SIDE as i64, SIDE as i64], input))
                .map_err(|e| format!("RTMDet tensor: {e}"))?;
            let outputs = self
                .detector
                .run(ort::inputs!["input" => tensor])
                .map_err(|e| format!("RTMDet inference: {e}"))?;
            let (ds, dets) = outputs
                .get("dets")
                .ok_or("RTMDet dets missing")?
                .try_extract_tensor::<f32>()
                .map_err(|e| e.to_string())?;
            let (ls, labels) = outputs
                .get("labels")
                .ok_or("RTMDet labels missing")?
                .try_extract_tensor::<i64>()
                .map_err(|e| e.to_string())?;
            if ds.len() != 3
                || ds[0] != 1
                || ds[2] != 5
                || ls.len() != 2
                || ls[0] != 1
                || ls[1] != ds[1]
            {
                return Err(format!(
                    "RTMDet output tensor shape mismatch: {ds:?}/{ls:?}"
                ));
            }
            for (row, &label) in dets.chunks_exact(5).zip(labels.iter()) {
                let min_detector_score = if view.width == W { 0.24 } else { 0.12 };
                if label != 0 || !row.iter().all(|v| v.is_finite()) || row[4] < min_detector_score {
                    continue;
                }
                let bb = [
                    (row[0] as f64 / ratio + view.left as f64) / W as f64,
                    (row[1] as f64 / ratio + view.top as f64) / H as f64,
                    (row[2] as f64 / ratio + view.left as f64) / W as f64,
                    (row[3] as f64 / ratio + view.top as f64) / H as f64,
                ]
                .map(|v| v.clamp(0.0, 1.0));
                if (bb[2] - bb[0]) * (W as f64) < 8.0 || (bb[3] - bb[1]) * (H as f64) < 20.0 {
                    continue;
                }
                proposals.push((row[4], bb));
            }
        }
        proposals.sort_by(|a, b| b.0.total_cmp(&a.0));
        // Remove duplicates before expensive pose inference. A preliminary
        // detector box never becomes a final published player observation.
        let mut distinct: Vec<(f32, [f64; 4])> = Vec::new();
        for candidate in proposals {
            if distinct
                .iter()
                .all(|old| overlap(old.1, candidate.1) < 0.58)
            {
                distinct.push(candidate);
            }
            if distinct.len() >= MAX_PERSONS * 3 {
                break;
            }
        }
        let mut results: Vec<PersonObservation> = Vec::new();
        for (det_score, crop) in distinct {
            if results.len() == MAX_PERSONS {
                break;
            }
            // A very small or border-truncated proposal is not a runtime
            // failure; RTMPose needs at least an 8x20 original-pixel crop.
            if (crop[2] - crop[0]) * (W as f64) < 8.0 || (crop[3] - crop[1]) * (H as f64) < 20.0 {
                continue;
            }
            let keypoints = self.pose.infer(rgb, crop)?;
            let small_far_player = crop[3] - crop[1] <= 0.17;
            let Some((bbox, pose_score)) = skeleton_box(&keypoints).or_else(|| {
                small_far_player
                    .then(|| skeleton_box_with_threshold(&keypoints, 0.28, 9))
                    .flatten()
            }) else {
                continue;
            };
            // Very-low detector confidence is valid only with strong evidence
            // from a coherent pose; never admit a weak silhouette alone.
            if det_score < 0.24 && pose_score < 0.49 {
                continue;
            }
            let head = (keypoints[0].score >= 0.32)
                .then_some([keypoints[0].x as f64, keypoints[0].y as f64]);
            let observation = PersonObservation {
                bbox,
                score: (0.35 * det_score + 0.65 * pose_score).clamp(0.0, 1.0),
                head,
                keypoints: keypoints
                    .map(|point| [point.x as f64, point.y as f64, point.score as f64]),
            };
            if let Some(previous) = results
                .iter_mut()
                .find(|other| same_person_box(other.bbox, bbox))
            {
                // Keep the more fully observed skeleton, not the first
                // detector crop that happened to see the person.
                if observation.score > previous.score {
                    *previous = observation;
                }
            } else {
                results.push(observation);
            }
        }
        Ok(results)
    }
}

#[cfg(test)]
fn preprocess(rgb: &[u8]) -> Result<Vec<f32>, String> {
    preprocess_view(rgb, VIEWS[0])
}

fn preprocess_view(rgb: &[u8], view: DetectorView) -> Result<Vec<f32>, String> {
    if rgb.len() != W * H * 3
        || view.width == 0
        || view.left + view.width > W
        || view.top + view.height > H
    {
        return Err("RTMDet expects a valid crop of a 640x360 RGB24 frame".into());
    }
    let mut output = vec![0.0_f32; 3 * SIDE * SIDE];
    let ratio = view.width as f64 / SIDE as f64;
    let active_height = (view.height as f64 / ratio).round() as usize;
    for y in 0..SIDE {
        for x in 0..SIDE {
            for channel in 0..3 {
                let pixel = if y < active_height {
                    let sx = view.left + ((x as f64 + 0.5) * ratio).floor() as usize;
                    let sy = view.top + ((y as f64 + 0.5) * ratio).floor() as usize;
                    rgb[(sy.min(H - 1) * W + sx.min(W - 1)) * 3 + (2 - channel)] as f32
                } else {
                    114.0
                };
                output[channel * SIDE * SIDE + y * SIDE + x] =
                    (pixel - MEAN_BGR[channel]) / STD_BGR[channel];
            }
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample(x: f32, y: f32, score: f32) -> Keypoint {
        Keypoint { x, y, score }
    }

    #[test]
    fn far_court_pose_threshold_recovers_occluded_small_athlete() {
        let mut points = [sample(0.0, 0.0, 0.0); KPT_COUNT];
        for (i, point) in points.iter_mut().enumerate() {
            *point = sample(
                0.43 + (i % 4) as f32 * 0.007,
                0.42 + (i / 4) as f32 * 0.02,
                if (5..=12).contains(&i) { 0.33 } else { 0.43 },
            );
        }
        assert!(skeleton_box(&points).is_none());
        assert!(skeleton_box_with_threshold(&points, 0.28, 9).is_some());
        assert!(same_person_box(
            [0.29, 0.69, 0.37, 1.0],
            [0.294, 0.70, 0.365, 0.86]
        ));
        assert!(!same_person_box(
            [0.29, 0.69, 0.37, 0.86],
            [0.40, 0.69, 0.48, 0.86]
        ));
    }

    #[test]
    fn bbox_is_derived_from_body_points_not_detector_box() {
        let mut pts = [sample(0.0, 0.0, 0.0); KPT_COUNT];
        for (i, p) in pts.iter_mut().enumerate() {
            *p = sample(
                0.40 + (i % 4) as f32 * 0.02,
                0.25 + (i / 4) as f32 * 0.08,
                0.9,
            );
        }
        let (bbox, score) = skeleton_box(&pts).unwrap();
        assert!(bbox[0] < 0.4 && bbox[2] > 0.46);
        assert!(score > 0.8);
        assert!(skeleton_box(&[sample(0.5, 0.5, 0.9); KPT_COUNT]).is_none());
    }

    #[test]
    #[ignore = "Run with PERSON_PREVIEW_RGB and ORT_DYLIB_PATH on host"]
    fn actual_onnx_model_detects_pose_people_without_ravel() {
        let path = std::env::var("PERSON_PREVIEW_RGB").expect("raw RGB path");
        let rgb = std::fs::read(path).expect("prepared RGB frame");
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .unwrap();
        let mut engine = PersonEngine::load(root, Device::Cpu, 4, 0).unwrap();
        let result = engine.detect(&rgb).unwrap();
        println!(
            "RTMDet+RTMPose players: {:?}",
            result.iter().map(|p| (p.bbox, p.score)).collect::<Vec<_>>()
        );
        if std::env::var_os("PERSON_DIAGNOSTICS").is_some() {
            for (view_id, view) in VIEWS.iter().copied().enumerate() {
                let input = preprocess_view(&rgb, view).unwrap();
                let tensor =
                    Tensor::from_array((vec![1, 3, SIDE as i64, SIDE as i64], input)).unwrap();
                let out = engine
                    .detector
                    .run(ort::inputs!["input" => tensor])
                    .unwrap();
                let (_, dets) = out
                    .get("dets")
                    .unwrap()
                    .try_extract_tensor::<f32>()
                    .unwrap();
                let (_, labels) = out
                    .get("labels")
                    .unwrap()
                    .try_extract_tensor::<i64>()
                    .unwrap();
                let ratio = SIDE as f64 / view.width as f64;
                for (row, label) in dets.chunks_exact(5).zip(labels) {
                    if *label != 0 || row[4] < 0.12 {
                        continue;
                    }
                    let bb = [
                        (row[0] as f64 / ratio + view.left as f64) / W as f64,
                        (row[1] as f64 / ratio + view.top as f64) / H as f64,
                        (row[2] as f64 / ratio + view.left as f64) / W as f64,
                        (row[3] as f64 / ratio + view.top as f64) / H as f64,
                    ]
                    .map(|p| p.clamp(0.0, 1.0));
                    let cx = (bb[0] + bb[2]) * 0.5;
                    if !(0.37..=0.52).contains(&cx) {
                        continue;
                    }
                    let pose = engine.pose.infer(&rgb, bb).unwrap();
                    let confirmed = skeleton_box(&pose);
                    let strong = pose
                        .iter()
                        .enumerate()
                        .filter(|(_, p)| p.score > 0.30)
                        .map(|(i, p)| (i, (p.score * 100.) as i32))
                        .collect::<Vec<_>>();
                    println!("VIEW={view_id} proposal={bb:?}, score={:.2}, joints={strong:?}, validated={:?}",
                        row[4], confirmed.map(|(b,s)|(b,s)));
                }
            }
        }
        assert!(
            !result.is_empty(),
            "Expected at least one player in match frame"
        );
    }

    #[test]
    fn refuses_missing_torso_or_incomplete_frame() {
        let mut points = [sample(0.5, 0.5, 0.9); KPT_COUNT];
        points[5].score = 0.1;
        assert!(skeleton_box(&points).is_none());
        assert!(preprocess(&vec![0; 3 * W * H - 1]).is_err());
        assert_eq!(
            preprocess(&vec![0; 3 * W * H]).unwrap().len(),
            3 * SIDE * SIDE
        );
        // The far-court tile preserves original image pixels (not a 2x blur).
        let mut rgb = vec![0; 3 * W * H];
        let px = (55 * W + 0) * 3;
        rgb[px] = 230;
        let tile = preprocess_view(&rgb, VIEWS[1]).unwrap();
        let red_as_bgr = (230.0 - MEAN_BGR[2]) / STD_BGR[2];
        assert!((tile[2 * SIDE * SIDE] - red_as_bgr).abs() < 0.01);
    }
}
