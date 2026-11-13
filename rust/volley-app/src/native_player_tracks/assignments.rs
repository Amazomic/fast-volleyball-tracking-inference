use super::*;

/// An overlap of lifespan ranges is not evidence of co-visibility:
/// the same athlete may have a lost/reacquired track during those ranges.
pub(super) fn tracks_co_visible(a: &Aggregate, b: &Aggregate) -> bool {
    let (mut i, mut j) = (0, 0);
    while i < a.visible_spans.len() && j < b.visible_spans.len() {
        let (a0, a1) = a.visible_spans[i];
        let (b0, b1) = b.visible_spans[j];
        if a0 <= b1 && b0 <= a1 { return true; }
        if a1 < b1 { i += 1; } else { j += 1; }
    }
    false
}pub(super) fn choose_automatic_photo_links(
    ctx: &Inputs,
    doc: &Value,
    matches: &Value,
) -> Result<BTreeMap<u64, String>, String> {
    let assigned = doc["assignments"]
        .as_object()
        .ok_or("Invalid assignments")?;
    let excluded: BTreeSet<_> = doc["excluded_ids"]
        .as_array()
        .ok_or("Invalid exclusions")?
        .iter()
        .filter_map(Value::as_u64)
        .collect();
    let suppressed: BTreeSet<_> = doc["automatic_suppressed_ids"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_u64)
        .collect();
    // Evaluate strongest evidence first. An athlete can have many tracklets,
    // but not two co-visible tracklets with distinct positions.
    let mut candidates = Vec::<(f64, u64, String)>::new();
    for (id, rows) in matches.as_object().ok_or("Invalid photo matches")? {
        let track_id = id.parse::<u64>().map_err(|_| "Invalid track ID")?;
        if !ctx.tracks.contains_key(&track_id)
            || assigned.contains_key(id)
            || excluded.contains(&track_id)
            || suppressed.contains(&track_id)
        {
            continue;
        }
        for row in rows.as_array().ok_or("Invalid candidate list")? {
            let Some(owner) = row["player_id"]
                .as_str()
                .filter(|owner| ctx.roster.contains_key(*owner)) else { continue };
            let Some(score) = row["cosine_similarity"]
                .as_f64()
                .filter(|score| score.is_finite() && *score > 0.30 && *score <= 1.0)
                else { continue };
            candidates.push((score, track_id, owner.to_owned()));
        }
    }
    // Try the best player first, then other photo matches above 30% only
    // when the strongest candidate conflicts with a co-visible track.
    let mut tracks = candidates.iter().map(|v| v.1).collect::<BTreeSet<_>>()
        .into_iter().collect::<Vec<_>>();
    let highest = |id| candidates.iter().filter(|v| v.1 == id)
        .map(|v| v.0).fold(0.0_f64, f64::max);
    tracks.sort_by(|a, b| {
        let (score_a, score_b) = (highest(*a), highest(*b));
        if score_a > 0.90 && score_b > 0.90 {
            let strength = |score: f64, id: u64| {
                score * (ctx.tracks[&id].observed as f64 + 1.0).ln()
            };
            strength(score_b, *b).total_cmp(&strength(score_a, *a))
        } else {
            score_b.total_cmp(&score_a)
        }
        .then_with(|| ctx.tracks[b].observed.cmp(&ctx.tracks[a].observed))
        .then_with(|| a.cmp(b))
    });
    let mut chosen = BTreeMap::<u64, String>::new();
    for id in tracks {
        let candidate = &ctx.tracks[&id];
        let overlaps = |other_id: u64| ctx.tracks.get(&other_id)
            .is_some_and(|other| tracks_co_visible(candidate, other));
        let mut ranked = candidates.iter().filter(|v| v.1 == id).collect::<Vec<_>>();
        ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.2.cmp(&b.2)));
        for (_, _, owner) in ranked {
            let locked = assigned.iter().any(|(other_id, other_owner)| {
                other_owner.as_str() == Some(owner.as_str())
                    && other_id.parse::<u64>().ok().is_some_and(overlaps)
            });
            let occupied = chosen.iter().any(|(other_id, other_owner)| {
                other_owner == owner && overlaps(*other_id)
            });
            if !locked && !occupied {
                chosen.insert(id, owner.clone());
                break;
            }
        }
    }
    Ok(chosen)
}
pub(super) fn existing(sidecar: &Path) -> Result<Option<Vec<u8>>, String> {
    versioned_store::read_optional_bounded(sidecar, MAX_LINKS)
}
fn automatic_links_current(state: &AppState, ctx: &Inputs, doc: &Value) -> bool {
    if doc["automatic_assignments"]
        .as_object()
        .is_none_or(|map| map.is_empty())
    {
        return true;
    }
    let Ok(model) = volley_inference::reid_openvino::fingerprint(&state.repository_root) else {
        return false;
    };
    if doc["automatic_identity_evidence"]["model_fingerprint"] != model {
        return false;
    }
    let mut albums = Vec::new();
    for player in ctx.roster.keys() {
        let Ok(folder) = checked_reference_folder(state, player, false) else {
            return false;
        };
        let Ok(raw) =
            versioned_store::read_optional_bounded(&folder.join("index.json"), MAX_REFERENCE_INDEX)
        else {
            return false;
        };
        let Ok(album) = references_doc(raw.as_deref(), player) else {
            return false;
        };
        for photo in album["photos"].as_array().into_iter().flatten() {
            let (Some(id), Some(expected)) = (photo["id"].as_str(), photo["image_sha256"].as_str())
            else {
                return false;
            };
            let Ok(Some(bytes)) = versioned_store::read_optional_bounded(
                &folder.join(format!("ref-{id}.jpg")),
                MAX_PHOTO_BYTES as u64,
            ) else {
                return false;
            };
            if format!("{:x}", Sha256::digest(&bytes)) != expected {
                return false;
            }
        }
        albums.push((player.clone(), revision(raw.as_deref())));
    }
    doc["automatic_identity_evidence"]["album_revisions"] == json!(albums)
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
    if let Some(automatic) = doc.get("automatic_assignments") {
        for (key, value) in automatic.as_object().ok_or("Invalid automatic links")? {
            let id = key
                .parse::<u64>()
                .map_err(|_| "Invalid automatic Track ID")?;
            if id.to_string() != *key
                || !ctx.known.contains_number(id)
                || assignments.contains_key(key)
                || excluded_set.contains(&id)
                || !value
                    .as_str()
                    .is_some_and(|owner| ctx.roster.contains_key(owner))
            {
                return Err("Stale or conflicting automatic Track ID link".into());
            }
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
    if !matches!(view, None | Some("unassigned") | Some("conflict") | Some("player"))
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
    let mut owners = reviewed.owners.clone();
    if let Some(automatic) = doc["automatic_assignments"].as_object() {
        for (id, owner) in automatic {
            if let (Ok(id), Some(owner)) = (id.parse::<u64>(), owner.as_str()) {
                if !excluded.contains(&id) {
                    owners.entry(id).or_insert_with(|| owner.to_owned());
                }
            }
        }
    }
    // Distinguish lack of photo similarity from a real co-visibility
    // conflict. A >30% photo match is not the same as a valid assignment.
    let unresolved_match_ids: BTreeSet<u64> = ctx.tracks.iter().filter(|(id, candidate)| {
        if owners.contains_key(id) { return false; }
        let Some(rows) = doc["automatic_identity_evidence"]["matches"][id.to_string()].as_array()
        else { return false };
        let eligible = rows.iter().filter(|row| {
            row["cosine_similarity"].as_f64().is_some_and(|score| score > 0.30)
                && row["player_id"].as_str().is_some_and(|name| ctx.roster.contains_key(name))
        }).collect::<Vec<_>>();
        !eligible.is_empty() && eligible.iter().all(|row| {
            let player = row["player_id"].as_str().unwrap_or_default();
            owners.iter().any(|(other_id, owner)| {
                owner == player && ctx.tracks.get(other_id)
                    .is_some_and(|other| tracks_co_visible(candidate, other))
            })
        })
    }).map(|(id, _)| *id).collect();
    let scoped_assigned = owners
        .keys()
        .filter(|id| ctx.tracks.contains_key(id))
        .count();
    let scoped_excluded = excluded
        .iter()
        .filter(|id| ctx.tracks.contains_key(id))
        .count();
    let matches = |id: &u64| match view {
        Some("unassigned") => !owners.contains_key(id) && !unresolved_match_ids.contains(id),
        Some("conflict") => unresolved_match_ids.contains(id),
        Some("player") => owners.get(id).map(String::as_str) == player_id,
        _ => true,
    };
    let filtered_total = ctx.tracks.keys().filter(|id| matches(id)).count();
    let mut player_track_counts = std::collections::BTreeMap::<&str, usize>::new();
    for id in ctx.tracks.keys() {
        if let Some(owner) = owners.get(id) {
            *player_track_counts.entry(owner.as_str()).or_default() += 1;
        }
    }
    let mut rows = Vec::new();
    for (id, info) in ctx
        .tracks
        .iter()
        .filter(|(id, _)| matches(id))
        .skip(offset)
        .take(limit)
    {
        let owner = owners.get(id).map(String::as_str);
        rows.push(json!({
            "track_id":id,"first_frame":info.first,"last_frame":info.last,
            "observations":info.observed,"best_frame":info.best_frame,
            "best_score":info.best_score,"bbox_xyxy_norm":info.best_box,
            "assigned_player_id":owner,
            "assigned_player_name":owner.and_then(|v|ctx.roster.get(v)),
            "excluded":excluded.contains(id),
            "identity_conflict":unresolved_match_ids.contains(id),
            "identity_source":if reviewed.owners.contains_key(id){"manual"}else if owner.is_some(){"automatic_photo"}else{"none"},
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
        "player_track_counts":player_track_counts,
        "unassigned_track_ids":ctx.tracks.keys().filter(|id| !owners.contains_key(id) && !unresolved_match_ids.contains(id)).copied().collect::<Vec<_>>(),
        "comparison_track_ids":ctx.tracks.keys().filter(|id| !owners.contains_key(id) && !excluded.contains(id)).copied().collect::<Vec<_>>(),
        "rally_track_ids":ctx.tracks.keys().copied().collect::<Vec<_>>(),
        "assigned_count":scoped_assigned,"excluded_count":scoped_excluded,
        "identity_conflict_track_ids":unresolved_match_ids,
        "roster":roster,"tracks":rows,
        "automatic_identity_matches":doc["automatic_identity_evidence"]["matches"],
        "athlete_links_require_manual_confirmation":true,
        "automatic_reid_binding":doc["automatic_assignments"].as_object().is_some_and(|map| !map.is_empty()),"photos_auto_registered":false,
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
pub(crate) fn display_player_names(
    state: &AppState,
    project_id: &str,
    video: &str,
    replay_id: &str,
) -> Result<(String, String, BTreeMap<String, String>), ApiError> {
    let ctx = source(state, project_id, video, replay_id)?;
    let raw = existing(&ctx.sidecar).map_err(storage::bad_request)?;
    let mut doc = validated(raw.as_deref(), &ctx).map_err(storage::bad_request)?;
    if !automatic_links_current(state, &ctx, &doc) {
        doc["automatic_assignments"] = json!({});
    }
    let mut names: BTreeMap<String, String> = doc["assignments"]
        .as_object()
        .into_iter()
        .flat_map(|map| map.iter())
        .filter_map(|(id, owner)| Some((id.clone(), ctx.roster.get(owner.as_str()?)?.to_owned())))
        .collect();
    names.extend(
        doc["automatic_assignments"]
            .as_object()
            .into_iter()
            .flat_map(|map| map.iter())
            .filter_map(|(id, owner)| {
                let name = ctx.roster.get(owner.as_str()?)?;
                Some((id.clone(), name.to_owned()))
            }),
    );
    Ok((ctx.source_sha, revision(raw.as_deref()), names))
}
pub(crate) fn automatic_links_revision(
    state: &AppState,
    project_id: &str,
    video: &str,
    replay_id: &str,
) -> Result<Option<String>, ApiError> {
    let ctx = source(state, project_id, video, replay_id)?;
    let raw = existing(&ctx.sidecar).map_err(storage::bad_request)?;
    let doc = validated(raw.as_deref(), &ctx).map_err(storage::bad_request)?;
    Ok(automatic_links_current(state, &ctx, &doc).then(|| revision(raw.as_deref())))
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
    let mut doc = validated(bytes.as_deref(), &ctx).map_err(storage::bad_request)?;
    if !automatic_links_current(state, &ctx, &doc) {
        doc["automatic_assignments"] = json!({});
    }
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
    let mut suppressed: BTreeSet<u64> = doc["automatic_suppressed_ids"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_u64)
        .collect();
    for id in &req.track_ids {
        if req.action == "unassign" {
            suppressed.insert(*id);
        }
        if req.action == "assign" || req.action == "restore" {
            suppressed.remove(id);
        }
    }
    doc["automatic_suppressed_ids"] = json!(suppressed);
    let conflict_tracks = source(state, &req.project_id, &req.video, &req.replay_id)?;
    if let Some(automatic) = doc
        .get_mut("automatic_assignments")
        .and_then(Value::as_object_mut)
    {
        for id in &req.track_ids {
            automatic.remove(&id.to_string());
        }
        automatic.retain(|id, owner| {
            let Some(track) = id
                .parse::<u64>()
                .ok()
                .and_then(|id| conflict_tracks.tracks.get(&id))
            else {
                return false;
            };
            !review.assignments.iter().any(|(other_id, other_owner)| {
                owner.as_str() == Some(other_owner.as_str())
                    && conflict_tracks
                        .tracks
                        .get(other_id)
                        .is_some_and(|other| track.first <= other.last && other.first <= track.last)
            })
        });
    }
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
    let mut view = validated(new.as_deref(), &ctx).map_err(storage::bad_request)?;
    if !automatic_links_current(state, &ctx, &view) {
        view["automatic_assignments"] = json!({});
    }
    page(
        &ctx,
        &view,
        &revision(new.as_deref()),
        req.offset,
        req.limit,
    )
    .map_err(storage::bad_request)
}

pub(super) fn save_automatic_photo_links(
    state: &AppState,
    req: &ReidReviewRequest,
    comparison: &Value,
) -> Result<Value, ApiError> {
    if !state.writes_enabled {
        return Err(storage::conflict(
            "Automatic links require isolated writable storage",
        ));
    }
    let _guard = project_guard::write_guard(state, &req.project_id)?;
    let snapshot = super::photo_comparison::snapshot(state, req)?;
    if comparison["identity_revision"] != json!(snapshot.identity_revision)
        || comparison["album_revisions"] != json!(snapshot.albums)
        || comparison["model_fingerprint"] != json!(snapshot.model)
    {
        return Err(storage::conflict(
            "Photo references or model changed during automatic comparison",
        ));
    }
    let ctx = source(state, &req.project_id, &req.video, &req.replay_id)?;
    if ctx.source_sha != req.expected_source_sha256 {
        return Err(storage::conflict("Source replay changed"));
    }
    let before = existing(&ctx.sidecar).map_err(storage::bad_request)?;
    if comparison["identity_revision"].as_str() != Some(revision(before.as_deref()).as_str()) {
        return Err(storage::conflict(
            "Player links changed during automatic comparison",
        ));
    }
    let mut doc = validated(before.as_deref(), &ctx).map_err(storage::bad_request)?;
    let scoped = source_for_rallies(
        state,
        &req.project_id,
        &req.video,
        &req.replay_id,
        req.rally_run_id.as_deref(),
    )?;
    // Rebuild identity membership once per CPU result, using the photo
    // comparison only. The old assignment sidecar must not bias this pass.
    let mut reset = doc.clone();
    reset["assignments"] = json!({});
    reset["automatic_assignments"] = json!({});
    reset["excluded_ids"] = json!([]);
    reset["automatic_suppressed_ids"] = json!([]);
    let matches = &comparison["player_matches"];
    let chosen = choose_automatic_photo_links(&scoped, &reset, matches)
        .map_err(storage::bad_request)?;
    doc = reset;
    doc["automatic_assignments"] = json!(chosen.iter()
        .map(|(id, player)| (id.to_string(), player.clone()))
        .collect::<BTreeMap<_, _>>());
    doc["automatic_identity_evidence"] = json!({
        "threshold":0.30, "matches":matches,
        "album_revisions":comparison["album_revisions"],
        "model_fingerprint":comparison["model_fingerprint"]
    });
    let unassigned: Vec<_> = scoped
        .tracks
        .keys()
        .filter(|id| {
            let key = id.to_string();
            !doc["assignments"]
                .as_object()
                .is_some_and(|map| map.contains_key(&key))
                && !doc["automatic_assignments"]
                    .as_object()
                    .is_some_and(|map| map.contains_key(&key))
        })
        .copied()
        .collect();
    let parent = ctx
        .sidecar
        .parent()
        .ok_or_else(|| storage::internal("Link directory missing"))?;
    versioned_store::write_json_checked(parent, &ctx.sidecar, &doc, MAX_LINKS)
        .map_err(storage::bad_request)?;
    let saved = existing(&ctx.sidecar).map_err(storage::bad_request)?;
    Ok(
        json!({"automatic_count":chosen.len(),"automatic_assignments":chosen,"unassigned_track_ids":unassigned,"identity_revision":revision(saved.as_deref()),"confirmed":false}),
    )
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
