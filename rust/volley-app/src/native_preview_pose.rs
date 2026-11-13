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
    change: Option<(u64, u64, i64, i64)>,
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
        if manifest["fusion_id"] != fusion {
            continue;
        }
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
            return Err("RTMPose source provenance mismatch".into());
        }
        let Some(sha) = manifest["sha256"]
            .as_str()
            .filter(|s| volley_core::contracts::sha256(s))
        else {
            return Err("Invalid RTMPose SHA-256 in source manifest".into());
        };
        let Some(stride) = manifest["stride"].as_u64().filter(|s| (1..=9).contains(s)) else {
            return Err("Invalid RTMPose sample stride".into());
        };
        let path = folder.join("poses.jsonl");
        if !regular(&path, MAX_SIZE)? {
            return Err("RTMPose manifest references missing pose evidence".into());
        }
        // One selected GPU fusion should own exactly one compatible pose snapshot.
        if found.is_some() {
            return Err(
                "Multiple pose runs for selected GPU fusion; refusing ambiguous overlay".into(),
            );
        }
        found = Some((path, sha.to_owned(), stride as usize));
    }
    Ok(found)
}

pub fn source_info(
    project_root: &Path,
    fusion: &str,
) -> Result<Option<(String, String, usize)>, String> {
    let Some((path, sha, stride)) = pose_run(project_root, fusion)? else {
        return Ok(None);
    };
    index_for(&path, &sha, stride)?;
    let id = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|s| s.to_str())
        .ok_or("Invalid pose run path")?;
    Ok(Some((id.to_owned(), sha, stride)))
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
    let change = volley_media::preparation::file_change_stamp(path, &m);
    let indices = INDICES.get_or_init(|| Mutex::new(Vec::new()));
    let mut cache = indices.lock().map_err(|_| "RTMPose index poisoned")?;
    if change.is_some() {
        if let Some(item) = cache.iter().find(|v| {
            v.path == path
                && v.sha == sha
                && v.size == m.len()
                && v.modified == modified
                && v.change == change
        }) {
            return Ok(item.samples.clone());
        }
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
    let after = fs::metadata(path).map_err(|e| e.to_string())?;
    if after.len() != m.len()
        || after.modified().ok() != modified
        || volley_media::preparation::file_change_stamp(path, &after) != change
    {
        return Err("RTMPose source changed during SHA verification".into());
    }
    let offsets = Arc::new(offsets);
    if change.is_some() {
        cache.retain(|item| item.path != path);
        cache.push(PoseIndex {
            path: path.to_owned(),
            sha: sha.to_owned(),
            size: m.len(),
            modified,
            change,
            samples: offsets.clone(),
        });
        if cache.len() > MAX_CACHE {
            cache.remove(0);
        }
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

fn attach_frame(row: &Value, frame: &mut Value, reject_ambiguous: bool) -> bool {
    let Some(poses) = row["players"].as_array() else {
        return false;
    };
    let Some(tracked) = frame["players"].as_array_mut() else {
        return false;
    };
    let mut used = vec![false; poses.len()];
    let mut matched = false;
    let boxes: Vec<_> = tracked.iter().map(|p| valid_box(&p["box"])).collect();
    for (track_index, player) in tracked.iter_mut().enumerate() {
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
            if reject_ambiguous {
                let alternative = poses
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| *j != i)
                    .filter_map(|(_, pose)| valid_box(&pose["box"]))
                    .map(|b| iou(bb, b))
                    .fold(0.0_f64, f64::max);
                let pose_box = valid_box(&poses[i]["box"]).unwrap_or(bb);
                let competing = boxes
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| *j != track_index)
                    .filter_map(|(_, b)| *b)
                    .map(|b| iou(b, pose_box))
                    .fold(0.0_f64, f64::max);
                if alternative >= 0.35 && max - alternative < 0.1
                    || competing >= 0.35 && max - competing < 0.1
                {
                    continue;
                }
            }
            if let Some(points) = keypoints(&poses[i]["keypoints"]) {
                player["pose"] = json!(points);
                used[i] = true;
                matched = true;
            }
        }
    }
    matched
}

/// Build an *overlay-only* intermediate pose. Unlike raw.poses, this
/// data is never saved as a model observation or used by CPU analytics.
/// Pose detections have no Track IDs: match by mutual, unambiguous box overlap,
/// or omit the ambiguous person rather than attaching limbs to a stranger.
fn interpolate_raw_players(before: &[Value], after: &[Value], t: f64) -> Vec<Value> {
    if !(0.0..1.0).contains(&t) || before.len() > 128 || after.len() > 128 {
        return Vec::new();
    }
    let first_boxes: Vec<_> = before.iter().map(|p| valid_box(&p["box"])).collect();
    let last_boxes: Vec<_> = after.iter().map(|p| valid_box(&p["box"])).collect();
    let mut result = Vec::new();
    for (i, left) in before.iter().enumerate() {
        let Some(a) = first_boxes[i] else { continue };
        let Some(a_points) = keypoints(&left["keypoints"]) else {
            continue;
        };
        let scored: Vec<_> = last_boxes
            .iter()
            .map(|bb| bb.map_or(0.0, |b| iou(a, b)))
            .collect();
        let Some((j, score)) = scored
            .iter()
            .copied()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(&b.1))
        else {
            continue;
        };
        if score < 0.35
            || scored
                .iter()
                .enumerate()
                .any(|(k, other)| k != j && *other >= score - 0.10)
        {
            continue;
        }
        // Check the reverse match: a player crossing another must not
        // borrow either one's raw joints on the intervening frames.
        if first_boxes.iter().enumerate().any(|(k, bb)| {
            k != i && bb.is_some_and(|b| iou(b, last_boxes[j].unwrap_or(a)) >= score - 0.10)
        }) {
            continue;
        }
        let Some(b) = last_boxes[j] else { continue };
        let Some(b_points) = keypoints(&after[j]["keypoints"]) else {
            continue;
        };
        let bbox: Vec<_> = a.iter().zip(b).map(|(p, q)| p + (q - p) * t).collect();
        let mut joints = Vec::with_capacity(17);
        for (p, q) in a_points.iter().zip(b_points) {
            let (Some(p), Some(q)) = (p.as_array(), q.as_array()) else {
                continue;
            };
            let (Some(px), Some(py), Some(pc), Some(qx), Some(qy), Some(qc)) = (
                p[0].as_f64(),
                p[1].as_f64(),
                p[2].as_f64(),
                q[0].as_f64(),
                q[1].as_f64(),
                q[2].as_f64(),
            ) else {
                continue;
            };
            // A missing/uncertain endpoint cannot be made confident by interpolation.
            joints.push(json!([px + (qx - px) * t, py + (qy - py) * t, pc.min(qc)]));
        }
        if joints.len() == 17 {
            result.push(json!({"box":bbox,"keypoints":joints}));
        }
    }
    result
}

/// Exact detections stay in raw.poses; interpolation for visual continuity
/// is kept separately in raw.poses_interpolated. CPU inference/analysis does
/// not read the synthetic poses, and each gap needs two saved RTMPose samples.
pub fn raw_window(
    project_root: &Path,
    fusion_id: &str,
    total_frames: u64,
    start: u64,
    end: u64,
    frames: &mut [Value],
) -> Result<Option<String>, String> {
    if !volley_core::contracts::job_id(fusion_id)
        || start >= end
        || end > total_frames
        || frames.len() != (end - start) as usize
    {
        return Err("Invalid raw RTMPose source or frame bounds".into());
    }
    let Some((path, sha, stride)) = pose_run(project_root, fusion_id)? else {
        return Ok(None);
    };
    let index = index_for(&path, &sha, stride)?;
    if index.iter().any(|(frame, _)| *frame >= total_frames) {
        return Err("RTMPose evidence exceeds selected GPU fusion frames".into());
    }
    // Fetch one original pose sample on either side of the request, so
    // each 180-frame API window shows the same smooth motion at its edges.
    let lo = index.partition_point(|(frame, _)| *frame < start.saturating_sub(stride as u64));
    let hi = index.partition_point(|(frame, _)| *frame < end.saturating_add(stride as u64));
    let mut reader = BufReader::new(File::open(&path).map_err(|e| e.to_string())?);
    let mut samples: Vec<(u64, Vec<Value>)> = Vec::new();
    for &(frame, offset) in &index[lo..hi] {
        reader
            .seek(SeekFrom::Start(offset))
            .map_err(|e| e.to_string())?;
        let line = bounded_line(&mut reader)?.ok_or("RTMPose indexed row is missing")?;
        let row: Value =
            serde_json::from_slice(&line).map_err(|_| "Malformed RTMPose raw frame")?;
        if row["frame"].as_u64() != Some(frame) {
            return Err("RTMPose indexed frame mismatch".into());
        }
        let poses = row["players"].as_array().ok_or("Missing RTMPose players")?;
        if poses.len() > 128 {
            return Err("RTMPose player count exceeds bounds".into());
        }
        let mut evidence = Vec::with_capacity(poses.len());
        for pose in poses {
            let bb = valid_box(&pose["box"]).ok_or("Invalid RTMPose box")?;
            let keypoints = keypoints(&pose["keypoints"]).ok_or("Invalid RTMPose keypoints")?;
            evidence.push(json!({"box":bb,"keypoints":keypoints}));
        }
        if frame >= start && frame < end {
            frames[(frame - start) as usize]["raw"]["poses"] = json!(evidence);
        }
        samples.push((frame, evidence));
    }
    for pair in samples.windows(2) {
        let (before_frame, before) = (&pair[0].0, &pair[0].1);
        let (after_frame, after) = (&pair[1].0, &pair[1].1);
        // Never extrapolate across missing model samples or scene cuts.
        if after_frame - before_frame != stride as u64 {
            continue;
        }
        for frame in (before_frame + 1).max(start)..(*after_frame).min(end) {
            let t = (frame - before_frame) as f64 / (after_frame - before_frame) as f64;
            let interpolated = interpolate_raw_players(before, after, t);
            if !interpolated.is_empty() {
                frames[(frame - start) as usize]["raw"]["poses_interpolated"] = json!(interpolated);
            }
        }
    }
    Ok(Some(
        path.parent()
            .and_then(Path::file_name)
            .and_then(|v| v.to_str())
            .ok_or("Invalid RTMPose source ID")?
            .to_owned(),
    ))
}

/// Enrich only selected rally frame samples. Returns false if old GPU run has no poses.
pub fn enrich(
    project_root: &Path,
    fusion_id: &str,
    start: u64,
    end: u64,
    frames: &mut [Value],
) -> Result<bool, String> {
    enrich_with_tolerance(project_root, fusion_id, start, end, frames, false)
}

pub fn enrich_exact(
    project_root: &Path,
    fusion_id: &str,
    start: u64,
    end: u64,
    frames: &mut [Value],
) -> Result<bool, String> {
    enrich_with_tolerance(project_root, fusion_id, start, end, frames, true)
}

fn enrich_with_tolerance(
    project_root: &Path,
    fusion_id: &str,
    start: u64,
    end: u64,
    frames: &mut [Value],
    exact: bool,
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
    let tolerance = if exact {
        0
    } else {
        (stride as u64).div_ceil(2)
    };
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
            if delta <= tolerance && delta < closest[i] && attach_frame(&item, target, exact) {
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
    fn raw_pose_interpolation_is_visual_only_and_leaves_original_samples_unchanged() {
        let root = std::env::temp_dir().join(format!("volley-raw-pose-{}", uuid::Uuid::new_v4()));
        let fusion = "a".repeat(32);
        let run = "b".repeat(32);
        let fusion_dir = root.join("native_gpu_fusions").join(&fusion);
        let pose_dir = root.join("native_gpu_pose_runs").join(&run);
        fs::create_dir_all(&fusion_dir).unwrap();
        fs::create_dir_all(&pose_dir).unwrap();
        let source_sha = "c".repeat(64);
        let identity = json!({"bytes":123,"mtime_ns":456});
        fs::write(
            fusion_dir.join("manifest.json"),
            serde_json::to_vec(&json!({"schema":"volley-native-gpu-fusion/1",
                "fusion_id":fusion,"project_id":"project","video":"match.mp4",
                "source_identity":identity,"sha256":source_sha}))
            .unwrap(),
        )
        .unwrap();
        let mut raw = Vec::new();
        for frame in [0, 3] {
            let keypoints = (0..17)
                .map(|_| json!([0.30 + f64::from(frame) * 0.03, 0.5, 0.91]))
                .collect::<Vec<_>>();
            raw.extend(
                serde_json::to_vec(&json!({"frame":frame,
                "players":[{"box":[0.2,0.2,0.5,0.9],"keypoints":keypoints}]}))
                .unwrap(),
            );
            raw.push(b'\n');
        }
        fs::write(pose_dir.join("poses.jsonl"), &raw).unwrap();
        let sha = format!("{:x}", Sha256::digest(&raw));
        let mut manifest = json!({"schema":"volley-native-gpu-pose/1",
            "state":"pose_evidence_complete","pose_run_id":run,
            "fusion_id":fusion,"project_id":"project","video":"match.mp4",
            "source_identity":identity,"source_fusion_sha256":source_sha,
            "model_sha256":MODEL_SHA256,"file":"poses.jsonl","sha256":sha,
            "stride":3});
        fs::write(
            pose_dir.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let mut frames = (0..5)
            .map(|frame| json!({"frame":frame,"raw":{"poses":[]}}))
            .collect::<Vec<_>>();
        let selected = raw_window(&root, &fusion, 9, 0, 5, &mut frames).unwrap();
        assert_eq!(selected.as_deref(), Some(run.as_str()));
        assert_eq!(frames[0]["raw"]["poses"][0]["keypoints"][0][2], 0.91);
        assert_eq!(frames[3]["raw"]["poses"][0]["keypoints"][0][2], 0.91);
        // Original model evidence stays sparse and is never substituted.
        assert!(frames[1]["raw"]["poses"].as_array().unwrap().is_empty());
        assert_eq!(frames[0]["raw"]["poses"][0]["keypoints"][0][0], 0.30);
        assert_eq!(frames[3]["raw"]["poses"][0]["keypoints"][0][0], 0.39);
        assert!(frames[0]["raw"].get("poses_interpolated").is_none());
        assert!(frames[3]["raw"].get("poses_interpolated").is_none());
        assert!(frames[4]["raw"].get("poses_interpolated").is_none());
        assert_eq!(
            frames[1]["raw"]["poses_interpolated"][0]["keypoints"][0][0],
            0.33
        );
        assert_eq!(
            frames[2]["raw"]["poses_interpolated"][0]["keypoints"][0][0],
            0.36
        );
        assert!(frames[1]["raw"]["poses_interpolated"][0]
            .get("track_id")
            .is_none());
        assert!(frames[0]["raw"]["poses"][0].get("track_id").is_none());
        // The window can start between RTMPose measurements. Its first
        // frames still interpolate from the real sample *outside* the window.
        let mut shifted = (1..4)
            .map(|frame| json!({"frame":frame,"raw":{"poses":[]}}))
            .collect::<Vec<_>>();
        raw_window(&root, &fusion, 9, 1, 4, &mut shifted).unwrap();
        assert_eq!(
            shifted[0]["raw"]["poses_interpolated"][0]["keypoints"][0][0],
            0.33
        );
        assert_eq!(
            shifted[1]["raw"]["poses_interpolated"][0]["keypoints"][0][0],
            0.36
        );
        assert!(shifted[2]["raw"]["poses_interpolated"].is_null());
        assert_eq!(shifted[2]["raw"]["poses"][0]["keypoints"][0][0], 0.39);
        manifest["sha256"] = json!("0".repeat(64));
        fs::write(
            pose_dir.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert!(raw_window(&root, &fusion, 9, 0, 5, &mut frames).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn raw_pose_interpolation_rejects_ambiguous_or_missing_players() {
        let joints = |confidence| {
            (0..17)
                .map(|_| json!([0.3, 0.4, confidence]))
                .collect::<Vec<_>>()
        };
        let person = |bb: [f64; 4], confidence| json!({"box":bb,"keypoints":joints(confidence)});
        let a = person([0.1, 0.1, 0.3, 0.7], 0.9);
        let duplicate = person([0.1, 0.1, 0.3, 0.7], 0.8);
        let distant = person([0.7, 0.1, 0.9, 0.7], 0.9);
        assert!(interpolate_raw_players(&[a.clone()], &[distant], 0.5).is_empty());
        assert!(
            interpolate_raw_players(&[a.clone(), duplicate.clone()], &[a.clone()], 0.5).is_empty()
        );
        assert!(interpolate_raw_players(&[a.clone()], &[a.clone(), duplicate], 0.5).is_empty());
        assert!(interpolate_raw_players(&[a.clone()], &[], 0.5).is_empty());
        let low_confidence = person([0.1, 0.1, 0.3, 0.7], 0.1);
        let preview = interpolate_raw_players(&[a], &[low_confidence], 0.5);
        assert_eq!(preview.len(), 1);
        assert_eq!(preview[0]["keypoints"][0][2], 0.1);
    }

    #[test]
    fn exact_pose_matching_rejects_overlapping_tracks() {
        let row = json!({"players":[{"box":[0.1,0.1,0.3,0.8],
            "keypoints":(0..17).map(|_|json!([0.2,0.4,0.9])).collect::<Vec<_>>()}]});
        let mut frame = json!({"players":[{"id":7,"box":[0.1,0.1,0.3,0.8]},
            {"id":8,"box":[0.1,0.1,0.3,0.8]}]});
        assert!(!attach_frame(&row, &mut frame, true));
        assert!(frame["players"][0].get("pose").is_none());
        assert!(frame["players"][1].get("pose").is_none());
    }
    #[test]
    fn tracks_match_poses_by_iou_not_id() {
        let row = json!({"players":[{"box":[0.1,0.1,0.3,0.8],
            "keypoints":(0..17).map(|_|json!([0.2,0.4,0.9])).collect::<Vec<_>>()}]});
        let mut frame = json!({"players":[{"id":7,"box":[0.1,0.1,0.3,0.8]}]});
        attach_frame(&row, &mut frame, false);
        assert_eq!(frame["players"][0]["pose"].as_array().unwrap().len(), 17);
    }
}
