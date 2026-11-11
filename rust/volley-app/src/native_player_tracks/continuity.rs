use super::*;
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeContinuityRequest {
    pub(super) project_id: String,
    pub(super) video: String,
    pub(super) replay_id: String,
    pub(super) expected_source_sha256: String,
    #[serde(default = "default_native_gap")]
    pub(super) extended_gap_sec: f64,
}
pub(super) fn default_native_gap() -> f64 {
    4.0
}

pub(super) fn continuity_from_native_replay(
    path: &Path,
    frames: u64,
    expected_sha: &str,
    known: &VerifiedTrackSet,
    exclusions: &BTreeSet<u64>,
    gap: f64,
) -> Result<ContinuityReport, String> {
    let config = ContinuityConfig {
        width: 1.0,
        height: 1.0,
        fps: 30.0,
        extended_gap_sec: gap,
    };
    config.validate().map_err(str::to_owned)?;
    let mut reader = BoundedReader::open(path)?;
    let mut detections = Vec::<PlayerDetection>::new();
    for frame_index in 0..frames {
        let bytes = reader.line()?.ok_or("Native continuity input truncated")?;
        let row: Frame = serde_json::from_slice(&bytes)
            .map_err(|e| format!("Invalid native continuity JSONL row: {e}"))?;
        if row.frame_index != frame_index
            || !row.track_ids_provisional
            || row.confirmed_athlete_identities
            || row.touches_confirmed
            || row.tracked_players.len() > 128
        {
            return Err(
                "Native continuity input contains fabricated identity or wrong frame index".into(),
            );
        }
        let mut seen = BTreeSet::new();
        for item in row.tracked_players {
            if item.is_verified_athlete
                || item.track_id == 0
                || !known.contains_number(item.track_id)
                || !seen.insert(item.track_id)
                || item.detection.frame_index != frame_index
                || item.detection.class_name != "player"
            {
                return Err("Native continuity input has unverified or duplicate Track IDs".into());
            }
            if detections.len() >= 5_000_000 {
                return Err("Native continuity has too many detected players".into());
            }
            detections.push(PlayerDetection {
                frame: u32::try_from(frame_index).map_err(|_| "Unsupported frame index")?,
                track_id: i64::try_from(item.track_id).map_err(|_| "Track ID exceeds i64")?,
                score: item.detection.score,
                bbox: item.detection.bbox_xyxy_norm,
                interpolated: false,
            });
        }
    }
    if reader.line()?.is_some() || reader.finish() != expected_sha {
        return Err("Native continuity source SHA-256 or declared frame count changed".into());
    }
    let excluded = exclusions
        .iter()
        .filter_map(|id| i64::try_from(*id).ok())
        .collect::<std::collections::HashSet<_>>();
    group_tracklets(&detections, config, &excluded).map_err(str::to_owned)
}
pub async fn native_continuity(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<NativeContinuityRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    ContinuityConfig {
        width: 1.0,
        height: 1.0,
        fps: 30.0,
        extended_gap_sec: req.extended_gap_sec,
    }
    .validate()
    .map_err(storage::bad_request)?;
    let _permit = state
        .continuity_limit
        .acquire()
        .await
        .map_err(|e| storage::internal(format!("Continuity CPU gate: {e}")))?;
    let worker = state.clone();
    let report = tokio::task::spawn_blocking(move || -> Result<Value, ApiError> {
        let _guard = project_guard::write_guard(&worker, &req.project_id)?;
        let ctx = source(&worker, &req.project_id, &req.video, &req.replay_id)?;
        if ctx.source_sha != req.expected_source_sha256 {
            return Err(storage::conflict("Selected native CPU replay changed"));
        }
        let doc = validated(
            existing(&ctx.sidecar)
                .map_err(storage::bad_request)?
                .as_deref(),
            &ctx,
        )
        .map_err(storage::bad_request)?;
        let excluded = doc["excluded_ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64)
            .collect::<BTreeSet<_>>();
        let root = projects::project_root(&worker)
            .join(&req.project_id)
            .join("native_cpu_replays")
            .join(&req.replay_id);
        let input =
            safe_file(&root, "tracked_frames.jsonl", MAX_EVIDENCE).map_err(storage::bad_request)?;
        let manifest =
            safe_file(&root, "manifest.json", MAX_MANIFEST).map_err(storage::bad_request)?;
        let metadata: Value = serde_json::from_slice(
            &fs::read(manifest)
                .map_err(|e| storage::bad_request(format!("Read native replay manifest: {e}")))?,
        )
        .map_err(|e| storage::bad_request(format!("Invalid native replay manifest: {e}")))?;
        let frames = metadata["frames"]
            .as_u64()
            .ok_or_else(|| storage::bad_request("Native replay frame count missing"))?;
        let result = continuity_from_native_replay(
            &input,
            frames,
            &ctx.source_sha,
            &ctx.known,
            &excluded,
            req.extended_gap_sec,
        )
        .map_err(storage::bad_request)?;
        Ok(json!({
            "schema":"volley-native-continuity-preview/1",
            "project_id":req.project_id,"video":req.video,"replay_id":req.replay_id,
            "source_replay_sha256":ctx.source_sha,
            "identity_revision":revision(existing(&ctx.sidecar)
                .map_err(storage::bad_request)?.as_deref()),
            "report":result,"auto_assign_enabled":false,
            "confirmed":false,"writable":false
        }))
    })
    .await
    .map_err(|e| storage::internal(format!("Continuity worker: {e}")))??;
    Ok(Json(report))
}
