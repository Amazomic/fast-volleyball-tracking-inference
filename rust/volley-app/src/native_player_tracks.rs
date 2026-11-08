//! Versioned manual assignment of *provisional* native CPU Track IDs to
//! athlete registry entries. The raw GPU/CPU evidence is NEVER rewritten.
//! ReID appearance vectors / saved athlete photo templates are not generated
//! by this module; each owner association requires an explicit human action.
use crate::{
    cpu_retrack::{opaque_id, safe_file, safe_folder, BoundedReader},
    inference_stream, native_reid_cache, player_catalog, preparation, project_archive, projects,
    storage::{self, ApiError, ApiResult},
    versioned_store::{self, revision},
    AppState,
};
use axum::{
    extract::{Query, State},
    http::HeaderMap,
    Json,
};
use fs2::FileExt;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};
use volley_core::{
    evidence_source::{EvidenceSource, FrameRate, SourceStamp, VerifiedTrackSet},
    fusion::Candidate,
    identity_review::{self, ManualReviewState, ReviewAction, ReviewErrorKind, ReviewedLink},
};

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
    evidence: EvidenceSource,
    known: VerifiedTrackSet,
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
    let evidence = EvidenceSource::native(
        project_id,
        video,
        replay_id,
        &sha,
        SourceStamp {
            bytes: size,
            mtime_ns: u64::try_from(mtime)
                .map_err(|_| storage::bad_request("Video timestamp outside supported range"))?,
        },
        FrameRate::Exact {
            numerator: 30,
            denominator: 1,
        },
    )
    .map_err(storage::bad_request)?;
    let known = VerifiedTrackSet::new(evidence.clone(), tracks.keys().copied())
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
        evidence,
        known,
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
fn existing(sidecar: &Path) -> Result<Option<Vec<u8>>, String> {
    versioned_store::read_optional_bounded(sidecar, MAX_LINKS)
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
        if !ctx.known.contains_number(id) || !excluded_set.insert(id) {
            return Err("Unknown or duplicate excluded Track ID".into());
        }
    }
    for (key, value) in assignments {
        let id = key
            .parse::<u64>()
            .map_err(|_| "Invalid assigned Track ID")?;
        if id.to_string() != *key || !ctx.known.contains_number(id) || excluded_set.contains(&id) {
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
    let reviewed_links = assignments
        .iter()
        .map(|(id, owner)| {
            Ok(ReviewedLink {
                track_id: id.parse::<u64>().map_err(|_| "Invalid reviewed ID")?,
                owner: owner
                    .as_str()
                    .ok_or("Invalid reviewed player ID")?
                    .to_owned(),
                reviewed: true,
            })
        })
        .collect::<Result<Vec<_>, &str>>()?;
    let reviewed =
        identity_review::resolve_reviewed(ctx.known.ids(), &ctx.roster, &reviewed_links, &excluded);
    let mut rows = Vec::new();
    for (id, info) in ctx.tracks.iter().skip(offset).take(limit) {
        let owner = reviewed.owners.get(id).map(String::as_str);
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
        "source_kind":ctx.evidence.source_kind(),
        "evidence_source":ctx.evidence,
        "revision":version,"writable":ctx.can_write,
        "total_tracks":ctx.known.len(),"offset":offset,"limit":limit,
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
    if !versioned_store::valid_revision(&req.expected_revision) {
        return Err(storage::bad_request("Invalid assignment revision"));
    }
    let ctx = source(state, &req.project_id, &req.video, &req.replay_id)?;
    if req
        .track_ids
        .iter()
        .any(|id| !ctx.known.contains_number(*id))
    {
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
    if versioned_store::expect_revision(before.as_deref(), &req.expected_revision).is_err() {
        return Err(storage::conflict(
            "Track bindings changed; refresh the revision",
        ));
    }
    let mut doc = validated(before.as_deref(), &ctx).map_err(storage::bad_request)?;
    // This adapter validates JSON and applies the one human-review policy.
    let assignments = doc["assignments"]
        .as_object()
        .ok_or_else(|| storage::bad_request("Missing assignment object"))?
        .iter()
        .map(|(key, value)| {
            Ok((
                key.parse::<u64>()
                    .map_err(|_| storage::bad_request("Invalid saved Track ID"))?,
                value
                    .as_str()
                    .ok_or_else(|| storage::bad_request("Invalid saved athlete ID"))?
                    .to_owned(),
            ))
        })
        .collect::<Result<BTreeMap<_, _>, ApiError>>()?;
    let excluded = doc["excluded_ids"]
        .as_array()
        .ok_or_else(|| storage::bad_request("Missing excluded list"))?
        .iter()
        .map(|v| {
            v.as_u64()
                .ok_or_else(|| storage::bad_request("Invalid excluded ID"))
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    let mut review = ManualReviewState {
        assignments,
        excluded,
        rejected: BTreeSet::new(),
    };
    review
        .apply(
            &ctx.known,
            &ctx.roster,
            ReviewAction::parse(&req.action).map_err(|e| storage::bad_request(e.message))?,
            &req.track_ids,
            req.player_id.as_deref(),
            false,
        )
        .map_err(|e| match e.kind {
            ReviewErrorKind::Invalid => storage::bad_request(e.message),
            ReviewErrorKind::Conflict => storage::conflict(e.message),
        })?;
    doc["assignments"] = json!(review
        .assignments
        .iter()
        .map(|(id, player)| (id.to_string(), player.clone()))
        .collect::<BTreeMap<_, _>>());
    doc["excluded_ids"] = json!(review.excluded);
    if before.is_none()
        || doc != validated(before.as_deref(), &ctx).map_err(storage::bad_request)?
    {
        let parent = ctx
            .sidecar
            .parent()
            .ok_or_else(|| storage::internal("Link directory missing"))?;
        versioned_store::write_json_checked(parent, &ctx.sidecar, &doc, MAX_LINKS)
            .map_err(storage::bad_request)?;
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

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhotoQuery {
    project_id: String,
    video: String,
    replay_id: String,
    track_id: u64,
}

fn crop_box(norm: [f64; 4], video_width: u32, video_height: u32) -> Result<[u32; 4], String> {
    if video_width < 64
        || video_height < 64
        || video_width > 16384
        || video_height > 16384
        || !norm.iter().all(|v| v.is_finite())
        || norm[0] < 0.0
        || norm[1] < 0.0
        || norm[2] > 1.0
        || norm[3] > 1.0
        || norm[2] <= norm[0]
        || norm[3] <= norm[1]
    {
        return Err("Invalid native player bounding box".into());
    }
    let pad_x = (norm[2] - norm[0]) * 0.12;
    let pad_y = (norm[3] - norm[1]) * 0.12;
    let left = ((norm[0] - pad_x).max(0.0) * video_width as f64).floor() as u32;
    let top = ((norm[1] - pad_y).max(0.0) * video_height as f64).floor() as u32;
    let right = ((norm[2] + pad_x).min(1.0) * video_width as f64).ceil() as u32;
    let bottom = ((norm[3] + pad_y).min(1.0) * video_height as f64).ceil() as u32;
    let width = right.min(video_width).saturating_sub(left);
    let height = bottom.min(video_height).saturating_sub(top);
    if width < 12 || height < 12 {
        return Err("Player detection is too small for photo preview".into());
    }
    Ok([left, top, width, height])
}
pub async fn photo(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PhotoQuery>,
) -> Result<axum::response::Response, ApiError> {
    let plan_state = state.clone();
    let photo_plan = tokio::task::spawn_blocking(move || -> Result<_, ApiError> {
        let _lock = project_archive::archive_guard(&plan_state, &query.project_id)?;
        let inputs = source(
            &plan_state,
            &query.project_id,
            &query.video,
            &query.replay_id,
        )?;
        let track = inputs
            .tracks
            .get(&query.track_id)
            .ok_or_else(|| storage::bad_request("Track ID absent from saved CPU replay"))?;
        let plan = preparation::plan_for(&plan_state, &query.project_id, &query.video)?;
        let prepared = plan
            .ready_path()
            .ok_or_else(|| storage::bad_request("Native preview video unavailable"))?;
        let video = volley_media::probe(prepared).map_err(storage::bad_request)?;
        let crop =
            crop_box(track.best_box, video.width, video.height).map_err(storage::bad_request)?;
        let stamp = inference_stream::source_stamp(prepared).map_err(storage::bad_request)?;
        Ok((prepared.to_path_buf(), track.best_frame, crop, stamp))
    })
    .await
    .map_err(|e| storage::internal(format!("Player photo verification task: {e}")))??;
    let _permit = state
        .preview_limit
        .acquire()
        .await
        .map_err(|e| storage::internal(format!("Photo decode slot: {e}")))?;
    let (prepared, frame, [x, y, width, height], stamp) = photo_plan;
    let time = format!("{:.4}", frame as f64 / 30.0);
    let vf = format!("crop={width}:{height}:{x}:{y},scale=240:-2,setsar=1");
    // Direct exec with validated numeric crop coordinates, never shell interpolation.
    let mut cmd = tokio::process::Command::new("ffmpeg");
    cmd.args([
        "-nostdin",
        "-hide_banner",
        "-loglevel",
        "error",
        "-ss",
        &time,
        "-i",
    ]);
    cmd.arg(&prepared);
    cmd.args([
        "-vf",
        &vf,
        "-frames:v",
        "1",
        "-an",
        "-c:v",
        "mjpeg",
        "-q:v",
        "4",
        "-f",
        "image2pipe",
        "pipe:1",
    ]);
    cmd.kill_on_drop(true);
    let bytes = tokio::time::timeout(std::time::Duration::from_secs(35), cmd.output())
        .await
        .map_err(|_| storage::bad_request("Player preview extraction timed out"))?
        .map_err(|e| storage::bad_request(format!("FFmpeg player crop: {e}")))?;
    if !bytes.status.success()
        || bytes.stdout.len() < 100
        || bytes.stdout.len() > 3_000_000
        || !bytes.stdout.starts_with(&[0xff, 0xd8])
    {
        return Err(storage::bad_request(
            "Unable to render native Track ID photo",
        ));
    }
    if inference_stream::source_stamp(&prepared).map_err(storage::bad_request)? != stamp {
        return Err(storage::conflict(
            "Source video changed while producing photo preview",
        ));
    }
    use axum::{body::Body, http::header};
    axum::response::Response::builder()
        .header(header::CONTENT_TYPE, "image/jpeg")
        .header(header::CACHE_CONTROL, "private, no-store")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(Body::from(bytes.stdout))
        .map_err(|e| storage::internal(format!("Player photo response: {e}")))
}

/// Explicit, bounded ReID feature extraction. No implicit athlete assignment,
/// no persistence and no reuse of legacy cached embeddings. The source replay
/// and model files must remain identical throughout the request.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingRequest {
    project_id: String,
    video: String,
    replay_id: String,
    expected_source_sha256: String,
    track_ids: Vec<u64>,
}

pub async fn embeddings(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<EmbeddingRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    if !valid_sha(&req.expected_source_sha256)
        || req.track_ids.is_empty()
        || req.track_ids.len() > 8
        || req.track_ids.iter().collect::<BTreeSet<_>>().len() != req.track_ids.len()
    {
        return Err(storage::bad_request(
            "Select 1–8 unique native Track IDs with exact source SHA-256",
        ));
    }
    let _reid_slot = state
        .reid_limit
        .acquire()
        .await
        .map_err(|e| storage::internal(format!("ReID gate: {e}")))?;
    // The exact current model fingerprint is required even for a cache hit.
    let repo = state.repository_root.clone();
    let before =
        volley_inference::reid_openvino::fingerprint(&repo).map_err(storage::bad_request)?;
    let cache_scope = native_reid_cache::CacheScope {
        project_id: req.project_id.clone(),
        video: req.video.clone(),
        replay_id: req.replay_id.clone(),
        replay_sha256: req.expected_source_sha256.clone(),
        model_fingerprint: before.clone(),
    };
    let expected_sha = req.expected_source_sha256.clone();
    let planned_state = state.clone();
    let (source_file, source_stamp, replay_file, replay_stamp, crops, evidence) =
        tokio::task::spawn_blocking(move || -> Result<_, ApiError> {
            let _guard = project_archive::archive_guard(&planned_state, &req.project_id)?;
            let ctx = source(&planned_state, &req.project_id, &req.video, &req.replay_id)?;
            if ctx.source_sha != req.expected_source_sha256 {
                return Err(storage::conflict(
                    "CPU replay changed; refresh the native Track IDs",
                ));
            }
            let plan = preparation::plan_for(&planned_state, &req.project_id, &req.video)?;
            let file = plan
                .ready_path()
                .ok_or_else(|| storage::bad_request("Verified video is unavailable"))?;
            let video = volley_media::probe(file).map_err(storage::bad_request)?;
            let mut crops = Vec::new();
            for id in &req.track_ids {
                if !ctx.known.contains_number(*id) {
                    return Err(storage::bad_request(
                        "Track ID not found in this verified replay",
                    ));
                }
                let info = ctx.tracks.get(id).ok_or_else(|| {
                    storage::bad_request("Track ID lacks a recorded bounding box")
                })?;
                let crop = crop_box(info.best_box, video.width, video.height)
                    .map_err(storage::bad_request)?;
                crops.push((*id, info.best_frame, crop));
            }
            let stamp = inference_stream::source_stamp(file).map_err(storage::bad_request)?;
            let replay_dir = projects::project_root(&planned_state)
                .join(&req.project_id)
                .join("native_cpu_replays")
                .join(&req.replay_id);
            let replay_file = safe_file(&replay_dir, "tracked_frames.jsonl", MAX_EVIDENCE)
                .map_err(storage::bad_request)?;
            let stat = fs::metadata(&replay_file)
                .map_err(|e| storage::bad_request(format!("CPU replay disappeared: {e}")))?;
            let replay_stamp = (stat.len(), stat.modified().ok());
            Ok((
                file.to_path_buf(),
                stamp,
                replay_file,
                replay_stamp,
                crops,
                ctx.evidence,
            ))
        })
        .await
        .map_err(|e| storage::internal(format!("ReID source check task: {e}")))??;
    let cache =
        native_reid_cache::read(&state.data_root, &cache_scope).map_err(storage::bad_request)?;
    let cache_invalidated = cache.invalidated;
    let mut selected = BTreeMap::<u64, Vec<f32>>::new();
    for (id, _, _) in &crops {
        if let Some(vector) = cache.vectors.get(id) {
            selected.insert(*id, vector.clone());
        }
    }
    let cached_count = selected.len();
    let _decode_slot = state
        .preview_limit
        .acquire()
        .await
        .map_err(|e| storage::internal(format!("ReID media slot: {e}")))?;
    let mut frames = Vec::with_capacity(crops.len());
    for &(track_id, best_frame, [x, y, w, h]) in &crops {
        if selected.contains_key(&track_id) {
            continue;
        }
        let secs = format!("{:.6}", best_frame as f64 / 30.0);
        let vf = format!(
            "crop={w}:{h}:{x}:{y},scale={}:{}:flags=bilinear,format=bgr24",
            volley_inference::reid_openvino::WIDTH,
            volley_inference::reid_openvino::HEIGHT
        );
        let mut command = tokio::process::Command::new("ffmpeg");
        command
            .args([
                "-nostdin",
                "-hide_banner",
                "-loglevel",
                "error",
                "-ss",
                &secs,
                "-i",
            ])
            .arg(&source_file)
            .args([
                "-vf",
                &vf,
                "-frames:v",
                "1",
                "-an",
                "-pix_fmt",
                "bgr24",
                "-f",
                "rawvideo",
                "pipe:1",
            ]);
        command.kill_on_drop(true);
        let result = tokio::time::timeout(std::time::Duration::from_secs(35), command.output())
            .await
            .map_err(|_| storage::bad_request("ReID video crop timed out"))?
            .map_err(|e| storage::bad_request(format!("FFmpeg ReID crop: {e}")))?;
        if !result.status.success()
            || result.stdout.len() != volley_inference::reid_openvino::PIXELS * 3
        {
            return Err(storage::bad_request(
                "Cannot decode an exact BGR Track ID crop",
            ));
        }
        let chw = volley_inference::reid_openvino::bgr24_to_nchw(&result.stdout)
            .map_err(storage::bad_request)?;
        frames.push((track_id, best_frame, chw));
    }
    let computed = if frames.is_empty() {
        Vec::<(u64, Vec<f32>)>::new()
    } else {
        // Only an actual cache miss needs the native OpenVINO runtime.
        let library = std::env::var("VOLLEY_OPENVINO_C_LIB").map_err(|_| {
            storage::bad_request(
                "Native OpenVINO C runtime is not configured: set VOLLEY_OPENVINO_C_LIB",
            )
        })?;
        tokio::task::spawn_blocking(move || -> Result<Vec<(u64, Vec<f32>)>, ApiError> {
            let mut encoder =
                volley_inference::reid_openvino::Encoder::load(&repo, Path::new(&library))
                    .map_err(storage::bad_request)?;
            let mut output = Vec::new();
            for (id, _, chw) in frames {
                let vector = encoder.infer(&chw).map_err(storage::bad_request)?;
                output.push((id, vector));
            }
            Ok(output)
        })
        .await
        .map_err(|e| storage::internal(format!("ReID native inference task: {e}")))??
    };
    let replay_stat = fs::metadata(&replay_file)
        .map_err(|_| storage::conflict("CPU replay disappeared during ReID inference"))?;
    if (replay_stat.len(), replay_stat.modified().ok()) != replay_stamp
        || inference_stream::source_stamp(&source_file).map_err(storage::bad_request)?
            != source_stamp
        || volley_inference::reid_openvino::fingerprint(&state.repository_root)
            .map_err(storage::bad_request)?
            != before
    {
        return Err(storage::conflict(
            "Video or ReID weights changed during feature extraction",
        ));
    }
    let computed_count = computed.len();
    let persisted = if state.writes_enabled && !computed.is_empty() {
        let writer = state.clone();
        let scope = cache_scope.clone();
        let vectors = computed.clone();
        let source_path = source_file.clone();
        let replay_path = replay_file.clone();
        tokio::task::spawn_blocking(move || -> Result<(), ApiError> {
            let _guard = project_archive::archive_guard(&writer, &scope.project_id)?;
            let stat = fs::metadata(&replay_path)
                .map_err(|_| storage::conflict("ReID replay disappeared before cache commit"))?;
            if (stat.len(), stat.modified().ok()) != replay_stamp
                || inference_stream::source_stamp(&source_path).map_err(storage::bad_request)?
                    != source_stamp
                || volley_inference::reid_openvino::fingerprint(&writer.repository_root)
                    .map_err(storage::bad_request)?
                    != scope.model_fingerprint
            {
                return Err(storage::conflict(
                    "Native ReID source/model changed before cache commit",
                ));
            }
            native_reid_cache::merge(&writer.data_root, &scope, &vectors)
                .map_err(storage::bad_request)?;
            Ok(())
        })
        .await
        .map_err(|e| storage::internal(format!("Native ReID cache task: {e}")))??;
        true
    } else {
        false
    };
    for (id, vector) in computed {
        selected.insert(id, vector);
    }
    let generated = crops
        .iter()
        .map(|(id, frame, _)| -> Result<Value, ApiError> {
            let vector = selected.get(id).ok_or_else(|| {
                storage::internal("Verified ReID feature missing from source/cache")
            })?;
            Ok(json!({
                "track_id":id,
                "best_frame":frame,
                "dimensions":vector.len(),
                "vector":vector,
                "cache_hit":cache.vectors.contains_key(id),
                "confirmed_athlete_identity":false
            }))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let source_kind = evidence.source_kind();
    Ok(Json(json!({
        "schema":"volley-native-reid-features/1",
        "source_replay_sha256":expected_sha,
        "evidence_source":evidence,
        "model":volley_inference::reid_openvino::MODEL_NAME,
        "model_fingerprint":before,
        "preprocessing":volley_inference::reid_openvino::PREPROCESS,
        "source_kind":source_kind,
        "dimensions":volley_inference::reid_openvino::DIMENSIONS,
        "embedding_count":generated.len(),
        "cache_hits":cached_count,
        "computed_count":computed_count,
        "cache_invalidated":cache_invalidated,
        "cache_persisted":persisted,
        "features":generated,
        "similarity_type":"cosine_similarity_not_probability",
        "identities_confirmed":false,
        "automatic_reid_binding":false
    })))
}

/// A real photo may enter an athlete's global album only after an explicit
/// manual Track ID link has already been committed in this exact CPU replay.
/// Photo references never mutate that identity link or any model evidence.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceRequest {
    project_id: String,
    player_id: String,
    action: String, // read, enroll, undo
    #[serde(default)]
    expected_revision: Option<String>,
    #[serde(default)]
    video: Option<String>,
    #[serde(default)]
    replay_id: Option<String>,
    #[serde(default)]
    track_id: Option<u64>,
    #[serde(default)]
    expected_source_sha256: Option<String>,
    #[serde(default)]
    reference_id: Option<String>,
}
const MAX_REFERENCE_INDEX: u64 = 256 * 1024;
const MAX_PHOTO_BYTES: usize = 3_000_000;
const MAX_PLAYER_PHOTOS: usize = 64;

fn checked_reference_folder(
    state: &AppState,
    player_id: &str,
    create: bool,
) -> Result<PathBuf, ApiError> {
    if !valid_player_id(player_id) {
        return Err(storage::bad_request("Invalid global athlete ID"));
    }
    if create && !state.writes_enabled {
        return Err(storage::conflict(
            "Saving athlete photos requires isolated Rust storage",
        ));
    }
    let base = state.data_root.join("players");
    let references = base.join("native_reference_photos");
    let target = references.join(player_id);
    for folder in [&base, &references, &target] {
        match fs::symlink_metadata(folder) {
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                return Err(storage::bad_request("Unsafe athlete photo directory"));
            }
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && create => {
                fs::create_dir(folder)
                    .map_err(|e| storage::internal(format!("Create reference directory: {e}")))?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(target.clone()),
            Err(e) => {
                return Err(storage::internal(format!("Check reference directory: {e}")));
            }
        }
    }
    Ok(target)
}
fn check_reference_id(id: &str) -> bool {
    id.len() == 24
        && id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
fn references_doc(raw: Option<&[u8]>, player_id: &str) -> Result<Value, ApiError> {
    let Some(bytes) = raw else {
        return Ok(json!({"schema":"volley-native-player-references/1",
            "player_id":player_id,"photos":[]}));
    };
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| storage::bad_request("Corrupted athlete photo index"))?;
    if value["schema"] != "volley-native-player-references/1"
        || value["player_id"] != player_id
        || value["photos"]
            .as_array()
            .is_none_or(|v| v.len() > MAX_PLAYER_PHOTOS)
    {
        return Err(storage::bad_request(
            "Invalid athlete reference photo schema",
        ));
    }
    let mut seen = BTreeSet::new();
    for photo in value["photos"].as_array().expect("validated photos array") {
        let id = photo["id"]
            .as_str()
            .ok_or_else(|| storage::bad_request("Photo ID missing"))?;
        if !check_reference_id(id)
            || !seen.insert(id.to_owned())
            || photo["filename"] != format!("ref-{id}.jpg")
            || photo["image_sha256"].as_str().is_none_or(|v| !valid_sha(v))
            || photo["source_replay_sha256"]
                .as_str()
                .is_none_or(|v| !valid_sha(v))
            || photo["manually_enrolled"] != true
            || photo["track_id"].as_u64().is_none_or(|n| n == 0)
        {
            return Err(storage::bad_request("Invalid saved reference photo"));
        }
    }
    Ok(value)
}
fn reference_player(state: &AppState, project_id: &str, player_id: &str) -> Result<(), ApiError> {
    storage::project_id(project_id)?;
    if !valid_player_id(player_id) {
        return Err(storage::bad_request("Invalid athlete ID"));
    }
    let project = projects::existing_project(state, project_id)?;
    if !project["player_ids"]
        .as_array()
        .is_some_and(|rows| rows.iter().any(|r| r.as_str() == Some(player_id)))
    {
        return Err(storage::bad_request(
            "Athlete does not belong to selected project",
        ));
    }
    let (catalog, _) = player_catalog::load(state, false)?;
    if !catalog["players"]
        .as_array()
        .is_some_and(|rows| rows.iter().any(|r| r["id"] == player_id))
    {
        return Err(storage::conflict(
            "Athlete was removed from the global roster",
        ));
    }
    Ok(())
}
fn verified_reference_source(state: &AppState, req: &ReferenceRequest) -> Result<(), ApiError> {
    let video = req
        .video
        .as_deref()
        .ok_or_else(|| storage::bad_request("Select a source video for enrollment"))?;
    let replay_id = req
        .replay_id
        .as_deref()
        .ok_or_else(|| storage::bad_request("Select a native replay"))?;
    let id = req
        .track_id
        .ok_or_else(|| storage::bad_request("Select a native Track ID"))?;
    let sha = req
        .expected_source_sha256
        .as_deref()
        .filter(|s| valid_sha(s))
        .ok_or_else(|| storage::bad_request("Supply verified CPU replay SHA-256"))?;
    let ctx = source(state, &req.project_id, video, replay_id)?;
    if ctx.source_sha != sha || !ctx.known.contains_number(id) {
        return Err(storage::conflict(
            "Source replay changed; repeat manual photo enrollment",
        ));
    }
    let bytes = existing(&ctx.sidecar).map_err(storage::bad_request)?;
    let document = validated(bytes.as_deref(), &ctx).map_err(storage::bad_request)?;
    if document["assignments"][id.to_string()].as_str() != Some(&req.player_id) {
        return Err(storage::conflict(
            "The selected Track ID must first be explicitly reviewed and linked to this athlete",
        ));
    }
    Ok(())
}
fn reference_response(state: &AppState, req: &ReferenceRequest) -> Result<Value, ApiError> {
    reference_player(state, &req.project_id, &req.player_id)?;
    let folder = checked_reference_folder(state, &req.player_id, false)?;
    let raw =
        versioned_store::read_optional_bounded(&folder.join("index.json"), MAX_REFERENCE_INDEX)
            .map_err(storage::bad_request)?;
    let doc = references_doc(raw.as_deref(), &req.player_id)?;
    Ok(json!({
        "schema":"volley-native-player-references/1",
        "project_id":req.project_id,"player_id":req.player_id,
        "revision":revision(raw.as_deref()),
        "photos":doc["photos"],"count":doc["photos"].as_array().map_or(0,Vec::len),
        "writable":state.writes_enabled,
        "automatic_reid_binding":false
    }))
}
fn edit_references(
    state: &AppState,
    req: &ReferenceRequest,
    jpeg: Option<&[u8]>,
) -> Result<Value, ApiError> {
    if !state.writes_enabled {
        return Err(storage::conflict(
            "Athlete photos require isolated Rust storage",
        ));
    }
    reference_player(state, &req.project_id, &req.player_id)?;
    let expected = req
        .expected_revision
        .as_deref()
        .filter(|s| versioned_store::valid_revision(s))
        .ok_or_else(|| {
            storage::bad_request("Read photo album and provide its expected_revision")
        })?;
    let _project_lock = if req.action == "enroll" {
        Some(project_archive::archive_guard(state, &req.project_id)?)
    } else {
        None
    };
    if req.action == "enroll" {
        verified_reference_source(state, req)?;
    }
    let folder = checked_reference_folder(state, &req.player_id, true)?;
    let lockfile = folder.join(".references.lock");
    if fs::symlink_metadata(&lockfile)
        .ok()
        .is_some_and(|m| m.file_type().is_symlink())
    {
        return Err(storage::bad_request("Photo album lock is a symlink"));
    }
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&lockfile)
        .map_err(|e| storage::internal(format!("Reference lock open: {e}")))?;
    lock.lock_exclusive()
        .map_err(|e| storage::internal(format!("Reference lock: {e}")))?;
    let index = folder.join("index.json");
    let bytes = versioned_store::read_optional_bounded(&index, MAX_REFERENCE_INDEX)
        .map_err(storage::bad_request)?;
    if versioned_store::expect_revision(bytes.as_deref(), expected).is_err() {
        return Err(storage::conflict(
            "Photo album changed; refresh its revision",
        ));
    }
    let mut doc = references_doc(bytes.as_deref(), &req.player_id)?;
    let items = doc["photos"].as_array_mut().expect("validated photo album");
    match req.action.as_str() {
        "enroll" => {
            if items.len() >= MAX_PLAYER_PHOTOS {
                return Err(storage::conflict(
                    "Athlete already has the maximum number of reference photos",
                ));
            }
            let jpeg = jpeg.ok_or_else(|| storage::bad_request("No verified player photo"))?;
            if jpeg.len() < 100
                || jpeg.len() > MAX_PHOTO_BYTES
                || !jpeg.starts_with(&[0xff, 0xd8])
                || !jpeg.ends_with(&[0xff, 0xd9])
            {
                return Err(storage::bad_request("Invalid JPEG reference photo"));
            }
            let sha = format!("{:x}", Sha256::digest(jpeg));
            let source = req
                .expected_source_sha256
                .as_ref()
                .expect("verified source");
            let tid = req.track_id.expect("verified Track ID");
            let mut digest = Sha256::new();
            digest.update(req.player_id.as_bytes());
            digest.update(req.project_id.as_bytes());
            digest.update(req.video.as_ref().expect("verified video").as_bytes());
            digest.update(req.replay_id.as_ref().expect("verified replay").as_bytes());
            digest.update(tid.to_le_bytes());
            digest.update(source.as_bytes());
            digest.update(sha.as_bytes());
            let id = format!("{:x}", digest.finalize())[..24].to_owned();
            if items.iter().any(|v| v["id"] == id) {
                return Err(storage::conflict(
                    "This exact photo is already in the athlete album",
                ));
            }
            let filename = format!("ref-{id}.jpg");
            let path = folder.join(&filename);
            let mut out = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|e| {
                    storage::conflict(format!(
                        "Reference photo already exists or cannot be created: {e}"
                    ))
                })?;
            let created = (|| -> std::io::Result<()> {
                out.write_all(jpeg)?;
                out.sync_all()
            })();
            drop(out);
            if let Err(e) = created {
                let _ = fs::remove_file(&path);
                return Err(storage::internal(format!("Write reference photo: {e}")));
            }
            items.push(json!({
                "id":id,"filename":filename,"image_sha256":sha,
                "project_id":req.project_id,"video":req.video,"replay_id":req.replay_id,
                "track_id":tid,"source_replay_sha256":source,
                "manually_enrolled":true,"saved_utc":chrono::Utc::now().to_rfc3339(),
            }));
            if let Err(e) =
                versioned_store::write_json_checked(&folder, &index, &doc, MAX_REFERENCE_INDEX)
            {
                let _ = fs::remove_file(path);
                return Err(storage::bad_request(e));
            }
        }
        "undo" => {
            let id = req
                .reference_id
                .as_deref()
                .filter(|s| check_reference_id(s))
                .ok_or_else(|| storage::bad_request("Supply reference_id to undo"))?;
            let before = items.len();
            items.retain(|v| v["id"].as_str() != Some(id));
            if items.len() == before {
                return Err(storage::conflict("Photo reference was already removed"));
            }
            versioned_store::write_json_checked(&folder, &index, &doc, MAX_REFERENCE_INDEX)
                .map_err(storage::bad_request)?;
            // The index is authoritative; removing its entry immediately
            // prevents deleted photos from being used for ReID enrollment.
            let path = folder.join(format!("ref-{id}.jpg"));
            if let Ok(meta) = fs::symlink_metadata(&path) {
                if meta.file_type().is_symlink() || !meta.is_file() {
                    return Err(storage::bad_request("Unsafe leftover reference photo"));
                }
                fs::remove_file(&path)
                    .map_err(|e| storage::internal(format!("Unlink old reference photo: {e}")))?;
            }
        }
        _ => return Err(storage::bad_request("Unknown photo reference action")),
    }
    // Return the revision AFTER the atomic index write, including the entire
    // reviewed album snapshot. Neither source evidence nor assignments changed.
    let new = versioned_store::read_optional_bounded(&index, MAX_REFERENCE_INDEX)
        .map_err(storage::bad_request)?;
    let verified = references_doc(new.as_deref(), &req.player_id)?;
    Ok(json!({
        "schema":"volley-native-player-references/1",
        "project_id":req.project_id,"player_id":req.player_id,
        "revision":revision(new.as_deref()),
        "photos":verified["photos"],"count":verified["photos"].as_array().map_or(0,Vec::len),
        "writable":true,"automatic_reid_binding":false
    }))
}
pub async fn references(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<ReferenceRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    if !matches!(req.action.as_str(), "read" | "enroll" | "undo") {
        return Err(storage::bad_request(
            "Expected photo action read, enroll or undo",
        ));
    }
    let _slot = state
        .identity_limit
        .acquire()
        .await
        .map_err(|e| storage::internal(format!("Photo reference gate: {e}")))?;
    if req.action == "read" {
        let snapshot = state.clone();
        return Ok(Json(
            tokio::task::spawn_blocking(move || reference_response(&snapshot, &req))
                .await
                .map_err(|e| storage::internal(format!("Reference read task: {e}")))??,
        ));
    }
    if req.action == "enroll" {
        if !state.writes_enabled {
            return Err(storage::conflict(
                "Photo enrollment requires isolated Rust storage",
            ));
        }
        let snapshot = state.clone();
        let source_req = req.clone();
        tokio::task::spawn_blocking(move || {
            let _lock = project_archive::archive_guard(&snapshot, &source_req.project_id)?;
            reference_player(&snapshot, &source_req.project_id, &source_req.player_id)?;
            verified_reference_source(&snapshot, &source_req)
        })
        .await
        .map_err(|e| storage::internal(format!("Photo source verification: {e}")))??;
        let photo = photo(
            State(state.clone()),
            Query(PhotoQuery {
                project_id: req.project_id.clone(),
                video: req.video.clone().expect("verified photo video"),
                replay_id: req.replay_id.clone().expect("verified photo replay"),
                track_id: req.track_id.expect("verified photo ID"),
            }),
        )
        .await?;
        let bytes = axum::body::to_bytes(photo.into_body(), MAX_PHOTO_BYTES)
            .await
            .map_err(|_| storage::bad_request("Photo is larger than allowed"))?;
        let snapshot = state.clone();
        let reply =
            tokio::task::spawn_blocking(move || edit_references(&snapshot, &req, Some(&bytes)))
                .await
                .map_err(|e| storage::internal(format!("Photo enrollment task: {e}")))??;
        return Ok(Json(reply));
    }
    let snapshot = state.clone();
    let reply = tokio::task::spawn_blocking(move || edit_references(&snapshot, &req, None))
        .await
        .map_err(|e| storage::internal(format!("Photo removal task: {e}")))??;
    Ok(Json(reply))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferencePhotoQuery {
    project_id: String,
    player_id: String,
    reference_id: String,
}
pub async fn reference_photo(
    State(state): State<Arc<AppState>>,
    Query(req): Query<ReferencePhotoQuery>,
) -> Result<axum::response::Response, ApiError> {
    let snapshot = state.clone();
    let content = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, ApiError> {
        reference_player(&snapshot, &req.project_id, &req.player_id)?;
        if !check_reference_id(&req.reference_id) {
            return Err(storage::bad_request("Invalid reference photo ID"));
        }
        let folder = checked_reference_folder(&snapshot, &req.player_id, false)?;
        let raw =
            versioned_store::read_optional_bounded(&folder.join("index.json"), MAX_REFERENCE_INDEX)
                .map_err(storage::bad_request)?;
        let doc = references_doc(raw.as_deref(), &req.player_id)?;
        let item = doc["photos"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|v| v["id"].as_str() == Some(&req.reference_id))
            .ok_or_else(|| storage::not_found("Reference photo was removed"))?;
        let path = folder.join(format!("ref-{}.jpg", req.reference_id));
        let bytes = versioned_store::read_optional_bounded(&path, MAX_PHOTO_BYTES as u64)
            .map_err(storage::bad_request)?
            .ok_or_else(|| storage::conflict("Reference JPEG vanished"))?;
        if bytes.len() < 100
            || bytes.len() > MAX_PHOTO_BYTES
            || !bytes.starts_with(&[0xff, 0xd8])
            || !bytes.ends_with(&[0xff, 0xd9])
            || format!("{:x}", Sha256::digest(&bytes))
                != item["image_sha256"].as_str().unwrap_or("")
        {
            return Err(storage::conflict("Reference photo bytes are corrupt"));
        }
        Ok(bytes)
    })
    .await
    .map_err(|e| storage::internal(format!("Read photo task: {e}")))??;
    use axum::{body::Body, http::header};
    axum::response::Response::builder()
        .header(header::CONTENT_TYPE, "image/jpeg")
        .header(header::CACHE_CONTROL, "private, no-store")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(Body::from(content))
        .map_err(|e| storage::internal(format!("Reference photo response: {e}")))
}
#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
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
        let evidence = EvidenceSource::native(
            "123456abcdef",
            "match.mp4",
            "0123456789abcdef0123456789abcdef",
            &sha,
            SourceStamp {
                bytes: 100,
                mtime_ns: 100,
            },
            FrameRate::Exact {
                numerator: 30,
                denominator: 1,
            },
        )
        .unwrap();
        let known = VerifiedTrackSet::new(evidence.clone(), tracks.keys().copied()).unwrap();
        Inputs {
            evidence,
            known,
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
        assert_eq!(original["source_kind"], "native_gpu_cpu_replay");
        assert_eq!(
            original["evidence_source"]["tracked_frames_sha256"],
            source_sha
        );
        assert_eq!(original["evidence_source"]["frame_rate"]["numerator"], 30);
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
        assert_eq!(saved["evidence_source"], original["evidence_source"]);
        let sidecar = data
            .join("projects")
            .join(id)
            .join("native_track_assignments")
            .join(format!("{replay}.json"));
        let stored: Value = serde_json::from_slice(&fs::read(sidecar).unwrap()).unwrap();
        assert!(stored.get("evidence_source").is_none());
        assert_eq!(stored["source_replay_sha256"], source_sha);
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
        assert!(verified_reference_source(
            &state,
            &ReferenceRequest {
                project_id: id.into(),
                player_id: "abcdef012345".into(),
                action: "enroll".into(),
                expected_revision: Some("absent".into()),
                video: Some(video.into()),
                replay_id: Some(replay.into()),
                track_id: Some(7),
                expected_source_sha256: Some(source_sha.clone()),
                reference_id: None
            }
        )
        .is_err());
        let reassigned = change(
            &state,
            &TrackEdit {
                expected_revision: restored["revision"].as_str().unwrap().into(),
                ..assign.clone()
            },
        )
        .unwrap();
        assert_eq!(reassigned["assigned_count"], 1);
        assert!(!repo.join("results").exists());
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let shared = Arc::new(state);
        runtime.block_on(async {
            let jpeg = photo(
                State(shared.clone()),
                Query(PhotoQuery {
                    project_id: id.into(),
                    video: video.into(),
                    replay_id: replay.into(),
                    track_id: 7,
                }),
            )
            .await
            .unwrap();
            assert_eq!(
                jpeg.headers()[axum::http::header::CONTENT_TYPE],
                "image/jpeg"
            );
            let bytes = axum::body::to_bytes(jpeg.into_body(), 3_000_000)
                .await
                .unwrap();
            assert!(bytes.starts_with(&[0xff, 0xd8]));
            assert!(bytes.len() > 100);
            let read_album = ReferenceRequest {
                project_id: id.into(),
                player_id: "abcdef012345".into(),
                action: "read".into(),
                expected_revision: None,
                video: None,
                replay_id: None,
                track_id: None,
                expected_source_sha256: None,
                reference_id: None,
            };
            let empty = references(
                State(shared.clone()),
                HeaderMap::new(),
                Json(read_album.clone()),
            )
            .await
            .unwrap()
            .0;
            assert_eq!(empty["revision"], "absent");
            assert_eq!(empty["count"], 0);
            let enroll = ReferenceRequest {
                action: "enroll".into(),
                expected_revision: Some("absent".into()),
                video: Some(video.into()),
                replay_id: Some(replay.into()),
                track_id: Some(7),
                expected_source_sha256: Some(source_sha.clone()),
                ..read_album.clone()
            };
            let album = references(
                State(shared.clone()),
                HeaderMap::new(),
                Json(enroll.clone()),
            )
            .await
            .unwrap()
            .0;
            assert_eq!(album["count"], 1);
            assert_eq!(album["photos"][0]["manually_enrolled"], true);
            assert_eq!(album["photos"][0]["track_id"], 7);
            assert_eq!(album["photos"][0]["source_replay_sha256"], source_sha);
            let photo_id = album["photos"][0]["id"].as_str().unwrap().to_owned();
            let stored_photo = reference_photo(
                State(shared.clone()),
                Query(ReferencePhotoQuery {
                    project_id: id.into(),
                    player_id: "abcdef012345".into(),
                    reference_id: photo_id.clone(),
                }),
            )
            .await
            .unwrap();
            let copied = axum::body::to_bytes(stored_photo.into_body(), MAX_PHOTO_BYTES)
                .await
                .unwrap();
            assert!(copied.starts_with(&[0xff, 0xd8]));
            assert!(copied.ends_with(&[0xff, 0xd9]));
            assert_eq!(
                format!("{:x}", Sha256::digest(&copied)),
                album["photos"][0]["image_sha256"].as_str().unwrap()
            );
            let album_path = data.join("players/native_reference_photos/abcdef012345");
            assert!(album_path.join(format!("ref-{photo_id}.jpg")).is_file());
            assert!(references(
                State(shared.clone()),
                HeaderMap::new(),
                Json(enroll.clone())
            )
            .await
            .is_err());
            assert!(references(
                State(shared.clone()),
                HeaderMap::new(),
                Json(ReferenceRequest {
                    expected_revision: Some(album["revision"].as_str().unwrap().into()),
                    expected_source_sha256: Some("0".repeat(64)),
                    ..enroll.clone()
                })
            )
            .await
            .is_err());
            let undone = references(
                State(shared.clone()),
                HeaderMap::new(),
                Json(ReferenceRequest {
                    action: "undo".into(),
                    expected_revision: Some(album["revision"].as_str().unwrap().into()),
                    video: None,
                    replay_id: None,
                    track_id: None,
                    expected_source_sha256: None,
                    reference_id: Some(photo_id.clone()),
                    ..read_album.clone()
                }),
            )
            .await
            .unwrap()
            .0;
            assert_eq!(undone["count"], 0);
            assert_ne!(undone["revision"], album["revision"]);
            assert!(!album_path.join(format!("ref-{photo_id}.jpg")).exists());
            assert!(reference_photo(
                State(shared.clone()),
                Query(ReferencePhotoQuery {
                    project_id: id.into(),
                    player_id: "abcdef012345".into(),
                    reference_id: photo_id,
                })
            )
            .await
            .is_err());
            assert!(references(
                State(shared.clone()),
                HeaderMap::new(),
                Json(ReferenceRequest {
                    action: "undo".into(),
                    expected_revision: Some(album["revision"].as_str().unwrap().into()),
                    reference_id: Some("0".repeat(24)),
                    ..read_album.clone()
                })
            )
            .await
            .is_err()); // This optional integration test runs the *actual* native C-ABI
                        // model on a verified synthetic CPU replay and a real FFmpeg crop.
                        // It cannot attribute an athlete from the resulting vector.
            if std::env::var("VOLLEY_OPENVINO_C_LIB").is_ok() {
                let fixture_models = Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../..")
                    .join("ov");
                let local_models = shared.repository_root.join("ov");
                fs::create_dir_all(&local_models).unwrap();
                for suffix in ["xml", "bin"] {
                    let filename = format!("person-reidentification-retail-0287.{suffix}");
                    fs::copy(fixture_models.join(&filename), local_models.join(&filename)).unwrap();
                }
                let output = embeddings(
                    State(shared.clone()),
                    HeaderMap::new(),
                    Json(EmbeddingRequest {
                        project_id: id.into(),
                        video: video.into(),
                        replay_id: replay.into(),
                        track_ids: vec![7],
                        expected_source_sha256: source_sha.clone(),
                    }),
                )
                .await
                .unwrap()
                .0;
                assert_eq!(output["schema"], "volley-native-reid-features/1");
                assert_eq!(output["source_replay_sha256"], source_sha);
                assert_eq!(output["embedding_count"], 1);
                assert_eq!(output["features"][0]["track_id"], 7);
                assert_eq!(output["features"][0]["dimensions"], 256);
                assert_eq!(output["identities_confirmed"], false);
                assert_eq!(output["automatic_reid_binding"], false);
                let vec = output["features"][0]["vector"].as_array().unwrap();
                assert_eq!(vec.len(), 256);
                let norm = vec.iter().map(|x| x.as_f64().unwrap().powi(2)).sum::<f64>();
                assert!((norm - 1.0).abs() < 0.00001);
                assert_eq!(output["cache_hits"], 0);
                assert_eq!(output["computed_count"], 1);
                assert_eq!(output["cache_persisted"], true);
                let again = embeddings(
                    State(shared.clone()),
                    HeaderMap::new(),
                    Json(EmbeddingRequest {
                        project_id: id.into(),
                        video: video.into(),
                        replay_id: replay.into(),
                        track_ids: vec![7],
                        expected_source_sha256: source_sha.clone(),
                    }),
                )
                .await
                .unwrap()
                .0;
                assert_eq!(again["cache_hits"], 1);
                assert_eq!(again["computed_count"], 0);
                assert_eq!(again["cache_persisted"], false);
                assert_eq!(
                    again["features"][0]["vector"],
                    output["features"][0]["vector"]
                );
                assert_eq!(again["model_fingerprint"], output["model_fingerprint"]);
                // A stale model-to-replay reference must be refused before
                // decoding or invoking the installed inference library.
                assert!(embeddings(
                    State(shared.clone()),
                    HeaderMap::new(),
                    Json(EmbeddingRequest {
                        project_id: id.into(),
                        video: video.into(),
                        replay_id: replay.into(),
                        track_ids: vec![7],
                        expected_source_sha256: "0".repeat(64),
                    })
                )
                .await
                .is_err());
            }
            let not_found = photo(
                State(shared),
                Query(PhotoQuery {
                    project_id: id.into(),
                    video: video.into(),
                    replay_id: replay.into(),
                    track_id: 999,
                }),
            )
            .await
            .unwrap_err();
            assert_eq!(not_found.0, axum::http::StatusCode::BAD_REQUEST);
        });
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reference_album_schema_rejects_stale_or_unreviewed_photos() {
        let athlete = "abcdef012345";
        let initial = references_doc(None, athlete).unwrap();
        assert_eq!(initial["photos"], json!([]));
        let valid = json!({"schema":"volley-native-player-references/1",
        "player_id":athlete,"photos":[{
            "id":"0123456789ab0123456789ab",
            "filename":"ref-0123456789ab0123456789ab.jpg",
            "image_sha256":"a".repeat(64),
            "source_replay_sha256":"b".repeat(64),
            "track_id":7,"manually_enrolled":true
        }]});
        assert!(references_doc(Some(&serde_json::to_vec(&valid).unwrap()), athlete).is_ok());
        let mut wrong = valid.clone();
        wrong["photos"][0]["manually_enrolled"] = json!(false);
        assert!(references_doc(Some(&serde_json::to_vec(&wrong).unwrap()), athlete).is_err());
        let mut wrong = valid.clone();
        wrong["photos"][0]["filename"] = json!("../secret.jpg");
        assert!(references_doc(Some(&serde_json::to_vec(&wrong).unwrap()), athlete).is_err());
        let mut wrong = valid.clone();
        wrong["photos"]
            .as_array_mut()
            .unwrap()
            .push(valid["photos"][0].clone());
        assert!(references_doc(Some(&serde_json::to_vec(&wrong).unwrap()), athlete).is_err());
    }
    #[test]
    fn photo_crop_is_bounded_and_rejects_corrupted_geometry() {
        assert_eq!(crop_box([0.2, 0.1, 0.4, 0.7], 640, 360).unwrap().len(), 4);
        let [x, y, w, h] = crop_box([0.0, 0.0, 1.0, 1.0], 640, 360).unwrap();
        assert_eq!((x, y, w, h), (0, 0, 640, 360));
        assert!(crop_box([0.2, 0.4, 0.1, 0.9], 640, 360).is_err());
        assert!(crop_box([-0.1, 0.0, 0.5, 0.6], 640, 360).is_err());
        assert!(crop_box([0.2, 0.1, 0.4, f64::NAN], 640, 360).is_err());
        assert!(crop_box([0.2, 0.1, 0.4, 0.7], 32, 360).is_err());
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
        let trusted_page = page(&ctx, &doc, "revision", 0, 40).unwrap();
        assert_eq!(trusted_page["assigned_count"], 1);
        assert_eq!(
            trusted_page["tracks"][0]["assigned_player_id"],
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
