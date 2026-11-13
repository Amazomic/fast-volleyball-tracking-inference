//! Re-evaluate motion-supported Touch candidates exclusively from immutable
//! native CPU Track IDs and selected-ball proposals, never rerunning ONNX.
//! These events are not yet authoritative game/rally/athlete statistics.
use crate::{
    court_touch,
    cpu_retrack::{opaque_id, safe_file, safe_folder, BoundedReader},
    inference_stream, native_preview_pose, preparation, project_guard, projects,
    storage::{self, ApiError, ApiResult},
    video_edits, AppState,
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
    court_geometry::CourtTransform,
    fusion::Candidate,
    pose_classification,
    touch_pipeline::{
        evaluate_block, evaluate_pose_block, exclude_ball_teleports, pose_contact_at_frame,
        reconstruct_track_gaps, PhysicalTouchCandidate, PoseFrame, PosePlayer, TouchAlgorithm,
        TouchBall, TouchFrame, TouchPlayer, TouchSettings,
    },
};

const MAX_INPUT_BYTES: u64 = 64 * 1024 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 16384;
const CHUNK: usize = 240;
const HALO: usize = 96;
const OUTPUT_FILE: &str = "touch_candidates.jsonl";

fn pose_frames(root: &Path, fusion: &str, frames: &[TouchFrame]) -> Result<Vec<PoseFrame>, String> {
    let Some(first) = frames.first() else {
        return Ok(Vec::new());
    };
    let Some(last) = frames.last() else {
        return Ok(Vec::new());
    };
    let mut values: Vec<Value> = frames.iter().map(|frame| json!({
        "frame":frame.frame_index,
        "players":frame.players.iter().map(|player| json!({"id":player.track_id,"box":player.bbox_xyxy_norm})).collect::<Vec<_>>()
    })).collect();
    if !native_preview_pose::enrich_exact(
        root,
        fusion,
        first.frame_index,
        last.frame_index + 1,
        &mut values,
    )? {
        return Err("Для выбранного режима необходим сохранённый анализ позы".into());
    }
    Ok(values
        .into_iter()
        .filter_map(|frame| {
            let frame_index = frame["frame"].as_u64()?;
            let players: Vec<PosePlayer> = frame["players"]
                .as_array()?
                .iter()
                .filter_map(|player| {
                    Some(PosePlayer {
                        track_id: player["id"].as_u64()?,
                        keypoints: serde_json::from_value(player.get("pose")?.clone()).ok()?,
                    })
                })
                .collect();
            (!players.is_empty()).then_some(PoseFrame {
                frame_index,
                players,
            })
        })
        .collect())
}

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
        .transpose()?
        .filter(|ball| ball.source_model == "vballnet");
    Ok(TouchFrame {
        frame_index: expected,
        players,
        selected_ball,
    })
}

/// Postprocessing-only confidence gate. Never changes raw GPU/replay artifacts.
fn gate_ball_confidence(frames: &mut [TouchFrame], min_score: f64) {
    for frame in frames {
        if frame
            .selected_ball
            .as_ref()
            .is_some_and(|ball| ball.source_model != "vballnet" || ball.score < min_score)
        {
            frame.selected_ball = None;
        }
    }
}

#[cfg(test)]
mod ball_confidence_tests {
    use super::*;

    #[test]
    fn postprocessing_removes_every_vballnet_observation_below_67_percent() {
        let mut frames = [0.08, 0.51, 0.66, 0.67, 0.81]
            .into_iter()
            .enumerate()
            .map(|(frame, score)| TouchFrame {
                frame_index: frame as u64,
                players: Vec::new(),
                selected_ball: Some(TouchBall {
                    source_model: "vballnet".into(),
                    score,
                    bbox_xyxy_norm: [0.3, 0.3, 0.32, 0.32],
                }),
            })
            .collect::<Vec<_>>();
        let raw_copy = frames.clone();
        gate_ball_confidence(&mut frames, 0.67);
        assert_eq!(
            frames
                .iter()
                .map(|f| f.selected_ball.is_some())
                .collect::<Vec<_>>(),
            vec![false, false, false, true, true]
        );
        assert!(raw_copy.iter().all(|f| f.selected_ball.is_some()));
    }
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

/// RTMPose is read from the same saved GPU fusion as the selected CPU replay.
/// A single-frame pose overlap cannot promote a candidate to a real Touch:
/// require same-Track-ID body intersection before/at/after a motion-gated hit.
/// When pose is optional, legacy results remain *unverified proposals*.
fn refine_pose_events(
    events: &mut Vec<PhysicalTouchCandidate>,
    frames: &[TouchFrame],
    video_width: u32,
    video_height: u32,
    project_root: &Path,
    fusion_id: &str,
    settings: TouchSettings,
) -> Result<(), String> {
    if events.is_empty() || frames.is_empty() {
        return Ok(());
    }
    if native_preview_pose::source_info(project_root, fusion_id)?.is_none() {
        if settings.require_pose {
            return Err("Для выбранного режима необходим сохранённый анализ позы".into());
        }
        return Ok(());
    }
    let poses = pose_frames(project_root, fusion_id, frames)?;
    let first = frames[0].frame_index;
    let mut supported = vec![false; events.len()];
    for (index, event) in events.iter_mut().enumerate() {
        if !event.physical_motion_supported {
            continue;
        }
        let Some(track_id) = event.provisional_track_id else {
            continue;
        };
        let Some(frame) = event
            .frame_index
            .checked_sub(first)
            .and_then(|offset| frames.get(usize::try_from(offset).ok()?))
            .filter(|frame| frame.frame_index == event.frame_index)
        else {
            continue;
        };
        let Some(ball) = frame
            .selected_ball
            .as_ref()
            .filter(|ball| ball.source_model == "vballnet")
        else {
            continue;
        };
        let w = video_width as f64;
        let h = video_height as f64;
        let [x1, y1, x2, y2] = ball.bbox_xyxy_norm;
        let center = [(x1 + x2) * 0.5 * w, (y1 + y2) * 0.5 * h];
        let radius = ((x2 - x1) * w + (y2 - y1) * h) * 0.25;
        // A stored event may not borrow a different ball's location.
        if (event.ball_x_px - center[0]).hypot(event.ball_y_px - center[1]) > radius.max(2.0) {
            continue;
        }
        let Some((evidence, _intersection)) = pose_contact_at_frame(
            frames,
            &poses,
            frame,
            track_id,
            video_width,
            video_height,
            settings,
        ) else {
            continue;
        };
        supported[index] = true;
        event.body_contact_supported = true;
        if let Some(geometry) = event.action_prediction.as_ref() {
            if let Some(refined) = pose_classification::refine(&evidence, geometry) {
                event.action_prediction = Some(refined);
            }
        }
    }
    // Preserve unconfirmed, motion-supported proposals when RTMPose is sparse or occluded.
    // Strict mode still requires an independently confirmed same-player body contact.
    if settings.require_pose || settings.algorithm == TouchAlgorithm::PoseMotion {
        let mut index = 0;
        events.retain(|_| {
            let keep = supported[index];
            index += 1;
            keep
        });
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
    detect_stream_with_pose(
        source,
        output,
        expected_sha,
        count,
        width,
        height,
        settings,
        None,
        None,
    )
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
    court: Option<&CourtTransform>,
) -> Result<(String, u64, u64), String> {
    settings.validate()?;
    if count == 0 || count > 48 * 3600 * 30 {
        return Err("Touch source frame count exceeds limit".into());
    }
    if settings.require_pose {
        let (root, fusion) =
            pose_source.ok_or("Для выбранного режима необходим сохранённый анализ позы")?;
        if !native_preview_pose::enrich(root, fusion, 0, 0, &mut [])? {
            return Err("Для выбранного режима необходим сохранённый анализ позы".into());
        }
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
            let mut block = input
                .iter()
                .skip(offset)
                .take(n)
                .cloned()
                .collect::<Vec<_>>();
            // Filter weak VBallNet proposals only in this derived CPU working
            // copy; raw GPU and saved CPU tracking snapshots remain immutable.
            gate_ball_confidence(&mut block, settings.min_ball_score);
            // A short false localization must not hide several neighboring impacts.
            // Only the bounded working copy is cleaned; the hashed CPU replay remains immutable.
            let clean = exclude_ball_teleports(&block, width, height);
            // Reconstruct bounded positions of existing tracked skeletons only.
            // The immutable replay and original GPU player detections remain unchanged.
            let continuous = reconstruct_track_gaps(&clean);
            let filtered = if let Some(calibration) = court {
                court_touch::filter_on_court(&continuous, calibration)
            } else {
                continuous
            };
            let evidence_frames: &[TouchFrame] = &filtered;
            let mut events = if settings.algorithm == TouchAlgorithm::PoseMotion {
                let (root, fusion) =
                    pose_source.ok_or("Для выбранного режима необходим сохранённый анализ позы")?;
                let poses = pose_frames(root, fusion, evidence_frames)?;
                evaluate_pose_block(evidence_frames, &poses, width, height, start, end, settings)?
            } else {
                let mut events =
                    evaluate_block(evidence_frames, width, height, start, end, settings)?;
                if let Some((root, fusion)) = pose_source {
                    refine_pose_events(
                        &mut events,
                        evidence_frames,
                        width,
                        height,
                        root,
                        fusion,
                        settings,
                    )?;
                    // The default legacy detector can miss a motion-supported
                    // wrist contact that RTMPose confirms independently.
                    // Add those source-backed candidates rather than restricting
                    // pose evidence to labels of already-found legacy touches.
                    let poses = pose_frames(root, fusion, evidence_frames)?;
                    events.extend(evaluate_pose_block(
                        evidence_frames, &poses, width, height, start, end, settings,
                    )?);
                    events.sort_by(|a, b| {
                        b.body_contact_supported.cmp(&a.body_contact_supported)
                            .then_with(|| b.velocity_residual_px_frame.total_cmp(&a.velocity_residual_px_frame))
                            .then_with(|| b.probability.total_cmp(&a.probability))
                    });
                    let mut unique = Vec::new();
                    let separation = (settings.min_interval_sec * 30.0).round().max(1.0) as u64;
                    for event in events {
                        if unique.iter().all(|prior: &PhysicalTouchCandidate| {
                            prior.frame_index.abs_diff(event.frame_index) > separation
                        }) {
                            unique.push(event);
                        }
                    }
                    events = unique;
                    events.sort_by_key(|event| event.frame_index);
                }
                events
            };
            if let Some(calibration) = court {
                court_touch::annotate(&mut events, evidence_frames, calibration);
            }
            for event in events {
                // A physical Touch without an exact-frame CPU Track is not publishable.
                if event.provisional_track_id.is_none() {
                    continue;
                }
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
    let court_calibration = video_edits::court_snapshot(state, &req.project_id, &req.video)?;
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
    let fusion_id = manifest["source_fusion_id"]
        .as_str()
        .filter(|id| opaque_id(id))
        .ok_or_else(|| storage::bad_request("Touch replay has no valid GPU fusion lineage"))?
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
        let pose_requested = req.pose_classification
            || settings.require_pose
            || settings.algorithm == TouchAlgorithm::PoseMotion;
        let pose_info = if pose_requested {
            native_preview_pose::source_info(&project_dir, &fusion_id)?
        } else {
            None
        };
        let (digest, events, raw) = detect_stream_with_pose(
            &source,
            &part,
            &sha,
            frames,
            dimensions.width,
            dimensions.height,
            settings,
            (req.pose_classification
                || settings.require_pose
                || settings.algorithm == TouchAlgorithm::PoseMotion)
                .then_some((project_dir.as_path(), fusion_id.as_str())),
            court_calibration.as_ref().map(|value| &value.transform),
        )?;
        if inference_stream::source_stamp(prepared)? != (size, mtime) {
            return Err("Prepared source changed during CPU Touch replay".into());
        }
        if pose_requested
            && native_preview_pose::source_info(&project_dir, &fusion_id)? != pose_info
        {
            return Err("Pose evidence changed during CPU Touch replay".into());
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
            "court_revision":court_calibration.as_ref().map(|c| &c.revision),
            "court_context_enabled":court_calibration.is_some(),
            "motion_supported_proposals_before_dedup":raw,
            "touch_candidates":events,
            "method":if settings.algorithm == TouchAlgorithm::PoseMotion { "vballnet_trajectory_body_contact_v7" } else { "vballnet_gravity_player_geometry_pose_contact_v7" },
            "pose_classification_requested":req.pose_classification || settings.require_pose || settings.algorithm == TouchAlgorithm::PoseMotion,
            "pose_source_fusion_id": if req.pose_classification || settings.require_pose || settings.algorithm == TouchAlgorithm::PoseMotion {json!(fusion_id)} else {Value::Null},
            "pose_source":pose_info.as_ref().map(|(id, sha, stride)| json!({"pose_run_id":id,"sha256":sha,"stride":stride})),
            "body_trajectory_intersection_required":settings.algorithm == TouchAlgorithm::PoseMotion || settings.require_pose,
            "ball_source_switch_guard":true,
            "file":OUTPUT_FILE,
            "sha256":digest,
            "contacts_authoritatively_confirmed":false,
            "athlete_identities_verified":false,
            "action_technique_classification_complete":false,
            "automatic_action_predictions_enabled":true,
            "automatic_action_prediction_method":if pose_requested {
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
    use std::path::PathBuf;

    /// Optional read-only quality gate for the 25-second new2 pilot replay.
    #[test]
    #[ignore = "requires VOLLEY_NEW2_TRACKED_FRAMES pointing at a local 756-frame replay"]
    fn new2_physics_priority_does_not_duplicate_7s_or_17s_touches() {
        let input = PathBuf::from(
            std::env::var("VOLLEY_NEW2_TRACKED_FRAMES").expect("pilot replay path required"),
        );
        let manifest: Value = serde_json::from_slice(
            &fs::read(input.parent().unwrap().join("manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["frames"], 756);
        let sha = manifest["sha256"].as_str().unwrap();
        for use_ravel_touch in [false, true] {
            let output =
                std::env::temp_dir().join(format!("volley-new2-physics-{}.jsonl", Uuid::new_v4()));
            let settings = TouchSettings {
                use_ravel_touch,
                ..TouchSettings::default()
            };
            let run = detect_stream(&input, &output, sha, 756, 1280, 720, settings).unwrap();
            let events: Vec<Value> = fs::read_to_string(&output)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            fs::remove_file(&output).unwrap();
            assert_eq!(run.1 as usize, events.len());
            for (start, end) in [(210, 240), (520, 570), (390, 420)] {
                let near: Vec<_> = events
                    .iter()
                    .filter(|row| (start..end).contains(&row["frame_index"].as_u64().unwrap()))
                    .collect();
                assert_eq!(
                    near.len(),
                    1,
                    "pilot: expected one real contact in frames {start}..{end}; RAVEL hints={use_ravel_touch}, actual={near:?}"
                );
            }
        }
    }

    #[test]
    fn old_ravel_ball_is_not_touch_evidence() {
        let (bytes, _) = sample_frames(true, 320);
        let first = bytes.split(|byte| *byte == b'\n').next().unwrap();
        let mut raw: Value = serde_json::from_slice(first).unwrap();
        raw["selected_ball"]["source_model"] = json!("ravel");
        let frame: TrackedFrame = serde_json::from_value(raw).unwrap();
        assert!(transform_frame(frame, 0).unwrap().selected_ball.is_none());
    }
    #[test]
    fn saved_pose_refines_motion_touch_without_rerunning_gpu() {
        use volley_core::action_prediction::{ActionKind, ActionPrediction, PredictionMethod};
        let root = std::env::temp_dir().join(format!("touch-pose-{}", Uuid::new_v4()));
        let fusion = "a".repeat(32);
        let pose_id = "b".repeat(32);
        let fusion_dir = root.join("native_gpu_fusions").join(&fusion);
        let pose_dir = root.join("native_gpu_pose_runs").join(&pose_id);
        fs::create_dir_all(&fusion_dir).unwrap();
        fs::create_dir_all(&pose_dir).unwrap();
        let source_sha = "e".repeat(64);
        let identity = json!({"bytes":1234,"mtime_ns":4321});
        fs::write(
            fusion_dir.join("manifest.json"),
            serde_json::to_vec(&json!({
                "schema":"volley-native-gpu-fusion/1","fusion_id":fusion,
                "project_id":"abc123def456","video":"video.mp4","sha256":source_sha,
                "source_identity":identity,
            }))
            .unwrap(),
        )
        .unwrap();
        let mut points = [[0.5, 0.5, 0.0]; 17];
        points[5] = [0.43, 0.44, 0.95];
        points[6] = [0.57, 0.44, 0.95];
        points[7] = [0.41, 0.31, 0.96];
        points[8] = [0.59, 0.31, 0.96];
        points[9] = [0.49, 0.22, 0.96];
        points[10] = [0.52, 0.22, 0.96];
        let row = [24, 30, 36, 234, 240, 246]
            .into_iter()
            .map(|frame| {
                let mut sample_points = points;
                if frame >= 234 {
                    sample_points[9] = [0.47 + (frame - 234) as f64 / 150.0, 0.375, 0.95];
                }
                let bbox = if frame >= 234 {
                    [0.27, 0.15, 0.47, 0.65]
                } else {
                    [0.35, 0.17, 0.65, 0.90]
                };
                format!(
                    "{}\n",
                    json!({"frame":frame,
                    "players":[{"box":bbox,"keypoints":sample_points}]})
                )
            })
            .collect::<String>();
        fs::write(pose_dir.join("poses.jsonl"), &row).unwrap();
        let digest = format!("{:x}", Sha256::digest(row.as_bytes()));
        fs::write(
            pose_dir.join("manifest.json"),
            serde_json::to_vec(&json!({
                "schema":"volley-native-gpu-pose/1","state":"pose_evidence_complete",
                "pose_run_id":pose_id,"fusion_id":fusion,"project_id":"abc123def456",
                "video":"video.mp4","model_sha256":volley_inference::pose::MODEL_SHA256,
                "source_fusion_sha256":source_sha,"source_identity":identity,
                "file":"poses.jsonl","sha256":digest,"stride":6
            }))
            .unwrap(),
        )
        .unwrap();
        let tracked = TouchFrame {
            frame_index: 31,
            players: vec![TouchPlayer {
                track_id: 8,
                score: 0.90,
                touch_probability: Some(0.94),
                bbox_xyxy_norm: [0.35, 0.17, 0.65, 0.90],
            }],
            selected_ball: None,
        };
        let block: Vec<TouchFrame> = (24..=36)
            .map(|frame_index| {
                let mut frame = tracked.clone();
                frame.frame_index = frame_index;
                let ball_x = if frame_index <= 31 {
                    0.43 + (frame_index - 24) as f64 * 0.01
                } else {
                    0.50 - (frame_index - 31) as f64 * 0.01
                };
                frame.selected_ball = Some(TouchBall {
                    source_model: "vballnet".into(),
                    score: 0.97,
                    bbox_xyxy_norm: [ball_x - 0.01, 0.19, ball_x + 0.01, 0.21],
                });
                frame
            })
            .collect();
        let samples = pose_frames(&root, &fusion, &block).unwrap();
        assert_eq!(
            samples
                .iter()
                .map(|frame| frame.frame_index)
                .collect::<Vec<_>>(),
            vec![24, 30, 36]
        );
        let mut events = vec![PhysicalTouchCandidate {
            frame_index: 31,
            provisional_track_id: Some(8),
            probability: 0.94,
            ball_x_px: 320.0,
            ball_y_px: 72.0,
            motion_frame_index: 31,
            direction_change_deg: 80.0,
            speed_before_px_frame: 7.0,
            speed_after_px_frame: 13.0,
            velocity_residual_px_frame: 12.0,
            technique: "unknown_touch",
            court: None,
            action_prediction: Some(ActionPrediction {
                kind: ActionKind::Attack,
                evidence_score: 0.87,
                method: PredictionMethod::ContactGeometryV1,
                depth_ratio: -0.1,
                posture_ratio: 0.4,
            }),
            physical_motion_supported: true,
            body_contact_supported: false,
            ball_occluded_at_contact: false,
            inferred_body_reach_supported: false,
            athlete_identity_verified: false,
        }];
        refine_pose_events(
            &mut events,
            &block,
            640,
            360,
            &root,
            &fusion,
            TouchSettings::default(),
        )
        .unwrap();
        let changed = events[0].action_prediction.as_ref().unwrap();
        assert_eq!(changed.kind, ActionKind::OverheadPass);
        assert_eq!(changed.method, PredictionMethod::PoseKeypointsV1);
        let mut distant = events.clone();
        distant[0].ball_x_px = 620.0;
        refine_pose_events(
            &mut distant,
            &block,
            640,
            360,
            &root,
            &fusion,
            TouchSettings {
                require_pose: true,
                ..TouchSettings::default()
            },
        )
        .unwrap();
        assert!(distant.is_empty());
        let mut close = events.clone();
        refine_pose_events(
            &mut close,
            &block,
            640,
            360,
            &root,
            &fusion,
            TouchSettings {
                require_pose: true,
                ..TouchSettings::default()
            },
        )
        .unwrap();
        assert_eq!(close.len(), 1);
        // A different fusion cannot borrow the saved pose from this one.
        refine_pose_events(
            &mut events,
            &[tracked],
            640,
            360,
            &root,
            &"c".repeat(32),
            TouchSettings::default(),
        )
        .unwrap();
        assert_eq!(
            events[0].action_prediction.as_ref().unwrap().method,
            PredictionMethod::PoseKeypointsV1
        );
        let (source_bytes, source_sha) = sample_frames(true, 240);
        let source = root.join("tracked.jsonl");
        fs::write(&source, source_bytes).unwrap();
        let (_, count, _) = detect_stream_with_pose(
            &source,
            &root.join("pose-contacts.jsonl"),
            &source_sha,
            630,
            640,
            360,
            TouchSettings {
                algorithm: TouchAlgorithm::PoseMotion,
                ..TouchSettings::default()
            },
            Some((&root, &fusion)),
            None,
        )
        .unwrap();
        assert_eq!(count, 1);
        let _ = fs::remove_dir_all(root);
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
        // The seam check must use a physically reachable trajectory reversal,
        // not a RAVEL-only peak rejected by motion-first detection.
        let reference = TouchSettings {
            min_gap: 10,
            max_reach_ratio: 0.70,
            min_interval_sec: 0.35,
            ..TouchSettings::default()
        };
        for impact in [240_u64, 479_u64] {
            let (bytes, sha) = sample_frames(true, impact);
            let source = root.join(format!("source-{impact}.jsonl"));
            fs::write(&source, bytes).unwrap();
            let path = root.join(format!("default-{impact}.jsonl"));
            let (_, events, _) =
                detect_stream(&source, &path, &sha, 630, 640, 360, reference).unwrap();
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
                ..reference
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

#[cfg(test)]
mod local_tuning_diagnostic {
    use super::*;

    #[test]
    #[ignore = "Use VOLLEY_TOUCH_REPLAY for isolated analysis; never writes production"]
    fn inspect_real_motion_candidates() {
        let path = std::env::var("VOLLEY_TOUCH_REPLAY").unwrap();
        let file = std::fs::read_to_string(path).unwrap();
        let frames = file
            .lines()
            .enumerate()
            .map(|(i, line)| {
                transform_frame(
                    serde_json::from_str::<TrackedFrame>(line).unwrap(),
                    i as u64,
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        let clean = reconstruct_track_gaps(&exclude_ball_teleports(&frames, 1280, 720));
        for (gap, interval, reach) in [(5, 0.2, 0.75), (5, 0.2, 0.90), (5, 0.2, 1.00)] {
            let settings = TouchSettings {
                min_gap: gap,
                min_interval_sec: interval,
                threshold: 0.48,
                max_reach_ratio: reach,
                ..TouchSettings::default()
            };
            let events =
                evaluate_block(&clean, 1280, 720, 0, clean.len() as u64, settings).unwrap();
            println!(
                "TUNING gap={gap} interval={interval} reach={reach}, count={} => {:?}",
                events.len(),
                events
                    .iter()
                    .map(|e| (
                        e.frame_index,
                        e.provisional_track_id,
                        (e.probability * 100.0).round() as i64,
                    ))
                    .collect::<Vec<_>>()
            );
        }
    }
}
