//! CPU-only replay of verified native RAVEL/VballNet fusion frames.
//! Reconstructs provisional player Track IDs without rerunning ONNX.
//! A tracklet ID is NOT an authenticated real athlete identity.
use crate::{
    inference_stream, preparation, project_guard, projects,
    storage::{self, ApiError, ApiResult},
    AppState,
};
use axum::{
    extract::{Query, State},
    http::HeaderMap,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    sync::Arc,
};
use uuid::Uuid;
use volley_core::{
    fusion::Candidate,
    tracker::{ProvisionalTracker, TrackConfirmation, TrackingOptions},
};

const MAX_MANIFEST_BYTES: u64 = 16 * 1024;
const MAX_INPUT_BYTES: u64 = 64 * 1024 * 1024 * 1024;
const MAX_ROW_BYTES: usize = 512 * 1024;
const OUTPUT_FILE: &str = "tracked_frames.jsonl";

pub(crate) use volley_core::contracts::job_id as opaque_id;
pub(crate) fn safe_file(folder: &Path, name: &str, limit: u64) -> Result<PathBuf, String> {
    let path = folder.join(name);
    let info = fs::symlink_metadata(&path).map_err(|e| format!("Read CPU input {name}: {e}"))?;
    if info.file_type().is_symlink() || !info.is_file() || info.len() > limit {
        return Err(format!("Unsafe CPU input file: {name}"));
    }
    Ok(path)
}
pub(crate) fn safe_folder(path: &Path) -> Result<(), String> {
    let info = fs::symlink_metadata(path).map_err(|e| format!("Read folder: {e}"))?;
    if info.file_type().is_symlink() || !info.is_dir() {
        return Err("CPU input folder must be a real directory".into());
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayRequest {
    pub(crate) project_id: String,
    pub(crate) video: String,
    pub(crate) fusion_id: String,
    #[serde(default)]
    pub(crate) settings: Option<TrackingOptions>,
}
#[derive(Deserialize)]
pub struct ReplayQuery {
    project_id: String,
    replay_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FusedFrame {
    frame_index: u64,
    player_candidates: Vec<Candidate>,
    ball_candidates: Vec<Value>,
    selected_ball: Option<Value>,
    player_track_ids_confirmed: bool,
    touches_confirmed: bool,
}

pub(crate) struct BoundedReader {
    reader: BufReader<File>,
    hash: Sha256,
}
impl BoundedReader {
    pub(crate) fn open(path: &Path) -> Result<Self, String> {
        Ok(Self {
            reader: BufReader::new(
                File::open(path).map_err(|e| format!("Open source frames: {e}"))?,
            ),
            hash: Sha256::new(),
        })
    }
    pub(crate) fn line(&mut self) -> Result<Option<Vec<u8>>, String> {
        let mut line = Vec::with_capacity(2048);
        loop {
            let chunk = self
                .reader
                .fill_buf()
                .map_err(|e| format!("Read fusion frames: {e}"))?;
            if chunk.is_empty() {
                if line.is_empty() {
                    return Ok(None);
                }
                return Err("Fusion JSONL must terminate every row with LF".into());
            }
            let n = chunk
                .iter()
                .position(|c| *c == b'\n')
                .map_or(chunk.len(), |pos| pos + 1);
            if line.len() + n > MAX_ROW_BYTES {
                return Err("Fusion JSONL row exceeds 512 KiB".into());
            }
            let done = chunk[n - 1] == b'\n';
            line.extend_from_slice(&chunk[..n]);
            self.hash.update(&chunk[..n]);
            self.reader.consume(n);
            if done {
                return Ok(Some(line));
            }
        }
    }
    pub(crate) fn finish(self) -> String {
        format!("{:x}", self.hash.finalize())
    }
}

#[derive(Default)]
struct ReplayTotals {
    frames: u64,
    detections: u64,
    rejected: u64,
    ambiguous: u64,
    frames_with_ball: u64,
    tentative_tracks: u64,
    unconfirmed_tracks: u64,
    short_track_detections: u64,
}

/// Replay once with provisional IDs, then filter complete-video track evidence
/// before publishing any derived Track IDs. A single streaming pass cannot
/// retract the first frame of an ID later shown to be a two-frame false positive.
fn replay_frames(
    source: &Path,
    output: &Path,
    count: u64,
    config: TrackingOptions,
    expected_sha: &str,
) -> Result<(String, ReplayTotals, u64), String> {
    if !(1..=48 * 3600 * 30).contains(&count) {
        return Err("Invalid fused CPU frame count".into());
    }
    let config = config.validate()?;
    let tentative = output.with_extension("unconfirmed");
    let result = replay_frames_verified(source, &tentative, output, count, config, expected_sha);
    // This intermediate output is never exposed as a CPU run, including errors.
    let _ = fs::remove_file(tentative);
    result
}

/// The CPU replay contains confirmed *observations*, plus bounded
/// inferred positions marked explicitly as such. The raw GPU proposals
/// remain byte-for-byte intact. Missing IDs may be filled only between two
/// real observations of that same confirmed Track ID, at most 3 seconds apart.
fn fill_confirmed_track_gaps(
    source: &Path,
    expected_sha: &str,
    frame_count: u64,
) -> Result<(String, u64), String> {
    use std::collections::{BTreeMap, VecDeque};

    let tmp = source.with_extension("continuity.partial");
    let mut input = BoundedReader::open(source)?;
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&tmp)
        .map_err(|e| format!("Create bounded tracking interpolation: {e}"))?;
    let mut output = BufWriter::new(file);
    let mut digest = Sha256::new();
    let mut queued: VecDeque<Value> = VecDeque::with_capacity(91);
    let mut anchors: BTreeMap<u64, (u64, Candidate)> = BTreeMap::new();
    let mut inferred = 0_u64;
    let write_one =
        |record: Value, writer: &mut BufWriter<File>, hash: &mut Sha256| -> Result<(), String> {
            let mut bytes = serde_json::to_vec(&record).map_err(|e| e.to_string())?;
            bytes.push(b'\n');
            if bytes.len() > MAX_ROW_BYTES {
                return Err("Interpolated tracking frame exceeds 512 KiB".into());
            }
            writer.write_all(&bytes).map_err(|e| e.to_string())?;
            hash.update(&bytes);
            Ok(())
        };
    let iou = |a: [f64; 4], b: [f64; 4]| {
        let dx = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
        let dy = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
        let s = |b: [f64; 4]| (b[2] - b[0]) * (b[3] - b[1]);
        dx * dy / (s(a) + s(b) - dx * dy).max(1e-9)
    };
    for current in 0..frame_count {
        let bytes = input.line()?.ok_or("Confirmed tracking is truncated")?;
        let row: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if row["frame_index"].as_u64() != Some(current) {
            return Err("Confirmed tracking frames out of order".into());
        }
        let observations: Vec<(u64, Candidate)> = row["tracked_players"]
            .as_array()
            .ok_or("Missing confirmed tracks")?
            .iter()
            .map(|entry| {
                Ok((
                    entry["track_id"]
                        .as_u64()
                        .ok_or("Invalid confirmed Track ID")?,
                    serde_json::from_value::<Candidate>(entry["detection"].clone())
                        .map_err(|e| format!("Confirmed Track candidate: {e}"))?,
                ))
            })
            .collect::<Result<_, String>>()?;
        queued.push_back(row);
        anchors.retain(|_, (last, _)| current.saturating_sub(*last) <= 90);
        for (track_id, observation) in observations {
            if let Some((left, prior)) = anchors.get(&track_id) {
                let duration = current - left;
                if (2..=90).contains(&duration) && prior.score.min(observation.score) >= 0.35 {
                    let a = prior.bbox_xyxy_norm;
                    let b = observation.bbox_xyxy_norm;
                    let aw = a[2] - a[0];
                    let bw = b[2] - b[0];
                    let ah = a[3] - a[1];
                    let bh = b[3] - b[1];
                    let center = |r: [f64; 4]| [(r[0] + r[2]) * 0.5, (r[1] + r[3]) * 0.5];
                    let ca = center(a);
                    let cb = center(b);
                    let drift = (ca[0] - cb[0]).hypot(ca[1] - cb[1]);
                    if aw.min(bw) >= 0.005
                        && ah.min(bh) >= 0.02
                        && aw.max(bw) / aw.min(bw) <= 1.8
                        && ah.max(bh) / ah.min(bh) <= 1.8
                        && drift <= (0.035 + (duration - 1) as f64 * 0.003).min(0.15)
                    {
                        for entry in queued.iter_mut().filter(|v| {
                            let f = v["frame_index"].as_u64().unwrap_or(u64::MAX);
                            f > *left && f < current
                        }) {
                            let at = entry["frame_index"].as_u64().unwrap();
                            let t = (at - left) as f64 / duration as f64;
                            let interpolated_box =
                                std::array::from_fn(|i| a[i] * (1.0 - t) + b[i] * t);
                            let mut estimate = prior.clone();
                            estimate.frame_index = at;
                            estimate.bbox_xyxy_norm = interpolated_box;
                            estimate.score = prior.score.min(observation.score) * 0.80;
                            estimate.query_index = Some(0);
                            estimate.touch_probability = None;
                            estimate.head_point_norm = prior
                                .head_point_norm
                                .zip(observation.head_point_norm)
                                .map(|(a, b)| std::array::from_fn(|i| a[i] * (1.0 - t) + b[i] * t));
                            estimate.validate(at - at % 9, "players")?;
                            let rows = entry["tracked_players"]
                                .as_array_mut()
                                .ok_or("Invalid queued tracking frame")?;
                            if rows.len() >= 128
                                || rows.iter().any(|v| {
                                    v["track_id"].as_u64() == Some(track_id)
                                        || serde_json::from_value::<Candidate>(
                                            v["detection"].clone(),
                                        )
                                        .is_ok_and(|present| {
                                            iou(present.bbox_xyxy_norm, interpolated_box) > 0.65
                                                || volley_core::tracker::same_head_body_duplicate(
                                                    &present, &estimate,
                                                )
                                                || volley_core::tracker::same_partial_body_duplicate(
                                                    &present, &estimate,
                                                )
                                        })
                                })
                            {
                                continue;
                            }
                            rows.push(json!({
                                "track_id":track_id,
                                "detection":estimate,
                                "is_verified_athlete":false
                            }));
                            if entry["interpolated_track_ids"].is_null() {
                                entry["interpolated_track_ids"] = json!([]);
                            }
                            entry["interpolated_track_ids"]
                                .as_array_mut()
                                .unwrap()
                                .push(json!(track_id));
                            inferred += 1;
                        }
                    }
                }
            }
            anchors.insert(track_id, (current, observation));
        }
        while queued.len() > 90 {
            write_one(queued.pop_front().unwrap(), &mut output, &mut digest)?;
        }
    }
    for record in queued {
        write_one(record, &mut output, &mut digest)?;
    }
    if input.line()?.is_some() || input.finish() != expected_sha {
        return Err("Confirmed tracking changed during interpolation".into());
    }
    output.flush().map_err(|e| e.to_string())?;
    output.get_ref().sync_all().map_err(|e| e.to_string())?;
    drop(output);
    // Atomic on the repository's existing filesystem.
    fs::rename(&tmp, source).map_err(|e| format!("Commit confirmed tracking: {e}"))?;
    Ok((format!("{:x}", digest.finalize()), inferred))
}

fn replay_frames_verified(
    source: &Path,
    tentative: &Path,
    output: &Path,
    count: u64,
    config: TrackingOptions,
    expected_sha: &str,
) -> Result<(String, ReplayTotals, u64), String> {
    let mut reader = BoundedReader::open(source)?;
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(tentative)
        .map_err(|e| format!("Create unconfirmed CPU tracking snapshot: {e}"))?;
    let mut writer = BufWriter::new(file);
    let mut tentative_sha = Sha256::new();
    let mut tracker = ProvisionalTracker::new(config)?;
    let mut confirmations = TrackConfirmation::default();
    let mut totals = ReplayTotals::default();
    for expected_frame in 0..count {
        let line = reader.line()?.ok_or("Fused frames truncated")?;
        let frame: FusedFrame =
            serde_json::from_slice(&line).map_err(|e| format!("Invalid fused frame JSON: {e}"))?;
        if frame.frame_index != expected_frame
            || frame.player_track_ids_confirmed
            || frame.touches_confirmed
            || frame.ball_candidates.len() > 2
        {
            return Err("Fusion frame index or raw status changed".into());
        }
        let tracking = tracker.process_frame(expected_frame, &frame.player_candidates)?;
        confirmations.observe(expected_frame, &tracking.tracks)?;
        totals.frames += 1;
        totals.rejected += tracking.rejected_boxes as u64;
        totals.ambiguous += tracking.ambiguous_detections as u64;
        let selected_ball = frame
            .selected_ball
            .filter(|ball| ball["source_model"] == "vballnet");
        if selected_ball.is_some() {
            totals.frames_with_ball += 1;
        }
        let mut bytes = serde_json::to_vec(&json!({
            "frame_index":expected_frame,
            "tracked_players":tracking.tracks,
            "raw_player_candidates":frame.player_candidates,
            "ball_candidates":frame.ball_candidates,
            "selected_ball":selected_ball,
            "ambiguous_detections":tracking.ambiguous_detections,
            "track_ids_provisional":true,
            "confirmed_athlete_identities":false,
            "touches_confirmed":false,
        }))
        .map_err(|e| format!("Serialize CPU tracking frame: {e}"))?;
        bytes.push(b'\n');
        if bytes.len() > MAX_ROW_BYTES {
            return Err("Tracked CPU frame exceeds 512 KiB".into());
        }
        writer
            .write_all(&bytes)
            .map_err(|e| format!("Write unconfirmed tracked frame: {e}"))?;
        tentative_sha.update(&bytes);
    }
    if reader.line()?.is_some() {
        return Err("Extra undeclared GPU fusion frames".into());
    }
    if reader.finish() != expected_sha {
        return Err("GPU fusion checksum mismatch during CPU replay".into());
    }
    writer
        .flush()
        .map_err(|e| format!("Flush tentative tracking: {e}"))?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|e| format!("Sync tentative tracking: {e}"))?;
    drop(writer);
    let expected_tentative_sha = format!("{:x}", tentative_sha.finalize());
    let summary = confirmations.summary(config.min_track_observations);
    totals.detections = summary.confirmed_detections;
    totals.unconfirmed_tracks = summary.discarded_tracks;
    totals.short_track_detections = summary.discarded_detections;
    totals.tentative_tracks = tracker.next_id() - 1;
    debug_assert_eq!(
        totals.tentative_tracks,
        summary.confirmed_tracks + summary.discarded_tracks
    );

    // Round two copies only confirmed tracks into the immutable CPU snapshot.
    // Ball evidence and the original raw candidate proposals stay untouched,
    // but Touch/Rally stages will see only actual published Track IDs.
    let mut tentative_reader = BoundedReader::open(tentative)?;
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .map_err(|e| format!("Create filtered CPU tracking output: {e}"))?;
    let mut writer = BufWriter::new(file);
    let mut sha = Sha256::new();
    let mut retained = 0_u64;
    let mut discarded = 0_u64;
    for expected_frame in 0..count {
        let line = tentative_reader
            .line()?
            .ok_or("Unconfirmed tracking truncated")?;
        let mut frame: Value =
            serde_json::from_slice(&line).map_err(|e| format!("Invalid tentative frame: {e}"))?;
        if frame["frame_index"].as_u64() != Some(expected_frame) {
            return Err("Unconfirmed CPU frame index changed".into());
        }
        let players = frame["tracked_players"]
            .as_array_mut()
            .ok_or("Unconfirmed CPU tracks array missing")?;
        let before = players.len();
        players.retain(|player| {
            player["track_id"]
                .as_u64()
                .is_some_and(|id| confirmations.confirmed(id, config.min_track_observations))
        });
        retained += players.len() as u64;
        discarded += (before - players.len()) as u64;
        let mut bytes = serde_json::to_vec(&frame)
            .map_err(|e| format!("Serialize filtered CPU tracking frame: {e}"))?;
        bytes.push(b'\n');
        if bytes.len() > MAX_ROW_BYTES {
            return Err("Filtered CPU frame exceeds 512 KiB".into());
        }
        writer
            .write_all(&bytes)
            .map_err(|e| format!("Write filtered CPU frame: {e}"))?;
        sha.update(&bytes);
    }
    if tentative_reader.line()?.is_some() {
        return Err("Unconfirmed tracking contains extra rows".into());
    }
    if tentative_reader.finish() != expected_tentative_sha
        || retained != summary.confirmed_detections
        || discarded != summary.discarded_detections
    {
        return Err("Tentative CPU tracking changed between verification and filtering".into());
    }
    writer
        .flush()
        .map_err(|e| format!("Flush confirmed CPU frames: {e}"))?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|e| format!("Sync confirmed CPU frames: {e}"))?;
    drop(writer);
    let observed_sha = format!("{:x}", sha.finalize());
    let (continuous_sha, _inferred_positions) =
        fill_confirmed_track_gaps(output, &observed_sha, count)?;
    Ok((continuous_sha, totals, summary.confirmed_tracks))
}

pub(crate) fn process(state: &AppState, req: &ReplayRequest) -> Result<Value, ApiError> {
    if !state.writes_enabled {
        return Err(storage::conflict(
            "CPU replay requires isolated VOLLEY_RUST_DATA_ROOT",
        ));
    }
    storage::project_id(&req.project_id)?;
    storage::video_name(&req.video)?;
    if !opaque_id(&req.fusion_id) {
        return Err(storage::bad_request("Invalid GPU fusion ID"));
    }
    let settings = req
        .settings
        .unwrap_or_default()
        .validate()
        .map_err(storage::bad_request)?;
    let _guard = project_guard::write_guard(state, &req.project_id)?;
    let project = projects::existing_project(state, &req.project_id)?;
    if !project["videos"]
        .as_array()
        .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(&req.video)))
    {
        return Err(storage::bad_request(
            "CPU replay video does not belong to project",
        ));
    }
    let plan = preparation::plan_for(state, &req.project_id, &req.video)?;
    let prepared = plan
        .ready_path()
        .ok_or_else(|| storage::bad_request("Prepared video is unavailable"))?;
    let (size, mtime) = inference_stream::source_stamp(prepared).map_err(storage::bad_request)?;
    let project_root = projects::project_root(state).join(&req.project_id);
    safe_folder(&project_root).map_err(storage::bad_request)?;
    let fusions = project_root.join("native_gpu_fusions");
    safe_folder(&fusions).map_err(storage::bad_request)?;
    let source_folder = fusions.join(&req.fusion_id);
    safe_folder(&source_folder).map_err(storage::bad_request)?;
    let manifest: Value = serde_json::from_slice(
        &fs::read(
            safe_file(&source_folder, "manifest.json", MAX_MANIFEST_BYTES)
                .map_err(storage::bad_request)?,
        )
        .map_err(|e| storage::internal(format!("Read fusion manifest: {e}")))?,
    )
    .map_err(|e| storage::bad_request(format!("Invalid fusion manifest: {e}")))?;
    if manifest["schema"] != "volley-native-gpu-fusion/1"
        || manifest["state"] != "raw_candidates_fused"
        || manifest["project_id"].as_str() != Some(&req.project_id)
        || manifest["fusion_id"].as_str() != Some(&req.fusion_id)
        || manifest["video"].as_str() != Some(&req.video)
        || manifest["fps"] != 30
        || manifest["file"] != "frames.jsonl"
        || manifest["cpu_processing_complete"] != false
        || manifest["player_track_ids_confirmed"] != false
        || manifest["touches_confirmed"] != false
        || manifest["source_identity"] != json!({"bytes":size,"mtime_ns":mtime})
    {
        return Err(storage::bad_request(
            "GPU fusion does not match project or prepared video",
        ));
    }
    let frames = manifest["emitted_frames"]
        .as_u64()
        .filter(|v| *v > 0 && *v <= 48 * 3600 * 30)
        .ok_or_else(|| storage::bad_request("Invalid GPU fusion frame count"))?;
    if manifest["windows"].as_u64() != Some(frames / 9) || frames % 9 != 0 {
        return Err(storage::bad_request(
            "Fusion window count does not match emitted frames",
        ));
    }
    let expected = manifest["sha256"]
        .as_str()
        .filter(|v| {
            v.len() == 64
                && v.bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        })
        .ok_or_else(|| storage::bad_request("Fusion SHA-256 missing or invalid"))?;
    let source =
        safe_file(&source_folder, "frames.jsonl", MAX_INPUT_BYTES).map_err(storage::bad_request)?;
    let root = project_root.join("native_cpu_replays");
    match fs::symlink_metadata(&root) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
            return Err(storage::bad_request("Unsafe CPU replay directory"))
        }
        Ok(_) => (),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => fs::create_dir(&root)
            .map_err(|e| storage::internal(format!("Create CPU replay root: {e}")))?,
        Err(e) => return Err(storage::internal(format!("Check CPU replay root: {e}"))),
    }
    let replay_id = Uuid::new_v4().simple().to_string();
    let directory = root.join(&replay_id);
    fs::create_dir(&directory).map_err(|e| storage::internal(format!("Create CPU replay: {e}")))?;
    let result = (|| -> Result<Value, String> {
        let part = directory.join("tracked_frames.jsonl.partial");
        let (digest, totals, track_count) =
            replay_frames(&source, &part, frames, settings, expected)?;
        if inference_stream::source_stamp(prepared)? != (size, mtime) {
            return Err("Prepared video changed during CPU replay".into());
        }
        fs::rename(part, directory.join(OUTPUT_FILE))
            .map_err(|e| format!("Publish CPU replay output: {e}"))?;
        let record = json!({
            "schema":"volley-native-cpu-tracking/1",
            "state":"provisional_tracklets_rebuilt",
            "interpolated_positions_version":"bounded_same_track_3s_v1",
            "project_id":req.project_id,
            "video":req.video,
            "replay_id":replay_id,
            "source_fusion_id":req.fusion_id,
            "source_fusion_sha256":expected,
            "ball_selection_policy":"vballnet_only_v1",
            "source_identity":{"bytes":size,"mtime_ns":mtime},
            "fps":30,
            "settings":settings,
            "frames":totals.frames,
            "tracked_player_detections":totals.detections,
            "discarded_player_detections":totals.rejected + totals.short_track_detections,
            "discarded_short_track_detections":totals.short_track_detections,
            "discarded_short_track_ids":totals.unconfirmed_tracks,
            "tentative_track_count":totals.tentative_tracks,
            "ambiguous_detections":totals.ambiguous,
            "frames_with_ball_proposals":totals.frames_with_ball,
            "provisional_track_count":track_count,
            "file":OUTPUT_FILE,
            "sha256":digest,
            "track_ids_provisional":true,
            "athlete_identities_confirmed":false,
            "touches_confirmed":false,
            "cpu_processing_complete":false,
        });
        storage::write_json_atomic(&directory.join("manifest.json"), &record)
            .map_err(|e| format!("Persist CPU tracking manifest: {e:?}"))?;
        Ok(record)
    })();
    match result {
        Ok(v) => Ok(v),
        Err(error) => {
            let _ = fs::remove_dir_all(&directory);
            Err(storage::bad_request(error))
        }
    }
}

pub async fn replay(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<ReplayRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    let result = tokio::task::spawn_blocking(move || process(&state, &request))
        .await
        .map_err(|e| storage::internal(format!("Native CPU tracking worker: {e}")))??;
    Ok(Json(result))
}
pub async fn status(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ReplayQuery>,
) -> ApiResult<Value> {
    storage::project_id(&query.project_id)?;
    if !opaque_id(&query.replay_id) {
        return Err(storage::bad_request("Invalid CPU replay ID"));
    }
    projects::existing_project(&state, &query.project_id)?;
    let project = projects::project_root(&state).join(&query.project_id);
    safe_folder(&project).map_err(storage::bad_request)?;
    let root = project.join("native_cpu_replays");
    safe_folder(&root).map_err(storage::bad_request)?;
    let folder = root.join(&query.replay_id);
    safe_folder(&folder).map_err(storage::bad_request)?;
    let value: Value = serde_json::from_slice(
        &fs::read(
            safe_file(&folder, "manifest.json", MAX_MANIFEST_BYTES)
                .map_err(storage::bad_request)?,
        )
        .map_err(|e| storage::internal(format!("Read CPU manifest: {e}")))?,
    )
    .map_err(|e| storage::bad_request(format!("Malformed CPU replay manifest: {e}")))?;
    if value["schema"] != "volley-native-cpu-tracking/1"
        || value["replay_id"].as_str() != Some(&query.replay_id)
        || value["project_id"].as_str() != Some(&query.project_id)
    {
        return Err(storage::bad_request(
            "CPU replay metadata belongs to a different project",
        ));
    }
    Ok(Json(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn example(frame: u64, x: f64, q: usize) -> Candidate {
        Candidate {
            frame_index: frame,
            class_name: "player".into(),
            score: 0.9,
            bbox_xyxy_norm: [x, 0.1, x + 0.1, 0.7],
            query_index: Some(q),
            touch_probability: Some(0.8),
            head_point_norm: None,
        }
    }
    #[test]
    fn cpu_replays_hash_bound_frames_and_provisional_ids() {
        let root = std::env::temp_dir().join(format!("volley-replay-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let file = root.join("source.jsonl");
        let mut bytes = Vec::new();
        for i in 0..18 {
            let item = json!({
                "frame_index":i,
                "player_candidates":if i<12 {
                    vec![example(i,0.2+i as f64*0.002,i as usize % 2)]
                }else{vec![]},
                "ball_candidates":if i==0 { vec![json!({"source_model":"ravel","detection":{"frame_index":0,"class_name":"ball","score":0.9,"bbox_xyxy_norm":[0.2,0.2,0.3,0.3]}})] } else { Vec::<Value>::new() },
                "selected_ball":if i==0 { json!({"source_model":"ravel","detection":{"frame_index":0,"class_name":"ball","score":0.9,"bbox_xyxy_norm":[0.2,0.2,0.3,0.3]}}) } else { Value::Null },
                "player_track_ids_confirmed":false,
                "touches_confirmed":false,
            });
            bytes.extend(serde_json::to_vec(&item).unwrap());
            bytes.push(b'\n');
        }
        fs::write(&file, &bytes).unwrap();
        let original_sha = format!("{:x}", Sha256::digest(&bytes));
        let output = root.join("out.jsonl");
        let (sha, counts, ids) = replay_frames(
            &file,
            &output,
            18,
            TrackingOptions::default(),
            &original_sha,
        )
        .unwrap();
        assert_eq!(ids, 1);
        assert_eq!(counts.detections, 12);
        assert_eq!(counts.frames, 18);
        assert_eq!(counts.tentative_tracks, 1);
        assert_eq!(counts.unconfirmed_tracks, 0);
        assert_eq!(
            sha,
            format!("{:x}", Sha256::digest(fs::read(&output).unwrap()))
        );
        let frames = fs::read_to_string(output)
            .unwrap()
            .lines()
            .map(|v| serde_json::from_str::<Value>(v).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(frames[0]["tracked_players"][0]["track_id"], 1);
        assert!(frames[0]["selected_ball"].is_null());
        assert_eq!(frames[0]["ball_candidates"][0]["source_model"], "ravel");
        assert_eq!(frames[1]["tracked_players"][0]["track_id"], 1);
        assert_eq!(
            frames[1]["tracked_players"][0]["detection"]["query_index"],
            1
        );
        assert_eq!(frames[1]["touches_confirmed"], false);
        assert_eq!(frames[11]["tracked_players"][0]["track_id"], 1);
        assert_eq!(frames[12]["tracked_players"].as_array().unwrap().len(), 0);
        let mut corrupted = bytes.clone();
        corrupted[10] = b' ';
        fs::write(&file, corrupted).unwrap();
        assert!(replay_frames(
            &file,
            &root.join("bad.jsonl"),
            18,
            TrackingOptions::default(),
            &original_sha
        )
        .is_err());
        assert!(!root.join("bad.jsonl").exists());
        assert!(!root.join("bad.unconfirmed").exists());
        fs::remove_dir_all(&root).unwrap();
    }
    #[test]
    fn real_player_detection_blocks_duplicate_interpolated_id() {
        let root = std::env::temp_dir().join(format!("volley-duplicate-gap-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("tracked.jsonl");
        let mut bytes = Vec::new();
        for frame in 0..6_u64 {
            let mut tracked = Vec::new();
            if frame == 0 || frame == 5 {
                let mut full = example(frame, 0.50, 0);
                full.bbox_xyxy_norm = [0.50, 0.40, 0.54, 0.55];
                full.head_point_norm = Some([0.52, 0.41]);
                full.score = 0.80;
                tracked.push(json!({"track_id":1,"detection":full,"is_verified_athlete":false}));
            }
            if (1..=4).contains(&frame) {
                let mut crop = example(frame, 0.502, 1);
                crop.bbox_xyxy_norm = [0.502, 0.40, 0.538, 0.48];
                crop.head_point_norm = Some([0.521, 0.41]);
                crop.score = 0.70;
                tracked.push(json!({"track_id":2,"detection":crop,"is_verified_athlete":false}));
            }
            bytes.extend(serde_json::to_vec(&json!({
                "frame_index":frame,"tracked_players":tracked,
                "track_ids_provisional":true,"confirmed_athlete_identities":false,
                "touches_confirmed":false
            })).unwrap());
            bytes.push(b'\n');
        }
        fs::write(&source, &bytes).unwrap();
        let sha = format!("{:x}", Sha256::digest(&bytes));
        let (written, inferred) = fill_confirmed_track_gaps(&source, &sha, 6).unwrap();
        assert_eq!(inferred, 0);
        assert_eq!(written, format!("{:x}", Sha256::digest(fs::read(&source).unwrap())));
        for (frame, line) in fs::read_to_string(&source).unwrap().lines().enumerate() {
            let row: Value = serde_json::from_str(line).unwrap();
            let tracks = row["tracked_players"].as_array().unwrap();
            assert_eq!(tracks.len(), 1, "duplicate on frame {frame}");
            assert_eq!(tracks[0]["track_id"], if frame == 0 || frame == 5 {1} else {2});
        }
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn cpu_discards_short_ids_from_every_frame_but_preserves_raw_candidates() {
        let root = std::env::temp_dir().join(format!("volley-short-id-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("fusion.jsonl");
        let output = root.join("tracked.jsonl");
        let mut raw = Vec::new();
        for frame in 0..30_u64 {
            let mut players = Vec::new();
            if frame < 25 && !(12..=14).contains(&frame) {
                players.push(example(
                    frame,
                    0.2 + frame as f64 * 0.001,
                    frame as usize % 2,
                ));
            }
            if (5..=7).contains(&frame) {
                players.push(example(frame, 0.72, 2));
            }
            let item = json!({
                "frame_index":frame,
                "player_candidates":players,
                "ball_candidates":[],
                "selected_ball":null,
                "player_track_ids_confirmed":false,
                "touches_confirmed":false,
            });
            raw.extend(serde_json::to_vec(&item).unwrap());
            raw.push(b'\n');
        }
        fs::write(&source, &raw).unwrap();
        let sha = format!("{:x}", Sha256::digest(&raw));
        let (digest, totals, ids) =
            replay_frames(&source, &output, 30, TrackingOptions::default(), &sha).unwrap();
        assert_eq!(totals.tentative_tracks, 2);
        assert_eq!(ids, 1);
        assert_eq!(totals.unconfirmed_tracks, 1);
        assert_eq!(totals.short_track_detections, 3);
        assert_eq!(totals.detections, 22);
        assert_eq!(
            digest,
            format!("{:x}", Sha256::digest(fs::read(&output).unwrap()))
        );
        let frames: Vec<Value> = fs::read_to_string(&output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        for frame in &frames {
            let tracks = frame["tracked_players"].as_array().unwrap();
            assert!(tracks.iter().all(|track| track["track_id"] == 1));
            assert!(tracks.len() <= 1);
        }
        assert_eq!(frames[5]["tracked_players"].as_array().unwrap().len(), 1);
        assert_eq!(
            frames[5]["raw_player_candidates"].as_array().unwrap().len(),
            2
        );
        assert_eq!(frames[13]["tracked_players"][0]["track_id"], 1);
        assert_eq!(frames[13]["interpolated_track_ids"][0], 1);
        assert!(frames[13]["raw_player_candidates"]
            .as_array()
            .unwrap()
            .is_empty());
        assert!(
            frames[13]["tracked_players"][0]["detection"]["score"]
                .as_f64()
                .unwrap()
                < frames[15]["tracked_players"][0]["detection"]["score"]
                    .as_f64()
                    .unwrap()
        );
        assert_eq!(
            frames[15]["tracked_players"][0]["track_id"], 1,
            "3-frame occlusion must not fragment the persistent athlete"
        );
        assert_eq!(fs::read(&source).unwrap(), raw);
        assert!(!root.join("tracked.unconfirmed").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn confirmation_threshold_12_is_inclusive_and_cannot_fabricate_ids() {
        let root = std::env::temp_dir().join(format!("volley-threshold-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("fusion.jsonl");
        let mut raw = Vec::new();
        for frame in 0..18 {
            raw.extend(
                serde_json::to_vec(&json!({
                    "frame_index":frame,
                    "player_candidates":if frame<11 {vec![example(frame,0.2,0)]}else{vec![]},
                    "ball_candidates":[],"selected_ball":null,
                    "player_track_ids_confirmed":false,"touches_confirmed":false
                }))
                .unwrap(),
            );
            raw.push(b'\n');
        }
        fs::write(&source, &raw).unwrap();
        let hash = format!("{:x}", Sha256::digest(&raw));
        let (_, counts, ids) = replay_frames(
            &source,
            &root.join("hidden.jsonl"),
            18,
            TrackingOptions::default(),
            &hash,
        )
        .unwrap();
        assert_eq!(ids, 0);
        assert_eq!(counts.unconfirmed_tracks, 1);
        assert_eq!(counts.short_track_detections, 11);
        let (_, counts, ids) = replay_frames(
            &source,
            &root.join("visible.jsonl"),
            18,
            TrackingOptions {
                min_track_observations: 11,
                ..TrackingOptions::default()
            },
            &hash,
        )
        .unwrap();
        assert_eq!(ids, 1);
        assert_eq!(counts.detections, 11);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cpu_recalc_versions_from_same_fusion_without_gpu() {
        use std::process::Command;
        if Command::new("ffmpeg").arg("-version").output().is_err() {
            return;
        }
        let root = std::env::temp_dir().join(format!("volley-recalc-{}", Uuid::new_v4()));
        let repo = root.join("original");
        let data = root.join("sandbox");
        let id = "1234abcdef01";
        let video = "fixture.mp4";
        let fusion_id = "0123456789abcdef0123456789abcdef";
        fs::create_dir_all(repo.join("project")).unwrap();
        let project_dir = data.join("projects").join(id);
        let fusion_dir = project_dir.join("native_gpu_fusions").join(fusion_id);
        fs::create_dir_all(&fusion_dir).unwrap();
        let source = repo.join("project").join(video);
        let command = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=64x64:rate=30:duration=0.6",
                "-an",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-y",
            ])
            .arg(&source)
            .status()
            .unwrap();
        assert!(command.success());
        storage::write_json_atomic(
            &data.join("projects").join("projects.json"),
            &json!({"schema":1,"projects":[{"id":id,"videos":[video],"name":"Test"}]}),
        )
        .unwrap();
        let state = AppState {
            repository_root: repo.clone(),
            data_root: data.clone(),
            writes_enabled: true,
            montage: crate::montage::MontageManager::new(),
            preparation: crate::preparation::PreparationManager::new(),
            preview_limit: tokio::sync::Semaphore::new(1),
            touch_limit: tokio::sync::Semaphore::new(1),
            continuity_limit: tokio::sync::Semaphore::new(1),
            reid_limit: tokio::sync::Semaphore::new(1),
            identity_limit: tokio::sync::Semaphore::new(1),
            statistics_limit: tokio::sync::Semaphore::new(1),
            marks_limit: tokio::sync::Semaphore::new(1),
            quick_reel: crate::quick_reel::QuickReelManager::new(),
            inference_limit: tokio::sync::Semaphore::new(1),
            full_inference: crate::inference_full::FullInferenceManager::new(),
        };
        let plan = preparation::plan_for(&state, id, video).unwrap();
        assert!(plan.ready_path().is_some());
        let (size, mtime) = inference_stream::source_stamp(&source).unwrap();
        let mut raw = Vec::new();
        for frame in 0..18_u64 {
            let proposal = Candidate {
                frame_index: frame,
                class_name: "player".into(),
                score: 0.8,
                bbox_xyxy_norm: [0.10, 0.20, 0.30, 0.70],
                query_index: Some((frame % 2) as usize),
                touch_probability: Some(0.7),
                head_point_norm: None,
            };
            raw.extend(
                serde_json::to_vec(&json!({
                    "frame_index":frame,
                    "player_candidates":[proposal],
                    "ball_candidates":[],
                    "selected_ball":null,
                    "player_track_ids_confirmed":false,
                    "touches_confirmed":false,
                }))
                .unwrap(),
            );
            raw.push(b'\n');
        }
        fs::write(fusion_dir.join("frames.jsonl"), &raw).unwrap();
        let rawsha = format!("{:x}", Sha256::digest(&raw));
        storage::write_json_atomic(
            &fusion_dir.join("manifest.json"),
            &json!({
                "schema":"volley-native-gpu-fusion/1",
                "state":"raw_candidates_fused",
                "project_id":id,"video":video,"fusion_id":fusion_id,
                "fps":30,"windows":2,"emitted_frames":18,
                "file":"frames.jsonl","sha256":rawsha,
                "source_identity":{"bytes":size,"mtime_ns":mtime},
                "cpu_processing_complete":false,
                "player_track_ids_confirmed":false,
                "touches_confirmed":false,
            }),
        )
        .unwrap();
        let request = ReplayRequest {
            project_id: id.into(),
            video: video.into(),
            fusion_id: fusion_id.into(),
            settings: None,
        };
        let first = process(&state, &request).unwrap();
        assert_eq!(first["provisional_track_count"], 1);
        assert_eq!(first["tracked_player_detections"], 18);
        let stricter = ReplayRequest {
            settings: Some(TrackingOptions {
                min_player_score: 0.95,
                ..TrackingOptions::default()
            }),
            ..request
        };
        let second = process(&state, &stricter).unwrap();
        assert_eq!(second["provisional_track_count"], 0);
        assert_eq!(second["discarded_player_detections"], 18);
        assert_ne!(first["replay_id"], second["replay_id"]);
        assert_eq!(fs::read(fusion_dir.join("frames.jsonl")).unwrap(), raw);
        let first_id = first["replay_id"].as_str().unwrap();
        let first_manifest = data
            .join("projects")
            .join(id)
            .join("native_cpu_replays")
            .join(first_id)
            .join("manifest.json");
        assert_eq!(storage::read_json(&first_manifest).unwrap().unwrap(), first);
        assert!(!repo.join("results").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reject_unsafe_count_and_invalid_id() {
        assert!(!opaque_id("../foo"));
        assert!(opaque_id("0123456789abcdef0123456789abcdef"));
        let folder = std::env::temp_dir().join(format!("cpu-safe-{}", Uuid::new_v4()));
        fs::create_dir_all(&folder).unwrap();
        assert!(replay_frames(
            &folder.join("missing"),
            &folder.join("out"),
            0,
            TrackingOptions::default(),
            "sha"
        )
        .is_err());
        fs::remove_dir_all(folder).unwrap();
    }
}
