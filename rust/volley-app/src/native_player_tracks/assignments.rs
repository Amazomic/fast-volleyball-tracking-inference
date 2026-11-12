use super::*;
pub(super) fn existing(sidecar: &Path) -> Result<Option<Vec<u8>>, String> {
    versioned_store::read_optional_bounded(sidecar, MAX_LINKS)
}
pub(super) fn base_document(ctx: &Inputs) -> Value {
    json!({
        "schema":"volley-native-player-track-links/1",
        "project_id":ctx.project_id,"video":ctx.video,
        "replay_id":ctx.replay_id,
        "source_replay_sha256":ctx.source_sha,
        "assignments":{},"excluded_ids":[],
        "athlete_identity_claims_manually_reviewed":true,
        "automatic_reid_binding":false,
    })
}
pub(super) fn validated(raw: Option<&[u8]>, ctx: &Inputs) -> Result<Value, String> {
    let Some(bytes) = raw else {
        return Ok(base_document(ctx));
    };
    let doc: Value = serde_json::from_slice(bytes)
        .map_err(|e| format!("Invalid track assignments JSON: {e}"))?;
    if doc["schema"] != "volley-native-player-track-links/1"
        || doc["project_id"] != ctx.project_id
        || doc["video"] != ctx.video
        || doc["replay_id"] != ctx.replay_id
        || doc["source_replay_sha256"] != ctx.source_sha
        || doc["athlete_identity_claims_manually_reviewed"] != true
        || doc["automatic_reid_binding"] != false
    {
        return Err("Manual Track ID links refer to a different replay or schema".into());
    }
    let assignments = doc["assignments"]
        .as_object()
        .ok_or("Invalid assigned Track IDs")?;
    let excluded = doc["excluded_ids"]
        .as_array()
        .ok_or("Invalid excluded Track IDs")?;
    if assignments.len() > MAX_TRACKS || excluded.len() > MAX_TRACKS {
        return Err("Manual player links exceed safety cap".into());
    }
    let mut excluded_set = BTreeSet::new();
    for value in excluded {
        let id = value.as_u64().ok_or("Invalid excluded Track ID")?;
        if !ctx.known.contains_number(id) || !excluded_set.insert(id) {
            return Err("Unknown or duplicate excluded Track ID".into());
        }
    }
    for (key, value) in assignments {
        let id = key
            .parse::<u64>()
            .map_err(|_| "Invalid assigned Track ID")?;
        if id.to_string() != *key || !ctx.known.contains_number(id) || excluded_set.contains(&id) {
            return Err("Stale, excluded or unknown assigned Track ID".into());
        }
        let owner = value.as_str().ok_or("Invalid player ID in Track link")?;
        if !ctx.roster.contains_key(owner) {
            return Err("Linked player is no longer on the project's roster".into());
        }
    }
    Ok(doc)
}
pub(super) fn page(
    ctx: &Inputs,
    doc: &Value,
    version: &str,
    offset: usize,
    limit: usize,
) -> Result<Value, String> {
    filtered_page(ctx, doc, version, offset, limit, None, None)
}
pub(super) fn filtered_page(
    ctx: &Inputs,
    doc: &Value,
    version: &str,
    offset: usize,
    limit: usize,
    view: Option<&str>,
    player_id: Option<&str>,
) -> Result<Value, String> {
    if limit == 0 || limit > 100 || offset > MAX_TRACKS {
        return Err("Invalid Track ID page size".into());
    }
    if !matches!(view, None | Some("unassigned") | Some("player"))
        || (view == Some("player") && player_id.is_none_or(|id| !ctx.roster.contains_key(id)))
        || (view != Some("player") && player_id.is_some())
    {
        return Err("Invalid Track ID gallery filter".into());
    }
    let assignments = doc["assignments"]
        .as_object()
        .ok_or("Assignments missing")?;
    let excluded = doc["excluded_ids"]
        .as_array()
        .ok_or("Excluded list missing")?
        .iter()
        .filter_map(Value::as_u64)
        .collect::<BTreeSet<_>>();
    let reviewed_links = assignments
        .iter()
        .map(|(id, owner)| {
            Ok(ReviewedLink {
                track_id: id.parse::<u64>().map_err(|_| "Invalid reviewed ID")?,
                owner: owner
                    .as_str()
                    .ok_or("Invalid reviewed player ID")?
                    .to_owned(),
                reviewed: true,
            })
        })
        .collect::<Result<Vec<_>, &str>>()?;
    let reviewed =
        identity_review::resolve_reviewed(ctx.known.ids(), &ctx.roster, &reviewed_links, &excluded);
    let scoped_assigned = assignments
        .keys()
        .filter(|id| {
            id.parse::<u64>()
                .ok()
                .is_some_and(|tid| ctx.tracks.contains_key(&tid))
        })
        .count();
    let scoped_excluded = excluded
        .iter()
        .filter(|id| ctx.tracks.contains_key(id))
        .count();
    let matches = |id: &u64| match view {
        Some("unassigned") => !reviewed.owners.contains_key(id),
        Some("player") => reviewed.owners.get(id).map(String::as_str) == player_id,
        _ => true,
    };
    let filtered_total = ctx.tracks.keys().filter(|id| matches(id)).count();
    let mut rows = Vec::new();
    for (id, info) in ctx
        .tracks
        .iter()
        .filter(|(id, _)| matches(id))
        .skip(offset)
        .take(limit)
    {
        let owner = reviewed.owners.get(id).map(String::as_str);
        rows.push(json!({
            "track_id":id,"first_frame":info.first,"last_frame":info.last,
            "observations":info.observed,"best_frame":info.best_frame,
            "best_score":info.best_score,"bbox_xyxy_norm":info.best_box,
            "assigned_player_id":owner,
            "assigned_player_name":owner.and_then(|v|ctx.roster.get(v)),
            "excluded":excluded.contains(id),
            "identity_source":if owner.is_some(){"manual"}else{"none"},
            "track_id_provisional":true,
        }));
    }
    let roster = ctx
        .roster
        .iter()
        .map(|(id, name)| json!({"id":id,"name":name}))
        .collect::<Vec<_>>();
    Ok(json!({
        "schema":"volley-native-player-track-links/1",
        "project_id":ctx.project_id,"video":ctx.video,"replay_id":ctx.replay_id,
        "source_replay_sha256":ctx.source_sha,
        "source_kind":ctx.evidence.source_kind(),
        "evidence_source":ctx.evidence,
        "revision":version,"writable":ctx.can_write,
        "total_tracks":ctx.tracks.len(),"offset":offset,"limit":limit,
        "filtered_total":filtered_total,
        "unassigned_track_ids":ctx.tracks.keys().filter(|id| !reviewed.owners.contains_key(id)).copied().collect::<Vec<_>>(),
        "comparison_track_ids":ctx.tracks.keys().filter(|id| !reviewed.owners.contains_key(id) && !excluded.contains(id)).copied().collect::<Vec<_>>(),
        "rally_track_ids":ctx.tracks.keys().copied().collect::<Vec<_>>(),
        "assigned_count":scoped_assigned,"excluded_count":scoped_excluded,
        "roster":roster,"tracks":rows,
        "athlete_links_require_manual_confirmation":true,
        "automatic_reid_binding":false,"photos_auto_registered":false,
    }))
}
pub(crate) struct NativeReviewedOwners {
    pub source_sha: String,
    pub revision: String,
    pub owners: BTreeMap<u64, String>,
    pub excluded: BTreeSet<u64>,
    pub roster: BTreeMap<String, String>,
}
pub(crate) fn reviewed_owners(
    state: &AppState,
    project_id: &str,
    video: &str,
    replay_id: &str,
) -> Result<NativeReviewedOwners, ApiError> {
    let ctx = source(state, project_id, video, replay_id)?;
    let raw = existing(&ctx.sidecar).map_err(storage::bad_request)?;
    let doc = validated(raw.as_deref(), &ctx).map_err(storage::bad_request)?;
    let links = doc["assignments"]
        .as_object()
        .into_iter()
        .flat_map(|map| map.iter())
        .map(|(id, owner)| ReviewedLink {
            track_id: id
                .parse::<u64>()
                .expect("Source-validated numeric Track ID"),
            owner: owner
                .as_str()
                .expect("Source-validated roster owner")
                .to_owned(),
            reviewed: true,
        })
        .collect::<Vec<_>>();
    let excluded = doc["excluded_ids"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_u64)
        .collect::<BTreeSet<_>>();
    let resolved =
        identity_review::resolve_reviewed(ctx.known.ids(), &ctx.roster, &links, &excluded);
    if !resolved.conflicts.is_empty() {
        return Err(storage::conflict(
            "Manual native athlete assignments contain contradictions",
        ));
    }
    Ok(NativeReviewedOwners {
        source_sha: ctx.source_sha,
        revision: revision(raw.as_deref()),
        owners: resolved.owners,
        excluded: resolved.excluded,
        roster: ctx.roster,
    })
}
pub(super) fn read_internal(state: &AppState, req: &TrackQuery) -> Result<Value, ApiError> {
    let ctx = source_for_rallies(
        state,
        &req.project_id,
        &req.video,
        &req.replay_id,
        req.rally_run_id.as_deref(),
    )?;
    let bytes = existing(&ctx.sidecar).map_err(storage::bad_request)?;
    let doc = validated(bytes.as_deref(), &ctx).map_err(storage::bad_request)?;
    filtered_page(
        &ctx,
        &doc,
        &revision(bytes.as_deref()),
        req.offset,
        req.limit,
        req.view.as_deref(),
        req.view_player_id.as_deref(),
    )
    .map_err(storage::bad_request)
}
pub(super) fn change(state: &AppState, req: &TrackEdit) -> Result<Value, ApiError> {
    if !state.writes_enabled {
        return Err(storage::conflict(
            "Editing Track ID requires isolated project storage",
        ));
    }
    if !matches!(
        req.action.as_str(),
        "assign" | "unassign" | "exclude" | "restore"
    ) || req.track_ids.is_empty()
        || req.track_ids.len() > 200
        || req.track_ids.iter().collect::<BTreeSet<_>>().len() != req.track_ids.len()
    {
        return Err(storage::bad_request(
            "Choose 1–200 unique IDs and a valid edit action",
        ));
    }
    if !versioned_store::valid_revision(&req.expected_revision) {
        return Err(storage::bad_request("Invalid assignment revision"));
    }
    let ctx = source_for_rallies(
        state,
        &req.project_id,
        &req.video,
        &req.replay_id,
        req.rally_run_id.as_deref(),
    )?;
    if req.track_ids.iter().any(|id| !ctx.tracks.contains_key(id)) {
        return Err(storage::bad_request(
            "Selected Track ID has no observation in the chosen rally version",
        ));
    }
    if req.action == "assign"
        && !req
            .player_id
            .as_ref()
            .is_some_and(|id| valid_player_id(id) && ctx.roster.contains_key(id))
    {
        return Err(storage::bad_request(
            "Selected athlete is not in project roster",
        ));
    }
    if req.action != "assign" && req.player_id.is_some() {
        return Err(storage::bad_request(
            "Player ID is only accepted for an assignment",
        ));
    }
    let before = existing(&ctx.sidecar).map_err(storage::bad_request)?;
    if versioned_store::expect_revision(before.as_deref(), &req.expected_revision).is_err() {
        return Err(storage::conflict(
            "Track bindings changed; refresh the revision",
        ));
    }
    let mut doc = validated(before.as_deref(), &ctx).map_err(storage::bad_request)?;
    let assignments = doc["assignments"]
        .as_object()
        .ok_or_else(|| storage::bad_request("Missing assignment object"))?
        .iter()
        .map(|(key, value)| {
            Ok((
                key.parse::<u64>()
                    .map_err(|_| storage::bad_request("Invalid saved Track ID"))?,
                value
                    .as_str()
                    .ok_or_else(|| storage::bad_request("Invalid saved athlete ID"))?
                    .to_owned(),
            ))
        })
        .collect::<Result<BTreeMap<_, _>, ApiError>>()?;
    let excluded = doc["excluded_ids"]
        .as_array()
        .ok_or_else(|| storage::bad_request("Missing excluded list"))?
        .iter()
        .map(|v| {
            v.as_u64()
                .ok_or_else(|| storage::bad_request("Invalid excluded ID"))
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    let mut review = ManualReviewState {
        assignments,
        excluded,
        rejected: BTreeSet::new(),
    };
    review
        .apply(
            &ctx.known,
            &ctx.roster,
            ReviewAction::parse(&req.action).map_err(|e| storage::bad_request(e.message))?,
            &req.track_ids,
            req.player_id.as_deref(),
            false,
        )
        .map_err(|e| match e.kind {
            ReviewErrorKind::Invalid => storage::bad_request(e.message),
            ReviewErrorKind::Conflict => storage::conflict(e.message),
        })?;
    doc["assignments"] = json!(review
        .assignments
        .iter()
        .map(|(id, player)| (id.to_string(), player.clone()))
        .collect::<BTreeMap<_, _>>());
    doc["excluded_ids"] = json!(review.excluded);
    if before.is_none()
        || doc != validated(before.as_deref(), &ctx).map_err(storage::bad_request)?
    {
        let parent = ctx
            .sidecar
            .parent()
            .ok_or_else(|| storage::internal("Link directory missing"))?;
        versioned_store::write_json_checked(parent, &ctx.sidecar, &doc, MAX_LINKS)
            .map_err(storage::bad_request)?;
    }
    let new = existing(&ctx.sidecar).map_err(storage::bad_request)?;
    page(
        &ctx,
        &validated(new.as_deref(), &ctx).map_err(storage::bad_request)?,
        &revision(new.as_deref()),
        req.offset,
        req.limit,
    )
    .map_err(storage::bad_request)
}
pub async fn read(
    State(state): State<Arc<AppState>>,
    Query(req): Query<TrackQuery>,
) -> ApiResult<Value> {
    // Reading a source-SHA-verified, immutable replay and an atomically
    // replaced assignment sidecar does not modify the project. Do not take
    // the exclusive writer lock: parallel gallery/ReID reads must coexist.
    let result = tokio::task::spawn_blocking(move || read_internal(&state, &req))
        .await
        .map_err(|e| storage::internal(format!("Track ID read task: {e}")))??;
    Ok(Json(result))
}
pub async fn edit(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<TrackEdit>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    let result = tokio::task::spawn_blocking(move || {
        let _lock = project_guard::write_guard(&state, &req.project_id)?;
        change(&state, &req)
    })
    .await
    .map_err(|e| storage::internal(format!("Track ID edit task: {e}")))??;
    Ok(Json(result))
}
