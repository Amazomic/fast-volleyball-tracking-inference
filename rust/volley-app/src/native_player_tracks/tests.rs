use super::*;
use sha2::{Digest, Sha256};
#[test]
fn photo_rebuild_does_not_inherit_previous_manual_or_automatic_owners() {
    let (root, _, sha) = fixture();
    let tracks = index_replay(&root.join("tracked_frames.jsonl"), &sha, 18).unwrap();
    let ctx = context(&root, sha, tracks);
    let mut previous = base_document(&ctx);
    previous["assignments"] = json!({"7":"abcdef012345"});
    previous["automatic_assignments"] = json!({"7":"abcdef012345"});
    let mut rebuilt = previous;
    rebuilt["assignments"] = json!({});
    rebuilt["automatic_assignments"] = json!({});
    rebuilt["excluded_ids"] = json!([]);
    rebuilt["automatic_suppressed_ids"] = json!([]);
    let empty = json!({});
    assert!(choose_automatic_photo_links(&ctx, &rebuilt, &empty).unwrap().is_empty());
    let matches = json!({"7":[{"player_id":"abcdef012345","cosine_similarity":0.88}]});
    assert_eq!(choose_automatic_photo_links(&ctx, &rebuilt, &matches).unwrap()[&7], "abcdef012345");
    fs::remove_dir_all(root).unwrap();
}#[test]
fn conflict_ids_are_not_mislabeled_as_no_photo_match() {
    let (root, _, sha) = fixture();
    let tracks = index_replay(&root.join("tracked_frames.jsonl"), &sha, 18).unwrap();
    let mut ctx = context(&root, sha, tracks);
    ctx.tracks.insert(8, ctx.tracks[&7].clone());
    ctx.known = VerifiedTrackSet::new(ctx.evidence.clone(), [7, 8]).unwrap();
    let mut doc = base_document(&ctx);
    doc["automatic_assignments"] = json!({"7":"abcdef012345"});
    doc["automatic_identity_evidence"] = json!({
        "matches":{"8":[{"player_id":"abcdef012345","cosine_similarity":0.81}]}
    });
    let unmatched = filtered_page(&ctx, &doc, "rev", 0, 10, Some("unassigned"), None).unwrap();
    assert_eq!(unmatched["unassigned_track_ids"], json!([]));
    assert_eq!(unmatched["filtered_total"], 0);
    assert_eq!(unmatched["identity_conflict_track_ids"], json!([8]));
    let conflicts = filtered_page(&ctx, &doc, "rev", 0, 10, Some("conflict"), None).unwrap();
    assert_eq!(conflicts["filtered_total"], 1);
    assert_eq!(conflicts["tracks"][0]["track_id"], 8);
    assert_eq!(conflicts["tracks"][0]["identity_conflict"], true);
    fs::remove_dir_all(root).unwrap();
}#[test]
fn photo_links_join_lost_tracks_without_real_co_visibility() {
    let (root, _, sha) = fixture();
    let tracks = index_replay(&root.join("tracked_frames.jsonl"), &sha, 18).unwrap();
    let mut ctx = context(&root, sha, tracks);
    let mut intermittent = ctx.tracks[&7].clone();
    intermittent.first = 3;
    intermittent.last = 14;
    intermittent.visible_spans = vec![(3, 5), (10, 14)];
    ctx.tracks.get_mut(&7).unwrap().visible_spans = vec![(1, 2), (6, 9)];
    ctx.tracks.insert(8, intermittent);
    ctx.known = VerifiedTrackSet::new(ctx.evidence.clone(), [7, 8]).unwrap();
    assert!(ctx.tracks[&7].first <= ctx.tracks[&8].last);
    assert!(!tracks_co_visible(&ctx.tracks[&7], &ctx.tracks[&8]));
    let matches = json!({
        "7":[{"player_id":"abcdef012345","cosine_similarity":0.88}],
        "8":[{"player_id":"abcdef012345","cosine_similarity":0.93}]
    });
    let assigned = choose_automatic_photo_links(&ctx, &base_document(&ctx), &matches).unwrap();
    assert_eq!(assigned.len(), 2);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn overlapping_people_try_next_photo_match_above_threshold() {
    let (root, _, sha) = fixture();
    let tracks = index_replay(&root.join("tracked_frames.jsonl"), &sha, 18).unwrap();
    let mut ctx = context(&root, sha, tracks);
    ctx.roster.insert("123456abcdef".into(), "Player B".into());
    ctx.tracks.insert(8, ctx.tracks[&7].clone());
    ctx.known = VerifiedTrackSet::new(ctx.evidence.clone(), [7, 8]).unwrap();
    assert!(tracks_co_visible(&ctx.tracks[&7], &ctx.tracks[&8]));
    let matches = json!({
        "7":[{"player_id":"abcdef012345","cosine_similarity":0.95}],
        "8":[
            {"player_id":"abcdef012345","cosine_similarity":0.89},
            {"player_id":"123456abcdef","cosine_similarity":0.76}
        ]
    });
    let assigned = choose_automatic_photo_links(&ctx, &base_document(&ctx), &matches).unwrap();
    assert_eq!(assigned.len(), 2);
    assert_eq!(assigned[&7], "abcdef012345");
    assert_eq!(assigned[&8], "123456abcdef");
    fs::remove_dir_all(root).unwrap();
}#[test]
fn automatic_photo_link_picks_the_highest_similarity_for_one_track() {
    let (root, _, sha) = fixture();
    let tracks = index_replay(&root.join("tracked_frames.jsonl"), &sha, 18).unwrap();
    let mut ctx = context(&root, sha, tracks);
    ctx.roster.insert("123456abcdef".into(), "Player B".into());
    let matches = json!({"7":[{"player_id":"abcdef012345","cosine_similarity":0.54},{"player_id":"123456abcdef","cosine_similarity":0.68}]});
    let chosen = choose_automatic_photo_links(&ctx, &base_document(&ctx), &matches).unwrap();
    assert_eq!(chosen[&7], "123456abcdef");
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn strongest_long_lived_track_wins_overlapping_assignment() {
    let (root, _, sha) = fixture();
    let tracks = index_replay(&root.join("tracked_frames.jsonl"), &sha, 18).unwrap();
    let mut ctx = context(&root, sha, tracks);
    let mut short = ctx.tracks[&7].clone();
    short.observed = 12;
    ctx.tracks.insert(8, short);
    ctx.tracks.get_mut(&7).unwrap().observed = 300;
    ctx.known = VerifiedTrackSet::new(ctx.evidence.clone(), [7, 8]).unwrap();
    let matches = json!({
        "7":[{"player_id":"abcdef012345","cosine_similarity":0.81}],
        "8":[{"player_id":"abcdef012345","cosine_similarity":0.99}]
    });
    let chosen = choose_automatic_photo_links(&ctx, &base_document(&ctx), &matches).unwrap();
    assert_eq!(chosen.len(), 1);
    assert_eq!(chosen[&8], "abcdef012345");
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn nearly_equal_strong_reid_prefers_sustained_track() {
    let (root, _, sha) = fixture();
    let tracks = index_replay(&root.join("tracked_frames.jsonl"), &sha, 18).unwrap();
    let mut ctx = context(&root, sha, tracks);
    let mut fragment = ctx.tracks[&7].clone();
    fragment.observed = 162;
    ctx.tracks.insert(8, fragment);
    ctx.tracks.get_mut(&7).unwrap().observed = 562;
    ctx.known = VerifiedTrackSet::new(ctx.evidence.clone(), [7, 8]).unwrap();
    let matches = json!({
        "7":[{"player_id":"abcdef012345","cosine_similarity":0.972}],
        "8":[{"player_id":"abcdef012345","cosine_similarity":0.997}]
    });
    let chosen = choose_automatic_photo_links(&ctx, &base_document(&ctx), &matches).unwrap();
    assert_eq!(chosen.len(), 1);
    assert_eq!(chosen[&7], "abcdef012345");
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn strongly_matched_player_is_not_stolen_by_weaker_track() {
    let (root, _, sha) = fixture();
    let tracks = index_replay(&root.join("tracked_frames.jsonl"), &sha, 18).unwrap();
    let mut ctx = context(&root, sha, tracks);
    ctx.roster.insert("123456abcdef".into(), "Player B".into());
    ctx.tracks.insert(8, ctx.tracks[&7].clone());
    ctx.known = VerifiedTrackSet::new(ctx.evidence.clone(), [7, 8]).unwrap();
    let matches = json!({
        "7":[
            {"player_id":"abcdef012345","cosine_similarity":0.972},
            {"player_id":"123456abcdef","cosine_similarity":0.39}
        ],
        "8":[{"player_id":"abcdef012345","cosine_similarity":0.634}]
    });
    let chosen = choose_automatic_photo_links(&ctx, &base_document(&ctx), &matches).unwrap();
    assert_eq!(chosen[&7], "abcdef012345");
    assert!(!chosen.contains_key(&8));
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn automatic_photo_links_enforce_threshold_and_conflicting_tracks() {
    let (root, _, sha) = fixture();
    let tracks = index_replay(&root.join("tracked_frames.jsonl"), &sha, 18).unwrap();
    let mut ctx = context(&root, sha, tracks);
    let mut other = ctx.tracks[&7].clone();
    other.first = 4;
    other.last = 14;
    ctx.tracks.insert(8, other);
    ctx.known = VerifiedTrackSet::new(ctx.evidence.clone(), [7, 8]).unwrap();
    let mut doc = base_document(&ctx);
    let matches = json!({"7":[{"player_id":"abcdef012345","cosine_similarity":0.80}],"8":[{"player_id":"abcdef012345","cosine_similarity":0.70}]});
    let chosen = choose_automatic_photo_links(&ctx, &doc, &matches).unwrap();
    assert_eq!(chosen.len(), 1);
    assert_eq!(chosen[&7], "abcdef012345");
    doc["excluded_ids"] = json!([7]);
    let below = json!({"7":[{"player_id":"abcdef012345","cosine_similarity":0.99}],"8":[{"player_id":"abcdef012345","cosine_similarity":0.30}]});
    assert!(choose_automatic_photo_links(&ctx, &doc, &below)
        .unwrap()
        .is_empty());
    let above = json!({"8":[{"player_id":"abcdef012345","cosine_similarity":0.30001}]});
    assert_eq!(
        choose_automatic_photo_links(&ctx, &doc, &above)
            .unwrap()
            .len(),
        1
    );
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn automatic_links_do_not_fall_back_to_the_wrong_player() {
    let (root, _, sha) = fixture();
    let tracks = index_replay(&root.join("tracked_frames.jsonl"), &sha, 18).unwrap();
    let mut ctx = context(&root, sha, tracks);
    ctx.roster.insert("123456abcdef".into(), "Player B".into());
    ctx.tracks.insert(8, ctx.tracks[&7].clone());
    ctx.known = VerifiedTrackSet::new(ctx.evidence.clone(), [7, 8]).unwrap();
    let matches = json!({"7":[{"player_id":"abcdef012345","cosine_similarity":0.90},{"player_id":"123456abcdef","cosine_similarity":0.80}],"8":[{"player_id":"abcdef012345","cosine_similarity":0.85},{"player_id":"123456abcdef","cosine_similarity":0.30}]});
    let chosen = choose_automatic_photo_links(&ctx, &base_document(&ctx), &matches).unwrap();
    assert_eq!(chosen.len(), 1);
    assert_eq!(chosen[&7], "abcdef012345");
    assert!(!chosen.contains_key(&8));
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn automatic_photo_link_appears_as_provisional_and_not_reviewed() {
    let (root, _, sha) = fixture();
    let tracks = index_replay(&root.join("tracked_frames.jsonl"), &sha, 18).unwrap();
    let ctx = context(&root, sha, tracks);
    let mut doc = base_document(&ctx);
    doc["automatic_assignments"] = json!({"7":"abcdef012345"});
    let response = page(&ctx, &doc, "rev", 0, 40).unwrap();
    assert_eq!(response["assigned_count"], 1);
    assert_eq!(response["tracks"][0]["identity_source"], "automatic_photo");
    assert_eq!(response["tracks"][0]["assigned_player_id"], "abcdef012345");
    assert_eq!(response["unassigned_track_ids"], json!([]));
    fs::remove_dir_all(root).unwrap();
}
fn fixture() -> (PathBuf, Vec<u8>, String) {
    let root = std::env::temp_dir().join(format!("native-ids-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let mut data = Vec::new();
    for frame in 0..18u64 {
        let candidate = Candidate {
            frame_index: frame,
            class_name: "player".into(),
            score: 0.91,
            bbox_xyxy_norm: [0.2, 0.1, 0.4, 0.65],
            query_index: Some(0),
            touch_probability: Some(0.1),
            head_point_norm: None,
        };
        let row = json!({
            "frame_index":frame,"track_ids_provisional":true,
            "confirmed_athlete_identities":false,"touches_confirmed":false,
            "tracked_players":if (1..15).contains(&frame) {
                json!([{"track_id":7,"detection":candidate,"is_verified_athlete":false}])
            }else{json!([])}
        });
        data.extend(serde_json::to_vec(&row).unwrap());
        data.push(b'\n');
    }
    let digest = format!("{:x}", Sha256::digest(&data));
    let path = root.join("tracked_frames.jsonl");
    fs::write(&path, &data).unwrap();
    (root, data, digest)
}
#[test]
fn reference_request_accepts_selected_rally_scope() {
    let request = json!({
        "project_id":"123456abcdef", "player_id":"abcdef012345", "action":"enroll",
        "rally_run_id":"0123456789abcdef0123456789abcdef"
    });
    assert!(serde_json::from_value::<ReferenceRequest>(request).is_ok());
}
fn context(root: &Path, sha: String, tracks: BTreeMap<u64, Aggregate>) -> Inputs {
    let evidence = EvidenceSource::native(
        "123456abcdef",
        "match.mp4",
        "0123456789abcdef0123456789abcdef",
        &sha,
        SourceStamp {
            bytes: 100,
            mtime_ns: 100,
        },
        FrameRate::Exact {
            numerator: 30,
            denominator: 1,
        },
    )
    .unwrap();
    let known = VerifiedTrackSet::new(evidence.clone(), tracks.keys().copied()).unwrap();
    Inputs {
        evidence,
        known,
        project_id: "123456abcdef".into(),
        video: "match.mp4".into(),
        replay_id: "0123456789abcdef0123456789abcdef".into(),
        source_sha: sha,
        tracks,
        roster: BTreeMap::from([("abcdef012345".into(), "Ильнур".into())]),
        sidecar: root
            .join("native_track_assignments")
            .join("0123456789abcdef0123456789abcdef.json"),
        can_write: true,
    }
}
#[test]
fn review_preview_displays_manual_groups_without_claiming_athlete_identity() {
    let (root, _, sha) = fixture();
    let tracks = index_replay(&root.join("tracked_frames.jsonl"), &sha, 18).unwrap();
    let mut ctx = context(&root, sha, tracks);
    let mut extra = ctx.tracks[&7].clone();
    extra.first = 20;
    extra.last = 25;
    ctx.tracks.insert(8, extra);
    ctx.known = VerifiedTrackSet::new(ctx.evidence.clone(), [7, 8]).unwrap();
    let identity = base_document(&ctx);
    let mut vectors = BTreeMap::new();
    let mut vector = vec![0_f32; 256];
    vector[0] = 1.0;
    vectors.insert(7, vector.clone());
    vectors.insert(8, vector);
    let mut review = ReidReviewDoc::new(&ctx, &"a".repeat(64));
    modify_review(&mut review, &ctx, &identity, "merge", &[7, 8]).unwrap();
    let current =
        review_response(&ctx, &identity, &vectors, &review, "rev".into(), true, 85).unwrap();
    assert_eq!(current["groups"].as_array().unwrap().len(), 1);
    assert_eq!(current["groups"][0]["source"], "manual");
    assert!(current["groups"][0]["similarity"].is_null());
    assert!(current["groups"][0]["average_similarity"].is_number());
    assert_eq!(current["auto_assign_enabled"], false);
    assert_eq!(current["confirmed"], false);
    let without_embeddings = review_response(
        &ctx,
        &identity,
        &BTreeMap::new(),
        &review,
        "rev".into(),
        true,
        85,
    )
    .unwrap();
    assert_eq!(
        without_embeddings["groups"][0]["track_ids"],
        json!(["7", "8"])
    );
    assert_eq!(without_embeddings["embedded_count"], 0);
    assert_eq!(without_embeddings["missing_embedding_count"], 2);
    assert_eq!(without_embeddings["unmatched_ids"], json!([]));
    modify_review(&mut review, &ctx, &identity, "split", &[7, 8]).unwrap();
    let split =
        review_response(&ctx, &identity, &vectors, &review, "rev2".into(), true, 85).unwrap();
    assert!(split["groups"].as_array().unwrap().is_empty());
    assert_eq!(split["suppressed_ids"], json!([7, 8]));
    assert_eq!(split["unmatched_ids"], json!(["7", "8"]));
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn review_scope_excludes_other_rallies_and_orders_groups_by_remaining_size() {
    let mut review = json!({"groups":[
        {"id":"a","track_ids":["1","2","3","4"],"average_similarity":0.2},
        {"id":"b","track_ids":["5","6","7"],"average_similarity":0.2}
    ]});
    let mut vector = vec![0_f32; 256];
    vector[0] = 1.0;
    let vectors = (1..=7).map(|id| (id, vector.clone())).collect();
    let scope = [1, 2, 5, 6, 7].into_iter().collect();
    scope_review_groups(&mut review, &scope, &vectors).unwrap();
    assert_eq!(review["groups"][0]["track_ids"], json!(["5", "6", "7"]));
    assert_eq!(review["groups"][1]["track_ids"], json!(["1", "2"]));
    assert_eq!(review["groups"][1]["average_similarity"], 1.0);
}
#[test]
fn reviewed_group_counts_only_unassigned_tracks_and_recomputes_average() {
    let (root, _, sha) = fixture();
    let tracks = index_replay(&root.join("tracked_frames.jsonl"), &sha, 18).unwrap();
    let mut ctx = context(&root, sha, tracks);
    for id in [8, 9] {
        let mut track = ctx.tracks[&7].clone();
        track.first = id * 20;
        track.last = id * 20 + 10;
        ctx.tracks.insert(id, track);
    }
    ctx.known = VerifiedTrackSet::new(ctx.evidence.clone(), [7, 8, 9]).unwrap();
    let mut identity = base_document(&ctx);
    identity["assignments"]["7"] = json!("abcdef012345");
    let mut review = ReidReviewDoc::new(&ctx, &"a".repeat(64));
    review.groups = vec![vec![7, 8, 9]];
    let mut vector = vec![0_f32; 256];
    vector[0] = 1.0;
    let vectors = BTreeMap::from([(7, vector.clone()), (8, vector.clone()), (9, vector)]);
    let result =
        review_response(&ctx, &identity, &vectors, &review, "rev".into(), true, 85).unwrap();
    assert_eq!(result["groups"][0]["track_ids"], json!(["8", "9"]));
    assert_eq!(result["groups"][0]["average_similarity"], 1.0);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn photo_reference_matches_skip_assigned_excluded_and_overlapping_tracks() {
    let (root, _, sha) = fixture();
    let tracks = index_replay(&root.join("tracked_frames.jsonl"), &sha, 18).unwrap();
    let mut ctx = context(&root, sha, tracks);
    let mut second = ctx.tracks[&7].clone();
    second.first = 20;
    second.last = 30;
    second.visible_spans = vec![(20, 30)];
    ctx.tracks.insert(8, second.clone());
    ctx.tracks.insert(9, second);
    ctx.known = VerifiedTrackSet::new(ctx.evidence.clone(), [7, 8, 9]).unwrap();
    let mut identity = base_document(&ctx);
    identity["assignments"] = json!({"7":"abcdef012345"});
    identity["excluded_ids"] = json!([9]);
    let mut vector = vec![0.0; 256];
    vector[0] = 1.0;
    let vectors = [
        (7, vector.clone()),
        (8, vector.clone()),
        (9, vector.clone()),
    ]
    .into();
    let references = vec![PhotoReference {
        player_id: "abcdef012345".into(),
        vector,
        project_id: ctx.project_id.clone(),
        video: ctx.video.clone(),
        replay_id: ctx.replay_id.clone(),
        track_id: 7,
    }];
    let result = photo_match_candidates(&ctx, &identity, &vectors, &references).unwrap();
    assert_eq!(result["8"][0]["cosine_similarity"], 1.0);
    assert!(result.get("7").is_none());
    assert!(result.get("9").is_none());
    ctx.tracks.get_mut(&8).unwrap().first = 10;
    ctx.tracks.get_mut(&8).unwrap().visible_spans = vec![(10, 30)];
    assert_eq!(
        photo_match_candidates(&ctx, &identity, &vectors, &references).unwrap(),
        json!({})
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reid_player_matches_require_reviewed_nonoverlapping_photo_evidence() {
    let (root, _, sha) = fixture();
    let tracks = index_replay(&root.join("tracked_frames.jsonl"), &sha, 18).unwrap();
    let mut ctx = context(&root, sha, tracks);
    let mut second = ctx.tracks[&7].clone();
    second.first = 20;
    second.last = 30;
    ctx.tracks.insert(8, second);
    ctx.known = VerifiedTrackSet::new(ctx.evidence.clone(), [7, 8]).unwrap();
    let mut identity = base_document(&ctx);
    let mut features = BTreeMap::new();
    let mut vector = vec![0.0_f32; 256];
    vector[0] = 1.0;
    features.insert(7, vector.clone());
    features.insert(8, vector);
    assert_eq!(
        player_match_candidates(&ctx, &identity, &features).unwrap(),
        json!({})
    );
    identity["assignments"] = json!({"7":"abcdef012345"});
    let matches = player_match_candidates(&ctx, &identity, &features).unwrap();
    assert_eq!(matches["8"][0]["name"], "Ильнур");
    assert_eq!(matches["8"][0]["cosine_similarity"], 1.0);
    assert_eq!(matches["8"][0]["reference_track_id"], 7);
    assert!(
        matches.get("7").is_none(),
        "Already assigned IDs are not suggestions"
    );
    // Same-frame player boxes cannot be the same athlete.
    ctx.tracks.get_mut(&8).unwrap().first = 10;
    assert_eq!(
        player_match_candidates(&ctx, &identity, &features).unwrap(),
        json!({})
    );
    ctx.tracks.get_mut(&8).unwrap().first = 20;
    identity["excluded_ids"] = json!([8]);
    assert_eq!(
        player_match_candidates(&ctx, &identity, &features).unwrap(),
        json!({})
    );
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn manual_reid_groups_merge_split_revisions_and_source_owner_checks() {
    let (root, _, sha) = fixture();
    let tracks = index_replay(&root.join("tracked_frames.jsonl"), &sha, 18).unwrap();
    let mut ctx = context(&root, sha, tracks);
    let mut second = ctx.tracks[&7].clone();
    second.first = 20;
    second.last = 28;
    ctx.tracks.insert(8, second.clone());
    ctx.known = VerifiedTrackSet::new(ctx.evidence.clone(), [7, 8]).unwrap();
    let identity = base_document(&ctx);
    let model = "b".repeat(64);
    let mut doc = ReidReviewDoc::new(&ctx, &model);
    assert!(checked_review(&doc, &ctx, &model, &identity).is_ok());
    assert!(modify_review(&mut doc, &ctx, &identity, "merge", &[7, 7]).is_err());
    assert!(modify_review(&mut doc, &ctx, &identity, "merge", &[7, 99]).is_err());
    modify_review(&mut doc, &ctx, &identity, "split", &[7]).unwrap();
    assert_eq!(doc.suppressed_ids, vec![7]);
    modify_review(&mut doc, &ctx, &identity, "merge", &[7, 8]).unwrap();
    assert_eq!(doc.groups, vec![vec![7, 8]]);
    assert!(checked_review(&doc, &ctx, &"c".repeat(64), &identity).is_err());
    let folder = root.join("reviews");
    let sidecar = folder.join("review.json");
    assert!(versioned_store::expect_revision(None, "absent").is_ok());
    versioned_store::write_json_checked(&folder, &sidecar, &json!(doc), MAX_REID_REVIEW_BYTES)
        .unwrap();
    let raw = versioned_store::read_optional_bounded(&sidecar, MAX_REID_REVIEW_BYTES)
        .unwrap()
        .unwrap();
    assert!(versioned_store::expect_revision(Some(&raw), "absent").is_err());
    let revised: ReidReviewDoc = serde_json::from_slice(&raw).unwrap();
    assert_eq!(revised.groups, vec![vec![7, 8]]);
    modify_review(&mut doc, &ctx, &identity, "split", &[7]).unwrap();
    assert!(doc.groups.is_empty());
    assert_eq!(doc.suppressed_ids, vec![7]);
    modify_review(&mut doc, &ctx, &identity, "merge", &[7, 8]).unwrap();
    assert!(doc.suppressed_ids.is_empty());
    ctx.tracks.get_mut(&8).unwrap().first = 10;
    assert!(checked_review(&doc, &ctx, &model, &identity).is_err());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn manual_reid_group_rejects_confirmed_distinct_athletes_without_mutation() {
    let (root, _, sha) = fixture();
    let tracks = index_replay(&root.join("tracked_frames.jsonl"), &sha, 18).unwrap();
    let mut ctx = context(&root, sha, tracks);
    let mut second = ctx.tracks[&7].clone();
    second.first = 20;
    second.last = 28;
    ctx.tracks.insert(8, second);
    ctx.known = VerifiedTrackSet::new(ctx.evidence.clone(), [7, 8]).unwrap();
    ctx.roster
        .insert("fedcba654321".into(), "Another athlete".into());
    let mut identity = base_document(&ctx);
    identity["assignments"] = json!({"7":"abcdef012345","8":"fedcba654321"});
    let model = "b".repeat(64);
    let mut doc = ReidReviewDoc::new(&ctx, &model);
    assert!(modify_review(&mut doc, &ctx, &identity, "merge", &[7, 8]).is_err());
    assert!(doc.groups.is_empty());
    identity["assignments"] = json!({});
    identity["excluded_ids"] = json!([8]);
    assert!(modify_review(&mut doc, &ctx, &identity, "merge", &[7, 8]).is_err());
    assert!(doc.groups.is_empty());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn native_reid_cached_features_are_proposals_with_hard_identity_and_overlap_gates() {
    let (root, _, sha) = fixture();
    let tracks = index_replay(&root.join("tracked_frames.jsonl"), &sha, 18).unwrap();
    let mut ctx = context(&root, sha, tracks);
    let mut second = ctx.tracks[&7].clone();
    second.first = 20;
    second.last = 30;
    second.observed = 11;
    ctx.tracks.insert(8, second.clone());
    ctx.known = VerifiedTrackSet::new(ctx.evidence.clone(), [7, 8]).unwrap();
    let mut features = BTreeMap::new();
    let mut same = vec![0.0_f32; 256];
    same[0] = 1.0;
    features.insert(7, same.clone());
    features.insert(8, same);
    // A poisoned out-of-scope cache entry must never reach comparison.
    features.insert(999, vec![0.0; 256]);
    let mut doc = base_document(&ctx);
    let report = native_reid_report(&ctx, &doc, &features, 85).unwrap();
    assert_eq!(report["confirmed"], false);
    assert_eq!(
        report["similarity_type"],
        "cosine_similarity_not_probability"
    );
    assert_eq!(report["groups"][0]["track_ids"], json!(["7", "8"]));
    assert_eq!(report["embedded_count"], 2);
    doc["assignments"]["7"] = json!("abcdef012345");
    let assigned = native_reid_report(&ctx, &doc, &features, 85).unwrap();
    assert!(assigned["groups"].as_array().unwrap().is_empty());
    ctx.roster
        .insert("fedcba654321".into(), "Second athlete".into());
    doc["assignments"]["7"] = json!("abcdef012345");
    doc["assignments"]["8"] = json!("fedcba654321");
    let forbidden = native_reid_report(&ctx, &doc, &features, 85).unwrap();
    assert!(forbidden["groups"].as_array().unwrap().is_empty());
    doc["assignments"] = json!({});
    ctx.tracks.get_mut(&8).unwrap().first = 10;
    let simultaneous = native_reid_report(&ctx, &doc, &features, 85).unwrap();
    assert!(simultaneous["groups"].as_array().unwrap().is_empty());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn native_reid_missing_vectors_remain_unmatched_and_never_assigned() {
    let (root, _, sha) = fixture();
    let tracks = index_replay(&root.join("tracked_frames.jsonl"), &sha, 18).unwrap();
    let ctx = context(&root, sha, tracks);
    let empty = native_reid_report(&ctx, &base_document(&ctx), &BTreeMap::new(), 85).unwrap();
    assert_eq!(empty["embedded_count"], 0);
    assert_eq!(empty["missing_visual_ids"], json!(["7"]));
    assert_eq!(empty["unmatched_ids"], json!(["7"]));
    assert!(native_reid_report(&ctx, &base_document(&ctx), &BTreeMap::new(), 100).is_err());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn native_ball_extrema_from_sha_bound_cpu_frames_refuse_model_switches() {
    let root = std::env::temp_dir().join(format!("motion-native-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let path = root.join("tracked_frames.jsonl");
    let mut bytes = Vec::new();
    for frame in 0_u64..50 {
        let x = if frame <= 25 {
            0.15 + frame as f64 * 0.01
        } else {
            0.40 - (frame - 25) as f64 * 0.01
        };
        let ball = Candidate {
            frame_index: frame,
            class_name: "ball".into(),
            score: 0.95,
            bbox_xyxy_norm: [x, 0.40, x + 0.01, 0.42],
            query_index: None,
            touch_probability: None,
            head_point_norm: None,
        };
        let json = json!({
            "frame_index":frame,"tracked_players":[],
            "selected_ball":{"source_model":"vballnet","detection":ball},
            "track_ids_provisional":true,
            "confirmed_athlete_identities":false,"touches_confirmed":false
        });
        bytes.extend(serde_json::to_vec(&json).unwrap());
        bytes.push(b'\n');
    }
    fs::write(&path, &bytes).unwrap();
    let sha = format!("{:x}", Sha256::digest(&bytes));
    let (points, source) =
        extract_native_ball_trajectory(&path, 50, &sha, 0, 50, 640, 360).unwrap();
    assert_eq!(points.len(), 50);
    assert_eq!(source.as_deref(), Some("vballnet"));
    let changes = ball_motion_extrema(&points, 8).unwrap();
    assert_eq!(changes.len(), 1);
    assert!(!changes[0].athlete_identity_verified);
    assert!(extract_native_ball_trajectory(&path, 50, &"0".repeat(64), 0, 50, 640, 360).is_err());
    assert!(extract_native_ball_trajectory(&path, 50, &sha, 0, 1201, 640, 360).is_err());
    let mut altered = bytes;
    let index = altered.windows(8).position(|w| w == b"vballnet").unwrap();
    altered[index..index + 8].copy_from_slice(b"ravel   ");
    fs::write(&path, altered).unwrap();
    assert!(extract_native_ball_trajectory(&path, 50, &sha, 0, 50, 640, 360).is_err());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn native_continuity_sha_status_and_finite_gap_are_bounded() {
    let (root, data, sha) = fixture();
    let path = root.join("tracked_frames.jsonl");
    let tracks = index_replay(&path, &sha, 18).unwrap();
    let ctx = context(&root, sha.clone(), tracks);
    let report =
        continuity_from_native_replay(&path, 18, &sha, &ctx.known, &BTreeSet::new(), 4.0).unwrap();
    assert_eq!(report.raw_track_ids, 1);
    assert_eq!(report.candidate_groups, 1);
    assert!(!report.confirmed);
    assert!(continuity_from_native_replay(
        &path,
        18,
        &"a".repeat(64),
        &ctx.known,
        &BTreeSet::new(),
        4.0
    )
    .is_err());
    assert!(
        continuity_from_native_replay(&path, 18, &sha, &ctx.known, &BTreeSet::new(), 4.1).is_err()
    );
    let mut modified = data;
    modified[0] = b' ';
    fs::write(&path, modified).unwrap();
    assert!(
        continuity_from_native_replay(&path, 18, &sha, &ctx.known, &BTreeSet::new(), 4.0).is_err()
    );
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn hash_checked_track_index_and_tampering_protection() {
    let (root, mut data, sha) = fixture();
    let path = root.join("tracked_frames.jsonl");
    let tracks = index_replay(&path, &sha, 18).unwrap();
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[&7].observed, 14);
    assert_eq!(tracks[&7].first, 1);
    assert_eq!(tracks[&7].last, 14);
    assert_eq!(tracks[&7].best_frame, 1);
    assert!(index_replay(&path, &"0".repeat(64), 18).is_err());
    data[0] = b' ';
    fs::write(&path, &data).unwrap();
    assert!(index_replay(&path, &sha, 18).is_err());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn track_index_cache_reuses_identity_and_invalidates_on_same_size_edits() {
    let (root, mut data, sha) = fixture();
    let path = root.join("tracked_frames.jsonl");
    let first = index_replay(&path, &sha, 18).unwrap();
    // Repeated UI requests must return the same verified aggregates.
    for _ in 0..8 {
        assert_eq!(index_replay(&path, &sha, 18).unwrap(), first);
    }
    // A same-size edit can share the mtime clock tick; the platform
    // change stamp must still force re-verification and reject it.
    data[0] = b' ';
    fs::write(&path, &data).unwrap();
    assert!(index_replay(&path, &sha, 18).is_err());
    data[0] = b'{';
    fs::write(&path, &data).unwrap();
    assert_eq!(index_replay(&path, &sha, 18).unwrap(), first);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn isolated_project_edits_reject_stale_revision_reassignment_and_excluded_track() {
    use std::process::Command;
    if Command::new("ffmpeg").arg("-version").output().is_err() {
        return;
    }
    let (root, source_bytes, source_sha) = fixture();
    let repo = root.join("repo");
    let data = root.join("sandbox");
    let id = "123456abcdef";
    let video = "match.mp4";
    let replay = "0123456789abcdef0123456789abcdef";
    fs::create_dir_all(repo.join("project")).unwrap();
    let source_video = repo.join("project").join(video);
    assert!(Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=64x64:rate=30:duration=0.6",
            "-an",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-y"
        ])
        .arg(&source_video)
        .status()
        .unwrap()
        .success());
    let replay_dir = data
        .join("projects")
        .join(id)
        .join("native_cpu_replays")
        .join(replay);
    fs::create_dir_all(&replay_dir).unwrap();
    fs::write(replay_dir.join("tracked_frames.jsonl"), &source_bytes).unwrap();
    storage::write_json_atomic(
        &data.join("projects").join("projects.json"),
        &json!({
            "schema":1,"projects":[{"id":id,"name":"Test","videos":[video],
                "player_ids":["abcdef012345","fedcba654321"]}]
        }),
    )
    .unwrap();
    storage::write_json_atomic(
        &data.join("players").join("registry.json"),
        &json!({
            "schema":1,"players":[
                {"id":"abcdef012345","name":"Ильнур","created_by":"manual"},
                {"id":"fedcba654321","name":"Ирек","created_by":"manual"}]
        }),
    )
    .unwrap();
    let state = AppState {
        repository_root: repo.clone(),
        data_root: data.clone(),
        writes_enabled: true,
        montage: crate::montage::MontageManager::new(),
        preparation: crate::preparation::PreparationManager::new(),
        preview_limit: tokio::sync::Semaphore::new(1),
        touch_limit: tokio::sync::Semaphore::new(1),
        continuity_limit: tokio::sync::Semaphore::new(1),
        reid_limit: tokio::sync::Semaphore::new(1),
        identity_limit: tokio::sync::Semaphore::new(1),
        statistics_limit: tokio::sync::Semaphore::new(1),
        marks_limit: tokio::sync::Semaphore::new(1),
        quick_reel: crate::quick_reel::QuickReelManager::new(),
        inference_limit: tokio::sync::Semaphore::new(1),
        full_inference: crate::inference_full::FullInferenceManager::new(),
    };
    let prepared = preparation::plan_for(&state, id, video).unwrap();
    let (size, mtime) = inference_stream::source_stamp(prepared.ready_path().unwrap()).unwrap();
    storage::write_json_atomic(
        &replay_dir.join("manifest.json"),
        &json!({
            "schema":"volley-native-cpu-tracking/1","state":"provisional_tracklets_rebuilt",
            "project_id":id,"replay_id":replay,"video":video,
            "source_identity":{"bytes":size,"mtime_ns":mtime},
            "fps":30,"frames":18,"file":"tracked_frames.jsonl","sha256":source_sha,
            "track_ids_provisional":true,"athlete_identities_confirmed":false,
            "touches_confirmed":false,"cpu_processing_complete":false
        }),
    )
    .unwrap();
    let query = TrackQuery {
        project_id: id.into(),
        video: video.into(),
        replay_id: replay.into(),
        rally_run_id: None,
        offset: 0,
        limit: 10,
        view: None,
        view_player_id: None,
    };
    let original = read_internal(&state, &query).unwrap();
    assert_eq!(original["total_tracks"], 1);
    assert_eq!(original["revision"], "absent");
    assert_eq!(original["source_kind"], "native_gpu_cpu_replay");
    assert_eq!(
        original["evidence_source"]["tracked_frames_sha256"],
        source_sha
    );
    assert_eq!(original["evidence_source"]["frame_rate"]["numerator"], 30);
    let assign = TrackEdit {
        project_id: id.into(),
        video: video.into(),
        replay_id: replay.into(),
        action: "assign".into(),
        track_ids: vec![7],
        rally_run_id: None,
        player_id: Some("abcdef012345".into()),
        expected_revision: "absent".into(),
        offset: 0,
        limit: 10,
    };
    let saved = change(&state, &assign).unwrap();
    assert_eq!(saved["assigned_count"], 1);
    assert_eq!(saved["tracks"][0]["assigned_player_name"], "Ильнур");
    assert_eq!(saved["evidence_source"], original["evidence_source"]);
    let sidecar = data
        .join("projects")
        .join(id)
        .join("native_track_assignments")
        .join(format!("{replay}.json"));
    let stored: Value = serde_json::from_slice(&fs::read(sidecar).unwrap()).unwrap();
    assert!(stored.get("evidence_source").is_none());
    assert_eq!(stored["source_replay_sha256"], source_sha);
    assert_eq!(read_internal(&state, &query).unwrap()["assigned_count"], 1);
    assert_eq!(
        change(&state, &assign).unwrap_err().0,
        axum::http::StatusCode::CONFLICT
    );
    let different = TrackEdit {
        player_id: Some("fedcba654321".into()),
        expected_revision: saved["revision"].as_str().unwrap().into(),
        ..assign.clone()
    };
    assert_eq!(
        change(&state, &different).unwrap_err().0,
        axum::http::StatusCode::CONFLICT
    );
    let excluded = change(
        &state,
        &TrackEdit {
            action: "exclude".into(),
            player_id: None,
            expected_revision: saved["revision"].as_str().unwrap().into(),
            ..assign.clone()
        },
    )
    .unwrap();
    assert_eq!(excluded["excluded_count"], 1);
    assert_eq!(excluded["assigned_count"], 0);
    assert!(change(
        &state,
        &TrackEdit {
            expected_revision: excluded["revision"].as_str().unwrap().into(),
            ..assign.clone()
        }
    )
    .is_err());
    let restored = change(
        &state,
        &TrackEdit {
            action: "restore".into(),
            player_id: None,
            expected_revision: excluded["revision"].as_str().unwrap().into(),
            ..assign.clone()
        },
    )
    .unwrap();
    assert_eq!(restored["excluded_count"], 0);
    assert_eq!(
        fs::read(replay_dir.join("tracked_frames.jsonl")).unwrap(),
        source_bytes
    );
    assert!(verified_reference_source(
        &state,
        &ReferenceRequest {
            project_id: id.into(),
            player_id: "abcdef012345".into(),
            action: "enroll".into(),
            expected_revision: Some("absent".into()),
            video: Some(video.into()),
            replay_id: Some(replay.into()),
            rally_run_id: None,
            photo_index: 0,
            track_id: Some(7),
            expected_source_sha256: Some(source_sha.clone()),
            reference_id: None
        }
    )
    .is_err());
    let reassigned = change(
        &state,
        &TrackEdit {
            expected_revision: restored["revision"].as_str().unwrap().into(),
            ..assign.clone()
        },
    )
    .unwrap();
    assert_eq!(reassigned["assigned_count"], 1);
    assert!(!repo.join("results").exists());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let shared = Arc::new(state);
    runtime.block_on(async {
        let jpeg = photo(
            State(shared.clone()),
            Query(PhotoQuery {
                project_id: id.into(),
                video: video.into(),
                replay_id: replay.into(),
                track_id: 7,
                rally_run_id: None,
                photo_index: 1,
            }),
        )
        .await
        .unwrap();
        assert_eq!(
            jpeg.headers()[axum::http::header::CONTENT_TYPE],
            "image/jpeg"
        );
        let bytes = axum::body::to_bytes(jpeg.into_body(), 3_000_000)
            .await
            .unwrap();
        assert!(bytes.starts_with(&[0xff, 0xd8]));
        assert!(bytes.len() > 100);
        let read_album = ReferenceRequest {
            project_id: id.into(),
            player_id: "abcdef012345".into(),
            action: "read".into(),
            expected_revision: None,
            video: None,
            replay_id: None,
            rally_run_id: None,
            photo_index: 0,
            track_id: None,
            expected_source_sha256: None,
            reference_id: None,
        };
        let empty = references(
            State(shared.clone()),
            HeaderMap::new(),
            Json(read_album.clone()),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(empty["revision"], "absent");
        assert_eq!(empty["count"], 0);
        let enroll = ReferenceRequest {
            action: "enroll".into(),
            expected_revision: Some("absent".into()),
            video: Some(video.into()),
            replay_id: Some(replay.into()),
            track_id: Some(7),
            photo_index: 1,
            expected_source_sha256: Some(source_sha.clone()),
            ..read_album.clone()
        };
        assert!(references(
            State(shared.clone()),
            HeaderMap::new(),
            Json(ReferenceRequest {
                rally_run_id: Some("11111111111111111111111111111111".into()),
                ..enroll.clone()
            }),
        )
        .await
        .is_err());
        let album = references(
            State(shared.clone()),
            HeaderMap::new(),
            Json(enroll.clone()),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(album["count"], 1);
        assert_eq!(album["photos"][0]["manually_enrolled"], true);
        assert_eq!(album["photos"][0]["track_id"], 7);
        assert_eq!(album["photos"][0]["source_replay_sha256"], source_sha);
        let photo_id = album["photos"][0]["id"].as_str().unwrap().to_owned();
        let stored_photo = reference_photo(
            State(shared.clone()),
            Query(ReferencePhotoQuery {
                project_id: Some(id.into()),
                player_id: "abcdef012345".into(),
                reference_id: photo_id.clone(),
            }),
        )
        .await
        .unwrap();
        let copied = axum::body::to_bytes(stored_photo.into_body(), MAX_PHOTO_BYTES)
            .await
            .unwrap();
        assert!(copied.starts_with(&[0xff, 0xd8]));
        assert!(copied.ends_with(&[0xff, 0xd9]));
        assert_eq!(copied, bytes);
        assert_eq!(
            format!("{:x}", Sha256::digest(&copied)),
            album["photos"][0]["image_sha256"].as_str().unwrap()
        );
        let album_path = data.join("players/native_reference_photos/abcdef012345");
        assert!(album_path.join(format!("ref-{photo_id}.jpg")).is_file());
        if std::env::var("VOLLEY_OPENVINO_C_LIB").is_ok() {
            let fixture_models = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join("models/openvino");
            fs::create_dir_all(repo.join("models/openvino")).unwrap();
            for suffix in ["xml", "bin"] {
                let filename = format!("person-reidentification-retail-0287.{suffix}");
                fs::copy(fixture_models.join(&filename), repo.join("models/openvino").join(&filename)).unwrap();
            }
            let run_id = "11111111111111111111111111111111";
            let touch_id = "22222222222222222222222222222222";
            let rally_dir = data.join("projects").join(id).join("native_cpu_rallies").join(run_id);
            let touch_dir = data.join("projects").join(id).join("native_cpu_touch_runs").join(touch_id);
            fs::create_dir_all(&rally_dir).unwrap();
            fs::create_dir_all(&touch_dir).unwrap();
            let candidate_id = format!("{:x}", Sha256::digest(format!("{replay}:{touch_id}:0:17").as_bytes()))[..24].to_string();
            let rally_row = json!({"candidate_id":candidate_id,"detected_start_frame":0,"detected_end_frame":17,
                "ball_observations":18,"ball_coverage":1.0,"motion_supported_touch_proposals":1,
                "first_touch_frame":3,"last_touch_frame":3,"max_touch_probability":0.9,
                "classification":"provisional_rally","authoritative_rally":false,
                "athlete_identities_verified":false,"serve_verified":false,"reviewed":false});
            let rally_bytes = format!("{}\n", rally_row).into_bytes();
            let rally_sha = format!("{:x}", Sha256::digest(&rally_bytes));
            let touch_sha = format!("{:x}", Sha256::digest([]));
            fs::write(rally_dir.join("rally_candidates.jsonl"), rally_bytes).unwrap();
            fs::write(touch_dir.join("touch_candidates.jsonl"), []).unwrap();
            let identity = json!({"bytes":size,"mtime_ns":mtime});
            let touch = json!({"schema":"volley-native-cpu-touch-proposals/1","state":"motion_gated_touch_proposals",
                "touch_id":touch_id,"project_id":id,"video":video,"source_replay_id":replay,
                "source_replay_sha256":source_sha,"source_identity":identity,"frames":18,"width":64,"height":64,
                "file":"touch_candidates.jsonl","touch_candidates":0,"contacts_authoritatively_confirmed":false,
                "athlete_identities_verified":false,"cpu_processing_complete":false,"sha256":touch_sha});
            let rally = json!({"schema":"volley-native-cpu-rally-proposals/1","state":"provisional_rally_segments",
                "project_id":id,"video":video,"rally_run_id":run_id,"source_replay_id":replay,
                "source_replay_sha256":source_sha,"source_touch_id":touch_id,"source_touch_sha256":touch_sha,
                "source_identity":identity,"width":64,"height":64,"fps":30,"frames":18,
                "settings":{"suggested_context_frames":0},"touch_candidates_ingested":0,"rally_candidates":1,
                "file":"rally_candidates.jsonl","sha256":rally_sha,"rallies_authoritatively_confirmed":false,
                "athlete_identities_verified":false,"action_classification_complete":false,"montage_ready":false,
                "cpu_processing_complete":false});
            fs::write(touch_dir.join("manifest.json"), serde_json::to_vec(&touch).unwrap()).unwrap();
            fs::write(rally_dir.join("manifest.json"), serde_json::to_vec(&rally).unwrap()).unwrap();
            let index_path = album_path.join("index.json");
            let original_index = fs::read(&index_path).unwrap();
            let mut other_project: Value = serde_json::from_slice(&original_index).unwrap();
            other_project["photos"][0]["project_id"] = json!("bbbbbbbbbbbb");
            fs::write(&index_path, serde_json::to_vec(&other_project).unwrap()).unwrap();
            let unassigned = change(&shared, &TrackEdit { action:"unassign".into(), player_id:None,
                expected_revision:reassigned["revision"].as_str().unwrap().into(), ..assign.clone() }).unwrap();
            let compare_req: ReidReviewRequest = serde_json::from_value(json!({"project_id":id,"video":video,
                "replay_id":replay,"rally_run_id":run_id,"expected_source_sha256":source_sha,
                "action":"compare_photos"})).unwrap();
            let result = reid_review(State(shared.clone()), HeaderMap::new(), Json(compare_req)).await.unwrap().0;
            assert_eq!(result["schema"], "volley-native-player-photo-comparison/1");
            assert_eq!(result["photo_count"], 1);
            assert_eq!(result["auto_assign_enabled"], false);
            assert_eq!(result["player_matches"]["7"][0]["player_id"], "abcdef012345");
            let score = result["player_matches"]["7"][0]["cosine_similarity"].as_f64().unwrap();
            println!("Synthetic reference cosine: {score}");
            assert!(score > 0.0 && score <= 1.0);
            change(&shared, &TrackEdit { expected_revision:unassigned["revision"].as_str().unwrap().into(), ..assign.clone() }).unwrap();
            fs::write(&index_path, original_index).unwrap();
        }
        assert!(references(
            State(shared.clone()),
            HeaderMap::new(),
            Json(enroll.clone())
        )
        .await
        .is_err());
        assert!(references(
            State(shared.clone()),
            HeaderMap::new(),
            Json(ReferenceRequest {
                expected_revision: Some(album["revision"].as_str().unwrap().into()),
                expected_source_sha256: Some("0".repeat(64)),
                ..enroll.clone()
            })
        )
        .await
        .is_err());
        let undone = references(
            State(shared.clone()),
            HeaderMap::new(),
            Json(ReferenceRequest {
                action: "undo".into(),
                expected_revision: Some(album["revision"].as_str().unwrap().into()),
                video: None,
                replay_id: None,
                track_id: None,
                expected_source_sha256: None,
                reference_id: Some(photo_id.clone()),
                ..read_album.clone()
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(undone["count"], 0);
        assert_ne!(undone["revision"], album["revision"]);
        assert!(!album_path.join(format!("ref-{photo_id}.jpg")).exists());
        assert!(reference_photo(
            State(shared.clone()),
            Query(ReferencePhotoQuery {
                project_id: Some(id.into()),
                player_id: "abcdef012345".into(),
                reference_id: photo_id,
            })
        )
        .await
        .is_err());
        assert!(references(
            State(shared.clone()),
            HeaderMap::new(),
            Json(ReferenceRequest {
                action: "undo".into(),
                expected_revision: Some(album["revision"].as_str().unwrap().into()),
                reference_id: Some("0".repeat(24)),
                ..read_album.clone()
            })
        )
        .await
        .is_err()); // This optional integration test runs the *actual* native C-ABI
                    // model on a verified synthetic CPU replay and a real FFmpeg crop.
                    // It cannot attribute an athlete from the resulting vector.
        if std::env::var("VOLLEY_OPENVINO_C_LIB").is_ok() {
            let fixture_models = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join("models/openvino");
            let local_models = shared.repository_root.join("models/openvino");
            fs::create_dir_all(&local_models).unwrap();
            for suffix in ["xml", "bin"] {
                let filename = format!("person-reidentification-retail-0287.{suffix}");
                fs::copy(fixture_models.join(&filename), local_models.join(&filename)).unwrap();
            }
            let output = embeddings(
                State(shared.clone()),
                HeaderMap::new(),
                Json(EmbeddingRequest {
                    project_id: id.into(),
                    video: video.into(),
                    replay_id: replay.into(),
                    rally_run_id: None,
                    track_ids: vec![7],
                    transient: false,
                    expected_source_sha256: source_sha.clone(),
                }),
            )
            .await
            .unwrap()
            .0;
            assert_eq!(output["schema"], "volley-native-reid-features/1");
            assert_eq!(output["source_replay_sha256"], source_sha);
            assert_eq!(output["embedding_count"], 1);
            assert_eq!(output["features"][0]["track_id"], 7);
            assert_eq!(output["features"][0]["dimensions"], 256);
            assert_eq!(output["identities_confirmed"], false);
            assert_eq!(output["automatic_reid_binding"], false);
            let vec = output["features"][0]["vector"].as_array().unwrap();
            assert_eq!(vec.len(), 256);
            let norm = vec.iter().map(|x| x.as_f64().unwrap().powi(2)).sum::<f64>();
            assert!((norm - 1.0).abs() < 0.00001);
            assert_eq!(output["cache_hits"], 0);
            assert_eq!(output["computed_count"], 1);
            assert_eq!(output["cache_persisted"], true);
            let again = embeddings(
                State(shared.clone()),
                HeaderMap::new(),
                Json(EmbeddingRequest {
                    project_id: id.into(),
                    video: video.into(),
                    replay_id: replay.into(),
                    rally_run_id: None,
                    track_ids: vec![7],
                    transient: false,
                    expected_source_sha256: source_sha.clone(),
                }),
            )
            .await
            .unwrap()
            .0;
            assert_eq!(again["cache_hits"], 1);
            assert_eq!(again["computed_count"], 0);
            assert_eq!(again["cache_persisted"], false);
            assert_eq!(
                again["features"][0]["vector"],
                output["features"][0]["vector"]
            );
            assert_eq!(again["model_fingerprint"], output["model_fingerprint"]);
            // A stale model-to-replay reference must be refused before
            // decoding or invoking the installed inference library.
            assert!(embeddings(
                State(shared.clone()),
                HeaderMap::new(),
                Json(EmbeddingRequest {
                    project_id: id.into(),
                    video: video.into(),
                    replay_id: replay.into(),
                    rally_run_id: None,
                    track_ids: vec![7],
                    transient: false,
                    expected_source_sha256: "0".repeat(64),
                })
            )
            .await
            .is_err());
        }
        let not_found = photo(
            State(shared),
            Query(PhotoQuery {
                project_id: id.into(),
                video: video.into(),
                replay_id: replay.into(),
                track_id: 999,
                rally_run_id: None,
                photo_index: 0,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(not_found.0, axum::http::StatusCode::BAD_REQUEST);
    });
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reference_album_schema_rejects_stale_or_unreviewed_photos() {
    let athlete = "abcdef012345";
    let initial = references_doc(None, athlete).unwrap();
    assert_eq!(initial["photos"], json!([]));
    let valid = json!({"schema":"volley-native-player-references/1",
    "player_id":athlete,"photos":[{
        "id":"0123456789ab0123456789ab",
        "filename":"ref-0123456789ab0123456789ab.jpg",
        "image_sha256":"a".repeat(64),
        "source_replay_sha256":"b".repeat(64),
        "track_id":7,"manually_enrolled":true
    }]});
    assert!(references_doc(Some(&serde_json::to_vec(&valid).unwrap()), athlete).is_ok());
    let mut wrong = valid.clone();
    wrong["photos"][0]["manually_enrolled"] = json!(false);
    assert!(references_doc(Some(&serde_json::to_vec(&wrong).unwrap()), athlete).is_err());
    let mut wrong = valid.clone();
    wrong["photos"][0]["filename"] = json!("../secret.jpg");
    assert!(references_doc(Some(&serde_json::to_vec(&wrong).unwrap()), athlete).is_err());
    let mut wrong = valid.clone();
    wrong["photos"]
        .as_array_mut()
        .unwrap()
        .push(valid["photos"][0].clone());
    assert!(references_doc(Some(&serde_json::to_vec(&wrong).unwrap()), athlete).is_err());
}
#[test]
fn photo_crop_is_bounded_and_rejects_corrupted_geometry() {
    assert_eq!(crop_box([0.2, 0.1, 0.4, 0.7], 640, 360).unwrap().len(), 4);
    let [x, y, w, h] = crop_box([0.0, 0.0, 1.0, 1.0], 640, 360).unwrap();
    assert_eq!((x, y, w, h), (0, 0, 640, 360));
    assert!(crop_box([0.2, 0.4, 0.1, 0.9], 640, 360).is_err());
    assert!(crop_box([-0.1, 0.0, 0.5, 0.6], 640, 360).is_err());
    assert!(crop_box([0.2, 0.1, 0.4, f64::NAN], 640, 360).is_err());
    assert!(crop_box([0.2, 0.1, 0.4, 0.7], 32, 360).is_err());
}

#[test]
fn track_gallery_filters_owners_before_pagination() {
    let (root, _, sha) = fixture();
    let tracks = index_replay(&root.join("tracked_frames.jsonl"), &sha, 18).unwrap();
    let mut ctx = context(&root, sha, tracks);
    let sample = ctx.tracks.get(&7).unwrap().clone();
    ctx.tracks.insert(8, sample.clone());
    ctx.tracks.insert(9, sample);
    ctx.known = VerifiedTrackSet::new(ctx.evidence.clone(), [7, 8, 9]).unwrap();
    let mut doc = base_document(&ctx);
    doc["assignments"]["7"] = json!("abcdef012345");
    doc["assignments"]["8"] = json!("abcdef012345");
    let unassigned = filtered_page(&ctx, &doc, "revision", 1, 1, Some("unassigned"), None).unwrap();
    assert_eq!(unassigned["player_track_counts"]["abcdef012345"], 2);
    assert_eq!(unassigned["filtered_total"], 1);
    assert_eq!(unassigned["unassigned_track_ids"], json!([9]));
    let player = filtered_page(
        &ctx,
        &doc,
        "revision",
        0,
        1,
        Some("player"),
        Some("abcdef012345"),
    )
    .unwrap();
    assert_eq!(player["filtered_total"], 2);
    assert_eq!(player["tracks"][0]["track_id"], 7);
    assert!(filtered_page(
        &ctx,
        &doc,
        "revision",
        0,
        1,
        Some("player"),
        Some("unknown")
    )
    .is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn native_manual_assignments_are_replay_scoped_and_roster_checked() {
    let (root, _, sha) = fixture();
    let tracks = index_replay(&root.join("tracked_frames.jsonl"), &sha, 18).unwrap();
    let ctx = context(&root, sha.clone(), tracks);
    let base = validated(None, &ctx).unwrap();
    assert_eq!(
        page(&ctx, &base, "absent", 0, 40).unwrap()["assigned_count"],
        0
    );
    let mut doc = base.clone();
    doc["assignments"]["7"] = json!("abcdef012345");
    assert_eq!(
        validated(Some(&serde_json::to_vec(&doc).unwrap()), &ctx).unwrap()["assignments"]["7"],
        "abcdef012345"
    );
    let trusted_page = page(&ctx, &doc, "revision", 0, 40).unwrap();
    assert_eq!(trusted_page["assigned_count"], 1);
    assert_eq!(
        trusted_page["tracks"][0]["assigned_player_id"],
        "abcdef012345"
    );
    let mut wrong = doc.clone();
    wrong["assignments"]["7"] = json!("fedcba654321");
    assert!(validated(Some(&serde_json::to_vec(&wrong).unwrap()), &ctx).is_err());
    let mut wrong = doc.clone();
    wrong["source_replay_sha256"] = json!("0".repeat(64));
    assert!(validated(Some(&serde_json::to_vec(&wrong).unwrap()), &ctx).is_err());
    let mut excluded = doc.clone();
    excluded["excluded_ids"] = json!([7]);
    assert!(validated(Some(&serde_json::to_vec(&excluded).unwrap()), &ctx).is_err());
    fs::remove_dir_all(root).unwrap();
}
