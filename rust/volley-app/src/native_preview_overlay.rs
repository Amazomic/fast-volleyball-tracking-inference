//! Bounded, source-bound frame overlays from the immutable CPU replay.
//! Indexed byte offsets make seeking inexpensive even in long source videos.
use crate::{
    cpu_retrack::{safe_file, safe_folder},
    native_rally_preview, projects,
    storage::{self, ApiResult},
    AppState,
};
use axum::{
    extract::{Query, State},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::SystemTime,
};

const MAX_MANIFEST_BYTES: u64 = 16 * 1024;
const MAX_REPLAY_BYTES: u64 = 64 * 1024 * 1024 * 1024;
const MAX_LINE_BYTES: u64 = 256 * 1024;
const MAX_WINDOW: u64 = 180;
const STRIDE: u64 = 90;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlayQuery {
    project_id: String,
    rally_run_id: String,
    start_frame: u64,
    count: u64,
    raw: Option<bool>,
    ball_confidence: Option<u8>,
    player_confidence: Option<u8>,
    touch_confidence: Option<u8>,
}

#[derive(Clone)]
struct ReplayIndex {
    path: PathBuf,
    sha: String,
    size: u64,
    modified: Option<SystemTime>,
    change: Option<(u64, u64, i64, i64)>,
    offsets: Arc<Vec<u64>>,
    frames: u64,
}
static INDICES: OnceLock<Mutex<Vec<ReplayIndex>>> = OnceLock::new();

fn bounded_line(reader: &mut BufReader<File>) -> Result<Option<Vec<u8>>, String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    let len = reader
        .take(MAX_LINE_BYTES)
        .read_until(b'\n', &mut bytes)
        .map_err(|e| format!("Read overlay row: {e}"))?;
    if len == 0 {
        return Ok(None);
    }
    if bytes.last() != Some(&b'\n') {
        return Err("Oversized or unfinished overlay row".into());
    }
    Ok(Some(bytes))
}

fn index_for(path: &Path, sha: &str, frames: u64) -> Result<ReplayIndex, String> {
    let store = INDICES.get_or_init(|| Mutex::new(Vec::new()));
    let mut cache = store.lock().map_err(|_| "Overlay index lock is poisoned")?;
    let meta = fs::metadata(path).map_err(|e| format!("Overlay metadata: {e}"))?;
    if meta.len() > MAX_REPLAY_BYTES {
        return Err("Overlay replay exceeds size limit".into());
    }
    let modified = meta.modified().ok();
    // mtime alone is unsafe on Windows (clock-tick granularity); the platform
    // change stamp detects same-size edits. Without it, never trust the cache.
    let change = volley_media::preparation::file_change_stamp(path, &meta);
    if change.is_some() {
        if let Some(hit) = cache
            .iter()
            .find(|item| {
                item.path == path
                    && item.sha == sha
                    && item.frames == frames
                    && item.size == meta.len()
                    && item.modified == modified
                    && item.change == change
            })
            .cloned()
        {
            return Ok(hit);
        }
    }
    // First access validates the entire immutable replay once and stores
    // sparse byte offsets. A new mtime/size/sha invalidates the index.
    let mut reader = BufReader::new(File::open(path).map_err(|e| format!("Open replay: {e}"))?);
    let mut digest = Sha256::new();
    let mut offsets = Vec::new();
    let mut byte_offset = 0u64;
    for index in 0..frames {
        if index % STRIDE == 0 {
            offsets.push(byte_offset);
        }
        let row = bounded_line(&mut reader)?.ok_or("Replay has fewer rows than its manifest")?;
        byte_offset += row.len() as u64;
        digest.update(&row);
    }
    if bounded_line(&mut reader)?.is_some() {
        return Err("Replay has extra rows".into());
    }
    if format!("{:x}", digest.finalize()) != sha {
        return Err("Replay SHA-256 does not match selected CPU version".into());
    }
    let entry = ReplayIndex {
        path: path.to_owned(),
        sha: sha.to_owned(),
        size: meta.len(),
        modified,
        change,
        offsets: Arc::new(offsets),
        frames,
    };
    if change.is_some() {
        cache.retain(|row| row.path != path);
        cache.push(entry.clone());
        if cache.len() > 6 {
            cache.remove(0);
        }
    }
    Ok(entry)
}

fn box_norm(value: &Value) -> Option<[f64; 4]> {
    let array = value.as_array()?;
    if array.len() != 4 {
        return None;
    }
    let bb = [
        array[0].as_f64()?,
        array[1].as_f64()?,
        array[2].as_f64()?,
        array[3].as_f64()?,
    ];
    if bb
        .iter()
        .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
        || bb[2] <= bb[0]
        || bb[3] <= bb[1]
    {
        return None;
    }
    Some(bb)
}

fn raw_frame(
    frame: &Value,
    ball_threshold: u8,
    player_threshold: u8,
    touch_threshold: u8,
) -> Value {
    let above = |value: &Value, threshold: u8| {
        value.as_f64().is_some_and(|score| {
            score.is_finite()
                && (0.0..=1.0).contains(&score)
                && score >= f64::from(threshold) / 100.0
        })
    };
    let balls = frame["ball_candidates"]
        .as_array()
        .into_iter()
        .flatten()
        .take(2)
        .filter_map(|item| {
            let detection = &item["detection"];
            let source = item["source_model"].as_str()?;
            if !matches!(source, "ravel" | "vballnet")
                || !above(&detection["score"], ball_threshold)
            {
                return None;
            }
            let b = box_norm(&detection["bbox_xyxy_norm"])?;
            Some(json!({"point":[(b[0]+b[2])*0.5,(b[1]+b[3])*0.5],"source":source}))
        })
        .collect::<Vec<_>>();
    let mut players = Vec::new();
    let mut touches = Vec::new();
    for item in frame["raw_player_candidates"]
        .as_array()
        .into_iter()
        .flatten()
        .take(128)
    {
        let Some(b) = box_norm(&item["bbox_xyxy_norm"]) else {
            continue;
        };
        if above(&item["score"], player_threshold) {
            players.push(json!({"box":b}));
        }
        if above(&item["touch_probability"], touch_threshold) {
            touches.push(json!({"box":b}));
        }
    }
    json!({"balls":balls,"players":players,"touches":touches})
}

fn gated_ball_observations(samples: &[Option<(&str, [f64; 2])>]) -> Vec<Option<[f64; 2]>> {
    samples
        .iter()
        .enumerate()
        .map(|(i, sample)| {
            let (source, point) = (*sample)?;
            if source == "vballnet" {
                return Some(point);
            }
            if source != "ravel" {
                return None;
            }
            let before = (i.saturating_sub(8)..i)
                .rev()
                .find_map(|j| samples[j].filter(|s| s.0 == "vballnet").map(|s| (j, s.1)))?;
            let after = (i + 1..samples.len().min(i + 9))
                .find_map(|j| samples[j].filter(|s| s.0 == "vballnet").map(|s| (j, s.1)))?;
            if after.0 - before.0 > 8 {
                return None;
            }
            let t = (i - before.0) as f64 / (after.0 - before.0) as f64;
            let expected = [
                before.1[0] + (after.1[0] - before.1[0]) * t,
                before.1[1] + (after.1[1] - before.1[1]) * t,
            ];
            ((point[0] - expected[0]).hypot(point[1] - expected[1]) <= 0.05).then_some(point)
        })
        .collect()
}

fn extract_frames(
    path: &Path,
    sha: &str,
    total: u64,
    start: u64,
    end: u64,
    scope: &native_rally_preview::RallyWindows,
    raw_thresholds: Option<(u8, u8, u8)>,
) -> Result<Vec<Value>, String> {
    let index = index_for(path, sha, total)?;
    let read_start = start.saturating_sub(8);
    let read_end = end.saturating_add(8).min(total);
    let anchor = read_start / STRIDE * STRIDE;
    let offset = index.offsets[(read_start / STRIDE) as usize];
    let mut file = File::open(path).map_err(|e| format!("Open indexed replay: {e}"))?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| format!("Seek replay: {e}"))?;
    let mut reader = BufReader::new(file);
    let mut output = Vec::with_capacity((end - start) as usize);
    for i in anchor..read_end {
        let bytes = bounded_line(&mut reader)?.ok_or("Indexed replay is truncated")?;
        if i < read_start {
            continue;
        }
        let frame: Value =
            serde_json::from_slice(&bytes).map_err(|_| "Malformed indexed overlay frame")?;
        if frame["frame_index"].as_u64() != Some(i)
            || frame["track_ids_provisional"] != true
            || frame["confirmed_athlete_identities"] != false
            || frame["touches_confirmed"] != false
        {
            return Err("Overlay frame provenance mismatch".into());
        }
        let ball = box_norm(&frame["selected_ball"]["detection"]["bbox_xyxy_norm"])
            .map(|bb| json!([(bb[0] + bb[2]) * 0.5, (bb[1] + bb[3]) * 0.5]));
        let players = frame["tracked_players"]
            .as_array()
            .into_iter()
            .flatten()
            .take(128)
            .filter_map(|p| {
                let id = p["track_id"].as_u64().filter(|id| *id > 0)?;
                if p["is_verified_athlete"] != false {
                    return None;
                }
                let bb = box_norm(&p["detection"]["bbox_xyxy_norm"])?;
                Some(json!({"id":id,"box":bb}))
            })
            .collect::<Vec<_>>();
        let raw =
            raw_thresholds.map(|(ball, player, touch)| raw_frame(&frame, ball, player, touch));
        output.push(json!({
            "frame":i,"ball":ball,"ball_source":frame["selected_ball"]["source_model"],
            "raw":raw,
            "players":if scope.contains(i){players}else{Vec::new()}
        }));
    }
    let samples = output
        .iter()
        .map(|row| {
            let source = row["ball_source"].as_str()?;
            let ball = row["ball"].as_array()?;
            Some((source, [ball[0].as_f64()?, ball[1].as_f64()?]))
        })
        .collect::<Vec<_>>();
    let gated = gated_ball_observations(&samples);
    for (row, ball) in output.iter_mut().zip(gated) {
        row["ball"] = json!(ball);
        row.as_object_mut().unwrap().remove("ball_source");
    }
    Ok(output
        .into_iter()
        .filter(|row| {
            row["frame"]
                .as_u64()
                .is_some_and(|frame| frame >= start && frame < end)
        })
        .collect())
}

pub async fn get(
    State(state): State<Arc<AppState>>,
    Query(query): Query<OverlayQuery>,
) -> ApiResult<Value> {
    // inspect_project runs ffprobe and SHA verification; this handler is
    // polled continuously during playback, so keep it off the tokio workers.
    let inspect_state = state.clone();
    let project_id = query.project_id.clone();
    let run_id = query.rally_run_id.clone();
    let run = tokio::task::spawn_blocking(move || {
        native_rally_preview::inspect_project(&inspect_state, &project_id, &run_id)
    })
    .await
    .map_err(|e| storage::internal(format!("Overlay inspect task: {e}")))??;
    if query.count == 0 || query.count > MAX_WINDOW || query.start_frame >= run.frames {
        return Err(storage::bad_request("Overlay frame window is out of range"));
    }
    let thresholds = (
        query.ball_confidence.unwrap_or(50),
        query.player_confidence.unwrap_or(50),
        query.touch_confidence.unwrap_or(50),
    );
    if thresholds.0 > 100 || thresholds.1 > 100 || thresholds.2 > 100 {
        return Err(storage::bad_request("Confidence must be between 0 and 100"));
    }
    let raw_thresholds = query.raw.unwrap_or(false).then_some(thresholds);
    let end = query
        .start_frame
        .saturating_add(query.count)
        .min(run.frames);
    let replay_dir = projects::project_root(&state)
        .join(&query.project_id)
        .join("native_cpu_replays")
        .join(&run.replay_id);
    safe_folder(&replay_dir).map_err(storage::bad_request)?;
    let manifest: Value = serde_json::from_slice(
        &fs::read(
            safe_file(&replay_dir, "manifest.json", MAX_MANIFEST_BYTES)
                .map_err(storage::bad_request)?,
        )
        .map_err(|e| storage::bad_request(format!("Read replay manifest: {e}")))?,
    )
    .map_err(|e| storage::bad_request(format!("Invalid replay manifest: {e}")))?;
    if manifest["schema"] != "volley-native-cpu-tracking/1"
        || manifest["state"] != "provisional_tracklets_rebuilt"
        || manifest["project_id"] != query.project_id
        || manifest["video"] != run.video
        || manifest["replay_id"] != run.replay_id
        || manifest["file"] != "tracked_frames.jsonl"
        || manifest["fps"] != 30
        || manifest["frames"].as_u64() != Some(run.frames)
        || manifest["sha256"].as_str() != Some(run.replay_sha.as_str())
        || manifest["track_ids_provisional"] != true
        || manifest["athlete_identities_confirmed"] != false
        || manifest["touches_confirmed"] != false
        || manifest["cpu_processing_complete"] != false
    {
        return Err(storage::bad_request(
            "Overlay replay does not match postprocessing",
        ));
    }
    let replay_path = safe_file(&replay_dir, "tracked_frames.jsonl", MAX_REPLAY_BYTES)
        .map_err(storage::bad_request)?;
    let replay_sha = run.replay_sha.clone();
    let total = run.frames;
    let start = query.start_frame;
    let preview_run = run.clone();
    let frames = tokio::task::spawn_blocking(move || {
        let windows = native_rally_preview::verified_track_windows(&preview_run)
            .map_err(|e| format!("Unverified rally scope: {e}"))?;
        extract_frames(
            &replay_path,
            &replay_sha,
            total,
            start,
            end,
            &windows,
            raw_thresholds,
        )
    })
    .await
    .map_err(|e| storage::internal(format!("Overlay task failure: {e}")))?
    .map_err(storage::bad_request)?;
    Ok(Json(json!({
        "schema":"volley-native-preview-overlay/1",
                "fps":30,
        "project_id":run.project_id,"video":run.video,
        "rally_run_id":run.run_id,"replay_sha256":run.replay_sha,
        "width":run.width,"height":run.height,
        "start_frame":start,"end_frame":end,
        "frames":frames,
        "touches_verified":false,"athlete_identities_verified":false,
    })))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn concurrent_overlay_requests_share_the_verified_index() {
        let root =
            std::env::temp_dir().join(format!("volley-overlay-index-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let input = root.join("tracked_frames.jsonl");
        let data = b"{}\n".repeat(100_000);
        fs::write(&input, &data).unwrap();
        let sha = format!("{:x}", Sha256::digest(&data));
        let barrier = Arc::new(std::sync::Barrier::new(4));
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let path = input.clone();
                let sha = sha.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    index_for(&path, &sha, 100_000).unwrap()
                })
            })
            .collect();
        let indices: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert!(indices
            .iter()
            .all(|index| index.offsets.as_ptr() == indices[0].offsets.as_ptr()));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn raw_thresholds_filter_ball_player_and_touch_independently() {
        let frame = json!({
            "ball_candidates":[{"source_model":"vballnet","detection":{"score":0.7,"bbox_xyxy_norm":[0.1,0.1,0.2,0.2]}}],
            "raw_player_candidates":[{"score":0.6,"touch_probability":0.9,"bbox_xyxy_norm":[0.2,0.2,0.4,0.8]}]
        });
        let raw = raw_frame(&frame, 80, 70, 80);
        assert_eq!(raw["balls"].as_array().unwrap().len(), 0);
        assert_eq!(raw["players"].as_array().unwrap().len(), 0);
        assert_eq!(raw["touches"].as_array().unwrap().len(), 1);
        let raw = raw_frame(&frame, 70, 60, 95);
        assert_eq!(raw["balls"].as_array().unwrap().len(), 1);
        assert_eq!(raw["players"].as_array().unwrap().len(), 1);
        assert_eq!(raw["touches"].as_array().unwrap().len(), 0);
    }
    #[test]
    fn ravel_ball_is_used_only_between_nearby_vball_observations() {
        let samples = vec![
            Some(("ravel", [0.1, 0.1])),
            Some(("vballnet", [0.2, 0.2])),
            Some(("ravel", [0.3, 0.2])),
            Some(("ravel", [0.9, 0.9])),
            Some(("vballnet", [0.5, 0.2])),
            Some(("ravel", [0.6, 0.2])),
        ];
        let result = gated_ball_observations(&samples);
        assert_eq!(
            result,
            vec![
                None,
                Some([0.2, 0.2]),
                Some([0.3, 0.2]),
                None,
                Some([0.5, 0.2]),
                None
            ]
        );
    }
    #[test]
    fn overlays_hide_players_outside_plays_and_keep_inside_player_ids() {
        let root =
            std::env::temp_dir().join(format!("volley-overlay-scope-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let input = root.join("tracked_frames.jsonl");
        let mut data = Vec::new();
        for frame in 0..6u64 {
            let row = json!({
                "frame_index":frame,"track_ids_provisional":true,
                "confirmed_athlete_identities":false,"touches_confirmed":false,
                "selected_ball":null,
                "tracked_players":[{"track_id":7,"is_verified_athlete":false,
                    "detection":{"bbox_xyxy_norm":[0.1,0.1,0.2,0.8]}}]
            });
            data.extend(serde_json::to_vec(&row).unwrap());
            data.push(b'\n');
        }
        fs::write(&input, &data).unwrap();
        let sha = format!("{:x}", Sha256::digest(&data));
        let active = native_rally_preview::RallyWindows {
            intervals: vec![(2, 4)],
        };
        let overlay = extract_frames(&input, &sha, 6, 0, 6, &active, None).unwrap();
        assert_eq!(overlay.len(), 6);
        for (frame, item) in overlay.iter().enumerate() {
            assert_eq!(
                item["players"].as_array().unwrap().len(),
                if (2..=4).contains(&frame) { 1 } else { 0 }
            );
        }
        assert_eq!(overlay[2]["players"][0]["id"], 7);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_invalid_boxes() {
        assert!(box_norm(&json!([0.1, 0.2, 0.9, 0.8])).is_some());
        assert!(box_norm(&json!([0.8, 0.2, 0.1, 0.7])).is_none());
        assert!(box_norm(&json!([-0.1, 0.2, 0.9, 0.7])).is_none());
        assert!(box_norm(&json!([0.1, 0.2, 0.9])).is_none());
    }
}
