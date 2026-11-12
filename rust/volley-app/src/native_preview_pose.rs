//! Read-only indexed pose overlays bound to the selected GPU fusion.
//! Pose samples are not player identities; match bounding boxes against CPU tracks.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::SystemTime,
};
use volley_inference::pose::MODEL_SHA256;

const MAX_LINE: usize = 64 * 1024;
const MAX_SIZE: u64 = 64 * 1024 * 1024 * 1024;
const MAX_RUNS: usize = 1000;
const MAX_CACHE: usize = 5;

struct PoseIndex {
    path: PathBuf,
    sha: String,
    size: u64,
    modified: Option<SystemTime>,
    samples: Arc<Vec<(u64, u64)>>,
}
static INDICES: OnceLock<Mutex<Vec<PoseIndex>>> = OnceLock::new();

fn regular(path: &Path, max_size: u64) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_file() && !m.file_type().is_symlink() && m.len() <= max_size => Ok(true),
        Ok(_) => Err(format!(
            "Invalid or oversized pose artifact: {}",
            path.display()
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("Inspect pose artifact: {e}")),
    }
}
fn read_manifest(path: &Path) -> Result<Option<Value>, String> {
    if !regular(path, 32 * 1024)? {
        return Ok(None);
    }
    let value = serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| format!("Invalid pose manifest: {e}"))?;
    Ok(Some(value))
}

fn pose_run(project_root: &Path, fusion: &str) -> Result<Option<(PathBuf, String, usize)>, String> {
    let runs = project_root.join("native_gpu_pose_runs");
    if !runs.exists() {
        return Ok(None);
    }
    let m = fs::symlink_metadata(&runs).map_err(|e| e.to_string())?;
    if !m.is_dir() || m.file_type().is_symlink() {
        return Err("Unsafe pose run root".into());
    }
    let fusion_file = project_root
        .join("native_gpu_fusions")
        .join(fusion)
        .join("manifest.json");
    let Some(source) = read_manifest(&fusion_file)? else {
        return Ok(None);
    };
    if source["schema"] != "volley-native-gpu-fusion/1" || source["fusion_id"] != fusion {
        return Err("Invalid pose source fusion".into());
    }
    let source_sha = source["sha256"]
        .as_str()
        .ok_or("Missing pose fusion digest")?;
    let mut found = None;
    let mut entries = 0usize;
    for entry in fs::read_dir(runs).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        entries += 1;
        if entries > MAX_RUNS {
            return Err("Too many GPU pose runs".into());
        }
        let id = entry.file_name().to_string_lossy().into_owned();
        if !volley_core::contracts::job_id(&id) {
            continue;
        }
        let folder = entry.path();
        let m = fs::symlink_metadata(&folder).map_err(|e| e.to_string())?;
        if !m.is_dir() || m.file_type().is_symlink() {
            continue;
        }
        let Some(manifest) = read_manifest(&folder.join("manifest.json"))? else {
            continue;
        };
        if manifest["schema"] != "volley-native-gpu-pose/1"
            || manifest["state"] != "pose_evidence_complete"
            || manifest["pose_run_id"] != id
            || manifest["model_sha256"] != MODEL_SHA256
            || manifest["fusion_id"] != fusion
            || manifest["source_fusion_sha256"] != source_sha
            || manifest["project_id"] != source["project_id"]
            || manifest["video"] != source["video"]
            || manifest["source_identity"] != source["source_identity"]
            || manifest["file"] != "poses.jsonl"
        {
            continue;
        }
        let Some(sha) = manifest["sha256"]
            .as_str()
            .filter(|s| volley_core::contracts::sha256(s))
        else {
            continue;
        };
        let Some(stride) = manifest["stride"].as_u64().filter(|s| (1..=9).contains(s)) else {
            continue;
        };
        let path = folder.join("poses.jsonl");
        if regular(&path, MAX_SIZE)? {
            // One selected GPU fusion should own exactly one compatible pose snapshot.
            if found.is_some() {
                return Err(
                    "Multiple pose runs for selected GPU fusion; refusing ambiguous overlay".into(),
                );
            }
            found = Some((path, sha.to_owned(), stride as usize));
        }
    }
    Ok(found)
}

fn bounded_line(reader: &mut BufReader<File>) -> Result<Option<Vec<u8>>, String> {
    use std::io::Read;
    let mut row = Vec::new();
    let n = reader
        .take((MAX_LINE + 1) as u64)
        .read_until(b'\n', &mut row)
        .map_err(|e| format!("Read pose row: {e}"))?;
    if n == 0 {
        return Ok(None);
    }
    if n > MAX_LINE || row.last() != Some(&b'\n') {
        return Err("RTMPose row too long or truncated".into());
    }
    Ok(Some(row))
}

fn index_for(path: &Path, sha: &str, stride: usize) -> Result<Arc<Vec<(u64, u64)>>, String> {
    let m = fs::metadata(path).map_err(|e| e.to_string())?;
    let modified = m.modified().ok();
    let indices = INDICES.get_or_init(|| Mutex::new(Vec::new()));
    let mut cache = indices.lock().map_err(|_| "RTMPose index poisoned")?;
    if let Some(item) = cache
        .iter()
        .find(|v| v.path == path && v.sha == sha && v.size == m.len() && v.modified == modified)
    {
        return Ok(item.samples.clone());
    }
    let mut reader = BufReader::new(File::open(path).map_err(|e| e.to_string())?);
    let mut offsets = Vec::new();
    let mut digest = Sha256::new();
    let mut at = 0_u64;
    let mut last = None;
    while let Some(line) = bounded_line(&mut reader)? {
        let row: Value =
            serde_json::from_slice(&line).map_err(|e| format!("Invalid RTMPose row: {e}"))?;
        let frame = row["frame"].as_u64().ok_or("Pose row has no frame")?;
        if last.is_some_and(|prev| frame <= prev)
            || frame % stride as u64 != 0
            || !row["players"].is_array()
        {
            return Err("Unordered or invalid RTMPose evidence".into());
        }
        offsets.push((frame, at));
        last = Some(frame);
        at += line.len() as u64;
        digest.update(&line);
    }
    if at != m.len() || format!("{:x}", digest.finalize()) != sha {
        return Err("RTMPose evidence SHA-256 mismatch".into());
    }
    let offsets = Arc::new(offsets);
    cache.retain(|item| item.path != path);
    cache.push(PoseIndex {
        path: path.to_owned(),
        sha: sha.to_owned(),
        size: m.len(),
        modified,
        samples: offsets.clone(),
    });
    if cache.len() > MAX_CACHE {
        cache.remove(0);
    }
    Ok(offsets)
}

fn valid_box(value: &Value) -> Option<[f64; 4]> {
    let a = value.as_array()?;
    let [x1, y1, x2, y2] = a.as_slice() else {
        return None;
    };
    let b = [x1.as_f64()?, y1.as_f64()?, x2.as_f64()?, y2.as_f64()?];
    if b.iter().any(|n| !n.is_finite() || !(0.0..=1.0).contains(n)) || b[0] >= b[2] || b[1] >= b[3]
    {
        return None;
    }
    Some(b)
}
fn iou(a: [f64; 4], b: [f64; 4]) -> f64 {
    let w = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let h = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let inter = w * h;
    inter / ((a[2] - a[0]) * (a[3] - a[1]) + (b[2] - b[0]) * (b[3] - b[1]) - inter).max(1e-10)
}
fn keypoints(value: &Value) -> Option<&Vec<Value>> {
    let a = value.as_array()?;
    if a.len() != 17 {
        return None;
    }
    for row in a {
        let parts = row.as_array()?;
        if parts.len() != 3
            || parts.iter().any(|v| {
                v.as_f64()
                    .is_none_or(|n| !n.is_finite() || n < 0.0 || n > 1.0)
            })
        {
            return None;
        }
    }
    Some(a)
}

fn attach_frame(row: &Value, frame: &mut Value) -> bool {
    let Some(poses) = row["players"].as_array() else {
        return false;
    };
    let Some(tracked) = frame["players"].as_array_mut() else {
        return false;
    };
    let mut used = vec![false; poses.len()];
    let mut matched = false;
    for player in tracked {
        let Some(bb) = valid_box(&player["box"]) else {
            continue;
        };
        let mut best = None;
        let mut max = 0.35_f64;
        for (i, pose) in poses.iter().enumerate() {
            if used[i] {
                continue;
            }
            let Some(pb) = valid_box(&pose["box"]) else {
                continue;
            };
            let confidence = iou(bb, pb);
            if confidence > max {
                max = confidence;
                best = Some(i);
            }
        }
        if let Some(i) = best {
            if let Some(points) = keypoints(&poses[i]["keypoints"]) {
                player["pose"] = json!(points);
                used[i] = true;
                matched = true;
            }
        }
    }
    matched
}

/// Enrich only selected rally frame samples. Returns false if old GPU run has no poses.
pub fn enrich(
    project_root: &Path,
    fusion_id: &str,
    start: u64,
    end: u64,
    frames: &mut [Value],
) -> Result<bool, String> {
    if !volley_core::contracts::job_id(fusion_id) {
        return Err("Invalid source fusion ID for RTMPose preview".into());
    }
    let Some((path, sha, stride)) = pose_run(project_root, fusion_id)? else {
        return Ok(false);
    };
    let index = index_for(&path, &sha, stride)?;
    // Closest observed RTMPose frame within half a sampling interval.
    // This keeps poses visible between sampled frames without generating
    // synthetic coordinates or accidentally attaching an unrelated player.
    let tolerance = (stride as u64).div_ceil(2);
    let lo = index.partition_point(|item| item.0 < start.saturating_sub(tolerance));
    let hi = index.partition_point(|item| item.0 < end.saturating_add(tolerance));
    if lo == hi {
        return Ok(true);
    }
    let mut reader = BufReader::new(File::open(&path).map_err(|e| e.to_string())?);
    let mut closest = vec![u64::MAX; frames.len()];
    for &(sample_frame, offset) in &index[lo..hi] {
        if !frames.iter().enumerate().any(|(i, f)| {
            f["frame"].as_u64().is_some_and(|frame| {
                frame.abs_diff(sample_frame) <= tolerance
                    && frame.abs_diff(sample_frame) < closest[i]
                    && f["players"].as_array().is_some_and(|p| !p.is_empty())
            })
        }) {
            continue;
        }
        reader
            .seek(SeekFrom::Start(offset))
            .map_err(|e| e.to_string())?;
        let row = bounded_line(&mut reader)?.ok_or("Pose index points past evidence")?;
        let item: Value = serde_json::from_slice(&row).map_err(|e| e.to_string())?;
        if item["frame"].as_u64() != Some(sample_frame) {
            return Err("RTMPose index frame mismatch".into());
        }
        for (i, target) in frames.iter_mut().enumerate() {
            let Some(frame) = target["frame"].as_u64() else {
                continue;
            };
            let delta = frame.abs_diff(sample_frame);
            if delta <= tolerance && delta < closest[i] && attach_frame(&item, target) {
                closest[i] = delta;
            }
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tracks_match_poses_by_iou_not_id() {
        let row = json!({"players":[{"box":[0.1,0.1,0.3,0.8],
            "keypoints":(0..17).map(|_|json!([0.2,0.4,0.9])).collect::<Vec<_>>()}]});
        let mut frame = json!({"players":[{"id":7,"box":[0.1,0.1,0.3,0.8]}]});
        attach_frame(&row, &mut frame);
        assert_eq!(frame["players"][0]["pose"].as_array().unwrap().len(), 17);
    }
}
