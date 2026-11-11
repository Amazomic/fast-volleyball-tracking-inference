fn lowercase_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn job_id(value: &str) -> bool {
    lowercase_hex(value, 32)
}
pub fn player_id(value: &str) -> bool {
    lowercase_hex(value, 12)
}
pub fn sha256(value: &str) -> bool {
    lowercase_hex(value, 64)
}
pub fn gpu_manifest(
    manifest: &serde_json::Value,
    project: &str,
    video: &str,
    job: &str,
    model: &str,
) -> bool {
    manifest["schema"] == "volley-native-gpu-model-detections/1"
        && manifest["status"] == "model_raw_detections_complete"
        && manifest["raw_candidates_only"] == true
        && manifest["studio_gpu_run_complete"] == false
        && manifest["project_id"].as_str() == Some(project)
        && manifest["video"].as_str() == Some(video)
        && manifest["job_id"].as_str() == Some(job)
        && manifest["model"].as_str() == Some(model)
        && manifest["file"] == "detections.jsonl"
        && manifest["fps"] == 30
        && manifest["stride"] == 9
}

pub fn candidate_id(value: &str) -> bool {
    lowercase_hex(value, 24)
}
