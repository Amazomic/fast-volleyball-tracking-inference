# RTMPose in Volley Studio

RTMPose-M Body7 (official ONNX, 256 x 192, 17 COCO joints) runs as an independent CUDA stage
AFTER VballNet/RAVEL GPU fusion. It operates on saved player proposals and prepared RGB frames.
It stores immutable JSONL evidence under native_gpu_pose_runs/<id>/ with a content digest and
parent fusion/source identity. No track or verified event is created by pose inference.

CPU touch postprocessing first detects motion-supported ball contacts and links a same-frame
Track ID. It optionally joins a saved RTMPose sample by nearest-frame and player-box IoU,
then uses wrists/elbows/shoulders to refine attack, overhead pass, dig or low touch.
Missing/weak pose evidence falls back to the Python-parity geometric predictor.
Block and serve require verified court and rally context and are not inferred from pose alone.
These are hypotheses, NOT human-confirmed match actions.

Primary GPU flags: pose_enabled=true and pose_stride=6 by default.
Postprocessing: pose_classification=true by default; CPU-only runs do not start GPU.
Preview: the Skelеtons button reads the saved evidence, restricted to rally scopes.

Portable one-time model setup (Python is optional and only used to fetch weights):
python scripts/setup_rtmpose_model.py

Installer downloads the official model zip, verifies its SHA256, then verifies and publishes
the ONNX model. Model binaries are not committed to Git. ONNX Runtime CUDA must be installed
for Rust GPU inference. A failed model load never silently falls back to CPU.

Measured accuracy on real volleyball footage is not yet established. Validate with a labeled
sample of distant/occluded athletes and attack, set, dig, block, serve contacts; quantify
per-class precision/recall, pose coverage, VRAM and end-to-end latency.
