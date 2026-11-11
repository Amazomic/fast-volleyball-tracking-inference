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
mod photos;
pub(crate) use photos::*;
mod embedding_jobs;
pub(crate) use embedding_jobs::*;
mod reference_store;
pub(crate) use reference_store::*;
#[cfg(test)]
mod tests;
