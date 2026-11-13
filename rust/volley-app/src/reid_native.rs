//! ReID from immutable, precomputed NumPy embedding caches.
//! No Python/OpenVINO runtime; no automatic identity changes. Cached features
//! are checked against current source and model timestamps to reject staleness.

use crate::{
    analysis_runs, player_continuity, projects, rallies, reid_npz,
    storage::{self, ApiError, ApiResult},
    AppState,
};
use axum::{extract::State, http::HeaderMap, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::{self, File},
    io::BufReader,
    path::{Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};
use volley_core::reid::{self, Reference, ReidGroup, TrackInfo};

const MODEL: &str = "person-reidentification-retail-0287";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReidRequest {
    pub project_id: String,
    pub run_id: String,
    pub video: String,
    #[serde(default = "default_threshold")]
    pub threshold_percent: u8,
}
fn default_threshold() -> u8 {
    85
}

fn read_json(path: &Path, limit: u64) -> Result<Value, ApiError> {
    let file = File::open(path).map_err(|_| {
        storage::bad_request(format!(
            "Не найден источник ReID: {}",
            path.file_name().unwrap_or_default().to_string_lossy()
        ))
    })?;
    if file.metadata().map(|stat| stat.len()).unwrap_or(u64::MAX) > limit {
        return Err(storage::bad_request("ReID JSON exceeds supported size"));
    }
    serde_json::from_reader(BufReader::new(file))
        .map_err(|error| storage::bad_request(format!("Invalid ReID JSON: {error}")))
}
fn num(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.parse::<f64>().ok()))
        .filter(|v| v.is_finite())
}
fn int(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|s| s.parse::<i64>().ok()))
        .filter(|v| *v >= 0)
}
fn fingerprint(source: &Path, models: &[PathBuf]) -> Result<String, ApiError> {
    let metadata = fs::metadata(source)
        .map_err(|_| storage::bad_request("ReID source players.json missing"))?;
    let mtime_ns = metadata
        .modified()
        .ok()
        .and_then(|v| v.duration_since(UNIX_EPOCH).ok())
        .ok_or_else(|| storage::bad_request("Cannot read ReID source timestamp"))?
        .as_nanos();
    let mut model_repr = Vec::new();
    for model in models {
        if let Ok(meta) = model.metadata() {
            let time = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .ok_or_else(|| storage::bad_request("Cannot read ReID model timestamp"))?
                .as_nanos();
            model_repr.push(format!("({time}, {})", meta.len()));
        } else {
            model_repr.push("None".into());
        }
    }
    let repr = format!(
        "(1, 'samples-v3-medoid-all-tracks', {{'mtime_ns': {mtime_ns}, 'size': {}}}, [{}])",
        metadata.len(),
        model_repr.join(", ")
    );
    Ok(format!("{:x}", Sha256::digest(repr.as_bytes())))
}

pub(crate) fn path_key(video: &str, cache_id: &str) -> String {
    let input = format!("{video}\0{cache_id}");
    format!("{:x}", Sha256::digest(input.as_bytes()))[..24].to_owned()
}

pub(crate) fn track_statistics(source: &Path) -> Result<Vec<TrackInfo>, ApiError> {
    let doc = read_json(source, 512 * 1024 * 1024)?;
    let rows = doc["predictions"]
        .as_array()
        .ok_or_else(|| storage::bad_request("ReID players.json lacks predictions"))?;
    if rows.len() > 5_000_000 {
        return Err(storage::bad_request("Too many player detections"));
    }
    let width = num(&doc["source_size"]["width"]).unwrap_or(0.0);
    let height = num(&doc["source_size"]["height"]).unwrap_or(0.0);
    let fps = num(&doc["fps"]).unwrap_or(0.0);
    if width <= 0.0 || height <= 0.0 || fps < 1.0 || fps > 240.0 {
        return Err(storage::bad_request(
            "Invalid player detector dimensions/FPS",
        ));
    }
    let mut tracks = HashMap::<i64, TrackInfo>::new();
    for row in rows {
        if row["class_name"].as_str() != Some("player") {
            continue;
        }
        let (Some(id), Some(frame)) = (int(&row["track_id"]), int(&row["frame_index"])) else {
            continue;
        };
        if frame > 10_000_000 {
            continue;
        }
        let Some(bounds) = row["bbox_xyxy"].as_array() else {
            continue;
        };
        if bounds.len() != 4 {
            continue;
        }
        let Some((x0, y0, x1, y1)) = num(&bounds[0])
            .zip(num(&bounds[1]))
            .zip(num(&bounds[2]).zip(num(&bounds[3])))
            .map(|((x0, y0), (x1, y1))| (x0, y0, x1, y1))
        else {
            continue;
        };
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        let center = [(x0 + x1) / (2.0 * width), (y0 + y1) / (2.0 * height)];
        let non_interpolated = row["interpolated"].as_bool() != Some(true);
        match tracks.get_mut(&id) {
            Some(track) => {
                if frame <= track.first_frame {
                    track.first_frame = frame;
                    track.start_center = center;
                }
                if frame >= track.last_frame {
                    track.last_frame = frame;
                    track.end_center = center;
                }
                if non_interpolated {
                    track.detected_frames += 1;
                }
            }
            None => {
                tracks.insert(
                    id,
                    TrackInfo {
                        id,
                        first_frame: frame,
                        last_frame: frame,
                        detected_frames: u32::from(non_interpolated),
                        start_center: center,
                        end_center: center,
                        fps,
                    },
                );
            }
        }
    }
    let mut result = tracks.into_values().collect::<Vec<_>>();
    result.sort_by_key(|row| row.id);
    Ok(result)
}

fn owner_constraints(
    document: &Value,
    video: &str,
    base: &str,
) -> (HashMap<i64, String>, HashSet<i64>) {
    let key = format!("{video}::{base}");
    let mut owners = HashMap::new();
    let mut conflicted = HashSet::new();
    if let Some(groups) = document["videos"][&key].as_array() {
        for group in groups {
            let Some(owner) = group["id"].as_str() else {
                continue;
            };
            if owner.is_empty() {
                continue;
            }
            if let Some(ids) = group["track_ids"].as_array() {
                for tid in ids {
                    let Some(tid) = int(tid) else {
                        continue;
                    };
                    let reviewed = group["link_meta"][tid.to_string()]["reviewed"]
                        .as_bool()
                        .unwrap_or(true);
                    if !reviewed {
                        continue;
                    }
                    match owners.insert(tid, owner.to_owned()) {
                        Some(previous) if previous != owner => {
                            conflicted.insert(tid);
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    for id in &conflicted {
        owners.remove(id);
    }
    (owners, conflicted)
}

/// Only validated, source-current manual groups are allowed to suppress new
/// automatic suggestions. Corrupt/physically impossible historical groups
/// remain on disk but are not presented as safe grouping evidence.
fn vetted_manual_groups(
    record: Option<&Value>,
    stats: &[TrackInfo],
    confirmed: &HashMap<i64, String>,
    excluded: &HashSet<i64>,
    conflicting: &HashSet<i64>,
) -> (Vec<ReidGroup>, HashSet<i64>, usize) {
    let mut vetted = Vec::new();
    let mut pinned = HashSet::new();
    let mut rejected = 0;
    let index = stats
        .iter()
        .map(|track| (track.id, track))
        .collect::<HashMap<_, _>>();
    let Some(groups) = record.and_then(|doc| doc["groups"].as_array()) else {
        return (vetted, pinned, rejected);
    };
    for group in groups {
        if group["source"].as_str() != Some("manual") {
            continue;
        }
        let Some(rows) = group["track_ids"].as_array() else {
            rejected += 1;
            continue;
        };
        let id = group["id"].as_str().filter(|id| {
            !id.is_empty()
                && id.len() <= 128
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        });
        let mut ids = Vec::new();
        let mut members = Vec::new();
        let mut seen = HashSet::new();
        let mut valid = (2..=200).contains(&rows.len()) && id.is_some();
        if valid {
            for row in rows {
                let Some(tid) = int(row) else {
                    valid = false;
                    break;
                };
                let Some(track) = index.get(&tid) else {
                    valid = false;
                    break;
                };
                if !seen.insert(tid)
                    || pinned.contains(&tid)
                    || excluded.contains(&tid)
                    || conflicting.contains(&tid)
                {
                    valid = false;
                    break;
                }
                members.push(reid::ManualGroupMember {
                    track_id: tid,
                    first_frame: track.first_frame,
                    last_frame: track.last_frame,
                    confirmed_owner: confirmed.get(&tid).map(String::as_str),
                });
                ids.push(tid);
            }
        }
        if !valid || !reid::valid_manual_group(&members) {
            rejected += 1;
            continue;
        }
        pinned.extend(ids.iter().copied());
        ids.sort_unstable();
        vetted.push(ReidGroup {
            id: id.expect("checked manual group ID").to_owned(),
            track_ids: ids.iter().map(ToString::to_string).collect(),
            similarity: None,
            average_similarity: None,
            source: "manual",
        });
    }
    (vetted, pinned, rejected)
}

fn restore_manual_groups(
    report: &mut reid::ReidReport,
    vetted: Vec<ReidGroup>,
    pinned: &HashSet<i64>,
) {
    if pinned.is_empty() {
        return;
    }
    report.groups.retain(|group| {
        group.track_ids.iter().all(|tid| {
            tid.parse::<i64>()
                .ok()
                .is_none_or(|id| !pinned.contains(&id))
        })
    });
    report.groups.extend(vetted);
    let used = report
        .groups
        .iter()
        .flat_map(|g| g.track_ids.iter().cloned())
        .collect::<HashSet<_>>();
    report.unmatched_ids.retain(|id| !used.contains(id));
    report
        .groups
        .sort_by_key(|group| std::cmp::Reverse(group.track_ids.len()));
}
fn py_quote(text: &str) -> String {
    let mut s = String::from("'");
    for c in text.chars() {
        match c {
            '\\' => s.push_str("\\\\"),
            '\'' => s.push_str("\\'"),
            '\n' => s.push_str("\\n"),
            '\r' => s.push_str("\\r"),
            '\t' => s.push_str("\\t"),
            _ => s.push(c),
        }
    }
    s.push('\'');
    s
}

fn album_stamp(
    directory: &Path,
    models: &[PathBuf],
    index: &Value,
) -> Result<(String, HashSet<[String; 4]>), ApiError> {
    let manifest = directory.join("index.json");
    let meta = fs::metadata(&manifest).map_err(|_| storage::bad_request("Album index missing"))?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .ok_or_else(|| storage::bad_request("Album index timestamp missing"))?
        .as_nanos();
    let mut sampled = Vec::new();
    let mut origins = HashSet::new();
    let mut keys = HashSet::new();
    if let Some(rows) = index["photos"].as_array() {
        for row in rows.iter().rev() {
            if row["track_assignment_reviewed"].as_bool() != Some(true) {
                continue;
            }
            let Some(name) = row["filename"].as_str() else {
                continue;
            };
            if !name.ends_with(".jpg")
                || name.len() > 240
                || Path::new(name).file_name().and_then(|v| v.to_str()) != Some(name)
            {
                continue;
            }
            if !directory.join("photos").join(name).is_file() {
                continue;
            }
            let origin = [
                row["project_id"].as_str().unwrap_or("").to_owned(),
                row["video"].as_str().unwrap_or("").to_owned(),
                row["cache_id"]
                    .as_str()
                    .unwrap_or("")
                    .split('.')
                    .next()
                    .unwrap_or("")
                    .to_owned(),
                row["track_id"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| int(&row["track_id"]).map(|id| id.to_string()))
                    .unwrap_or_default(),
            ];
            if !keys.insert(origin.clone()) {
                continue;
            }
            sampled.push(name.to_owned());
            origins.insert(origin);
            if sampled.len() >= 36 {
                break;
            }
        }
    }
    let photos = format!(
        "[{}]",
        sampled
            .iter()
            .map(|s| py_quote(s))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let stamp1 = format!(
        "{:x}",
        Sha256::digest(format!("({mtime}, {}, {photos})", meta.len()).as_bytes())
    )[..24]
        .to_owned();
    let mut model_parts = Vec::new();
    for path in models {
        if let Ok(meta) = path.metadata() {
            let time = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .ok_or_else(|| storage::bad_request("Model timestamp unavailable"))?
                .as_nanos();
            model_parts.push(format!("({}, {time})", meta.len()));
        } else {
            model_parts.push("None".to_owned());
        }
    }
    let repr = format!(
        "({}, [{}], 'uniform-crop-v2')",
        py_quote(&stamp1),
        model_parts.join(", ")
    );
    let stamp2 = format!("{:x}", Sha256::digest(repr.as_bytes()))[..24].to_owned();
    Ok((stamp2, origins))
}

fn vetted_album_references(
    state: &AppState,
    roster: &BTreeMap<String, String>,
    mode: reid::FeatureMode,
    models: &[PathBuf],
) -> Vec<Reference> {
    let mut references = Vec::new();
    for pid in roster.keys() {
        if pid.is_empty()
            || pid.len() > 128
            || !pid
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        {
            continue;
        }
        // Historical photo albums remain read-only under the original data
        // root; isolated Rust output projects do not modify these files.
        let folder = state.repository_root.join("results/players").join(pid);
        let index_path = folder.join("index.json");
        let Ok(index) = read_json(&index_path, 16 * 1024 * 1024) else {
            continue;
        };
        let Ok((stamp, allowed)) = album_stamp(&folder, models, &index) else {
            continue;
        };
        let file = folder.join(format!("reid-reference-{}.npz", mode.as_str()));
        let Ok(cache) = reid_npz::read_reference_embeddings(&file) else {
            continue;
        };
        if cache.stamp != stamp {
            continue;
        }
        for (vector, origin) in cache.vectors.into_iter().zip(cache.origins) {
            if !allowed.contains(&origin) {
                continue;
            }
            references.push(Reference {
                player_id: pid.clone(),
                vector,
                origin,
            });
        }
    }
    references
}

pub(crate) fn calculate(state: &AppState, request: ReidRequest) -> Result<Value, ApiError> {
    storage::project_id(&request.project_id)?;
    storage::video_name(&request.video)?;
    if !(55..=99).contains(&request.threshold_percent) {
        return Err(storage::bad_request(
            "Порог сходства ReID должен быть от 55 до 99%",
        ));
    }
    let project = projects::existing_project(state, &request.project_id)?;
    let run = analysis_runs::list_runs(state, &request.project_id)?
        .into_iter()
        .find(|v| v["id"].as_str() == Some(&request.run_id))
        .ok_or_else(|| storage::bad_request("Не найден сохранённый запуск анализа"))?;
    let cache_id = run["videos"][&request.video]
        .as_str()
        .ok_or_else(|| storage::bad_request("Это видео не входит в запуск"))?;
    let selected = rallies::cache_root(state, &request.project_id, &request.video, cache_id)?;
    let base_id = cache_id.split('.').next().unwrap_or(cache_id);
    let base = if cache_id.contains('.') {
        selected
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| storage::bad_request("Invalid CPU path"))?
            .to_path_buf()
    } else {
        selected
    };
    let source = base.join("players.json");
    let models = [
        state
            .repository_root
            .join("models/openvino")
            .join(format!("{MODEL}.xml")),
        state
            .repository_root
            .join("models/openvino")
            .join(format!("{MODEL}.bin")),
    ];
    let key = path_key(&request.video, base_id);
    let group_dir = projects::project_root(state)
        .join(&request.project_id)
        .join("player_identity_groups");
    let npz = group_dir.join(format!("{key}.npz"));
    let cache=reid_npz::read_embeddings(&npz).map_err(|error|storage::bad_request(
        format!("Сохранённые ReID-признаки недоступны или повреждены ({error}). Извлечение новых признаков пока не реализовано.")
    ))?;
    let expected = fingerprint(&source, &models)?;
    if cache.fingerprint != expected {
        return Err(storage::bad_request(
            "ReID-кэш устарел: видео детектора или модель изменились. Нельзя сравнивать устаревшие признаки."
        ));
    }
    let stats = track_statistics(&source)?;
    let known = stats.iter().map(|row| row.id).collect::<HashSet<_>>();
    let excluded =
        player_continuity::excluded_ids(state, &request.project_id, &request.video, base_id)?;
    let identity_path = projects::project_root(state)
        .join(&request.project_id)
        .join("player_identities.json");
    let identity = if identity_path.is_file() {
        read_json(&identity_path, 16 * 1024 * 1024)?
    } else {
        json!({})
    };
    let (confirmed, conflicts) = owner_constraints(&identity, &request.video, base_id);
    let manual_doc = storage::read_json(&group_dir.join(format!("{key}.json")))?;
    if let Some(ref saved) = manual_doc {
        let meta = fs::metadata(&source)
            .map_err(|_| storage::bad_request("GPU player source is unavailable"))?;
        let modified = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|time| time.as_nanos());
        let saved_stamp = saved["stamp"]["mtime_ns"].as_u64().map(u128::from);
        if saved["schema"].as_u64() != Some(1)
            || modified != saved_stamp
            || saved["stamp"]["size"].as_u64() != Some(meta.len())
        {
            return Err(storage::conflict(
                "Сохранённые ReID-группы относятся к старому GPU-источнику; не применяйте их автоматически"
            ));
        }
    }
    let mut prohibited = excluded.clone();
    prohibited.extend(conflicts.iter().copied());
    // Pinned/manual group members are never re-assigned to a different
    // automatic group when the threshold slider changes.
    let mut manual_excluded = HashSet::new();
    if let Some(ref doc) = manual_doc {
        if let Some(rows) = doc["manual_excluded_ids"].as_array() {
            for row in rows {
                if let Some(id) = int(row) {
                    manual_excluded.insert(id);
                }
            }
        }
    }
    let (vetted_groups, pinned, rejected_manual_groups) = vetted_manual_groups(
        manual_doc.as_ref(),
        &stats,
        &confirmed,
        &excluded,
        &conflicts,
    );
    prohibited.extend(manual_excluded.iter().copied());
    prohibited.extend(pinned.iter().copied());
    let mut result = reid::build_groups(
        &cache.features,
        &stats,
        &confirmed,
        &prohibited,
        cache.mode,
        request.threshold_percent,
    )
    .map_err(storage::bad_request)?;
    restore_manual_groups(&mut result, vetted_groups, &pinned);
    // A user-split ID remains visible as unassigned, not quietly erased
    // from the ReID table when the similarity threshold changes.
    for tid in &manual_excluded {
        if known.contains(tid)
            && !excluded.contains(tid)
            && !result.unmatched_ids.contains(&tid.to_string())
        {
            result.unmatched_ids.push(tid.to_string());
        }
    }
    result
        .unmatched_ids
        .sort_by_key(|id| id.parse::<i64>().unwrap_or(i64::MAX));
    result.total_tracks = known.iter().filter(|id| !excluded.contains(id)).count();
    let usable_features = cache
        .features
        .iter()
        .filter(|f| !excluded.contains(&f.id) && !conflicts.contains(&f.id))
        .cloned()
        .collect::<Vec<_>>();
    let trusted_owners = confirmed
        .iter()
        .filter(|(id, _)| !excluded.contains(id) && !conflicts.contains(id))
        .map(|(id, owner)| (*id, owner.clone()))
        .collect::<HashMap<_, _>>();
    let mut roster = BTreeMap::new();
    let registry_path = state.repository_root.join("results/players/registry.json");
    if let Ok(catalog) = read_json(&registry_path, 16 * 1024 * 1024) {
        let roster_ids = project["player_ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<HashSet<_>>();
        if let Some(players) = catalog["players"].as_array() {
            for player in players {
                if let (Some(id), Some(name)) = (player["id"].as_str(), player["name"].as_str()) {
                    if roster_ids.contains(id) {
                        roster.insert(id.to_owned(), name.to_owned());
                    }
                }
            }
        }
    }
    let mut refs = Vec::new();
    for feat in &usable_features {
        if let Some(pid) = trusted_owners.get(&feat.id) {
            if roster.contains_key(pid) && !excluded.contains(&feat.id) {
                refs.push(Reference {
                    player_id: pid.clone(),
                    vector: feat.vector.clone(),
                    origin: [
                        request.project_id.clone(),
                        request.video.clone(),
                        base_id.to_owned(),
                        feat.id.to_string(),
                    ],
                });
            }
        }
    }
    let enrolled = refs.len();
    refs.extend(vetted_album_references(state, &roster, cache.mode, &models));
    let (matches, counts) = reid::match_references(
        &usable_features,
        &roster,
        &trusted_owners,
        &refs,
        reid::MatchScope {
            project_id: &request.project_id,
            video: &request.video,
            base_cache: base_id,
        },
        request.threshold_percent,
    )
    .map_err(storage::bad_request)?;
    // If the source or OpenVINO weight set changes during this calculation,
    // refuse to publish a stale match result even to a read-only caller.
    if fingerprint(&source, &models)? != expected {
        return Err(storage::conflict(
            "ReID source changed during computation; retry",
        ));
    }
    Ok(json!({
        "schema":1,
        "similarity_schema":6,
        "status":"ready",
        "method":cache.mode.as_str(),
        "project_id":request.project_id,
        "run_id":request.run_id,
        "video":request.video,
        "cache_id":base_id,
        "read_only":true,
        "embedding_dimension":cache.dimensions,
        "excluded_track_ids":excluded.len(),
        "manual_split_ids":manual_excluded.len(),
        "manual_pinned_ids":pinned.len(),
        "invalid_manual_groups_ignored":rejected_manual_groups,
        "conflicting_manual_assignments":conflicts.len(),
        "trusted_track_references":enrolled,
        "trusted_photo_references":refs.len().saturating_sub(enrolled),
        "reference_counts":counts,
        "player_matches":matches,
        "auto_assign_enabled":false,
        "score_type":"cosine_similarity_not_probability",
        "warning":"All automatic ReID groups and player matches are suggestions, never committed identities. Missing or stale NPZ requires future native embedding capture.",
        "embedding":result,
    }))
}

pub async fn preview(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<ReidRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    let _permit = state
        .reid_limit
        .acquire()
        .await
        .map_err(|e| storage::internal(format!("ReID preview concurrency: {e}")))?;
    let reading = state.clone();
    let result = tokio::task::spawn_blocking(move || calculate(&reading, request))
        .await
        .map_err(|e| storage::internal(format!("ReID task failed: {e}")))??;
    Ok(Json(result))
}

#[cfg(test)]
mod manual_group_tests {
    use super::*;
    fn track(id: i64, start: i64, end: i64) -> TrackInfo {
        TrackInfo {
            id,
            first_frame: start,
            last_frame: end,
            detected_frames: 12,
            start_center: [0.2, 0.2],
            end_center: [0.2, 0.2],
            fps: 30.0,
        }
    }
    #[test]
    fn restored_manual_groups_never_bypass_overlap_or_confirmed_owner_checks() {
        let tracks = vec![
            track(1, 0, 10),
            track(2, 5, 15),
            track(3, 20, 30),
            track(4, 40, 50),
        ];
        let owners = HashMap::from([
            (1, "athlete-a".to_owned()),
            (2, "athlete-b".to_owned()),
            (3, "athlete-a".to_owned()),
            (4, "athlete-a".to_owned()),
        ]);
        let saved = json!({"groups":[
            {"id":"manual-overlap","source":"manual","track_ids":["1","2"]},
            {"id":"manual-safe","source":"manual","track_ids":["1","3"]},
            {"id":"manual-duplicate","source":"manual","track_ids":["3","4"]},
            {"id":"manual-distinct-owner","source":"manual","track_ids":["2","4"]}
        ]});
        let (groups, pinned, rejected) = vetted_manual_groups(
            Some(&saved),
            &tracks,
            &owners,
            &HashSet::new(),
            &HashSet::new(),
        );
        assert_eq!(rejected, 3);
        assert_eq!(pinned, HashSet::from([1, 3]));
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].track_ids, ["1", "3"]);
        assert_eq!(groups[0].similarity, None);
        assert_eq!(groups[0].source, "manual");
    }
    #[test]
    fn rejected_manual_group_cannot_mask_tracks_from_auto_suggestions() {
        let tracks = vec![track(1, 0, 10), track(2, 10, 15), track(3, 16, 30)];
        let saved = json!({"groups":[
            {"id":"manual-invalid","source":"manual","track_ids":["1","2"]},
            {"id":"manual-valid","source":"manual","track_ids":["1","3"]}
        ]});
        let (groups, pinned, rejected) = vetted_manual_groups(
            Some(&saved),
            &tracks,
            &HashMap::new(),
            &HashSet::new(),
            &HashSet::new(),
        );
        assert_eq!(rejected, 1);
        assert_eq!(groups.len(), 1);
        assert_eq!(pinned, HashSet::from([1, 3]));
        assert!(!pinned.contains(&2));
        let (none, empty, rejected) = vetted_manual_groups(
            Some(&saved),
            &tracks,
            &HashMap::new(),
            &HashSet::from([3]),
            &HashSet::new(),
        );
        assert!(none.is_empty());
        assert!(empty.is_empty());
        assert_eq!(rejected, 2);
    }
}
