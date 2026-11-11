//! Explicitly reviewed athlete attacks -> ranked original-video rally clips.
//! No speculative visual ReID or auto-classified actions can produce entries.
use crate::{
    montage, native_action_review, native_player_statistics, native_player_tracks,
    native_rally_preview::{self, ValidatedRun},
    project_guard, projects,
    storage::{self, ApiError, ApiResult},
    AppState,
};
use axum::{extract::State, http::HeaderMap, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    sync::Arc,
};
use volley_media::montage::{SelectedClip, VideoLayout};

const MAX_RUNS: usize = 40;
const MAX_CLIPS: usize = 80;
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    project_id: String,
    player_id: String,
    rally_run_ids: Vec<String>,
    #[serde(default = "preview")]
    operation: String,
    #[serde(default = "auto")]
    selection_mode: String,
    #[serde(default)]
    manual_rally_keys: Vec<String>,
    #[serde(default = "default_max")]
    max_clips: usize,
    #[serde(default)]
    expected_source_fingerprint: Option<String>,
    #[serde(default)]
    encoder: Option<String>,
    #[serde(default)]
    layout: VideoLayout,
    #[serde(default)]
    audio_file: Option<String>,
    #[serde(default = "default_volume")]
    audio_volume: f64,
}
fn preview() -> String {
    "preview".into()
}
fn auto() -> String {
    "auto".into()
}
fn default_volume() -> f64 {
    40.0
}
fn default_max() -> usize {
    12
}

#[derive(Debug, Clone)]
struct Candidate {
    key: String,
    candidate_id: String,
    rally_run_id: String,
    video: String,
    start: u64,
    end: u64,
    verified_attacks: u64,
}
impl Candidate {
    fn view(&self) -> Value {
        json!({
            "key":self.key,"candidate_id":self.candidate_id,
            "rally_run_id":self.rally_run_id,"video":self.video,
            "detected_start_frame":self.start,"detected_end_frame":self.end,
            "manually_verified_player_attacks":self.verified_attacks,
            "action_label_source":"explicit_human_review",
            "athlete_link_source":"manually_reviewed_native_track_ids"
        })
    }
}
fn select_candidates(
    candidates: &[Candidate],
    mode: &str,
    manual: &[String],
    max: usize,
) -> Result<Vec<Candidate>, String> {
    if max == 0 || max > MAX_CLIPS || !matches!(mode, "auto" | "manual") {
        return Err("Choose auto or manual highlights, 1..80 clips".into());
    }
    if mode == "auto" {
        if !manual.is_empty() {
            return Err("Automatic highlights cannot accept manual IDs".into());
        }
        return Ok(candidates.iter().take(max).cloned().collect());
    }
    if manual.is_empty()
        || manual.len() > max
        || manual.iter().collect::<HashSet<_>>().len() != manual.len()
    {
        return Err("Manual highlights require 1..max distinct source-scoped rally keys".into());
    }
    let by_key = candidates
        .iter()
        .map(|c| (c.key.as_str(), c))
        .collect::<HashMap<_, _>>();
    manual
        .iter()
        .map(|key| {
            by_key
                .get(key.as_str())
                .map(|c| (*c).clone())
                .ok_or_else(|| format!("Unverified or stale athlete attack rally: {key}"))
        })
        .collect()
}
/// Source review can name an action, but a physical Touch only counts for a
/// specific player after the separate, manual, non-excluded Track ID review.
fn attack_for_player(
    action: &Value,
    owners: &native_player_tracks::NativeReviewedOwners,
    player_id: &str,
) -> Result<Option<(u64, u64)>, String> {
    if action["action_kind"] != "attack" {
        return Ok(None);
    }
    if action["action_verified"] != true
        || action["motion_supported"] != true
        || action["action_source"] != "explicit_human_review"
        || action["athlete_identity_verified"] != false
    {
        return Err("Unreviewed attack cannot become an athlete highlight".into());
    }
    let (Some(frame), Some(track)) = (
        action["frame_index"].as_u64(),
        action["provisional_track_id"].as_u64(),
    ) else {
        return Ok(None);
    };
    if owners.excluded.contains(&track)
        || owners.owners.get(&track).map(String::as_str) != Some(player_id)
    {
        return Ok(None);
    }
    Ok(Some((frame, track)))
}
fn rally_for_frame(intervals: &[(String, u64, u64)], frame: u64) -> Option<usize> {
    let index = intervals.partition_point(|(_, start, _)| *start <= frame);
    index.checked_sub(1).filter(|i| frame <= intervals[*i].2)
}
struct Plan {
    response: Value,
    clips: Vec<SelectedClip>,
}
fn plan(state: &AppState, request: &Request) -> Result<Plan, ApiError> {
    storage::project_id(&request.project_id)?;
    if request.player_id.is_empty()
        || request.player_id.len() > 128
        || request.rally_run_ids.is_empty()
        || request.rally_run_ids.len() > MAX_RUNS
        || request.rally_run_ids.iter().collect::<HashSet<_>>().len() != request.rally_run_ids.len()
        || request.max_clips == 0
        || request.max_clips > MAX_CLIPS
        || !matches!(request.selection_mode.as_str(), "auto" | "manual")
    {
        return Err(storage::bad_request(
            "Invalid player, native CPU runs or highlight settings",
        ));
    }
    if !request.audio_volume.is_finite() || !(0.0..=100.0).contains(&request.audio_volume) {
        return Err(storage::bad_request("Soundtrack volume must be 0..100"));
    }
    if !matches!(request.operation.as_str(), "preview" | "start") {
        return Err(storage::bad_request("Supported operations: preview/start"));
    }
    let expected = request.expected_source_fingerprint.as_deref();
    if (request.operation == "start"
        && !expected.is_some_and(|v| {
            v.len() == 64
                && v.bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        }))
        || (request.operation == "preview" && expected.is_some())
    {
        return Err(storage::bad_request(
            "Start requires a valid preview SHA fingerprint",
        ));
    }
    let project = projects::existing_project(state, &request.project_id)?;
    if request.rally_run_ids.len() > project["videos"].as_array().map_or(0, Vec::len) {
        return Err(storage::bad_request(
            "Multiple CPU versions of the same video cannot be combined",
        ));
    }
    let mut videos = BTreeSet::new();
    let mut runs = BTreeMap::<String, ValidatedRun>::new();
    let mut sources = Vec::new();
    let mut candidates = Vec::<Candidate>::new();
    let mut athlete_name = None::<String>;
    // Sorted source IDs make fingerprint deterministic regardless of UI order.
    let mut source_ids = request.rally_run_ids.clone();
    source_ids.sort();
    for run_id in &source_ids {
        let run = native_rally_preview::inspect_project(state, &request.project_id, run_id)?;
        if !videos.insert(run.video.clone()) {
            return Err(storage::conflict(
                "More than one CPU version selected for the same video",
            ));
        }
        // The legacy and native run adapters MUST NOT be mixed by ID guessing.
        let owners = native_player_tracks::reviewed_owners(
            state,
            &request.project_id,
            &run.video,
            &run.replay_id,
        )?;
        if owners.source_sha != run.replay_sha {
            return Err(storage::conflict(
                "Athlete assignments belong to another replay SHA",
            ));
        }
        let name = owners
            .roster
            .get(&request.player_id)
            .ok_or_else(|| storage::bad_request("Player is not in selected project roster"))?;
        if athlete_name.as_ref().is_some_and(|old| old != name) {
            return Err(storage::conflict(
                "Athlete catalog changed between selected runs",
            ));
        }
        athlete_name = Some(name.clone());
        // This streams both original replay frames AND Touch, enforcing
        // physically supported same-frame Track ID ownership with both SHAs.
        native_player_statistics::verify_contacts_for_montage(state, &run, &owners)?;
        let reviewed = native_action_review::verified_actions(state, &run)?;
        if reviewed["schema"] != "volley-native-action-review/1"
            || reviewed["source_replay_id"] != run.replay_id
            || reviewed["source_touch_id"] != run.touch_id
            || reviewed["source_touch_sha256"] != run.touch_sha
            || reviewed["source_rally_sha256"] != run.rally_sha
            || reviewed["automatic_game_action_classification"] != false
        {
            return Err(storage::conflict(
                "Action labels have a mismatched source/version",
            ));
        }
        let intervals =
            native_rally_preview::verified_rally_intervals(&run).map_err(storage::bad_request)?;
        let mut per_rally = vec![0_u64; intervals.len()];
        let action_rows = reviewed["reviewed_actions"]
            .as_array()
            .ok_or_else(|| storage::conflict("Missing human-reviewed actions"))?;
        let mut counted = BTreeSet::<(u64, u64)>::new();
        for action in action_rows {
            let Some((frame, track)) = attack_for_player(action, &owners, &request.player_id)
                .map_err(storage::conflict)?
            else {
                continue;
            };
            if !counted.insert((frame, track)) {
                return Err(storage::conflict(
                    "Duplicate reviewed attack in same source",
                ));
            }
            if let Some(index) = rally_for_frame(&intervals, frame) {
                per_rally[index] += 1;
            }
        }
        for (i, (id, start, end)) in intervals.into_iter().enumerate() {
            if per_rally[i] == 0 {
                continue;
            }
            candidates.push(Candidate {
                key: format!("{}:{id}", run.run_id),
                candidate_id: id,
                rally_run_id: run.run_id.clone(),
                video: run.video.clone(),
                start,
                end,
                verified_attacks: per_rally[i],
            });
        }
        sources.push(json!({
            "video":run.video,"rally_run_id":run.run_id,
            "replay_sha256":run.replay_sha,
            "touch_sha256":run.touch_sha,"rally_sha256":run.rally_sha,
            "identity_revision":owners.revision,
            "action_revision":reviewed["revision"],
            "manual_identity_only":true,"human_reviewed_actions_only":true
        }));
        runs.insert(run.run_id.clone(), run);
    }
    candidates.sort_by(|a, b| {
        b.verified_attacks
            .cmp(&a.verified_attacks)
            .then_with(|| a.video.cmp(&b.video))
            .then_with(|| a.start.cmp(&b.start))
            .then_with(|| a.key.cmp(&b.key))
    });
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&json!({
                "project_id":request.project_id,"player_id":request.player_id,"sources":sources,
            }))
            .map_err(|e| storage::internal(format!("Encode highlights fingerprint: {e}")))?
        )
    );
    if let Some(expected) = expected {
        if expected != fingerprint {
            return Err(storage::conflict(
                "Reviewed player, actions or CPU sources changed since preview",
            ));
        }
    }
    let selected = select_candidates(
        &candidates,
        &request.selection_mode,
        &request.manual_rally_keys,
        request.max_clips,
    )
    .map_err(storage::bad_request)?;
    // No UI frame bounds or FFmpeg source paths are accepted. The shared
    // original-source clip selector reconstructs intervals from full SHA.
    let mut by_run = BTreeMap::<String, Vec<String>>::new();
    for item in &selected {
        by_run
            .entry(item.rally_run_id.clone())
            .or_default()
            .push(item.candidate_id.clone());
    }
    let mut checked = BTreeMap::<String, SelectedClip>::new();
    for (id, keys) in by_run {
        let run = runs
            .get(&id)
            .ok_or_else(|| storage::internal("Validated run absent"))?;
        let clips = native_rally_preview::verified_manual_clips(run, keys.clone())
            .map_err(storage::bad_request)?;
        for (candidate, clip) in keys.into_iter().zip(clips) {
            checked.insert(format!("{id}:{candidate}"), clip);
        }
    }
    let clips = selected
        .iter()
        .map(|entry| {
            checked
                .remove(&entry.key)
                .ok_or_else(|| storage::internal("Missing verified source interval"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let attack_total = selected.iter().map(|c| c.verified_attacks).sum::<u64>();
    let view = json!({
        "schema":"volley-native-reviewed-player-highlights/1",
        "project_id":request.project_id,
        "player_id":request.player_id,"player_name":athlete_name,
        "selection_mode":request.selection_mode,
        "sources":sources,"source_fingerprint":fingerprint,
        "candidates":candidates.iter().take(80).map(Candidate::view).collect::<Vec<_>>(),
        "total_eligible_rallies":candidates.len(),
        "selected":selected.iter().map(Candidate::view).collect::<Vec<_>>(),
        "selected_verified_attacks":attack_total,
        "selected_clips":clips.len(),
        "reviewed_actions_only":true,
        "manually_linked_athlete_only":true,
        "automatic_action_classification":false,
        "render_source":"original_recording",
        "cancellation_route":"/api/montage/native/cancel",
    });
    Ok(Plan {
        response: view,
        clips,
    })
}
pub async fn highlights(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<Request>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    let _permit = state
        .statistics_limit
        .acquire()
        .await
        .map_err(|e| storage::internal(format!("Highlights CPU gate: {e}")))?;
    let operation = request.operation.clone();
    let project_id = request.project_id.clone();
    let encoder = request.encoder.clone();
    let layout = request.layout;
    let audio_file = request.audio_file.clone();
    let audio_volume = request.audio_volume;
    let state_copy = state.clone();
    let planned = tokio::task::spawn_blocking(move || {
        let _guard = project_guard::write_guard(&state_copy, &request.project_id)?;
        plan(&state_copy, &request)
    })
    .await
    .map_err(|e| storage::internal(format!("Highlights planning task: {e}")))??;
    drop(_permit);
    if operation == "preview" {
        return Ok(Json(planned.response));
    }
    if planned.clips.is_empty() {
        return Err(storage::bad_request(
            "No verified player attacks selected for montage",
        ));
    }
    let evidence = json!({
        "schema":"volley-native-verified-player-highlights/1",
        "player_id":planned.response["player_id"],
        "player_name":planned.response["player_name"],
        "source_fingerprint":planned.response["source_fingerprint"],
        "sources":planned.response["sources"],
        "selected_rallies":planned.response["selected"],
        "selection_mode":planned.response["selection_mode"],
        "verified_attack_count":planned.response["selected_verified_attacks"],
        "human_review_only":true,
        "source_video":"original_recording",
    });
    let Json(job) = montage::start_native_montage(
        State(state),
        headers,
        Json(montage::MontageRequest {
            project_id,
            clips: planned.clips,
            audio_file,
            audio_volume,
            encoder,
            layout,
            verified_highlights: Some(evidence),
        }),
    )
    .await?;
    let mut response = planned.response;
    response["operation"] = json!("start");
    response["job_id"] = job["job_id"].clone();
    response["status"] = job["status"].clone();
    Ok(Json(response))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn candidate(run: &str, key: &str, count: u64) -> Candidate {
        Candidate {
            key: format!("{run}:{key}"),
            candidate_id: key.into(),
            rally_run_id: run.into(),
            video: format!("{run}.mp4"),
            start: 100,
            end: 180,
            verified_attacks: count,
        }
    }
    #[test]
    fn only_explicit_attacks_of_manually_linked_athlete_count() {
        let mut owners = native_player_tracks::NativeReviewedOwners {
            source_sha: "a".repeat(64),
            revision: "absent".into(),
            owners: BTreeMap::from([(7, "player-a".into()), (8, "player-b".into())]),
            excluded: BTreeSet::new(),
            roster: BTreeMap::new(),
        };
        let verified = json!({
            "action_kind":"attack","action_verified":true,
            "motion_supported":true,"action_source":"explicit_human_review",
            "athlete_identity_verified":false,
            "frame_index":20,"provisional_track_id":7,
        });
        assert_eq!(
            attack_for_player(&verified, &owners, "player-a").unwrap(),
            Some((20, 7))
        );
        assert_eq!(
            attack_for_player(&verified, &owners, "player-b").unwrap(),
            None
        );
        let mut spoof = verified.clone();
        spoof["action_source"] = json!("automatic_reid");
        assert!(attack_for_player(&spoof, &owners, "player-a").is_err());
        spoof = verified.clone();
        spoof["action_verified"] = json!(false);
        assert!(attack_for_player(&spoof, &owners, "player-a").is_err());
        spoof = verified.clone();
        spoof["action_kind"] = json!("unknown_touch");
        assert_eq!(
            attack_for_player(&spoof, &owners, "player-a").unwrap(),
            None
        );
        owners.excluded.insert(7);
        assert_eq!(
            attack_for_player(&verified, &owners, "player-a").unwrap(),
            None
        );
        let intervals = vec![("rally-1".into(), 10, 20), ("rally-2".into(), 30, 40)];
        assert_eq!(rally_for_frame(&intervals, 10), Some(0));
        assert_eq!(rally_for_frame(&intervals, 20), Some(0));
        assert_eq!(rally_for_frame(&intervals, 21), None);
        assert_eq!(rally_for_frame(&intervals, 30), Some(1));
        assert_eq!(rally_for_frame(&intervals, 41), None);
    }
    #[test]
    fn automatic_top_attacks_and_source_scoped_manual_overrides() {
        let mut rows = vec![
            candidate("runA", "a", 3),
            candidate("runB", "b", 2),
            candidate("runA", "c", 1),
        ];
        assert_eq!(
            select_candidates(&rows, "auto", &[], 2)
                .unwrap()
                .into_iter()
                .map(|r| r.key)
                .collect::<Vec<_>>(),
            vec!["runA:a", "runB:b"]
        );
        assert_eq!(
            select_candidates(&rows, "manual", &["runA:c".into(), "runA:a".into()], 2)
                .unwrap()
                .into_iter()
                .map(|r| r.key)
                .collect::<Vec<_>>(),
            vec!["runA:c", "runA:a"]
        );
        assert!(
            select_candidates(&rows, "manual", &["runA:a".into(), "runA:a".into()], 2).is_err()
        );
        assert!(select_candidates(&rows, "manual", &["runC:a".into()], 2).is_err());
        assert!(select_candidates(&rows, "auto", &["runA:a".into()], 2).is_err());
        assert!(select_candidates(&rows, "manual", &[], 2).is_err());
        assert!(select_candidates(&rows, "auto", &[], 0).is_err());
        rows[0].key = "runB:a".into();
        assert!(select_candidates(&rows, "manual", &["runA:a".into()], 2).is_err());
    }
}
