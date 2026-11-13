use super::*;
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BallTrajectoryRequest {
    pub(super) project_id: String,
    pub(super) video: String,
    pub(super) replay_id: String,
    pub(super) expected_source_sha256: String,
    pub(super) start_frame: u64,
    #[serde(default = "trajectory_count")]
    pub(super) count: u64,
}
pub(super) fn trajectory_count() -> u64 {
    300
}
pub(super) fn extract_native_ball_trajectory(
    path: &Path,
    expected_frames: u64,
    expected_sha: &str,
    start: u64,
    count: u64,
    width: u32,
    height: u32,
) -> Result<(Vec<BallObservation>, Option<String>), String> {
    if count == 0
        || count > 1200
        || start
            .checked_add(count)
            .is_none_or(|end| end > expected_frames)
        || !(64..=16384).contains(&width)
        || !(64..=16384).contains(&height)
    {
        return Err("Selected-ball diagnostic window is out of bounds".into());
    }
    let mut reader = BoundedReader::open(path)?;
    let mut observations = Vec::new();
    let mut source_model = None::<String>;
    for frame_index in 0..expected_frames {
        let bytes = reader
            .line()?
            .ok_or("Native ball trajectory input truncated")?;
        let row: Frame = serde_json::from_slice(&bytes)
            .map_err(|e| format!("Invalid saved native frame: {e}"))?;
        if row.frame_index != frame_index
            || !row.track_ids_provisional
            || row.confirmed_athlete_identities
            || row.touches_confirmed
        {
            return Err("Ball trajectory source lacks provisional native status".into());
        }
        if frame_index < start || frame_index >= start + count {
            continue;
        }
        let Some(ball) = row.selected_ball else {
            continue;
        };
        if !matches!(ball.source_model.as_str(), "vballnet" | "ravel")
            || ball.detection.class_name != "ball"
            || ball.detection.frame_index != frame_index
        {
            return Err("Saved ball trajectory contains unsupported detection".into());
        }
        ball.detection.validate(
            frame_index - frame_index % 9,
            if ball.source_model == "ravel" {
                "players"
            } else {
                "ball"
            },
        )?;
        if ball.source_model != "vballnet" {
            continue;
        }
        let bbox = ball.detection.bbox_xyxy_norm;
        if source_model
            .as_ref()
            .is_some_and(|s| s != &ball.source_model)
        {
            return Err(
                "Selected-ball source switched in motion window; select a narrower window".into(),
            );
        }
        source_model = Some(ball.source_model);
        observations.push(BallObservation {
            frame: i64::try_from(frame_index).map_err(|_| "Frame index overflow")?,
            x: (bbox[0] + bbox[2]) * width as f64 * 0.5,
            y: (bbox[1] + bbox[3]) * height as f64 * 0.5,
        });
    }
    if reader.line()?.is_some() || reader.finish() != expected_sha {
        return Err("Saved native selected-ball replay SHA-256 has changed".into());
    }
    Ok((observations, source_model))
}
pub async fn ball_trajectory(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<BallTrajectoryRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    if req.count == 0 || req.count > 1200 {
        return Err(storage::bad_request(
            "Ball trajectory preview covers 1..1200 frames",
        ));
    }
    let _permit = state
        .touch_limit
        .acquire()
        .await
        .map_err(|e| storage::internal(format!("Ball CPU gate: {e}")))?;
    let worker = state.clone();
    let response = tokio::task::spawn_blocking(move || -> Result<Value, ApiError> {
        let _guard = project_guard::write_guard(&worker, &req.project_id)?;
        let ctx = source(&worker, &req.project_id, &req.video, &req.replay_id)?;
        if ctx.source_sha != req.expected_source_sha256 {
            return Err(storage::conflict("Selected-ball replay source SHA changed"));
        }
        let project = projects::project_root(&worker)
            .join(&req.project_id)
            .join("native_cpu_replays")
            .join(&req.replay_id);
        let manifest =
            safe_file(&project, "manifest.json", MAX_MANIFEST).map_err(storage::bad_request)?;
        let header: Value = serde_json::from_slice(
            &fs::read(manifest)
                .map_err(|e| storage::internal(format!("Read ball manifest: {e}")))?,
        )
        .map_err(|e| storage::bad_request(format!("Bad native ball manifest: {e}")))?;
        let frames = header["frames"]
            .as_u64()
            .ok_or_else(|| storage::bad_request("Native ball manifest lacks frame count"))?;
        let plan = preparation::plan_for(&worker, &req.project_id, &req.video)?;
        let prepared = plan.ready_path().ok_or_else(|| {
            storage::bad_request("Prepared video unavailable for trajectory dimensions")
        })?;
        let dimensions = volley_media::probe(prepared).map_err(storage::bad_request)?;
        let input = safe_file(&project, "tracked_frames.jsonl", MAX_EVIDENCE)
            .map_err(storage::bad_request)?;
        let (samples, source_model) = extract_native_ball_trajectory(
            &input,
            frames,
            &ctx.source_sha,
            req.start_frame,
            req.count,
            dimensions.width,
            dimensions.height,
        )
        .map_err(storage::bad_request)?;
        let extrema = ball_motion_extrema(&samples, 8).map_err(storage::bad_request)?;
        Ok(json!({
            "schema":"volley-native-ball-extrema/1",
            "project_id":req.project_id,"video":req.video,"replay_id":req.replay_id,
            "source_replay_sha256":ctx.source_sha,
            "selected_ball_source":source_model,
            "window_start_frame":req.start_frame,"window_frames":req.count,
            "observations":samples.len(),"motion_extrema":extrema,
            "method":"gravity_compensated_rust_ball_motion",
            "touches_confirmed":false,"athlete_identities_verified":false,
            "gpu_inference_rerun":false
        }))
    })
    .await
    .map_err(|e| storage::internal(format!("Ball trajectory worker: {e}")))??;
    Ok(Json(response))
}
