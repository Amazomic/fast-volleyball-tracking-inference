//! Explicit reviewed game-action labels for SHA-verified native Touch events.
//! No automatically inferred attack, serve or block; no identity promotion.
use crate::{
    cpu_retrack::{safe_folder, BoundedReader},
    native_rally_preview::{self, NativeMotion, ValidatedRun},
    project_guard, projects,
    storage::{self, ApiError, ApiResult},
    versioned_store::{self, revision},
    AppState,
};
use axum::{extract::State, http::HeaderMap, Json};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    sync::Arc,
};
use volley_core::action_review::{self, ContactEvidence, ReviewedGameAction};

const MAX_DOC_BYTES: u64 = 512 * 1024;
const MAX_REVIEWED: usize = 1024;
const SCHEMA: &str = "volley-native-action-review/1";
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewRequest {
    project_id: String,
    rally_run_id: String,
    action: String,
    #[serde(default)]
    frame_index: Option<u64>,
    #[serde(default)]
    provisional_track_id: Option<u64>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    expected_revision: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewedEntry {
    frame_index: u64,
    provisional_track_id: Option<u64>,
    label: String,
    manually_reviewed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    schema: String,
    project_id: String,
    rally_run_id: String,
    source_replay_id: String,
    source_touch_id: String,
    source_rally_sha256: String,
    source_touch_sha256: String,
    entries: Vec<ReviewedEntry>,
}
impl Document {
    fn empty(run: &ValidatedRun) -> Self {
        Self {
            schema: SCHEMA.into(),
            project_id: run.project_id.clone(),
            rally_run_id: run.run_id.clone(),
            source_replay_id: run.replay_id.clone(),
            source_touch_id: run.touch_id.clone(),
            source_rally_sha256: run.rally_sha.clone(),
            source_touch_sha256: run.touch_sha.clone(),
            entries: Vec::new(),
        }
    }
    fn validate(&self, run: &ValidatedRun) -> Result<(), String> {
        if self.schema != SCHEMA
            || self.project_id != run.project_id
            || self.rally_run_id != run.run_id
            || self.source_replay_id != run.replay_id
            || self.source_touch_id != run.touch_id
            || self.source_rally_sha256 != run.rally_sha
            || self.source_touch_sha256 != run.touch_sha
        {
            return Err("Saved manual action reviews belong to a different source version".into());
        }
        if self.entries.len() > MAX_REVIEWED {
            return Err("Too many manually reviewed actions".into());
        }
        let mut seen = BTreeSet::new();
        for entry in &self.entries {
            if !entry.manually_reviewed
                || entry.frame_index >= run.frames
                || !action_review::REVIEWABLE_ACTIONS.contains(&entry.label.as_str())
                || !seen.insert((entry.frame_index, entry.provisional_track_id))
            {
                return Err("Invalid or duplicate reviewed action labels".into());
            }
        }
        Ok(())
    }
}
fn path_for(state: &AppState, run: &ValidatedRun) -> Result<PathBuf, String> {
    let project = projects::project_root(state).join(&run.project_id);
    safe_folder(&project)?;
    let folder = project.join("native_action_reviews");
    if let Ok(meta) = fs::symlink_metadata(&folder) {
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err("Unsafe native reviewed-action directory".into());
        }
    }
    Ok(folder.join(format!("{}.json", run.run_id)))
}
fn verify_source(
    run: &ValidatedRun,
    wanted: &BTreeSet<(u64, Option<u64>)>,
) -> Result<BTreeMap<(u64, Option<u64>), ContactEvidence>, String> {
    if wanted.len() > MAX_REVIEWED + 1 {
        return Err("Too many requested reviewed contacts".into());
    }
    let mut reader = BoundedReader::open(&run.touch_file)?;
    let mut before = None::<u64>;
    let mut count = 0u64;
    let mut found = BTreeMap::new();
    while let Some(bytes) = reader.line()? {
        let event: NativeMotion = serde_json::from_slice(&bytes)
            .map_err(|e| format!("Malformed reviewed Touch source: {e}"))?;
        if !event.valid(run.frames, run.width, run.height) {
            return Err("Unsafe or nonphysical source Touch record".into());
        }
        let (frame, owner) = event.event_key();
        if before.is_some_and(|v| v >= frame) {
            return Err("Unsorted or duplicate reviewed Touch source".into());
        }
        before = Some(frame);
        count += 1;
        if wanted.contains(&(frame, owner)) {
            found.insert(
                (frame, owner),
                ContactEvidence {
                    frame_index: frame,
                    provisional_track_id: owner,
                    probability: event.probability(),
                    physical_motion_supported: event.is_physical(),
                    sha_verified: true,
                },
            );
        }
    }
    if count != run.expected_touches || reader.finish() != run.touch_sha {
        return Err("Source Touch record count or SHA256 changed".into());
    }
    if found.len() != wanted.len() {
        return Err("Reviewed Touch frame/Track ID absent from saved source".into());
    }
    Ok(found)
}
fn response(
    doc: &Document,
    raw: Option<&[u8]>,
    evidence: &BTreeMap<(u64, Option<u64>), ContactEvidence>,
    writable: bool,
) -> Result<Value, String> {
    let mut actions = Vec::<ReviewedGameAction>::new();
    for entry in &doc.entries {
        let key = (entry.frame_index, entry.provisional_track_id);
        let src = *evidence
            .get(&key)
            .ok_or("Reviewed action no longer belongs to source")?;
        actions.push(action_review::classify(src, Some(&entry.label)).map_err(str::to_owned)?);
    }
    Ok(json!({
        "schema":SCHEMA,"project_id":doc.project_id,
        "rally_run_id":doc.rally_run_id,
        "source_replay_id":doc.source_replay_id,
        "source_touch_id":doc.source_touch_id,
        "source_rally_sha256":doc.source_rally_sha256,
        "source_touch_sha256":doc.source_touch_sha256,
        "revision":revision(raw),"writable":writable,
        "reviewed_actions":actions,"reviewed_count":actions.len(),
        "automatic_game_action_classification":false,
        "athlete_identities_verified":false
    }))
}
fn handle(state: &AppState, req: &ReviewRequest) -> Result<Value, ApiError> {
    let _guard = project_guard::write_guard(state, &req.project_id)?;
    native_rally_preview::verify_run_or_candidate(state, &req.project_id, &req.rally_run_id, None)?;
    let run = native_rally_preview::inspect_project(state, &req.project_id, &req.rally_run_id)?;
    let file = path_for(state, &run).map_err(storage::bad_request)?;
    let before = versioned_store::read_optional_bounded(&file, MAX_DOC_BYTES)
        .map_err(storage::bad_request)?;
    let mut doc = match before.as_deref() {
        Some(bytes) => serde_json::from_slice::<Document>(bytes)
            .map_err(|e| storage::bad_request(format!("Invalid reviewed actions: {e}")))?,
        None => Document::empty(&run),
    };
    doc.validate(&run).map_err(storage::conflict)?;
    let writing = matches!(req.action.as_str(), "set" | "clear");
    if !matches!(req.action.as_str(), "read" | "set" | "clear") {
        return Err(storage::bad_request(
            "Supported action review operations: read/set/clear",
        ));
    }
    if writing {
        if !state.writes_enabled {
            return Err(storage::conflict(
                "Reviewed actions require isolated storage",
            ));
        }
        let expected = req
            .expected_revision
            .as_deref()
            .ok_or_else(|| storage::bad_request("Expected action review revision missing"))?;
        versioned_store::expect_revision(before.as_deref(), expected).map_err(storage::conflict)?;
        if req.frame_index.is_none() {
            return Err(storage::bad_request("Specify an exact native Touch frame"));
        }
        if req.action == "set"
            && !req
                .label
                .as_deref()
                .is_some_and(|s| action_review::REVIEWABLE_ACTIONS.contains(&s))
        {
            return Err(storage::bad_request(
                "Select a supported explicitly reviewed action label",
            ));
        }
    }
    let mut wanted = doc
        .entries
        .iter()
        .map(|e| (e.frame_index, e.provisional_track_id))
        .collect::<BTreeSet<_>>();
    if writing {
        wanted.insert((req.frame_index.unwrap(), req.provisional_track_id));
    }
    let evidence = verify_source(&run, &wanted).map_err(storage::conflict)?;
    let mut changed = false;
    if writing {
        let key = (req.frame_index.unwrap(), req.provisional_track_id);
        let old = doc
            .entries
            .iter()
            .position(|e| (e.frame_index, e.provisional_track_id) == key);
        match req.action.as_str() {
            "set" => {
                let reviewed = action_review::classify(
                    *evidence.get(&key).expect("verified"),
                    req.label.as_deref(),
                )
                .map_err(storage::bad_request)?;
                if !reviewed.action_verified {
                    return Err(storage::bad_request("Unknown action is not reviewed"));
                }
                let item = ReviewedEntry {
                    frame_index: key.0,
                    provisional_track_id: key.1,
                    label: req.label.clone().unwrap(),
                    manually_reviewed: true,
                };
                if let Some(i) = old {
                    if doc.entries[i].label != item.label {
                        doc.entries[i] = item;
                        changed = true;
                    }
                } else {
                    if doc.entries.len() >= MAX_REVIEWED {
                        return Err(storage::bad_request("Reviewed action limit exceeded"));
                    }
                    doc.entries.push(item);
                    changed = true;
                }
            }
            "clear" => {
                if let Some(i) = old {
                    doc.entries.remove(i);
                    changed = true;
                }
            }
            _ => unreachable!(),
        }
        if changed {
            doc.entries
                .sort_by_key(|e| (e.frame_index, e.provisional_track_id));
            doc.validate(&run).map_err(storage::bad_request)?;
            let parent = file
                .parent()
                .ok_or_else(|| storage::internal("Missing review directory"))?;
            versioned_store::write_json_checked(
                parent,
                &file,
                &serde_json::to_value(&doc)
                    .map_err(|e| storage::internal(format!("Serialize reviewed actions: {e}")))?,
                MAX_DOC_BYTES,
            )
            .map_err(storage::bad_request)?;
        }
    }
    let now = versioned_store::read_optional_bounded(&file, MAX_DOC_BYTES)
        .map_err(storage::bad_request)?;
    let mut out = response(&doc, now.as_deref(), &evidence, state.writes_enabled)
        .map_err(storage::bad_request)?;
    out["changed"] = json!(changed);
    Ok(out)
}
/// Reuses the complete SHA-bound read path, including physical Touch
/// matching, reviewed-only labels and revision/source guards.
pub(crate) fn verified_actions(state: &AppState, run: &ValidatedRun) -> Result<Value, ApiError> {
    let file = path_for(state, run).map_err(storage::bad_request)?;
    let bytes = versioned_store::read_optional_bounded(&file, MAX_DOC_BYTES)
        .map_err(storage::bad_request)?;
    let doc = match bytes.as_deref() {
        Some(raw) => serde_json::from_slice::<Document>(raw)
            .map_err(|e| storage::bad_request(format!("Malformed reviewed-action source: {e}")))?,
        None => Document::empty(run),
    };
    doc.validate(run).map_err(storage::conflict)?;
    let wanted = doc
        .entries
        .iter()
        .map(|e| (e.frame_index, e.provisional_track_id))
        .collect::<BTreeSet<_>>();
    let evidence = verify_source(run, &wanted).map_err(storage::conflict)?;
    response(&doc, bytes.as_deref(), &evidence, false).map_err(storage::bad_request)
}
pub async fn review(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<ReviewRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    let _permit = state
        .marks_limit
        .acquire()
        .await
        .map_err(|e| storage::internal(format!("Action review CPU gate: {e}")))?;
    let state = state.clone();
    let result = tokio::task::spawn_blocking(move || handle(&state, &req))
        .await
        .map_err(|e| storage::internal(format!("Reviewed action worker failed: {e}")))??;
    Ok(Json(result))
}
#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use uuid::Uuid;
    fn fake_run(root: &std::path::Path) -> ValidatedRun {
        let mut lines = Vec::new();
        for (frame, id) in [(12u64, 7u64), (35, 8)] {
            let item = json!({
                "frame_index":frame,"provisional_track_id":id,"probability":0.94,
                "ball_x_px":120.0,"ball_y_px":100.0,"motion_frame_index":frame,
                "direction_change_deg":89.0,"speed_before_px_frame":4.0,
                "speed_after_px_frame":9.0,"velocity_residual_px_frame":10.0,
                "technique":"unknown_touch","physical_motion_supported":true,
                "athlete_identity_verified":false
            });
            lines.extend(serde_json::to_vec(&item).unwrap());
            lines.push(b'\n');
        }
        let touch_file = root.join("touch.jsonl");
        fs::write(&touch_file, &lines).unwrap();
        ValidatedRun {
            project_id: "123456abcdef".into(),
            video: "match.mp4".into(),
            run_id: "0".repeat(32),
            project_source: root.join("original.mp4"),
            rally_file: root.join("rallies.jsonl"),
            touch_file,
            replay_id: "1".repeat(32),
            replay_sha: "a".repeat(64),
            touch_id: "2".repeat(32),
            rally_sha: "3".repeat(64),
            touch_sha: format!("{:x}", Sha256::digest(&lines)),
            frames: 60,
            source_duration_sec: 2.0,
            width: 640,
            height: 360,
            context: 30,
            expected_rallies: 1,
            expected_touches: 2,
        }
    }
    #[test]
    fn review_labels_must_match_physical_touch_source_and_exact_sha() {
        let root = std::env::temp_dir().join(format!("action-review-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let run = fake_run(&root);
        let mut doc = Document::empty(&run);
        assert!(doc.validate(&run).is_ok());
        doc.entries.push(ReviewedEntry {
            frame_index: 12,
            provisional_track_id: Some(7),
            label: "attack".into(),
            manually_reviewed: true,
        });
        assert!(doc.validate(&run).is_ok());
        let target = BTreeSet::from([(12, Some(7)), (35, Some(8))]);
        let verified = verify_source(&run, &target).unwrap();
        let result = response(&doc, None, &verified, true).unwrap();
        assert_eq!(result["reviewed_count"], 1);
        assert_eq!(result["reviewed_actions"][0]["action_kind"], "attack");
        assert_eq!(result["reviewed_actions"][0]["action_verified"], true);
        assert_eq!(
            result["reviewed_actions"][0]["athlete_identity_verified"],
            false
        );
        assert_eq!(result["automatic_game_action_classification"], false);
        assert!(verify_source(&run, &BTreeSet::from([(12, Some(999))])).is_err());
        let mut wrong = run.clone();
        wrong.touch_sha = "f".repeat(64);
        assert!(verify_source(&wrong, &target).is_err());
        assert!(doc.validate(&wrong).is_err());
        doc.entries.push(ReviewedEntry {
            frame_index: 12,
            provisional_track_id: Some(7),
            label: "serve".into(),
            manually_reviewed: true,
        });
        assert!(doc.validate(&run).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn reviewed_action_rejects_label_forgery_and_removed_manual_status() {
        let root = std::env::temp_dir().join(format!("action-review-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let run = fake_run(&root);
        let mut doc = Document::empty(&run);
        doc.entries.push(ReviewedEntry {
            frame_index: 12,
            provisional_track_id: Some(7),
            label: "power_attack_unverified".into(),
            manually_reviewed: true,
        });
        assert!(doc.validate(&run).is_err());
        doc.entries[0].label = "block".into();
        doc.entries[0].manually_reviewed = false;
        assert!(doc.validate(&run).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
