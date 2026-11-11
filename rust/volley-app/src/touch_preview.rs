//! Non-mutating RAVEL-013 Touch diagnostics from a selected stored inference/CPU run.
//! Verified project+run ownership; no Python process, no expensive GPU replay.
use crate::{
    analysis_runs, player_continuity, projects, rallies,
    storage::{self, ApiResult},
    AppState,
};
use axum::{extract::State, http::HeaderMap, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap},
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    sync::Arc,
};
use volley_core::{
    continuity::{group_tracklets, ContinuityConfig, PlayerDetection},
    motion::BallObservation,
    touch::{
        associate_touch_players, confirm_ravel_contacts, select_touch_peaks, PeakConfig,
        TouchCandidate, TouchContact, TouchPeak, TrackedPlayer,
    },
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TouchPreviewRequest {
    pub project_id: String,
    pub run_id: String,
    pub video: String,
    #[serde(default = "default_threshold")]
    pub threshold: f64,
    #[serde(default = "default_smooth")]
    pub smooth: usize,
    #[serde(default = "default_gap")]
    pub min_gap: usize,
    #[serde(default = "default_reach")]
    pub max_reach_ratio: f64,
}

fn default_threshold() -> f64 {
    0.5
}
fn default_smooth() -> usize {
    5
}
fn default_gap() -> usize {
    10
}
fn default_reach() -> f64 {
    0.35
}

fn number(row: &Value) -> Option<f64> {
    row.as_f64()
        .or_else(|| row.as_str().and_then(|value| value.parse::<f64>().ok()))
        .filter(|n| n.is_finite())
}

fn frame_num(value: &Value) -> Option<u32> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|s| s.parse::<u64>().ok()))
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n <= 2_000_000)
}

fn bounding(row: &Value, width: f64, height: f64, normalized: bool) -> Option<[f64; 4]> {
    let vals = row.as_array()?;
    if vals.len() != 4 {
        return None;
    }
    let factors = if normalized {
        [width, height, width, height]
    } else {
        [1.0; 4]
    };
    let mut box_xyxy = [0_f64; 4];
    for (i, value) in vals.iter().enumerate() {
        box_xyxy[i] = number(value)? * factors[i];
    }
    (box_xyxy[2] > box_xyxy[0] && box_xyxy[3] > box_xyxy[1]).then_some(box_xyxy)
}

fn capped_json(path: &Path, size_limit: u64) -> Result<Value, storage::ApiError> {
    let file =
        File::open(path).map_err(|_| storage::bad_request("Сохранённые обнаружения недоступны"))?;
    if file.metadata().map(|meta| meta.len()).unwrap_or(u64::MAX) > size_limit {
        return Err(storage::bad_request(
            "Слишком большой файл результатов для просмотра Touch",
        ));
    }
    serde_json::from_reader(BufReader::new(file))
        .map_err(|e| storage::bad_request(format!("Повреждённый JSON анализа: {e}")))
}

fn gpu_cache_root(
    state: &AppState,
    project_id: &str,
    run_id: &str,
    video: &str,
) -> Result<(PathBuf, PathBuf), storage::ApiError> {
    projects::existing_project(state, project_id)?;
    let run = analysis_runs::list_runs(state, project_id)?
        .into_iter()
        .find(|row| row["id"].as_str() == Some(run_id))
        .ok_or_else(|| storage::bad_request("Не найден указанный запуск анализа"))?;
    let chosen = run["videos"][video]
        .as_str()
        .ok_or_else(|| storage::bad_request("Видео не принадлежит выбранному запуску"))?;
    let derived = rallies::cache_root(state, project_id, video, chosen)?;
    let base = if chosen.contains('.') {
        derived
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| storage::bad_request("Неверный кэш CPU"))?
            .to_path_buf()
    } else {
        derived.clone()
    };
    let manifest = capped_json(&base.join("inference.manifest.json"), 4 * 1024 * 1024)?;
    if manifest["schema"].as_str() != Some("volley-inference/1")
        || manifest["player_model"].as_str() != Some("RAVEL-VB-013-9f")
        || !manifest["outputs"].get("players.raw.v2.jsonl").is_some()
    {
        return Err(storage::bad_request(
            "Для этого запуска нет исходных RAVEL-013 Touch-предсказаний",
        ));
    }
    Ok((base, derived))
}

fn read_raw(
    base: &Path,
    width: f64,
    height: f64,
) -> Result<(Vec<TouchCandidate>, usize), storage::ApiError> {
    let raw = base.join("players.raw.v2.jsonl");
    if raw.metadata().map(|meta| meta.len()).unwrap_or(u64::MAX) > 4 * 1024 * 1024 * 1024 {
        return Err(storage::bad_request(
            "Исходные RAVEL-предсказания слишком большие",
        ));
    }
    let input =
        File::open(raw).map_err(|_| storage::bad_request("Raw RAVEL предсказания отсутствуют"))?;
    let mut strongest: HashMap<u32, TouchCandidate> = HashMap::new();
    let mut count = 0_usize;
    for line in BufReader::new(input).lines() {
        let line = line.map_err(|e| storage::bad_request(format!("Ошибка чтения RAVEL: {e}")))?;
        if line.len() > 1_500_000 {
            return Err(storage::bad_request("Слишком длинная запись RAVEL"));
        }
        let row: Value = serde_json::from_str(&line)
            .map_err(|e| storage::bad_request(format!("Повреждённый RAVEL JSONL: {e}")))?;
        let Some(frame) = frame_num(&row["frame"]) else {
            continue;
        };
        let Some(items) = row["predictions"].as_array() else {
            continue;
        };
        for item in items {
            if item["class_name"].as_str() != Some("player") {
                continue;
            }
            let Some(score) =
                number(&item["action_scores"]["Touch"]).or_else(|| number(&item["action_score"]))
            else {
                continue;
            };
            if !(0.0..=1.0).contains(&score) || score == 0.0 {
                continue;
            }
            let Some(bbox) = bounding(&item["bbox_xyxy_norm"], width, height, true) else {
                continue;
            };
            count += 1;
            let candidate = TouchCandidate {
                frame,
                probability: score,
                bbox,
            };
            if strongest
                .get(&frame)
                .is_none_or(|current| score > current.probability)
            {
                strongest.insert(frame, candidate);
            }
        }
        if strongest.len() > 450_000 {
            return Err(storage::bad_request("Неверная длительность raw RAVEL"));
        }
    }
    let mut result = strongest.into_values().collect::<Vec<_>>();
    result.sort_by_key(|row| row.frame);
    Ok((result, count))
}

fn tracked_players(
    source: &Path,
    width: f64,
    height: f64,
) -> Result<Vec<TrackedPlayer>, storage::ApiError> {
    let data = capped_json(source, 512 * 1024 * 1024)?;
    let preds = data["predictions"].as_array().ok_or_else(|| {
        storage::bad_request("В сохранённом players JSON отсутствуют predictions")
    })?;
    if preds.len() > 3_000_000 {
        return Err(storage::bad_request("Слишком много player predictions"));
    }
    let mut tracks = Vec::new();
    for row in preds {
        if row["class_name"].as_str() != Some("player")
            || row["interpolated"].as_bool() == Some(true)
        {
            continue;
        }
        let Some(frame) = frame_num(&row["frame_index"]) else {
            continue;
        };
        let Some(id) = row["track_id"].as_i64().or_else(|| {
            row["track_id"]
                .as_str()
                .and_then(|text| text.parse::<i64>().ok())
        }) else {
            continue;
        };
        let bbox = bounding(&row["bbox_xyxy"], width, height, false)
            .or_else(|| bounding(&row["bbox_xyxy_norm"], width, height, true));
        if let Some(bbox) = bbox {
            tracks.push(TrackedPlayer {
                frame,
                track_id: id,
                score: number(&row["score"]).unwrap_or(0.0),
                bbox,
            });
        }
    }
    Ok(tracks)
}

fn contact_tracks(
    folder: &Path,
    peaks: &[TouchPeak],
    threshold: f64,
    reach: f64,
) -> Result<Vec<TouchContact>, storage::ApiError> {
    let path = folder.join("analysis/game-tracks.json");
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let data = capped_json(&path, 256 * 1024 * 1024)?;
    let rows = data
        .as_array()
        .ok_or_else(|| storage::bad_request("Некорректные ball tracks выбранной CPU-версии"))?;
    let mut result = Vec::new();
    for row in rows {
        let classification = &row["rally_classification"];
        if classification["is_rally"].as_bool() != Some(true)
            && classification["label"].as_str() != Some("rally")
        {
            continue;
        }
        let Some(start) = frame_num(&row["start_frame"]) else {
            continue;
        };
        let Some(end) = frame_num(&row["last_frame"]) else {
            continue;
        };
        if end < start {
            continue;
        }
        let Some(observed) = row["frame_observations"].as_array() else {
            continue;
        };
        if observed.len() > 1_000_000 {
            return Err(storage::bad_request("Слишком длинный трек мяча"));
        }
        let mut points = observed
            .iter()
            .filter_map(|p| {
                Some(BallObservation {
                    frame: frame_num(&p["frame"])? as i64,
                    x: number(&p["x"])?,
                    y: number(&p["y"])?,
                })
            })
            .collect::<Vec<_>>();
        points.sort_by_key(|p| p.frame);
        let visible = peaks
            .iter()
            .filter(|p| p.frame_index >= start && p.frame_index <= end)
            .cloned()
            .collect::<Vec<_>>();
        if !visible.is_empty() && points.len() >= 4 {
            result.extend(confirm_ravel_contacts(
                &visible,
                &points,
                start as i64,
                end as i64,
                threshold,
                reach,
                0.35,
            ));
        }
    }
    result.sort_by_key(|p| p.frame);
    // One physical touch may be close to two separate ball-track fragments.
    result.dedup_by(|b, a| b.frame.abs_diff(a.frame) <= 5 && b.motion_frame == a.motion_frame);
    Ok(result)
}

fn preview(state: &AppState, request: TouchPreviewRequest) -> Result<Value, storage::ApiError> {
    storage::project_id(&request.project_id)?;
    storage::video_name(&request.video)?;
    if !request.max_reach_ratio.is_finite() || !(0.0..=1.0).contains(&request.max_reach_ratio) {
        return Err(storage::bad_request("Недопустимая дальность касания"));
    }
    let config = PeakConfig {
        threshold: request.threshold,
        smooth: request.smooth,
        min_gap: request.min_gap,
    };
    config.validate().map_err(storage::bad_request)?;
    let (base, selected) =
        gpu_cache_root(state, &request.project_id, &request.run_id, &request.video)?;
    let manifest = capped_json(&base.join("inference.manifest.json"), 4 * 1024 * 1024)?;
    let width = number(&manifest["width"]).unwrap_or(0.0);
    let height = number(&manifest["height"]).unwrap_or(0.0);
    if width <= 0.0
        || height <= 0.0
        || number(&manifest["fps"]).is_none_or(|n| (n - 30.0).abs() > 0.2)
    {
        return Err(storage::bad_request(
            "Неподдерживаемые размеры/FPS inference cache",
        ));
    }
    let (predictions, count) = read_raw(&base, width, height)?;
    let mut peaks = select_touch_peaks(&predictions, config).map_err(storage::bad_request)?;
    let from_replay = selected.join("players.replay.json");
    let players = if from_replay.is_file() {
        from_replay
    } else {
        base.join("players.json")
    };
    let base_cache_id = base
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| storage::bad_request("Неверный идентификатор инференса"))?;
    let excluded =
        player_continuity::excluded_ids(state, &request.project_id, &request.video, base_cache_id)?;
    let mut stable = tracked_players(&players, width, height)?;
    // Rejected IDs may not be attributed to a player again. Contacts can
    // still exist as unknown actors; this does not erase ball evidence.
    stable.retain(|row| !excluded.contains(&row.track_id));
    associate_touch_players(&mut peaks, &stable);
    let attributed = peaks.iter().filter(|p| p.track_id.is_some()).count();
    let contacts = contact_tracks(
        &selected,
        &peaks,
        request.threshold,
        request.max_reach_ratio,
    )?;
    let mut candidate_touch_counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut unmatched_touches = 0_u64;
    if !contacts.is_empty() {
        let observations: Vec<PlayerDetection> = stable
            .iter()
            .map(|row| PlayerDetection {
                frame: row.frame,
                track_id: row.track_id,
                score: row.score,
                bbox: row.bbox,
                interpolated: false,
            })
            .collect();
        let groups = group_tracklets(
            &observations,
            ContinuityConfig {
                width,
                height,
                fps: 30.0,
                extended_gap_sec: 4.0,
            },
            &excluded,
        )
        .map_err(storage::bad_request)?;
        for contact in &contacts {
            let candidate = contact
                .player_track_id
                .and_then(|id| groups.track_to_candidate.get(&id.to_string()));
            if let Some(candidate) = candidate {
                *candidate_touch_counts.entry(candidate.clone()).or_insert(0) += 1;
            } else {
                unmatched_touches += 1;
            }
        }
    }
    Ok(json!({
        "project_id":request.project_id,
        "run_id":request.run_id,
        "video":request.video,
        "source":"immutable RAVEL-013 proposals + saved CPU ball tracks",
        "mode":"read_only_diagnostic",
        "threshold":request.threshold,
        "smooth":request.smooth,
        "min_gap":request.min_gap,
        "raw_proposals":count,
        "observed_frames":predictions.len(),
        "peaks_total":peaks.len(),
        "attributed_peaks":attributed,
        "confirmed_contacts":contacts,
        "confirmed_count":contacts.len(),
        "unconfirmed_candidate_touch_counts":candidate_touch_counts,
        "touches_without_candidate_id":unmatched_touches,
        "excluded_track_ids":excluded.len(),
        "candidate_id_status":"unconfirmed_geometry_only; not a real player identity",
        "peaks":peaks,
        "motion_evidence_available":selected.join("analysis/game-tracks.json").is_file(),
        "warning":"Diagnostic only: not persisted, no technique classes, no global geometry/CPU event merge"
    }))
}

pub async fn touch_preview(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<TouchPreviewRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    let _permit = state
        .touch_limit
        .acquire()
        .await
        .map_err(|e| storage::internal(format!("Touch preview concurrency: {e}")))?;
    let reading = state.clone();
    let value = tokio::task::spawn_blocking(move || preview(&reading, request))
        .await
        .map_err(|e| storage::internal(format!("Touch preview task failed: {e}")))??;
    Ok(Json(value))
}
