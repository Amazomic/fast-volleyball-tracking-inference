//! Immutable, versioned RTMPose evidence over RAVEL player candidates.
//! Uses the prepared video and selected GPU fusion, never CPU Track IDs.
use crate::{inference_stream, preparation, projects, AppState};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{BufRead, BufReader, BufWriter, Read, Write},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
use uuid::Uuid;
use volley_inference::{
    pose::{PoseEngine, MODEL_FILE, MODEL_SHA256},
    Device,
};
use volley_media::raw_decoder::{decode_full_video, ModelPixels};

const MAX_FUSION: u64 = 64 * 1024 * 1024 * 1024;
const MAX_LINE: u64 = 512 * 1024;
const MAX_POSES_PER_FRAME: usize = 12;
const POSE_FILE: &str = "poses.jsonl";

fn file_hash(path: &Path) -> Result<String, String> {
    let mut input = File::open(path).map_err(|e| format!("Hash open {}: {e}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let n = input
            .read(&mut buffer)
            .map_err(|e| format!("Hash read: {e}"))?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn safe_file(path: &Path, max: u64) -> Result<(), String> {
    let meta =
        fs::symlink_metadata(path).map_err(|e| format!("Pose source {}: {e}", path.display()))?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > max {
        return Err(format!(
            "Unsafe or oversized pose source: {}",
            path.display()
        ));
    }
    Ok(())
}

fn bbox(candidate: &Value) -> Option<[f64; 4]> {
    if candidate["class_name"] != "player" || candidate["score"].as_f64()? < 0.45 {
        return None;
    }
    let values = candidate["bbox_xyxy_norm"].as_array()?;
    let [a, b, c, d] = values.as_slice() else {
        return None;
    };
    let rect = [a.as_f64()?, b.as_f64()?, c.as_f64()?, d.as_f64()?];
    if rect
        .iter()
        .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
        || rect[0] >= rect[2]
        || rect[1] >= rect[3]
    {
        return None;
    }
    if (rect[2] - rect[0]) * 640.0 < 8.0 || (rect[3] - rect[1]) * 360.0 < 20.0 {
        return None;
    }
    Some(rect)
}

fn iou(a: [f64; 4], b: [f64; 4]) -> f64 {
    let w = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let h = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let intersection = w * h;
    let sa = (a[2] - a[0]) * (a[3] - a[1]);
    let sb = (b[2] - b[0]) * (b[3] - b[1]);
    intersection / (sa + sb - intersection).max(1e-10)
}

fn boxes(frame: &Value) -> Vec<[f64; 4]> {
    let mut unique = Vec::new();
    if let Some(candidates) = frame["player_candidates"].as_array() {
        for candidate in candidates {
            if unique.len() >= MAX_POSES_PER_FRAME {
                break;
            }
            if let Some(bb) = bbox(candidate) {
                if unique.iter().all(|&existing| iou(existing, bb) < 0.6) {
                    unique.push(bb);
                }
            }
        }
    }
    unique
}

/// Probe the selected fusion and run ONNX using one persistent CUDA session.
/// In-progress output is never exposed; returns an immutable manifest after sync.
#[expect(clippy::too_many_arguments)]
pub fn process(
    app: &AppState,
    project_id: &str,
    video: &str,
    fusion_id: &str,
    stride: usize,
    memory_mb: u64,
    threads: usize,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(f64) -> Result<(), String>,
) -> Result<Value, String> {
    if !app.writes_enabled {
        return Err("Pose analysis requires writable isolated studio storage".into());
    }
    if !(1..=9).contains(&stride) {
        return Err("RTMPose stride must be between 1 and 9".into());
    }
    if !volley_core::contracts::job_id(fusion_id) {
        return Err("Invalid GPU fusion identifier".into());
    }
    let _ = projects::existing_project(app, project_id)
        .map_err(|e| format!("Pose project missing: {e:?}"))?;
    let root = projects::project_root(app).join(project_id);
    let dir = root.join("native_gpu_fusions").join(fusion_id);
    let manifest_file = dir.join("manifest.json");
    let frames_file = dir.join("frames.jsonl");
    safe_file(&manifest_file, 32 * 1024)?;
    safe_file(&frames_file, MAX_FUSION)?;
    let manifest: Value =
        serde_json::from_slice(&fs::read(&manifest_file).map_err(|e| e.to_string())?)
            .map_err(|e| format!("Pose fusion manifest: {e}"))?;
    if manifest["schema"] != "volley-native-gpu-fusion/1"
        || manifest["state"] != "raw_candidates_fused"
        || manifest["project_id"] != project_id
        || manifest["video"] != video
        || manifest["fusion_id"] != fusion_id
        || manifest["sha256"]
            .as_str()
            .is_none_or(|sha| !volley_core::contracts::sha256(sha))
    {
        return Err("RTMPose fusion provenance mismatch".into());
    }
    if file_hash(&frames_file)? != manifest["sha256"].as_str().unwrap_or_default() {
        return Err("RTMPose source fusion SHA mismatch".into());
    }
    let plan = preparation::plan_for(app, project_id, video).map_err(|e| format!("{e:?}"))?;
    let prepared = plan.ready_path().ok_or("Prepared pose video missing")?;
    let source = inference_stream::source_stamp(prepared)?;
    if manifest["source_identity"] != json!({"bytes":source.0,"mtime_ns":source.1}) {
        return Err("Prepared video changed since GPU fusion".into());
    }
    let model = app.repository_root.join("models").join(MODEL_FILE);
    safe_file(&model, 256 * 1024 * 1024)?;
    let hash = file_hash(&model)?;
    if hash != MODEL_SHA256 {
        return Err(
            "RTMPose-M Body7 model hash differs from verified official ONNX artifact".into(),
        );
    }
    let mut engine = PoseEngine::load(&app.repository_root, Device::Cuda, threads, memory_mb)?;
    let runs = root.join("native_gpu_pose_runs");
    fs::create_dir_all(&runs).map_err(|e| format!("Create pose run root: {e}"))?;
    let run_id = Uuid::new_v4().simple().to_string();
    let output = runs.join(&run_id);
    fs::create_dir(&output).map_err(|e| format!("Create pose run directory: {e}"))?;
    let result = (|| -> Result<Value, String> {
        let file = File::create(output.join("poses.jsonl.partial"))
            .map_err(|e| format!("Create pose evidence: {e}"))?;
        let mut writer = BufWriter::new(file);
        let mut checksum = Sha256::new();
        let mut reader = BufReader::new(File::open(&frames_file).map_err(|e| e.to_string())?);
        let mut current_index = 0u64;
        let mut line = Vec::new();
        let mut samples = 0u64;
        let mut players = 0u64;
        let mut evidence_bytes = 0u64;
        let estimated = manifest["emitted_frames"]
            .as_u64()
            .filter(|v| *v > 0)
            .unwrap_or(1);
        let decode = decode_full_video(
            prepared,
            ModelPixels::PlayersRgb,
            stride,
            cancel,
            |window| {
                // Fusion has one ordered row per prepared frame. Advance only as necessary.
                while current_index <= window.first_frame {
                    line.clear();
                    let n = reader
                        .read_until(b'\n', &mut line)
                        .map_err(|e| e.to_string())?;
                    if n == 0 || n as u64 > MAX_LINE || line.last() != Some(&b'\n') {
                        return Err("Pose fusion stream truncated or malformed".into());
                    }
                    if current_index < window.first_frame {
                        current_index += 1;
                        continue;
                    }
                    let frame: Value = serde_json::from_slice(&line)
                        .map_err(|e| format!("Invalid GPU fusion row: {e}"))?;
                    if frame["frame_index"].as_u64() != Some(current_index) {
                        return Err("Pose fusion frame index mismatch".into());
                    }
                    let mut poses = Vec::new();
                    // Avoid identifying spectators/players while the ball is absent.
                    if !frame["selected_ball"].is_null() {
                        let bytes = 640 * 360 * 3;
                        let rgb = window
                            .data
                            .get(..bytes)
                            .ok_or("Pose decoder RGB frame too short")?;
                        for bb in boxes(&frame) {
                            let points = engine.infer(rgb, bb)?;
                            let keypoints =
                                points.map(|p| [p.x as f64, p.y as f64, p.score as f64]);
                            poses.push(json!({"box":bb,"keypoints":keypoints}));
                        }
                    }
                    players += poses.len() as u64;
                    samples += 1;
                    let mut row = serde_json::to_vec(&json!({
                        "frame":current_index,"players":poses
                    }))
                    .map_err(|e| e.to_string())?;
                    row.push(b'\n');
                    evidence_bytes += row.len() as u64;
                    if row.len() > 64 * 1024 || evidence_bytes > MAX_FUSION {
                        return Err("RTMPose evidence exceeds bounded output".into());
                    }
                    writer
                        .write_all(&row)
                        .map_err(|e| format!("Write pose evidence: {e}"))?;
                    checksum.update(&row);
                    current_index += 1;
                }
                if samples.is_multiple_of(40) {
                    on_progress((window.first_frame as f64 / estimated as f64).clamp(0.0, 1.0))?;
                }
                Ok(())
            },
        )?;
        if cancel.load(Ordering::Acquire) {
            return Err("cancelled".into());
        }
        if inference_stream::source_stamp(prepared)? != source {
            return Err("Prepared video changed during RTMPose analysis".into());
        }
        if file_hash(&frames_file)? != manifest["sha256"].as_str().unwrap_or_default() {
            return Err("GPU fusion changed during RTMPose analysis".into());
        }
        writer.flush().map_err(|e| e.to_string())?;
        writer.get_ref().sync_all().map_err(|e| e.to_string())?;
        drop(writer);
        fs::rename(output.join("poses.jsonl.partial"), output.join(POSE_FILE))
            .map_err(|e| format!("Commit RTMPose evidence: {e}"))?;
        let pose_manifest = json!({
            "schema":"volley-native-gpu-pose/1","state":"pose_evidence_complete",
            "pose_run_id":run_id, "fusion_id":fusion_id,
            "project_id":project_id, "video":video, "fps":30,
            "stride":stride, "decoded_frames":decode.decoded_frames,
            "sampled_frames":samples, "player_poses":players,
            "model":"RTMPose-M-Body7","model_sha256":hash,
            "source_fusion_sha256":manifest["sha256"], "source_identity":manifest["source_identity"],
            "file":POSE_FILE, "sha256":format!("{:x}",checksum.finalize())
        });
        crate::storage::write_json_atomic(&output.join("manifest.json"), &pose_manifest)
            .map_err(|e| format!("Write pose manifest: {e:?}"))?;
        on_progress(1.0)?;
        Ok(pose_manifest)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&output);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_tiny_poses_and_deduplicates() {
        let f = json!({"player_candidates":[
            {"class_name":"player","score":0.9,"bbox_xyxy_norm":[0.1,0.1,0.3,0.8]},
            {"class_name":"player","score":0.8,"bbox_xyxy_norm":[0.1,0.1,0.3,0.8]},
            {"class_name":"player","score":0.4,"bbox_xyxy_norm":[0.6,0.1,0.8,0.8]}]});
        assert_eq!(boxes(&f).len(), 1);
        assert!(bbox(
            &json!({"class_name":"player","score":0.9,"bbox_xyxy_norm":[0.1,0.1,0.101,0.102]})
        )
        .is_none());
    }
}
