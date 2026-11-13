use super::*;
#[derive(Clone)]
pub(super) struct TrackIndex {
    path: PathBuf,
    sha: String,
    rally_sha: Option<String>,
    frames: u64,
    size: u64,
    modified: Option<SystemTime>,
    change: Option<(u64, u64, i64, i64)>,
    tracks: BTreeMap<u64, Aggregate>,
}
static TRACK_INDICES: OnceLock<Mutex<Vec<TrackIndex>>> = OnceLock::new();
type TrackObservations = BTreeMap<u64, Aggregate>;
pub(super) fn index_replay(
    path: &Path,
    expected_sha: &str,
    frames: u64,
) -> Result<BTreeMap<u64, Aggregate>, String> {
    index_with_scope(path, expected_sha, frames, None).map(|(all, _)| all)
}

pub(super) fn index_rally_replay(
    path: &Path,
    expected_sha: &str,
    frames: u64,
    rally_sha: &str,
    windows: &crate::native_rally_preview::RallyWindows,
) -> Result<(TrackObservations, TrackObservations), String> {
    let (selected, full) =
        index_with_scope(path, expected_sha, frames, Some((rally_sha, windows)))?;
    Ok((
        selected,
        full.ok_or("Full source index missing after scoped tracking scan")?,
    ))
}

fn index_with_scope(
    path: &Path,
    expected_sha: &str,
    frames: u64,
    scope: Option<(&str, &crate::native_rally_preview::RallyWindows)>,
) -> Result<(TrackObservations, Option<TrackObservations>), String> {
    if frames == 0 || frames > 48 * 3600 * 30 || !valid_sha(expected_sha) {
        return Err("Unsupported native track replay length or SHA-256".into());
    }
    if let Some((sha, _)) = scope {
        if !valid_sha(sha) {
            return Err("Invalid selected rally SHA-256".into());
        }
    }
    let meta = fs::metadata(path).map_err(|e| format!("Track replay metadata: {e}"))?;
    let modified = meta.modified().ok();
    let change = volley_media::preparation::file_change_stamp(path, &meta);
    let store = TRACK_INDICES.get_or_init(|| Mutex::new(Vec::new()));
    if change.is_some() {
        let cached = store.lock().map_err(|_| "Track index lock is poisoned")?;
        let same_file = |item: &TrackIndex| {
            item.path == path
                && item.sha == expected_sha
                && item.frames == frames
                && item.size == meta.len()
                && item.modified == modified
                && item.change == change
        };
        if let Some(hit) = cached
            .iter()
            .find(|item| same_file(item) && item.rally_sha.as_deref() == scope.map(|(sha, _)| sha))
        {
            if scope.is_none() {
                return Ok((hit.tracks.clone(), None));
            }
            // A scoped caller also needs the complete verified index for
            // preexisting manual assignments. Cache hits must supply both.
            if let Some(full) = cached
                .iter()
                .find(|item| same_file(item) && item.rally_sha.is_none())
            {
                return Ok((hit.tracks.clone(), Some(full.tracks.clone())));
            }
        }
    }
    let (tracks, full) = scan_with_scope(
        path,
        expected_sha,
        frames,
        scope.map(|(_, windows)| windows),
    )?;
    if change.is_some() {
        let mut cache = store.lock().map_err(|_| "Track index lock is poisoned")?;
        // One pass produces both the full source index (for persisted manual
        // link validation) and the rally-only review index. Keep both warm.
        let entry = |rally_sha: Option<&str>, tracks: BTreeMap<u64, Aggregate>| TrackIndex {
            path: path.to_owned(),
            sha: expected_sha.to_owned(),
            rally_sha: rally_sha.map(str::to_owned),
            frames,
            size: meta.len(),
            modified,
            change,
            tracks,
        };
        if let Some(ref all) = full {
            cache.retain(|row| row.path != path || row.rally_sha.is_some());
            cache.push(entry(None, all.clone()));
        }
        cache.retain(|row| {
            row.path != path || row.rally_sha.as_deref() != scope.map(|(sha, _)| sha)
        });
        cache.push(entry(scope.map(|(sha, _)| sha), tracks.clone()));
        while cache.len() > 8 {
            cache.remove(0);
        }
    }
    Ok((tracks, full))
}
fn observe_track(
    tracks: &mut BTreeMap<u64, Aggregate>,
    tracked: &Tracked,
    frame_idx: u64,
) -> Result<(), String> {
    if !tracks.contains_key(&tracked.track_id) && tracks.len() >= MAX_TRACKS {
        return Err("Too many distinct native Track IDs".into());
    }
    let record = tracks.entry(tracked.track_id).or_insert_with(|| Aggregate {
        first: frame_idx,
        last: frame_idx,
        observed: 0,
        best_frame: frame_idx,
        best_score: 0.0,
        best_box: [0.0; 4],
        photo_samples: Vec::new(),
        visible_spans: Vec::new(),
    });
    if let Some((_, end)) = record.visible_spans.last_mut() {
        if *end == frame_idx || end.saturating_add(1) == frame_idx {
            *end = frame_idx;
        } else {
            record.visible_spans.push((frame_idx, frame_idx));
        }
    } else {
        record.visible_spans.push((frame_idx, frame_idx));
    }
    record.last = frame_idx;
    record.observed += 1;
    if record.photo_samples.is_empty() || frame_idx.is_multiple_of(6) {
        let sample = PhotoSample {
            frame: frame_idx,
            bbox: tracked.detection.bbox_xyxy_norm,
        };
        if record.photo_samples.len() < 12 {
            record.photo_samples.push(sample);
        } else {
            let slot = (frame_idx / 6 % 12) as usize;
            record.photo_samples[slot] = sample;
        }
    }
    if tracked.detection.score > record.best_score {
        record.best_score = tracked.detection.score;
        record.best_frame = frame_idx;
        record.best_box = tracked.detection.bbox_xyxy_norm;
    }
    Ok(())
}
fn scan_with_scope(
    path: &Path,
    expected_sha: &str,
    frames: u64,
    windows: Option<&crate::native_rally_preview::RallyWindows>,
) -> Result<(TrackObservations, Option<TrackObservations>), String> {
    let mut reader = BoundedReader::open(path)?;
    let mut full = BTreeMap::<u64, Aggregate>::new();
    let mut scoped = windows.map(|_| BTreeMap::<u64, Aggregate>::new());
    for frame_idx in 0..frames {
        let bytes = reader
            .line()?
            .ok_or("Native Track ID frame source is truncated")?;
        let frame: Frame = serde_json::from_slice(&bytes)
            .map_err(|e| format!("Invalid native Track ID row: {e}"))?;
        if frame.frame_index != frame_idx
            || !frame.track_ids_provisional
            || frame.confirmed_athlete_identities
            || frame.touches_confirmed
            || frame.tracked_players.len() > 128
        {
            return Err("Native track source is out of order or claims verified identities".into());
        }
        let in_play = windows.is_some_and(|w| w.contains(frame_idx));
        let mut appeared = BTreeSet::new();
        for tracked in &frame.tracked_players {
            if tracked.track_id == 0
                || tracked.track_id > i64::MAX as u64
                || tracked.is_verified_athlete
                || !appeared.insert(tracked.track_id)
                || tracked.detection.frame_index != frame_idx
                || tracked.detection.class_name != "player"
            {
                return Err("Native track has fabricated identity or repeated ID per frame".into());
            }
            tracked
                .detection
                .validate(frame_idx - frame_idx % 9, "players")?;
            observe_track(&mut full, tracked, frame_idx)?;
            if in_play {
                if let Some(ref mut selected) = scoped {
                    observe_track(selected, tracked, frame_idx)?;
                }
            }
        }
    }
    if reader.line()?.is_some() {
        return Err("Extra undeclared Track ID frames".into());
    }
    if reader.finish() != expected_sha {
        return Err("Native Track ID SHA-256 mismatch".into());
    }
    Ok(match scoped {
        Some(selected) => (selected, Some(full)),
        None => (full, None),
    })
}

pub(super) fn source(
    state: &AppState,
    project_id: &str,
    video: &str,
    replay_id: &str,
) -> Result<Inputs, ApiError> {
    source_with_verified_tracks(state, project_id, video, replay_id, None)
}

/// A scoped index is already hash-verified from the exact immutable replay.
/// Reuse its full-scope identity set for manual link validation instead of
/// reading the entire video a second time on filesystems without change stamps.
fn source_with_verified_tracks(
    state: &AppState,
    project_id: &str,
    video: &str,
    replay_id: &str,
    verified: Option<(&str, u64, TrackObservations)>,
) -> Result<Inputs, ApiError> {
    storage::project_id(project_id)?;
    storage::video_name(video)?;
    if !opaque_id(replay_id) {
        return Err(storage::bad_request("Invalid native replay ID"));
    }
    let project = projects::existing_project(state, project_id)?;
    if !project["videos"]
        .as_array()
        .is_some_and(|v| v.iter().any(|row| row.as_str() == Some(video)))
    {
        return Err(storage::bad_request(
            "Video does not belong to selected project",
        ));
    }
    let plan = preparation::plan_for(state, project_id, video)?;
    let prepared = plan
        .ready_path()
        .ok_or_else(|| storage::bad_request("Prepared video missing"))?;
    let (size, mtime) = inference_stream::source_stamp(prepared).map_err(storage::bad_request)?;
    let project_dir = projects::project_root(state).join(project_id);
    safe_folder(&project_dir).map_err(storage::bad_request)?;
    let replay_root = project_dir.join("native_cpu_replays");
    safe_folder(&replay_root).map_err(storage::bad_request)?;
    let replay_dir = replay_root.join(replay_id);
    safe_folder(&replay_dir).map_err(storage::bad_request)?;
    let manifest: Value = serde_json::from_slice(
        &fs::read(
            safe_file(&replay_dir, "manifest.json", MAX_MANIFEST).map_err(storage::bad_request)?,
        )
        .map_err(|e| storage::internal(format!("Read Track ID manifest: {e}")))?,
    )
    .map_err(|e| storage::bad_request(format!("Bad Track ID manifest: {e}")))?;
    if manifest["schema"] != "volley-native-cpu-tracking/1"
        || manifest["state"] != "provisional_tracklets_rebuilt"
        || manifest["project_id"].as_str() != Some(project_id)
        || manifest["video"].as_str() != Some(video)
        || manifest["replay_id"].as_str() != Some(replay_id)
        || manifest["source_identity"] != json!({"bytes":size,"mtime_ns":mtime})
        || manifest["fps"] != 30
        || manifest["file"] != "tracked_frames.jsonl"
        || manifest["track_ids_provisional"] != true
        || manifest["athlete_identities_confirmed"] != false
        || manifest["touches_confirmed"] != false
        || manifest["cpu_processing_complete"] != false
    {
        return Err(storage::bad_request(
            "Track ID replay is not valid for this video",
        ));
    }
    let frames = manifest["frames"]
        .as_u64()
        .filter(|x| *x > 0 && *x <= 48 * 3600 * 30)
        .ok_or_else(|| storage::bad_request("Invalid native Track ID frame count"))?;
    let sha = manifest["sha256"]
        .as_str()
        .filter(|s| valid_sha(s))
        .ok_or_else(|| storage::bad_request("Missing Track ID source SHA-256"))?
        .to_owned();
    let tracks = if let Some((verified_sha, verified_frames, tracks)) = verified {
        if sha != verified_sha || frames != verified_frames {
            return Err(storage::bad_request(
                "Rally-scoped and full-video Track ID source manifests disagree",
            ));
        }
        tracks
    } else {
        index_replay(
            &safe_file(&replay_dir, "tracked_frames.jsonl", MAX_EVIDENCE)
                .map_err(storage::bad_request)?,
            &sha,
            frames,
        )
        .map_err(storage::bad_request)?
    };
    let evidence = EvidenceSource::native(
        project_id,
        video,
        replay_id,
        &sha,
        SourceStamp {
            bytes: size,
            mtime_ns: u64::try_from(mtime)
                .map_err(|_| storage::bad_request("Video timestamp outside supported range"))?,
        },
        FrameRate::Exact {
            numerator: 30,
            denominator: 1,
        },
    )
    .map_err(storage::bad_request)?;
    let known = VerifiedTrackSet::new(evidence.clone(), tracks.keys().copied())
        .map_err(storage::bad_request)?;
    let (catalog, _) = player_catalog::load(state, false)?;
    let allowed = project["player_ids"]
        .as_array()
        .ok_or_else(|| storage::bad_request("Project roster must contain a player_ids array"))?
        .iter()
        .filter_map(Value::as_str)
        .collect::<BTreeSet<_>>();
    let mut roster = BTreeMap::new();
    for entry in catalog["players"].as_array().into_iter().flatten() {
        if let (Some(id), Some(name)) = (entry["id"].as_str(), entry["name"].as_str()) {
            if valid_player_id(id) && allowed.contains(id) {
                roster.insert(id.to_owned(), name.to_owned());
            }
        }
    }
    let links_dir = project_dir.join("native_track_assignments");
    if let Ok(meta) = fs::symlink_metadata(&links_dir) {
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(storage::bad_request("Unsafe player assignment directory"));
        }
    }
    Ok(Inputs {
        evidence,
        known,
        project_id: project_id.into(),
        video: video.into(),
        replay_id: replay_id.into(),
        source_sha: sha,
        tracks,
        roster,
        sidecar: links_dir.join(format!("{replay_id}.json")),
        can_write: state.writes_enabled,
    })
}

/// A rally run is the authoritative scope for displayed/assigned Track IDs.
/// Keep full-frame evidence unchanged for subsequent CPU Touch/Rally reprocessing.
pub(super) fn source_for_rallies(
    state: &AppState,
    project_id: &str,
    video: &str,
    replay_id: &str,
    rally_run_id: Option<&str>,
) -> Result<Inputs, ApiError> {
    let Some(run_id) = rally_run_id else {
        return source(state, project_id, video, replay_id);
    };
    let run = crate::native_rally_preview::inspect_project(state, project_id, run_id)?;
    if run.replay_id != replay_id || run.video != video {
        return Err(storage::bad_request(
            "Selected rally version does not match CPU Track ID source",
        ));
    }
    let windows =
        crate::native_rally_preview::verified_track_windows(&run).map_err(storage::bad_request)?;
    let replay_dir = projects::project_root(state)
        .join(project_id)
        .join("native_cpu_replays")
        .join(replay_id);
    let (scoped, full) = index_rally_replay(
        &safe_file(&replay_dir, "tracked_frames.jsonl", MAX_EVIDENCE)
            .map_err(storage::bad_request)?,
        &run.replay_sha,
        run.frames,
        &run.rally_sha,
        &windows,
    )
    .map_err(storage::bad_request)?;
    // The same verified scan collected *both* the rally subset and all IDs.
    let mut ctx = source_with_verified_tracks(
        state,
        project_id,
        video,
        replay_id,
        Some((&run.replay_sha, run.frames, full)),
    )?;
    if run.replay_sha != ctx.source_sha {
        return Err(storage::bad_request("Rally/CPU replay checksum mismatch"));
    }
    ctx.tracks = scoped;
    // Only observations inside detected play count for player review.
    // Avoid admitting someone observed for a few frames during a rally
    // just because they were visible for minutes during an intermission.
    let manifest: Value = serde_json::from_slice(
        &fs::read(
            safe_file(&replay_dir, "manifest.json", MAX_MANIFEST).map_err(storage::bad_request)?,
        )
        .map_err(|e| storage::internal(format!("Read CPU tracking settings: {e}")))?,
    )
    .map_err(|e| storage::bad_request(format!("Malformed CPU tracking settings: {e}")))?;
    let min_observations = manifest["settings"]["min_track_observations"]
        .as_u64()
        .filter(|n| (6..=90).contains(n))
        .unwrap_or(12);
    ctx.tracks.retain(|_, row| row.observed >= min_observations);
    Ok(ctx)
}

#[cfg(test)]
mod rally_scope_tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use uuid::Uuid;

    #[test]
    fn exact_rally_intervals_exclude_intermission_ids_and_photo_candidates() {
        let root = std::env::temp_dir().join(format!("volley-rally-index-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("tracked_frames.jsonl");
        let mut raw = Vec::new();
        for frame in 0..48u64 {
            let mut tracks = vec![(7, 0.90)];
            if (2..=5).contains(&frame) {
                tracks.push((9, 0.99)); // spectator visible only before the play
            }
            if (16..=18).contains(&frame) {
                tracks.push((8, 0.91)); // genuine player appearing during play
            }
            if (29..=32).contains(&frame) {
                tracks.push((10, 0.98)); // player visible only between plays
            }
            let players = tracks
                .iter()
                .enumerate()
                .map(|(query, (id, score))| {
                    json!({
                        "track_id":id,"is_verified_athlete":false,
                        "detection":{
                            "frame_index":frame,"class_name":"player","score":score,
                            "bbox_xyxy_norm":[0.15 + 0.15 * (query as f64),0.2,
                                0.25 + 0.15 * (query as f64),0.75],
                            "query_index":query,
                            "touch_probability":null,"head_point_norm":null
                        }
                    })
                })
                .collect::<Vec<_>>();
            let row = json!({
                "frame_index":frame,"track_ids_provisional":true,
                "confirmed_athlete_identities":false,"touches_confirmed":false,
                "tracked_players":players,"selected_ball":null
            });
            raw.extend(serde_json::to_vec(&row).unwrap());
            raw.push(b'\n');
        }
        fs::write(&path, &raw).unwrap();
        let sha = format!("{:x}", Sha256::digest(&raw));
        let (full, secondary) = scan_with_scope(&path, &sha, 48, None).unwrap();
        assert!(secondary.is_none());
        assert_eq!(full.len(), 4);
        let windows = crate::native_rally_preview::RallyWindows {
            intervals: vec![(12, 20), (40, 44)],
        };
        let (scoped, full_also) = scan_with_scope(&path, &sha, 48, Some(&windows)).unwrap();
        assert_eq!(full_also.unwrap().len(), 4);
        assert_eq!(scoped.len(), 2);
        assert_eq!(scoped.get(&7).unwrap().observed, 14);
        assert_eq!(scoped.get(&7).unwrap().first, 12);
        assert_eq!(scoped.get(&7).unwrap().last, 44);
        assert_eq!(scoped.get(&7).unwrap().best_frame, 12);
        let photos = &scoped.get(&7).unwrap().photo_samples;
        assert!(photos.len() >= 2);
        assert!(photos.iter().all(|photo| windows.contains(photo.frame)));
        assert_ne!(photos[0].frame, photos[1].frame);
        assert_eq!(scoped.get(&8).unwrap().observed, 3);
        assert!(!scoped.contains_key(&9));
        assert!(!scoped.contains_key(&10));
        assert!(scan_with_scope(&path, &"0".repeat(64), 48, Some(&windows)).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
