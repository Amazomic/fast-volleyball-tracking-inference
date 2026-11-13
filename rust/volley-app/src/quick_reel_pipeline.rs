//! One fresh GPU -> CPU -> original-media FFmpeg pipeline PER Quick Reel run.
//! Reuses only the project's canonical stage implementations and global
//! prepared media; the transient virtual project is contained in Quick Reel.
//! Never writes to real project registries or analysis caches.
use super::*;
/// Transient worker registry under quick_reels/<uuid>/pipeline only:
/// never adds a real project or consumes previously analyzed results.
pub(super) struct RunWorkspace {
    pub(super) project_id: String,
    pub(super) aliases: Vec<String>,
    pub(super) app: Arc<AppState>,
}
pub(super) fn create_run_workspace(
    original: &AppState,
    run: &PreparedRun,
) -> Result<RunWorkspace, ApiError> {
    if !is_id(&run.id) || run.sources.is_empty() || run.sources.len() != run.plans.len() {
        return Err(storage::bad_request("Invalid Quick Reel source set"));
    }
    let pipeline = run.root.join("pipeline");
    fs::create_dir(&pipeline)
        .map_err(|e| storage::internal(format!("Quick Reel pipeline: {e}")))?;
    let projects = pipeline.join("projects");
    fs::create_dir(&projects)
        .map_err(|e| storage::internal(format!("Quick Reel projects: {e}")))?;
    let project_id = run.id[..12].to_owned();
    fs::create_dir(projects.join(&project_id))
        .map_err(|e| storage::internal(format!("Quick Reel internal project: {e}")))?;
    let mut aliases = Vec::new();
    let mut names = Map::new();
    let mut ids = Map::new();
    for (index, (src, plan)) in run.sources.iter().zip(&run.plans).enumerate() {
        let canonical = verify_source(src)?;
        if plan.source != canonical || plan.ready_path().is_none() {
            return Err(storage::conflict(
                "Quick Reel original or prepared cache changed",
            ));
        }
        let ext = Path::new(&src.name)
            .extension()
            .and_then(|s| s.to_str())
            .ok_or_else(|| storage::bad_request("Quick Reel video extension missing"))?;
        let alias = format!("quick_{index:02}.{ext}");
        storage::video_name(&alias)?;
        names.insert(alias.clone(), json!(src.source));
        ids.insert(alias.clone(), json!(src.id));
        aliases.push(alias);
    }
    storage::write_json_atomic(
        &projects.join("projects.json"),
        &json!({"projects":[{
            "id":project_id,"videos":aliases,"quick_reel_internal":true,
            "quick_reel_run_id":run.id,"source_paths":names,"source_ids":ids
        }]}),
    )?;
    let music = pipeline.join("uploads").join("music");
    fs::create_dir_all(&music)
        .map_err(|e| storage::internal(format!("Quick Reel audio folder: {e}")))?;
    let name = Path::new(&run.audio)
        .file_name()
        .ok_or_else(|| storage::bad_request("Missing Quick Reel audio file name"))?;
    let target = music.join(name);
    fs::copy(&run.audio, &target)
        .map_err(|e| storage::internal(format!("Quick Reel audio copy: {e}")))?;
    if !volley_media::montage::soundtrack_is_audio(&target).map_err(storage::bad_request)? {
        return Err(storage::conflict("Quick Reel soundtrack changed"));
    }
    let app = Arc::new(AppState {
        repository_root: original.repository_root.clone(),
        data_root: pipeline,
        writes_enabled: true,
        montage: montage::MontageManager::new(),
        preparation: preparation::PreparationManager::new(),
        preview_limit: tokio::sync::Semaphore::new(1),
        touch_limit: tokio::sync::Semaphore::new(1),
        continuity_limit: tokio::sync::Semaphore::new(1),
        reid_limit: tokio::sync::Semaphore::new(1),
        identity_limit: tokio::sync::Semaphore::new(1),
        statistics_limit: tokio::sync::Semaphore::new(1),
        marks_limit: tokio::sync::Semaphore::new(1),
        quick_reel: QuickReelManager::new(),
        inference_limit: tokio::sync::Semaphore::new(1),
        full_inference: inference_full::FullInferenceManager::new(),
    });
    Ok(RunWorkspace {
        project_id,
        aliases,
        app,
    })
}
fn reason(error: ApiError) -> String {
    error.1 .0["error"]
        .as_str()
        .unwrap_or("Video processing error")
        .to_owned()
}
fn stage_status(
    run: &PreparedRun,
    videos: &[Value],
    stage: &str,
    index: u8,
    pct: f64,
) -> Result<(), String> {
    save_status(
        run,
        &running_json(run, "running", stage, index, pct, videos, None),
    )
    .map_err(reason)
}
fn require_id(response: &Value, key: &str) -> Result<String, String> {
    response[key]
        .as_str()
        .filter(|v| is_id(v))
        .map(str::to_owned)
        .ok_or_else(|| format!("Missing Quick Reel source-scoped {key}"))
}
#[derive(Debug, Clone)]
pub(super) struct RankedRally {
    pub(super) run_id: String,
    pub(super) candidate_id: String,
    pub(super) touches: u64,
    pub(super) coverage: f64,
    pub(super) start: u64,
    pub(super) end: u64,
}
pub(super) fn best_quick_rallies(mut proposals: Vec<RankedRally>) -> Vec<RankedRally> {
    // Ranked by motion-supported Touch evidence, NOT "attack"/serve labels.
    proposals.retain(|row| {
        row.touches > 0
            && row.end >= row.start
            && (row.end - row.start + 1) >= 90
            && row.coverage >= 0.15
    });
    proposals.sort_by(|a, b| {
        b.touches
            .cmp(&a.touches)
            .then_with(|| b.coverage.total_cmp(&a.coverage))
            .then_with(|| a.run_id.cmp(&b.run_id))
            .then_with(|| a.start.cmp(&b.start))
    });
    let mut total = 0u64;
    proposals
        .into_iter()
        .filter(|row| {
            if total + (row.end - row.start + 1) + 90 > 600 * 30 {
                return false;
            }
            total += row.end - row.start + 1 + 90;
            true
        })
        .take(12)
        .collect()
}

async fn quick_gpu_model(
    work: &RunWorkspace,
    run: &PreparedRun,
    videos: &mut [Value],
    index: usize,
    model: volley_inference::ModelKind,
    cancelled: &watch::Receiver<bool>,
    percentage: f64,
) -> Result<String, String> {
    let alias = &work.aliases[index];
    let request: inference_full::StartRequest = serde_json::from_value(json!({
        "project_id":work.project_id,"video":alias,
        "model":model,"stride":9,"device":"cuda",
        "gpu_device_id":0,"gpu_memory_limit_mb":0,"cpu_threads":8
    }))
    .map_err(|e| format!("Quick Reel GPU parameters: {e}"))?;
    if *cancelled.borrow() {
        return Err("cancelled".into());
    }
    let Json(start) =
        inference_full::start(State(work.app.clone()), HeaderMap::new(), Json(request))
            .await
            .map_err(reason)?;
    let job_id = require_id(&start, "job_id")?;
    crate::gpu_poll::wait(
        &work.app,
        &work.project_id,
        &job_id,
        || *cancelled.borrow(),
        |status| {
            let model_label = if model == volley_inference::ModelKind::Players {
                "RTMDet + RTMPose"
            } else {
                "VballNetV4c"
            };
            videos[index]["state"] = json!("gpu");
            videos[index]["model"] = json!(model_label);
            videos[index]["gpu_percent"] = status["progress_percent"].clone();
            let local = status["progress_percent"].as_f64().unwrap_or(0.0);
            stage_status(
                run,
                videos,
                &format!("GPU {model_label}: {}", index + 1),
                2,
                (percentage + local * 0.19 / work.aliases.len() as f64).min(69.0),
            )
        },
    )
    .await
}

// Generic stage runner: request, workspace, status coordinates, cancellation
// and the handler itself. A context struct is planned for stage E.
#[expect(clippy::too_many_arguments)]
async fn quick_cpu_stage<T, F>(
    request: T,
    work: &RunWorkspace,
    stage: &str,
    run: &PreparedRun,
    videos: &[Value],
    percent: f64,
    cancelled: &watch::Receiver<bool>,
    handler: F,
) -> Result<Value, String>
where
    T: Send + 'static,
    F: FnOnce(&AppState, &T) -> Result<Value, ApiError> + Send + 'static,
{
    if *cancelled.borrow() {
        return Err("cancelled".into());
    }
    stage_status(run, videos, stage, 3, percent)?;
    let state = work.app.clone();
    let result = tokio::task::spawn_blocking(move || handler(&state, &request))
        .await
        .map_err(|e| format!("Quick Reel native CPU task panicked: {e}"))?
        .map_err(reason)?;
    if *cancelled.borrow() {
        return Err("cancelled".into());
    }
    Ok(result)
}

async fn quick_montage(
    work: &RunWorkspace,
    run: &PreparedRun,
    videos: &[Value],
    clips: Vec<SelectedClip>,
    cancelled: &watch::Receiver<bool>,
) -> Result<Value, String> {
    if *cancelled.borrow() {
        return Err("cancelled".into());
    }
    let audio = Path::new(&run.audio)
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("Quick Reel isolated soundtrack has no file name")?
        .to_owned();
    let Json(start) = montage::start_native_montage(
        State(work.app.clone()),
        HeaderMap::new(),
        Json(montage::MontageRequest {
            project_id: work.project_id.clone(),
            clips,
            audio_file: Some(audio),
            audio_volume: 40.0,
            encoder: Some("auto".into()),
            layout: VideoLayout::Vertical,
            verified_highlights: None,
        }),
    )
    .await
    .map_err(reason)?;
    let job_id = start["job_id"]
        .as_str()
        .filter(|id| id.starts_with("montage_") && id.len() == 40)
        .ok_or("Quick Reel montage did not return a valid job")?
        .to_owned();
    let mut cancel_sent = false;
    loop {
        if *cancelled.borrow() && !cancel_sent {
            let _ = montage::native_cancel(
                State(work.app.clone()),
                HeaderMap::new(),
                Json(json!({"job_id":job_id})),
            )
            .await;
            cancel_sent = true;
        }
        let Json(status) = montage::native_status(State(work.app.clone())).await;
        if status["id"].as_str() != Some(job_id.as_str()) {
            return Err("Quick Reel montage switched to another job".into());
        }
        let pct = status["overall_percent"].as_f64().unwrap_or(0.0);
        stage_status(
            run,
            videos,
            "Монтаж оригинальных видео + AAC",
            4,
            85.0 + pct * 0.14,
        )?;
        match status["state"].as_str() {
            Some("complete") if !cancel_sent => break,
            Some("cancelled" | "failed" | "complete") if cancel_sent => {
                return Err("cancelled".into())
            }
            Some("cancelled" | "failed") => {
                return Err(format!(
                    "Quick Reel FFmpeg: {}",
                    status["error"]
                        .as_str()
                        .unwrap_or("No completed original-media montage")
                ))
            }
            Some("queued" | "running") => {}
            _ => return Err("Unexpected Quick Reel FFmpeg job state".into()),
        }
        tokio::time::sleep(Duration::from_millis(650)).await;
    }
    if *cancelled.borrow() {
        return Err("cancelled".into());
    }
    let folder = work
        .app
        .data_root
        .join("projects")
        .join(&work.project_id)
        .join("montages")
        .join(&job_id);
    let receipt = storage::read_json(&folder.join("selection.json"))
        .map_err(reason)?
        .ok_or("Quick Reel montage output receipt missing")?;
    if receipt["schema"] != "volley-rust-montage/1"
        || receipt["render_source"] != "original_recording"
        || receipt["layout"] != "vertical"
        || receipt["source_clips_native_resolution_verified"] != true
        || receipt["mp4_quality"]["has_audio"] != true
        || receipt["project_id"].as_str() != Some(work.project_id.as_str())
    {
        return Err(
            "Quick Reel MP4 did not pass native-source video/audio quality controls".into(),
        );
    }
    let source = folder.join("montage.mp4");
    let target = run.root.join("quick_reel.mp4");
    if target.exists() {
        return Err("Quick Reel output already exists".into());
    }
    let copy_source = source.clone();
    let copy_target = target.clone();
    tokio::task::spawn_blocking(move || fs::copy(&copy_source, &copy_target))
        .await
        .map_err(|e| format!("Publish Quick Reel MP4: {e}"))?
        .map_err(|e| format!("Publish Quick Reel MP4: {e}"))?;
    let filesize = fs::metadata(&target)
        .map_err(|e| format!("Quick Reel MP4 metadata: {e}"))?
        .len();
    if filesize < 1024 {
        let _ = fs::remove_file(&target);
        return Err("Quick Reel MP4 is unexpectedly empty".into());
    }
    Ok(json!({
        "file":"quick_reel.mp4","size_bytes":filesize,
        "render_source":"original_recording","layout":"vertical",
        "mp4_quality":receipt["mp4_quality"],"montage_job_id":job_id,
        "native_original_clips_verified":true
    }))
}

pub(super) async fn run_native_pipeline(
    original: &Arc<AppState>,
    run: &PreparedRun,
    mut videos: Vec<Value>,
    cancelled: watch::Receiver<bool>,
) -> Result<(Value, Vec<Value>), String> {
    if *cancelled.borrow() {
        return Err("cancelled".into());
    }
    // Workspace creation verifies prepared-cache SHA receipts and copies the
    // soundtrack; keep that blocking work off the tokio workers.
    let workspace_state = original.clone();
    let workspace_run = run.clone();
    let workspace =
        tokio::task::spawn_blocking(move || create_run_workspace(&workspace_state, &workspace_run))
            .await
            .map_err(|e| format!("Quick Reel workspace task: {e}"))?
            .map_err(reason)?;
    let count = workspace.aliases.len();
    let mut candidates = Vec::<RankedRally>::new();
    let mut verified = std::collections::BTreeMap::<String, String>::new();
    let _gpu_permit = original
        .inference_limit
        .acquire()
        .await
        .map_err(|e| format!("Cannot reserve Quick Reel CUDA device: {e}"))?;
    for (index, alias) in workspace.aliases.iter().enumerate() {
        if *cancelled.borrow() {
            return Err("cancelled".into());
        }
        verify_source(&run.sources[index]).map_err(reason)?;
        let gpu_pct = 15.0 + 54.0 * index as f64 / count as f64;
        let ball = quick_gpu_model(
            &workspace,
            run,
            &mut videos,
            index,
            volley_inference::ModelKind::Ball,
            &cancelled,
            gpu_pct,
        )
        .await?;
        let players = quick_gpu_model(
            &workspace,
            run,
            &mut videos,
            index,
            volley_inference::ModelKind::Players,
            &cancelled,
            gpu_pct + 27.0 / count as f64,
        )
        .await?;
        if ball == players {
            return Err("Quick Reel model runs accidentally reused an ID".into());
        }
        videos[index]["fresh_gpu_jobs"] = json!({"ball":ball,"players":players});
        let project_id = workspace.project_id.clone();
        let fusion = quick_cpu_stage(
            inference_fusion::FuseRequest {
                project_id: project_id.clone(),
                video: alias.clone(),
                person_job_id: players,
                vball_job_id: ball,
            },
            &workspace,
            "CPU: объединение двух свежих GPU-моделей",
            run,
            &videos,
            70.0 + 14.0 * index as f64 / count as f64,
            &cancelled,
            inference_fusion::do_fusion,
        )
        .await?;
        let fusion_id = require_id(&fusion, "fusion_id")?;
        let replay = quick_cpu_stage(
            cpu_retrack::ReplayRequest {
                project_id: project_id.clone(),
                video: alias.clone(),
                fusion_id,
                settings: None,
            },
            &workspace,
            "CPU: Track ID",
            run,
            &videos,
            71.0 + 14.0 * index as f64 / count as f64,
            &cancelled,
            cpu_retrack::process,
        )
        .await?;
        let replay_id = require_id(&replay, "replay_id")?;
        let touches = quick_cpu_stage(
            cpu_touches::TouchRequest {
                project_id: project_id.clone(),
                video: alias.clone(),
                replay_id: replay_id.clone(),
                settings: None,
                pose_classification: false,
            },
            &workspace,
            "CPU: физические касания",
            run,
            &videos,
            72.0 + 14.0 * index as f64 / count as f64,
            &cancelled,
            cpu_touches::process,
        )
        .await?;
        let touch_id = require_id(&touches, "touch_id")?;
        let rally = quick_cpu_stage(
            cpu_rallies::RallyRequest {
                project_id: project_id.clone(),
                video: alias.clone(),
                replay_id,
                touch_id,
                settings: None,
            },
            &workspace,
            "CPU: реконструкция розыгрышей",
            run,
            &videos,
            73.0 + 14.0 * index as f64 / count as f64,
            &cancelled,
            cpu_rallies::process,
        )
        .await?;
        let rally_id = require_id(&rally, "rally_run_id")?;
        let state = workspace.app.clone();
        let project = workspace.project_id.clone();
        let runid = rally_id.clone();
        let scored = tokio::task::spawn_blocking(move || {
            let validated = native_rally_preview::inspect_project(&state, &project, &runid)?;
            native_rally_preview::verified_quick_rally_candidates(&validated)
                .map_err(storage::bad_request)
        })
        .await
        .map_err(|e| format!("Quick Reel rally verify panic: {e}"))?
        .map_err(reason)?;
        for (candidate_id, start, end, touch_count, coverage) in scored {
            candidates.push(RankedRally {
                run_id: rally_id.clone(),
                candidate_id,
                touches: touch_count,
                coverage,
                start,
                end,
            });
        }
        videos[index]["state"] = json!("cpu_complete");
        videos[index]["progress"] = json!(100.0);
        videos[index]["cpu_runs"] = json!({
            "fusion":fusion["fusion_id"],"tracking":replay["replay_id"],
            "touches":touches["touch_id"],"rallies":rally_id,
            "replay_sha256":replay["sha256"],"touch_sha256":touches["sha256"],
            "rally_sha256":rally["sha256"]
        });
        verified.insert(rally_id, alias.clone());
        stage_status(
            run,
            &videos,
            "CPU готово; проверка розыгрышей",
            3,
            70.0 + 15.0 * (index + 1) as f64 / count as f64,
        )?;
    }
    drop(_gpu_permit);
    let selected = best_quick_rallies(candidates);
    if selected.is_empty() {
        return Err("Quick Reel: GPU/CPU complete, but no motion-verified rallies >=3 seconds; no fake MP4 published".into());
    }
    let mut by_run = std::collections::BTreeMap::<String, Vec<String>>::new();
    for row in &selected {
        if !verified.contains_key(&row.run_id) {
            return Err("Cross-source rally selected for Quick Reel".into());
        }
        by_run
            .entry(row.run_id.clone())
            .or_default()
            .push(row.candidate_id.clone());
    }
    let mut originals = std::collections::BTreeMap::<(String, String), SelectedClip>::new();
    for (rally_id, ids) in by_run {
        let clip_state = workspace.app.clone();
        let clip_project = workspace.project_id.clone();
        let clip_run = rally_id.clone();
        let clip_ids = ids.clone();
        let clips = tokio::task::spawn_blocking(move || {
            let validated =
                native_rally_preview::inspect_project(&clip_state, &clip_project, &clip_run)
                    .map_err(reason)?;
            native_rally_preview::verified_manual_clips(&validated, clip_ids)
        })
        .await
        .map_err(|e| format!("Quick Reel clip verify task: {e}"))??;
        if clips.len() != ids.len() {
            return Err("Quick Reel rally identity mismatch".into());
        }
        for (id, clip) in ids.into_iter().zip(clips) {
            originals.insert((rally_id.clone(), id), clip);
        }
    }
    let mut ordered = Vec::new();
    let mut actually_selected = Vec::new();
    let mut duration = 0.0;
    for row in &selected {
        let clip = originals
            .remove(&(row.run_id.clone(), row.candidate_id.clone()))
            .ok_or("Missing original Quick Reel rally clip")?;
        duration += (clip.end_frame - clip.start_frame + 1) as f64 / 30.0;
        if duration > 600.0 {
            break;
        }
        ordered.push(clip);
        actually_selected.push(row.clone());
    }
    if ordered.is_empty() {
        return Err("No bounded original-media clips for Quick Reel".into());
    }
    stage_status(run, &videos, "Монтаж из оригинальных видеозаписей", 4, 85.0)?;
    let mut outcome = quick_montage(&workspace, run, &videos, ordered, &cancelled).await?;
    if *cancelled.borrow() {
        let _ = fs::remove_file(run.root.join("quick_reel.mp4"));
        return Err("cancelled".into());
    }
    let receipt = json!({
        "schema":"volley-native-quick-reel/2","id":run.id,
        "source_type":"fresh_native_gpu_cpu",
        "reused_artifacts":["video_preparation_only"],
        "project_created":false,
        "source_selection":run.sources,
        "cpu_sources":videos,
        "highlight_policy":"motion_supported_rallies_not_verified_attacks",
        "selected_rallies":actually_selected.iter().map(|r|json!({
            "rally_run_id":r.run_id,"candidate_id":r.candidate_id,
            "motion_supported_contacts":r.touches,"ball_coverage":r.coverage
        })).collect::<Vec<_>>(),
        "output":outcome,
    });
    storage::write_json_atomic(&run.root.join("result.json"), &receipt).map_err(reason)?;
    let _ = fs::remove_dir_all(&workspace.app.data_root);
    outcome["fresh_gpu_cpu"] = json!(true);
    outcome["cached_inference_reused"] = json!(false);
    Ok((outcome, videos))
}
