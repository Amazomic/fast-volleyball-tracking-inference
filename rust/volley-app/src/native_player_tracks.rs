//! Versioned manual assignment of *provisional* native CPU Track IDs to
//! athlete registry entries. The raw GPU/CPU evidence is NEVER rewritten.
//! ReID appearance vectors / saved athlete photo templates are not generated
//! by this module; each owner association requires an explicit human action.
use crate::{
    cpu_retrack::{opaque_id, safe_file, safe_folder, BoundedReader},
    inference_stream, player_catalog, preparation, project_archive, projects,
    storage::{self, ApiError, ApiResult},
    versioned_store::{self, revision},
    AppState,
};
use axum::{
    extract::{Query, State},
    http::HeaderMap,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use volley_core::{
    evidence_source::{EvidenceSource, FrameRate, SourceStamp, VerifiedTrackSet},
    fusion::Candidate,
    identity_review::{ManualReviewState, ReviewAction, ReviewErrorKind},
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
