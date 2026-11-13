use super::*;
use tokio::io::AsyncWriteExt;

struct PhotoInput {
    player_id: String,
    project_id: String,
    video: String,
    replay_id: String,
    track_id: u64,
    bytes: Vec<u8>,
}

pub(super) struct Snapshot {
    ctx: Inputs,
    identity: Value,
    pub(super) identity_revision: String,
    pub(super) albums: Vec<(String, String)>,
    photos: Vec<PhotoInput>,
    vectors: BTreeMap<u64, Vec<f32>>,
    pub(super) model: String,
}

pub(super) fn snapshot(state: &AppState, req: &ReidReviewRequest) -> Result<Snapshot, ApiError> {
    let ctx = source_for_rallies(
        state,
        &req.project_id,
        &req.video,
        &req.replay_id,
        req.rally_run_id.as_deref(),
    )?;
    if ctx.source_sha != req.expected_source_sha256 {
        return Err(storage::conflict(
            "CPU replay changed; refresh player gallery",
        ));
    }
    let raw = existing(&ctx.sidecar).map_err(storage::bad_request)?;
    let identity = validated(raw.as_deref(), &ctx).map_err(storage::bad_request)?;
    let model = volley_inference::reid_openvino::fingerprint(&state.repository_root)
        .map_err(storage::bad_request)?;
    let assigned = identity["assignments"]
        .as_object()
        .ok_or_else(|| storage::bad_request("Invalid assignments"))?;
    let excluded = identity["excluded_ids"]
        .as_array()
        .ok_or_else(|| storage::bad_request("Invalid excluded IDs"))?
        .iter()
        .filter_map(Value::as_u64)
        .collect::<BTreeSet<_>>();
    let rebuild = req.action == "auto_assign";
    let eligible = ctx
        .tracks
        .keys()
        .filter(|id| (rebuild || !assigned.contains_key(&id.to_string())) && (rebuild || !excluded.contains(id)))
        .collect::<Vec<_>>();
    if eligible.len() > 1024 {
        return Err(storage::bad_request(
            "Comparison is limited to 1024 Track IDs",
        ));
    }
    let mut albums = Vec::new();
    let mut photos = Vec::new();
    let mut total_bytes = 0usize;
    for player_id in ctx.roster.keys() {
        let folder = checked_reference_folder(state, player_id, false)?;
        let raw =
            versioned_store::read_optional_bounded(&folder.join("index.json"), MAX_REFERENCE_INDEX)
                .map_err(storage::bad_request)?;
        let doc = references_doc(raw.as_deref(), player_id)?;
        albums.push((player_id.clone(), revision(raw.as_deref())));
        let mut seen = BTreeSet::new();
        for row in doc["photos"].as_array().into_iter().flatten() {
            let sha = row["image_sha256"]
                .as_str()
                .ok_or_else(|| storage::bad_request("Invalid photo checksum"))?;
            if !seen.insert(sha.to_owned()) {
                continue;
            }
            if photos.len() >= 256 {
                return Err(storage::bad_request(
                    "Too many reference photos for one comparison (maximum 256)",
                ));
            }
            let id = row["id"]
                .as_str()
                .ok_or_else(|| storage::bad_request("Invalid reference ID"))?;
            let bytes = versioned_store::read_optional_bounded(
                &folder.join(format!("ref-{id}.jpg")),
                MAX_PHOTO_BYTES as u64,
            )
            .map_err(storage::bad_request)?
            .ok_or_else(|| storage::conflict("Reference photo missing"))?;
            total_bytes = total_bytes.saturating_add(bytes.len());
            if total_bytes > 64 * 1024 * 1024 {
                return Err(storage::bad_request(
                    "Reference photos exceed 64 MiB for one comparison",
                ));
            }
            if bytes.len() < 100
                || !bytes.starts_with(&[0xff, 0xd8])
                || !bytes.ends_with(&[0xff, 0xd9])
                || format!("{:x}", Sha256::digest(&bytes)) != sha
            {
                return Err(storage::conflict("Reference photo checksum mismatch"));
            }
            let field = |name: &str| {
                row[name]
                    .as_str()
                    .filter(|v| !v.is_empty())
                    .map(str::to_owned)
                    .ok_or_else(|| storage::bad_request("Reference photo provenance missing"))
            };
            photos.push(PhotoInput {
                player_id: player_id.clone(),
                project_id: field("project_id")?,
                video: field("video")?,
                replay_id: field("replay_id")?,
                track_id: row["track_id"]
                    .as_u64()
                    .ok_or_else(|| storage::bad_request("Invalid reference track"))?,
                bytes,
            });
        }
    }
    // An empty album means there are no identity references yet, not a CPU
    // postprocessing failure. Preserve the analysis and report zero matches.
    Ok(Snapshot {
        ctx,
        identity,
        identity_revision: revision(raw.as_deref()),
        albums,
        photos,
        vectors: BTreeMap::new(),
        model,
    })
}

async fn decode(bytes: &[u8]) -> Result<Vec<f32>, ApiError> {
    let mut command = tokio::process::Command::new("ffmpeg");
    command
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            "pipe:0",
            "-vf",
            "scale=128:256:flags=bilinear,format=bgr24",
            "-frames:v",
            "1",
            "-an",
            "-pix_fmt",
            "bgr24",
            "-f",
            "rawvideo",
            "pipe:1",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|e| storage::bad_request(format!("FFmpeg photo decoder: {e}")))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| storage::internal("Photo decoder input unavailable"))?;
    let data = bytes.to_vec();
    let writer = tokio::spawn(async move { stdin.write_all(&data).await });
    let result = tokio::time::timeout(std::time::Duration::from_secs(35), child.wait_with_output())
        .await
        .map_err(|_| storage::bad_request("Photo decoding timed out"))?
        .map_err(|e| storage::bad_request(format!("Photo decoder failed: {e}")))?;
    writer
        .await
        .map_err(|e| storage::internal(format!("Photo decoder input: {e}")))?
        .map_err(|e| storage::bad_request(format!("Photo decoder input: {e}")))?;
    if !result.status.success()
        || result.stdout.len() != volley_inference::reid_openvino::PIXELS * 3
    {
        return Err(storage::bad_request("Cannot decode saved reference photo"));
    }
    volley_inference::reid_openvino::bgr24_to_nchw(&result.stdout).map_err(storage::bad_request)
}

pub(super) async fn compare(
    state: Arc<AppState>,
    headers: HeaderMap,
    req: ReidReviewRequest,
) -> Result<Value, ApiError> {
    if req.rally_run_id.as_deref().is_some_and(|id| !opaque_id(id)) {
        return Err(storage::bad_request(
            "Invalid rally run for photo comparison",
        ));
    }
    let cloned = state.clone();
    let query = req.clone();
    let mut snap = tokio::task::spawn_blocking(move || snapshot(&cloned, &query))
        .await
        .map_err(|e| storage::internal(format!("Photo comparison source: {e}")))??;
    if snap.photos.is_empty() {
        return Ok(json!({
            "schema":"volley-native-player-photo-comparison/1",
            "project_id":snap.ctx.project_id,"replay_id":snap.ctx.replay_id,
            "source_replay_sha256":snap.ctx.source_sha,
            "player_matches":{},"photo_count":0,
            "identity_revision":snap.identity_revision,
            "album_revisions":snap.albums,
            "model_fingerprint":snap.model,
            "auto_assign_enabled":false,"confirmed":false
        }));
    }
    let assigned = snap.identity["assignments"]
        .as_object()
        .ok_or_else(|| storage::bad_request("Invalid assignments"))?;
    let excluded = snap.identity["excluded_ids"]
        .as_array()
        .ok_or_else(|| storage::bad_request("Invalid excluded IDs"))?
        .iter()
        .filter_map(Value::as_u64)
        .collect::<BTreeSet<_>>();
    let ids = snap
        .ctx
        .tracks
        .keys()
        .filter(|id| (req.action == "auto_assign" || !assigned.contains_key(&id.to_string()))
            && (req.action == "auto_assign" || !excluded.contains(id)))
        .copied()
        .collect::<Vec<_>>();
    if ids.is_empty() {
        return Ok(
            json!({"schema":"volley-native-player-photo-comparison/1","project_id":snap.ctx.project_id,"replay_id":snap.ctx.replay_id,"source_replay_sha256":snap.ctx.source_sha,"player_matches":{},"photo_count":0,"identity_revision":snap.identity_revision,"album_revisions":snap.albums,"model_fingerprint":snap.model,"auto_assign_enabled":false,"confirmed":false}),
        );
    }
    for chunk in ids.chunks(8) {
        let Json(result) = embeddings(
            State(state.clone()),
            headers.clone(),
            Json(EmbeddingRequest {
                project_id: req.project_id.clone(),
                video: req.video.clone(),
                replay_id: req.replay_id.clone(),
                rally_run_id: req.rally_run_id.clone(),
                expected_source_sha256: req.expected_source_sha256.clone(),
                track_ids: chunk.to_vec(),
                transient: true,
            }),
        )
        .await?;
        if result["model_fingerprint"] != snap.model {
            return Err(storage::conflict("ReID model changed during comparison"));
        }
        for row in result["features"]
            .as_array()
            .ok_or_else(|| storage::internal("Track features missing"))?
        {
            let id = row["track_id"]
                .as_u64()
                .ok_or_else(|| storage::internal("Track feature ID missing"))?;
            let vector: Vec<f32> = serde_json::from_value(row["vector"].clone())
                .map_err(|_| storage::internal("Invalid track feature"))?;
            if !chunk.contains(&id) || snap.vectors.insert(id, vector).is_some() {
                return Err(storage::internal("Unexpected track feature"));
            }
        }
        if chunk.iter().any(|id| !snap.vectors.contains_key(id)) {
            return Err(storage::internal("Incomplete track features"));
        }
    }
    let _permit = state
        .reid_limit
        .acquire()
        .await
        .map_err(|e| storage::internal(format!("ReID gate: {e}")))?;
    let _preview = state
        .preview_limit
        .acquire()
        .await
        .map_err(|e| storage::internal(format!("Photo decoder gate: {e}")))?;
    let mut inputs = Vec::new();
    for photo in &snap.photos {
        inputs.push(decode(&photo.bytes).await?);
    }
    let library = std::env::var("VOLLEY_OPENVINO_C_LIB")
        .map_err(|_| storage::bad_request("Native OpenVINO C runtime is not configured"))?;
    let repo = state.repository_root.clone();
    let vectors = tokio::task::spawn_blocking(move || -> Result<Vec<Vec<f32>>, ApiError> {
        let mut encoder =
            volley_inference::reid_openvino::Encoder::load(&repo, Path::new(&library))
                .map_err(storage::bad_request)?;
        inputs
            .iter()
            .map(|input| encoder.infer(input).map_err(storage::bad_request))
            .collect()
    })
    .await
    .map_err(|e| storage::internal(format!("Photo ReID worker: {e}")))??;
    let check_state = state.clone();
    let check_req = req.clone();
    let current = tokio::task::spawn_blocking(move || snapshot(&check_state, &check_req))
        .await
        .map_err(|e| storage::internal(format!("Photo comparison recheck: {e}")))??;
    if current.identity_revision != snap.identity_revision
        || current.albums != snap.albums
        || current.model != snap.model
        || current.ctx.roster != snap.ctx.roster
        || current.ctx.tracks != snap.ctx.tracks
    {
        return Err(storage::conflict(
            "Player assignments, photo album or ReID model changed during comparison",
        ));
    }
    let refs = snap
        .photos
        .drain(..)
        .zip(vectors)
        .map(|(p, vector)| PhotoReference {
            player_id: p.player_id,
            project_id: p.project_id,
            video: p.video,
            replay_id: p.replay_id,
            track_id: p.track_id,
            vector,
        })
        .collect::<Vec<_>>();
    // CPU postprocessing is a complete rebuild: old manual/automatic links
    // must never suppress photo candidates from the new identity pass.
    let mut match_identity = snap.identity.clone();
    if req.action == "auto_assign" {
        match_identity["assignments"] = json!({});
        match_identity["excluded_ids"] = json!([]);
    }
    let matches = photo_match_candidates(&snap.ctx, &match_identity, &snap.vectors, &refs)
        .map_err(storage::bad_request)?;
    Ok(
        json!({"schema":"volley-native-player-photo-comparison/1", "project_id":snap.ctx.project_id,
        "replay_id":snap.ctx.replay_id,"source_replay_sha256":snap.ctx.source_sha,
        "player_matches":matches,"photo_count":refs.len(),"identity_revision":snap.identity_revision,"album_revisions":snap.albums,"model_fingerprint":snap.model,"auto_assign_enabled":false,"confirmed":false}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn saved_jpeg_decodes_to_openvino_input() {
        let image = tokio::process::Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=red:s=16x32",
                "-frames:v",
                "1",
                "-c:v",
                "mjpeg",
                "-f",
                "image2pipe",
                "pipe:1",
            ])
            .output()
            .await
            .unwrap();
        assert!(image.status.success());
        let input = decode(&image.stdout).await.unwrap();
        assert_eq!(input.len(), volley_inference::reid_openvino::PIXELS * 3);
        assert!(input[2 * volley_inference::reid_openvino::PIXELS] > input[0]);
    }
}
