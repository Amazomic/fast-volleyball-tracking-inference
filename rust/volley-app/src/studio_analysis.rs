//! Read-only bounded inventory of isolated native GPU -> CPU stages.
//! This is a *metadata-only* overview; individual processing endpoints
//! revalidate actual SHA-256 evidence before consuming their source.
use crate::{
    cpu_retrack::{opaque_id, safe_file, safe_folder},
    projects,
    storage::{self, ApiError, ApiResult},
    AppState,
};
use axum::{
    extract::{Query, State},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{fs, path::Path, sync::Arc, time::SystemTime};

const MAX_STAGE_ENTRIES: usize = 350;
const RETURN_STAGE_ENTRIES: usize = 60;
const MAX_MANIFEST: u64 = 16 * 1024;
const STAGES: [(&str, &str, &str, &str); 5] = [
    (
        "gpu",
        "native_gpu_runs",
        "volley-native-gpu-model-detections/1",
        "job_id",
    ),
    (
        "fusion",
        "native_gpu_fusions",
        "volley-native-gpu-fusion/1",
        "fusion_id",
    ),
    (
        "tracking",
        "native_cpu_replays",
        "volley-native-cpu-tracking/1",
        "replay_id",
    ),
    (
        "touches",
        "native_cpu_touch_runs",
        "volley-native-cpu-touch-proposals/1",
        "touch_id",
    ),
    (
        "rallies",
        "native_cpu_rallies",
        "volley-native-cpu-rally-proposals/1",
        "rally_run_id",
    ),
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnalysisQuery {
    project_id: String,
    video: String,
}
fn valid_sha(item: &Value) -> bool {
    item.as_str().is_some_and(|sha| {
        sha.len() == 64
            && sha
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}
fn trustworthy_manifest(
    manifest: &Value,
    project_id: &str,
    video: &str,
    id: &str,
    stage: &str,
    schema: &str,
    id_field: &str,
) -> bool {
    if manifest["schema"] != schema
        || manifest["project_id"].as_str() != Some(project_id)
        || manifest["video"].as_str() != Some(video)
        || manifest[id_field].as_str() != Some(id)
        || manifest["fps"] != 30
        || !valid_sha(&manifest["sha256"])
    {
        return false;
    }
    let source = &manifest["source_identity"];
    if !source["bytes"].is_u64() || !source["mtime_ns"].is_u64() {
        return false;
    }
    match stage {
        "gpu" => {
            manifest["status"] == "model_raw_detections_complete"
                && manifest["raw_candidates_only"] == true
                && matches!(manifest["model"].as_str(), Some("ball" | "players"))
        }
        "fusion" => {
            manifest["state"] == "raw_candidates_fused"
                && manifest["player_track_ids_confirmed"] == false
        }
        "tracking" => {
            manifest["state"] == "provisional_tracklets_rebuilt"
                && manifest["track_ids_provisional"] == true
                && manifest["athlete_identities_confirmed"] == false
        }
        "touches" => {
            manifest["state"] == "motion_gated_touch_proposals"
                && manifest["contacts_authoritatively_confirmed"] == false
        }
        "rallies" => {
            manifest["state"] == "provisional_rally_segments"
                && manifest["rallies_authoritatively_confirmed"] == false
                && manifest["montage_ready"] == false
        }
        _ => false,
    }
}
fn checked_manifest(folder: &Path) -> Option<Value> {
    let file = safe_file(folder, "manifest.json", MAX_MANIFEST).ok()?;
    let content = fs::read(file).ok()?;
    serde_json::from_slice::<Value>(&content).ok()
}
fn created(folder: &Path) -> u64 {
    fs::metadata(folder)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|stamp| stamp.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map_or(0, |v| v.as_secs())
}
fn stage_summary(
    stage: &str,
    root: &Path,
    project_id: &str,
    video: &str,
) -> Result<Value, ApiError> {
    if !root.exists() {
        return Ok(json!({"key":stage,"items":[],"total":0,"truncated":false}));
    }
    safe_folder(root).map_err(storage::bad_request)?;
    let (_, _, schema, id_field) = STAGES
        .iter()
        .find(|(s, _, _, _)| *s == stage)
        .ok_or_else(|| storage::internal("Unknown native analysis stage"))?;
    let mut found = Vec::new();
    let mut dirs = 0usize;
    for entry in
        fs::read_dir(root).map_err(|e| storage::internal(format!("List {stage} stage: {e}")))?
    {
        let entry = entry.map_err(|e| storage::internal(format!("Read stage entry: {e}")))?;
        let id = entry.file_name().to_string_lossy().to_string();
        if !opaque_id(&id) {
            continue;
        }
        dirs += 1;
        if dirs > MAX_STAGE_ENTRIES {
            return Err(storage::bad_request(
                "Too many stage versions to safely enumerate",
            ));
        }
        let folder = entry.path();
        if safe_folder(&folder).is_err() {
            continue;
        }
        let Some(record) = checked_manifest(&folder) else {
            continue;
        };
        if !trustworthy_manifest(&record, project_id, video, &id, stage, schema, id_field) {
            continue;
        }
        let state = match stage {
            "gpu" => "model_raw_detections_complete",
            "fusion" => "raw_candidates_fused",
            "tracking" => "provisional_tracklets_rebuilt",
            "touches" => "motion_gated_touch_proposals",
            "rallies" => "provisional_rally_segments",
            _ => "unknown",
        };
        let count_key = match stage {
            "gpu" => "windows",
            "fusion" => "emitted_frames",
            "tracking" => "provisional_track_count",
            "touches" => "touch_candidates",
            "rallies" => "rally_candidates",
            _ => "frames",
        };
        found.push((
            created(&folder),
            json!({
                "id":id,"kind":stage,"state":state,"model":record["model"],
                "display_name":crate::analysis_management::display_name(root.parent().ok_or_else(|| storage::internal("Missing project root"))?, if stage == "tracking" { "unlinked" } else { stage }, &id)?,
                "created_at":created(&folder),
                "person_job_id":record["sources"]["person_pose"]["job_id"],
                "vball_job_id":record["sources"]["vballnet"]["job_id"],
                "count":record[count_key],"count_unit":count_key,
                "source_fusion_id":record["source_fusion_id"],
                "source_replay_id":record["source_replay_id"],
                "source_touch_id":record["source_touch_id"],
                "sha256":record["sha256"],
                "source_identity":record["source_identity"],
                "metadata_only":true,
                "authoritative_game_events":false,
            }),
        ));
    }
    found.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1["id"].as_str().cmp(&a.1["id"].as_str()))
    });
    let total = found.len();
    Ok(json!({
        "key":stage,"items":found.into_iter().take(RETURN_STAGE_ENTRIES)
            .map(|(_,v)|v).collect::<Vec<_>>(),
        "total":total,"truncated":total>RETURN_STAGE_ENTRIES
    }))
}
fn overview(state: &AppState, project_id: &str, video: &str) -> Result<Value, ApiError> {
    storage::project_id(project_id)?;
    storage::video_name(video)?;
    let project = projects::existing_project(state, project_id)?;
    if !project["videos"]
        .as_array()
        .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(video)))
    {
        return Err(storage::bad_request(
            "Video does not belong to the selected project",
        ));
    }
    let project_root = projects::project_root(state).join(project_id);
    safe_folder(&project_root).map_err(storage::bad_request)?;
    let _guard = if state.writes_enabled {
        Some(crate::analysis_management::catalog_guard(
            state, project_id,
        )?)
    } else {
        None
    };
    if state.writes_enabled {
        crate::analysis_management::recover(&project_root)?;
    }
    let mut stages = Vec::new();
    for (stage, folder, _, _) in STAGES {
        stages.push(stage_summary(
            stage,
            &project_root.join(folder),
            project_id,
            video,
        )?);
    }
    Ok(json!({
        "schema":"volley-native-analysis-overview/1",
        "project_id":project_id,"video":video,"stages":stages,
        "metadata_only":true,"hashes_not_reverified":true,
        "pipeline_complete":false,"authoritative_game_events":false,
    }))
}
pub async fn get(
    State(state): State<Arc<AppState>>,
    Query(query): Query<AnalysisQuery>,
) -> ApiResult<Value> {
    let result =
        tokio::task::spawn_blocking(move || overview(&state, &query.project_id, &query.video))
            .await
            .map_err(|e| storage::internal(format!("Native overview task: {e}")))??;
    Ok(Json(result))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distrust_foreign_or_incomplete_artifact_manifests() {
        let p = "0123456789ab";
        let run = "abcdef0123456789abcdef0123456789";
        let mut item = json!({
            "schema":"volley-native-cpu-touch-proposals/1","project_id":p,
            "video":"game.mp4","touch_id":run,"fps":30,
            "sha256":"a".repeat(64),"source_identity":{"bytes":1024,"mtime_ns":120},
            "state":"motion_gated_touch_proposals","contacts_authoritatively_confirmed":false,
        });
        assert!(trustworthy_manifest(
            &item,
            p,
            "game.mp4",
            run,
            "touches",
            "volley-native-cpu-touch-proposals/1",
            "touch_id"
        ));
        assert!(!trustworthy_manifest(
            &item,
            "987654321abc",
            "game.mp4",
            run,
            "touches",
            "volley-native-cpu-touch-proposals/1",
            "touch_id"
        ));
        item["contacts_authoritatively_confirmed"] = json!(true);
        assert!(!trustworthy_manifest(
            &item,
            p,
            "game.mp4",
            run,
            "touches",
            "volley-native-cpu-touch-proposals/1",
            "touch_id"
        ));
    }
    #[test]
    fn scoped_metadata_inventory_ignores_foreign_and_unverified_status() {
        let root =
            std::env::temp_dir().join(format!("volley-native-overview-{}", uuid::Uuid::new_v4()));
        let data = root.join("sandbox");
        let project_id = "123456abcdef";
        let run = "0123456789abcdef0123456789abcdef";
        let touch = data
            .join("projects")
            .join(project_id)
            .join("native_cpu_touch_runs")
            .join(run);
        fs::create_dir_all(&touch).unwrap();
        storage::write_json_atomic(
            &data.join("projects").join("projects.json"),
            &json!({"schema":1,"projects":[{"id":project_id,
                "name":"Test","videos":["match.mp4"]}]}),
        )
        .unwrap();
        let state = AppState {
            repository_root: root.join("original"),
            data_root: data.clone(),
            writes_enabled: false,
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
        let mut record = json!({
            "schema":"volley-native-cpu-touch-proposals/1",
            "state":"motion_gated_touch_proposals",
            "project_id":project_id,"video":"match.mp4","touch_id":run,
            "fps":30,"touch_candidates":5,"sha256":"a".repeat(64),
            "source_identity":{"bytes":3456,"mtime_ns":999},
            "contacts_authoritatively_confirmed":false
        });
        storage::write_json_atomic(&touch.join("manifest.json"), &record).unwrap();
        let response = overview(&state, project_id, "match.mp4").unwrap();
        assert_eq!(response["stages"][3]["total"], 1);
        assert_eq!(response["stages"][3]["items"][0]["count"], 5);
        assert_eq!(response["stages"][3]["items"][0]["metadata_only"], true);
        assert_eq!(response["hashes_not_reverified"], true);
        assert!(overview(&state, project_id, "other.mp4").is_err());
        record["contacts_authoritatively_confirmed"] = json!(true);
        storage::write_json_atomic(&touch.join("manifest.json"), &record).unwrap();
        assert_eq!(
            overview(&state, project_id, "match.mp4").unwrap()["stages"][3]["total"],
            0
        );
        record["contacts_authoritatively_confirmed"] = json!(false);
        record["project_id"] = json!("999999999999");
        storage::write_json_atomic(&touch.join("manifest.json"), &record).unwrap();
        assert_eq!(
            overview(&state, project_id, "match.mp4").unwrap()["stages"][3]["total"],
            0
        );
        assert!(!state.repository_root.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn inventory_is_bounded_and_empty_root_is_safe() {
        let root =
            std::env::temp_dir().join(format!("volley-empty-analysis-{}", uuid::Uuid::new_v4()));
        let item = stage_summary("rallies", &root, "0123456789ab", "game.mp4").unwrap();
        assert_eq!(item["total"], 0);
        assert_eq!(item["truncated"], false);
    }
}
