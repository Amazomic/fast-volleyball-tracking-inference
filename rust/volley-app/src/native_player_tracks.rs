//! Versioned manual assignment of *provisional* native CPU Track IDs to
//! athlete registry entries. The raw GPU/CPU evidence is NEVER rewritten.
//! ReID appearance vectors / saved athlete photo templates are not generated
//! by this module; each owner association requires an explicit human action.
use crate::{
    cpu_retrack::{opaque_id, safe_file, safe_folder, BoundedReader},
    inference_stream, player_catalog, preparation, project_archive, projects,
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
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use volley_core::fusion::Candidate;

const MAX_TRACKS: usize = 20_000;
const MAX_MANIFEST: u64 = 16 * 1024;
const MAX_EVIDENCE: u64 = 64 * 1024 * 1024 * 1024;
const MAX_LINKS: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrackQuery {
    project_id: String,
    video: String,
    replay_id: String,
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_limit")]
    limit: usize,
}
fn default_limit() -> usize {
    40
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrackEdit {
    project_id: String,
    video: String,
    replay_id: String,
    action: String,
    track_ids: Vec<u64>,
    #[serde(default)]
    player_id: Option<String>,
    expected_revision: String,
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_limit")]
    limit: usize,
}
#[derive(Debug, Default, Clone)]
struct Aggregate {
    first: u64,
    last: u64,
    observed: u64,
    best_frame: u64,
    best_score: f64,
    best_box: [f64; 4],
}
#[derive(Deserialize)]
struct Tracked {
    track_id: u64,
    detection: Candidate,
    is_verified_athlete: bool,
}
#[derive(Deserialize)]
struct Frame {
    frame_index: u64,
    tracked_players: Vec<Tracked>,
    track_ids_provisional: bool,
    confirmed_athlete_identities: bool,
    touches_confirmed: bool,
}
struct Inputs {
    project_id: String,
    video: String,
    replay_id: String,
    source_sha: String,
    tracks: BTreeMap<u64, Aggregate>,
    roster: BTreeMap<String, String>,
    sidecar: PathBuf,
    can_write: bool,
}
fn valid_player_id(id: &str) -> bool {
    id.len() == 12
        && id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
fn valid_sha(sha: &str) -> bool {
    sha.len() == 64
        && sha
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
fn index_replay(
    path: &Path,
    expected_sha: &str,
    frames: u64,
) -> Result<BTreeMap<u64, Aggregate>, String> {
    if frames == 0 || frames > 48 * 3600 * 30 || !valid_sha(expected_sha) {
        return Err("Unsupported native track replay length or SHA-256".into());
    }
    let mut reader = BoundedReader::open(path)?;
    let mut tracks = BTreeMap::<u64, Aggregate>::new();
    for frame_idx in 0..frames {
        let bytes = reader
            .line()?
            .ok_or("Native Track ID frame source is truncated")?;
        let frame: Frame = serde_json::from_slice(&bytes)
            .map_err(|e| format!("Invalid native Track ID row: {e}"))?;
        if frame.frame_index != frame_idx
            || !frame.track_ids_provisional
            || frame.confirmed_athlete_identities
            || frame.touches_confirmed
            || frame.tracked_players.len() > 128
        {
            return Err("Native track source is out of order or claims verified identities".into());
        }
        let mut appeared = BTreeSet::new();
        for tracked in frame.tracked_players {
            if tracked.track_id == 0
                || tracked.track_id > i64::MAX as u64
                || tracked.is_verified_athlete
                || !appeared.insert(tracked.track_id)
                || tracked.detection.frame_index != frame_idx
                || tracked.detection.class_name != "player"
            {
                return Err("Native track has fabricated identity or repeated ID per frame".into());
            }
            tracked
                .detection
                .validate(frame_idx - frame_idx % 9, "players")?;
            if !tracks.contains_key(&tracked.track_id) && tracks.len() >= MAX_TRACKS {
                return Err("Too many distinct native Track IDs".into());
            }
            let record = tracks.entry(tracked.track_id).or_insert_with(|| Aggregate {
                first: frame_idx,
                last: frame_idx,
                observed: 0,
                best_frame: frame_idx,
                best_score: 0.0,
                best_box: [0.0; 4],
            });
            record.last = frame_idx;
            record.observed += 1;
            if tracked.detection.score > record.best_score {
                record.best_score = tracked.detection.score;
                record.best_frame = frame_idx;
                record.best_box = tracked.detection.bbox_xyxy_norm;
            }
        }
    }
    if reader.line()?.is_some() {
        return Err("Extra undeclared Track ID frames".into());
    }
    if reader.finish() != expected_sha {
        return Err("Native Track ID SHA-256 mismatch".into());
    }
    Ok(tracks)
}
fn source(
    state: &AppState,
    project_id: &str,
    video: &str,
    replay_id: &str,
) -> Result<Inputs, ApiError> {
    storage::project_id(project_id)?;
    storage::video_name(video)?;
    if !opaque_id(replay_id) {
        return Err(storage::bad_request("Invalid native replay ID"));
    }
    let project = projects::existing_project(state, project_id)?;
    if !project["videos"]
        .as_array()
        .is_some_and(|v| v.iter().any(|row| row.as_str() == Some(video)))
    {
        return Err(storage::bad_request(
            "Video does not belong to selected project",
        ));
    }
    let plan = preparation::plan_for(state, project_id, video)?;
    let prepared = plan
        .ready_path()
        .ok_or_else(|| storage::bad_request("Prepared video missing"))?;
    let (size, mtime) = inference_stream::source_stamp(prepared).map_err(storage::bad_request)?;
    let project_dir = projects::project_root(state).join(project_id);
    safe_folder(&project_dir).map_err(storage::bad_request)?;
    let replay_root = project_dir.join("native_cpu_replays");
    safe_folder(&replay_root).map_err(storage::bad_request)?;
    let replay_dir = replay_root.join(replay_id);
    safe_folder(&replay_dir).map_err(storage::bad_request)?;
    let manifest: Value = serde_json::from_slice(
        &fs::read(
            safe_file(&replay_dir, "manifest.json", MAX_MANIFEST).map_err(storage::bad_request)?,
        )
        .map_err(|e| storage::internal(format!("Read Track ID manifest: {e}")))?,
    )
    .map_err(|e| storage::bad_request(format!("Bad Track ID manifest: {e}")))?;
    if manifest["schema"] != "volley-native-cpu-tracking/1"
        || manifest["state"] != "provisional_tracklets_rebuilt"
        || manifest["project_id"].as_str() != Some(project_id)
        || manifest["video"].as_str() != Some(video)
        || manifest["replay_id"].as_str() != Some(replay_id)
        || manifest["source_identity"] != json!({"bytes":size,"mtime_ns":mtime})
        || manifest["fps"] != 30
        || manifest["file"] != "tracked_frames.jsonl"
        || manifest["track_ids_provisional"] != true
        || manifest["athlete_identities_confirmed"] != false
        || manifest["touches_confirmed"] != false
        || manifest["cpu_processing_complete"] != false
    {
        return Err(storage::bad_request(
            "Track ID replay is not valid for this video",
        ));
    }
    let frames = manifest["frames"]
        .as_u64()
        .filter(|x| *x > 0 && *x <= 48 * 3600 * 30)
        .ok_or_else(|| storage::bad_request("Invalid native Track ID frame count"))?;
    let sha = manifest["sha256"]
        .as_str()
        .filter(|s| valid_sha(s))
        .ok_or_else(|| storage::bad_request("Missing Track ID source SHA-256"))?
        .to_owned();
    let tracks = index_replay(
        &safe_file(&replay_dir, "tracked_frames.jsonl", MAX_EVIDENCE)
            .map_err(storage::bad_request)?,
        &sha,
        frames,
    )
    .map_err(storage::bad_request)?;
    let (catalog, _) = player_catalog::load(state, false)?;
    let allowed = project["player_ids"]
        .as_array()
        .ok_or_else(|| storage::bad_request("Project roster must contain a player_ids array"))?
        .iter()
        .filter_map(Value::as_str)
        .collect::<BTreeSet<_>>();
    let mut roster = BTreeMap::new();
    for entry in catalog["players"].as_array().into_iter().flatten() {
        if let (Some(id), Some(name)) = (entry["id"].as_str(), entry["name"].as_str()) {
            if valid_player_id(id) && allowed.contains(id) {
                roster.insert(id.to_owned(), name.to_owned());
            }
        }
    }
    let links_dir = project_dir.join("native_track_assignments");
    if let Ok(meta) = fs::symlink_metadata(&links_dir) {
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(storage::bad_request("Unsafe player assignment directory"));
        }
    }
    Ok(Inputs {
        project_id: project_id.into(),
        video: video.into(),
        replay_id: replay_id.into(),
        source_sha: sha,
        tracks,
        roster,
        sidecar: links_dir.join(format!("{replay_id}.json")),
        can_write: state.writes_enabled,
    })
}
fn revision(bytes: Option<&[u8]>) -> String {
    bytes
        .map(|b| format!("{:x}", Sha256::digest(b)))
        .unwrap_or_else(|| "absent".into())
}
fn existing(sidecar: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::symlink_metadata(sidecar) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_file() || meta.len() > MAX_LINKS => {
            Err("Unsafe or oversized native track assignment file".into())
        }
        Ok(_) => fs::read(sidecar)
            .map(Some)
            .map_err(|e| format!("Read track assignment: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("Check player assignment: {e}")),
    }
}
fn base_document(ctx: &Inputs) -> Value {
    json!({
        "schema":"volley-native-player-track-links/1",
        "project_id":ctx.project_id,"video":ctx.video,
        "replay_id":ctx.replay_id,
        "source_replay_sha256":ctx.source_sha,
        "assignments":{},"excluded_ids":[],
        "athlete_identity_claims_manually_reviewed":true,
        "automatic_reid_binding":false,
    })
}
fn validated(raw: Option<&[u8]>, ctx: &Inputs) -> Result<Value, String> {
    let Some(bytes) = raw else {
        return Ok(base_document(ctx));
    };
    let doc: Value = serde_json::from_slice(bytes)
        .map_err(|e| format!("Invalid track assignments JSON: {e}"))?;
    if doc["schema"] != "volley-native-player-track-links/1"
        || doc["project_id"] != ctx.project_id
        || doc["video"] != ctx.video
        || doc["replay_id"] != ctx.replay_id
        || doc["source_replay_sha256"] != ctx.source_sha
        || doc["athlete_identity_claims_manually_reviewed"] != true
        || doc["automatic_reid_binding"] != false
    {
        return Err("Manual Track ID links refer to a different replay or schema".into());
    }
    let assignments = doc["assignments"]
        .as_object()
        .ok_or("Invalid assigned Track IDs")?;
    let excluded = doc["excluded_ids"]
        .as_array()
        .ok_or("Invalid excluded Track IDs")?;
    if assignments.len() > MAX_TRACKS || excluded.len() > MAX_TRACKS {
        return Err("Manual player links exceed safety cap".into());
    }
    let mut excluded_set = BTreeSet::new();
    for value in excluded {
        let id = value.as_u64().ok_or("Invalid excluded Track ID")?;
        if !ctx.tracks.contains_key(&id) || !excluded_set.insert(id) {
            return Err("Unknown or duplicate excluded Track ID".into());
        }
    }
    for (key, value) in assignments {
        let id = key
            .parse::<u64>()
            .map_err(|_| "Invalid assigned Track ID")?;
        if id.to_string() != *key || !ctx.tracks.contains_key(&id) || excluded_set.contains(&id) {
            return Err("Stale, excluded or unknown assigned Track ID".into());
        }
        let owner = value.as_str().ok_or("Invalid player ID in Track link")?;
        if !ctx.roster.contains_key(owner) {
            return Err("Linked player is no longer on the project's roster".into());
        }
    }
    Ok(doc)
}
fn page(
    ctx: &Inputs,
    doc: &Value,
    version: &str,
    offset: usize,
    limit: usize,
) -> Result<Value, String> {
    if limit == 0 || limit > 100 || offset > MAX_TRACKS {
        return Err("Invalid Track ID page size".into());
    }
    let assignments = doc["assignments"]
        .as_object()
        .ok_or("Assignments missing")?;
    let excluded = doc["excluded_ids"]
        .as_array()
        .ok_or("Excluded list missing")?
        .iter()
        .filter_map(Value::as_u64)
        .collect::<BTreeSet<_>>();
    let mut rows = Vec::new();
    for (id, info) in ctx.tracks.iter().skip(offset).take(limit) {
        let owner = assignments.get(&id.to_string()).and_then(Value::as_str);
        rows.push(json!({
            "track_id":id,"first_frame":info.first,"last_frame":info.last,
            "observations":info.observed,"best_frame":info.best_frame,
            "best_score":info.best_score,"bbox_xyxy_norm":info.best_box,
            "assigned_player_id":owner,
            "assigned_player_name":owner.and_then(|v|ctx.roster.get(v)),
            "excluded":excluded.contains(id),
            "identity_source":if owner.is_some(){"manual"}else{"none"},
            "track_id_provisional":true,
        }));
    }
    let roster = ctx
        .roster
        .iter()
        .map(|(id, name)| json!({"id":id,"name":name}))
        .collect::<Vec<_>>();
    Ok(json!({
        "schema":"volley-native-player-track-links/1",
        "project_id":ctx.project_id,"video":ctx.video,"replay_id":ctx.replay_id,
        "source_replay_sha256":ctx.source_sha,
        "revision":version,"writable":ctx.can_write,
        "total_tracks":ctx.tracks.len(),"offset":offset,"limit":limit,
        "assigned_count":assignments.len(),"excluded_count":excluded.len(),
        "roster":roster,"tracks":rows,
        "athlete_links_require_manual_confirmation":true,
        "automatic_reid_binding":false,"photos_auto_registered":false,
    }))
}
fn read_internal(state: &AppState, req: &TrackQuery) -> Result<Value, ApiError> {
    let ctx = source(state, &req.project_id, &req.video, &req.replay_id)?;
    let bytes = existing(&ctx.sidecar).map_err(storage::bad_request)?;
    let doc = validated(bytes.as_deref(), &ctx).map_err(storage::bad_request)?;
    page(
        &ctx,
        &doc,
        &revision(bytes.as_deref()),
        req.offset,
        req.limit,
    )
    .map_err(storage::bad_request)
}
fn change(state: &AppState, req: &TrackEdit) -> Result<Value, ApiError> {
    if !state.writes_enabled {
        return Err(storage::conflict(
            "Manual Track ID editing requires isolated Rust storage",
        ));
    }
    if !matches!(
        req.action.as_str(),
        "assign" | "unassign" | "exclude" | "restore"
    ) || req.track_ids.is_empty()
        || req.track_ids.len() > 200
        || req.track_ids.iter().collect::<BTreeSet<_>>().len() != req.track_ids.len()
    {
        return Err(storage::bad_request(
            "Choose 1–200 unique IDs and a valid edit action",
        ));
    }
    if req.expected_revision != "absent" && !valid_sha(&req.expected_revision) {
        return Err(storage::bad_request("Invalid assignment revision"));
    }
    let ctx = source(state, &req.project_id, &req.video, &req.replay_id)?;
    if req.track_ids.iter().any(|id| !ctx.tracks.contains_key(id)) {
        return Err(storage::bad_request(
            "Selected Track ID not present in verified CPU replay",
        ));
    }
    if req.action == "assign"
        && !req
            .player_id
            .as_ref()
            .is_some_and(|id| valid_player_id(id) && ctx.roster.contains_key(id))
    {
        return Err(storage::bad_request(
            "Selected athlete is not in project roster",
        ));
    }
    if req.action != "assign" && req.player_id.is_some() {
        return Err(storage::bad_request(
            "Player ID is only accepted for an assignment",
        ));
    }
    let before = existing(&ctx.sidecar).map_err(storage::bad_request)?;
    let old_revision = revision(before.as_deref());
    if old_revision != req.expected_revision {
        return Err(storage::conflict(
            "Track bindings changed; refresh the revision",
        ));
    }
    let mut doc = validated(before.as_deref(), &ctx).map_err(storage::bad_request)?;
    let mut assignments = doc["assignments"]
        .as_object()
        .cloned()
        .ok_or_else(|| storage::bad_request("Missing assignment object"))?;
    let mut excluded = doc["excluded_ids"]
        .as_array()
        .ok_or_else(|| storage::bad_request("Missing excluded list"))?
        .iter()
        .filter_map(Value::as_u64)
        .collect::<BTreeSet<_>>();
    for id in &req.track_ids {
        let key = id.to_string();
        match req.action.as_str() {
            "assign" => {
                if excluded.contains(id) {
                    return Err(storage::conflict(
                        "Restore an excluded Track ID before assignment",
                    ));
                }
                if assignments
                    .get(&key)
                    .and_then(Value::as_str)
                    .is_some_and(|owner| Some(owner) != req.player_id.as_deref())
                {
                    return Err(storage::conflict(
                        "Track ID already belongs to another player; unassign it first",
                    ));
                }
                assignments.insert(key, json!(req.player_id));
            }
            "unassign" => {
                assignments.remove(&key);
            }
            "exclude" => {
                assignments.remove(&key);
                excluded.insert(*id);
            }
            "restore" => {
                excluded.remove(id);
            }
            _ => unreachable!(),
        }
    }
    doc["assignments"] = json!(assignments);
    doc["excluded_ids"] = json!(excluded);
    if before.is_none()
        || doc != validated(before.as_deref(), &ctx).map_err(storage::bad_request)?
    {
        let parent = ctx
            .sidecar
            .parent()
            .ok_or_else(|| storage::internal("Link directory missing"))?;
        match fs::symlink_metadata(parent) {
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                return Err(storage::bad_request("Unsafe assignment directory"))
            }
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => fs::create_dir(parent)
                .map_err(|e| storage::internal(format!("Create assignment folder: {e}")))?,
            Err(e) => return Err(storage::internal(format!("Inspect assignment folder: {e}"))),
        }
        if fs::symlink_metadata(&ctx.sidecar)
            .is_ok_and(|m| m.file_type().is_symlink() || !m.is_file())
        {
            return Err(storage::bad_request("Unsafe Track ID assignment target"));
        }
        storage::write_json_atomic(&ctx.sidecar, &doc)?;
    }
    let new = existing(&ctx.sidecar).map_err(storage::bad_request)?;
    page(
        &ctx,
        &validated(new.as_deref(), &ctx).map_err(storage::bad_request)?,
        &revision(new.as_deref()),
        req.offset,
        req.limit,
    )
    .map_err(storage::bad_request)
}
pub async fn read(
    State(state): State<Arc<AppState>>,
    Query(req): Query<TrackQuery>,
) -> ApiResult<Value> {
    let result = tokio::task::spawn_blocking(move || {
        let _lock = project_archive::archive_guard(&state, &req.project_id)?;
        read_internal(&state, &req)
    })
    .await
    .map_err(|e| storage::internal(format!("Track ID read task: {e}")))??;
    Ok(Json(result))
}
pub async fn edit(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<TrackEdit>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    let result = tokio::task::spawn_blocking(move || {
        let _lock = project_archive::archive_guard(&state, &req.project_id)?;
        change(&state, &req)
    })
    .await
    .map_err(|e| storage::internal(format!("Track ID edit task: {e}")))??;
    Ok(Json(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (PathBuf, Vec<u8>, String) {
        let root = std::env::temp_dir().join(format!("native-ids-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let mut data = Vec::new();
        for frame in 0..18u64 {
            let candidate = Candidate {
                frame_index: frame,
                class_name: "player".into(),
                score: 0.91,
                bbox_xyxy_norm: [0.2, 0.1, 0.4, 0.65],
                query_index: Some(0),
                touch_probability: Some(0.1),
                head_point_norm: None,
            };
            let row = json!({
                "frame_index":frame,"track_ids_provisional":true,
                "confirmed_athlete_identities":false,"touches_confirmed":false,
                "tracked_players":if (1..15).contains(&frame) {
                    json!([{"track_id":7,"detection":candidate,"is_verified_athlete":false}])
                }else{json!([])}
            });
            data.extend(serde_json::to_vec(&row).unwrap());
            data.push(b'\n');
        }
        let digest = format!("{:x}", Sha256::digest(&data));
        let path = root.join("tracked_frames.jsonl");
        fs::write(&path, &data).unwrap();
        (root, data, digest)
    }
    fn context(root: &Path, sha: String, tracks: BTreeMap<u64, Aggregate>) -> Inputs {
        Inputs {
            project_id: "123456abcdef".into(),
            video: "match.mp4".into(),
            replay_id: "0123456789abcdef0123456789abcdef".into(),
            source_sha: sha,
            tracks,
            roster: BTreeMap::from([("abcdef012345".into(), "Ильнур".into())]),
            sidecar: root
                .join("native_track_assignments")
                .join("0123456789abcdef0123456789abcdef.json"),
            can_write: true,
        }
    }
    #[test]
    fn hash_checked_track_index_and_tampering_protection() {
        let (root, mut data, sha) = fixture();
        let path = root.join("tracked_frames.jsonl");
        let tracks = index_replay(&path, &sha, 18).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[&7].observed, 14);
        assert_eq!(tracks[&7].first, 1);
        assert_eq!(tracks[&7].last, 14);
        assert_eq!(tracks[&7].best_frame, 1);
        assert!(index_replay(&path, &"0".repeat(64), 18).is_err());
        data[0] = b' ';
        fs::write(&path, &data).unwrap();
        assert!(index_replay(&path, &sha, 18).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn isolated_project_edits_reject_stale_revision_reassignment_and_excluded_track() {
        use std::process::Command;
        if Command::new("ffmpeg").arg("-version").output().is_err() {
            return;
        }
        let (root, source_bytes, source_sha) = fixture();
        let repo = root.join("repo");
        let data = root.join("sandbox");
        let id = "123456abcdef";
        let video = "match.mp4";
        let replay = "0123456789abcdef0123456789abcdef";
        fs::create_dir_all(repo.join("project")).unwrap();
        let source_video = repo.join("project").join(video);
        assert!(Command::new("ffmpeg")
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
                "-y"
            ])
            .arg(&source_video)
            .status()
            .unwrap()
            .success());
        let replay_dir = data
            .join("projects")
            .join(id)
            .join("native_cpu_replays")
            .join(replay);
        fs::create_dir_all(&replay_dir).unwrap();
        fs::write(replay_dir.join("tracked_frames.jsonl"), &source_bytes).unwrap();
        storage::write_json_atomic(
            &data.join("projects").join("projects.json"),
            &json!({
                "schema":1,"projects":[{"id":id,"name":"Test","videos":[video],
                    "player_ids":["abcdef012345","fedcba654321"]}]
            }),
        )
        .unwrap();
        storage::write_json_atomic(
            &data.join("players").join("registry.json"),
            &json!({
                "schema":1,"players":[
                    {"id":"abcdef012345","name":"Ильнур","created_by":"manual"},
                    {"id":"fedcba654321","name":"Ирек","created_by":"manual"}]
            }),
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
        let prepared = preparation::plan_for(&state, id, video).unwrap();
        let (size, mtime) = inference_stream::source_stamp(prepared.ready_path().unwrap()).unwrap();
        storage::write_json_atomic(
            &replay_dir.join("manifest.json"),
            &json!({
                "schema":"volley-native-cpu-tracking/1","state":"provisional_tracklets_rebuilt",
                "project_id":id,"replay_id":replay,"video":video,
                "source_identity":{"bytes":size,"mtime_ns":mtime},
                "fps":30,"frames":18,"file":"tracked_frames.jsonl","sha256":source_sha,
                "track_ids_provisional":true,"athlete_identities_confirmed":false,
                "touches_confirmed":false,"cpu_processing_complete":false
            }),
        )
        .unwrap();
        let query = TrackQuery {
            project_id: id.into(),
            video: video.into(),
            replay_id: replay.into(),
            offset: 0,
            limit: 10,
        };
        let original = read_internal(&state, &query).unwrap();
        assert_eq!(original["total_tracks"], 1);
        assert_eq!(original["revision"], "absent");
        let assign = TrackEdit {
            project_id: id.into(),
            video: video.into(),
            replay_id: replay.into(),
            action: "assign".into(),
            track_ids: vec![7],
            player_id: Some("abcdef012345".into()),
            expected_revision: "absent".into(),
            offset: 0,
            limit: 10,
        };
        let saved = change(&state, &assign).unwrap();
        assert_eq!(saved["assigned_count"], 1);
        assert_eq!(saved["tracks"][0]["assigned_player_name"], "Ильнур");
        assert_eq!(read_internal(&state, &query).unwrap()["assigned_count"], 1);
        assert_eq!(
            change(&state, &assign).unwrap_err().0,
            axum::http::StatusCode::CONFLICT
        );
        let different = TrackEdit {
            player_id: Some("fedcba654321".into()),
            expected_revision: saved["revision"].as_str().unwrap().into(),
            ..assign.clone()
        };
        assert_eq!(
            change(&state, &different).unwrap_err().0,
            axum::http::StatusCode::CONFLICT
        );
        let excluded = change(
            &state,
            &TrackEdit {
                action: "exclude".into(),
                player_id: None,
                expected_revision: saved["revision"].as_str().unwrap().into(),
                ..assign.clone()
            },
        )
        .unwrap();
        assert_eq!(excluded["excluded_count"], 1);
        assert_eq!(excluded["assigned_count"], 0);
        assert!(change(
            &state,
            &TrackEdit {
                expected_revision: excluded["revision"].as_str().unwrap().into(),
                ..assign.clone()
            }
        )
        .is_err());
        let restored = change(
            &state,
            &TrackEdit {
                action: "restore".into(),
                player_id: None,
                expected_revision: excluded["revision"].as_str().unwrap().into(),
                ..assign.clone()
            },
        )
        .unwrap();
        assert_eq!(restored["excluded_count"], 0);
        assert_eq!(
            fs::read(replay_dir.join("tracked_frames.jsonl")).unwrap(),
            source_bytes
        );
        assert!(!repo.join("results").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_manual_assignments_are_replay_scoped_and_roster_checked() {
        let (root, _, sha) = fixture();
        let tracks = index_replay(&root.join("tracked_frames.jsonl"), &sha, 18).unwrap();
        let ctx = context(&root, sha.clone(), tracks);
        let base = validated(None, &ctx).unwrap();
        assert_eq!(
            page(&ctx, &base, "absent", 0, 40).unwrap()["assigned_count"],
            0
        );
        let mut doc = base.clone();
        doc["assignments"]["7"] = json!("abcdef012345");
        assert_eq!(
            validated(Some(&serde_json::to_vec(&doc).unwrap()), &ctx).unwrap()["assignments"]["7"],
            "abcdef012345"
        );
        let mut wrong = doc.clone();
        wrong["assignments"]["7"] = json!("fedcba654321");
        assert!(validated(Some(&serde_json::to_vec(&wrong).unwrap()), &ctx).is_err());
        let mut wrong = doc.clone();
        wrong["source_replay_sha256"] = json!("0".repeat(64));
        assert!(validated(Some(&serde_json::to_vec(&wrong).unwrap()), &ctx).is_err());
        let mut excluded = doc.clone();
        excluded["excluded_ids"] = json!([7]);
        assert!(validated(Some(&serde_json::to_vec(&excluded).unwrap()), &ctx).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
