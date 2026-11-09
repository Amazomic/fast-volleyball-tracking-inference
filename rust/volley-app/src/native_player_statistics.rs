//! Project-scoped person statistics over one unique, SHA-verified native CPU
//! source per video, counting only explicitly reviewed Track ID assignments.
//! Unknown touches remain unassigned. This never infers technique from motion.
use crate::{
    cpu_retrack::{safe_file, BoundedReader},
    native_player_tracks::{self, NativeReviewedOwners},
    native_rally_preview::{self, NativeMotion, ValidatedRun},
    project_archive, projects,
    storage::{self, ApiError, ApiResult},
    versioned_store::{self, revision},
    AppState,
};
use axum::{extract::State, http::HeaderMap, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
};
const MAX_SOURCE: u64 = 64 * 1024 * 1024 * 1024;
const MAX_STATS: u64 = 8 * 1024 * 1024;
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatsSource {
    rally_run_id: String,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    project_id: String,
    sources: Vec<StatsSource>,
    #[serde(default = "preview")]
    mode: String,
    #[serde(default)]
    expected_revision: Option<String>,
}
fn preview() -> String {
    "preview".into()
}
#[derive(Default, Debug)]
struct ContactCounts {
    per_track: BTreeMap<u64, u64>,
    unassigned: u64,
    touches: u64,
}
/// Re-verify the exact video-to-touch and frame-to-Track relationship, not
/// merely that a guessed Track ID appears somewhere in the match.
fn count_native_contacts(
    run: &ValidatedRun,
    replay_file: &Path,
    owners: &NativeReviewedOwners,
) -> Result<ContactCounts, String> {
    if owners.source_sha != run.replay_sha {
        return Err("Player review source differs from Touch's CPU Track ID snapshot".into());
    }
    let mut source = BoundedReader::open(replay_file)?;
    let mut contacts = BoundedReader::open(&run.touch_file)?;
    let mut next = contacts
        .line()?
        .map(|row| {
            serde_json::from_slice::<NativeMotion>(&row)
                .map_err(|e| format!("Malformed native statistics Touch: {e}"))
        })
        .transpose()?;
    let mut before = None::<u64>;
    let mut total = ContactCounts::default();
    for frame in 0..run.frames {
        let raw = source
            .line()?
            .ok_or("Native player-statistics source truncated")?;
        let row: Value = serde_json::from_slice(&raw)
            .map_err(|e| format!("Malformed CPU Track ID source: {e}"))?;
        if row["frame_index"].as_u64() != Some(frame)
            || row["track_ids_provisional"] != true
            || row["confirmed_athlete_identities"] != false
            || row["touches_confirmed"] != false
        {
            return Err("Invalid or non-provisional track source frame".into());
        }
        let players = row["tracked_players"]
            .as_array()
            .ok_or("Missing player Track ID evidence")?;
        if players.len() > 128 {
            return Err("Unbounded player detections per frame".into());
        }
        let mut observed = BTreeSet::new();
        for player in players {
            let id = player["track_id"]
                .as_u64()
                .ok_or("Invalid actual player detection Track ID")?;
            if id == 0
                || id > i64::MAX as u64
                || !observed.insert(id)
                || player["is_verified_athlete"] != false
                || player["detection"]["frame_index"].as_u64() != Some(frame)
                || player["detection"]["class_name"] != "player"
            {
                return Err("Untrusted player detection in Touch source".into());
            }
        }
        if let Some(event) = next.as_ref() {
            let (contact_frame, track) = event.event_key();
            if contact_frame < frame
                || before.is_some_and(|last| contact_frame <= last)
                || !event.valid(run.frames, run.width, run.height)
                || !event.is_physical()
            {
                return Err("Untrusted, duplicate or unphysical Touch candidate".into());
            }
            if contact_frame == frame {
                if row["selected_ball"].is_null() {
                    return Err("Source Touch frame has no observed selected ball".into());
                }
                if track.is_some_and(|id| !observed.contains(&id)) {
                    return Err("Touch Track ID is absent from the same source frame".into());
                }
                total.touches += 1;
                match track {
                    Some(id) if !owners.excluded.contains(&id) => {
                        *total.per_track.entry(id).or_default() += 1
                    }
                    _ => total.unassigned += 1,
                }
                before = Some(contact_frame);
                next = contacts
                    .line()?
                    .map(|row| {
                        serde_json::from_slice::<NativeMotion>(&row)
                            .map_err(|e| format!("Malformed native Touch: {e}"))
                    })
                    .transpose()?;
            }
        }
    }
    if source.line()?.is_some()
        || source.finish() != run.replay_sha
        || next.is_some()
        || total.touches != run.expected_touches
        || contacts.finish() != run.touch_sha
    {
        return Err("Native player-statistics inputs changed or have undeclared events".into());
    }
    Ok(total)
}

fn select_unique_video(seen: &mut BTreeSet<String>, video: &str) -> bool {
    seen.insert(video.to_owned())
}
fn calculate(state: &AppState, request: &Request) -> Result<Value, ApiError> {
    storage::project_id(&request.project_id)?;
    if request.sources.is_empty() || request.sources.len() > 40 {
        return Err(storage::bad_request(
            "Select 1..40 native CPU runs, one per video",
        ));
    }

    let project = projects::existing_project(state, &request.project_id)?;
    let video_count = project["videos"].as_array().map_or(0, Vec::len);
    if request.sources.len() > video_count {
        return Err(storage::bad_request(
            "More CPU runs selected than project videos",
        ));
    }
    let mut selected = BTreeSet::new();
    let mut names = BTreeMap::<String, String>::new();
    let mut results = BTreeMap::<String, u64>::new();
    let mut unknown = 0u64;
    let mut contacts = 0u64;
    let mut sources = Vec::new();
    let mut track_breakdown = Vec::new();
    for input in &request.sources {
        native_rally_preview::verify_run_or_candidate(
            state,
            &request.project_id,
            &input.rally_run_id,
            None,
        )?;
        let run =
            native_rally_preview::inspect_project(state, &request.project_id, &input.rally_run_id)?;
        if !select_unique_video(&mut selected, &run.video) {
            return Err(storage::conflict(
                "Do not double-count multiple CPU revisions of one video",
            ));
        }
        let owners = native_player_tracks::reviewed_owners(
            state,
            &request.project_id,
            &run.video,
            &run.replay_id,
        )?;
        if run.replay_sha != owners.source_sha {
            return Err(storage::conflict(
                "Stats Touch/Track identity source SHA mismatch",
            ));
        }
        for (id, name) in &owners.roster {
            if names
                .insert(id.clone(), name.clone())
                .is_some_and(|old| old != *name)
            {
                return Err(storage::conflict(
                    "Project athlete registry changed during processing",
                ));
            }
        }
        let root = projects::project_root(state)
            .join(&request.project_id)
            .join("native_cpu_replays")
            .join(&run.replay_id);
        let replay =
            safe_file(&root, "tracked_frames.jsonl", MAX_SOURCE).map_err(storage::bad_request)?;
        let subtotal =
            count_native_contacts(&run, &replay, &owners).map_err(storage::bad_request)?;
        contacts += subtotal.touches;
        unknown += subtotal.unassigned;
        let mut run_unassigned = subtotal.unassigned;
        for (tid, count) in subtotal.per_track {
            let athlete = owners.owners.get(&tid);
            if let Some(player) = athlete {
                *results.entry(player.clone()).or_default() += count;
            } else {
                unknown += count;
                run_unassigned += count;
            }
            track_breakdown.push(json!({
                "video":run.video,"source_replay_id":run.replay_id,
                "source_touch_id":run.touch_id,"track_id":tid,"touches":count,
                "confirmed_player_id":athlete,"manually_reviewed":athlete.is_some()
            }));
        }
        sources.push(json!({
            "video":run.video,"rally_run_id":run.run_id,
            "source_replay_id":run.replay_id,"source_replay_sha256":run.replay_sha,
            "source_touch_id":run.touch_id,"source_touch_sha256":run.touch_sha,
            "source_rally_sha256":run.rally_sha,
            "identity_revision":owners.revision,
            "total_touch_candidates":subtotal.touches,
            "unassigned_touch_candidates":run_unassigned
        }));
    }
    sources.sort_by(|a, b| a["video"].as_str().cmp(&b["video"].as_str()));
    track_breakdown.sort_by(|a, b| {
        a["video"]
            .as_str()
            .cmp(&b["video"].as_str())
            .then_with(|| a["track_id"].as_u64().cmp(&b["track_id"].as_u64()))
    });
    let players = names
        .iter()
        .map(|(id, name)| {
            json!({
                "player_id":id,"name":name,"touches":results.get(id).copied().unwrap_or(0),
                "identity_source":"manually_reviewed_native_track_ids"
            })
        })
        .collect::<Vec<_>>();
    let source_checksum = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&sources)
                .map_err(|e| storage::internal(format!("Encode source fingerprint: {e}")))?
        )
    );
    let sum: u64 = results.values().sum();
    if sum + unknown != contacts {
        return Err(storage::internal(
            "Native stats conservation accounting failed",
        ));
    }
    Ok(json!({
        "schema":"volley-native-player-statistics/1",
        "algorithm":"verified-touch-frame-and-manual-athlete-links-v1",
        "project_id":request.project_id,
        "source_fingerprint":source_checksum,
        "sources":sources,"video_count":selected.len(),
        "total_touch_candidates":contacts,
        "assigned_touch_candidates":sum,
        "unassigned_touch_candidates":unknown,
        "players":players,"raw_tracks":track_breakdown,
        "auto_player_identity_enabled":false,
        "automatic_action_classification":false,
        "all_contacts_physically_verified":true
    }))
}
fn run(state: &AppState, req: &Request) -> Result<Value, ApiError> {
    let _guard = project_archive::archive_guard(state, &req.project_id)?;
    if !matches!(req.mode.as_str(), "preview" | "recalculate") {
        return Err(storage::bad_request(
            "Native stats mode must be preview or recalculate",
        ));
    }
    if req.mode == "preview" && req.expected_revision.is_some() {
        return Err(storage::bad_request(
            "Preview does not accept a write revision",
        ));
    }
    let mut output = calculate(state, req)?;
    let project = projects::project_root(state).join(&req.project_id);
    let file = project.join("native_player_statistics.json");
    let raw =
        versioned_store::read_optional_bounded(&file, MAX_STATS).map_err(storage::bad_request)?;
    if req.mode == "recalculate" {
        if !state.writes_enabled {
            return Err(storage::conflict(
                "Stats persistence needs isolated Rust storage",
            ));
        }
        let expected = req
            .expected_revision
            .as_deref()
            .ok_or_else(|| storage::bad_request("Expected native statistics revision missing"))?;
        versioned_store::expect_revision(raw.as_deref(), expected).map_err(storage::conflict)?;
        versioned_store::write_json_checked(&project, &file, &output, MAX_STATS)
            .map_err(storage::bad_request)?;
    }
    let current = if req.mode == "recalculate" {
        versioned_store::read_optional_bounded(&file, MAX_STATS).map_err(storage::bad_request)?
    } else {
        raw
    };
    output["saved_revision"] = json!(revision(current.as_deref()));
    output["snapshot_persisted"] = json!(req.mode == "recalculate");
    Ok(output)
}
pub async fn statistics(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<Request>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    let _permit = state
        .statistics_limit
        .acquire()
        .await
        .map_err(|e| storage::internal(format!("Native stats CPU gate: {e}")))?;
    let worker = state.clone();
    let output = tokio::task::spawn_blocking(move || run(&worker, &req))
        .await
        .map_err(|e| storage::internal(format!("Native stats worker: {e}")))??;
    Ok(Json(output))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use uuid::Uuid;
    fn fixture() -> (
        std::path::PathBuf,
        ValidatedRun,
        std::path::PathBuf,
        NativeReviewedOwners,
    ) {
        let root = std::env::temp_dir().join(format!("native-stats-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let replay = root.join("tracked_frames.jsonl");
        let touches = root.join("touch_candidates.jsonl");
        let mut replay_bytes = Vec::new();
        let mut touch_bytes = Vec::new();
        for frame in 0..80u64 {
            let track = if frame < 40 { 7 } else { 8 };
            let row = json!({
                "frame_index":frame,"track_ids_provisional":true,
                "confirmed_athlete_identities":false,"touches_confirmed":false,
                "selected_ball":{"source_model":"vballnet"},
                "tracked_players":[{
                    "track_id":track,"is_verified_athlete":false,
                    "detection":{"frame_index":frame,"class_name":"player"}
                }]
            });
            replay_bytes.extend(serde_json::to_vec(&row).unwrap());
            replay_bytes.push(b'\n');
            if [10, 20, 42, 50, 60].contains(&frame) {
                let contact = json!({
                    "frame_index":frame,"provisional_track_id":track,
                    "probability":0.96,"ball_x_px":200.0,
                    "ball_y_px":100.0,"motion_frame_index":frame,
                    "direction_change_deg":90.0,"speed_before_px_frame":4.0,
                    "speed_after_px_frame":8.0,"velocity_residual_px_frame":10.0,
                    "technique":"unknown_touch","physical_motion_supported":true,
                    "athlete_identity_verified":false
                });
                touch_bytes.extend(serde_json::to_vec(&contact).unwrap());
                touch_bytes.push(b'\n');
            }
        }
        fs::write(&replay, &replay_bytes).unwrap();
        fs::write(&touches, &touch_bytes).unwrap();
        let sha = format!("{:x}", Sha256::digest(&replay_bytes));
        let run = ValidatedRun {
            project_id: "abcdef012345".into(),
            video: "match.mp4".into(),
            run_id: "1".repeat(32),
            project_source: root.join("video.mp4"),
            rally_file: root.join("rallies.jsonl"),
            touch_file: touches,
            replay_id: "2".repeat(32),
            replay_sha: sha.clone(),
            touch_id: "3".repeat(32),
            rally_sha: "4".repeat(64),
            touch_sha: format!("{:x}", Sha256::digest(&touch_bytes)),
            frames: 80,
            width: 640,
            height: 360,
            context: 45,
            expected_rallies: 1,
            expected_touches: 5,
        };
        let owner = NativeReviewedOwners {
            source_sha: sha,
            revision: "a".repeat(64),
            owners: BTreeMap::from([(7, "player1".into()), (8, "player2".into())]),
            excluded: BTreeSet::new(),
            roster: BTreeMap::from([
                ("player1".into(), "Alice".into()),
                ("player2".into(), "Bob".into()),
            ]),
        };
        (root, run, replay, owner)
    }
    #[test]
    fn native_contact_count_credits_only_same_frame_reviewed_owners() {
        let (root, run, replay, mut owners) = fixture();
        let counted = count_native_contacts(&run, &replay, &owners).unwrap();
        assert_eq!(counted.touches, 5);
        assert_eq!(counted.unassigned, 0);
        assert_eq!(counted.per_track.get(&7), Some(&2));
        assert_eq!(counted.per_track.get(&8), Some(&3));
        owners.owners.remove(&8);
        let counted = count_native_contacts(&run, &replay, &owners).unwrap();
        assert_eq!(counted.per_track.get(&8), Some(&3)); // observed raw only
        assert!(owners.owners.get(&8).is_none()); // never auto-attaches
        owners.excluded.insert(8);
        let counted = count_native_contacts(&run, &replay, &owners).unwrap();
        assert_eq!(counted.unassigned, 3);
        assert_eq!(counted.per_track.len(), 1);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn native_stats_reject_missing_track_in_touch_frame_even_with_new_sha() {
        let (root, mut run, replay, mut owners) = fixture();
        let raw = fs::read(&replay).unwrap();
        let mut modified = Vec::new();
        for line in raw.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
            let mut row: Value = serde_json::from_slice(line).unwrap();
            if row["frame_index"] == 42 {
                row["tracked_players"][0]["track_id"] = json!(9);
            }
            modified.extend(serde_json::to_vec(&row).unwrap());
            modified.push(b'\n');
        }
        fs::write(&replay, &modified).unwrap();
        run.replay_sha = format!("{:x}", Sha256::digest(&modified));
        owners.source_sha = run.replay_sha.clone();
        let failure = count_native_contacts(&run, &replay, &owners).unwrap_err();
        assert!(
            failure.contains("absent"),
            "Incorrect source gate: {failure}"
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn native_stats_reject_double_counting_same_video_cpu_revisions() {
        let mut seen = BTreeSet::new();
        assert!(select_unique_video(&mut seen, "match.mp4"));
        assert!(!select_unique_video(&mut seen, "match.mp4"));
        assert!(select_unique_video(&mut seen, "second.mp4"));
        assert_eq!(seen.len(), 2);
    }
}
