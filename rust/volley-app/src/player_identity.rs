//! Explicit manual identity edits within the isolated Rust development root.
//! GPU detections, source projects, and global photo albums are immutable.
//! The storage contract matches legacy project-scoped player_identities.json.

use crate::{
    analysis_runs, projects, rallies, reid_native,
    storage::{self, ApiError, ApiResult},
    versioned_store::{self, revision as ident_revision},
    AppState,
};
use axum::{extract::State, http::HeaderMap, Json};
use fs2::FileExt;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};
use volley_core::evidence_source::{EvidenceSource, FrameRate, SourceStamp, VerifiedTrackSet};

const MAX_IDENTITIES_BYTES: u64 = 16 * 1024 * 1024;
const MAX_IDS_PER_ACTION: usize = 200;
const MAX_KNOWN_TRACKS: usize = 20_000;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityRequest {
    pub project_id: String,
    pub run_id: String,
    pub video: String,
    /// One of: read, assign, unassign, exclude, restore.
    #[serde(default = "read_action")]
    pub action: String,
    #[serde(default)]
    pub track_ids: Vec<String>,
    #[serde(default)]
    pub group_id: Option<String>,
    /// Revision of the saved ReID group catalogue at the time of clicking it.
    #[serde(default)]
    pub expected_group_revision: Option<String>,
    #[serde(default)]
    pub player_id: Option<String>,
    /// Must equal the revision returned by a preceding read.
    #[serde(default)]
    pub expected_revision: Option<String>,
    /// Explicit override required if an ID is already assigned to somebody else.
    #[serde(default)]
    pub allow_reassign: bool,
}
fn read_action() -> String {
    "read".into()
}

#[derive(Debug)]
struct Context {
    evidence: EvidenceSource,
    player_file: PathBuf,
    identity_path: PathBuf,
    group_path: PathBuf,
    project_id: String,
    video: String,
    base_cache_id: String,
    storage_key: String,
    known: VerifiedTrackSet,
    roster: BTreeMap<String, String>,
}

fn numeric_id(value: &str) -> Option<i64> {
    if value.is_empty() || value.len() > 12 || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse::<i64>().ok().filter(|n| *n >= 0)
}
fn value_id(value: &Value) -> Option<String> {
    let id = match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.as_i64()?.to_string(),
        _ => return None,
    };
    numeric_id(&id).map(|n| n.to_string())
}
fn raw_file(path: &Path) -> Result<Option<Vec<u8>>, ApiError> {
    versioned_store::read_optional_bounded(path, MAX_IDENTITIES_BYTES).map_err(storage::bad_request)
}
fn parse_document(raw: Option<&[u8]>) -> Result<Value, ApiError> {
    let Some(bytes) = raw else {
        return Ok(json!({"schema":2,"videos":{},"excluded_ids":{},"auto_rejected":{}}));
    };
    let document: Value = serde_json::from_slice(bytes)
        .map_err(|err| storage::bad_request(format!("Corrupt player identities: {err}")))?;
    if !document.is_object() || document["videos"].as_object().is_none() {
        return Err(storage::bad_request(
            "Invalid player identities JSON schema",
        ));
    }
    if (document["excluded_ids"].is_object() || document["excluded_ids"].is_null())
        && (document["auto_rejected"].is_object() || document["auto_rejected"].is_null())
    {
        Ok(document)
    } else {
        Err(storage::bad_request(
            "Invalid excluded/rejected track registry",
        ))
    }
}
/// GPU detections are immutable for one inference. Cache only their numeric
/// Track IDs (not the full detections) by canonical source path/mtime/size.
/// This keeps UI actions fast and invalidates on file modification.
#[derive(Clone)]
struct KnownSnapshot {
    source: PathBuf,
    modified: SystemTime,
    size: u64,
    tracks: BTreeSet<String>,
}
static KNOWN_SNAPSHOTS: OnceLock<Mutex<Vec<KnownSnapshot>>> = OnceLock::new();

fn known_tracks(source: &Path) -> Result<BTreeSet<String>, ApiError> {
    let path = source
        .canonicalize()
        .map_err(|_| storage::bad_request("Missing saved inference player detections"))?;
    let before = fs::metadata(&path)
        .map_err(|_| storage::bad_request("Cannot read player detections metadata"))?;
    let modified = before
        .modified()
        .map_err(|_| storage::bad_request("Cannot timestamp player detections"))?;
    let snapshots = KNOWN_SNAPSHOTS.get_or_init(|| Mutex::new(Vec::new()));
    {
        let ready = snapshots
            .lock()
            .map_err(|_| storage::internal("Track ID index lock poisoned"))?;
        if let Some(hit) = ready
            .iter()
            .find(|s| s.source == path && s.modified == modified && s.size == before.len())
        {
            return Ok(hit.tracks.clone());
        }
    }
    let ids = reid_native::track_statistics(&path)?
        .into_iter()
        .map(|row| row.id.to_string())
        .collect::<BTreeSet<_>>();
    let after = fs::metadata(&path)
        .map_err(|_| storage::conflict("Player detections disappeared during indexing"))?;
    if after.len() != before.len() || after.modified().ok() != Some(modified) {
        return Err(storage::conflict(
            "Player detections changed during indexing",
        ));
    }
    if ids.len() > MAX_KNOWN_TRACKS {
        return Err(storage::bad_request("Too many Track IDs"));
    }
    let mut ready = snapshots
        .lock()
        .map_err(|_| storage::internal("Track ID index lock poisoned"))?;
    ready.retain(|s| s.source != path);
    ready.insert(
        0,
        KnownSnapshot {
            source: path,
            modified,
            size: before.len(),
            tracks: ids.clone(),
        },
    );
    ready.truncate(8);
    Ok(ids)
}

fn validate_context(state: &AppState, req: &IdentityRequest) -> Result<Context, ApiError> {
    storage::project_id(&req.project_id)?;
    storage::video_name(&req.video)?;
    let project = projects::existing_project(state, &req.project_id)?;
    let project_videos = project["videos"]
        .as_array()
        .ok_or_else(|| storage::bad_request("Invalid project video list"))?;
    if !project_videos
        .iter()
        .any(|row| row.as_str() == Some(&req.video))
    {
        return Err(storage::bad_request("Video is not part of this project"));
    }
    let run = analysis_runs::list_runs(state, &req.project_id)?
        .into_iter()
        .find(|row| row["id"].as_str() == Some(&req.run_id))
        .ok_or_else(|| storage::bad_request("Selected CPU/GPU run is not available"))?;
    let cache_id = run["videos"][&req.video]
        .as_str()
        .ok_or_else(|| storage::bad_request("Selected run contains no such video"))?;
    let selected = rallies::cache_root(state, &req.project_id, &req.video, cache_id)?;
    let base_cache_id = cache_id.split('.').next().unwrap_or(cache_id).to_owned();
    let base = if cache_id.contains('.') {
        selected
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| storage::bad_request("Invalid CPU cache path"))?
            .to_path_buf()
    } else {
        selected
    };
    let player_file = base.join("players.json");
    let known_ids = known_tracks(&player_file)?;
    let (bytes, mtime_ns) =
        crate::inference_stream::source_stamp(&player_file).map_err(storage::bad_request)?;
    let evidence = EvidenceSource::legacy(
        &req.project_id,
        &req.video,
        &req.run_id,
        &base_cache_id,
        SourceStamp {
            bytes,
            mtime_ns: u64::try_from(mtime_ns)
                .map_err(|_| storage::bad_request("Legacy timestamp outside supported range"))?,
        },
        FrameRate::Unknown,
    )
    .map_err(storage::bad_request)?;
    let numeric_ids = known_ids
        .iter()
        .map(|id| {
            id.parse::<u64>()
                .map_err(|_| storage::bad_request("Legacy tracking ID is not an integer"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let known =
        VerifiedTrackSet::new(evidence.clone(), numeric_ids).map_err(storage::bad_request)?;
    if known.len() > MAX_KNOWN_TRACKS {
        return Err(storage::bad_request("Too many Track IDs"));
    }
    let players = state.data_root.join("players").join("registry.json");
    let registry = storage::read_json(&players)?
        .ok_or_else(|| storage::bad_request("Global player catalog missing in Rust data root"))?;
    let project_roster = project["player_ids"]
        .as_array()
        .ok_or_else(|| storage::bad_request("Project roster is not configured"))?
        .iter()
        .filter_map(Value::as_str)
        .collect::<HashSet<_>>();
    let mut roster = BTreeMap::new();
    for athlete in registry["players"].as_array().into_iter().flatten() {
        if let (Some(id), Some(name)) = (athlete["id"].as_str(), athlete["name"].as_str()) {
            if project_roster.contains(id) {
                roster.insert(id.to_owned(), name.to_owned());
            }
        }
    }
    let root = projects::project_root(state).join(&req.project_id);
    let identity_path = root.join("player_identities.json");
    let raw = format!("{}\0{}", req.video, base_cache_id);
    let key = format!("{:x}", Sha256::digest(raw.as_bytes()))[..24].to_owned();
    Ok(Context {
        evidence,
        player_file,
        identity_path,
        group_path: root
            .join("player_identity_groups")
            .join(format!("{key}.json")),
        project_id: req.project_id.clone(),
        video: req.video.clone(),
        storage_key: format!("{}::{}", req.video, base_cache_id),
        base_cache_id,
        known,
        roster,
    })
}

fn select_ids(req: &IdentityRequest, ctx: &Context) -> Result<Vec<String>, ApiError> {
    let mut ids = Vec::new();
    if let Some(group_id) = &req.group_id {
        if !req.track_ids.is_empty() {
            return Err(storage::bad_request("Specify either group_id or track_ids"));
        }
        if group_id.is_empty()
            || group_id.len() > 128
            || !group_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(storage::bad_request("Invalid ReID group ID"));
        }
        let source = raw_file(&ctx.group_path)?.ok_or_else(|| {
            storage::bad_request("No saved ReID groups for this selected inference")
        })?;
        let expected = req.expected_group_revision.as_deref().ok_or_else(|| {
            storage::bad_request("Для привязки группы требуется её актуальная revision")
        })?;
        if ident_revision(Some(&source)) != expected {
            return Err(storage::conflict(
                "ReID-группа изменилась — откройте её повторно",
            ));
        }
        let record: Value = serde_json::from_slice(&source)
            .map_err(|_| storage::bad_request("Invalid saved ReID group JSON"))?;
        let meta = fs::metadata(&ctx.player_file)
            .map_err(|_| storage::bad_request("Player source file missing"))?;
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|t| t.as_nanos().to_string())
            .ok_or_else(|| storage::bad_request("No source timestamp"))?;
        if record["schema"].as_u64() != Some(1)
            || record["stamp"]["mtime_ns"].as_u64().map(|v| v.to_string()) != Some(modified)
            || record["stamp"]["size"].as_u64() != Some(meta.len())
        {
            return Err(storage::bad_request(
                "Saved ReID groups are stale; select individual Track IDs",
            ));
        }
        let groups = record["groups"]
            .as_array()
            .ok_or_else(|| storage::bad_request("Invalid saved ReID groups"))?;
        let Some(group) = groups
            .iter()
            .find(|g| g["id"].as_str() == Some(group_id.as_str()))
        else {
            return Err(storage::bad_request("Saved ReID group not found"));
        };
        for item in group["track_ids"].as_array().into_iter().flatten() {
            ids.push(
                value_id(item)
                    .ok_or_else(|| storage::bad_request("Invalid Track ID in saved group"))?,
            );
        }
    } else {
        ids = req.track_ids.clone();
    }
    if ids.is_empty() || ids.len() > MAX_IDS_PER_ACTION {
        return Err(storage::bad_request("Select 1–200 Track IDs"));
    }
    let mut unique = BTreeSet::new();
    for id in &mut ids {
        let normalized = numeric_id(id)
            .ok_or_else(|| storage::bad_request("Invalid numeric Track ID"))?
            .to_string();
        if !ctx.known.contains_number(
            normalized
                .parse::<u64>()
                .map_err(|_| storage::bad_request("Invalid normalized Track ID"))?,
        ) {
            return Err(storage::bad_request(format!(
                "Track ID {normalized} does not belong to selected inference"
            )));
        }
        if !unique.insert(normalized.clone()) {
            return Err(storage::bad_request("Duplicate Track ID in request"));
        }
        *id = normalized;
    }
    ids.sort_by_key(|id| id.parse::<u64>().unwrap_or(u64::MAX));
    Ok(ids)
}

fn set_of(raw: &Value, field: &str, key: &str) -> Result<BTreeSet<String>, ApiError> {
    let mut set = BTreeSet::new();
    match raw[field][key].as_array() {
        Some(rows) => {
            for item in rows {
                if let Some(id) = value_id(item) {
                    set.insert(id);
                }
            }
        }
        None if raw[field][key].is_null() => {}
        None => {
            return Err(storage::bad_request(format!(
                "Invalid identity field {field}"
            )))
        }
    }
    Ok(set)
}
fn put_set(
    raw: &mut Value,
    field: &str,
    key: &str,
    values: &BTreeSet<String>,
) -> Result<(), ApiError> {
    let dict = raw
        .as_object_mut()
        .expect("validated identity JSON")
        .entry(field)
        .or_insert_with(|| json!({}));
    if dict.is_null() {
        *dict = json!({});
    }
    let dict = dict
        .as_object_mut()
        .ok_or_else(|| storage::bad_request("Identity registry is not a map"))?;
    let mut list = values.iter().cloned().collect::<Vec<_>>();
    list.sort_by_key(|id| id.parse::<u64>().unwrap_or(u64::MAX));
    dict.insert(key.to_owned(), json!(list));
    Ok(())
}

fn groups_mut<'a>(raw: &'a mut Value, key: &str) -> Result<&'a mut Vec<Value>, ApiError> {
    let videos = raw["videos"]
        .as_object_mut()
        .ok_or_else(|| storage::bad_request("Invalid identity video registry"))?;
    let groups = videos.entry(key).or_insert_with(|| json!([]));
    groups
        .as_array_mut()
        .ok_or_else(|| storage::bad_request("Invalid identity assignments"))
}
fn remove_ids(groups: &mut Vec<Value>, ids: &BTreeSet<String>) {
    for group in groups.iter_mut() {
        if let Some(values) = group["track_ids"].as_array_mut() {
            values.retain(|value| value_id(value).is_none_or(|id| !ids.contains(&id)));
        }
        if let Some(meta) = group["link_meta"].as_object_mut() {
            for id in ids {
                meta.remove(id);
            }
        }
    }
    groups.retain(|g| g["track_ids"].as_array().is_some_and(|arr| !arr.is_empty()));
}

fn assigned_to(groups: &[Value], id: &str) -> HashSet<String> {
    groups
        .iter()
        .filter_map(|group| {
            let has = group["track_ids"]
                .as_array()
                .is_some_and(|arr| arr.iter().any(|item| value_id(item).as_deref() == Some(id)));
            has.then(|| group["id"].as_str().map(str::to_owned))
                .flatten()
        })
        .collect()
}

fn apply(
    raw: &mut Value,
    req: &IdentityRequest,
    ctx: &Context,
    ids: &[String],
) -> Result<(), ApiError> {
    let key = &ctx.storage_key;
    let chosen = ids.iter().cloned().collect::<BTreeSet<_>>();
    let mut excluded = set_of(raw, "excluded_ids", key)?;
    let mut auto_rejected = set_of(raw, "auto_rejected", key)?;
    let target = match req.action.as_str() {
        "assign" => {
            let pid = req
                .player_id
                .as_deref()
                .ok_or_else(|| storage::bad_request("Choose an existing project player"))?;
            if !ctx.roster.contains_key(pid) {
                return Err(storage::bad_request(
                    "Player is not in the selected project's roster",
                ));
            }
            if chosen.iter().any(|id| excluded.contains(id)) {
                return Err(storage::conflict(
                    "Excluded Track IDs must be restored explicitly before assigning",
                ));
            }
            Some(pid.to_owned())
        }
        "exclude" | "restore" | "unassign" => {
            if req.action == "restore" && chosen.iter().any(|id| !excluded.contains(id)) {
                return Err(storage::conflict(
                    "Only explicitly excluded Track IDs can be restored",
                ));
            }
            None
        }
        _ => return Err(storage::bad_request("Unsupported manual identity action")),
    };
    {
        let groups = groups_mut(raw, key)?;
        if let Some(ref new_owner) = target {
            for id in ids {
                let previous = assigned_to(groups, id);
                if previous.len() > 1 {
                    return Err(storage::conflict(
                        "Conflicting existing owners; resolve this ID manually",
                    ));
                }
                if previous.iter().any(|owner| owner != new_owner) && !req.allow_reassign {
                    return Err(storage::conflict(
                        "Track ID already belongs to another player; explicit allow_reassign is required",
                    ));
                }
            }
            remove_ids(groups, &chosen);
            let name = ctx.roster.get(new_owner).expect("project roster verified");
            let index = groups
                .iter()
                .position(|g| g["id"].as_str() == Some(new_owner));
            let group = match index {
                Some(i) => &mut groups[i],
                None => {
                    groups.push(json!({"id":new_owner,"name":name,"track_ids":[],"link_meta":{}}));
                    groups.last_mut().expect("just pushed")
                }
            };
            group["name"] = json!(name);
            let mut existing = group["track_ids"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(value_id)
                .collect::<BTreeSet<_>>();
            existing.extend(ids.iter().cloned());
            let mut sorted = existing.into_iter().collect::<Vec<_>>();
            sorted.sort_by_key(|id| id.parse::<u64>().unwrap_or(u64::MAX));
            group["track_ids"] = json!(sorted);
            if group["link_meta"].as_object().is_none() {
                group["link_meta"] = json!({});
            }
            let meta = group["link_meta"].as_object_mut().expect("new object");
            for id in ids {
                meta.insert(id.clone(), json!({"source":"manual","reviewed":true}));
            }
            for id in ids {
                auto_rejected.remove(id);
            }
        } else {
            // A restore changes the exclusion list only. Do NOT detach any
            // unrelated owner or clear rejection preferences by accident.
            if req.action != "restore" {
                remove_ids(groups, &chosen);
            }
            match req.action.as_str() {
                "exclude" => {
                    excluded.extend(chosen.iter().cloned());
                    auto_rejected.extend(chosen.iter().cloned());
                }
                "unassign" => {
                    auto_rejected.extend(chosen.iter().cloned());
                }
                "restore" => {
                    for id in ids {
                        excluded.remove(id);
                    }
                    // Keep auto_rejected: restoring visibility never silently
                    // re-enables automatic identity proposals.
                }
                _ => {}
            }
        }
    }
    put_set(raw, "excluded_ids", key, &excluded)?;
    put_set(raw, "auto_rejected", key, &auto_rejected)?;
    raw["schema"] = json!(2);
    Ok(())
}

fn present(ctx: &Context, raw: &Value, revision: String) -> Result<Value, ApiError> {
    let groups = raw["videos"][&ctx.storage_key]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let excluded = set_of(raw, "excluded_ids", &ctx.storage_key)?;
    let rejected = set_of(raw, "auto_rejected", &ctx.storage_key)?;
    let mut assigned = HashMap::<String, String>::new();
    let mut conflicts = BTreeSet::new();
    for group in &groups {
        if let Some(owner) = group["id"].as_str() {
            for id in group["track_ids"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(value_id)
            {
                if !id
                    .parse::<u64>()
                    .ok()
                    .is_some_and(|n| ctx.known.contains_number(n))
                    || excluded.contains(&id)
                {
                    continue;
                }
                if let Some(old) = assigned.insert(id.clone(), owner.to_owned()) {
                    if old != owner {
                        conflicts.insert(id);
                    }
                }
            }
        }
    }
    for id in &conflicts {
        assigned.remove(id);
    }
    Ok(json!({
        "project_id":ctx.project_id,
        "video":ctx.video,
        "base_cache_id":ctx.base_cache_id,
        "source_kind":ctx.evidence.source_kind(),
        "evidence_source":ctx.evidence,
        "revision":revision,
        "read_only":false,
        "source":"project-scoped manual review; immutable GPU detections",
        "groups":groups,
        "excluded_track_ids":excluded,
        "auto_rejected_track_ids":rejected,
        "known_track_count":ctx.known.len(),
        "assigned_count":assigned.len(),
        "unassigned_count":ctx.known.ids().iter().filter(|id|{
            let text=id.to_string();
            !assigned.contains_key(&text) && !excluded.contains(&text)
        }).count(),
        "conflicting_assignment_ids":conflicts,
        "roster":ctx.roster,
    }))
}

/// A whole suggested group must not turn two confirmed people (or two
/// simultaneous on-court identities) into one person, even when the caller
/// permits reassignment of a single ID.
fn guard_group_assignment(ctx: &Context, doc: &Value, ids: &[String]) -> Result<(), ApiError> {
    let selected = ids.iter().collect::<HashSet<_>>();
    let mut confirmed = HashSet::new();
    for group in doc["videos"][&ctx.storage_key]
        .as_array()
        .into_iter()
        .flatten()
    {
        let Some(owner) = group["id"].as_str() else {
            continue;
        };
        for raw in group["track_ids"].as_array().into_iter().flatten() {
            let Some(id) = value_id(raw) else {
                continue;
            };
            if !selected.contains(&id) {
                continue;
            }
            if group["link_meta"][&id]["reviewed"]
                .as_bool()
                .unwrap_or(true)
            {
                confirmed.insert(owner);
            }
        }
    }
    if confirmed.len() > 1 {
        return Err(storage::conflict(
            "В выбранной группе уже подтверждены разные игроки; разделите группу",
        ));
    }
    let timeline = reid_native::track_statistics(&ctx.player_file)?
        .into_iter()
        .map(|row| (row.id.to_string(), (row.first_frame, row.last_frame)))
        .collect::<HashMap<_, _>>();
    for (index, id) in ids.iter().enumerate() {
        let (start, end) = timeline
            .get(id)
            .ok_or_else(|| storage::conflict("Track ID отсутствует в GPU-результатах"))?;
        for other in ids.iter().skip(index + 1) {
            let (next_start, next_end) = timeline
                .get(other)
                .ok_or_else(|| storage::conflict("Track ID отсутствует в GPU-результатах"))?;
            if start <= next_end && next_start <= end {
                return Err(storage::conflict(
                    "Одновременно видимые ID нельзя привязать одной ReID-группой",
                ));
            }
        }
    }
    Ok(())
}

fn calculate(state: &AppState, req: IdentityRequest) -> Result<Value, ApiError> {
    if !matches!(
        req.action.as_str(),
        "read" | "assign" | "unassign" | "exclude" | "restore"
    ) {
        return Err(storage::bad_request("Unsupported identity action"));
    }
    let read_only = req.action == "read";
    if !read_only && !state.writes_enabled {
        return Err(storage::conflict(
            "Manual identity writes require VOLLEY_RUST_DATA_ROOT outside the original repository",
        ));
    }
    let ctx = validate_context(state, &req)?;
    // No writes while reading: snapshot is atomic and read-only.
    if read_only {
        let raw = raw_file(&ctx.identity_path)?;
        let revision = ident_revision(raw.as_deref());
        let data = parse_document(raw.as_deref())?;
        let mut output = present(&ctx, &data, revision)?;
        output["read_only"] = json!(true);
        return Ok(output);
    }
    let expected = req.expected_revision.as_deref().ok_or_else(|| {
        storage::bad_request("Read the identity state first and supply expected_revision")
    })?;
    if expected != "absent"
        && (expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(storage::bad_request("Invalid identity revision"));
    }
    let ids = select_ids(&req, &ctx)?;
    let parent = ctx
        .identity_path
        .parent()
        .ok_or_else(|| storage::internal("Invalid project identity path"))?;
    // The dev root may be in a shared user-controlled path. Do not traverse
    // any pre-existing symlink for the target or its project directory.
    let real_parent = parent
        .canonicalize()
        .map_err(|_| storage::bad_request("Project directory cannot be resolved"))?;
    let real_root = projects::project_root(state)
        .canonicalize()
        .map_err(|_| storage::bad_request("Isolated project root cannot be resolved"))?;
    if !real_parent.starts_with(&real_root) || real_parent == real_root {
        return Err(storage::bad_request(
            "Project identity path outside isolated root",
        ));
    }
    let lock_path = parent.join(".player_identities.lock");
    if fs::symlink_metadata(&lock_path)
        .ok()
        .is_some_and(|meta| meta.file_type().is_symlink())
    {
        return Err(storage::bad_request("Identity lock must not be a symlink"));
    }
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|err| storage::internal(format!("Cannot open identity lock: {err}")))?;
    lock.lock_exclusive()
        .map_err(|err| storage::internal(format!("Cannot acquire identity lock: {err}")))?;
    // Re-read under lock to defeat TOCTOU and stale multi-tab edits.
    let old = raw_file(&ctx.identity_path)?;
    // Group editor and identity editor use the same exclusive project lock.
    // Check the saved group revision again *under the lock* before assigning.
    if req.group_id.is_some() {
        let group_raw = raw_file(&ctx.group_path)?;
        if ident_revision(group_raw.as_deref())
            != req.expected_group_revision.as_deref().unwrap_or("")
        {
            return Err(storage::conflict(
                "ReID-группа уже изменена другой операцией; обновите список",
            ));
        }
    }
    let current = ident_revision(old.as_deref());
    if current != expected {
        return Err(storage::conflict(
            "Player assignments were changed in another tab; refresh and retry",
        ));
    }
    let mut data = parse_document(old.as_deref())?;
    if req.action == "assign" && req.group_id.is_some() {
        guard_group_assignment(&ctx, &data, &ids)?;
    }
    apply(&mut data, &req, &ctx, &ids)?;
    let parent = ctx
        .identity_path
        .parent()
        .ok_or_else(|| storage::internal("Player identity directory missing"))?;
    versioned_store::write_json_checked(parent, &ctx.identity_path, &data, MAX_IDENTITIES_BYTES)
        .map_err(storage::bad_request)?;
    let updated = raw_file(&ctx.identity_path)?
        .ok_or_else(|| storage::internal("Written identity JSON missing"))?;
    let mut payload = present(&ctx, &data, ident_revision(Some(&updated)))?;
    payload["action"] = json!(req.action);
    payload["changed_track_ids"] = json!(ids);
    payload["run_id"] = json!(req.run_id);
    Ok(payload)
}

pub async fn read_or_edit(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<IdentityRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    let _permit = state
        .identity_limit
        .acquire()
        .await
        .map_err(|error| storage::internal(format!("Identity concurrency error: {error}")))?;
    let reading = state.clone();
    let response = tokio::task::spawn_blocking(move || calculate(&reading, req))
        .await
        .map_err(|err| storage::internal(format!("Manual identity task failed: {err}")))??;
    Ok(Json(response))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> Context {
        let evidence = EvidenceSource::legacy(
            "1234abcdef01",
            "match.mp4",
            "run_20261107_123456",
            &"a".repeat(32),
            SourceStamp {
                bytes: 100,
                mtime_ns: 100,
            },
            FrameRate::Unknown,
        )
        .unwrap();
        Context {
            evidence: evidence.clone(),
            player_file: PathBuf::new(),
            identity_path: PathBuf::new(),
            group_path: PathBuf::new(),
            project_id: "1234abcdef01".into(),
            video: "match.mp4".into(),
            base_cache_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            storage_key: "match.mp4::aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            known: VerifiedTrackSet::new(evidence, [5, 8]).unwrap(),
            roster: BTreeMap::from([
                ("player-a".into(), "Player A".into()),
                ("player-b".into(), "Player B".into()),
            ]),
        }
    }
    fn request(action: &str, player_id: Option<&str>, allow_reassign: bool) -> IdentityRequest {
        IdentityRequest {
            project_id: "1234abcdef01".into(),
            run_id: "run_20261107_123456".into(),
            video: "match.mp4".into(),
            action: action.into(),
            track_ids: vec!["5".into()],
            group_id: None,
            expected_group_revision: None,
            player_id: player_id.map(str::to_owned),
            expected_revision: Some("absent".into()),
            allow_reassign,
        }
    }

    #[test]
    fn assignment_requires_explicit_reassignment_and_marks_reviewed() {
        let mut data = parse_document(None).unwrap();
        let ctx = context();
        apply(
            &mut data,
            &request("assign", Some("player-a"), false),
            &ctx,
            &["5".into()],
        )
        .unwrap();
        assert_eq!(
            data["videos"]["match.mp4::aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"][0]["link_meta"]["5"]
                ["reviewed"],
            true
        );
        assert!(apply(
            &mut data,
            &request("assign", Some("player-b"), false),
            &ctx,
            &["5".into()]
        )
        .is_err());
        apply(
            &mut data,
            &request("assign", Some("player-b"), true),
            &ctx,
            &["5".into()],
        )
        .unwrap();
        let groups = data["videos"]["match.mp4::aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
            .as_array()
            .unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0]["id"], "player-b");
        let reply = present(&ctx, &data, "absent".into()).unwrap();
        assert_eq!(reply["source_kind"], "legacy_cpu_cache");
        assert_eq!(reply["evidence_source"]["frame_rate"]["kind"], "unknown");
        assert!(reply["evidence_source"]
            .get("tracked_frames_sha256")
            .is_none());
        assert_eq!(ctx.evidence.clone().track(5).unwrap().track_id, 5);
    }

    #[test]
    fn restoration_does_not_relink_or_forget_manual_rejection() {
        let ctx = context();
        let mut data = parse_document(None).unwrap();
        apply(
            &mut data,
            &request("assign", Some("player-a"), false),
            &ctx,
            &["5".into()],
        )
        .unwrap();
        apply(
            &mut data,
            &request("exclude", None, false),
            &ctx,
            &["5".into()],
        )
        .unwrap();
        assert!(
            data["videos"]["match.mp4::aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(apply(
            &mut data,
            &request("assign", Some("player-a"), false),
            &ctx,
            &["5".into()]
        )
        .is_err());
        apply(
            &mut data,
            &request("restore", None, false),
            &ctx,
            &["5".into()],
        )
        .unwrap();
        assert!(
            data["excluded_ids"]["match.mp4::aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            data["auto_rejected"]["match.mp4::aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"][0],
            "5"
        );
        assert!(
            data["videos"]["match.mp4::aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        // A second restore on a now-visible ID cannot drop unrelated labels.
        assert!(apply(
            &mut data,
            &request("restore", None, false),
            &ctx,
            &["5".into()]
        )
        .is_err());
    }
    #[test]
    fn revision_is_stable_for_unchanged_input_and_distinguishes_absence() {
        assert_eq!(ident_revision(None), "absent");
        assert_eq!(ident_revision(Some(b"{}")), ident_revision(Some(b"{}")));
        assert_ne!(ident_revision(Some(b"{}")), ident_revision(Some(b"{ }")));
    }
}
