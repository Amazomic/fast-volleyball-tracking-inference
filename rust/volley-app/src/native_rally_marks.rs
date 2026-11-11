//! Gold bookmarks for exact, immutable native provisional rally versions.
//! This is NOT the legacy confirmed-CPU-run gold marks format.
use crate::{
    cpu_retrack::{opaque_id, safe_folder},
    native_rally_preview, project_guard, projects,
    storage::{self, ApiError, ApiResult},
    versioned_store::{self, revision as digest},
    AppState,
};
use axum::{
    extract::{Query, State},
    http::HeaderMap,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::BTreeSet, path::Path, sync::Arc};
use volley_core::gold::{edit_gold_set, valid_gold_id, MAX_GOLD_MARKS};

const MAX_FILE: u64 = 256 * 1024;
const MAX_MARKS: usize = MAX_GOLD_MARKS;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarksQuery {
    project_id: String,
    rally_run_id: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetMark {
    project_id: String,
    rally_run_id: String,
    candidate_id: String,
    marked: bool,
    /// Compare-and-swap; mandatory for updates. First revision is "absent".
    expected_revision: String,
}
fn valid_candidate(id: &str) -> bool {
    valid_gold_id(id)
}
fn bytes_for(root: &Path, run_id: &str) -> Result<Option<Vec<u8>>, String> {
    if !root.exists() {
        return Ok(None);
    }
    safe_folder(root)?;
    versioned_store::read_optional_bounded(&root.join(format!("{run_id}.json")), MAX_FILE)
}
fn decode(bytes: Option<&[u8]>, run_id: &str) -> Result<BTreeSet<String>, String> {
    let Some(raw) = bytes else {
        return Ok(BTreeSet::new());
    };
    let doc: Value =
        serde_json::from_slice(raw).map_err(|e| format!("Malformed native rally marks: {e}"))?;
    if doc["schema"] != "volley-native-rally-marks/1" || doc["rally_run_id"] != run_id {
        return Err("Native bookmark run version mismatch".into());
    }
    let rows = doc["candidate_ids"]
        .as_array()
        .filter(|a| a.len() <= MAX_MARKS)
        .ok_or("Invalid native bookmark list")?;
    let mut result = BTreeSet::new();
    for value in rows {
        let id = value
            .as_str()
            .filter(|s| valid_candidate(s))
            .ok_or("Invalid saved rally bookmark")?;
        if !result.insert(id.to_owned()) {
            return Err("Duplicate saved rally bookmark".into());
        }
    }
    Ok(result)
}
fn render(
    project_id: &str,
    run_id: &str,
    revision: &str,
    marks: &BTreeSet<String>,
    writable: bool,
) -> Value {
    json!({
        "schema":"volley-native-rally-marks/1",
        "project_id":project_id,
        "rally_run_id":run_id,
        "revision":revision,
        "candidate_ids":marks,
        "marked_count":marks.len(),
        "writable":writable,
        "scope":"exact_versioned_provisional_rally_run",
    })
}
fn store(root: &Path, run_id: &str, ids: &BTreeSet<String>) -> Result<(), String> {
    versioned_store::write_json_checked(
        root,
        &root.join(format!("{run_id}.json")),
        &json!({
            "schema":"volley-native-rally-marks/1",
            "rally_run_id":run_id,
            "candidate_ids":ids,
        }),
        MAX_FILE,
    )
}
/// Internal, validation-only access for a run already checked under the
/// project archive guard. Does not reenter the preview verifier.
pub(crate) fn marked_for_run(
    state: &AppState,
    project_id: &str,
    run_id: &str,
) -> Result<BTreeSet<String>, ApiError> {
    storage::project_id(project_id)?;
    if !opaque_id(run_id) {
        return Err(storage::bad_request("Invalid native rally run"));
    }
    let root = projects::project_root(state)
        .join(project_id)
        .join("native_rally_marks");
    let bytes = bytes_for(&root, run_id).map_err(storage::bad_request)?;
    decode(bytes.as_deref(), run_id).map_err(storage::bad_request)
}
pub(crate) fn read_status(
    state: &AppState,
    project_id: &str,
    run_id: &str,
) -> Result<Value, ApiError> {
    storage::project_id(project_id)?;
    if !opaque_id(run_id) {
        return Err(storage::bad_request("Invalid native rally run ID"));
    }
    // Listing bookmarks is metadata-only. The immutable run and its source
    // lineage are validated here; a full SHA pass over the (potentially huge)
    // Touch JSONL belongs to candidate edits and montage, not every refresh.
    native_rally_preview::inspect_project(state, project_id, run_id)?;
    let root = projects::project_root(state)
        .join(project_id)
        .join("native_rally_marks");
    let bytes = bytes_for(&root, run_id).map_err(storage::bad_request)?;
    let marks = decode(bytes.as_deref(), run_id).map_err(storage::bad_request)?;
    Ok(render(
        project_id,
        run_id,
        &digest(bytes.as_deref()),
        &marks,
        state.writes_enabled,
    ))
}
fn change(state: &AppState, request: &SetMark) -> Result<Value, ApiError> {
    if !state.writes_enabled {
        return Err(storage::conflict(
            "Saving rally marks requires isolated project storage",
        ));
    }
    storage::project_id(&request.project_id)?;
    if !opaque_id(&request.rally_run_id) || !valid_candidate(&request.candidate_id) {
        return Err(storage::bad_request("Invalid native rally or candidate ID"));
    }
    if request.expected_revision != "absent"
        && (request.expected_revision.len() != 64
            || !request
                .expected_revision
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()))
    {
        return Err(storage::bad_request("Invalid gold marks revision"));
    }
    let _guard = project_guard::write_guard(state, &request.project_id)?;
    native_rally_preview::verify_run_or_candidate(
        state,
        &request.project_id,
        &request.rally_run_id,
        Some(&request.candidate_id),
    )?;
    let root = projects::project_root(state)
        .join(&request.project_id)
        .join("native_rally_marks");
    let existing = bytes_for(&root, &request.rally_run_id).map_err(storage::bad_request)?;
    if versioned_store::expect_revision(existing.as_deref(), &request.expected_revision).is_err() {
        return Err(storage::conflict(
            "Gold marks changed; refresh before editing",
        ));
    }
    let mut marks =
        decode(existing.as_deref(), &request.rally_run_id).map_err(storage::bad_request)?;
    let (updated, _) = edit_gold_set(&marks, &request.candidate_id, request.marked)
        .map_err(storage::bad_request)?;
    marks = updated; // Only write when actual state changes; a duplicate click is idempotent.
    let would_change = existing.is_none() || {
        let old =
            decode(existing.as_deref(), &request.rally_run_id).map_err(storage::bad_request)?;
        old != marks
    };
    if would_change {
        store(&root, &request.rally_run_id, &marks).map_err(storage::bad_request)?;
    }
    let updated = bytes_for(&root, &request.rally_run_id).map_err(storage::bad_request)?;
    Ok(render(
        &request.project_id,
        &request.rally_run_id,
        &digest(updated.as_deref()),
        &marks,
        true,
    ))
}
pub async fn get(
    State(state): State<Arc<AppState>>,
    Query(query): Query<MarksQuery>,
) -> ApiResult<Value> {
    let result = tokio::task::spawn_blocking(move || {
        // Read-only marks status does not mutate the project. The setter alone
        // requires the exclusive project write lock.
        read_status(&state, &query.project_id, &query.rally_run_id)
    })
    .await
    .map_err(|e| storage::internal(format!("Read gold rally marks: {e}")))??;
    Ok(Json(result))
}
pub async fn set(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<SetMark>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    let result = tokio::task::spawn_blocking(move || change(&state, &req))
        .await
        .map_err(|e| storage::internal(format!("Write gold rally mark: {e}")))??;
    Ok(Json(result))
}
#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::fs;
    #[test]
    fn mark_storage_revision_and_duplicate_protection() {
        let root = std::env::temp_dir().join(format!("volley-rally-gold-{}", uuid::Uuid::new_v4()));
        let run = "abcdef0123456789abcdef0123456789";
        let id = "aabbccddeeff001122334455";
        assert_eq!(digest(None), "absent");
        assert!(decode(None, run).unwrap().is_empty());
        let mut ids = BTreeSet::new();
        ids.insert(id.to_owned());
        store(&root, run, &ids).unwrap();
        let b = bytes_for(&root, run).unwrap().unwrap();
        assert_eq!(decode(Some(&b), run).unwrap(), ids);
        assert_eq!(digest(Some(&b)).len(), 64);
        assert!(decode(Some(&b), "00112233445566778899aabbccddeeff").is_err());
        let mut bad = serde_json::from_slice::<Value>(&b).unwrap();
        bad["candidate_ids"] = json!([id, id]);
        assert!(decode(Some(&serde_json::to_vec(&bad).unwrap()), run).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn native_project_gold_roundtrip_uses_expected_revision_and_real_candidate() {
        use std::process::Command;
        if Command::new("ffmpeg").arg("-version").output().is_err() {
            return;
        }
        let root =
            std::env::temp_dir().join(format!("volley-gold-integration-{}", uuid::Uuid::new_v4()));
        let repo = root.join("repo");
        let data = root.join("sandbox");
        let id = "1234abcdef01";
        let video = "fixture.mp4";
        let run_id = "0123456789abcdef0123456789abcdef";
        let replay_id = "abcdef0123456789abcdef0123456789";
        let touch_id = "00112233445566778899aabbccddeeff";
        let run_folder = data
            .join("projects")
            .join(id)
            .join("native_cpu_rallies")
            .join(run_id);
        let touch_folder = data
            .join("projects")
            .join(id)
            .join("native_cpu_touch_runs")
            .join(touch_id);
        fs::create_dir_all(repo.join("project")).unwrap();
        fs::create_dir_all(&run_folder).unwrap();
        fs::create_dir_all(&touch_folder).unwrap();
        let source = repo.join("project").join(video);
        let result = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=64x64:rate=30:duration=4",
                "-an",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-y",
            ])
            .arg(&source)
            .status()
            .unwrap();
        assert!(result.success());
        storage::write_json_atomic(
            &data.join("projects").join("projects.json"),
            &json!({"schema":1,"projects":[{"id":id,"videos":[video],"name":"Gold fixture"}]}),
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
        let plan = crate::preparation::plan_for(&state, id, video).unwrap();
        assert!(plan.ready_path().is_some());
        let (size, mtime) = crate::inference_stream::source_stamp(&source).unwrap();
        let identity = json!({"bytes":size,"mtime_ns":mtime});
        let candidate_id = format!(
            "{:x}",
            Sha256::digest(format!("{replay_id}:{touch_id}:10:100").as_bytes())
        )[..24]
            .to_string();
        let mut rows = Vec::new();
        rows.extend(
            serde_json::to_vec(&json!({
                "candidate_id":candidate_id,"detected_start_frame":10,
                "detected_end_frame":100,"ball_observations":90,
                "ball_coverage":0.95,"motion_supported_touch_proposals":2,
                "first_touch_frame":35,"last_touch_frame":75,
                "max_touch_probability":0.94,
                "classification":"provisional_rally",
                "authoritative_rally":false,
                "athlete_identities_verified":false,
                "serve_verified":false,"reviewed":false,
            }))
            .unwrap(),
        );
        rows.push(b'\n');
        fs::write(run_folder.join("rally_candidates.jsonl"), &rows).unwrap();
        let rally_sha = format!("{:x}", Sha256::digest(&rows));
        let mut touch_rows = Vec::new();
        for frame in [35u64, 75u64] {
            touch_rows.extend(
                serde_json::to_vec(&json!({
                    "frame_index":frame,"provisional_track_id":5,"probability":0.94,
                    "ball_x_px":32.0,"ball_y_px":32.0,"motion_frame_index":frame,
                    "direction_change_deg":90.0,"speed_before_px_frame":5.0,
                    "speed_after_px_frame":9.0,"velocity_residual_px_frame":7.0,
                    "technique":"unknown_touch","physical_motion_supported":true,
                    "athlete_identity_verified":false
                }))
                .unwrap(),
            );
            touch_rows.push(b'\n');
        }
        fs::write(touch_folder.join("touch_candidates.jsonl"), &touch_rows).unwrap();
        let touch_sha = format!("{:x}", Sha256::digest(&touch_rows));
        let replay_sha = "a".repeat(64);
        storage::write_json_atomic(
            &touch_folder.join("manifest.json"),
            &json!({
                "schema":"volley-native-cpu-touch-proposals/1",
                "state":"motion_gated_touch_proposals","project_id":id,
                "touch_id":touch_id,"video":video,"source_replay_id":replay_id,
                "source_replay_sha256":replay_sha,"source_identity":identity,
                "frames":120,"fps":30,"width":64,"height":64,
                "file":"touch_candidates.jsonl","sha256":touch_sha,
                "touch_candidates":2,"contacts_authoritatively_confirmed":false,
                "athlete_identities_verified":false,"cpu_processing_complete":false,
            }),
        )
        .unwrap();
        storage::write_json_atomic(
            &run_folder.join("manifest.json"),
            &json!({
                "schema":"volley-native-cpu-rally-proposals/1",
                "state":"provisional_rally_segments","project_id":id,
                "rally_run_id":run_id,"video":video,"source_replay_id":replay_id,
                "source_replay_sha256":replay_sha,"source_touch_id":touch_id,
                "source_touch_sha256":touch_sha,"source_identity":identity,
                "frames":120,"fps":30,"width":64,"height":64,
                "file":"rally_candidates.jsonl","sha256":rally_sha,
                "rally_candidates":1,"touch_candidates_ingested":2,
                "settings":{"suggested_context_frames":30},
                "rallies_authoritatively_confirmed":false,
                "athlete_identities_verified":false,"action_classification_complete":false,
                "montage_ready":false,"cpu_processing_complete":false
            }),
        )
        .unwrap();
        let initial = read_status(&state, id, run_id).unwrap();
        assert_eq!(initial["revision"], "absent");
        assert_eq!(initial["marked_count"], 0);
        let request = SetMark {
            project_id: id.into(),
            rally_run_id: run_id.into(),
            candidate_id: candidate_id.clone(),
            marked: true,
            expected_revision: "absent".into(),
        };
        let result = change(&state, &request).unwrap();
        assert_eq!(result["marked_count"], 1);
        assert_eq!(result["candidate_ids"][0], candidate_id);
        assert_eq!(read_status(&state, id, run_id).unwrap()["marked_count"], 1);
        // Compare-and-swap prevents stale client from destroying another edit.
        assert!(change(&state, &request).is_err());
        let current = result["revision"].as_str().unwrap().to_owned();
        let again = change(
            &state,
            &SetMark {
                expected_revision: current.clone(),
                ..request
            },
        )
        .unwrap();
        assert_eq!(again["revision"], current);
        assert!(change(
            &state,
            &SetMark {
                project_id: id.into(),
                rally_run_id: run_id.into(),
                candidate_id: "0".repeat(24),
                marked: true,
                expected_revision: current.clone(),
            }
        )
        .is_err());
        let removed = change(
            &state,
            &SetMark {
                project_id: id.into(),
                rally_run_id: run_id.into(),
                candidate_id: candidate_id.clone(),
                marked: false,
                expected_revision: current,
            },
        )
        .unwrap();
        assert_eq!(removed["marked_count"], 0);
        assert_eq!(read_status(&state, id, run_id).unwrap()["marked_count"], 0);
        let stored = fs::read(
            data.join("projects")
                .join(id)
                .join("native_rally_marks")
                .join(format!("{run_id}.json")),
        )
        .unwrap();
        assert!(stored.len() < MAX_FILE as usize);
        // Listing bookmarks must not rehash multi-GB Touch evidence. The
        // mutation path still verifies the immutable Touch SHA on every edit.
        fs::write(touch_folder.join("touch_candidates.jsonl"), b"corrupted\n").unwrap();
        assert_eq!(read_status(&state, id, run_id).unwrap()["marked_count"], 0);
        assert!(change(
            &state,
            &SetMark {
                project_id: id.into(),
                rally_run_id: run_id.into(),
                candidate_id: candidate_id.clone(),
                marked: true,
                expected_revision: removed["revision"].as_str().unwrap().to_owned(),
            }
        )
        .is_err());
        assert!(!repo.join("results").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reject_invalid_candidate_ids() {
        assert!(valid_candidate("abcdef0123456789abcdef01"));
        assert!(!valid_candidate("../bad"));
        assert!(!valid_candidate("AAAAAAAAAAAAAAAAAAAAAAAA"));
    }
}
