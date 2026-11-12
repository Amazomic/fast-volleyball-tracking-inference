//! Re-evaluate motion-supported Touch candidates exclusively from immutable
//! native CPU Track IDs and selected-ball proposals, never rerunning ONNX.
//! These events are not yet authoritative game/rally/athlete statistics.
use crate::{
    cpu_retrack::{opaque_id, safe_file, safe_folder, BoundedReader},
    inference_stream, native_preview_pose, preparation, project_guard, projects,
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
    collections::VecDeque,
    fs::{self, OpenOptions},
    io::{BufWriter, Write},
    path::Path,
    sync::Arc,
};
use uuid::Uuid;
use volley_core::{
    fusion::Candidate,
    pose_classification::{self, PoseEvidence},
    touch_pipeline::{
        evaluate_block, PhysicalTouchCandidate, TouchBall, TouchFrame, TouchPlayer, TouchSettings,
    },
};

const MAX_INPUT_BYTES: u64 = 64 * 1024 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 16384;
const CHUNK: usize = 240;
const HALO: usize = 40;
const OUTPUT_FILE: &str = "touch_candidates.jsonl";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TouchRequest {
    pub(crate) project_id: String,
    pub(crate) video: String,
    pub(crate) replay_id: String,
    #[serde(default)]
    pub(crate) settings: Option<TouchSettings>,
    #[serde(default)]
    pub(crate) pose_classification: bool,
}

#[derive(Debug, Deserialize)]
pub struct TouchQuery {
    project_id: String,
    touch_id: String,
}

#[derive(Deserialize)]
struct TrackedCandidate {
    track_id: u64,
    detection: Candidate,
    is_verified_athlete: bool,
}
#[derive(Deserialize)]
struct SelectedBall {
    source_model: String,
    detection: Candidate,
}
#[derive(Deserialize)]
struct TrackedFrame {
    frame_index: u64,
    tracked_players: Vec<TrackedCandidate>,
    selected_ball: Option<SelectedBall>,
    track_ids_provisional: bool,
    confirmed_athlete_identities: bool,
    touches_confirmed: bool,
}

fn transform_frame(item: TrackedFrame, expected: u64) -> Result<TouchFrame, String> {
    if item.frame_index != expected
        || !item.track_ids_provisional
        || item.confirmed_athlete_identities
        || item.touches_confirmed
        || item.tracked_players.len() > 128
    {
        return Err("CPU source has incorrect frame index or verified status".into());
    }
    let mut players = Vec::with_capacity(item.tracked_players.len());
    for player in item.tracked_players {
        let sample = &player.detection;
        if player.is_verified_athlete
            || player.track_id == 0
            || sample.frame_index != expected
            || sample.class_name != "player"
            || sample.query_index.is_none()
            || sample.score <= 0.0
        {
            return Err("CPU source contains fabricated athlete identity or invalid track".into());
        }
        sample.validate(expected - (expected % 9), "players")?;
        players.push(TouchPlayer {
            track_id: player.track_id,
            score: sample.score,
            bbox_xyxy_norm: sample.bbox_xyxy_norm,
            touch_probability: sample.touch_probability,
        });
    }
    let selected_ball = item
        .selected_ball
        .map(|ball| -> Result<TouchBall, String> {
            if !matches!(ball.source_model.as_str(), "vballnet" | "ravel")
                || ball.detection.frame_index != expected
                || ball.detection.class_name != "ball"
            {
                return Err("Invalid source for selected ball".into());
            }
            let kind = if ball.source_model == "ravel" {
                "players"
            } else {
                "ball"
            };
            ball.detection.validate(expected - (expected % 9), kind)?;
            Ok(TouchBall {
                source_model: ball.source_model,
                score: ball.detection.score,
                bbox_xyxy_norm: ball.detection.bbox_xyxy_norm,
            })
        })
        .transpose()?;
    Ok(TouchFrame {
        frame_index: expected,
        players,
        selected_ball,
    })
}

fn emit_record(
    writer: &mut BufWriter<std::fs::File>,
    hash: &mut Sha256,
    event: &PhysicalTouchCandidate,
) -> Result<(), String> {
    let mut bytes =
        serde_json::to_vec(event).map_err(|e| format!("Serialize motion-backed touch: {e}"))?;
    bytes.push(b'\n');
    writer
        .write_all(&bytes)
        .map_err(|e| format!("Write CPU touch: {e}"))?;
    hash.update(&bytes);
    Ok(())
}

/// Pose evidence comes from the same immutable fusion as the replay.
/// Missing/low-confidence landmarks never override the old geometry hypothesis.
fn refine_pose_events(
    events: &mut [PhysicalTouchCandidate],
    frames: &[TouchFrame],
    video_width: u32,
    video_height: u32,
    project_root: &Path,
    fusion_id: &str,
) -> Result<(), String> {
    if events.is_empty() || frames.is_empty() { return Ok(()); }
    let first=frames[0].frame_index;
    let last=frames.last().ok_or("Empty pose source frames")?.frame_index;
    let placeholders=events.iter().map(|event| {
        let player=event.provisional_track_id.and_then(|id| {
            let frame=frames.get(event.frame_index.checked_sub(first)? as usize)?;
            frame.players.iter().find(|player|player.track_id==id).map(|p| {
                json!({"id":id,"box":p.bbox_xyxy_norm})
            })
        });
        json!({"frame":event.frame_index,"players":player.into_iter().collect::<Vec<_>>()})
    }).collect::<Vec<Value>>();
    let mut matches=placeholders;
    if !native_preview_pose::enrich(project_root,fusion_id,first,last+1,&mut matches)? {
        return Ok(());
    }
    for (event,frame) in events.iter_mut().zip(matches) {
        let Some(geometry)=event.action_prediction.as_ref() else{continue};
        let Some(point)=frame["players"].as_array().and_then(|players|players.first()) else{continue};
        let Some(pose)=point.get("pose") else{continue};
        let Ok(keypoints)=serde_json::from_value::<[[f64;3];17]>(pose.clone()) else{continue};
        let Some(id)=event.provisional_track_id else{continue};
        let Some(source)=frames.get(event.frame_index.saturating_sub(first) as usize) else{continue};
        let Some(player)=source.players.iter().find(|p|p.track_id==id) else{continue};
        let evidence=PoseEvidence {
            keypoints,
            ball_x_norm:event.ball_x_px/video_width as f64,
            ball_y_norm:event.ball_y_px/video_height as f64,
            video_width:video_width as f64,
            video_height:video_height as f64,
            player_bbox_norm:player.bbox_xyxy_norm,
        };
        if let Some(refined)=pose_classification::refine(&evidence,geometry) {
            event.action_prediction=Some(refined);
        }
    }
    Ok(())
}

/// Bounded 240-frame chunk, 40-frame halos for 5-frame smoothing, motion
/// window and time-cluster NMS. A single pending event deduplicates across
/// chunk seams. Source is hashed in the same pass. Whole matches never
/// require their frame records or raw RAVEL boxes to be resident in memory.
#[cfg(test)]
pub(crate) fn detect_stream(
    source: &Path,
    output: &Path,
    expected_sha: &str,
    count: u64,
    width: u32,
    height: u32,
    settings: TouchSettings,
) -> Result<(String, u64, u64), String> {
    detect_stream_with_pose(source,output,expected_sha,count,width,height,settings,None)
}

#[expect(clippy::too_many_arguments)]
fn detect_stream_with_pose(
    source: &Path,
    output: &Path,
    expected_sha: &str,
    count: u64,
    width: u32,
    height: u32,
    settings: TouchSettings,
    pose_source: Option<(&Path, &str)>,
) -> Result<(String, u64, u64), String> {
    settings.validate()?;
    if count == 0 || count > 48 * 3600 * 30 {
        return Err("Touch source frame count exceeds limit".into());
    }
    let mut reader = BoundedReader::open(source)?;
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .map_err(|e| format!("Create isolated touch output: {e}"))?;
    let mut writer = BufWriter::new(file);
    let mut output_hash = Sha256::new();
    let mut buffer: VecDeque<TouchFrame> = VecDeque::new();
    let mut next_emit: u64 = 0;
    let mut emitted: u64 = 0;
    let mut motion_candidates: u64 = 0;
    let mut pending: Option<PhysicalTouchCandidate> = None;
    let separation = (settings.min_interval_sec * 30.0).round().max(1.0) as u64;
    let mut process_block =
        |start: u64, end: u64, input: &VecDeque<TouchFrame>| -> Result<(), String> {
            let region_start = start.saturating_sub(HALO as u64);
            let region_end = (end + HALO as u64).min(count);
            let first = input
                .front()
                .ok_or("Touch frame buffer is empty")?
                .frame_index;
            if region_start < first
                || region_end
                    > input
                        .back()
                        .ok_or("Touch frame buffer truncated")?
                        .frame_index
                        + 1
            {
                return Err("Touch segment missing lookbehind or lookahead".into());
            }
            let offset = (region_start - first) as usize;
            let n = (region_end - region_start) as usize;
            let block = input
                .iter()
                .skip(offset)
                .take(n)
                .cloned()
                .collect::<Vec<_>>();
            let mut events = evaluate_block(&block, width, height, start, end, settings)?;
            if let Some((root,fusion))=pose_source {
                refine_pose_events(&mut events, &block, width, height, root, fusion)?;
            }
            for event in events {
                motion_candidates += 1;
                if let Some(ref last) = pending {
                    if event.frame_index.saturating_sub(last.frame_index) <= separation {
                        if event.probability > last.probability {
                            pending = Some(event);
                        }
                        continue;
                    }
                }
                if let Some(last) = pending.take() {
                    emit_record(&mut writer, &mut output_hash, &last)?;
                    emitted += 1;
                }
                pending = Some(event);
            }
            Ok(())
        };
    for index in 0..count {
        let bytes = reader.line()?.ok_or("CPU tracked frame JSONL truncated")?;
        let raw: TrackedFrame = serde_json::from_slice(&bytes)
            .map_err(|e| format!("Invalid CPU tracked frame: {e}"))?;
        buffer.push_back(transform_frame(raw, index)?);
        while next_emit + (CHUNK + HALO) as u64 <= index + 1 {
            let end = next_emit + CHUNK as u64;
            process_block(next_emit, end, &buffer)?;
            next_emit = end;
            while buffer
                .front()
                .is_some_and(|row| row.frame_index < next_emit.saturating_sub(HALO as u64))
            {
                buffer.pop_front();
            }
        }
    }
    while next_emit < count {
        let end = (next_emit + CHUNK as u64).min(count);
        process_block(next_emit, end, &buffer)?;
        next_emit = end;
        while buffer
            .front()
            .is_some_and(|row| row.frame_index < next_emit.saturating_sub(HALO as u64))
        {
            buffer.pop_front();
        }
    }
    if reader.line()?.is_some() {
        return Err("CPU replay has extra undeclared frames".into());
    }
    if reader.finish() != expected_sha {
        return Err("CPU replay SHA-256 changed while reading Touch input".into());
    }
    if let Some(last) = pending {
        emit_record(&mut writer, &mut output_hash, &last)?;
        emitted += 1;
    }
    writer
        .flush()
        .map_err(|e| format!("Flush physical touch candidates: {e}"))?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|e| format!("Sync physical touch candidates: {e}"))?;
    Ok((
        format!("{:x}", output_hash.finalize()),
        emitted,
        motion_candidates,
    ))
}

pub(crate) fn process(state: &AppState, req: &TouchRequest) -> Result<Value, ApiError> {
    if !state.writes_enabled {
        return Err(storage::conflict(
            "Touch recalculation requires an isolated working directory",
        ));
    }
    storage::project_id(&req.project_id)?;
    storage::video_name(&req.video)?;
    if !opaque_id(&req.replay_id) {
        return Err(storage::bad_request("Invalid CPU track replay ID"));
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
        .is_some_and(|v| v.iter().any(|name| name.as_str() == Some(&req.video)))
    {
        return Err(storage::bad_request("Video not present in project"));
    }
    let plan = preparation::plan_for(state, &req.project_id, &req.video)?;
    let prepared = plan
        .ready_path()
        .ok_or_else(|| storage::bad_request("Prepared video unavailable"))?;
    let (size, mtime) = inference_stream::source_stamp(prepared).map_err(storage::bad_request)?;
    let dimensions = volley_media::probe(prepared).map_err(storage::bad_request)?;
    if !(64..=16384).contains(&dimensions.width)
        || !(64..=16384).contains(&dimensions.height)
        || plan.identity.fps != 30
    {
        return Err(storage::bad_request(
            "CPU Touch requires 30-fps prepared media dimensions",
        ));
    }
    let project_dir = projects::project_root(state).join(&req.project_id);
    safe_folder(&project_dir).map_err(storage::bad_request)?;
    let replay_root = project_dir.join("native_cpu_replays");
    safe_folder(&replay_root).map_err(storage::bad_request)?;
    let replay_dir = replay_root.join(&req.replay_id);
    safe_folder(&replay_dir).map_err(storage::bad_request)?;
    let manifest_file = safe_file(&replay_dir, "manifest.json", MAX_MANIFEST_BYTES)
        .map_err(storage::bad_request)?;
    let manifest: Value = serde_json::from_slice(
        &fs::read(manifest_file)
            .map_err(|e| storage::internal(format!("Read CPU tracking manifest: {e}")))?,
    )
    .map_err(|e| storage::bad_request(format!("Invalid tracking manifest: {e}")))?;
    if manifest["schema"] != "volley-native-cpu-tracking/1"
        || manifest["state"] != "provisional_tracklets_rebuilt"
        || manifest["project_id"].as_str() != Some(&req.project_id)
        || manifest["replay_id"].as_str() != Some(&req.replay_id)
        || manifest["video"].as_str() != Some(&req.video)
        || manifest["fps"] != 30
        || manifest["file"] != "tracked_frames.jsonl"
        || manifest["track_ids_provisional"] != true
        || manifest["athlete_identities_confirmed"] != false
        || manifest["touches_confirmed"] != false
        || manifest["cpu_processing_complete"] != false
        || manifest["source_identity"] != json!({"bytes":size,"mtime_ns":mtime})
    {
        return Err(storage::bad_request(
            "CPU tracked frames are incompatible with source video",
        ));
    }
    let frames = manifest["frames"]
        .as_u64()
        .filter(|n| *n > 0 && *n <= 48 * 3600 * 30)
        .ok_or_else(|| storage::bad_request("Invalid CPU tracked frame count"))?;
    let sha = manifest["sha256"]
        .as_str()
        .filter(|s| {
            s.len() == 64
                && s.bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        })
        .ok_or_else(|| storage::bad_request("Tracked-frame checksum missing"))?
        .to_owned();
    let source = safe_file(&replay_dir, "tracked_frames.jsonl", MAX_INPUT_BYTES)
        .map_err(storage::bad_request)?;
    let fusion_id = manifest["source_fusion_id"].as_str()
        .filter(|id|opaque_id(id)).ok_or_else(||
            storage::bad_request("Touch replay has no valid GPU fusion lineage"))?
        .to_owned();
    let output_root = project_dir.join("native_cpu_touch_runs");
    match fs::symlink_metadata(&output_root) {
        Ok(meta) if !meta.is_dir() || meta.file_type().is_symlink() => {
            return Err(storage::bad_request("Unsafe native touch run root"))
        }
        Ok(_) => (),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => fs::create_dir(&output_root)
            .map_err(|e| storage::internal(format!("Create native Touch runs folder: {e}")))?,
        Err(e) => {
            return Err(storage::internal(format!(
                "Check native Touch runs root: {e}"
            )))
        }
    }
    let touch_id = Uuid::new_v4().simple().to_string();
    let folder = output_root.join(&touch_id);
    fs::create_dir(&folder)
        .map_err(|e| storage::internal(format!("Create Touch run folder: {e}")))?;
    let result = (|| -> Result<Value, String> {
        let part = folder.join("touch_candidates.jsonl.partial");
        let (digest, events, raw) = detect_stream_with_pose(
            &source,
            &part,
            &sha,
            frames,
            dimensions.width,
            dimensions.height,
            settings,
            req.pose_classification.then_some((project_dir.as_path(), fusion_id.as_str())),
        )?;
        if inference_stream::source_stamp(prepared)? != (size, mtime) {
            return Err("Prepared source changed during CPU Touch replay".into());
        }
        fs::rename(part, folder.join(OUTPUT_FILE))
            .map_err(|e| format!("Publish motion-gated Touch events: {e}"))?;
        let record = json!({
            "schema":"volley-native-cpu-touch-proposals/1",
            "state":"motion_gated_touch_proposals",
            "project_id":req.project_id,
            "video":req.video,
            "touch_id":touch_id,
            "source_replay_id":req.replay_id,
            "source_replay_sha256":sha,
            "source_identity":{"bytes":size,"mtime_ns":mtime},
            "width":dimensions.width,"height":dimensions.height,"fps":30,
            "frames":frames,
            "settings":settings,
            "motion_supported_proposals_before_dedup":raw,
            "touch_candidates":events,
            "method":"RAVEL Touch peaks + motion-backed geometry + optional RTMPose17 refinement",
            "pose_classification_requested":req.pose_classification,
            "pose_source_fusion_id": if req.pose_classification {json!(fusion_id)} else {Value::Null},
            "ball_source_switch_guard":true,
            "file":OUTPUT_FILE,
            "sha256":digest,
            "contacts_authoritatively_confirmed":false,
            "athlete_identities_verified":false,
            "action_technique_classification_complete":false,
            "automatic_action_predictions_enabled":true,
            "automatic_action_prediction_method":if req.pose_classification {
                "pose_keypoints_v1_with_geometry_fallback"
            } else { "contact_geometry_v1" },
            "cpu_processing_complete":false,
        });
        storage::write_json_atomic(&folder.join("manifest.json"), &record)
            .map_err(|e| format!("Save native Touch manifest: {e:?}"))?;
        Ok(record)
    })();
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            let _ = fs::remove_dir_all(&folder);
            Err(storage::bad_request(error))
        }
    }
}
pub async fn run(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<TouchRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    let result = tokio::task::spawn_blocking(move || process(&state, &request))
        .await
        .map_err(|e| storage::internal(format!("CPU Touch task failed: {e}")))??;
    Ok(Json(result))
}
pub async fn status(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TouchQuery>,
) -> ApiResult<Value> {
    storage::project_id(&query.project_id)?;
    if !opaque_id(&query.touch_id) {
        return Err(storage::bad_request("Invalid Touch ID"));
    }
    projects::existing_project(&state, &query.project_id)?;
    let project = projects::project_root(&state).join(&query.project_id);
    safe_folder(&project).map_err(storage::bad_request)?;
    let root = project.join("native_cpu_touch_runs");
    safe_folder(&root).map_err(storage::bad_request)?;
    let folder = root.join(&query.touch_id);
    safe_folder(&folder).map_err(storage::bad_request)?;
    let data = fs::read(
        safe_file(&folder, "manifest.json", MAX_MANIFEST_BYTES).map_err(storage::bad_request)?,
    )
    .map_err(|e| storage::internal(format!("Read native Touch metadata: {e}")))?;
    let value: Value = serde_json::from_slice(&data)
        .map_err(|e| storage::bad_request(format!("Corrupt native Touch metadata: {e}")))?;
    if value["schema"] != "volley-native-cpu-touch-proposals/1"
        || value["touch_id"].as_str() != Some(&query.touch_id)
        || value["project_id"].as_str() != Some(&query.project_id)
    {
        return Err(storage::bad_request(
            "Touch manifest belongs to different project",
        ));
    }
    Ok(Json(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saved_pose_refines_motion_touch_without_rerunning_gpu() {
        use volley_core::action_prediction::{ActionKind, ActionPrediction, PredictionMethod};
        let root=std::env::temp_dir().join(format!("touch-pose-{}", Uuid::new_v4()));
        let fusion="a".repeat(32);
        let pose_id="b".repeat(32);
        let fusion_dir=root.join("native_gpu_fusions").join(&fusion);
        let pose_dir=root.join("native_gpu_pose_runs").join(&pose_id);
        fs::create_dir_all(&fusion_dir).unwrap();
        fs::create_dir_all(&pose_dir).unwrap();
        let source_sha="e".repeat(64);
        let identity=json!({"bytes":1234,"mtime_ns":4321});
        fs::write(fusion_dir.join("manifest.json"),serde_json::to_vec(&json!({
            "schema":"volley-native-gpu-fusion/1","fusion_id":fusion,
            "project_id":"abc123def456","video":"video.mp4","sha256":source_sha,
            "source_identity":identity,
        })).unwrap()).unwrap();
        let mut points=[[0.5,0.5,0.0];17];
        points[5]=[0.43,0.44,0.95];points[6]=[0.57,0.44,0.95];
        points[7]=[0.41,0.31,0.96];points[8]=[0.59,0.31,0.96];
        points[9]=[0.49,0.22,0.96];points[10]=[0.52,0.22,0.96];
        let row=format!("{}\n",serde_json::to_string(&json!({"frame":30,
            "players":[{"box":[0.35,0.17,0.65,0.90],"keypoints":points}]})).unwrap());
        fs::write(pose_dir.join("poses.jsonl"),&row).unwrap();
        let digest=format!("{:x}",Sha256::digest(row.as_bytes()));
        fs::write(pose_dir.join("manifest.json"),serde_json::to_vec(&json!({
            "schema":"volley-native-gpu-pose/1","state":"pose_evidence_complete",
            "pose_run_id":pose_id,"fusion_id":fusion,"project_id":"abc123def456",
            "video":"video.mp4","model_sha256":volley_inference::pose::MODEL_SHA256,
            "source_fusion_sha256":source_sha,"source_identity":identity,
            "file":"poses.jsonl","sha256":digest,"stride":6
        })).unwrap()).unwrap();
        let tracked=TouchFrame{
            frame_index:31,
            players:vec![TouchPlayer{
                track_id:8,score:0.90,touch_probability:Some(0.94),
                bbox_xyxy_norm:[0.35,0.17,0.65,0.90],
            }],
            selected_ball:None,
        };
        let mut events=vec![PhysicalTouchCandidate{
            frame_index:31,provisional_track_id:Some(8),probability:0.94,
            ball_x_px:320.0,ball_y_px:72.0,motion_frame_index:31,
            direction_change_deg:80.0,speed_before_px_frame:7.0,
            speed_after_px_frame:13.0,velocity_residual_px_frame:12.0,
            technique:"unknown_touch",action_prediction:Some(ActionPrediction{
                kind:ActionKind::Attack,evidence_score:0.87,
                method:PredictionMethod::ContactGeometryV1,
                depth_ratio:-0.1,posture_ratio:0.4,
            }),physical_motion_supported:true,athlete_identity_verified:false,
        }];
        refine_pose_events(&mut events,std::slice::from_ref(&tracked),640,360,&root,&fusion).unwrap();
        let changed=events[0].action_prediction.as_ref().unwrap();
        assert_eq!(changed.kind,ActionKind::OverheadPass);
        assert_eq!(changed.method,PredictionMethod::PoseKeypointsV1);
        // A different fusion cannot borrow the saved pose from this one.
        refine_pose_events(&mut events,&[tracked],640,360,&root,&"c".repeat(32)).unwrap();
        assert_eq!(events[0].action_prediction.as_ref().unwrap().method,
            PredictionMethod::PoseKeypointsV1);
        let _=fs::remove_dir_all(root);
    }

    fn sample_frames(curve: bool, impact: u64) -> (Vec<u8>, String) {
        let mut bytes = Vec::new();
        for i in 0..630_u64 {
            let x = if i < impact - 40 {
                0.20
            } else if curve && i >= impact {
                0.50 - (i - impact).min(40) as f64 * 0.0075
            } else {
                0.20 + (i - (impact - 40)).min(80) as f64 * 0.0075
            };
            let y = 0.36;
            let candidate = Candidate {
                frame_index: i,
                class_name: "player".into(),
                score: 0.95,
                bbox_xyxy_norm: [0.27, 0.15, 0.47, 0.65],
                query_index: Some(0),
                touch_probability: Some(if (impact - 3..=impact + 3).contains(&i) {
                    0.97
                } else {
                    0.01
                }),
                head_point_norm: None,
            };
            let ball = Candidate {
                frame_index: i,
                class_name: "ball".into(),
                score: 0.92,
                bbox_xyxy_norm: [x, y, x + 0.02, y + 0.03],
                query_index: None,
                touch_probability: None,
                head_point_norm: None,
            };
            let row = json!({
                "frame_index":i,
                "tracked_players":[{"track_id":1,"detection":candidate,"is_verified_athlete":false}],
                "selected_ball":{"source_model":"vballnet","detection":ball},
                "track_ids_provisional":true,"confirmed_athlete_identities":false,
                "touches_confirmed":false,
            });
            bytes.extend(serde_json::to_vec(&row).unwrap());
            bytes.push(b'\n');
        }
        let sha = format!("{:x}", Sha256::digest(&bytes));
        (bytes, sha)
    }
    #[test]
    fn chunked_replay_verifies_sha_and_does_not_invent_straight_flight_touch() {
        let root = std::env::temp_dir().join(format!("touch-replay-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("source.jsonl");
        let (bytes, sha) = sample_frames(true, 320);
        fs::write(&source, bytes).unwrap();
        let (result_sha, events, raw) = detect_stream(
            &source,
            &root.join("out.jsonl"),
            &sha,
            630,
            640,
            360,
            TouchSettings::default(),
        )
        .unwrap();
        assert!(events <= raw);
        assert!(events >= 1, "Expected synthetic reversal candidate");
        let written = fs::read(root.join("out.jsonl")).unwrap();
        assert_eq!(result_sha, format!("{:x}", Sha256::digest(&written)));
        let (straight, straight_hash) = sample_frames(false, 320);
        fs::write(&source, straight).unwrap();
        let (_digest, straight_events, _) = detect_stream(
            &source,
            &root.join("straight.jsonl"),
            &straight_hash,
            630,
            640,
            360,
            TouchSettings::default(),
        )
        .unwrap();
        assert_eq!(straight_events, 0);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn touch_crosses_chunk_boundaries_once_and_cpu_threshold_replays() {
        let root = std::env::temp_dir().join(format!("touch-seams-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        for impact in [240_u64, 479_u64] {
            let (bytes, sha) = sample_frames(true, impact);
            let source = root.join(format!("source-{impact}.jsonl"));
            fs::write(&source, bytes).unwrap();
            let path = root.join(format!("default-{impact}.jsonl"));
            let (_, events, _) = detect_stream(
                &source,
                &path,
                &sha,
                630,
                640,
                360,
                TouchSettings::default(),
            )
            .unwrap();
            assert_eq!(
                events, 1,
                "Touch should appear exactly once near chunk border {impact}"
            );
            let rows = fs::read_to_string(path).unwrap();
            let parsed = rows
                .lines()
                .map(|s| serde_json::from_str::<Value>(s).unwrap())
                .collect::<Vec<_>>();
            assert_eq!(parsed.len(), 1);
            assert!(parsed[0]["frame_index"].as_u64().unwrap().abs_diff(impact) <= 4);
            let strict = TouchSettings {
                threshold: 0.99,
                ..TouchSettings::default()
            };
            let (_, strict_events, _) = detect_stream(
                &source,
                &root.join(format!("strict-{impact}.jsonl")),
                &sha,
                630,
                640,
                360,
                strict,
            )
            .unwrap();
            assert_eq!(
                strict_events, 0,
                "A CPU threshold rerun must not reuse outdated contact decisions"
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_checksum_or_truncated_rows_rejected() {
        let root = std::env::temp_dir().join(format!("touch-bad-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let (bytes, sha) = sample_frames(true, 320);
        let source = root.join("raw.jsonl");
        fs::write(&source, &bytes).unwrap();
        let err = detect_stream(
            &source,
            &root.join("bad.jsonl"),
            &"0".repeat(64),
            630,
            640,
            360,
            TouchSettings::default(),
        )
        .unwrap_err();
        assert!(err.contains("SHA-256"));
        fs::write(&source, &bytes[..bytes.len() / 2]).unwrap();
        assert!(detect_stream(
            &source,
            &root.join("truncated.jsonl"),
            &sha,
            630,
            640,
            360,
            TouchSettings::default()
        )
        .is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
