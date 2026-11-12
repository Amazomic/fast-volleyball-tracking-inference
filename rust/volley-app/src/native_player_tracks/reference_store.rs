use super::*;
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceRequest {
    pub(super) project_id: String,
    pub(super) player_id: String,
    pub(super) action: String, // read, enroll, undo
    #[serde(default)]
    pub(super) expected_revision: Option<String>,
    #[serde(default)]
    pub(super) video: Option<String>,
    #[serde(default)]
    pub(super) replay_id: Option<String>,
    #[serde(default)]
    pub(super) rally_run_id: Option<String>,
    #[serde(default)]
    pub(super) photo_index: usize,
    #[serde(default)]
    pub(super) track_id: Option<u64>,
    #[serde(default)]
    pub(super) expected_source_sha256: Option<String>,
    #[serde(default)]
    pub(super) reference_id: Option<String>,
}
pub(super) const MAX_REFERENCE_INDEX: u64 = 256 * 1024;
pub(super) const MAX_PHOTO_BYTES: usize = 3_000_000;
pub(super) const MAX_PLAYER_PHOTOS: usize = 64;

pub(super) fn checked_reference_folder(
    state: &AppState,
    player_id: &str,
    create: bool,
) -> Result<PathBuf, ApiError> {
    if !valid_player_id(player_id) {
        return Err(storage::bad_request("Invalid global athlete ID"));
    }
    if create && !state.writes_enabled {
        return Err(storage::conflict(
            "Saving athlete photos requires isolated project storage",
        ));
    }
    let base = state.data_root.join("players");
    let references = base.join("native_reference_photos");
    let target = references.join(player_id);
    for folder in [&base, &references, &target] {
        match fs::symlink_metadata(folder) {
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                return Err(storage::bad_request("Unsafe athlete photo directory"));
            }
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && create => {
                fs::create_dir(folder)
                    .map_err(|e| storage::internal(format!("Create reference directory: {e}")))?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(target.clone()),
            Err(e) => {
                return Err(storage::internal(format!("Check reference directory: {e}")));
            }
        }
    }
    Ok(target)
}
pub(super) fn check_reference_id(id: &str) -> bool {
    id.len() == 24
        && id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
pub(super) fn references_doc(raw: Option<&[u8]>, player_id: &str) -> Result<Value, ApiError> {
    let Some(bytes) = raw else {
        return Ok(json!({"schema":"volley-native-player-references/1",
            "player_id":player_id,"photos":[]}));
    };
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| storage::bad_request("Corrupted athlete photo index"))?;
    if value["schema"] != "volley-native-player-references/1"
        || value["player_id"] != player_id
        || value["photos"]
            .as_array()
            .is_none_or(|v| v.len() > MAX_PLAYER_PHOTOS)
    {
        return Err(storage::bad_request(
            "Invalid athlete reference photo schema",
        ));
    }
    let mut seen = BTreeSet::new();
    for photo in value["photos"].as_array().expect("validated photos array") {
        let id = photo["id"]
            .as_str()
            .ok_or_else(|| storage::bad_request("Photo ID missing"))?;
        if !check_reference_id(id)
            || !seen.insert(id.to_owned())
            || photo["filename"] != format!("ref-{id}.jpg")
            || photo["image_sha256"].as_str().is_none_or(|v| !valid_sha(v))
            || photo["source_replay_sha256"]
                .as_str()
                .is_none_or(|v| !valid_sha(v))
            || photo["manually_enrolled"] != true
            || photo["track_id"].as_u64().is_none_or(|n| n == 0)
        {
            return Err(storage::bad_request("Invalid saved reference photo"));
        }
    }
    Ok(value)
}
/// Photo metadata for global library cards, independent of project membership.
/// Never touches the video or enumerates unvalidated image paths.
pub(crate) fn player_photo_summary(
    state: &AppState,
    player_id: &str,
) -> Result<(usize, Option<String>), ApiError> {
    let folder = checked_reference_folder(state, player_id, false)?;
    let bytes =
        versioned_store::read_optional_bounded(&folder.join("index.json"), MAX_REFERENCE_INDEX)
            .map_err(storage::bad_request)?;
    let document = references_doc(bytes.as_deref(), player_id)?;
    let photos = document["photos"]
        .as_array()
        .expect("validated photo album");
    let cover = photos
        .last()
        .and_then(|photo| photo["id"].as_str())
        .map(str::to_owned);
    Ok((photos.len(), cover))
}
pub(super) fn reference_player(
    state: &AppState,
    project_id: &str,
    player_id: &str,
) -> Result<(), ApiError> {
    storage::project_id(project_id)?;
    if !valid_player_id(player_id) {
        return Err(storage::bad_request("Invalid athlete ID"));
    }
    let project = projects::existing_project(state, project_id)?;
    if !project["player_ids"]
        .as_array()
        .is_some_and(|rows| rows.iter().any(|r| r.as_str() == Some(player_id)))
    {
        return Err(storage::bad_request(
            "Athlete does not belong to selected project",
        ));
    }
    let (catalog, _) = player_catalog::load(state, false)?;
    if !catalog["players"]
        .as_array()
        .is_some_and(|rows| rows.iter().any(|r| r["id"] == player_id))
    {
        return Err(storage::conflict(
            "Athlete was removed from the global roster",
        ));
    }
    Ok(())
}
pub(super) fn verified_reference_source(
    state: &AppState,
    req: &ReferenceRequest,
) -> Result<(), ApiError> {
    if req.photo_index > 12 {
        return Err(storage::bad_request("Invalid Track ID photo index"));
    }
    let video = req
        .video
        .as_deref()
        .ok_or_else(|| storage::bad_request("Select a source video for enrollment"))?;
    let replay_id = req
        .replay_id
        .as_deref()
        .ok_or_else(|| storage::bad_request("Select a native replay"))?;
    let id = req
        .track_id
        .ok_or_else(|| storage::bad_request("Select a native Track ID"))?;
    let sha = req
        .expected_source_sha256
        .as_deref()
        .filter(|s| valid_sha(s))
        .ok_or_else(|| storage::bad_request("Supply verified CPU replay SHA-256"))?;
    let ctx = source_for_rallies(
        state,
        &req.project_id,
        video,
        replay_id,
        req.rally_run_id.as_deref(),
    )?;
    if ctx.source_sha != sha || !ctx.known.contains_number(id) || !ctx.tracks.contains_key(&id) {
        return Err(storage::conflict(
            "Source replay changed; repeat manual photo enrollment",
        ));
    }
    let bytes = existing(&ctx.sidecar).map_err(storage::bad_request)?;
    let document = validated(bytes.as_deref(), &ctx).map_err(storage::bad_request)?;
    if document["assignments"][id.to_string()].as_str() != Some(&req.player_id) {
        return Err(storage::conflict(
            "The selected Track ID must first be explicitly reviewed and linked to this athlete",
        ));
    }
    Ok(())
}
pub(super) fn reference_response(
    state: &AppState,
    req: &ReferenceRequest,
) -> Result<Value, ApiError> {
    reference_player(state, &req.project_id, &req.player_id)?;
    let folder = checked_reference_folder(state, &req.player_id, false)?;
    let raw =
        versioned_store::read_optional_bounded(&folder.join("index.json"), MAX_REFERENCE_INDEX)
            .map_err(storage::bad_request)?;
    let doc = references_doc(raw.as_deref(), &req.player_id)?;
    Ok(json!({
        "schema":"volley-native-player-references/1",
        "project_id":req.project_id,"player_id":req.player_id,
        "revision":revision(raw.as_deref()),
        "photos":doc["photos"],"count":doc["photos"].as_array().map_or(0,Vec::len),
        "writable":state.writes_enabled,
        "automatic_reid_binding":false
    }))
}
pub(super) fn edit_references(
    state: &AppState,
    req: &ReferenceRequest,
    jpeg: Option<&[u8]>,
) -> Result<Value, ApiError> {
    if !state.writes_enabled {
        return Err(storage::conflict(
            "Athlete photos require isolated project storage",
        ));
    }
    reference_player(state, &req.project_id, &req.player_id)?;
    let expected = req
        .expected_revision
        .as_deref()
        .filter(|s| versioned_store::valid_revision(s))
        .ok_or_else(|| {
            storage::bad_request("Read photo album and provide its expected_revision")
        })?;
    let _project_lock = if req.action == "enroll" {
        Some(project_guard::write_guard(state, &req.project_id)?)
    } else {
        None
    };
    if req.action == "enroll" {
        verified_reference_source(state, req)?;
    }
    let folder = checked_reference_folder(state, &req.player_id, true)?;
    let lockfile = folder.join(".references.lock");
    if fs::symlink_metadata(&lockfile)
        .ok()
        .is_some_and(|m| m.file_type().is_symlink())
    {
        return Err(storage::bad_request("Photo album lock is a symlink"));
    }
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lockfile)
        .map_err(|e| storage::internal(format!("Reference lock open: {e}")))?;
    lock.lock_exclusive()
        .map_err(|e| storage::internal(format!("Reference lock: {e}")))?;
    let index = folder.join("index.json");
    let bytes = versioned_store::read_optional_bounded(&index, MAX_REFERENCE_INDEX)
        .map_err(storage::bad_request)?;
    if versioned_store::expect_revision(bytes.as_deref(), expected).is_err() {
        return Err(storage::conflict(
            "Photo album changed; refresh its revision",
        ));
    }
    let mut doc = references_doc(bytes.as_deref(), &req.player_id)?;
    let items = doc["photos"].as_array_mut().expect("validated photo album");
    match req.action.as_str() {
        "enroll" => {
            if items.len() >= MAX_PLAYER_PHOTOS {
                return Err(storage::conflict(
                    "Athlete already has the maximum number of reference photos",
                ));
            }
            let jpeg = jpeg.ok_or_else(|| storage::bad_request("No verified player photo"))?;
            if jpeg.len() < 100
                || jpeg.len() > MAX_PHOTO_BYTES
                || !jpeg.starts_with(&[0xff, 0xd8])
                || !jpeg.ends_with(&[0xff, 0xd9])
            {
                return Err(storage::bad_request("Invalid JPEG reference photo"));
            }
            let sha = format!("{:x}", Sha256::digest(jpeg));
            let source = req
                .expected_source_sha256
                .as_ref()
                .expect("verified source");
            let tid = req.track_id.expect("verified Track ID");
            let mut digest = Sha256::new();
            digest.update(req.player_id.as_bytes());
            digest.update(req.project_id.as_bytes());
            digest.update(req.video.as_ref().expect("verified video").as_bytes());
            digest.update(req.replay_id.as_ref().expect("verified replay").as_bytes());
            digest.update(tid.to_le_bytes());
            digest.update(source.as_bytes());
            digest.update(sha.as_bytes());
            let id = format!("{:x}", digest.finalize())[..24].to_owned();
            if items.iter().any(|v| v["id"] == id) {
                return Err(storage::conflict(
                    "This exact photo is already in the athlete album",
                ));
            }
            let filename = format!("ref-{id}.jpg");
            let path = folder.join(&filename);
            let mut out = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|e| {
                    storage::conflict(format!(
                        "Reference photo already exists or cannot be created: {e}"
                    ))
                })?;
            let created = (|| -> std::io::Result<()> {
                out.write_all(jpeg)?;
                out.sync_all()
            })();
            drop(out);
            if let Err(e) = created {
                let _ = fs::remove_file(&path);
                return Err(storage::internal(format!("Write reference photo: {e}")));
            }
            items.push(json!({
                "id":id,"filename":filename,"image_sha256":sha,
                "project_id":req.project_id,"video":req.video,"replay_id":req.replay_id,
                "rally_run_id":req.rally_run_id,
                "photo_index":req.photo_index,
                "track_id":tid,"source_replay_sha256":source,
                "manually_enrolled":true,"saved_utc":chrono::Utc::now().to_rfc3339(),
            }));
            if let Err(e) =
                versioned_store::write_json_checked(&folder, &index, &doc, MAX_REFERENCE_INDEX)
            {
                let _ = fs::remove_file(path);
                return Err(storage::bad_request(e));
            }
        }
        "undo" => {
            let id = req
                .reference_id
                .as_deref()
                .filter(|s| check_reference_id(s))
                .ok_or_else(|| storage::bad_request("Supply reference_id to undo"))?;
            let before = items.len();
            items.retain(|v| v["id"].as_str() != Some(id));
            if items.len() == before {
                return Err(storage::conflict("Photo reference was already removed"));
            }
            versioned_store::write_json_checked(&folder, &index, &doc, MAX_REFERENCE_INDEX)
                .map_err(storage::bad_request)?;
            let path = folder.join(format!("ref-{id}.jpg"));
            if let Ok(meta) = fs::symlink_metadata(&path) {
                if meta.file_type().is_symlink() || !meta.is_file() {
                    return Err(storage::bad_request("Unsafe leftover reference photo"));
                }
                fs::remove_file(&path)
                    .map_err(|e| storage::internal(format!("Unlink old reference photo: {e}")))?;
            }
        }
        _ => return Err(storage::bad_request("Unknown photo reference action")),
    }
    let new = versioned_store::read_optional_bounded(&index, MAX_REFERENCE_INDEX)
        .map_err(storage::bad_request)?;
    let verified = references_doc(new.as_deref(), &req.player_id)?;
    Ok(json!({
        "schema":"volley-native-player-references/1",
        "project_id":req.project_id,"player_id":req.player_id,
        "revision":revision(new.as_deref()),
        "photos":verified["photos"],"count":verified["photos"].as_array().map_or(0,Vec::len),
        "writable":true,"automatic_reid_binding":false
    }))
}
pub async fn references(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<ReferenceRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    if !matches!(req.action.as_str(), "read" | "enroll" | "undo") {
        return Err(storage::bad_request(
            "Expected photo action read, enroll or undo",
        ));
    }
    let _slot = state
        .identity_limit
        .acquire()
        .await
        .map_err(|e| storage::internal(format!("Photo reference gate: {e}")))?;
    if req.action == "read" {
        let snapshot = state.clone();
        return Ok(Json(
            tokio::task::spawn_blocking(move || reference_response(&snapshot, &req))
                .await
                .map_err(|e| storage::internal(format!("Reference read task: {e}")))??,
        ));
    }
    if req.action == "enroll" {
        if !state.writes_enabled {
            return Err(storage::conflict(
                "Saving reference photos requires isolated project storage",
            ));
        }
        let snapshot = state.clone();
        let source_req = req.clone();
        tokio::task::spawn_blocking(move || {
            let _lock = project_guard::write_guard(&snapshot, &source_req.project_id)?;
            reference_player(&snapshot, &source_req.project_id, &source_req.player_id)?;
            verified_reference_source(&snapshot, &source_req)
        })
        .await
        .map_err(|e| storage::internal(format!("Photo source verification: {e}")))??;
        let photo = photo(
            State(state.clone()),
            Query(PhotoQuery {
                project_id: req.project_id.clone(),
                video: req.video.clone().expect("verified photo video"),
                replay_id: req.replay_id.clone().expect("verified photo replay"),
                track_id: req.track_id.expect("verified photo ID"),
                rally_run_id: req.rally_run_id.clone(),
                photo_index: req.photo_index,
            }),
        )
        .await?;
        let bytes = axum::body::to_bytes(photo.into_body(), MAX_PHOTO_BYTES)
            .await
            .map_err(|_| storage::bad_request("Photo is larger than allowed"))?;
        let snapshot = state.clone();
        let reply =
            tokio::task::spawn_blocking(move || edit_references(&snapshot, &req, Some(&bytes)))
                .await
                .map_err(|e| storage::internal(format!("Photo enrollment task: {e}")))??;
        return Ok(Json(reply));
    }
    let snapshot = state.clone();
    let reply = tokio::task::spawn_blocking(move || edit_references(&snapshot, &req, None))
        .await
        .map_err(|e| storage::internal(format!("Photo removal task: {e}")))??;
    Ok(Json(reply))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferencePhotoQuery {
    pub(super) project_id: Option<String>,
    pub(super) player_id: String,
    pub(super) reference_id: String,
}
pub async fn reference_photo(
    State(state): State<Arc<AppState>>,
    Query(req): Query<ReferencePhotoQuery>,
) -> Result<axum::response::Response, ApiError> {
    let snapshot = state.clone();
    let content = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, ApiError> {
        if let Some(project_id) = req.project_id.as_deref() {
            reference_player(&snapshot, project_id, &req.player_id)?;
        } else {
            let (catalog, _) = player_catalog::load(&snapshot, false)?;
            if !catalog["players"].as_array().is_some_and(|players| {
                players
                    .iter()
                    .any(|player| player["id"] == req.player_id && player["deleted"] != true)
            }) {
                return Err(storage::not_found("Athlete not found in active library"));
            }
        }
        if !check_reference_id(&req.reference_id) {
            return Err(storage::bad_request("Invalid reference photo ID"));
        }
        let folder = checked_reference_folder(&snapshot, &req.player_id, false)?;
        let raw =
            versioned_store::read_optional_bounded(&folder.join("index.json"), MAX_REFERENCE_INDEX)
                .map_err(storage::bad_request)?;
        let doc = references_doc(raw.as_deref(), &req.player_id)?;
        let item = doc["photos"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|v| v["id"].as_str() == Some(&req.reference_id))
            .ok_or_else(|| storage::not_found("Reference photo was removed"))?;
        let path = folder.join(format!("ref-{}.jpg", req.reference_id));
        let bytes = versioned_store::read_optional_bounded(&path, MAX_PHOTO_BYTES as u64)
            .map_err(storage::bad_request)?
            .ok_or_else(|| storage::conflict("Reference JPEG vanished"))?;
        if bytes.len() < 100
            || bytes.len() > MAX_PHOTO_BYTES
            || !bytes.starts_with(&[0xff, 0xd8])
            || !bytes.ends_with(&[0xff, 0xd9])
            || format!("{:x}", Sha256::digest(&bytes))
                != item["image_sha256"].as_str().unwrap_or("")
        {
            return Err(storage::conflict("Reference photo bytes are corrupt"));
        }
        Ok(bytes)
    })
    .await
    .map_err(|e| storage::internal(format!("Read photo task: {e}")))??;
    use axum::{body::Body, http::header};
    axum::response::Response::builder()
        .header(header::CONTENT_TYPE, "image/jpeg")
        .header(header::CACHE_CONTROL, "private, no-store")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(Body::from(content))
        .map_err(|e| storage::internal(format!("Reference photo response: {e}")))
}

pub async fn library_photo(
    state: State<Arc<AppState>>,
    Query(req): Query<LibraryPhotoQuery>,
) -> Result<axum::response::Response, ApiError> {
    reference_photo(
        state,
        Query(ReferencePhotoQuery {
            project_id: None,
            player_id: req.player_id,
            reference_id: req.reference_id,
        }),
    )
    .await
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryPhotoQuery {
    player_id: String,
    reference_id: String,
}
