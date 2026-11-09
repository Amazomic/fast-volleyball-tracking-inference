//! Read-only, version-bound preview of native provisional rally snapshots.
//! Manual montage source intervals are suggestions; not canonical game events.
use crate::{
    cpu_retrack::{opaque_id, safe_file, safe_folder, BoundedReader},
    inference_stream, montage, preparation, project_archive, projects,
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
    collections::{BTreeSet, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use volley_core::event_evidence::{
    bounded_clips, interpret_motion, verify_rally_interval, Interval, MotionInput,
};
use volley_core::rally_selector::{
    self, Choice as SharedChoice, GoldFilter, SourceFormat, SourceRally,
};

const MAX_MANIFEST: u64 = 16 * 1024;
const MAX_EVIDENCE: u64 = 64 * 1024 * 1024 * 1024;
const MAX_PREVIEW: usize = 80;
const MAX_EVENTS_PER_CARD: usize = 48;
const MAX_RALLIES: usize = 200_000;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewQuery {
    pub project_id: String,
    pub rally_run_id: String,
    #[serde(default)]
    pub offset: usize,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default = "default_gold_filter")]
    pub gold_filter: String,
}
fn default_gold_filter() -> String {
    "all".into()
}
fn default_limit() -> usize {
    24
}

#[derive(Debug, Deserialize)]
struct NativeRally {
    candidate_id: String,
    detected_start_frame: u64,
    detected_end_frame: u64,
    ball_observations: u64,
    ball_coverage: f64,
    motion_supported_touch_proposals: u64,
    first_touch_frame: u64,
    last_touch_frame: u64,
    max_touch_probability: f64,
    classification: String,
    authoritative_rally: bool,
    athlete_identities_verified: bool,
    serve_verified: bool,
    reviewed: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeMotion {
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
impl NativeMotion {
    pub(crate) fn event_key(&self) -> (u64, Option<u64>) {
        (self.frame_index, self.provisional_track_id)
    }
    pub(crate) fn is_physical(&self) -> bool {
        self.physical_motion_supported && !self.athlete_identity_verified
    }
    pub(crate) fn probability(&self) -> f64 {
        self.probability
    }
    pub(crate) fn valid(&self, frames: u64, width: u32, height: u32) -> bool {
        self.frame_index < frames
            && self.motion_frame_index < frames
            && self.motion_frame_index.abs_diff(self.frame_index) <= 4
            && self.physical_motion_supported
            && !self.athlete_identity_verified
            && self.technique == "unknown_touch"
            && self
                .provisional_track_id
                .is_none_or(|id| id > 0 && id <= i64::MAX as u64)
            && self.probability.is_finite()
            && (0.0..=1.0).contains(&self.probability)
            && self.ball_x_px.is_finite()
            && (0.0..=width as f64).contains(&self.ball_x_px)
            && self.ball_y_px.is_finite()
            && (0.0..=height as f64).contains(&self.ball_y_px)
    }
}
fn expected_id(replay_id: &str, touch_id: &str, start: u64, end: u64) -> String {
    let material = format!("{replay_id}:{touch_id}:{start}:{end}");
    format!("{:x}", Sha256::digest(material.as_bytes()))[..24].to_string()
}
fn checksum(value: &Value) -> Result<&str, String> {
    value
        .as_str()
        .filter(|s| {
            s.len() == 64
                && s.bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        })
        .ok_or("Missing SHA-256 input checksum".into())
}
fn read_manifest(dir: &Path, kind: &str) -> Result<Value, String> {
    safe_folder(dir)?;
    serde_json::from_slice(
        &fs::read(safe_file(dir, "manifest.json", MAX_MANIFEST)?)
            .map_err(|e| format!("Read {kind} manifest: {e}"))?,
    )
    .map_err(|e| format!("Malformed {kind} manifest: {e}"))
}
fn validate_ids(list: &[String]) -> Result<(), String> {
    if list.is_empty()
        || list.len() > MAX_PREVIEW
        || list.iter().any(|id| {
            id.len() != 24
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        })
        || list.iter().collect::<HashSet<_>>().len() != list.len()
    {
        return Err("Select between 1 and 80 distinct saved rally IDs".into());
    }
    Ok(())
}
#[derive(Debug, Clone)]
pub(crate) struct ValidatedRun {
    pub(crate) project_id: String,
    pub(crate) video: String,
    pub(crate) run_id: String,
    pub(crate) project_source: PathBuf,
    pub(crate) rally_file: PathBuf,
    pub(crate) touch_file: PathBuf,
    pub(crate) replay_id: String,
    pub(crate) replay_sha: String,
    pub(crate) touch_id: String,
    pub(crate) rally_sha: String,
    pub(crate) touch_sha: String,
    pub(crate) frames: u64,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) context: u64,
    pub(crate) expected_rallies: u64,
    pub(crate) expected_touches: u64,
}

pub(crate) fn inspect_project(
    state: &AppState,
    project_id: &str,
    rally_run_id: &str,
) -> Result<ValidatedRun, ApiError> {
    storage::project_id(project_id)?;
    if !opaque_id(rally_run_id) {
        return Err(storage::bad_request("Invalid native rally run ID"));
    }
    let project = projects::existing_project(state, project_id)?;
    let project_dir = projects::project_root(state).join(project_id);
    safe_folder(&project_dir).map_err(storage::bad_request)?;
    let run_root = project_dir.join("native_cpu_rallies");
    safe_folder(&run_root).map_err(storage::bad_request)?;
    let run_dir = run_root.join(rally_run_id);
    let rally = read_manifest(&run_dir, "rally").map_err(storage::bad_request)?;
    if rally["schema"] != "volley-native-cpu-rally-proposals/1"
        || rally["state"] != "provisional_rally_segments"
        || rally["project_id"].as_str() != Some(project_id)
        || rally["rally_run_id"].as_str() != Some(rally_run_id)
        || rally["file"] != "rally_candidates.jsonl"
        || rally["fps"] != 30
        || rally["rallies_authoritatively_confirmed"] != false
        || rally["athlete_identities_verified"] != false
        || rally["action_classification_complete"] != false
        || rally["montage_ready"] != false
        || rally["cpu_processing_complete"] != false
    {
        return Err(storage::bad_request(
            "Cannot preview an untrusted rally snapshot",
        ));
    }
    let video = rally["video"]
        .as_str()
        .ok_or_else(|| storage::bad_request("Missing saved rally video"))?
        .to_owned();
    storage::video_name(&video)?;
    if !project["videos"]
        .as_array()
        .is_some_and(|a| a.iter().any(|entry| entry.as_str() == Some(&video)))
    {
        return Err(storage::bad_request(
            "Saved rally video does not belong to project",
        ));
    }
    let replay_id = rally["source_replay_id"]
        .as_str()
        .filter(|id| opaque_id(id))
        .ok_or_else(|| storage::bad_request("Invalid source Track ID snapshot"))?
        .to_owned();
    let touch_id = rally["source_touch_id"]
        .as_str()
        .filter(|id| opaque_id(id))
        .ok_or_else(|| storage::bad_request("Invalid source Touch snapshot"))?
        .to_owned();
    let touch_root = project_dir.join("native_cpu_touch_runs");
    safe_folder(&touch_root).map_err(storage::bad_request)?;
    let touch_dir = touch_root.join(&touch_id);
    let touch = read_manifest(&touch_dir, "Touch").map_err(storage::bad_request)?;
    let plan = preparation::plan_for(state, project_id, &video)?;
    let prepared = plan
        .ready_path()
        .ok_or_else(|| storage::bad_request("Prepared source is not ready"))?;
    let (bytes, mtime) = inference_stream::source_stamp(prepared).map_err(storage::bad_request)?;
    let identity = json!({"bytes":bytes,"mtime_ns":mtime});
    let dimensions = volley_media::probe(prepared).map_err(storage::bad_request)?;
    let frames = rally["frames"]
        .as_u64()
        .filter(|n| *n > 0 && *n <= 48 * 3600 * 30)
        .ok_or_else(|| storage::bad_request("Invalid saved rally video length"))?;
    let count = rally["rally_candidates"]
        .as_u64()
        .filter(|n| *n <= MAX_RALLIES as u64)
        .ok_or_else(|| storage::bad_request("Invalid saved rally candidate count"))?;
    let touches = rally["touch_candidates_ingested"]
        .as_u64()
        .filter(|n| *n <= frames)
        .ok_or_else(|| storage::bad_request("Invalid saved Touch candidate count"))?;
    if plan.identity.fps != 30
        || rally["width"].as_u64() != Some(dimensions.width as u64)
        || rally["height"].as_u64() != Some(dimensions.height as u64)
        || rally["source_identity"] != identity
        || !matches!(
            touch["schema"].as_str(),
            Some("volley-native-cpu-touch-proposals/1")
        )
        || touch["state"] != "motion_gated_touch_proposals"
        || touch["touch_id"].as_str() != Some(&touch_id)
        || touch["project_id"].as_str() != Some(project_id)
        || touch["video"].as_str() != Some(&video)
        || touch["source_replay_id"].as_str() != Some(&replay_id)
        || touch["source_identity"] != identity
        || touch["frames"].as_u64() != Some(frames)
        || touch["width"].as_u64() != Some(dimensions.width as u64)
        || touch["height"].as_u64() != Some(dimensions.height as u64)
        || touch["file"] != "touch_candidates.jsonl"
        || touch["touch_candidates"].as_u64() != Some(touches)
        || touch["contacts_authoritatively_confirmed"] != false
        || touch["athlete_identities_verified"] != false
        || touch["cpu_processing_complete"] != false
        || rally["source_touch_sha256"] != touch["sha256"]
        || rally["source_replay_sha256"] != touch["source_replay_sha256"]
    {
        return Err(storage::bad_request(
            "Rally/Touch source version or video identity mismatch",
        ));
    }
    let rally_sha = checksum(&rally["sha256"])
        .map_err(storage::bad_request)?
        .to_owned();
    let touch_sha = checksum(&touch["sha256"])
        .map_err(storage::bad_request)?
        .to_owned();
    if rally["source_touch_sha256"].as_str() != Some(touch_sha.as_str()) {
        return Err(storage::bad_request(
            "Touch SHA does not match recorded source",
        ));
    }
    let context = rally["settings"]["suggested_context_frames"]
        .as_u64()
        .filter(|n| *n <= 180)
        .ok_or_else(|| storage::bad_request("Invalid saved context padding"))?;
    let source_info = volley_media::probe(&plan.source).map_err(storage::bad_request)?;
    if frames as f64 / 30.0 > source_info.duration_sec + 0.067 {
        return Err(storage::bad_request(
            "Rally source frames exceed original video's timeline",
        ));
    }
    Ok(ValidatedRun {
        project_id: project_id.into(),
        video,
        run_id: rally_run_id.into(),
        project_source: plan.source,
        rally_file: safe_file(&run_dir, "rally_candidates.jsonl", MAX_EVIDENCE)
            .map_err(storage::bad_request)?,
        touch_file: safe_file(&touch_dir, "touch_candidates.jsonl", MAX_EVIDENCE)
            .map_err(storage::bad_request)?,
        replay_sha: checksum(&rally["source_replay_sha256"])
            .map_err(storage::bad_request)?
            .to_owned(),
        replay_id,
        touch_id,
        rally_sha,
        touch_sha,
        frames,
        width: dimensions.width,
        height: dimensions.height,
        context,
        expected_rallies: count,
        expected_touches: touches,
    })
}
#[derive(Clone)]
enum Choice {
    Page {
        offset: usize,
        limit: usize,
        filter: String,
        marked: BTreeSet<String>,
    },
    Manual {
        ids: Vec<String>,
    },
}
struct PreviewContent {
    value: Value,
    clips: Vec<volley_media::montage::SelectedClip>,
}
/// Hash-verify all source evidence; expose only 1..80 selected candidates.
fn generate(run: &ValidatedRun, choice: Choice) -> Result<PreviewContent, String> {
    let (offset, limit, manual, filter, marked) = match choice {
        Choice::Page {
            offset,
            limit,
            filter,
            marked,
        } => {
            if limit == 0
                || limit > MAX_PREVIEW
                || offset > MAX_RALLIES
                || !matches!(filter.as_str(), "all" | "marked" | "unmarked")
            {
                return Err("Invalid preview page size or gold filter".into());
            }
            (offset, limit, None, filter, marked)
        }
        Choice::Manual { ids } => {
            validate_ids(&ids)?;
            (0, ids.len(), Some(ids), "all".into(), BTreeSet::new())
        }
    };
    let mut reader = BoundedReader::open(&run.rally_file)?;
    let mut records = Vec::<Value>::with_capacity(limit);
    let mut intervals = Vec::<Interval>::new();
    let mut all_ids = HashMap::<String, usize>::new();
    let mut order = Vec::<String>::new();
    let mut filtered_total = 0usize;
    let mut page_indices = Vec::<usize>::new();
    while let Some(bytes) = reader.line()? {
        if intervals.len() >= MAX_RALLIES {
            return Err("Too many rally candidates".into());
        }
        let item: NativeRally =
            serde_json::from_slice(&bytes).map_err(|e| format!("Invalid saved rally row: {e}"))?;
        let candidate = serde_json::from_slice::<Value>(&bytes)
            .map_err(|e| format!("Invalid candidate JSON: {e}"))?;
        let start = item.detected_start_frame;
        let end = item.detected_end_frame;
        if item.candidate_id != expected_id(&run.replay_id, &run.touch_id, start, end)
            || item.classification != "provisional_rally"
            || item.authoritative_rally
            || item.athlete_identities_verified
            || item.serve_verified
            || item.reviewed
            || start > end
            || end >= run.frames
            || intervals.last().is_some_and(|prior| prior.end >= start)
            || !item.ball_coverage.is_finite()
            || !(0.0..=1.0).contains(&item.ball_coverage)
            || !item.max_touch_probability.is_finite()
            || !(0.0..=1.0).contains(&item.max_touch_probability)
            || item.ball_observations > end - start + 1
            || item.motion_supported_touch_proposals < 1
            || item.first_touch_frame < start
            || item.last_touch_frame > end
            || item.first_touch_frame > item.last_touch_frame
            || all_ids
                .insert(item.candidate_id.clone(), intervals.len())
                .is_some()
        {
            return Err("Untrusted, duplicate or overlapping rally candidate".into());
        }
        let index = intervals.len();
        intervals.push(Interval { start, end });
        let chosen = if let Some(ids) = manual.as_ref() {
            ids.iter().any(|id| id == &item.candidate_id)
        } else {
            let eligible = match filter.as_str() {
                "marked" => marked.contains(&item.candidate_id),
                "unmarked" => !marked.contains(&item.candidate_id),
                _ => true,
            };
            if eligible {
                let position = filtered_total;
                filtered_total += 1;
                if position >= offset && position < offset.saturating_add(limit) {
                    page_indices.push(index);
                    true
                } else {
                    false
                }
            } else {
                false
            }
        };
        if chosen {
            records.push(candidate);
            order.push(item.candidate_id);
        }
    }
    if intervals.len() as u64 != run.expected_rallies || reader.finish() != run.rally_sha {
        return Err("Rally JSONL count or SHA-256 mismatch".into());
    }
    if let Some(ids) = manual.as_ref() {
        if records.len() != ids.len() {
            return Err("A selected rally ID is stale or absent".into());
        }
    }
    // The one typed selector validates immutable, source-scoped IDs before
    // either the preview page or a manual montage receives those intervals.
    let mut original_ids = vec![""; intervals.len()];
    for (id, idx) in &all_ids {
        original_ids[*idx] = id;
    }
    let source_refs = intervals
        .iter()
        .enumerate()
        .map(|(i, interval)| SourceRally {
            source: SourceFormat::NativeCpu,
            stable_id: original_ids[i],
            source_scope: &run.video,
            interval: *interval,
            marked: marked.contains(original_ids[i]),
        })
        .collect::<Vec<_>>();
    let core_indices = match manual.as_ref() {
        Some(ids) => rally_selector::select(&source_refs, SharedChoice::Manual(ids))?,
        None => rally_selector::select(
            &source_refs,
            SharedChoice::Page {
                offset,
                limit,
                gold: GoldFilter::parse(&filter)?,
            },
        )?,
    };
    if manual.is_none() && core_indices != page_indices {
        return Err("Source-scoped preview selector disagrees with saved page".into());
    }
    let bounds = bounded_clips(&intervals, run.frames, run.context)?;
    let mut by_id = HashMap::<String, Value>::new();
    for (record, id) in records.into_iter().zip(order) {
        by_id.insert(id, record);
    }
    let selected_indices = core_indices;
    let mut cards = Vec::<Value>::with_capacity(selected_indices.len());
    let mut clips = Vec::with_capacity(selected_indices.len());
    for &i in &selected_indices {
        let interval = intervals[i];
        let id = expected_id(&run.replay_id, &run.touch_id, interval.start, interval.end);
        let mut record = by_id
            .remove(&id)
            .ok_or("Missing validated rally candidate")?;
        let clip = &bounds[i];
        let raw_clip = volley_media::montage::SelectedClip {
            video: run.video.clone(),
            start_frame: clip.start_frame,
            end_frame: clip.end_frame,
            analysis_offset_sec: Some(0.0),
        };
        let scoped = verify_rally_interval("native_cpu", &id, interval, clip.clone(), run.frames)?;
        record["source_interval_contract"] = json!(scoped);
        record["manual_clip"] = json!({
            "video":run.video,"start_frame":clip.start_frame,
            "end_frame":clip.end_frame,"analysis_offset_sec":0.0
        });
        record["clip_boundaries_provisional"] = json!(true);
        record["manual_selection_only"] = json!(true);
        record["motion_events"] = json!([]);
        record["motion_event_count"] = json!(0);
        record["motion_event_preview_truncated"] = json!(false);
        cards.push(record);
        clips.push(raw_clip);
    }
    // Verify the complete Touch source as it is streamed, attaching only
    // physical evidence from intervals on the requested preview page.
    let mut contacts = BoundedReader::open(&run.touch_file)?;
    let mut prior = None;
    let mut count = 0u64;
    while let Some(bytes) = contacts.line()? {
        let event: NativeMotion = serde_json::from_slice(&bytes)
            .map_err(|e| format!("Invalid native Touch evidence row: {e}"))?;
        if !event.valid(run.frames, run.width, run.height)
            || prior.is_some_and(|last| event.frame_index <= last)
        {
            return Err("Untrusted, nonphysical or unsorted native Touch row".into());
        }
        prior = Some(event.frame_index);
        count += 1;
        if let Some((idx, _)) = selected_indices.iter().enumerate().find(|(_, index)| {
            let interval = intervals[**index];
            event.frame_index >= interval.start && event.frame_index <= interval.end
        }) {
            let annotation = interpret_motion(MotionInput {
                probability: event.probability,
                direction_change_deg: event.direction_change_deg,
                speed_before_px_frame: event.speed_before_px_frame,
                speed_after_px_frame: event.speed_after_px_frame,
                velocity_residual_px_frame: event.velocity_residual_px_frame,
                provisional_track_id: event.provisional_track_id,
            })?;
            let row = &mut cards[idx];
            let prev = row["motion_event_count"].as_u64().unwrap_or(0);
            row["motion_event_count"] = json!(prev + 1);
            if row["motion_events"]
                .as_array()
                .is_some_and(|a| a.len() < MAX_EVENTS_PER_CARD)
            {
                let mut detail = serde_json::to_value(annotation)
                    .map_err(|e| format!("Serialize motion evidence: {e}"))?;
                detail["frame_index"] = json!(event.frame_index);
                detail["ball_position_px"] = json!([event.ball_x_px, event.ball_y_px]);
                row["motion_events"]
                    .as_array_mut()
                    .ok_or("Invalid motion event card")?
                    .push(detail);
            } else {
                row["motion_event_preview_truncated"] = json!(true);
            }
        }
    }
    if count != run.expected_touches || contacts.finish() != run.touch_sha {
        return Err("Touch count or SHA-256 mismatch during rally preview".into());
    }
    Ok(PreviewContent {
        value: json!({
            "schema":"volley-native-rally-preview/1",
            "project_id":run.project_id,
            "video":run.video,
            "rally_run_id":run.run_id,
            "source_replay_id":run.replay_id,
            "source_touch_id":run.touch_id,
            "total_candidates":intervals.len(),
            "filtered_candidates":if manual.is_some(){intervals.len()}else{filtered_total},
            "gold_filter":filter,
            "offset":offset,
            "limit":limit,
            "items":cards,
            "source_rallies_sha256":run.rally_sha,
            "source_touches_sha256":run.touch_sha,
            "mode":"read_only_provisional_preview",
            "requires_explicit_manual_selection":true,
            "automatic_montage_eligible":false,
            "authoritative_game_events":false,
            "athlete_identities_verified":false,
            "selection_changed_sources":false
        }),
        clips,
    })
}
pub(crate) fn verify_run_or_candidate(
    state: &AppState,
    project_id: &str,
    rally_run_id: &str,
    candidate_id: Option<&str>,
) -> Result<(), ApiError> {
    let run = inspect_project(state, project_id, rally_run_id)?;
    let selection = match candidate_id {
        Some(id) => Choice::Manual {
            ids: vec![id.to_owned()],
        },
        None => Choice::Page {
            offset: 0,
            limit: 1,
            filter: "all".into(),
            marked: BTreeSet::new(),
        },
    };
    generate(&run, selection).map_err(storage::bad_request)?;
    Ok(())
}

pub async fn preview(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PreviewQuery>,
) -> ApiResult<Value> {
    let result = tokio::task::spawn_blocking(move || -> Result<Value, ApiError> {
        let _guard = project_archive::archive_guard(&state, &query.project_id)?;
        let run = inspect_project(&state, &query.project_id, &query.rally_run_id)?;
        let marked = crate::native_rally_marks::marked_for_run(
            &state,
            &query.project_id,
            &query.rally_run_id,
        )?;
        Ok(generate(
            &run,
            Choice::Page {
                offset: query.offset,
                limit: query.limit,
                filter: query.gold_filter,
                marked,
            },
        )
        .map_err(storage::bad_request)?
        .value)
    })
    .await
    .map_err(|e| storage::internal(format!("Native rally preview task: {e}")))??;
    Ok(Json(result))
}

/// Explicit manual-only choice from a *specific* immutable provisional
/// rally run. This does not enable automatic montage, and never accepts
/// client-provided trim offsets or arbitrary path strings.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManualMontageRequest {
    pub project_id: String,
    pub rally_run_id: String,
    pub candidate_ids: Vec<String>,
    #[serde(default)]
    pub audio_file: Option<String>,
    #[serde(default = "default_volume")]
    pub audio_volume: f64,
    #[serde(default)]
    pub encoder: Option<String>,
}
fn default_volume() -> f64 {
    40.0
}

/// Full native source validation is delegated to generate(), then a second
/// bounded SHA-verified pass yields every stable, non-overlapping original
/// detected rally interval for matching reviewed attack frames.
pub(crate) fn verified_rally_intervals(
    run: &ValidatedRun,
) -> Result<Vec<(String, u64, u64)>, String> {
    generate(
        run,
        Choice::Page {
            offset: 0,
            limit: 1,
            filter: "all".into(),
            marked: BTreeSet::new(),
        },
    )?;
    let mut source = BoundedReader::open(&run.rally_file)?;
    let mut found = Vec::new();
    while let Some(line) = source.line()? {
        if found.len() >= MAX_RALLIES {
            return Err("Too many validated rally rows".into());
        }
        let rally: NativeRally = serde_json::from_slice(&line)
            .map_err(|e| format!("Malformed verified rally interval: {e}"))?;
        found.push((
            rally.candidate_id,
            rally.detected_start_frame,
            rally.detected_end_frame,
        ));
    }
    if found.len() as u64 != run.expected_rallies || source.finish() != run.rally_sha {
        return Err("Native rally source changed during highlight review".into());
    }
    Ok(found)
}

/// All selected candidates are checked against the canonical, original-video
/// manual selector (including context, source bounds and full SHA evidence).
pub(crate) fn verified_manual_clips(
    run: &ValidatedRun,
    ids: Vec<String>,
) -> Result<Vec<volley_media::montage::SelectedClip>, String> {
    manual_plan(run, ids)
}
fn manual_plan(
    run: &ValidatedRun,
    ids: Vec<String>,
) -> Result<Vec<volley_media::montage::SelectedClip>, String> {
    let result = generate(run, Choice::Manual { ids })?;
    let mut seconds = 0.0;
    for clip in &result.clips {
        let actual = volley_media::montage::OriginalClip::from_analysis(
            &run.project_source,
            clip.start_frame,
            clip.end_frame,
            0.0,
        )?;
        seconds += actual.duration_sec();
        if seconds > 600.0 {
            return Err("Manually selected episodes exceed the 600-second montage limit".into());
        }
    }
    if seconds < 1.0 {
        return Err("Manual montage must last at least one second".into());
    }
    Ok(result.clips)
}

pub async fn start_manual_montage(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<ManualMontageRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    if !state.writes_enabled {
        return Err(storage::conflict(
            "Manual Rust montage requires isolated development storage",
        ));
    }
    if !request.audio_volume.is_finite() || !(0.0..=100.0).contains(&request.audio_volume) {
        return Err(storage::bad_request("Audio volume outside 0–100 percent"));
    }
    validate_ids(&request.candidate_ids).map_err(storage::bad_request)?;
    let project_id = request.project_id.clone();
    let run_id = request.rally_run_id.clone();
    let ids = request.candidate_ids;
    let preparing = state.clone();
    let project_for_worker = project_id.clone();
    let run_for_worker = run_id.clone();
    let clips = tokio::task::spawn_blocking(move || -> Result<_, ApiError> {
        let _guard = project_archive::archive_guard(&preparing, &project_for_worker)?;
        let run = inspect_project(&preparing, &project_for_worker, &run_for_worker)?;
        manual_plan(&run, ids).map_err(storage::bad_request)
    })
    .await
    .map_err(|e| storage::internal(format!("Manual rally planning task: {e}")))??;
    let output = montage::start_native_montage(
        State(state),
        headers,
        Json(montage::MontageRequest {
            project_id,
            clips,
            audio_file: request.audio_file,
            audio_volume: request.audio_volume,
            encoder: request.encoder,
            verified_highlights: None,
        }),
    )
    .await?;
    let Json(mut body) = output;
    body["source_provisional_rally_run_id"] = json!(run_id);
    body["manual_rally_selection"] = json!(true);
    body["authoritative_action_classification"] = json!(false);
    Ok(Json(body))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (ValidatedRun, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("rally-evidence-preview-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let rally_file = root.join("rallies.jsonl");
        let touch_file = root.join("touches.jsonl");
        let replay_id = "0123456789abcdef0123456789abcdef";
        let touch_id = "abcdef0123456789abcdef0123456789";
        let bounds = [(30, 80), (95, 155), (310, 380)];
        let mut rows = Vec::new();
        for (start, end) in bounds {
            let row = json!({
                "candidate_id":expected_id(replay_id,touch_id,start,end),
                "detected_start_frame":start,"detected_end_frame":end,
                "ball_observations":end-start+1,"ball_coverage":1.0,
                "motion_supported_touch_proposals":2,
                "first_touch_frame":start+10,"last_touch_frame":end-10,
                "max_touch_probability":0.9,"classification":"provisional_rally",
                "authoritative_rally":false,"athlete_identities_verified":false,
                "serve_verified":false,"reviewed":false
            });
            rows.extend_from_slice(serde_json::to_string(&row).unwrap().as_bytes());
            rows.push(b'\n');
        }
        let mut events = Vec::new();
        for frame in [40, 70, 115, 145, 330, 370] {
            let row = json!({
                "frame_index":frame,"probability":0.93,
                "provisional_track_id":3,
                "ball_x_px":150.0,"ball_y_px":100.0,
                "motion_frame_index":frame,
                "direction_change_deg":100.0,
                "speed_before_px_frame":4.0,"speed_after_px_frame":11.0,
                "velocity_residual_px_frame":8.0,
                "technique":"unknown_touch",
                "physical_motion_supported":true,"athlete_identity_verified":false,
            });
            events.extend_from_slice(serde_json::to_string(&row).unwrap().as_bytes());
            events.push(b'\n');
        }
        fs::write(&rally_file, &rows).unwrap();
        fs::write(&touch_file, &events).unwrap();
        let run = ValidatedRun {
            project_id: "1234abcdef01".into(),
            video: "match.mp4".into(),
            run_id: "0123456789abcdef0123456789abcdef".into(),
            project_source: root.join("source.mp4"),
            rally_file,
            touch_file,
            replay_id: replay_id.into(),
            replay_sha: "a".repeat(64),
            touch_id: touch_id.into(),
            rally_sha: format!("{:x}", Sha256::digest(rows)),
            touch_sha: format!("{:x}", Sha256::digest(events)),
            frames: 480,
            width: 640,
            height: 360,
            context: 45,
            expected_rallies: 3,
            expected_touches: 6,
        };
        (run, root)
    }
    #[test]
    fn paged_preview_has_safe_original_clips_and_evidence_labels() {
        let (run, root) = fixture();
        let page = generate(
            &run,
            Choice::Page {
                offset: 1,
                limit: 1,
                filter: "all".into(),
                marked: BTreeSet::new(),
            },
        )
        .unwrap();
        assert_eq!(page.value["total_candidates"], 3);
        assert_eq!(page.value["items"].as_array().unwrap().len(), 1);
        let card = &page.value["items"][0];
        assert_eq!(card["detected_start_frame"], 95);
        assert_eq!(card["motion_event_count"], 2);
        assert_eq!(card["motion_events"][0]["event_type"], "unknown_touch");
        assert_eq!(
            card["motion_events"][0]["motion_pattern"],
            "direction_and_speed_change"
        );
        assert_eq!(card["motion_events"][0]["game_action_verified"], false);
        assert_eq!(card["manual_clip"]["video"], "match.mp4");
        assert_eq!(
            card["source_interval_contract"]["source_kind"],
            "native_cpu"
        );
        assert_eq!(
            card["source_interval_contract"]["source_rally_id"],
            card["candidate_id"]
        );
        assert_eq!(
            card["source_interval_contract"]["source_start_frame"],
            card["manual_clip"]["start_frame"]
        );
        assert_eq!(
            card["source_interval_contract"]["source_end_frame"],
            card["manual_clip"]["end_frame"]
        );
        assert_eq!(
            card["source_interval_contract"]["detected_interval_complete"],
            true
        );
        assert!(card["manual_clip"]["start_frame"].as_u64().unwrap() <= 95);
        assert!(card["manual_clip"]["end_frame"].as_u64().unwrap() >= 155);
        assert_eq!(page.clips.len(), 1);
        assert_eq!(page.value["automatic_montage_eligible"], false);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn global_gold_filter_pages_over_the_full_saved_run() {
        let (run, root) = fixture();
        let first = expected_id(&run.replay_id, &run.touch_id, 30, 80);
        let last = expected_id(&run.replay_id, &run.touch_id, 310, 380);
        let marked = BTreeSet::from([first.clone(), last.clone()]);
        let page = generate(
            &run,
            Choice::Page {
                offset: 0,
                limit: 1,
                filter: "marked".into(),
                marked: marked.clone(),
            },
        )
        .unwrap();
        assert_eq!(page.value["total_candidates"], 3);
        assert_eq!(page.value["filtered_candidates"], 2);
        assert_eq!(page.value["items"][0]["candidate_id"], first);
        let second = generate(
            &run,
            Choice::Page {
                offset: 1,
                limit: 1,
                filter: "marked".into(),
                marked: marked.clone(),
            },
        )
        .unwrap();
        assert_eq!(second.value["filtered_candidates"], 2);
        assert_eq!(second.value["items"][0]["candidate_id"], last);
        let unmarked = generate(
            &run,
            Choice::Page {
                offset: 0,
                limit: 24,
                filter: "unmarked".into(),
                marked: marked.clone(),
            },
        )
        .unwrap();
        assert_eq!(unmarked.value["filtered_candidates"], 1);
        assert_eq!(unmarked.value["items"][0]["detected_start_frame"], 95);
        assert!(generate(
            &run,
            Choice::Page {
                offset: 0,
                limit: 24,
                filter: "untrusted".into(),
                marked
            }
        )
        .is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn manual_select_by_immutable_ids_regardless_of_page_and_order() {
        let (run, root) = fixture();
        let first = expected_id(&run.replay_id, &run.touch_id, 30, 80);
        let last = expected_id(&run.replay_id, &run.touch_id, 310, 380);
        let content = generate(
            &run,
            Choice::Manual {
                ids: vec![last, first],
            },
        )
        .unwrap();
        assert_eq!(content.clips.len(), 2);
        assert_eq!(content.clips[0].start_frame, 265);
        assert_eq!(content.clips[1].start_frame, 0);
        assert!(generate(
            &run,
            Choice::Manual {
                ids: vec!["0".repeat(24)]
            }
        )
        .is_err());
        assert!(generate(
            &run,
            Choice::Manual {
                ids: vec![expected_id(&run.replay_id, &run.touch_id, 30, 80); 2]
            }
        )
        .is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn manual_original_media_plan_rejects_stale_or_tampered_candidates() {
        use std::process::Command;
        if Command::new("ffmpeg").arg("-version").output().is_err() {
            return;
        }
        let (run, root) = fixture();
        let status = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=black:s=64x64:r=30:d=16",
                "-an",
                "-c:v",
                "mpeg4",
                "-y",
            ])
            .arg(&run.project_source)
            .status()
            .unwrap();
        assert!(status.success());
        let first = expected_id(&run.replay_id, &run.touch_id, 30, 80);
        let last = expected_id(&run.replay_id, &run.touch_id, 310, 380);
        let selected = manual_plan(&run, vec![last.clone(), first.clone()]).unwrap();
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].video, "match.mp4");
        assert!(selected[0].start_frame <= 310);
        assert!(selected[0].end_frame >= 380);
        assert!(selected[1].start_frame <= 30);
        assert!(selected[1].end_frame >= 80);
        assert_eq!(selected[0].analysis_offset_sec, Some(0.0));
        assert!(manual_plan(&run, vec![first.clone(), first.clone()]).is_err());
        assert!(manual_plan(&run, vec!["a".repeat(24)]).is_err());
        let mut damaged = fs::read(&run.rally_file).unwrap();
        damaged[0] = b' ';
        fs::write(&run.rally_file, damaged).unwrap();
        assert!(manual_plan(&run, vec![first]).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corruption_or_wrong_source_sha_blocks_entire_preview() {
        let (mut run, root) = fixture();
        let correct = run.rally_sha.clone();
        run.rally_sha = "0".repeat(64);
        assert!(generate(
            &run,
            Choice::Page {
                offset: 0,
                limit: 24,
                filter: "all".into(),
                marked: BTreeSet::new(),
            }
        )
        .is_err());
        run.rally_sha = correct;
        let mut events = fs::read(&run.touch_file).unwrap();
        events[0] = b' ';
        fs::write(&run.touch_file, events).unwrap();
        assert!(generate(
            &run,
            Choice::Page {
                offset: 0,
                limit: 24,
                filter: "all".into(),
                marked: BTreeSet::new(),
            }
        )
        .is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
