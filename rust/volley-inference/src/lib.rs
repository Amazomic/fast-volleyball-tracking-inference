//! Standalone ONNX Runtime engine. No Python process, model inference cache,
//! browser hooks or silent CUDA -> CPU fallback. The 9-frame clip is fed from
//! a future native video decoder; source preprocessing is a separate stage.
pub mod decode;

use ort::{
    execution_providers::{CPUExecutionProvider, CUDAExecutionProvider, ExecutionProvider},
    session::Session,
    value::Tensor,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

const BALL_FILE: &str = "VballNetV4c_seq9_grayscale_20260908_213829.onnx";
const PLAYER_FILE: &str = "RAVEL-VB-013-9f.onnx";
const MAX_MODEL_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_OUTPUT_VALUES: usize = 25_000_000;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelKind {
    Ball,
    Players,
}

impl ModelKind {
    pub fn filename(self) -> &'static str {
        match self {
            Self::Ball => BALL_FILE,
            Self::Players => PLAYER_FILE,
        }
    }
    pub fn expected_input(self) -> &'static [i64] {
        match self {
            Self::Ball => &[1, 9, 288, 512],
            Self::Players => &[1, 9, 3, 360, 640],
        }
    }
    pub fn input_name(self) -> &'static str {
        match self {
            Self::Ball => "clip",
            Self::Players => "frames",
        }
    }
    pub fn required_outputs(self) -> &'static [&'static str] {
        match self {
            Self::Ball => &["heatmaps"],
            Self::Players => &[
                "logits",
                "boxes",
                "temporal_association",
                "action_logits",
                "action_actor_logits",
                "ball_grid",
                "ball_size",
            ],
        }
    }
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Device {
    Cuda,
    Cpu,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub model: ModelKind,
    pub device: Device,
    #[serde(default)]
    pub gpu_device_id: i32,
    #[serde(default)]
    pub gpu_memory_limit_mb: u64,
    #[serde(default = "default_threads")]
    pub cpu_threads: usize,
}
fn default_threads() -> usize {
    8
}
impl Config {
    pub fn verify(&self) -> Result<(), String> {
        if !(0..=15).contains(&self.gpu_device_id)
            || self.gpu_memory_limit_mb > 131072
            || !(1..=128).contains(&self.cpu_threads)
        {
            return Err("Invalid CUDA device, GPU memory limit or CPU threads".into());
        }
        if self.device == Device::Cpu && (self.gpu_device_id != 0 || self.gpu_memory_limit_mb != 0)
        {
            return Err("CPU session cannot request CUDA device or GPU memory limit".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ModelSpec {
    pub kind: ModelKind,
    pub file: String,
    pub size_bytes: u64,
    pub input_name: String,
    pub input_shape: Vec<i64>,
    pub outputs: Vec<String>,
    pub output_expected: String,
    pub runtime_required: String,
}

fn spec_for(repo: &Path, kind: ModelKind) -> Result<(ModelSpec, PathBuf), String> {
    let models = repo
        .join("models")
        .canonicalize()
        .map_err(|e| format!("Cannot locate canonical models directory: {e}"))?;
    let model = models
        .join(kind.filename())
        .canonicalize()
        .map_err(|e| format!("ONNX model missing: {e}"))?;
    if !model.starts_with(&models) || !model.is_file() {
        return Err("ONNX model is outside the approved repository models directory".into());
    }
    let size = model
        .metadata()
        .map_err(|e| format!("ONNX size: {e}"))?
        .len();
    if size < 1024 || size > MAX_MODEL_BYTES {
        return Err("ONNX model size is invalid".into());
    }
    let metadata = model.with_extension("json");
    let bytes = fs::read(&metadata).map_err(|e| format!("Missing ONNX sidecar metadata: {e}"))?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err("ONNX sidecar too large".into());
    }
    let sidecar: Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("Invalid ONNX sidecar: {e}"))?;
    let input = sidecar["input_name"]
        .as_str()
        .ok_or("Sidecar has no input_name")?;
    if input != kind.input_name() {
        return Err("ONNX model input name mismatch".into());
    }
    let shape = sidecar["input_shape"]
        .as_array()
        .ok_or("Missing model input_shape")?
        .iter()
        .map(|value| {
            value
                .as_i64()
                .ok_or_else(|| "Bad input dimension".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    if shape != kind.expected_input() {
        return Err(format!("Unexpected ONNX input shape: {shape:?}"));
    }
    let outputs = sidecar["output_names"]
        .as_array()
        .ok_or("Missing model outputs")?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| "Bad output name".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let output_set = outputs.iter().map(String::as_str).collect::<HashSet<_>>();
    if outputs.len() != output_set.len()
        || outputs.len() > 32
        || kind
            .required_outputs()
            .iter()
            .any(|name| !output_set.contains(name))
    {
        return Err("ONNX sidecar lacks required outputs, or has duplicate names".into());
    }
    if kind == ModelKind::Ball {
        let expected = sidecar["output_shape"]
            .as_array()
            .ok_or("VballNet output_shape missing")?
            .iter()
            .map(|v| v.as_i64())
            .collect::<Option<Vec<_>>>()
            .ok_or("Invalid VballNet output shape")?;
        if expected != [1, 18, 288, 512] {
            return Err("Incompatible VballNet heatmap/radius layout".into());
        }
    } else {
        let version = sidecar["release_id"].as_str();
        if version != Some("RAVEL-VB-013")
            || sidecar["action_names"]
                .as_array()
                .is_none_or(|v| v.len() != 2 || v[0] != "Touch" || v[1] != "none")
        {
            return Err("Incompatible RAVEL-013 Touch release".into());
        }
    }
    let spec=ModelSpec{
        kind,file:model.to_string_lossy().into_owned(),
        size_bytes:size,input_name:input.to_owned(),input_shape:shape,
        outputs,
        output_expected:match kind{
            ModelKind::Ball=>"[1,18,288,512] f32 heatmap[9] + radius[9]".into(),
            ModelKind::Players=>"logits/boxes/association/ball/Touch named tensors".into(),
        },
        runtime_required:"ONNX Runtime shared library (ORT_DYLIB_PATH), CUDAExecutionProvider for cuda; no Python fallback".into(),
    };
    Ok((spec, model))
}
/// A read-only model preflight: no session, no GPU memory allocation.
pub fn inspect(repo: &Path, kind: ModelKind) -> Result<ModelSpec, String> {
    spec_for(repo, kind).map(|(spec, _)| spec)
}

#[derive(Debug)]
pub struct Output {
    pub name: String,
    pub dimensions: Vec<i64>,
    pub data: Vec<f32>,
}

pub struct Engine {
    model: ModelSpec,
    session: Session,
}
impl Engine {
    /// Commit one native ONNX Runtime session, strictly to requested backend.
    ///
    /// ORT_DYLIB_PATH must point to the right ONNX Runtime 1.22 ABI shared
    /// library. A cuda request may NEVER automatically use CPU only.
    pub fn load(repo: &Path, config: &Config) -> Result<Self, String> {
        config.verify()?;
        let (model, path) = spec_for(repo, config.model)?;
        // load-dynamic is configured at compile time; process startup must
        // load a compatible ONNX Runtime native library through ort.
        ort::init()
            .commit()
            .map_err(|e| format!("ONNX Runtime initialization: {e}"))?;
        let mut builder = Session::builder()
            .map_err(|e| format!("ONNX session builder: {e}"))?
            .with_intra_threads(config.cpu_threads)
            .map_err(|e| format!("ONNX CPU thread config: {e}"))?;
        if config.device == Device::Cuda {
            let mut cuda = CUDAExecutionProvider::default().with_device_id(config.gpu_device_id);
            if config.gpu_memory_limit_mb > 0 {
                let bytes = usize::try_from(config.gpu_memory_limit_mb * 1024 * 1024)
                    .map_err(|_| "GPU memory limit exceeds target platform address space")?;
                cuda = cuda.with_memory_limit(bytes);
            }
            if !cuda
                .is_available()
                .map_err(|e| format!("CUDA availability: {e}"))?
            {
                return Err(
                    "CUDAExecutionProvider unavailable; refusing silent CPU fallback".into(),
                );
            }
            builder = builder
                .with_execution_providers([cuda.build().error_on_failure()])
                .map_err(|e| format!("Cannot register CUDAExecutionProvider: {e}"))?;
        } else {
            builder = builder
                .with_execution_providers([CPUExecutionProvider::default().build()])
                .map_err(|e| format!("Cannot register CPUExecutionProvider: {e}"))?;
        }
        let session = builder
            .commit_from_file(&path)
            .map_err(|e| format!("ONNX model session initialization: {e}"))?;
        let names = session
            .inputs
            .iter()
            .map(|x| x.name.as_str())
            .collect::<Vec<_>>();
        if !names.contains(&model.input_name.as_str()) {
            return Err("Loaded ONNX session input differs from model sidecar".into());
        }
        let produced = session
            .outputs
            .iter()
            .map(|x| x.name.as_str())
            .collect::<HashSet<_>>();
        if model.outputs.iter().any(|x| !produced.contains(x.as_str())) {
            return Err("Loaded ONNX session is missing declared outputs".into());
        }
        Ok(Self { model, session })
    }
    pub fn spec(&self) -> &ModelSpec {
        &self.model
    }
    /// Run exactly one 9-frame clip from the native decoder/preprocessor.
    /// Return all named model tensors, raw; CPU postprocessing owns their
    /// interpretation (and keeps probability extrema for later tuning).
    pub fn infer_clip(&mut self, frames: Vec<f32>) -> Result<Vec<Output>, String> {
        let expected = self
            .model
            .input_shape
            .iter()
            .try_fold(1usize, |acc, &x| acc.checked_mul(usize::try_from(x).ok()?))
            .ok_or("Invalid ONNX tensor shape")?;
        if frames.len() != expected || frames.iter().any(|v| !v.is_finite()) {
            return Err(format!(
                "Expected {expected} finite f32 input pixels, got {}",
                frames.len()
            ));
        }
        let tensor = Tensor::from_array((self.model.input_shape.clone(), frames))
            .map_err(|e| format!("Create ONNX input tensor: {e}"))?;
        let outputs = self
            .session
            .run(ort::inputs![self.model.input_name.as_str()=>tensor])
            .map_err(|e| format!("Run ONNX inference: {e}"))?;
        let mut result = Vec::new();
        for name in &self.model.outputs {
            let value = outputs
                .get(name.as_str())
                .ok_or_else(|| format!("Missing output tensor: {name}"))?;
            let (dimensions, data) = value
                .try_extract_tensor::<f32>()
                .map_err(|e| format!("Invalid ONNX tensor {name}: {e}"))?;
            if data.len() > MAX_OUTPUT_VALUES {
                return Err(format!("ONNX output {name} exceeds safety limit"));
            }
            result.push(Output {
                name: name.clone(),
                dimensions: dimensions.iter().copied().collect(),
                data: data.to_vec(),
            });
        }
        Ok(result)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_shapes() {
        assert_eq!(ModelKind::Players.expected_input(), &[1, 9, 3, 360, 640]);
        assert_eq!(ModelKind::Ball.expected_input(), &[1, 9, 288, 512]);
    }
    #[test]
    fn gpu_memory_is_bounded() {
        assert!(Config {
            model: ModelKind::Ball,
            device: Device::Cuda,
            gpu_device_id: 0,
            gpu_memory_limit_mb: 0,
            cpu_threads: 8
        }
        .verify()
        .is_ok());
        assert!(Config {
            model: ModelKind::Ball,
            device: Device::Cpu,
            gpu_device_id: 0,
            gpu_memory_limit_mb: 1024,
            cpu_threads: 8
        }
        .verify()
        .is_err());
    }

    /// Explicit integration smoke, disabled in normal CI: needs original
    /// ONNX models and an ABI-compatible native ORT_DYLIB_PATH (CUDA optional).
    #[test]
    #[ignore = "Requires the installed ONNX Runtime shared library and model files"]
    fn native_onnx_clip_infers_and_decodes() {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let device = match std::env::var("VOLLEY_TEST_ORT_DEVICE")
            .unwrap_or_else(|_| "cpu".into())
            .as_str()
        {
            "cuda" => Device::Cuda,
            "cpu" => Device::Cpu,
            other => panic!("Unexpected VOLLEY_TEST_ORT_DEVICE: {other}"),
        };
        for kind in [ModelKind::Ball, ModelKind::Players] {
            let config = Config {
                model: kind,
                device,
                gpu_device_id: 0,
                gpu_memory_limit_mb: 0,
                cpu_threads: 2,
            };
            let mut engine = Engine::load(repo, &config)
                .unwrap_or_else(|e| panic!("Cannot open native {kind:?} session: {e}"));
            let count = kind
                .expected_input()
                .iter()
                .map(|&v| usize::try_from(v).unwrap())
                .product::<usize>();
            let results = engine
                .infer_clip(vec![0.0_f32; count])
                .unwrap_or_else(|e| panic!("Native {kind:?} inference failed: {e}"));
            assert_eq!(results.len(), engine.spec().outputs.len());
            let detections = match kind {
                ModelKind::Ball => decode::decode_vball(&results, 0, 0.05),
                ModelKind::Players => {
                    decode::decode_ravel(&results, 0, decode::DecodeConfig::default())
                }
            }
            .unwrap_or_else(|e| panic!("Native {kind:?} tensor decoding failed: {e}"));
            assert!(detections.iter().all(|row| row.frame_index < 9));
            println!(
                "Real native {kind:?} {device:?}: {} tensors, {} raw detections",
                results.len(),
                detections.len()
            );
        }
    }
}
