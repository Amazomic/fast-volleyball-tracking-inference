//! Versioned manual ReID candidate merge/split, distinct from player assignment.
//! Keeps the existing Python-compatible, per-inference group sidecar schema.
//! All edits require an isolated writable Rust project root. No GPU changes.

use crate::{
    analysis_runs, projects, rallies, reid_native,
    storage::{self, ApiError, ApiResult},
    versioned_store::{self, revision},
    AppState,
};
use axum::{extract::State, http::HeaderMap, Json};
use fs2::FileExt;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use volley_core::reid::{self, ManualGroupMember};

const MAX_GROUP_BYTES: u64 = 16 * 1024 * 1024;
const MAX_IDS: usize = 200;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupEditRequest {
    pub project_id: String,
    pub run_id: String,
    pub video: String,
    /// read, merge or split.
    #[serde(default = "read_action")]
    pub action: String,
    #[serde(default)]
    pub track_ids: Vec<String>,
    #[serde(default)]
    pub expected_revision: Option<String>,
    #[serde(default = "default_threshold")]
    pub threshold_percent: u8,
}
fn read_action() -> String {
    "read".to_owned()
}
fn default_threshold() -> u8 {
    85
}

struct Context {
    source: PathBuf,
    group_file: PathBuf,
    identities: PathBuf,
    project_root: PathBuf,
    video: String,
    project_id: String,
    run_id: String,
    base_cache: String,
    known: HashMap<String, (i64, i64)>,
    source_modified: SystemTime,
    source_size: u64,
    source_stamp: Value,
}

fn id_num(value: &str) -> Option<u64> {
    (!value.is_empty() && value.len() <= 12 && value.bytes().all(|b| b.is_ascii_digit()))
        .then(|| value.parse::<u64>().ok())
        .flatten()
}
fn canonical_id(value: &Value) -> Option<String> {
    match value {
        Value::String(raw) => id_num(raw).map(|n| n.to_string()),
        Value::Number(n) => n
            .as_u64()
            .filter(|n| n.to_string().len() <= 12)
            .map(|n| n.to_string()),
        _ => None,
    }
}
fn unique_ids(values: &[String]) -> Result<Vec<String>, ApiError> {
    if values.is_empty() || values.len() > MAX_IDS {
        return Err(storage::bad_request("Выберите от 1 до 200 Track ID"));
    }
    let mut result = BTreeSet::new();
    for id in values {
        let id =
            id_num(id).ok_or_else(|| storage::bad_request("Недопустимый числовой Track ID"))?;
        if !result.insert(id) {
            return Err(storage::bad_request("Повторяющийся Track ID"));
        }
    }
    Ok(result.into_iter().map(|id| id.to_string()).collect())
}

fn read_raw(path: &Path) -> Result<Option<Vec<u8>>, ApiError> {
    versioned_store::read_optional_bounded(path, MAX_GROUP_BYTES).map_err(storage::bad_request)
}

fn source_info(path: &Path) -> Result<(SystemTime, u64, Value), ApiError> {
    let stat = fs::metadata(path)
        .map_err(|_| storage::bad_request("Сохранённый GPU players.json недоступен"))?;
    let modified = stat
        .modified()
        .map_err(|_| storage::bad_request("У players.json нет даты изменения"))?;
    let mtime = modified
        .duration_since(UNIX_EPOCH)
        .map_err(|_| storage::bad_request("Некорректная дата изменения GPU-кэша"))?
        .as_nanos();
    let stamp = json!({"mtime_ns":mtime,"size":stat.len()});
    Ok((modified, stat.len(), stamp))
}

fn context(state: &AppState, req: &GroupEditRequest) -> Result<Context, ApiError> {
    storage::project_id(&req.project_id)?;
    storage::video_name(&req.video)?;
    if !(55..=99).contains(&req.threshold_percent) {
        return Err(storage::bad_request("Порог ReID должен быть от 55 до 99"));
    }
    let project = projects::existing_project(state, &req.project_id)?;
    if !project["videos"].as_array().is_some_and(|videos| {
        videos
            .iter()
            .any(|video| video.as_str() == Some(&req.video))
    }) {
        return Err(storage::bad_request(
            "Видео отсутствует в выбранном проекте",
        ));
    }
    let run = analysis_runs::list_runs(state, &req.project_id)?
        .into_iter()
        .find(|v| v["id"].as_str() == Some(&req.run_id))
        .ok_or_else(|| storage::bad_request("Указанный запуск анализа отсутствует"))?;
    let cache_id = run["videos"][&req.video]
        .as_str()
        .ok_or_else(|| storage::bad_request("В запуске нет выбранного видео"))?;
    let selected = rallies::cache_root(state, &req.project_id, &req.video, cache_id)?;
    let base_cache = cache_id.split('.').next().unwrap_or(cache_id).to_owned();
    let base = if cache_id.contains('.') {
        selected
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| storage::bad_request("Некорректный CPU-кэш"))?
            .to_path_buf()
    } else {
        selected
    };
    let source = base.join("players.json");
    let known = reid_native::track_statistics(&source)?
        .into_iter()
        .map(|row| (row.id.to_string(), (row.first_frame, row.last_frame)))
        .collect::<HashMap<_, _>>();
    if known.len() > 20_000 {
        return Err(storage::bad_request("Слишком много Track ID"));
    }
    let (source_modified, source_size, source_stamp) = source_info(&source)?;
    let project_root = projects::project_root(state).join(&req.project_id);
    let hash = reid_native::path_key(&req.video, &base_cache);
    Ok(Context {
        source,
        group_file: project_root
            .join("player_identity_groups")
            .join(format!("{hash}.json")),
        identities: project_root.join("player_identities.json"),
        project_root,
        video: req.video.clone(),
        project_id: req.project_id.clone(),
        run_id: req.run_id.clone(),
        base_cache,
        known,
        source_modified,
        source_size,
        source_stamp,
    })
}

fn valid_saved(doc: &Value, ctx: &Context) -> Result<(), ApiError> {
    if doc["schema"].as_u64() != Some(1)
        || doc["stamp"] != ctx.source_stamp
        || doc["groups"].as_array().is_none()
        || doc["unmatched_ids"].as_array().is_none()
    {
        return Err(storage::conflict(
            "ReID-группы относятся к другой версии GPU-результатов; обновите источник анализа",
        ));
    }
    if doc["groups"]
        .as_array()
        .is_some_and(|rows| rows.len() > 20_000)
    {
        return Err(storage::bad_request("Too many ReID groups"));
    }
    Ok(())
}

fn read_saved(raw: Option<&[u8]>, ctx: &Context) -> Result<Option<Value>, ApiError> {
    let Some(bytes) = raw else {
        return Ok(None);
    };
    let doc: Value =
        serde_json::from_slice(bytes).map_err(|_| storage::bad_request("Повреждён ReID JSON"))?;
    valid_saved(&doc, ctx)?;
    Ok(Some(doc))
}

/// Computed baseline is never written by a read request. It reuses existing
/// cached vectors and the conservative Rust native matching kernel.
fn fresh_baseline(
    state: &AppState,
    req: &GroupEditRequest,
    ctx: &Context,
) -> Result<Value, ApiError> {
    let report = reid_native::calculate(
        state,
        reid_native::ReidRequest {
            project_id: req.project_id.clone(),
            run_id: req.run_id.clone(),
            video: req.video.clone(),
            threshold_percent: req.threshold_percent,
        },
    )?;
    let groups = report["embedding"]["groups"]
        .as_array()
        .ok_or_else(|| storage::bad_request("В анализе нет рассчитанных ReID-групп"))?;
    let unmatched = report["embedding"]["unmatched_ids"]
        .as_array()
        .ok_or_else(|| storage::bad_request("В анализе нет данных о непривязанных ID"))?;
    let mut doc = json!({
        "schema":1,
        "similarity_schema":6,
        "stamp":ctx.source_stamp,
        "method":report["method"],
        "threshold_percent":req.threshold_percent,
        "manual_excluded_ids":[],
        "groups":groups,
        "unmatched_ids":unmatched,
        "embedded_count":report["embedding"]["embedded_count"],
        "total_tracks":report["embedding"]["total_tracks"],
        "generated_by":"rust-native-reid-groups",
        "auto_assign_enabled":false,
    });
    // The GPU source must still be identical after the full native ReID calculation.
    verify_source(ctx)?;
    if doc["groups"].is_null() {
        doc["groups"] = json!([]);
    }
    Ok(doc)
}

fn verify_source(ctx: &Context) -> Result<(), ApiError> {
    let (modified, size, _) = source_info(&ctx.source)?;
    if size != ctx.source_size || modified != ctx.source_modified {
        return Err(storage::conflict(
            "GPU обнаружения изменились — обновите ReID-анализ",
        ));
    }
    Ok(())
}

fn id_set_from_group(group: &Value) -> Result<Vec<String>, ApiError> {
    let rows = group["track_ids"]
        .as_array()
        .ok_or_else(|| storage::bad_request("У группы отсутствует список Track ID"))?;
    let mut members = BTreeSet::new();
    for entry in rows {
        let id = canonical_id(entry)
            .ok_or_else(|| storage::bad_request("В ReID-группе недопустимый Track ID"))?;
        if !members.insert(id) {
            return Err(storage::bad_request("Повторный ID в ReID-группе"));
        }
    }
    Ok(members.into_iter().collect())
}

/// Reviewed track→player mapping, conflicting track ids and excluded ids.
type OwnerMaps = (HashMap<String, String>, HashSet<String>, HashSet<String>);

fn owners(ctx: &Context) -> Result<OwnerMaps, ApiError> {
    let mut mapping = HashMap::new();
    let mut conflict = HashSet::new();
    let mut excluded = HashSet::new();
    let Some(bytes) = read_raw(&ctx.identities)? else {
        return Ok((mapping, conflict, excluded));
    };
    let raw: Value = serde_json::from_slice(&bytes)
        .map_err(|_| storage::bad_request("Повреждён реестр ручных привязок"))?;
    let key = format!("{}::{}", ctx.video, ctx.base_cache);
    if let Some(ids) = raw["excluded_ids"][&key].as_array() {
        for id in ids.iter().filter_map(canonical_id) {
            excluded.insert(id);
        }
    }
    if let Some(groups) = raw["videos"][&key].as_array() {
        for group in groups {
            let Some(pid) = group["id"].as_str() else {
                continue;
            };
            for id in group["track_ids"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(canonical_id)
            {
                let reviewed = group["link_meta"][&id]["reviewed"]
                    .as_bool()
                    .unwrap_or(true);
                if !reviewed {
                    continue;
                }
                if mapping
                    .insert(id.clone(), pid.to_owned())
                    .is_some_and(|previous| previous != pid)
                {
                    conflict.insert(id);
                }
            }
        }
    }
    for tid in &conflict {
        mapping.remove(tid);
    }
    Ok((mapping, conflict, excluded))
}

fn sort_ids(ids: &mut [String]) {
    ids.sort_by_key(|id| id.parse::<u64>().unwrap_or(u64::MAX));
}
fn normalized_groups(doc: &Value, ctx: &Context) -> Result<HashSet<String>, ApiError> {
    let groups = doc["groups"].as_array().expect("validated group document");
    let mut seen = HashSet::new();
    for group in groups {
        let members = id_set_from_group(group)?;
        if members.len() < 2 {
            return Err(storage::bad_request(
                "Группа должна содержать минимум два Track ID",
            ));
        }
        for tid in members {
            if !ctx.known.contains_key(&tid) || !seen.insert(tid) {
                return Err(storage::conflict("В группах повторные/чужие Track ID"));
            }
        }
    }
    Ok(seen)
}

fn edit_groups(
    doc: &mut Value,
    action: &str,
    ids: &[String],
    ctx: &Context,
    mapping: &HashMap<String, String>,
    conflicts: &HashSet<String>,
    excluded: &HashSet<String>,
) -> Result<(), ApiError> {
    let selection = ids.iter().cloned().collect::<HashSet<_>>();
    for id in ids {
        if !ctx.known.contains_key(id) || conflicts.contains(id) || excluded.contains(id) {
            return Err(storage::conflict(
                "Track ID не принадлежит анализу, исключён или имеет противоречивые ручные привязки",
            ));
        }
    }
    normalized_groups(doc, ctx)?;
    let old_groups = doc["groups"].as_array().expect("validated saved groups");
    let old_members = old_groups
        .iter()
        .map(id_set_from_group)
        .collect::<Result<Vec<_>, _>>()?;
    if action == "split"
        && !old_members
            .iter()
            .any(|members| members.iter().any(|id| selection.contains(id)))
    {
        return Err(storage::bad_request(
            "Выбранные ID уже находятся вне ReID-групп",
        ));
    }
    if action == "merge" {
        if ids.len() < 2 {
            return Err(storage::bad_request("Для объединения нужно минимум два ID"));
        }
        let members = ids
            .iter()
            .map(|id| {
                let (first_frame, last_frame) = ctx.known[id];
                Ok(ManualGroupMember {
                    track_id: id
                        .parse::<i64>()
                        .map_err(|_| storage::bad_request("Invalid Track ID"))?,
                    first_frame,
                    last_frame,
                    confirmed_owner: mapping.get(id).map(String::as_str),
                })
            })
            .collect::<Result<Vec<_>, ApiError>>()?;
        if !reid::valid_manual_group(&members) {
            return Err(storage::conflict(
                "Cannot combine co-visible Track IDs or distinct confirmed players",
            ));
        }
    }
    let mut existing_excluded = doc["manual_excluded_ids"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(canonical_id)
        .collect::<HashSet<_>>();
    let mut groups = Vec::new();
    for (row, members) in old_groups.iter().zip(old_members) {
        let mut untouched = members
            .into_iter()
            .filter(|id| !selection.contains(id))
            .collect::<Vec<_>>();
        sort_ids(&mut untouched);
        if untouched.len() >= 2 {
            let mut updated = row.clone();
            updated["track_ids"] = json!(untouched);
            // Any manually edited group is pinned through subsequent
            // resimilarity calculations; no automatic import of other IDs.
            updated["source"] = json!("manual");
            updated["similarity"] = Value::Null;
            groups.push(updated);
        }
    }

    match action {
        "split" => {
            existing_excluded.extend(ids.iter().cloned());
        }
        "merge" => {
            for id in ids {
                existing_excluded.remove(id);
            }
            let hash = ids.join(",");
            let gid = format!("manual-{:x}", Sha256::digest(hash.as_bytes()));
            groups.push(json!({
                "id":&gid[..19],
                "track_ids":ids,
                "similarity":null,
                "source":"manual"
            }));
        }
        _ => return Err(storage::bad_request("Неизвестное действие с ReID-группами")),
    }
    let mut manual = existing_excluded.into_iter().collect::<Vec<_>>();
    sort_ids(&mut manual);
    let used = groups
        .iter()
        .flat_map(|group| {
            group["track_ids"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(canonical_id)
        })
        .collect::<HashSet<_>>();
    let mut unmatched = ctx
        .known
        .keys()
        .filter(|id| !used.contains(*id) && !excluded.contains(*id))
        .cloned()
        .collect::<Vec<_>>();
    sort_ids(&mut unmatched);
    groups.sort_by(|a, b| {
        let ac = a["track_ids"].as_array().map_or(0, Vec::len);
        let bc = b["track_ids"].as_array().map_or(0, Vec::len);
        bc.cmp(&ac)
            .then_with(|| a["id"].as_str().cmp(&b["id"].as_str()))
    });
    doc["groups"] = json!(groups);
    doc["unmatched_ids"] = json!(unmatched);
    doc["manual_excluded_ids"] = json!(manual);
    doc["last_manual_action"] = json!(action);
    doc["auto_assign_enabled"] = json!(false);
    Ok(())
}

fn response(ctx: &Context, doc: Value, rev: String, writes_enabled: bool) -> Value {
    json!({
        "schema":1,
        "project_id":ctx.project_id,
        "video":ctx.video,
        "run_id":ctx.run_id,
        "cache_id":ctx.base_cache,
        "revision":rev,
        "writable":writes_enabled,
        "confirmed":false,
        "groups":doc["groups"],
        "unmatched_ids":doc["unmatched_ids"],
        "manual_excluded_ids":doc["manual_excluded_ids"],
        "threshold_percent":doc["threshold_percent"],
        "method":doc["method"],
        "status":"ready",
        "warning":"Группы являются кандидатами, не подтверждёнными игроками. Для привязки используйте отдельный endpoint identity/native."
    })
}

fn compute(state: &AppState, req: GroupEditRequest) -> Result<Value, ApiError> {
    if !matches!(req.action.as_str(), "read" | "merge" | "split") {
        return Err(storage::bad_request(
            "Допустимые действия: read, merge, split",
        ));
    }
    if req.action != "read" && !state.writes_enabled {
        return Err(storage::conflict(
            "Изменения ReID-групп разрешены только в рабочем хранилище",
        ));
    }
    let ctx = context(state, &req)?;
    if req.action == "read" {
        let raw = read_raw(&ctx.group_file)?;
        let doc = match read_saved(raw.as_deref(), &ctx)? {
            Some(record) => record,
            None => fresh_baseline(state, &req, &ctx)?,
        };
        return Ok(response(
            &ctx,
            doc,
            revision(raw.as_deref()),
            state.writes_enabled,
        ));
    }
    let ids = unique_ids(&req.track_ids)?;
    let expected = req
        .expected_revision
        .as_deref()
        .ok_or_else(|| storage::bad_request("Сначала откройте группы и передайте revision"))?;
    if expected != "absent"
        && (expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(storage::bad_request("Недопустимая версия ReID-групп"));
    }
    let parent = ctx
        .group_file
        .parent()
        .ok_or_else(|| storage::internal("Missing parent of ReID groups file"))?;
    // The input project must already live in the isolated root. Never
    // materialize a new ReID folder under the production repository.
    let canonical_project = ctx
        .project_root
        .canonicalize()
        .map_err(|_| storage::bad_request("Изолированный проект не существует"))?;
    let canonical_root = projects::project_root(state)
        .canonicalize()
        .map_err(|_| storage::bad_request("Изолированное хранилище проектов недоступно"))?;
    if canonical_project == canonical_root || !canonical_project.starts_with(&canonical_root) {
        return Err(storage::bad_request(
            "Директория проекта выходит за пределы рабочего хранилища",
        ));
    }
    if parent.exists()
        && (fs::symlink_metadata(parent)
            .ok()
            .is_some_and(|m| m.file_type().is_symlink())
            || parent
                .canonicalize()
                .ok()
                .is_none_or(|p| !p.starts_with(&canonical_project)))
    {
        return Err(storage::bad_request("Некорректная директория ReID-групп"));
    }
    fs::create_dir_all(parent)
        .map_err(|e| storage::internal(format!("Cannot create ReID group folder: {e}")))?;
    if !parent
        .canonicalize()
        .ok()
        .is_some_and(|p| p.starts_with(&canonical_project))
    {
        return Err(storage::bad_request(
            "ReID directory escaped project storage",
        ));
    }
    let lock_path = ctx.project_root.join(".player_identities.lock");
    if fs::symlink_metadata(&lock_path)
        .ok()
        .is_some_and(|m| m.file_type().is_symlink())
    {
        return Err(storage::bad_request(
            "Identity lock file must not be a symlink",
        ));
    }
    // The same lock is used by manual player assignment. This prevents
    // changing confirmed owners concurrently with the proposed group edit.
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|e| storage::internal(format!("Cannot open identity lock: {e}")))?;
    lock.lock_exclusive()
        .map_err(|e| storage::internal(format!("Cannot lock player identities: {e}")))?;
    let group_lock_path = parent.join(".reid_groups.lock");
    if fs::symlink_metadata(&group_lock_path)
        .ok()
        .is_some_and(|m| m.file_type().is_symlink())
    {
        return Err(storage::bad_request("ReID group lock cannot be a symlink"));
    }
    let group_lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&group_lock_path)
        .map_err(|e| storage::internal(format!("Cannot open ReID group lock: {e}")))?;
    group_lock
        .lock_exclusive()
        .map_err(|e| storage::internal(format!("Cannot acquire ReID lock: {e}")))?;
    let raw = read_raw(&ctx.group_file)?;
    if revision(raw.as_deref()) != expected {
        return Err(storage::conflict(
            "ReID-группы уже изменены; перечитайте актуальную версию",
        ));
    }
    let mut doc = match read_saved(raw.as_deref(), &ctx)? {
        Some(doc) => doc,
        None => fresh_baseline(state, &req, &ctx)?,
    };
    let (mapping, conflicts, excluded) = owners(&ctx)?;
    edit_groups(
        &mut doc,
        &req.action,
        &ids,
        &ctx,
        &mapping,
        &conflicts,
        &excluded,
    )?;
    verify_source(&ctx)?;
    let parent = ctx
        .group_file
        .parent()
        .ok_or_else(|| storage::internal("ReID group directory missing"))?;
    versioned_store::write_json_checked(parent, &ctx.group_file, &doc, MAX_GROUP_BYTES)
        .map_err(storage::bad_request)?;
    let updated =
        read_raw(&ctx.group_file)?.ok_or_else(|| storage::internal("ReID group write vanished"))?;
    let mut answer = response(&ctx, doc, revision(Some(&updated)), true);
    answer["action"] = json!(req.action);
    answer["changed_track_ids"] = json!(ids);
    Ok(answer)
}

pub async fn group_edits(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<GroupEditRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    // Use same in-process gate as the identity editor (and independent
    // cross-process file locks above), preventing conflicting user actions.
    let _permit = state
        .identity_limit
        .acquire()
        .await
        .map_err(|error| storage::internal(format!("ReID edit concurrency: {error}")))?;
    let reading = state.clone();
    let result = tokio::task::spawn_blocking(move || compute(&reading, req))
        .await
        .map_err(|error| storage::internal(format!("ReID group task failed: {error}")))??;
    Ok(Json(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context_for_unit() -> Context {
        Context {
            source: PathBuf::new(),
            group_file: PathBuf::new(),
            identities: PathBuf::new(),
            project_root: PathBuf::new(),
            video: "match.mp4".into(),
            project_id: "1234abcdef01".into(),
            run_id: "run_example".into(),
            base_cache: "inference".into(),
            source_modified: UNIX_EPOCH,
            source_size: 100,
            source_stamp: json!({"mtime_ns":1,"size":100}),
            known: HashMap::from([
                ("1".into(), (0, 20)),
                ("2".into(), (30, 50)),
                ("3".into(), (60, 80)),
                ("4".into(), (10, 40)),
            ]),
        }
    }
    fn existing() -> Value {
        json!({
            "schema":1,"stamp":{"mtime_ns":1,"size":100},
            "groups":[{"id":"group-1","source":"automatic","track_ids":["1","2","3"],"similarity":0.93}],
            "unmatched_ids":["4"],"manual_excluded_ids":[]
        })
    }
    #[test]
    fn only_selected_members_are_removed_on_split() {
        let ctx = context_for_unit();
        let mut doc = existing();
        edit_groups(
            &mut doc,
            "split",
            &["2".into()],
            &ctx,
            &HashMap::new(),
            &HashSet::new(),
            &HashSet::new(),
        )
        .unwrap();
        assert_eq!(doc["groups"][0]["track_ids"], json!(["1", "3"]));
        assert_eq!(doc["groups"][0]["source"], "manual");
        assert_eq!(doc["manual_excluded_ids"], json!(["2"]));
        assert!(doc["unmatched_ids"]
            .as_array()
            .unwrap()
            .contains(&json!("2")));
    }
    #[test]
    fn cannot_merge_concurrently_visible_ids() {
        let ctx = context_for_unit();
        let mut doc = existing();
        assert!(edit_groups(
            &mut doc,
            "merge",
            &["1".into(), "4".into()],
            &ctx,
            &HashMap::new(),
            &HashSet::new(),
            &HashSet::new()
        )
        .is_err());
    }
    #[test]
    fn distinct_confirmed_players_block_merges() {
        let ctx = context_for_unit();
        let mut doc = existing();
        let owners = HashMap::from([
            ("1".into(), "athlete-a".into()),
            ("2".into(), "athlete-b".into()),
        ]);
        assert!(edit_groups(
            &mut doc,
            "merge",
            &["1".into(), "2".into()],
            &ctx,
            &owners,
            &HashSet::new(),
            &HashSet::new()
        )
        .is_err());
    }
    #[test]
    fn merge_pins_exact_ids_and_preserves_non_selected_members() {
        let ctx = context_for_unit();
        let mut doc = existing();
        edit_groups(
            &mut doc,
            "merge",
            &["1".into(), "3".into()],
            &ctx,
            &HashMap::new(),
            &HashSet::new(),
            &HashSet::new(),
        )
        .unwrap();
        assert!(doc["groups"]
            .as_array()
            .unwrap()
            .iter()
            .any(|group| group["source"] == "manual" && group["track_ids"] == json!(["1", "3"])));
        assert!(doc["unmatched_ids"]
            .as_array()
            .unwrap()
            .contains(&json!("2")));
    }
}
