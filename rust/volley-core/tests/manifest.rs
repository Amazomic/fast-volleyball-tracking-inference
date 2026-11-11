use serde_json::json;
use volley_core::contracts::gpu_manifest;

#[test]
fn gpu_manifest_checks_scope_stride_and_provisional_contract() {
    let mut manifest = json!({"schema":"volley-native-gpu-model-detections/1","status":"model_raw_detections_complete","raw_candidates_only":true,"studio_gpu_run_complete":false,"project_id":"project","video":"a.mp4","job_id":"job","model":"ball","file":"detections.jsonl","fps":30,"stride":9});
    assert!(gpu_manifest(&manifest, "project", "a.mp4", "job", "ball"));
    for (key, value) in [
        ("stride", json!(1)),
        ("video", json!("b.mp4")),
        ("raw_candidates_only", json!(false)),
        ("studio_gpu_run_complete", json!(true)),
    ] {
        let old = manifest[key].clone();
        manifest[key] = value;
        assert!(!gpu_manifest(&manifest, "project", "a.mp4", "job", "ball"));
        manifest[key] = old;
    }
}
