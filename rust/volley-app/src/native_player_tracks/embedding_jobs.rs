use super::*;
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingRequest {
    pub(super) project_id: String,
    pub(super) video: String,
    pub(super) replay_id: String,
    #[serde(default)]
    pub(super) rally_run_id: Option<String>,
    pub(super) expected_source_sha256: String,
    pub(super) track_ids: Vec<u64>,
    #[serde(default)]
    pub(super) transient: bool,
}

pub async fn embeddings(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<EmbeddingRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    if !valid_sha(&req.expected_source_sha256)
        || req.track_ids.is_empty()
        || req.track_ids.len() > 8
        || req.track_ids.iter().collect::<BTreeSet<_>>().len() != req.track_ids.len()
    {
        return Err(storage::bad_request(
            "Select 1–8 unique native Track IDs with exact source SHA-256",
        ));
    }
    let _reid_slot = state
        .reid_limit
        .acquire()
        .await
        .map_err(|e| storage::internal(format!("ReID gate: {e}")))?;
    let repo = state.repository_root.clone();
    let fingerprint_repo = repo.clone();
    let before = tokio::task::spawn_blocking(move || {
        volley_inference::reid_openvino::fingerprint(&fingerprint_repo)
    })
    .await
    .map_err(|e| storage::internal(format!("ReID fingerprint task: {e}")))?
    .map_err(storage::bad_request)?;
    let cache_scope = native_reid_cache::CacheScope {
        project_id: req.project_id.clone(),
        video: req.video.clone(),
        replay_id: req.replay_id.clone(),
        replay_sha256: req.expected_source_sha256.clone(),
        model_fingerprint: before.clone(),
    };
    let expected_sha = req.expected_source_sha256.clone();
    // Legacy full-replay vectors may come from intermissions. Scoped requests
    // must decode a real frame within play, even if a legacy vector is cached.
    let transient = req.transient;
    let scope_requires_new_embedding = req.rally_run_id.is_some() || transient;
    let planned_state = state.clone();
    let (source_file, source_stamp, replay_file, replay_stamp, crops, evidence) =
        tokio::task::spawn_blocking(move || -> Result<_, ApiError> {
            let _guard = project_guard::write_guard(&planned_state, &req.project_id)?;
            let ctx = source_for_rallies(
                &planned_state,
                &req.project_id,
                &req.video,
                &req.replay_id,
                req.rally_run_id.as_deref(),
            )?;
            if ctx.source_sha != req.expected_source_sha256 {
                return Err(storage::conflict(
                    "CPU replay changed; refresh the native Track IDs",
                ));
            }
            let plan = preparation::plan_for(&planned_state, &req.project_id, &req.video)?;
            let file = plan
                .ready_path()
                .ok_or_else(|| storage::bad_request("Verified video is unavailable"))?;
            let video = volley_media::probe(file).map_err(storage::bad_request)?;
            let mut crops = Vec::new();
            for id in &req.track_ids {
                if !ctx.known.contains_number(*id) {
                    return Err(storage::bad_request(
                        "Track ID not found in this verified replay",
                    ));
                }
                let info = ctx.tracks.get(id).ok_or_else(|| {
                    storage::bad_request("Track ID lacks a recorded bounding box")
                })?;
                let crop = crop_box(info.best_box, video.width, video.height)
                    .map_err(storage::bad_request)?;
                crops.push((*id, info.best_frame, crop));
            }
            let stamp = inference_stream::source_stamp(file).map_err(storage::bad_request)?;
            let replay_dir = projects::project_root(&planned_state)
                .join(&req.project_id)
                .join("native_cpu_replays")
                .join(&req.replay_id);
            let replay_file = safe_file(&replay_dir, "tracked_frames.jsonl", MAX_EVIDENCE)
                .map_err(storage::bad_request)?;
            let stat = fs::metadata(&replay_file)
                .map_err(|e| storage::bad_request(format!("CPU replay disappeared: {e}")))?;
            let replay_stamp = (stat.len(), stat.modified().ok());
            Ok((
                file.to_path_buf(),
                stamp,
                replay_file,
                replay_stamp,
                crops,
                ctx.evidence,
            ))
        })
        .await
        .map_err(|e| storage::internal(format!("ReID source check task: {e}")))??;
    let cache_root = state.data_root.clone();
    let read_scope = cache_scope.clone();
    let cache =
        tokio::task::spawn_blocking(move || native_reid_cache::read(&cache_root, &read_scope))
            .await
            .map_err(|e| storage::internal(format!("ReID cache read task: {e}")))?
            .map_err(storage::bad_request)?;
    let cache_invalidated = cache.invalidated;
    let mut selected = BTreeMap::<u64, Vec<f32>>::new();
    for (id, _, _) in &crops {
        if !scope_requires_new_embedding {
            if let Some(vector) = cache.vectors.get(id) {
                selected.insert(*id, vector.clone());
            }
        }
    }
    let cached_count = selected.len();
    let _decode_slot = state
        .preview_limit
        .acquire()
        .await
        .map_err(|e| storage::internal(format!("ReID media slot: {e}")))?;
    let mut frames = Vec::with_capacity(crops.len());
    for &(track_id, best_frame, [x, y, w, h]) in &crops {
        if selected.contains_key(&track_id) {
            continue;
        }
        let secs = format!("{:.6}", best_frame as f64 / 30.0);
        let vf = format!(
            "crop={w}:{h}:{x}:{y},scale={}:{}:flags=bilinear,format=bgr24",
            volley_inference::reid_openvino::WIDTH,
            volley_inference::reid_openvino::HEIGHT
        );
        let mut command = tokio::process::Command::new("ffmpeg");
        command
            .args([
                "-nostdin",
                "-hide_banner",
                "-loglevel",
                "error",
                "-ss",
                &secs,
                "-i",
            ])
            .arg(&source_file)
            .args([
                "-vf",
                &vf,
                "-frames:v",
                "1",
                "-an",
                "-pix_fmt",
                "bgr24",
                "-f",
                "rawvideo",
                "pipe:1",
            ]);
        command.kill_on_drop(true);
        let result = tokio::time::timeout(std::time::Duration::from_secs(35), command.output())
            .await
            .map_err(|_| storage::bad_request("ReID video crop timed out"))?
            .map_err(|e| storage::bad_request(format!("FFmpeg ReID crop: {e}")))?;
        if !result.status.success()
            || result.stdout.len() != volley_inference::reid_openvino::PIXELS * 3
        {
            return Err(storage::bad_request(
                "Cannot decode an exact BGR Track ID crop",
            ));
        }
        let chw = volley_inference::reid_openvino::bgr24_to_nchw(&result.stdout)
            .map_err(storage::bad_request)?;
        frames.push((track_id, best_frame, chw));
    }
    let computed = if frames.is_empty() {
        Vec::<(u64, Vec<f32>)>::new()
    } else {
        let library = std::env::var("VOLLEY_OPENVINO_C_LIB").map_err(|_| {
            storage::bad_request(
                "Native OpenVINO C runtime is not configured: set VOLLEY_OPENVINO_C_LIB",
            )
        })?;
        tokio::task::spawn_blocking(move || -> Result<Vec<(u64, Vec<f32>)>, ApiError> {
            let mut encoder =
                volley_inference::reid_openvino::Encoder::load(&repo, Path::new(&library))
                    .map_err(storage::bad_request)?;
            let mut output = Vec::new();
            for (id, _, chw) in frames {
                let vector = encoder.infer(&chw).map_err(storage::bad_request)?;
                output.push((id, vector));
            }
            Ok(output)
        })
        .await
        .map_err(|e| storage::internal(format!("ReID native inference task: {e}")))??
    };
    let check_replay = replay_file.clone();
    let check_source = source_file.clone();
    let check_repo = state.repository_root.clone();
    let expected_before = before.clone();
    tokio::task::spawn_blocking(move || {
        let replay_stat = fs::metadata(&check_replay)
            .map_err(|_| storage::conflict("CPU replay disappeared during ReID inference"))?;
        if (replay_stat.len(), replay_stat.modified().ok()) != replay_stamp
            || inference_stream::source_stamp(&check_source).map_err(storage::bad_request)?
                != source_stamp
            || volley_inference::reid_openvino::fingerprint(&check_repo)
                .map_err(storage::bad_request)?
                != expected_before
        {
            return Err(storage::conflict(
                "Video or ReID weights changed during feature extraction",
            ));
        }
        Ok::<_, ApiError>(())
    })
    .await
    .map_err(|e| storage::internal(format!("ReID recheck task: {e}")))??;
    let computed_count = computed.len();
    let persisted = if state.writes_enabled && !transient && !computed.is_empty() {
        let writer = state.clone();
        let scope = cache_scope.clone();
        let vectors = computed.clone();
        let source_path = source_file.clone();
        let replay_path = replay_file.clone();
        tokio::task::spawn_blocking(move || -> Result<(), ApiError> {
            let _guard = project_guard::write_guard(&writer, &scope.project_id)?;
            let stat = fs::metadata(&replay_path)
                .map_err(|_| storage::conflict("ReID replay disappeared before cache commit"))?;
            if (stat.len(), stat.modified().ok()) != replay_stamp
                || inference_stream::source_stamp(&source_path).map_err(storage::bad_request)?
                    != source_stamp
                || volley_inference::reid_openvino::fingerprint(&writer.repository_root)
                    .map_err(storage::bad_request)?
                    != scope.model_fingerprint
            {
                return Err(storage::conflict(
                    "Native ReID source/model changed before cache commit",
                ));
            }
            native_reid_cache::merge(&writer.data_root, &scope, &vectors)
                .map_err(storage::bad_request)?;
            Ok(())
        })
        .await
        .map_err(|e| storage::internal(format!("Native ReID cache task: {e}")))??;
        true
    } else {
        false
    };
    for (id, vector) in computed {
        selected.insert(id, vector);
    }
    let generated = crops
        .iter()
        .map(|(id, frame, _)| -> Result<Value, ApiError> {
            let vector = selected.get(id).ok_or_else(|| {
                storage::internal("Verified ReID feature missing from source/cache")
            })?;
            Ok(json!({
                "track_id":id,
                "best_frame":frame,
                "dimensions":vector.len(),
                "vector":vector,
                "cache_hit":cache.vectors.contains_key(id),
                "confirmed_athlete_identity":false
            }))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let source_kind = evidence.source_kind();
    Ok(Json(json!({
        "schema":"volley-native-reid-features/1",
        "source_replay_sha256":expected_sha,
        "evidence_source":evidence,
        "model":volley_inference::reid_openvino::MODEL_NAME,
        "model_fingerprint":before,
        "preprocessing":volley_inference::reid_openvino::PREPROCESS,
        "source_kind":source_kind,
        "dimensions":volley_inference::reid_openvino::DIMENSIONS,
        "embedding_count":generated.len(),
        "cache_hits":cached_count,
        "computed_count":computed_count,
        "cache_invalidated":cache_invalidated,
        "cache_persisted":persisted,
        "features":generated,
        "similarity_type":"cosine_similarity_not_probability",
        "identities_confirmed":false,
        "automatic_reid_binding":false
    })))
}
