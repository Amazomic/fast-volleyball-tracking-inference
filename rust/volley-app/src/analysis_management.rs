#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;

    fn record(root: &Path, folder: &str, id: &str, mut data: Value) {
        let (schema, field) = match folder {
            "native_gpu_runs" => ("volley-native-gpu-model-detections/1", "job_id"),
            "native_gpu_fusions" => ("volley-native-gpu-fusion/1", "fusion_id"),
            "native_cpu_replays" => ("volley-native-cpu-tracking/1", "replay_id"),
            "native_cpu_touch_runs" => ("volley-native-cpu-touch-proposals/1", "touch_id"),
            "native_cpu_rallies" => ("volley-native-cpu-rally-proposals/1", "rally_run_id"),
            _ => panic!("unknown fixture"),
        };
        data["schema"] = json!(schema);
        data[field] = json!(id);
        data["project_id"] = json!("123456abcdef");
        data["video"] = json!("match.mp4");
        let path = root.join(folder).join(id);
        fs::create_dir_all(&path).unwrap();
        fs::write(
            path.join("manifest.json"),
            serde_json::to_vec(&data).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn rename_and_delete_primary_preserve_unrelated_analysis() {
        let root = std::env::temp_dir().join(format!("volley-analysis-edit-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let f = "11111111111111111111111111111111";
        let other = "22222222222222222222222222222222";
        let replay = "33333333333333333333333333333333";
        let touch = "44444444444444444444444444444444";
        let rally = "55555555555555555555555555555555";
        record(&root, "native_gpu_fusions", f, json!({}));
        record(&root, "native_gpu_fusions", other, json!({}));
        record(
            &root,
            "native_cpu_replays",
            replay,
            json!({"source_fusion_id":f}),
        );
        record(
            &root,
            "native_cpu_touch_runs",
            touch,
            json!({"source_replay_id":replay}),
        );
        record(
            &root,
            "native_cpu_rallies",
            rally,
            json!({"source_replay_id":replay,"source_touch_id":touch}),
        );
        let mut request = EditRequest {
            project_id: "123456abcdef".into(),
            video: "match.mp4".into(),
            kind: "fusion".into(),
            id: f.into(),
            action: "rename".into(),
            name: Some("Мой анализ".into()),
        };
        edit(&root, &request).unwrap();
        assert_eq!(
            display_name(&root, "fusion", f).unwrap().as_deref(),
            Some("Мой анализ")
        );
        request.action = "delete".into();
        edit(&root, &request).unwrap();
        assert!(!root.join("native_gpu_fusions").join(f).exists());
        assert!(!root.join("native_cpu_replays").join(replay).exists());
        assert!(!root.join("native_cpu_touch_runs").join(touch).exists());
        assert!(!root.join("native_cpu_rallies").join(rally).exists());
        assert!(root.join("native_gpu_fusions").join(other).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn deleting_secondary_keeps_the_source_and_other_results() {
        let root = std::env::temp_dir().join(format!("volley-analysis-edit-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let replay = "33333333333333333333333333333333";
        let first = "44444444444444444444444444444444";
        let second = "55555555555555555555555555555555";
        record(&root, "native_cpu_replays", replay, json!({}));
        record(
            &root,
            "native_cpu_rallies",
            first,
            json!({"source_replay_id":replay}),
        );
        record(
            &root,
            "native_cpu_rallies",
            second,
            json!({"source_replay_id":replay}),
        );
        let request = EditRequest {
            project_id: "123456abcdef".into(),
            video: "match.mp4".into(),
            kind: "rallies".into(),
            id: first.into(),
            action: "delete".into(),
            name: None,
        };
        edit(&root, &request).unwrap();
        assert!(!root.join("native_cpu_rallies").join(first).exists());
        assert!(root.join("native_cpu_rallies").join(second).exists());
        assert!(root.join("native_cpu_replays").join(replay).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn deleting_secondary_removes_only_unshared_postprocessing_data() {
        let root = std::env::temp_dir().join(format!("volley-analysis-edit-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let replay = "33333333333333333333333333333333";
        let touch = "44444444444444444444444444444444";
        let rally = "55555555555555555555555555555555";
        record(&root, "native_cpu_replays", replay, json!({}));
        record(
            &root,
            "native_cpu_touch_runs",
            touch,
            json!({"source_replay_id":replay}),
        );
        record(
            &root,
            "native_cpu_rallies",
            rally,
            json!({"source_replay_id":replay,"source_touch_id":touch}),
        );
        let request = EditRequest {
            project_id: "123456abcdef".into(),
            video: "match.mp4".into(),
            kind: "rallies".into(),
            id: rally.into(),
            action: "delete".into(),
            name: None,
        };
        edit(&root, &request).unwrap();
        assert!(!root.join("native_cpu_rallies").join(rally).exists());
        assert!(!root.join("native_cpu_touch_runs").join(touch).exists());
        assert!(!root.join("native_cpu_replays").join(replay).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn incomplete_gpu_job_does_not_prevent_renaming_a_completed_result() {
        let root = std::env::temp_dir().join(format!("volley-analysis-edit-{}", Uuid::new_v4()));
        fs::create_dir_all(
            root.join("native_gpu_runs")
                .join("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        )
        .unwrap();
        let id = "11111111111111111111111111111111";
        record(&root, "native_gpu_fusions", id, json!({}));
        let request = EditRequest {
            project_id: "123456abcdef".into(),
            video: "match.mp4".into(),
            kind: "fusion".into(),
            id: id.into(),
            action: "rename".into(),
            name: Some("Название".into()),
        };
        edit(&root, &request).unwrap();
        assert_eq!(
            display_name(&root, "fusion", id).unwrap().as_deref(),
            Some("Название")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn interrupted_delete_restores_moved_analysis() {
        let root = std::env::temp_dir().join(format!("volley-analysis-edit-{}", Uuid::new_v4()));
        let id = "11111111111111111111111111111111";
        record(&root, "native_gpu_fusions", id, json!({}));
        let txn = root.join("native_analysis_deletion");
        fs::create_dir(&txn).unwrap();
        storage::write_json_atomic(
            &txn.join("journal.json"),
            &json!({"paths":[["native_gpu_fusions",id]]}),
        )
        .unwrap();
        fs::rename(root.join("native_gpu_fusions").join(id), txn.join("0")).unwrap();
        recover(&root).unwrap();
        assert!(root.join("native_gpu_fusions").join(id).exists());
        assert!(!txn.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn committed_delete_finishes_interrupted_cleanup() {
        let root = std::env::temp_dir().join(format!("volley-analysis-edit-{}", Uuid::new_v4()));
        fs::create_dir_all(root.join("native_analysis_deletion").join("0")).unwrap();
        storage::write_json_atomic(
            &root.join("native_analysis_deletion_committed.json"),
            &json!({"paths":[]}),
        )
        .unwrap();
        recover(&root).unwrap();
        assert!(!root.join("native_analysis_deletion").exists());
        assert!(!root
            .join("native_analysis_deletion_committed.json")
            .exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_restores_every_interruption_point_before_commit() {
        for moved in 0..=3 {
            let root = std::env::temp_dir()
                .join(format!("volley-analysis-interruption-{}", Uuid::new_v4()));
            let items = [
                ("native_gpu_fusions", "11111111111111111111111111111111"),
                ("native_cpu_replays", "22222222222222222222222222222222"),
                ("native_cpu_rallies", "33333333333333333333333333333333"),
            ];
            for (folder, id) in items {
                record(&root, folder, id, json!({}));
            }
            let txn = root.join("native_analysis_deletion");
            fs::create_dir(&txn).unwrap();
            storage::write_json_atomic(&txn.join("journal.json"), &json!({"paths":items})).unwrap();
            for (index, (folder, id)) in items.iter().enumerate().take(moved) {
                fs::rename(root.join(folder).join(id), txn.join(index.to_string())).unwrap();
            }
            recover(&root).unwrap();
            for (folder, id) in items {
                assert!(root.join(folder).join(id).join("manifest.json").is_file());
            }
            assert!(!txn.exists());
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn recovery_finishes_cleanup_after_each_committed_deletion_step() {
        for remaining in 0..=3 {
            let root =
                std::env::temp_dir().join(format!("volley-analysis-committed-{}", Uuid::new_v4()));
            let txn = root.join("native_analysis_deletion");
            fs::create_dir_all(&txn).unwrap();
            for index in 0..remaining {
                fs::create_dir(txn.join(index.to_string())).unwrap();
            }
            storage::write_json_atomic(
                &root.join("native_analysis_deletion_committed.json"),
                &json!({"paths":[]}),
            )
            .unwrap();
            recover(&root).unwrap();
            assert!(!txn.exists());
            assert!(!root
                .join("native_analysis_deletion_committed.json")
                .exists());
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn recovery_removes_incomplete_journal_before_any_move() {
        let root = std::env::temp_dir().join(format!("volley-analysis-prepare-{}", Uuid::new_v4()));
        let txn = root.join("native_analysis_deletion");
        fs::create_dir_all(&txn).unwrap();
        fs::write(
            txn.join(format!(".journal.json.{}.tmp", Uuid::new_v4())),
            b"partial",
        )
        .unwrap();
        recover(&root).unwrap();
        assert!(!txn.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cross_video_touch_reference_rejects_deletion() {
        let root = std::env::temp_dir().join(format!("volley-analysis-edit-{}", Uuid::new_v4()));
        let fusion = "11111111111111111111111111111111";
        let replay = "22222222222222222222222222222222";
        let touch = "33333333333333333333333333333333";
        let rally = "44444444444444444444444444444444";
        record(&root, "native_gpu_fusions", fusion, json!({}));
        record(
            &root,
            "native_cpu_replays",
            replay,
            json!({"source_fusion_id":fusion}),
        );
        record(
            &root,
            "native_cpu_touch_runs",
            touch,
            json!({"source_replay_id":replay}),
        );
        record(
            &root,
            "native_cpu_rallies",
            rally,
            json!({"source_replay_id":replay,"source_touch_id":touch}),
        );
        let path = root
            .join("native_cpu_touch_runs")
            .join(touch)
            .join("manifest.json");
        let mut value = storage::read_json(&path).unwrap().unwrap();
        value["video"] = json!("other.mp4");
        storage::write_json_atomic(&path, &value).unwrap();
        let request = EditRequest {
            project_id: "123456abcdef".into(),
            video: "match.mp4".into(),
            kind: "fusion".into(),
            id: fusion.into(),
            action: "delete".into(),
            name: None,
        };
        assert!(edit(&root, &request).is_err());
        assert!(root.join("native_gpu_fusions").join(fusion).exists());
        assert!(root.join("native_cpu_touch_runs").join(touch).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn external_video_reference_prevents_deleting_shared_replay() {
        let root = std::env::temp_dir().join(format!("volley-analysis-edit-{}", Uuid::new_v4()));
        let fusion = "11111111111111111111111111111111";
        let replay = "22222222222222222222222222222222";
        let rally = "44444444444444444444444444444444";
        record(&root, "native_gpu_fusions", fusion, json!({}));
        record(
            &root,
            "native_cpu_replays",
            replay,
            json!({"source_fusion_id":fusion}),
        );
        record(
            &root,
            "native_cpu_rallies",
            rally,
            json!({"source_replay_id":replay}),
        );
        let path = root
            .join("native_cpu_rallies")
            .join(rally)
            .join("manifest.json");
        let mut value = storage::read_json(&path).unwrap().unwrap();
        value["video"] = json!("other.mp4");
        storage::write_json_atomic(&path, &value).unwrap();
        let request = EditRequest {
            project_id: "123456abcdef".into(),
            video: "match.mp4".into(),
            kind: "fusion".into(),
            id: fusion.into(),
            action: "delete".into(),
            name: None,
        };
        assert!(edit(&root, &request).is_err());
        assert!(root.join("native_cpu_replays").join(replay).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupted_identity_rejects_cascade_without_deleting() {
        let root = std::env::temp_dir().join(format!("volley-analysis-edit-{}", Uuid::new_v4()));
        let id = "11111111111111111111111111111111";
        let replay = "22222222222222222222222222222222";
        record(&root, "native_gpu_fusions", id, json!({}));
        record(
            &root,
            "native_cpu_replays",
            replay,
            json!({"source_fusion_id":id,"replay_id":"wrong"}),
        );
        let path = root
            .join("native_cpu_replays")
            .join(replay)
            .join("manifest.json");
        let mut value = storage::read_json(&path).unwrap().unwrap();
        value["replay_id"] = json!("wrong");
        storage::write_json_atomic(&path, &value).unwrap();
        let request = EditRequest {
            project_id: "123456abcdef".into(),
            video: "match.mp4".into(),
            kind: "fusion".into(),
            id: id.into(),
            action: "delete".into(),
            name: None,
        };
        assert!(edit(&root, &request).is_err());
        assert!(root.join("native_gpu_fusions").join(id).exists());
        assert!(root.join("native_cpu_replays").join(replay).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn primary_deletion_follows_only_its_linked_postprocessing() {
        let entries = vec![
            ("tracking", "a", json!({"source_fusion_id":"f1"})),
            ("tracking", "b", json!({"source_fusion_id":"f2"})),
            ("touches", "c", json!({"source_replay_id":"a"})),
            ("touches", "d", json!({"source_replay_id":"b"})),
            (
                "rallies",
                "e",
                json!({"source_replay_id":"a","source_touch_id":"c"}),
            ),
            (
                "rallies",
                "f",
                json!({"source_replay_id":"b","source_touch_id":"d"}),
            ),
        ];
        let result = descendants("f1", &entries);
        assert_eq!(
            result,
            vec![("tracking", "a"), ("touches", "c"), ("rallies", "e")]
        );
    }
}
use crate::{
    cpu_retrack::{opaque_id, safe_folder},
    project_guard, projects,
    storage::{self, ApiError, ApiResult},
    AppState,
};
use axum::{extract::State, http::HeaderMap, Json};
use fs2::FileExt;
use serde::Deserialize;
use serde_json::json;
use serde_json::Value;
use std::collections::HashSet;
use std::{fs, fs::File, path::Path, sync::Arc};

pub(crate) fn catalog_guard(state: &AppState, project_id: &str) -> Result<File, ApiError> {
    storage::project_id(project_id)?;
    let root = projects::project_root(state);
    safe_folder(&root).map_err(storage::bad_request)?;
    let locks = root.join(".analysis-locks");
    match fs::create_dir(&locks) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(storage::internal(format!("Analysis locks: {error}"))),
    }
    safe_folder(&locks).map_err(storage::bad_request)?;
    let path = locks.join(format!("{project_id}.lock"));
    if fs::symlink_metadata(&path).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return Err(storage::bad_request("Небезопасная блокировка анализа"));
    }
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|e| storage::internal(format!("Analysis lock: {e}")))?;
    lock.lock_exclusive()
        .map_err(|e| storage::internal(format!("Lock analysis: {e}")))?;
    Ok(lock)
}

pub(crate) fn recover(root: &Path) -> Result<(), ApiError> {
    let txn = root.join("native_analysis_deletion");
    let committed = root.join("native_analysis_deletion_committed.json");
    if committed.exists() {
        storage::read_json(&committed)?
            .ok_or_else(|| storage::conflict("Повреждён журнал удаления"))?;
        if txn.exists() {
            safe_folder(&txn).map_err(storage::bad_request)?;
            fs::remove_dir_all(&txn)
                .map_err(|e| storage::internal(format!("Finish deletion: {e}")))?;
        }
        fs::remove_file(committed)
            .map_err(|e| storage::internal(format!("Finish deletion journal: {e}")))?;
        return Ok(());
    }
    if !txn.exists() {
        return Ok(());
    }
    safe_folder(&txn).map_err(storage::bad_request)?;
    if !txn.join("journal.json").exists() {
        for entry in fs::read_dir(&txn)
            .map_err(|e| storage::internal(format!("Read deletion journal: {e}")))?
        {
            let entry =
                entry.map_err(|e| storage::internal(format!("Read deletion entry: {e}")))?;
            let name = entry.file_name().to_string_lossy().to_string();
            let Some(id) = name
                .strip_prefix(".journal.json.")
                .and_then(|value| value.strip_suffix(".tmp"))
            else {
                return Err(storage::conflict(
                    "Незавершённое удаление требует восстановления",
                ));
            };
            if uuid::Uuid::parse_str(id).is_err()
                || !entry
                    .file_type()
                    .map_err(|e| storage::internal(format!("Inspect deletion entry: {e}")))?
                    .is_file()
            {
                return Err(storage::conflict("Небезопасный журнал удаления"));
            }
            fs::remove_file(entry.path())
                .map_err(|e| storage::internal(format!("Remove temporary journal: {e}")))?;
        }
        fs::remove_dir(&txn)
            .map_err(|e| storage::internal(format!("Remove empty deletion journal: {e}")))?;
        return Ok(());
    }
    let journal = storage::read_json(&txn.join("journal.json"))?
        .ok_or_else(|| storage::conflict("Незавершённое удаление требует восстановления"))?;
    let paths = journal["paths"]
        .as_array()
        .ok_or_else(|| storage::conflict("Повреждён журнал удаления"))?;
    for (index, item) in paths.iter().enumerate().rev() {
        let folder = item[0]
            .as_str()
            .ok_or_else(|| storage::conflict("Повреждён журнал удаления"))?;
        let name = item[1]
            .as_str()
            .ok_or_else(|| storage::conflict("Повреждён журнал удаления"))?;
        let stage_folder =
            STAGES.iter().any(|(_, path)| *path == folder) || folder == "native_gpu_pose_runs";
        let sidecar_folder = [
            "native_rally_marks",
            "native_action_reviews",
            "native_track_assignments",
            "native_reid_reviews",
        ]
        .contains(&folder);
        let valid_name = if stage_folder {
            opaque_id(name)
        } else {
            sidecar_folder && name.strip_suffix(".json").is_some_and(opaque_id)
        };
        if !valid_name {
            return Err(storage::conflict("Повреждён журнал удаления"));
        }
        let parent = root.join(folder);
        safe_folder(&parent).map_err(storage::bad_request)?;
        let saved = txn.join(index.to_string());
        let original = parent.join(name);
        if saved.exists() {
            if original.exists() {
                return Err(storage::conflict(
                    "Невозможно восстановить результат: имя занято",
                ));
            }
            fs::rename(saved, original)
                .map_err(|e| storage::internal(format!("Restore analysis: {e}")))?;
        }
    }
    fs::remove_dir_all(txn)
        .map_err(|e| storage::internal(format!("Remove deletion journal: {e}")))?;
    Ok(())
}

pub(crate) fn recover_projects(state: &AppState) -> Result<(), ApiError> {
    if !state.writes_enabled {
        return Ok(());
    }
    for project in projects::load_projects(state)? {
        let Some(id) = project["id"].as_str() else {
            continue;
        };
        storage::project_id(id)?;
        let root = projects::project_root(state).join(id);
        if !root.exists() {
            continue;
        }
        safe_folder(&root).map_err(storage::bad_request)?;
        if root.join("native_analysis_deletion").exists()
            || root
                .join("native_analysis_deletion_committed.json")
                .exists()
        {
            let _lock = project_guard::write_guard(state, id)?;
            recover(&root)?;
        }
    }
    Ok(())
}

const STAGES: [(&str, &str); 5] = [
    ("gpu", "native_gpu_runs"),
    ("fusion", "native_gpu_fusions"),
    ("tracking", "native_cpu_replays"),
    ("touches", "native_cpu_touch_runs"),
    ("rallies", "native_cpu_rallies"),
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditRequest {
    project_id: String,
    video: String,
    kind: String,
    id: String,
    action: String,
    name: Option<String>,
}

fn entries(root: &Path) -> Result<Vec<(String, String, Value)>, ApiError> {
    let mut result = Vec::new();
    for (stage, folder) in STAGES {
        let dir = root.join(folder);
        if !dir.exists() {
            continue;
        }
        safe_folder(&dir).map_err(storage::bad_request)?;
        for entry in
            fs::read_dir(dir).map_err(|e| storage::internal(format!("Read analyses: {e}")))?
        {
            let entry = entry.map_err(|e| storage::internal(format!("Read analysis: {e}")))?;
            let id = entry.file_name().to_string_lossy().to_string();
            if !opaque_id(&id) {
                continue;
            }
            safe_folder(&entry.path()).map_err(storage::bad_request)?;
            let Some(value) = storage::read_json(&entry.path().join("manifest.json"))? else {
                continue;
            };
            let (schema, field) = match stage {
                "gpu" => ("volley-native-gpu-model-detections/1", "job_id"),
                "fusion" => ("volley-native-gpu-fusion/1", "fusion_id"),
                "tracking" => ("volley-native-cpu-tracking/1", "replay_id"),
                "touches" => ("volley-native-cpu-touch-proposals/1", "touch_id"),
                "rallies" => ("volley-native-cpu-rally-proposals/1", "rally_run_id"),
                _ => return Err(storage::internal("Unknown analysis stage")),
            };
            if value["schema"] != schema || value[field] != id {
                return Err(storage::conflict("Повреждены сведения об анализе"));
            }
            result.push((stage.to_string(), id, value));
            if result.len() > 10000 {
                return Err(storage::conflict(
                    "Слишком много результатов для безопасного удаления",
                ));
            }
        }
    }
    Ok(result)
}

pub(crate) fn display_name(root: &Path, kind: &str, id: &str) -> Result<Option<String>, ApiError> {
    let names =
        storage::read_json(&root.join("native_analysis_names.json"))?.unwrap_or_else(|| json!({}));
    Ok(names[format!("{kind}:{id}")].as_str().map(str::to_owned))
}

fn edit(root: &Path, req: &EditRequest) -> Result<(), ApiError> {
    recover(root)?;
    if !opaque_id(&req.id)
        || !matches!(req.kind.as_str(), "gpu" | "fusion" | "unlinked" | "rallies")
    {
        return Err(storage::bad_request("Некорректный результат"));
    }
    let all = entries(root)?;
    let target_kind = if req.kind == "unlinked" {
        "tracking"
    } else {
        req.kind.as_str()
    };
    let target = all
        .iter()
        .find(|(stage, id, row)| {
            stage == target_kind
                && id == &req.id
                && row["video"] == req.video
                && row["project_id"] == req.project_id
        })
        .or_else(|| {
            if req.kind == "unlinked" {
                all.iter().find(|(stage, id, row)| {
                    stage == "rallies"
                        && (id == &req.id || row["source_replay_id"] == req.id)
                        && row["video"] == req.video
                        && row["project_id"] == req.project_id
                })
            } else {
                None
            }
        })
        .ok_or_else(|| storage::not_found("Результат не найден"))?;
    let names_path = root.join("native_analysis_names.json");
    let mut names = storage::read_json(&names_path)?.unwrap_or_else(|| json!({}));
    let names_map = names
        .as_object_mut()
        .ok_or_else(|| storage::conflict("Некорректные имена анализов"))?;
    let key = format!("{}:{}", req.kind, req.id);
    if req.action == "rename" {
        let name = req.name.as_deref().unwrap_or("").trim();
        if name.is_empty() || name.chars().count() > 100 || name.chars().any(char::is_control) {
            return Err(storage::bad_request(
                "Введите название длиной от 1 до 100 символов",
            ));
        }
        names_map.insert(key, json!(name));
        return storage::write_json_atomic(&names_path, &names);
    }
    if req.action != "delete" {
        return Err(storage::bad_request("Неизвестное действие"));
    }
    if req.kind == "gpu"
        && all.iter().any(|(kind, _, v)| {
            kind == "fusion"
                && (v["sources"]["ravel"]["job_id"] == req.id
                    || v["sources"]["vballnet"]["job_id"] == req.id)
        })
    {
        return Err(storage::conflict(
            "Этот GPU-результат используется другим анализом",
        ));
    }
    let mut selected = vec![(target.0.as_str(), target.1.as_str())];
    let refs: Vec<_> = all
        .iter()
        .filter(|(_, _, v)| v["project_id"] == req.project_id && v["video"] == req.video)
        .map(|(a, b, c)| (a.as_str(), b.as_str(), c.clone()))
        .collect();
    for (stage, _, rally) in &refs {
        if *stage != "rallies" {
            continue;
        }
        if let Some(touch) = rally["source_touch_id"].as_str() {
            if let Some((_, _, touch_record)) = all
                .iter()
                .find(|(kind, id, _)| kind == "touches" && id == touch)
            {
                if touch_record["source_replay_id"] != rally["source_replay_id"]
                    || touch_record["video"] != rally["video"]
                    || touch_record["project_id"] != rally["project_id"]
                {
                    return Err(storage::conflict("Связи постобработки повреждены"));
                }
            }
        }
        if let Some(replay) = rally["source_replay_id"].as_str() {
            if let Some((_, _, record)) = all
                .iter()
                .find(|(kind, id, _)| kind == "tracking" && id == replay)
            {
                if record["video"] != rally["video"] || record["project_id"] != rally["project_id"]
                {
                    return Err(storage::conflict("Связи постобработки повреждены"));
                }
            }
        }
    }
    if req.kind == "fusion" {
        selected.extend(descendants(&req.id, &refs));
    }
    if req.kind == "unlinked" {
        selected.extend(
            refs.iter()
                .filter(|(stage, _, v)| {
                    (*stage == "touches" || *stage == "rallies") && v["source_replay_id"] == req.id
                })
                .map(|(stage, id, _)| (*stage, *id)),
        );
    }
    if req.kind == "rallies" {
        if let Some(touch) = target.2["source_touch_id"].as_str() {
            let shared = all.iter().any(|(stage, id, row)| {
                stage == "rallies" && id != &req.id && row["source_touch_id"] == touch
            });
            if !shared
                && refs
                    .iter()
                    .any(|(stage, id, _)| *stage == "touches" && *id == touch)
            {
                selected.push(("touches", touch));
            }
        }
        if let Some(replay) = target.2["source_replay_id"].as_str() {
            let shared = all.iter().any(|(stage, id, row)| {
                ((stage == "rallies" && id != &req.id)
                    || (stage == "touches" && !selected.contains(&("touches", id.as_str()))))
                    && row["source_replay_id"] == replay
            });
            if !shared
                && refs
                    .iter()
                    .any(|(stage, id, _)| *stage == "tracking" && *id == replay)
            {
                selected.push(("tracking", replay));
            }
        }
    }
    if req.kind == "fusion" {
        for id in [
            target.2["sources"]["ravel"]["job_id"].as_str(),
            target.2["sources"]["vballnet"]["job_id"].as_str(),
        ]
        .into_iter()
        .flatten()
        .filter(|id| opaque_id(id))
        {
            let shared = all.iter().any(|(kind, other, v)| {
                kind == "fusion"
                    && other != &req.id
                    && (v["sources"]["ravel"]["job_id"] == id
                        || v["sources"]["vballnet"]["job_id"] == id)
            });
            if !shared
                && all.iter().any(|(kind, other, v)| {
                    kind == "gpu"
                        && other == id
                        && v["project_id"] == req.project_id
                        && v["video"] == req.video
                })
            {
                selected.push(("gpu", id));
            }
        }
    }
    selected.sort_unstable();
    selected.dedup();
    for (stage, _, row) in &all {
        if row["project_id"] == req.project_id && row["video"] == req.video {
            continue;
        }
        let referenced = match stage.as_str() {
            "fusion" => ["ravel", "vballnet"].iter().any(|key| {
                row["sources"][key]["job_id"]
                    .as_str()
                    .is_some_and(|id| selected.contains(&("gpu", id)))
            }),
            "tracking" => row["source_fusion_id"]
                .as_str()
                .is_some_and(|id| selected.contains(&("fusion", id))),
            "touches" => row["source_replay_id"]
                .as_str()
                .is_some_and(|id| selected.contains(&("tracking", id))),
            "rallies" => {
                row["source_replay_id"]
                    .as_str()
                    .is_some_and(|id| selected.contains(&("tracking", id)))
                    || row["source_touch_id"]
                        .as_str()
                        .is_some_and(|id| selected.contains(&("touches", id)))
            }
            _ => false,
        };
        if referenced {
            return Err(storage::conflict("Результат связан с другим видео"));
        }
    }
    let mut paths: Vec<(String, String)> = Vec::new();
    for (stage, id) in &selected {
        let folder = STAGES.iter().find(|(s, _)| s == stage).unwrap().1;
        safe_folder(&root.join(folder).join(id)).map_err(storage::bad_request)?;
        paths.push((folder.to_owned(), (*id).to_owned()));
        let sidecars: &[&str] = match *stage {
            "rallies" => &["native_rally_marks", "native_action_reviews"],
            "tracking" => &["native_track_assignments", "native_reid_reviews"],
            _ => &[],
        };
        for sidecar in sidecars {
            let parent = root.join(sidecar);
            if !parent.exists() {
                continue;
            }
            safe_folder(&parent).map_err(storage::bad_request)?;
            let name = format!("{id}.json");
            let path = parent.join(&name);
            if fs::symlink_metadata(&path).is_ok() {
                storage::read_json(&path)?;
                paths.push(((*sidecar).to_owned(), name));
            }
        }
    }
    if req.kind == "fusion" {
        let parent = root.join("native_gpu_pose_runs");
        if parent.exists() {
            safe_folder(&parent).map_err(storage::bad_request)?;
            for entry in fs::read_dir(&parent)
                .map_err(|e| storage::internal(format!("List pose runs: {e}")))?
            {
                let entry = entry.map_err(|e| storage::internal(format!("Read pose run: {e}")))?;
                let id = entry.file_name().to_string_lossy().to_string();
                if !opaque_id(&id) {
                    continue;
                }
                safe_folder(&entry.path()).map_err(storage::bad_request)?;
                let Some(row) = storage::read_json(&entry.path().join("manifest.json"))? else {
                    continue;
                };
                if row["schema"] != "volley-native-gpu-pose/1" || row["pose_run_id"] != id {
                    return Err(storage::conflict("Повреждены сведения о позах"));
                }
                if row["fusion_id"] == req.id
                    && row["project_id"] == req.project_id
                    && row["video"] == req.video
                {
                    paths.push(("native_gpu_pose_runs".into(), id));
                }
            }
        }
    }
    let txn = root.join("native_analysis_deletion");
    fs::create_dir(&txn).map_err(|e| storage::internal(format!("Create deletion journal: {e}")))?;
    if let Err(error) =
        storage::write_json_atomic(&txn.join("journal.json"), &json!({"paths":paths}))
    {
        let _ = fs::remove_dir(&txn);
        return Err(error);
    }
    for (index, (folder, name)) in paths.iter().enumerate() {
        if let Err(error) = fs::rename(root.join(folder).join(name), txn.join(index.to_string())) {
            recover(root)?;
            return Err(storage::internal(format!(
                "Move analysis for deletion: {error}"
            )));
        }
    }
    if let Err(error) = fs::rename(
        txn.join("journal.json"),
        root.join("native_analysis_deletion_committed.json"),
    ) {
        recover(root)?;
        return Err(storage::internal(format!(
            "Commit analysis deletion: {error}"
        )));
    }
    if fs::remove_dir_all(txn).is_ok() {
        let _ = fs::remove_file(root.join("native_analysis_deletion_committed.json"));
    }
    Ok(())
}

pub async fn change(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<EditRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    storage::video_name(&req.video)?;
    projects::existing_project(&state, &req.project_id)?;
    let _lock = project_guard::write_guard(&state, &req.project_id)?;
    let _catalog_lock = catalog_guard(&state, &req.project_id)?;
    if crate::studio_pipeline::has_active().await
        || state.full_inference.has_active().await
        || state.quick_reel.has_active().await
        || state.montage.has_active().await
        || state.preparation.has_active().await
    {
        return Err(storage::conflict("Дождитесь завершения активного анализа"));
    }
    let root = projects::project_root(&state).join(&req.project_id);
    safe_folder(&root).map_err(storage::bad_request)?;
    edit(&root, &req)?;
    Ok(Json(json!({"ok":true})))
}

pub async fn rename(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<EditRequest>,
) -> ApiResult<Value> {
    if req.action != "rename" {
        return Err(storage::bad_request("Доступно только переименование"));
    }
    change(State(state), headers, Json(req)).await
}

pub async fn capabilities() -> Json<Value> {
    Json(json!({"analysis_edit":true}))
}

fn descendants<'a>(
    fusion: &str,
    entries: &'a [(&'a str, &'a str, Value)],
) -> Vec<(&'a str, &'a str)> {
    let replay: HashSet<&str> = entries
        .iter()
        .filter(|(stage, _, record)| *stage == "tracking" && record["source_fusion_id"] == fusion)
        .map(|(_, id, _)| *id)
        .collect();
    let touches: HashSet<&str> = entries
        .iter()
        .filter(|(stage, _, record)| {
            *stage == "touches"
                && record["source_replay_id"]
                    .as_str()
                    .is_some_and(|id| replay.contains(id))
        })
        .map(|(_, id, _)| *id)
        .collect();
    entries
        .iter()
        .filter(|(stage, id, record)| match *stage {
            "tracking" => replay.contains(id),
            "touches" => touches.contains(id),
            "rallies" => {
                record["source_replay_id"]
                    .as_str()
                    .is_some_and(|id| replay.contains(id))
                    || record["source_touch_id"]
                        .as_str()
                        .is_some_and(|id| touches.contains(id))
            }
            _ => false,
        })
        .map(|(stage, id, _)| (*stage, *id))
        .collect()
}
