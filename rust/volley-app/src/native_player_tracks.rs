//! Versioned manual assignment of *provisional* native CPU Track IDs to
//! athlete registry entries. The raw GPU/CPU evidence is NEVER rewritten.
//! ReID appearance vectors / saved athlete photo templates are not generated
//! by this module; each owner association requires an explicit human action.
use crate::{
    cpu_retrack::{opaque_id, safe_file, safe_folder, BoundedReader},
    inference_stream, native_reid_cache, player_catalog, preparation, project_guard, projects,
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
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::SystemTime,
};
use volley_core::{
    continuity::{group_tracklets, ContinuityConfig, ContinuityReport, PlayerDetection},
    evidence_source::{EvidenceSource, FrameRate, SourceStamp, VerifiedTrackSet},
    fusion::Candidate,
    identity_review::{self, ManualReviewState, ReviewAction, ReviewErrorKind, ReviewedLink},
    motion::{ball_motion_extrema, BallObservation},
    reid::{self, Feature, FeatureMode, TrackInfo},
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
    /// Optional exact CPU rally run for player review (not the raw replay).
    #[serde(default)]
    rally_run_id: Option<String>,
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default)]
    view: Option<String>,
    #[serde(default)]
    view_player_id: Option<String>,
}
fn default_limit() -> usize {
    40
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectTrackReset {
    project_id: String,
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
    rally_run_id: Option<String>,
    #[serde(default)]
    player_id: Option<String>,
    expected_revision: String,
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_limit")]
    limit: usize,
}
#[derive(Debug, Default, Clone, PartialEq)]
struct Aggregate {
    first: u64,
    last: u64,
    observed: u64,
    best_frame: u64,
    best_score: f64,
    best_box: [f64; 4],
    photo_samples: Vec<PhotoSample>,
    visible_spans: Vec<(u64, u64)>,
}
#[derive(Debug, Clone, PartialEq)]
struct PhotoSample {
    frame: u64,
    bbox: [f64; 4],
}
#[derive(Deserialize)]
struct Tracked {
    track_id: u64,
    detection: Candidate,
    is_verified_athlete: bool,
}
#[derive(Deserialize)]
struct NativeSelectedBall {
    source_model: String,
    detection: Candidate,
}
#[derive(Deserialize)]
struct Frame {
    frame_index: u64,
    selected_ball: Option<NativeSelectedBall>,
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
pub(crate) use volley_core::contracts::player_id as valid_player_id;
pub(crate) use volley_core::contracts::sha256 as valid_sha;
mod source_index;
use source_index::*;
mod assignments;
pub(crate) use assignments::*;

mod trajectory;
pub(crate) use trajectory::*;
mod continuity;
pub(crate) use continuity::*;
mod reid_handlers;
pub(crate) use reid_handlers::*;
mod photo_comparison;
mod photos;
pub(crate) use photos::*;
mod embedding_jobs;
pub(crate) use embedding_jobs::*;
mod reference_store;
pub(crate) use reference_store::*;
#[cfg(test)]
mod tests;

// Reset all saved player Track ID bindings for this project, keeping evidence and albums.
pub async fn reset_project(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<ProjectTrackReset>,
) -> crate::storage::ApiResult<Value> {
    storage::validate_origin(&headers)?;
    let result = tokio::task::spawn_blocking(move || {
        let _guard = project_guard::write_guard(&state, &req.project_id)?;
        if !state.writes_enabled {
            return Err(storage::conflict("Project binding edits are disabled"));
        }
        storage::project_id(&req.project_id)?;
        let root = projects::project_root(&state).join(&req.project_id);
        safe_folder(&root).map_err(storage::bad_request)?;
        let directory = root.join("native_track_assignments");
        if !directory.exists() {
            return Ok(json!({"reset_tracks":0,"reset_replays":0}));
        }
        safe_folder(&directory).map_err(storage::bad_request)?;
        let mut count = 0usize;
        let mut versions = 0usize;
        for item in fs::read_dir(&directory)
            .map_err(|e| storage::internal(format!("Read bindings directory: {e}")))?
        {
            let item = item.map_err(|e| storage::internal(format!("Read binding item: {e}")))?;
            let path = item.path();
            let metadata = fs::symlink_metadata(&path)
                .map_err(|e| storage::bad_request(format!("Check binding item: {e}")))?;
            if metadata.file_type().is_symlink()
                || !metadata.is_file()
                || path.extension().and_then(|v| v.to_str()) != Some("json")
            {
                return Err(storage::bad_request("Unsafe Track ID binding entry"));
            }
            let Some(bytes) = versioned_store::read_optional_bounded(&path, MAX_LINKS)
                .map_err(storage::bad_request)? else { continue };
            let mut doc: Value = serde_json::from_slice(&bytes)
                .map_err(|e| storage::bad_request(format!("Invalid bindings JSON: {e}")))?;
            if doc["schema"] != "volley-native-player-track-links/1"
                || doc["project_id"] != req.project_id
                || doc["replay_id"].as_str() != path.file_stem().and_then(|v| v.to_str())
            {
                return Err(storage::bad_request("Invalid project Track ID bindings"));
            }
            let manual = doc["assignments"].as_object().ok_or_else(|| storage::bad_request("Invalid manual bindings"))?.clone();
            let automatic = doc["automatic_assignments"].as_object().cloned().unwrap_or_default();
            if manual.is_empty() && automatic.is_empty() { continue; }
            let mut suppressed: BTreeSet<u64> = doc["automatic_suppressed_ids"]
                .as_array().into_iter().flatten().filter_map(Value::as_u64).collect();
            for key in manual.keys().chain(automatic.keys()) {
                let id = key.parse::<u64>().map_err(|_| storage::bad_request("Invalid linked ID"))?;
                suppressed.insert(id);
            }
            count += manual.len() + automatic.len();
            doc["assignments"] = json!({});
            doc["automatic_assignments"] = json!({});
            doc["automatic_suppressed_ids"] = json!(suppressed);
            versioned_store::write_json_checked(&directory, &path, &doc, MAX_LINKS)
                .map_err(storage::bad_request)?;
            versions += 1;
        }
        Ok(json!({"reset_tracks":count,"reset_replays":versions}))
    })
    .await
    .map_err(|e| storage::internal(format!("Reset project Track ID task: {e}")))??;
    Ok(Json(result))
}