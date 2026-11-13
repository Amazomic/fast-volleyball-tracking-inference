//! Safe single-pass fusion of two completed, independent GPU model jobs.
//! Reads and verifies immutable JSONL+SHA-256 without loading a match into RAM.
//! Outputs raw CPU-replay candidates, NOT stable tracks or confirmed touches.
use crate::{
    inference_stream, preparation, project_guard, projects,
    storage::{self, ApiError, ApiResult},
    AppState,
};
use axum::{
    extract::{Query, State},
    http::HeaderMap,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    sync::Arc,
};
use uuid::Uuid;
use volley_core::fusion::{fuse_window, Candidate};

const MAX_MANIFEST: u64 = 16384;
const MAX_LINE: usize = 512 * 1024;
const MAX_JSONL_BYTES: u64 = 64 * 1024 * 1024 * 1024;
const OUTPUT_FILE: &str = "frames.jsonl";
const SOURCE_FILE: &str = "detections.jsonl";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FuseRequest {
    pub(crate) project_id: String,
    pub(crate) video: String,
    pub(crate) person_job_id: String,
    pub(crate) vball_job_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Window {
    first_frame: u64,
    raw_detections: Vec<Candidate>,
    confirmed_tracks: bool,
    confirmed_touches: bool,
}

pub(crate) use volley_core::contracts::job_id as opaque_id;

fn existing_file(folder: &Path, name: &str, max: u64) -> Result<PathBuf, String> {
    let path = folder.join(name);
    let meta = fs::symlink_metadata(&path).map_err(|e| format!("GPU evidence file {name}: {e}"))?;
    if meta.file_type().is_symlink() || !meta.is_file() || meta.len() > max {
        return Err(format!("Unsafe, missing or oversized GPU evidence: {name}"));
    }
    Ok(path)
}
fn verified_run_folder(root: &Path, id: &str) -> Result<PathBuf, String> {
    if !opaque_id(id) {
        return Err("Invalid raw GPU run ID".into());
    }
    let root_meta = fs::symlink_metadata(root).map_err(|e| format!("GPU runs root: {e}"))?;
    if root_meta.file_type().is_symlink() || !root_meta.is_dir() {
        return Err("Invalid GPU runs directory".into());
    }
    let path = root.join(id);
    let meta = fs::symlink_metadata(&path).map_err(|e| format!("GPU model run: {e}"))?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err("Unsafe GPU model run directory".into());
    }
    Ok(path)
}

#[derive(Clone)]
struct ModelEvidence {
    folder: PathBuf,
    job_id: String,
    sha256: String,
    windows: u64,
    decoded_frames: u64,
    source_identity: Value,
}

fn open_evidence(
    root: &Path,
    job_id: &str,
    project_id: &str,
    video: &str,
    model: &str,
) -> Result<ModelEvidence, String> {
    let folder = verified_run_folder(root, job_id)?;
    let manifest_file = existing_file(&folder, "manifest.json", MAX_MANIFEST)?;
    let manifest: Value = serde_json::from_slice(
        &fs::read(manifest_file).map_err(|e| format!("Read GPU manifest: {e}"))?,
    )
    .map_err(|e| format!("Parse GPU manifest: {e}"))?;
    if !volley_core::contracts::gpu_manifest(&manifest, project_id, video, job_id, model) {
        return Err("Incompatible, incomplete or untrusted GPU model manifest".into());
    }
    let windows = manifest["windows"]
        .as_u64()
        .filter(|n| *n > 0 && *n <= 48 * 3600 * 30 / 9)
        .ok_or("Invalid model window count")?;
    let decoded = manifest["decoded_frames"]
        .as_u64()
        .ok_or("Missing decoded frame count")?;
    if decoded < windows * 9 || decoded > windows * 9 + 8 {
        return Err("Incomplete or inconsistent GPU frame sequence".into());
    }
    let hash = manifest["sha256"]
        .as_str()
        .filter(|s| {
            s.len() == 64
                && s.bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        })
        .ok_or("Invalid GPU result checksum")?
        .to_owned();
    if !manifest["source_identity"]["bytes"].is_u64()
        || !manifest["source_identity"]["mtime_ns"].is_u64()
    {
        return Err("Missing prepared-video identity".into());
    }
    existing_file(&folder, SOURCE_FILE, MAX_JSONL_BYTES)?;
    Ok(ModelEvidence {
        folder,
        job_id: job_id.into(),
        sha256: hash,
        windows,
        decoded_frames: decoded,
        source_identity: manifest["source_identity"].clone(),
    })
}

struct WindowReader {
    source: BufReader<File>,
    digest: Sha256,
    windows: u64,
}
impl WindowReader {
    fn open(input: &ModelEvidence) -> Result<Self, String> {
        let file = File::open(existing_file(&input.folder, SOURCE_FILE, MAX_JSONL_BYTES)?)
            .map_err(|e| format!("Open model evidence: {e}"))?;
        Ok(Self {
            source: BufReader::new(file),
            digest: Sha256::new(),
            windows: 0,
        })
    }
    /// Bounded equivalent of BufRead::read_until: malformed JSONL with one
    /// 100-MB line must never cause a giant allocation in the Rust server.
    fn next_line(&mut self) -> Result<Option<Vec<u8>>, String> {
        let mut line = Vec::with_capacity(4096);
        loop {
            let chunk = self
                .source
                .fill_buf()
                .map_err(|e| format!("Read evidence: {e}"))?;
            if chunk.is_empty() {
                if line.is_empty() {
                    return Ok(None);
                }
                return Err("GPU JSONL line does not end with a newline".into());
            }
            let count = chunk
                .iter()
                .position(|b| *b == b'\n')
                .map_or(chunk.len(), |pos| pos + 1);
            if line.len() + count > MAX_LINE {
                return Err("GPU evidence JSONL line exceeds safe limit".into());
            }
            let found_newline = chunk[count - 1] == b'\n';
            line.extend_from_slice(&chunk[..count]);
            self.digest.update(&chunk[..count]);
            self.source.consume(count);
            if found_newline {
                return Ok(Some(line));
            }
        }
    }
    fn window(&mut self, first: u64) -> Result<Window, String> {
        let bytes = self.next_line()?.ok_or("GPU model JSONL truncated")?;
        let item: Window = serde_json::from_slice(&bytes)
            .map_err(|e| format!("Invalid neural detection JSONL: {e}"))?;
        if item.first_frame != first || item.confirmed_tracks || item.confirmed_touches {
            return Err("Neural window ordering or raw status is invalid".into());
        }
        self.windows += 1;
        Ok(item)
    }
    fn finish(mut self, expected: &ModelEvidence) -> Result<(), String> {
        if self.next_line()?.is_some() || self.windows != expected.windows {
            return Err("Unexpected extra GPU windows or count mismatch".into());
        }
        let digest = format!("{:x}", self.digest.finalize());
        if digest != expected.sha256 {
            return Err("GPU JSONL SHA-256 mismatch; source evidence changed".into());
        }
        Ok(())
    }
}

#[derive(Default)]
struct FusedTotals {
    frames: u64,
    players: u64,
    ball_candidates: u64,
    selected_vball: u64,
}

fn write_fused(
    ball: &ModelEvidence,
    ravel: &ModelEvidence,
    output: &Path,
) -> Result<(String, FusedTotals), String> {
    if ball.windows != ravel.windows
        || ball.decoded_frames != ravel.decoded_frames
        || ball.source_identity != ravel.source_identity
    {
        return Err(
            "GPU model runs represent different frame sequences or source recordings".into(),
        );
    }
    let mut players = WindowReader::open(ravel)?;
    let mut balls = WindowReader::open(ball)?;
    let result = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .map_err(|e| format!("Create new fused output: {e}"))?;
    let mut writer = BufWriter::new(result);
    let mut hash = Sha256::new();
    let mut totals = FusedTotals::default();
    for window in 0..ball.windows {
        let first = window * 9;
        let ravel_window = players.window(first)?;
        let ball_window = balls.window(first)?;
        for frame in fuse_window(
            first,
            ravel_window.raw_detections,
            ball_window.raw_detections,
        )? {
            totals.frames += 1;
            totals.players += frame.player_candidates.len() as u64;
            totals.ball_candidates += frame.ball_candidates.len() as u64;
            if let Some(best) = &frame.selected_ball {
                if best.source_model != "vballnet" {
                    return Err("Only VballNet may supply the selected ball".into());
                }
                totals.selected_vball += 1;
            }
            let mut line = serde_json::to_vec(&frame)
                .map_err(|e| format!("Serialize fused model frame: {e}"))?;
            line.push(b'\n');
            if line.len() > MAX_LINE {
                return Err("Fused frame exceeds safe output size".into());
            }
            writer
                .write_all(&line)
                .map_err(|e| format!("Write fusion output: {e}"))?;
            hash.update(&line);
        }
    }
    players.finish(ravel)?;
    balls.finish(ball)?;
    writer
        .flush()
        .map_err(|e| format!("Flush fused output: {e}"))?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|e| format!("Sync fused output: {e}"))?;
    Ok((format!("{:x}", hash.finalize()), totals))
}

pub(crate) fn do_fusion(state: &AppState, request: &FuseRequest) -> Result<Value, ApiError> {
    if !state.writes_enabled {
        return Err(storage::conflict(
            "GPU fusion writes require isolated VOLLEY_RUST_DATA_ROOT",
        ));
    }
    storage::project_id(&request.project_id)?;
    storage::video_name(&request.video)?;
    if !opaque_id(&request.person_job_id)
        || !opaque_id(&request.vball_job_id)
        || request.person_job_id == request.vball_job_id
    {
        return Err(storage::bad_request("Invalid or duplicate GPU job IDs"));
    }
    // Prevent a concurrent archive transaction while validating and publishing.
    let _lock = project_guard::write_guard(state, &request.project_id)?;
    let project = projects::existing_project(state, &request.project_id)?;
    if !project["videos"]
        .as_array()
        .is_some_and(|list| list.iter().any(|row| row.as_str() == Some(&request.video)))
    {
        return Err(storage::bad_request("Video not in project"));
    }
    let plan = preparation::plan_for(state, &request.project_id, &request.video)?;
    let prepared = plan
        .ready_path()
        .ok_or_else(|| storage::bad_request("Prepared video unavailable"))?;
    let (bytes, mtime) = inference_stream::source_stamp(prepared).map_err(storage::bad_request)?;
    let identity = json!({"bytes":bytes,"mtime_ns":mtime});
    let project_root = projects::project_root(state).join(&request.project_id);
    let root = project_root.join("native_gpu_runs");
    let ravel = open_evidence(
        &root,
        &request.person_job_id,
        &request.project_id,
        &request.video,
        "players",
    )
    .map_err(storage::bad_request)?;
    let ball = open_evidence(
        &root,
        &request.vball_job_id,
        &request.project_id,
        &request.video,
        "ball",
    )
    .map_err(storage::bad_request)?;
    if ravel.source_identity != identity || ball.source_identity != identity {
        return Err(storage::conflict(
            "Prepared source changed since original model runs",
        ));
    }
    let fusions = project_root.join("native_gpu_fusions");
    match fs::symlink_metadata(&fusions) {
        Ok(meta) if !meta.is_dir() || meta.file_type().is_symlink() => {
            return Err(storage::bad_request("Invalid GPU fusion output directory"))
        }
        Ok(_) => (),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => fs::create_dir(&fusions)
            .map_err(|e| storage::internal(format!("Create fusion directory: {e}")))?,
        Err(e) => return Err(storage::internal(format!("Check fusion output: {e}"))),
    }
    let fusion_id = Uuid::new_v4().simple().to_string();
    let output = fusions.join(&fusion_id);
    fs::create_dir(&output)
        .map_err(|e| storage::internal(format!("Create isolated fusion snapshot: {e}")))?;
    let result = (|| -> Result<Value, String> {
        let temp = output.join("frames.jsonl.partial");
        let (digest, totals) = write_fused(&ball, &ravel, &temp)?;
        // Reject changing either source between validation and publication.
        if inference_stream::source_stamp(prepared)? != (bytes, mtime) {
            return Err("Prepared video changed during fusion".into());
        }
        fs::rename(temp, output.join(OUTPUT_FILE))
            .map_err(|e| format!("Publish fusion JSONL: {e}"))?;
        let manifest = json!({
            "schema":"volley-native-gpu-fusion/1",
            "fusion_id":fusion_id,
            "project_id":request.project_id,
            "video":request.video,
            "state":"raw_candidates_fused",
            "sources":{
                "person_pose":{"job_id":ravel.job_id,"sha256":ravel.sha256},
                "vballnet":{"job_id":ball.job_id,"sha256":ball.sha256},
            },
            "source_identity":identity,
            "fps":30,
            "windows":ball.windows,
            "decoded_frames":ball.decoded_frames,
            "emitted_frames":totals.frames,
            "player_proposals":totals.players,
            "ball_candidates":totals.ball_candidates,
            "selected_ball_vballnet":totals.selected_vball,
            "ball_policy":"vballnet_only",
            "ball_gap_policy":"short_interpolation_in_processed_preview_only",
            "file":OUTPUT_FILE,
            "sha256":digest,
            "player_track_ids_confirmed":false,
            "touches_confirmed":false,
            "studio_gpu_run_complete":false,
            "cpu_processing_complete":false,
        });
        storage::write_json_atomic(&output.join("manifest.json"), &manifest)
            .map_err(|e| format!("Write fusion manifest: {e:?}"))?;
        Ok(manifest)
    })();
    match result {
        Ok(manifest) => Ok(manifest),
        Err(error) => {
            let _ = fs::remove_dir_all(&output);
            Err(storage::bad_request(error))
        }
    }
}

pub async fn fuse(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<FuseRequest>,
) -> ApiResult<Value> {
    storage::validate_origin(&headers)?;
    let result = tokio::task::spawn_blocking(move || do_fusion(&state, &request))
        .await
        .map_err(|e| storage::internal(format!("GPU fuse task: {e}")))??;
    Ok(Json(result))
}

#[derive(Deserialize)]
pub struct FusionQuery {
    project_id: String,
    fusion_id: String,
}
pub async fn manifest(
    State(state): State<Arc<AppState>>,
    Query(query): Query<FusionQuery>,
) -> ApiResult<Value> {
    storage::project_id(&query.project_id)?;
    if !opaque_id(&query.fusion_id) {
        return Err(storage::bad_request("Invalid fusion ID"));
    }
    projects::existing_project(&state, &query.project_id)?;
    let root = projects::project_root(&state)
        .join(&query.project_id)
        .join("native_gpu_fusions");
    let folder = verified_run_folder(&root, &query.fusion_id).map_err(storage::bad_request)?;
    let bytes = fs::read(
        existing_file(&folder, "manifest.json", MAX_MANIFEST).map_err(storage::bad_request)?,
    )
    .map_err(|e| storage::internal(format!("Read fusion manifest: {e}")))?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|e| storage::bad_request(format!("Invalid fusion manifest: {e}")))?;
    if value["schema"] != "volley-native-gpu-fusion/1"
        || value["fusion_id"].as_str() != Some(&query.fusion_id)
        || value["project_id"].as_str() != Some(&query.project_id)
    {
        return Err(storage::bad_request("Fusion manifest identity mismatch"));
    }
    Ok(Json(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn candidate(frame: u64, kind: &str, query: Option<usize>, score: f64) -> Candidate {
        Candidate {
            frame_index: frame,
            class_name: kind.into(),
            score,
            bbox_xyxy_norm: [0.1, 0.2, 0.4, 0.6],
            query_index: query,
            touch_probability: if kind == "player" { Some(0.8) } else { None },
            head_point_norm: None,
        }
    }
    fn build_input(
        folder: &Path,
        model: &str,
        id: &str,
        video: &str,
        rows: Vec<Vec<Candidate>>,
    ) -> ModelEvidence {
        fs::create_dir_all(folder).unwrap();
        let mut data = Vec::new();
        for (i, candidates) in rows.into_iter().enumerate() {
            let mut bytes = serde_json::to_vec(&json!({
                "first_frame":i*9,"raw_detections":candidates,
                "confirmed_tracks":false,"confirmed_touches":false,
            }))
            .unwrap();
            bytes.push(b'\n');
            data.extend(bytes);
        }
        fs::write(folder.join(SOURCE_FILE), &data).unwrap();
        let digest = format!("{:x}", Sha256::digest(&data));
        let manifest = json!({
            "schema":"volley-native-gpu-model-detections/1",
            "status":"model_raw_detections_complete",
            "project_id":"1234abcdef01","video":video,"job_id":id,"model":model,
            "raw_candidates_only":true,"studio_gpu_run_complete":false,
            "file":SOURCE_FILE,"fps":30,"stride":9,
            "windows":2,"decoded_frames":18,
            "source_identity":{"bytes":100,"mtime_ns":50},"sha256":digest
        });
        fs::write(
            folder.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        ModelEvidence {
            folder: folder.to_path_buf(),
            job_id: id.into(),
            sha256: digest,
            windows: 2,
            decoded_frames: 18,
            source_identity: manifest["source_identity"].clone(),
        }
    }
    #[test]
    fn fuses_verified_gpu_jsonl_stream_without_rerunning_models() {
        let root = std::env::temp_dir().join(format!("volley-native-fusion-{}", Uuid::new_v4()));
        let run = root.join("runs");
        fs::create_dir_all(&run).unwrap();
        let ravel_id = "0123456789abcdef0123456789abcdef";
        let ball_id = "abcdef0123456789abcdef0123456789";
        let ravel = build_input(
            &run.join(ravel_id),
            "players",
            ravel_id,
            "match.mp4",
            vec![
                vec![
                    candidate(0, "player", Some(3), 0.9),
                    candidate(0, "ball", None, 0.99),
                    candidate(1, "ball", None, 0.96),
                ],
                vec![candidate(9, "player", Some(2), 0.7)],
            ],
        );
        let ball = build_input(
            &run.join(ball_id),
            "ball",
            ball_id,
            "match.mp4",
            vec![
                vec![candidate(0, "ball", None, 0.2)],
                vec![candidate(9, "ball", None, 0.7)],
            ],
        );
        assert!(open_evidence(&run, ravel_id, "1234abcdef01", "match.mp4", "players").is_ok());
        assert!(open_evidence(&run, ball_id, "1234abcdef01", "match.mp4", "ball").is_ok());
        let out = root.join("merged.jsonl");
        let (sha, counts) = write_fused(&ball, &ravel, &out).unwrap();
        let bytes = fs::read(&out).unwrap();
        assert_eq!(sha, format!("{:x}", Sha256::digest(&bytes)));
        let rows = bytes
            .split(|v| *v == b'\n')
            .filter(|row| !row.is_empty())
            .map(|row| serde_json::from_slice::<Value>(row).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), 18);
        assert_eq!(counts.frames, 18);
        assert_eq!(counts.players, 2);
        assert_eq!(counts.selected_vball, 2);
        assert_eq!(rows[0]["selected_ball"]["source_model"], "vballnet");
        assert_eq!(rows[0]["ball_candidates"].as_array().unwrap().len(), 2);
        assert!(
            rows[1]["selected_ball"].is_null(),
            "RAVEL cannot fill a VballNet hole"
        );
        assert_eq!(rows[1]["ball_candidates"][0]["source_model"], "ravel");
        assert_eq!(rows[0]["player_candidates"][0]["touch_probability"], 0.8);
        assert_eq!(rows[9]["frame_index"], 9);
        assert_eq!(rows[0]["touches_confirmed"], false);
        fs::remove_dir_all(&root).unwrap();
    }
    #[test]
    fn tampered_source_and_mismatched_input_are_rejected() {
        let root =
            std::env::temp_dir().join(format!("volley-native-fusion-bad-{}", Uuid::new_v4()));
        let run = root.join("runs");
        fs::create_dir_all(&run).unwrap();
        let id = "0123456789abcdef0123456789abcdef";
        let ravel = build_input(
            &run.join(id),
            "players",
            id,
            "match.mp4",
            vec![vec![], vec![]],
        );
        let id2 = "abcdef0123456789abcdef0123456789";
        let ball = build_input(
            &run.join(id2),
            "ball",
            id2,
            "match.mp4",
            vec![vec![], vec![]],
        );
        assert!(open_evidence(&run, id, "1234abcdef01", "other.mp4", "players").is_err());
        let mut altered = fs::read(ball.folder.join(SOURCE_FILE)).unwrap();
        altered[0] = b' ';
        fs::write(ball.folder.join(SOURCE_FILE), altered).unwrap();
        assert!(write_fused(&ball, &ravel, &root.join("merged.jsonl")).is_err());
        fs::remove_dir_all(&root).unwrap();
    }
    #[test]
    fn isolated_project_fusion_publishes_verified_manifest_without_gpu() {
        use std::process::Command;
        if Command::new("ffmpeg").arg("-version").output().is_err() {
            return;
        }
        let root = std::env::temp_dir().join(format!("volley-fusion-api-{}", Uuid::new_v4()));
        let repo = root.join("source");
        let data = root.join("rust-data");
        let project_id = "1234abcdef01";
        let video = "fixture.mp4";
        fs::create_dir_all(repo.join("project")).unwrap();
        fs::create_dir_all(data.join("projects").join(project_id)).unwrap();
        let original = repo.join("project").join(video);
        let result = Command::new("ffmpeg")
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
                "-y",
            ])
            .arg(&original)
            .status()
            .unwrap();
        assert!(result.success());
        storage::write_json_atomic(
            &data.join("projects/projects.json"),
            &json!({"schema":1,"projects":[{
                "id":project_id,"name":"Test","videos":[video]
            }]}),
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
        let prepared = preparation::plan_for(&state, project_id, video).unwrap();
        assert!(prepared.ready_path().is_some());
        let (size, mtime) = inference_stream::source_stamp(&original).unwrap();
        let runs = projects::project_root(&state)
            .join(project_id)
            .join("native_gpu_runs");
        let rid = "0123456789abcdef0123456789abcdef";
        let bid = "abcdef0123456789abcdef0123456789";
        let players = build_input(
            &runs.join(rid),
            "players",
            rid,
            video,
            vec![vec![candidate(0, "player", Some(1), 0.9)], vec![]],
        );
        let balls = build_input(
            &runs.join(bid),
            "ball",
            bid,
            video,
            vec![vec![candidate(0, "ball", None, 0.55)], vec![]],
        );
        for model in [&players, &balls] {
            let path = model.folder.join("manifest.json");
            let mut m: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            m["source_identity"] = json!({"bytes":size,"mtime_ns":mtime});
            fs::write(&path, serde_json::to_vec(&m).unwrap()).unwrap();
        }
        let request = FuseRequest {
            project_id: project_id.into(),
            video: video.into(),
            person_job_id: rid.into(),
            vball_job_id: bid.into(),
        };
        let result = do_fusion(&state, &request).unwrap();
        assert_eq!(result["state"], "raw_candidates_fused");
        assert_eq!(result["emitted_frames"], 18);
        assert_eq!(result["player_proposals"], 1);
        assert_eq!(result["selected_ball_vballnet"], 1);
        assert_eq!(result["cpu_processing_complete"], false);
        let fusion_id = result["fusion_id"].as_str().unwrap();
        let folder = projects::project_root(&state)
            .join(project_id)
            .join("native_gpu_fusions")
            .join(fusion_id);
        let raw = fs::read(folder.join(OUTPUT_FILE)).unwrap();
        assert_eq!(
            result["sha256"].as_str(),
            Some(format!("{:x}", Sha256::digest(&raw)).as_str())
        );
        let reread = storage::read_json(&folder.join("manifest.json"))
            .unwrap()
            .unwrap();
        assert_eq!(reread, result);
        assert!(original.exists());
        assert!(!repo.join("results").exists());
        let wrong = FuseRequest {
            video: "other.mp4".into(),
            ..request
        };
        assert!(do_fusion(&state, &wrong).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_incomplete_overlap_or_unbounded_source_lines() {
        let root =
            std::env::temp_dir().join(format!("volley-native-fusion-invalid-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let id = "0123456789abcdef0123456789abcdef";
        let ravel = build_input(
            &root.join(id),
            "players",
            id,
            "match.mp4",
            vec![vec![], vec![]],
        );
        let mut ball = ravel.clone();
        ball.windows = 3;
        assert!(write_fused(&ball, &ravel, &root.join("merged.jsonl")).is_err());
        let mut ball = ravel.clone();
        ball.decoded_frames = 20;
        assert!(write_fused(&ball, &ravel, &root.join("other.jsonl")).is_err());
        fs::write(ravel.folder.join(SOURCE_FILE), vec![b'x'; MAX_LINE + 1]).unwrap();
        let mut reader = WindowReader::open(&ravel).unwrap();
        assert!(reader.next_line().is_err());
        assert!(!opaque_id("../unsafe"));
        fs::remove_dir_all(&root).unwrap();
    }
}
