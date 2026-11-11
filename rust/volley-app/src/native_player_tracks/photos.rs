use super::*;
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhotoQuery {
    pub(super) project_id: String,
    pub(super) video: String,
    pub(super) replay_id: String,
    pub(super) track_id: u64,
    #[serde(default)]
    pub(super) rally_run_id: Option<String>,
    #[serde(default)]
    pub(super) photo_index: usize,
}

pub(super) fn crop_box(
    norm: [f64; 4],
    video_width: u32,
    video_height: u32,
) -> Result<[u32; 4], String> {
    if video_width < 64
        || video_height < 64
        || video_width > 16384
        || video_height > 16384
        || !norm.iter().all(|v| v.is_finite())
        || norm[0] < 0.0
        || norm[1] < 0.0
        || norm[2] > 1.0
        || norm[3] > 1.0
        || norm[2] <= norm[0]
        || norm[3] <= norm[1]
    {
        return Err("Invalid native player bounding box".into());
    }
    let pad_x = (norm[2] - norm[0]) * 0.12;
    let pad_y = (norm[3] - norm[1]) * 0.12;
    let left = ((norm[0] - pad_x).max(0.0) * video_width as f64).floor() as u32;
    let top = ((norm[1] - pad_y).max(0.0) * video_height as f64).floor() as u32;
    let right = ((norm[2] + pad_x).min(1.0) * video_width as f64).ceil() as u32;
    let bottom = ((norm[3] + pad_y).min(1.0) * video_height as f64).ceil() as u32;
    let width = right.min(video_width).saturating_sub(left);
    let height = bottom.min(video_height).saturating_sub(top);
    if width < 12 || height < 12 {
        return Err("Player detection is too small for photo preview".into());
    }
    Ok([left, top, width, height])
}
fn photo_candidate(track: &Aggregate, index: usize) -> (u64, [f64; 4]) {
    let alternatives: Vec<_> = track
        .photo_samples
        .iter()
        .filter(|sample| sample.frame != track.best_frame)
        .collect();
    match index.checked_sub(1) {
        Some(index) if !alternatives.is_empty() => {
            let sample = alternatives[index % alternatives.len()];
            (sample.frame, sample.bbox)
        }
        _ => (track.best_frame, track.best_box),
    }
}
pub async fn photo(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PhotoQuery>,
) -> Result<axum::response::Response, ApiError> {
    // A photo preview does not mutate project state. Parallel thumbnails must
    // never contend for the exclusive GPU/CPU writer lock. Bound the *entire*
    // source verification + FFmpeg operation with the shared preview semaphore.
    let _permit = state
        .preview_limit
        .acquire()
        .await
        .map_err(|e| storage::internal(format!("Photo decode slot: {e}")))?;
    let plan_state = state.clone();
    let photo_plan = tokio::task::spawn_blocking(move || -> Result<_, ApiError> {
        let inputs = source_for_rallies(
            &plan_state,
            &query.project_id,
            &query.video,
            &query.replay_id,
            query.rally_run_id.as_deref(),
        )?;
        let track = inputs
            .tracks
            .get(&query.track_id)
            .ok_or_else(|| storage::bad_request("Track ID absent from saved CPU replay"))?;
        let plan = preparation::plan_for(&plan_state, &query.project_id, &query.video)?;
        let prepared = plan
            .ready_path()
            .ok_or_else(|| storage::bad_request("Native preview video unavailable"))?;
        let video = volley_media::probe(prepared).map_err(storage::bad_request)?;
        let (frame, bbox) = photo_candidate(track, query.photo_index);
        let crop = crop_box(bbox, video.width, video.height).map_err(storage::bad_request)?;
        let stamp = inference_stream::source_stamp(prepared).map_err(storage::bad_request)?;
        Ok((prepared.to_path_buf(), frame, crop, stamp))
    })
    .await
    .map_err(|e| storage::internal(format!("Player photo verification task: {e}")))??;
    let (prepared, frame, [x, y, width, height], stamp) = photo_plan;
    let time = format!("{:.4}", frame as f64 / 30.0);
    let vf = format!("crop={width}:{height}:{x}:{y},scale=240:-2,setsar=1");
    let mut cmd = tokio::process::Command::new("ffmpeg");
    cmd.args([
        "-nostdin",
        "-hide_banner",
        "-loglevel",
        "error",
        "-ss",
        &time,
        "-i",
    ]);
    cmd.arg(&prepared);
    cmd.args([
        "-vf",
        &vf,
        "-frames:v",
        "1",
        "-an",
        "-c:v",
        "mjpeg",
        "-q:v",
        "4",
        "-f",
        "image2pipe",
        "pipe:1",
    ]);
    cmd.kill_on_drop(true);
    let bytes = tokio::time::timeout(std::time::Duration::from_secs(35), cmd.output())
        .await
        .map_err(|_| storage::bad_request("Player preview extraction timed out"))?
        .map_err(|e| storage::bad_request(format!("FFmpeg player crop: {e}")))?;
    if !bytes.status.success()
        || bytes.stdout.len() < 100
        || bytes.stdout.len() > 3_000_000
        || !bytes.stdout.starts_with(&[0xff, 0xd8])
    {
        return Err(storage::bad_request(
            "Unable to render native Track ID photo",
        ));
    }
    if inference_stream::source_stamp(&prepared).map_err(storage::bad_request)? != stamp {
        return Err(storage::conflict(
            "Source video changed while producing photo preview",
        ));
    }
    use axum::{body::Body, http::header};
    axum::response::Response::builder()
        .header(header::CONTENT_TYPE, "image/jpeg")
        .header(header::CACHE_CONTROL, "private, no-store")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(Body::from(bytes.stdout))
        .map_err(|e| storage::internal(format!("Player photo response: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn photo_candidates_advance_and_wrap_with_their_own_boxes() {
        let track = Aggregate {
            best_frame: 6,
            best_box: [0.1, 0.2, 0.3, 0.4],
            photo_samples: vec![
                PhotoSample {
                    frame: 6,
                    bbox: [0.1, 0.2, 0.3, 0.4],
                },
                PhotoSample {
                    frame: 12,
                    bbox: [0.2, 0.3, 0.4, 0.5],
                },
                PhotoSample {
                    frame: 18,
                    bbox: [0.3, 0.4, 0.5, 0.6],
                },
            ],
            ..Aggregate::default()
        };
        assert_eq!(photo_candidate(&track, 0), (6, [0.1, 0.2, 0.3, 0.4]));
        assert_eq!(photo_candidate(&track, 1), (12, [0.2, 0.3, 0.4, 0.5]));
        assert_eq!(photo_candidate(&track, 2), (18, [0.3, 0.4, 0.5, 0.6]));
        assert_eq!(photo_candidate(&track, 3), (12, [0.2, 0.3, 0.4, 0.5]));
    }
}
