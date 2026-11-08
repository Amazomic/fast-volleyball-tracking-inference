//! Native CPU provisional rally reconstruction over immutable, SHA-256
//! verified player-track and motion-supported Touch replay snapshots.
//! This does NOT certify rallies, serve/attack techniques, or player identity.
use crate::{
    cpu_retrack::{opaque_id, safe_file, safe_folder, BoundedReader},
    inference_stream, preparation, project_archive, projects,
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
    io::{BufWriter, Write},
    path::Path,
    sync::Arc,
};
use uuid::Uuid;
use volley_core::{
    fusion::Candidate,
    rally_reconstruction::{
        BallFrame, ContactFrame, RallyProposal, RallySegmenter, RallySettings, RallyTotals,
    },
};

const MAX_MANIFEST: u64 = 16 * 1024;
const MAX_EVIDENCE: u64 = 64 * 1024 * 1024 * 1024;
const OUTPUT_FILE: &str = "rally_candidates.jsonl";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RallyRequest {
    pub(crate) project_id: String,
    pub(crate) video: String,
    pub(crate) replay_id: String,
    pub(crate) touch_id: String,
    #[serde(default)]
    pub(crate) settings: Option<RallySettings>,
}

#[derive(Deserialize)]
pub struct RallyQuery {
    project_id: String,
    rally_run_id: String,
}

#[derive(Deserialize)]
struct SelectedBall {
    source_model: String,
    detection: Candidate,
}
#[derive(Deserialize)]
struct ReplayFrame {
    frame_index: u64,
    selected_ball: Option<SelectedBall>,
    track_ids_provisional: bool,
    confirmed_athlete_identities: bool,
    touches_confirmed: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MotionContact {
    frame_index: u64,
    provisional_track_id: Option<u64>,
    probability: f64,
    ball_x_px: f64,
    ball_y_px: f64,
    motion_frame_index: u64,
    direction_change_deg: f64,
    speed_before_px_frame: f64,
    speed_after_px_frame: f64,
    velocity_residual_px_frame: f64,
    technique: String,
    physical_motion_supported: bool,
    athlete_identity_verified: bool,
}
impl MotionContact {
    fn verify(&self, frames: u64, width: u32, height: u32) -> Result<(), String> {
        if self.frame_index >= frames
            || !self.physical_motion_supported
            || self.athlete_identity_verified
            || self.technique != "unknown_touch"
            || self
                .provisional_track_id
                .is_some_and(|id| id == 0 || id > i64::MAX as u64)
            || self.motion_frame_index >= frames
            || self.motion_frame_index.abs_diff(self.frame_index) > 4
            || !self.probability.is_finite()
            || !(0.0..=1.0).contains(&self.probability)
            || ![
                self.ball_x_px,
                self.ball_y_px,
                self.direction_change_deg,
                self.speed_before_px_frame,
                self.speed_after_px_frame,
                self.velocity_residual_px_frame,
            ]
            .iter()
            .all(|n| n.is_finite() && *n >= 0.0)
            || self.ball_x_px > width as f64
            || self.ball_y_px > height as f64
            || self.direction_change_deg > 180.0
            || self.velocity_residual_px_frame == 0.0
        {
            return Err("Invalid, out-of-order or nonphysical Touch evidence".into());
        }
        Ok(())
    }
}

fn source_manifest(folder: &Path, kind: &str) -> Result<Value, String> {
    safe_folder(folder)?;
    let bytes = fs::read(safe_file(folder, "manifest.json", MAX_MANIFEST)?)
        .map_err(|e| format!("Read {kind} manifest: {e}"))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("Malformed {kind} manifest: {e}"))
}
fn sha(value: &Value) -> Result<String, String> {
    value
        .as_str()
        .filter(|s| {
            s.len() == 64
                && s.bytes()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        })
        .map(str::to_owned)
        .ok_or("Missing or invalid SHA-256".into())
}

fn next_contact(reader: &mut BoundedReader) -> Result<Option<MotionContact>, String> {
    reader
        .line()?
        .map(|bytes| {
            serde_json::from_slice(&bytes).map_err(|e| format!("Corrupt Touch JSONL: {e}"))
        })
        .transpose()
}

#[derive(Default)]
struct OutputCounters {
    proposals: u64,
    ingested_touch_candidates: u64,
}
fn write_proposal(
    writer: &mut BufWriter<File>,
    out_hash: &mut Sha256,
    proposal: RallyProposal,
    replay_id: &str,
    touch_id: &str,
    counters: &mut OutputCounters,
) -> Result<(), String> {
    let mut value =
        serde_json::to_value(&proposal).map_err(|e| format!("Serialize rally candidate: {e}"))?;
    let key = format!(
        "{replay_id}:{touch_id}:{}:{}",
        proposal.detected_start_frame, proposal.detected_end_frame
    );
    let unique = format!("{:x}", Sha256::digest(key.as_bytes()));
    value["candidate_id"] = json!(&unique[..24]);
    value["reviewed"] = json!(false);
    let mut data =
        serde_json::to_vec(&value).map_err(|e| format!("Encode rally candidate: {e}"))?;
    data.push(b'\n');
    writer
        .write_all(&data)
        .map_err(|e| format!("Write rally candidate: {e}"))?;
    out_hash.update(&data);
    counters.proposals += 1;
    Ok(())
}

fn rebuild_from_snapshots(
    frames_file: &Path,
    touches_file: &Path,
    replay_sha: &str,
    touch_sha: &str,
    frames: u64,
    expected_touches: u64,
    width: u32,
    height: u32,
    settings: RallySettings,
    output: &Path,
    replay_id: &str,
    touch_id: &str,
) -> Result<(String, RallyTotals, OutputCounters), String> {
    settings.validate()?;
    if frames == 0 || frames > 48 * 3600 * 30 || expected_touches > frames {
        return Err("Native rally source frame/contact count exceeds limits".into());
    }
    let mut inputs = BoundedReader::open(frames_file)?;
    let mut touches = BoundedReader::open(touches_file)?;
    let output_file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(output)
        .map_err(|e| format!("Create isolated rally output: {e}"))?;
    let mut writer = BufWriter::new(output_file);
    let mut digest = Sha256::new();
    let mut totals = OutputCounters::default();
    let mut segmenter = RallySegmenter::new(settings, width, height)?;
    let mut next = next_contact(&mut touches)?;
    let mut previous_contact: Option<u64> = None;
    for frame_index in 0..frames {
        let bytes = inputs.line()?.ok_or("Tracked frame JSONL is truncated")?;
        let frame: ReplayFrame = serde_json::from_slice(&bytes)
            .map_err(|e| format!("Malformed tracked source frame: {e}"))?;
        if frame.frame_index != frame_index
            || !frame.track_ids_provisional
            || frame.confirmed_athlete_identities
            || frame.touches_confirmed
        {
            return Err("Tracked frame ordering or identity status is incompatible".into());
        }
        let ball = match frame.selected_ball {
            Some(ball) => {
                if !matches!(ball.source_model.as_str(), "vballnet" | "ravel")
                    || ball.detection.frame_index != frame_index
                    || ball.detection.class_name != "ball"
                {
                    return Err("Invalid selected-ball proposal from CPU replay".into());
                }
                let model = if ball.source_model == "ravel" {
                    "players"
                } else {
                    "ball"
                };
                ball.detection
                    .validate(frame_index - frame_index % 9, model)?;
                let [left, top, right, bottom] = ball.detection.bbox_xyxy_norm;
                Some(BallFrame {
                    x_px: (left + right) * 0.5 * width as f64,
                    y_px: (top + bottom) * 0.5 * height as f64,
                    score: ball.detection.score,
                })
            }
            None => None,
        };
        let mut events = Vec::new();
        while next.as_ref().is_some_and(|t| t.frame_index == frame_index) {
            let item = next.take().expect("checked");
            item.verify(frames, width, height)?;
            if previous_contact.is_some_and(|prev| item.frame_index <= prev) {
                return Err("Motion contacts must be strictly ordered and unique".into());
            }
            previous_contact = Some(item.frame_index);
            totals.ingested_touch_candidates += 1;
            events.push(ContactFrame {
                probability: item.probability,
                provisional_track_id: item.provisional_track_id,
            });
            next = next_contact(&mut touches)?;
        }
        if next.as_ref().is_some_and(|t| t.frame_index < frame_index) {
            return Err("Out-of-order native Touch frame index".into());
        }
        if let Some(proposal) = segmenter.push(frame_index, ball, &events)? {
            write_proposal(
                &mut writer,
                &mut digest,
                proposal,
                replay_id,
                touch_id,
                &mut totals,
            )?;
        }
    }
    if next.is_some() || inputs.line()?.is_some() || touches.line()?.is_some() {
        return Err("Native rally inputs contain undeclared frames or contacts".into());
    }
    if totals.ingested_touch_candidates != expected_touches {
        return Err("Touch candidate count differs from versioned manifest".into());
    }
    if inputs.finish() != replay_sha || touches.finish() != touch_sha {
        return Err("Native rally source SHA-256 mismatch".into());
    }
    if let Some(proposal) = segmenter.finish() {
        write_proposal(
            &mut writer,
            &mut digest,
            proposal,
            replay_id,
            touch_id,
            &mut totals,
        )?;
    }
    writer
        .flush()
        .map_err(|e| format!("Flush rally snapshot: {e}"))?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|e| format!("Sync rally snapshot: {e}"))?;
    Ok((
        format!("{:x}", digest.finalize()),
        segmenter.totals().clone(),
        totals,
    ))
}

pub(crate) fn process(state: &AppState, request: &RallyRequest) -> Result<Value, ApiError> {
    if !state.writes_enabled {
        return Err(storage::conflict(
            "Native rally analysis requires a separate Rust sandbox",
        ));
    }
    storage::project_id(&request.project_id)?;
    storage::video_name(&request.video)?;
    if !opaque_id(&request.replay_id) || !opaque_id(&request.touch_id) {
        return Err(storage::bad_request(
            "Invalid CPU Track ID or Touch snapshot ID",
        ));
    }
    let settings = request
        .settings
        .unwrap_or_default()
        .validate()
        .map_err(storage::bad_request)?;
    let _lock = project_archive::archive_guard(state, &request.project_id)?;
    let project = projects::existing_project(state, &request.project_id)?;
    if !project["videos"]
        .as_array()
        .is_some_and(|v| v.iter().any(|name| name.as_str() == Some(&request.video)))
    {
        return Err(storage::bad_request("Video does not belong to project"));
    }
    let plan = preparation::plan_for(state, &request.project_id, &request.video)?;
    let prepared = plan
        .ready_path()
        .ok_or_else(|| storage::bad_request("Prepared video missing from selected project"))?;
    if plan.identity.fps != 30 {
        return Err(storage::bad_request("Native rally source must be 30 FPS"));
    }
    let (size, mtime) = inference_stream::source_stamp(prepared).map_err(storage::bad_request)?;
    let dimensions = volley_media::probe(prepared).map_err(storage::bad_request)?;
    let project_dir = projects::project_root(state).join(&request.project_id);
    safe_folder(&project_dir).map_err(storage::bad_request)?;
    let track_root = project_dir.join("native_cpu_replays");
    let touch_root = project_dir.join("native_cpu_touch_runs");
    safe_folder(&track_root).map_err(storage::bad_request)?;
    safe_folder(&touch_root).map_err(storage::bad_request)?;
    let track_dir = track_root.join(&request.replay_id);
    let touch_dir = touch_root.join(&request.touch_id);
    let tracked = source_manifest(&track_dir, "CPU Track ID").map_err(storage::bad_request)?;
    let contacts = source_manifest(&touch_dir, "CPU Touch").map_err(storage::bad_request)?;
    let identity = json!({"bytes":size,"mtime_ns":mtime});
    if tracked["schema"] != "volley-native-cpu-tracking/1"
        || tracked["state"] != "provisional_tracklets_rebuilt"
        || tracked["project_id"].as_str() != Some(&request.project_id)
        || tracked["video"].as_str() != Some(&request.video)
        || tracked["replay_id"].as_str() != Some(&request.replay_id)
        || tracked["source_identity"] != identity
        || tracked["track_ids_provisional"] != true
        || tracked["athlete_identities_confirmed"] != false
        || tracked["touches_confirmed"] != false
        || tracked["cpu_processing_complete"] != false
        || tracked["fps"] != 30
        || tracked["file"] != "tracked_frames.jsonl"
        || contacts["schema"] != "volley-native-cpu-touch-proposals/1"
        || contacts["state"] != "motion_gated_touch_proposals"
        || contacts["project_id"].as_str() != Some(&request.project_id)
        || contacts["video"].as_str() != Some(&request.video)
        || contacts["touch_id"].as_str() != Some(&request.touch_id)
        || contacts["source_replay_id"].as_str() != Some(&request.replay_id)
        || contacts["source_identity"] != identity
        || contacts["width"].as_u64() != Some(dimensions.width as u64)
        || contacts["height"].as_u64() != Some(dimensions.height as u64)
        || contacts["fps"] != 30
        || contacts["file"] != "touch_candidates.jsonl"
        || contacts["contacts_authoritatively_confirmed"] != false
        || contacts["athlete_identities_verified"] != false
        || contacts["cpu_processing_complete"] != false
    {
        return Err(storage::bad_request(
            "CPU tracking and Touch source manifests are incompatible",
        ));
    }
    let frames = tracked["frames"]
        .as_u64()
        .filter(|n| *n > 0 && *n <= 48 * 3600 * 30 && contacts["frames"].as_u64() == Some(*n))
        .ok_or_else(|| storage::bad_request("Native rally frame counts do not match"))?;
    let expected_touches = contacts["touch_candidates"]
        .as_u64()
        .filter(|n| *n <= frames)
        .ok_or_else(|| storage::bad_request("Unbounded or missing motion Touch count"))?;
    let replay_sha = sha(&tracked["sha256"]).map_err(storage::bad_request)?;
    let touch_sha = sha(&contacts["sha256"]).map_err(storage::bad_request)?;
    if contacts["source_replay_sha256"].as_str() != Some(replay_sha.as_str()) {
        return Err(storage::bad_request(
            "Touch proposals were built from another CPU track version",
        ));
    }
    let tracked_file = safe_file(&track_dir, "tracked_frames.jsonl", MAX_EVIDENCE)
        .map_err(storage::bad_request)?;
    let touch_file = safe_file(&touch_dir, "touch_candidates.jsonl", MAX_EVIDENCE)
        .map_err(storage::bad_request)?;
    let output_root = project_dir.join("native_cpu_rallies");
    match fs::symlink_metadata(&output_root) {
        Ok(m) if m.file_type().is_symlink() || !m.is_dir() => {
            return Err(storage::bad_request("Unsafe native rally output directory"))
        }
        Ok(_) => (),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => fs::create_dir(&output_root)
            .map_err(|e| storage::internal(format!("Create native rally storage: {e}")))?,
        Err(e) => {
            return Err(storage::internal(format!(
                "Check native rally storage: {e}"
            )))
        }
    }
    let run_id = Uuid::new_v4().simple().to_string();
    let folder = output_root.join(&run_id);
    fs::create_dir(&folder)
        .map_err(|e| storage::internal(format!("Create isolated rally snapshot: {e}")))?;
    let result = (|| -> Result<Value, String> {
        let part = folder.join("rally_candidates.jsonl.partial");
        let (output_sha, stats, output) = rebuild_from_snapshots(
            &tracked_file,
            &touch_file,
            &replay_sha,
            &touch_sha,
            frames,
            expected_touches,
            dimensions.width,
            dimensions.height,
            settings,
            &part,
            &request.replay_id,
            &request.touch_id,
        )?;
        if inference_stream::source_stamp(prepared)? != (size, mtime) {
            return Err("Prepared video changed during native rally processing".into());
        }
        fs::rename(part, folder.join(OUTPUT_FILE))
            .map_err(|e| format!("Publish rally candidates: {e}"))?;
        let manifest = json!({
            "schema":"volley-native-cpu-rally-proposals/1",
            "state":"provisional_rally_segments",
            "project_id":request.project_id,
            "video":request.video,
            "rally_run_id":run_id,
            "source_replay_id":request.replay_id,
            "source_replay_sha256":replay_sha,
            "source_touch_id":request.touch_id,
            "source_touch_sha256":touch_sha,
            "source_identity":identity,
            "width":dimensions.width,"height":dimensions.height,"fps":30,
            "frames":frames,
            "settings":settings,
            "touch_candidates_ingested":output.ingested_touch_candidates,
            "rally_candidates":output.proposals,
            "diagnostics":stats,
            "file":OUTPUT_FILE,
            "sha256":output_sha,
            "rallies_authoritatively_confirmed":false,
            "athlete_identities_verified":false,
            "action_classification_complete":false,
            "montage_ready":false,
            "cpu_processing_complete":false,
        });
        storage::write_json_atomic(&folder.join("manifest.json"), &manifest)
            .map_err(|e| format!("Save native rally manifest: {e:?}"))?;
        Ok(manifest)
    })();
    match result {
        Ok(v) => Ok(v),
        Err(e) => {
            let _ = fs::remove_dir_all(&folder);
            Err(storage::bad_request(e))
        }
    }
}

pub async fn run(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<RallyRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    let result = tokio::task::spawn_blocking(move || process(&state, &request))
        .await
        .map_err(|e| storage::internal(format!("CPU rally task: {e}")))??;
    Ok(Json(result))
}
pub async fn status(
    State(state): State<Arc<AppState>>,
    Query(query): Query<RallyQuery>,
) -> ApiResult<Value> {
    storage::project_id(&query.project_id)?;
    if !opaque_id(&query.rally_run_id) {
        return Err(storage::bad_request("Invalid native rally run ID"));
    }
    projects::existing_project(&state, &query.project_id)?;
    let project_dir = projects::project_root(&state).join(&query.project_id);
    safe_folder(&project_dir).map_err(storage::bad_request)?;
    let root = project_dir.join("native_cpu_rallies");
    safe_folder(&root).map_err(storage::bad_request)?;
    let dir = root.join(&query.rally_run_id);
    let manifest = source_manifest(&dir, "native rally").map_err(storage::bad_request)?;
    if manifest["schema"] != "volley-native-cpu-rally-proposals/1"
        || manifest["project_id"].as_str() != Some(&query.project_id)
        || manifest["rally_run_id"].as_str() != Some(&query.rally_run_id)
    {
        return Err(storage::bad_request(
            "Rally manifest belongs to another project",
        ));
    }
    Ok(Json(manifest))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(
        frames: u64,
        cut: Option<(u64, u64)>,
        touches: &[u64],
    ) -> (Vec<u8>, Vec<u8>, String, String) {
        let mut tracked = Vec::new();
        for frame in 0..frames {
            let ball = if cut.is_some_and(|(begin, end)| frame >= begin && frame <= end) {
                Value::Null
            } else {
                let t = frame % 200;
                let x = 0.10 + (t.min(200 - t) as f64 * 0.005);
                json!({
                    "source_model":"vballnet",
                    "detection":{
                        "frame_index":frame,"class_name":"ball","score":0.92,
                        "bbox_xyxy_norm":[x,0.36,x+0.02,0.39]
                    }
                })
            };
            let row = json!({
                "frame_index":frame,
                "selected_ball":ball,
                "track_ids_provisional":true,
                "confirmed_athlete_identities":false,
                "touches_confirmed":false,
            });
            tracked.extend(serde_json::to_vec(&row).unwrap());
            tracked.push(b'\n');
        }
        let mut contacts = Vec::new();
        for &frame in touches {
            let event = json!({
                "frame_index":frame,
                "provisional_track_id":3,
                "probability":0.94,
                "ball_x_px":150.0,"ball_y_px":120.0,
                "motion_frame_index":frame,
                "direction_change_deg":90.0,
                "speed_before_px_frame":4.0,
                "speed_after_px_frame":5.0,
                "velocity_residual_px_frame":10.0,
                "technique":"unknown_touch",
                "physical_motion_supported":true,
                "athlete_identity_verified":false,
            });
            contacts.extend(serde_json::to_vec(&event).unwrap());
            contacts.push(b'\n');
        }
        let track_sha = format!("{:x}", Sha256::digest(&tracked));
        let touch_sha = format!("{:x}", Sha256::digest(&contacts));
        (tracked, contacts, track_sha, touch_sha)
    }
    fn verify(
        tracks: &[u8],
        touches: &[u8],
        track_sha: &str,
        touch_sha: &str,
        frames: u64,
        contact_count: u64,
        settings: RallySettings,
    ) -> Result<(Value, Vec<Value>), String> {
        let root = std::env::temp_dir().join(format!("native-rally-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let frames_file = root.join("tracked.jsonl");
        let touches_file = root.join("contacts.jsonl");
        let output = root.join("out.jsonl");
        fs::write(&frames_file, tracks).unwrap();
        fs::write(&touches_file, touches).unwrap();
        let result = rebuild_from_snapshots(
            &frames_file,
            &touches_file,
            track_sha,
            touch_sha,
            frames,
            contact_count,
            640,
            360,
            settings,
            &output,
            "0123456789abcdef0123456789abcdef",
            "abcdef0123456789abcdef0123456789",
        );
        let response = result.map(|(sha, stats, counts)| {
            let bytes = fs::read(&output).unwrap();
            assert_eq!(sha, format!("{:x}", Sha256::digest(&bytes)));
            let rows = bytes
                .split(|b| *b == b'\n')
                .filter(|row| !row.is_empty())
                .map(|row| serde_json::from_slice::<Value>(row).unwrap())
                .collect::<Vec<_>>();
            (json!({"proposals":counts.proposals,"stats":stats}), rows)
        });
        fs::remove_dir_all(root).unwrap();
        response
    }
    #[test]
    fn two_ball_segments_and_two_pairs_of_motion_touches_make_two_proposals() {
        let (frames, touches, fh, th) = fixture(450, Some((160, 289)), &[30, 80, 310, 370]);
        let (counts, rows) = verify(
            &frames,
            &touches,
            &fh,
            &th,
            450,
            4,
            RallySettings::default(),
        )
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(counts["proposals"], 2);
        assert!(rows[0]["detected_end_frame"].as_u64().unwrap() < 160);
        assert_eq!(rows[1]["detected_start_frame"], 290);
        assert_eq!(rows[0]["motion_supported_touch_proposals"], 2);
        assert_eq!(rows[0]["classification"], "provisional_rally");
        assert_eq!(rows[0]["authoritative_rally"], false);
        assert_eq!(rows[0]["provisional_track_ids"][0], 3);
        assert_ne!(rows[0]["candidate_id"], rows[1]["candidate_id"]);
    }
    #[test]
    fn rescoring_requires_no_gpu_and_single_contact_is_not_a_rally() {
        let (frames, touches, fh, th) = fixture(140, None, &[35]);
        let (_summary, rows) = verify(
            &frames,
            &touches,
            &fh,
            &th,
            140,
            1,
            RallySettings::default(),
        )
        .unwrap();
        assert!(rows.is_empty());
        let relaxed = RallySettings {
            min_touch_candidates: 1,
            ..RallySettings::default()
        };
        let (_, rows) = verify(&frames, &touches, &fh, &th, 140, 1, relaxed).unwrap();
        assert_eq!(rows.len(), 1);
    }
    #[test]
    fn refuses_hash_corruption_out_of_order_touch_and_undeclared_rows() {
        let (frames, touches, fh, th) = fixture(140, None, &[35, 90]);
        assert!(verify(
            &frames,
            &touches,
            &fh,
            &"0".repeat(64),
            140,
            2,
            RallySettings::default()
        )
        .is_err());
        assert!(verify(
            &frames,
            &touches,
            &fh,
            &th,
            140,
            3,
            RallySettings::default()
        )
        .is_err());
        let mut extra = touches.clone();
        extra.extend_from_slice(&touches);
        let newsha = format!("{:x}", Sha256::digest(&extra));
        assert!(verify(
            &frames,
            &extra,
            &fh,
            &newsha,
            140,
            4,
            RallySettings::default()
        )
        .is_err());
        let mut bad = frames.clone();
        bad[0] = b' ';
        assert!(verify(&bad, &touches, &fh, &th, 140, 2, RallySettings::default()).is_err());
        let mut truncated = frames.clone();
        truncated.truncate(frames.len() / 2);
        assert!(verify(
            &truncated,
            &touches,
            &fh,
            &th,
            140,
            2,
            RallySettings::default()
        )
        .is_err());
    }
}
