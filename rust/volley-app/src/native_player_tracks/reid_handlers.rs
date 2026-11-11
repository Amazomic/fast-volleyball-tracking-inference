use super::*;
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeReidQuery {
    pub(super) project_id: String,
    pub(super) video: String,
    pub(super) replay_id: String,
    pub(super) expected_source_sha256: String,
    #[serde(default = "default_reid_threshold")]
    pub(super) threshold_percent: u8,
}
pub(super) fn default_reid_threshold() -> u8 {
    85
}

pub(super) fn native_reid_report(
    ctx: &Inputs,
    doc: &Value,
    cached: &BTreeMap<u64, Vec<f32>>,
    threshold: u8,
) -> Result<Value, String> {
    let mut manual = BTreeMap::<u64, String>::new();
    for (id, player) in doc["assignments"]
        .as_object()
        .ok_or("Invalid assignment registry")?
    {
        manual.insert(
            id.parse::<u64>().map_err(|_| "Invalid assigned track ID")?,
            player
                .as_str()
                .ok_or("Invalid assigned athlete")?
                .to_owned(),
        );
    }
    let excluded = doc["excluded_ids"]
        .as_array()
        .ok_or("Invalid excluded tracks")?
        .iter()
        .map(|v| v.as_u64().ok_or("Invalid excluded ID"))
        .collect::<Result<BTreeSet<_>, _>>()?;
    let reviewed = identity_review::resolve_reviewed(
        ctx.known.ids(),
        &ctx.roster,
        &manual
            .into_iter()
            .map(|(track_id, owner)| ReviewedLink {
                track_id,
                owner,
                reviewed: true,
            })
            .collect::<Vec<_>>(),
        &excluded,
    );
    if !reviewed.conflicts.is_empty() {
        return Err("Conflicting manual ownership blocks ReID suggestions".into());
    }
    let stats = ctx
        .tracks
        .iter()
        .map(|(&id, track)| TrackInfo {
            id: i64::try_from(id).expect("Replay Track ID bounded by verified manifest"),
            first_frame: i64::try_from(track.first).expect("Replay frame bounded by manifest"),
            last_frame: i64::try_from(track.last).expect("Replay frame bounded by manifest"),
            detected_frames: u32::try_from(track.observed).unwrap_or(u32::MAX),
            start_center: [
                (track.best_box[0] + track.best_box[2]) * 0.5,
                (track.best_box[1] + track.best_box[3]) * 0.5,
            ],
            end_center: [
                (track.best_box[0] + track.best_box[2]) * 0.5,
                (track.best_box[1] + track.best_box[3]) * 0.5,
            ],
            fps: 30.0,
        })
        .collect::<Vec<_>>();
    let features = cached
        .iter()
        .filter(|(id, _)| ctx.known.contains_number(**id))
        .map(|(&id, vec)| {
            Ok(Feature {
                id: i64::try_from(id).map_err(|_| "Track ID out of ReID range")?,
                vector: vec.clone(),
            })
        })
        .collect::<Result<Vec<_>, &str>>()?;
    let owners = reviewed
        .owners
        .into_iter()
        .map(|(id, player)| (id as i64, player))
        .collect::<std::collections::HashMap<_, _>>();
    let excluded = reviewed
        .excluded
        .into_iter()
        .map(|id| id as i64)
        .collect::<std::collections::HashSet<_>>();
    let report = reid::build_groups(
        &features,
        &stats,
        &owners,
        &excluded,
        FeatureMode::OpenVino,
        threshold,
    )?;
    Ok(json!(report))
}
pub async fn reid_preview(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<NativeReidQuery>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    if !(55..=99).contains(&req.threshold_percent) {
        return Err(storage::bad_request("ReID threshold must be 55..99"));
    }
    let _permit = state
        .reid_limit
        .acquire()
        .await
        .map_err(|e| storage::internal(format!("Native ReID gate: {e}")))?;
    let state = state.clone();
    let response = tokio::task::spawn_blocking(move || -> Result<Value, ApiError> {
        let _guard = project_guard::write_guard(&state, &req.project_id)?;
        let ctx = source(&state, &req.project_id, &req.video, &req.replay_id)?;
        if ctx.source_sha != req.expected_source_sha256 {
            return Err(storage::conflict("Selected ReID replay SHA changed"));
        }
        let before = volley_inference::reid_openvino::fingerprint(&state.repository_root)
            .map_err(storage::bad_request)?;
        let scope = native_reid_cache::CacheScope {
            project_id: req.project_id.clone(),
            video: req.video.clone(),
            replay_id: req.replay_id.clone(),
            replay_sha256: ctx.source_sha.clone(),
            model_fingerprint: before.clone(),
        };
        let cache = native_reid_cache::read(&state.data_root, &scope)
            .map_err(storage::bad_request)?;
        if cache.invalidated {
            return Err(storage::conflict("ReID embeddings are stale; regenerate from current model"));
        }
        if cache.vectors.is_empty() {
            return Err(storage::bad_request("No native embeddings cached; extract Track ID features first"));
        }
        let raw = existing(&ctx.sidecar).map_err(storage::bad_request)?;
        let identity = validated(raw.as_deref(), &ctx).map_err(storage::bad_request)?;
        let report = native_reid_report(&ctx, &identity, &cache.vectors, req.threshold_percent)
            .map_err(storage::bad_request)?;
        if volley_inference::reid_openvino::fingerprint(&state.repository_root)
            .map_err(storage::bad_request)? != before {
            return Err(storage::conflict("ReID model changed while comparing cached features"));
        }
        Ok(json!({
            "schema":"volley-native-reid-groups/1",
            "source_kind":ctx.evidence.source_kind(),
            "project_id":ctx.project_id,
            "video":ctx.video,
            "replay_id":ctx.replay_id,
            "source_replay_sha256":ctx.source_sha,
            "model_fingerprint":before,
            "method":volley_inference::reid_openvino::MODEL_NAME,
            "reviewed_identity_revision":revision(raw.as_deref()),
            "embedding":report,
            "auto_assign_enabled":false,
            "confirmed":false,
            "warning":"Cosine similarity is not identity probability. ReID groups are proposals only."
        }))
    }).await.map_err(|e| storage::internal(format!("Native ReID worker: {e}")))??;
    Ok(Json(response))
}
pub(super) const MAX_REID_REVIEW_BYTES: u64 = 256 * 1024;
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReidReviewRequest {
    pub(super) project_id: String,
    pub(super) video: String,
    pub(super) replay_id: String,
    pub(super) expected_source_sha256: String,
    pub(super) action: String,
    #[serde(default)]
    pub(super) track_ids: Vec<u64>,
    #[serde(default)]
    pub(super) expected_revision: Option<String>,
    #[serde(default = "default_reid_threshold")]
    pub(super) threshold_percent: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReidReviewDoc {
    pub(super) schema: String,
    pub(super) project_id: String,
    pub(super) video: String,
    pub(super) replay_id: String,
    pub(super) replay_sha256: String,
    pub(super) model_fingerprint: String,
    pub(super) groups: Vec<Vec<u64>>,
    pub(super) suppressed_ids: Vec<u64>,
}
impl ReidReviewDoc {
    pub(super) fn new(ctx: &Inputs, fingerprint: &str) -> Self {
        Self {
            schema: "volley-native-reid-group-review/1".into(),
            project_id: ctx.project_id.clone(),
            video: ctx.video.clone(),
            replay_id: ctx.replay_id.clone(),
            replay_sha256: ctx.source_sha.clone(),
            model_fingerprint: fingerprint.into(),
            groups: Vec::new(),
            suppressed_ids: Vec::new(),
        }
    }
}
pub(super) fn reviewed_group_is_safe(ctx: &Inputs, identities: &Value, ids: &[u64]) -> bool {
    let owners = identities["assignments"].as_object();
    let Some(owners) = owners else { return false };
    let excluded = identities["excluded_ids"].as_array();
    let Some(excluded) = excluded else {
        return false;
    };
    let excluded = excluded
        .iter()
        .filter_map(Value::as_u64)
        .collect::<BTreeSet<_>>();
    let mut members = Vec::with_capacity(ids.len());
    for id in ids {
        let Some(row) = ctx.tracks.get(id) else {
            return false;
        };
        if excluded.contains(id) {
            return false;
        }
        members.push(reid::ManualGroupMember {
            track_id: match i64::try_from(*id) {
                Ok(id) => id,
                Err(_) => return false,
            },
            first_frame: row.first as i64,
            last_frame: row.last as i64,
            confirmed_owner: owners.get(&id.to_string()).and_then(Value::as_str),
        });
    }
    reid::valid_manual_group(&members)
}
pub(super) fn checked_review(
    doc: &ReidReviewDoc,
    ctx: &Inputs,
    fingerprint: &str,
    identities: &Value,
) -> Result<(), String> {
    if doc.schema != "volley-native-reid-group-review/1"
        || doc.project_id != ctx.project_id
        || doc.video != ctx.video
        || doc.replay_id != ctx.replay_id
        || doc.replay_sha256 != ctx.source_sha
        || doc.model_fingerprint != fingerprint
    {
        return Err("Saved ReID manual review belongs to another replay/model".into());
    }
    if doc.groups.len() > 200 || doc.suppressed_ids.len() > MAX_TRACKS {
        return Err("Too many reviewed ReID groups or suppressed IDs".into());
    }
    let mut used = BTreeSet::new();
    for group in &doc.groups {
        if !reviewed_group_is_safe(ctx, identities, group)
            || group.iter().any(|id| !used.insert(*id))
        {
            return Err("Saved ReID group has unsafe, stale or duplicated Track IDs".into());
        }
    }
    let mut suppressed = BTreeSet::new();
    for id in &doc.suppressed_ids {
        if !ctx.known.contains_number(*id) || used.contains(id) || !suppressed.insert(*id) {
            return Err("Invalid saved manually split ReID Track IDs".into());
        }
    }
    Ok(())
}
pub(super) fn modify_review(
    doc: &mut ReidReviewDoc,
    ctx: &Inputs,
    identities: &Value,
    action: &str,
    ids: &[u64],
) -> Result<(), String> {
    if ids.is_empty() || ids.len() > 200 || (action == "merge" && ids.len() < 2) {
        return Err("Choose 1–200 Track IDs (at least 2 to merge)".into());
    }
    let chosen = ids.iter().copied().collect::<BTreeSet<_>>();
    if chosen.len() != ids.len() || ids.iter().any(|id| !ctx.known.contains_number(*id)) {
        return Err("Duplicate or unknown Track ID in manual ReID review".into());
    }
    if action == "merge" && !reviewed_group_is_safe(ctx, identities, ids) {
        return Err("Cannot merge overlapping, excluded or differently confirmed players".into());
    }
    if action != "merge" && action != "split" {
        return Err("Unsupported ReID review action".into());
    }
    let mut updated = doc.clone();
    if action == "split" && chosen.iter().all(|id| updated.suppressed_ids.contains(id)) {
        return Err("Selected IDs were already manually split".into());
    }
    for group in &mut updated.groups {
        group.retain(|id| !chosen.contains(id));
    }
    updated.groups.retain(|group| group.len() >= 2);
    if action == "merge" {
        updated.groups.push(chosen.iter().copied().collect());
        updated.suppressed_ids.retain(|id| !chosen.contains(id));
    } else {
        let mut suppressed = updated
            .suppressed_ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        suppressed.extend(chosen);
        updated.suppressed_ids = suppressed.into_iter().collect();
    }
    checked_review(&updated, ctx, &doc.model_fingerprint, identities)?;
    *doc = updated;
    Ok(())
}
pub(super) fn review_path(state: &AppState, ctx: &Inputs) -> Result<PathBuf, ApiError> {
    let project = projects::project_root(state).join(&ctx.project_id);
    safe_folder(&project).map_err(storage::bad_request)?;
    let folder = project.join("native_reid_reviews");
    if let Ok(meta) = fs::symlink_metadata(&folder) {
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(storage::bad_request("Unsafe manual ReID review folder"));
        }
    }
    Ok(folder.join(format!("{}.json", ctx.replay_id)))
}
/// Rank only against explicitly reviewed athlete Track IDs from the same
/// SHA-bound CPU replay. Cosine is a visual similarity, NOT a probability of
/// identity. Saved photo references have no verified embedding in this cache.
pub(super) fn player_match_candidates(
    ctx: &Inputs,
    identity: &Value,
    vectors: &BTreeMap<u64, Vec<f32>>,
) -> Result<Value, String> {
    let assignments = identity["assignments"]
        .as_object()
        .ok_or("Invalid reviewed athlete assignments")?;
    let excluded = identity["excluded_ids"]
        .as_array()
        .ok_or("Invalid excluded IDs")?
        .iter()
        .filter_map(Value::as_u64)
        .collect::<BTreeSet<_>>();
    let anchors = assignments
        .iter()
        .filter_map(|(id, owner)| {
            let id = id.parse::<u64>().ok()?;
            let owner = owner.as_str()?;
            let vector = vectors.get(&id)?;
            let source = ctx.tracks.get(&id)?;
            (!excluded.contains(&id) && ctx.roster.contains_key(owner))
                .then_some((id, owner, source, vector))
        })
        .collect::<Vec<_>>();
    let mut matches = serde_json::Map::new();
    for (&track_id, vector) in vectors {
        if assignments.contains_key(&track_id.to_string()) || excluded.contains(&track_id) {
            continue;
        }
        let Some(candidate) = ctx.tracks.get(&track_id) else {
            continue;
        };
        let mut nearest = BTreeMap::<String, (f64, u64)>::new();
        for &(anchor_id, player_id, reference, other) in &anchors {
            if candidate.first <= reference.last && reference.first <= candidate.last
                || other.len() != vector.len()
            {
                continue;
            }
            let cosine = vector
                .iter()
                .zip(other)
                .map(|(a, b)| f64::from(*a) * f64::from(*b))
                .sum::<f64>();
            if !cosine.is_finite() || cosine <= 0.0 {
                continue;
            }
            let similarity = cosine.min(1.0);
            let entry = nearest
                .entry(player_id.to_owned())
                .or_insert((similarity, anchor_id));
            if similarity > entry.0 {
                *entry = (similarity, anchor_id);
            }
        }
        let mut ranked = nearest.into_iter().collect::<Vec<_>>();
        ranked.sort_by(|a, b| b.1 .0.total_cmp(&a.1 .0).then_with(|| a.0.cmp(&b.0)));
        if ranked.is_empty() {
            continue;
        }
        let rows = ranked
            .into_iter()
            .take(3)
            .map(|(player_id, (score, reference_id))| {
                json!({
                    "player_id":player_id,
                    "name":ctx.roster[&player_id],
                    "cosine_similarity":(score * 10_000.0).round() / 10_000.0,
                    "reference_track_id":reference_id
                })
            })
            .collect::<Vec<_>>();
        matches.insert(track_id.to_string(), json!(rows));
    }
    Ok(Value::Object(matches))
}

pub(super) fn review_response(
    ctx: &Inputs,
    identity: &Value,
    vectors: &BTreeMap<u64, Vec<f32>>,
    doc: &ReidReviewDoc,
    version: String,
    can_write: bool,
    threshold: u8,
) -> Result<Value, String> {
    checked_review(doc, ctx, &doc.model_fingerprint, identity)?;
    let pinned = doc
        .groups
        .iter()
        .flatten()
        .copied()
        .collect::<BTreeSet<_>>();
    let suppressed = doc.suppressed_ids.iter().copied().collect::<BTreeSet<_>>();
    let mut preview = native_reid_report(ctx, identity, vectors, threshold)?;
    let groups = preview["groups"]
        .as_array_mut()
        .ok_or("ReID groups missing")?;
    groups.retain(|group| {
        group["track_ids"].as_array().is_some_and(|ids| {
            ids.iter().all(|id| {
                id.as_str()
                    .and_then(|s| s.parse::<u64>().ok())
                    .is_some_and(|id| !pinned.contains(&id) && !suppressed.contains(&id))
            })
        })
    });
    for members in &doc.groups {
        let key = members
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let digest = format!("{:x}", Sha256::digest(key.as_bytes()));
        groups.push(json!({
            "id":format!("manual-{}", &digest[..16]),
            "track_ids":members.iter().map(u64::to_string).collect::<Vec<_>>(),
            "similarity":null,
            "source":"manual"
        }));
    }
    let used = groups
        .iter()
        .flat_map(|group| {
            group["track_ids"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter_map(|s| s.parse::<u64>().ok())
        })
        .collect::<BTreeSet<_>>();
    let excluded = identity["excluded_ids"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_u64)
        .collect::<BTreeSet<_>>();
    let unmatched = ctx
        .known
        .ids()
        .iter()
        .filter(|id| !used.contains(id) && !excluded.contains(id))
        .map(u64::to_string)
        .collect::<Vec<_>>();
    preview["unmatched_ids"] = json!(unmatched);
    preview["confirmed"] = json!(false);
    let player_matches = player_match_candidates(ctx, identity, vectors)?;
    Ok(json!({
        "schema":"volley-native-reid-group-review/1",
        "project_id":ctx.project_id,"video":ctx.video,"replay_id":ctx.replay_id,
        "source_replay_sha256":ctx.source_sha,"revision":version,
        "threshold_percent":threshold,"writable":can_write,
        "groups":preview["groups"],"unmatched_ids":preview["unmatched_ids"],
        "player_matches":player_matches,
        "embedded_count":preview["embedded_count"],
        "missing_embedding_count":preview["missing_visual_ids"].as_array().map_or(0, Vec::len),
        "co_visible_veto_count":preview["co_visible_veto_count"],
        "confirmed_owner_veto_count":preview["confirmed_owner_veto_count"],
        "ambiguous_lookalike_veto_count":preview["ambiguous_lookalike_veto_count"],
        "suppressed_ids":doc.suppressed_ids,
        "similarity_type":"cosine_similarity_not_probability",
        "player_match_method":"reviewed_track_to_track_openvino_cosine",
        "auto_assign_enabled":false,"confirmed":false
    }))
}
pub async fn reid_review(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<ReidReviewRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    if !matches!(req.action.as_str(), "read" | "merge" | "split")
        || !(55..=99).contains(&req.threshold_percent)
    {
        return Err(storage::bad_request(
            "Invalid manual ReID review action or threshold",
        ));
    }
    let _slot = state
        .reid_limit
        .acquire()
        .await
        .map_err(|e| storage::internal(format!("Manual ReID lock slot: {e}")))?;
    let state = state.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<Value, ApiError> {
        let _guard = project_guard::write_guard(&state, &req.project_id)?;
        let ctx = source(&state, &req.project_id, &req.video, &req.replay_id)?;
        if ctx.source_sha != req.expected_source_sha256 {
            return Err(storage::conflict("Native CPU replay SHA changed"));
        }
        let model = volley_inference::reid_openvino::fingerprint(&state.repository_root)
            .map_err(storage::bad_request)?;
        let raw_identity = existing(&ctx.sidecar).map_err(storage::bad_request)?;
        let identity = validated(raw_identity.as_deref(), &ctx).map_err(storage::bad_request)?;
        let path = review_path(&state, &ctx)?;
        let raw = versioned_store::read_optional_bounded(&path, MAX_REID_REVIEW_BYTES)
            .map_err(storage::bad_request)?;
        let mut doc = match raw.as_deref() {
            Some(data) => serde_json::from_slice::<ReidReviewDoc>(data)
                .map_err(|e| storage::bad_request(format!("Invalid saved ReID review: {e}")))?,
            None => ReidReviewDoc::new(&ctx, &model),
        };
        checked_review(&doc, &ctx, &model, &identity).map_err(storage::conflict)?;
        let scope = native_reid_cache::CacheScope {
            project_id: ctx.project_id.clone(),
            video: ctx.video.clone(),
            replay_id: ctx.replay_id.clone(),
            replay_sha256: ctx.source_sha.clone(),
            model_fingerprint: model.clone(),
        };
        let cache =
            native_reid_cache::read(&state.data_root, &scope).map_err(storage::bad_request)?;
        // A missing feature cache is not an empty Track ID source. Always
        // return manual groups and unmatched singleton IDs for read requests.
        // Cached vectors are optional for reading/editing explicit manual groups.
        if cache.invalidated {
            return Err(storage::conflict(
                "ReID embeddings are stale; regenerate current features",
            ));
        }
        if req.action != "read" {
            if !state.writes_enabled {
                return Err(storage::conflict(
                    "Editing groups requires isolated project storage",
                ));
            }
            let expected = req.expected_revision.as_deref().ok_or_else(|| {
                storage::bad_request("Read current group revision before editing")
            })?;
            versioned_store::expect_revision(raw.as_deref(), expected)
                .map_err(storage::conflict)?;
            modify_review(&mut doc, &ctx, &identity, &req.action, &req.track_ids)
                .map_err(storage::conflict)?;
            let folder = path
                .parent()
                .ok_or_else(|| storage::internal("Missing ReID review folder"))?;
            versioned_store::write_json_checked(
                folder,
                &path,
                &serde_json::to_value(&doc)
                    .map_err(|e| storage::internal(format!("Encode ReID groups: {e}")))?,
                MAX_REID_REVIEW_BYTES,
            )
            .map_err(storage::bad_request)?;
        }
        let now = versioned_store::read_optional_bounded(&path, MAX_REID_REVIEW_BYTES)
            .map_err(storage::bad_request)?;
        if volley_inference::reid_openvino::fingerprint(&state.repository_root)
            .map_err(storage::bad_request)?
            != model
        {
            return Err(storage::conflict(
                "OpenVINO model changed during manual ReID review",
            ));
        }
        review_response(
            &ctx,
            &identity,
            &cache.vectors,
            &doc,
            revision(now.as_deref()),
            state.writes_enabled,
            req.threshold_percent,
        )
        .map_err(storage::bad_request)
    })
    .await
    .map_err(|e| storage::internal(format!("Manual ReID task: {e}")))??;
    Ok(Json(result))
}
