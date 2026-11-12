//! Direct native OpenVINO C-ABI inference for the bundled 0287 person ReID IR.
//! No Python interpreter or old Studio process participates in this pipeline.
//! Output is a 256-dimensional visual descriptor, never an athlete identity.
use libloading::Library;
use sha2::{Digest, Sha256};
use std::{
    ffi::{c_char, c_void, CString},
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    ptr,
};

pub const MODEL_NAME: &str = "person-reidentification-retail-0287";
pub const PREPROCESS: &str = "raw-bgr-f32-nchw-1x3x256x128-v1";
pub const WIDTH: usize = 128;
pub const HEIGHT: usize = 256;
pub const PIXELS: usize = WIDTH * HEIGHT;
pub const DIMENSIONS: usize = 256;
const INPUT_LEN: usize = PIXELS * 3;
const MAX_MODEL: u64 = 32 * 1024 * 1024;

fn model_paths(repository: &Path) -> (PathBuf, PathBuf) {
    let folder = repository.join("models/openvino");
    (
        folder.join(format!("{MODEL_NAME}.xml")),
        folder.join(format!("{MODEL_NAME}.bin")),
    )
}
fn stream_hash(path: &Path, digest: &mut Sha256) -> Result<(), String> {
    let mut file = File::open(path).map_err(|e| format!("Cannot read ReID model: {e}"))?;
    let len = file.metadata().map_err(|e| e.to_string())?.len();
    if len == 0 || len > MAX_MODEL || !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Invalid or oversized native ReID model".into());
    }
    let mut buf = [0_u8; 65536];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("Read ReID weights: {e}"))?;
        if n == 0 {
            break;
        }
        digest.update(&buf[..n]);
    }
    Ok(())
}

/// The model identifier includes BOTH weight files and the exact BGR conversion,
/// so visually similar vectors from a different model are never compared.
pub fn fingerprint(repository: &Path) -> Result<String, String> {
    let (xml, bin) = model_paths(repository);
    let mut hash = Sha256::new();
    hash.update(MODEL_NAME.as_bytes());
    hash.update(b"\0");
    hash.update(PREPROCESS.as_bytes());
    hash.update(b"\0xml\0");
    stream_hash(&xml, &mut hash)?;
    hash.update(b"\0bin\0");
    stream_hash(&bin, &mut hash)?;
    Ok(format!("{:x}", hash.finalize()))
}

/// Convert EXACT BGR24 FFmpeg output into floating point CHW, with no hidden
/// color swap, mean centering or scale to [0..1].
pub fn bgr24_to_nchw(src: &[u8]) -> Result<Vec<f32>, String> {
    if src.len() != INPUT_LEN {
        return Err("Expected exactly one 128x256 BGR24 crop".into());
    }
    let mut output = vec![0_f32; INPUT_LEN];
    for (p, color) in src.as_chunks::<3>().0.iter().enumerate() {
        for channel in 0..3 {
            output[channel * PIXELS + p] = color[channel] as f32;
        }
    }
    Ok(output)
}

/// Visual matching requires a unit vector; zero/NaN/Inf descriptors never
/// silently enter the ReID similarity service.
pub fn normalize_embedding(raw: &[f32]) -> Result<Vec<f32>, String> {
    if raw.len() != DIMENSIONS || raw.iter().any(|v| !v.is_finite()) {
        return Err("Invalid 256-dimensional ReID model output".into());
    }
    let squared = raw.iter().map(|v| (*v as f64).powi(2)).sum::<f64>();
    if squared <= 1e-12 || !squared.is_finite() {
        return Err("OpenVINO produced a zero/non-finite ReID vector".into());
    }
    let norm = squared.sqrt();
    Ok(raw.iter().map(|v| (*v as f64 / norm) as f32).collect())
}

#[repr(C)]
struct OvShape {
    rank: i64,
    dims: *mut i64,
}
type Handle = *mut c_void;
type Status = i32;
type CoreCreate = unsafe extern "C" fn(*mut Handle) -> Status;
type CoreCompile = unsafe extern "C" fn(
    *const c_void,
    *const c_char,
    *const c_char,
    usize,
    *mut Handle,
    ...
) -> Status;
type RequestCreate = unsafe extern "C" fn(*const c_void, *mut Handle) -> Status;
type TensorCreate = unsafe extern "C" fn(i32, OvShape, *mut Handle) -> Status;
type TensorData = unsafe extern "C" fn(*const c_void, *mut Handle) -> Status;
type TensorSize = unsafe extern "C" fn(*const c_void, *mut usize) -> Status;
type TensorType = unsafe extern "C" fn(*const c_void, *mut i32) -> Status;
type RequestSet = unsafe extern "C" fn(Handle, *const c_void) -> Status;
type RequestInfer = unsafe extern "C" fn(Handle) -> Status;
type RequestGet = unsafe extern "C" fn(*const c_void, *mut Handle) -> Status;
type Free = unsafe extern "C" fn(Handle);

macro_rules! symbol {
    ($lib:expr, $name:literal, $kind:ty) => {{
        // SAFETY: each declaration is transcribed from the shipped OpenVINO C
        // headers; lib remains alive for the complete encoder lifetime.
        unsafe {
            *$lib
                .get::<$kind>(concat!($name, "\0").as_bytes())
                .map_err(|e| format!("Missing OpenVINO symbol {}: {e}", $name))?
        }
    }};
}
fn check(status: Status, name: &str) -> Result<(), String> {
    if status == 0 {
        Ok(())
    } else {
        Err(format!("OpenVINO {name} failed (status={status})"))
    }
}

/// Own all native handles. There is one compiled session per request batch;
/// an aborted or invalid call closes all handles deterministically.
pub struct Encoder {
    _lib: Library,
    core: Handle,
    compiled: Handle,
    request: Handle,
    tensor: Handle,
    tensor_create: TensorCreate,
    tensor_data: TensorData,
    tensor_size: TensorSize,
    tensor_type: TensorType,
    request_set: RequestSet,
    request_infer: RequestInfer,
    request_get: RequestGet,
    core_free: Free,
    compiled_free: Free,
    request_free: Free,
    tensor_free: Free,
}
impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: OpenVINO owns each non-null opaque handle; request and tensor
        // must be freed before the compiled model and library are unloaded.
        unsafe {
            if !self.tensor.is_null() {
                (self.tensor_free)(self.tensor);
            }
            if !self.request.is_null() {
                (self.request_free)(self.request);
            }
            if !self.compiled.is_null() {
                (self.compiled_free)(self.compiled);
            }
            if !self.core.is_null() {
                (self.core_free)(self.core);
            }
        }
    }
}
impl Encoder {
    /// Requires explicit absolute native .so/.dll path, no automatic fallback.
    /// Run-time errors fail closed instead of pretending the track is matched.
    pub fn load(repository: &Path, library_path: &Path) -> Result<Self, String> {
        if !library_path.is_absolute() {
            return Err("VOLLEY_OPENVINO_C_LIB must be an absolute shared-library path".into());
        }
        let (xml, bin) = model_paths(repository);
        fingerprint(repository)?; // model files must exist and be within bounds
                                  // SAFETY: the configured library is an administrator-supplied native
                                  // OpenVINO runtime; the API is resolved and called by the declared ABI.
        let lib = unsafe { Library::new(library_path) }
            .map_err(|e| format!("Load OpenVINO native library: {e}"))?;
        let core_create = symbol!(lib, "ov_core_create", CoreCreate);
        let core_compile = symbol!(lib, "ov_core_compile_model_from_file", CoreCompile);
        let request_create = symbol!(lib, "ov_compiled_model_create_infer_request", RequestCreate);
        let core_free = symbol!(lib, "ov_core_free", Free);
        let compiled_free = symbol!(lib, "ov_compiled_model_free", Free);
        let request_free = symbol!(lib, "ov_infer_request_free", Free);
        let tensor_free = symbol!(lib, "ov_tensor_free", Free);
        let encoder = Self {
            tensor_create: symbol!(lib, "ov_tensor_create", TensorCreate),
            tensor_data: symbol!(lib, "ov_tensor_data", TensorData),
            tensor_size: symbol!(lib, "ov_tensor_get_size", TensorSize),
            tensor_type: symbol!(lib, "ov_tensor_get_element_type", TensorType),
            request_set: symbol!(lib, "ov_infer_request_set_input_tensor", RequestSet),
            request_infer: symbol!(lib, "ov_infer_request_infer", RequestInfer),
            request_get: symbol!(lib, "ov_infer_request_get_output_tensor", RequestGet),
            _lib: lib,
            core: ptr::null_mut(),
            compiled: ptr::null_mut(),
            request: ptr::null_mut(),
            tensor: ptr::null_mut(),
            core_free,
            compiled_free,
            request_free,
            tensor_free,
        };
        let mut encoder = encoder;
        let model = CString::new(xml.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
        // The XML resolves the adjacent .bin file. Both file contents are
        // included in the caller-visible model fingerprint.
        let _ = bin;
        let cpu = CString::new("CPU").expect("literal CPU");
        // SAFETY: all pointers are either OpenVINO-owned or CString-backed and
        // remain alive during the C call; varargs count is zero.
        unsafe {
            check(core_create(&mut encoder.core), "core_create")?;
            check(
                core_compile(
                    encoder.core,
                    model.as_ptr(),
                    cpu.as_ptr(),
                    0,
                    &mut encoder.compiled,
                ),
                "compile_model_from_file",
            )?;
            check(
                request_create(encoder.compiled, &mut encoder.request),
                "create_infer_request",
            )?;
            let mut dims = [1_i64, 3, HEIGHT as i64, WIDTH as i64];
            let shape = OvShape {
                rank: 4,
                dims: dims.as_mut_ptr(),
            };
            // OpenVINO C element type F32 == 4 (ov_common.h).
            check(
                (encoder.tensor_create)(4, shape, &mut encoder.tensor),
                "tensor_create",
            )?;
        }
        Ok(encoder)
    }

    /// Input already CHW F32, BGR channel order, with 0..255 pixel values.
    /// The returned descriptor is L2-normalized and unconfirmed.
    pub fn infer(&mut self, input: &[f32]) -> Result<Vec<f32>, String> {
        if input.len() != INPUT_LEN
            || input
                .iter()
                .any(|v| !v.is_finite() || *v < 0.0 || *v > 255.0)
        {
            return Err("Invalid ReID BGR F32 image tensor".into());
        }
        // SAFETY: native tensor size is checked before copying; pointers remain
        // valid until output is copied and the corresponding tensor is freed.
        unsafe {
            let mut size = 0;
            check((self.tensor_size)(self.tensor, &mut size), "input_size")?;
            if size != INPUT_LEN {
                return Err("Unexpected compiled ReID input shape".into());
            }
            let mut data = ptr::null_mut();
            check((self.tensor_data)(self.tensor, &mut data), "input_data")?;
            if data.is_null() {
                return Err("OpenVINO input pointer is null".into());
            }
            ptr::copy_nonoverlapping(input.as_ptr(), data as *mut f32, INPUT_LEN);
            check((self.request_set)(self.request, self.tensor), "set_input")?;
            check((self.request_infer)(self.request), "infer")?;
            let mut output: Handle = ptr::null_mut();
            check((self.request_get)(self.request, &mut output), "get_output")?;
            if output.is_null() {
                return Err("OpenVINO output pointer is null".into());
            }
            // Output tensor is separately owned; free even when validation fails.
            let result = (|| {
                let mut output_len = 0;
                check((self.tensor_size)(output, &mut output_len), "output_size")?;
                let mut output_type = 0;
                check((self.tensor_type)(output, &mut output_type), "output_type")?;
                if output_len != DIMENSIONS || output_type != 4 {
                    return Err("OpenVINO ReID output must be 256 FP32 elements".into());
                }
                let mut raw = ptr::null_mut();
                check((self.tensor_data)(output, &mut raw), "output_data")?;
                if raw.is_null() {
                    return Err("OpenVINO output memory unavailable".into());
                }
                let values = std::slice::from_raw_parts(raw as *const f32, DIMENSIONS);
                normalize_embedding(values)
            })();
            (self.tensor_free)(output);
            result
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bgr_preprocessing_is_channel_first_not_rgb() {
        let mut data = vec![0_u8; INPUT_LEN];
        data[0..3].copy_from_slice(&[11, 21, 31]);
        data[3..6].copy_from_slice(&[12, 22, 32]);
        let float = bgr24_to_nchw(&data).unwrap();
        assert_eq!(&float[..2], &[11.0, 12.0]);
        assert_eq!(&float[PIXELS..PIXELS + 2], &[21.0, 22.0]);
        assert_eq!(&float[2 * PIXELS..2 * PIXELS + 2], &[31.0, 32.0]);
        assert!(bgr24_to_nchw(&data[..data.len() - 1]).is_err());
    }
    #[test]
    fn normalization_rejects_bad_features() {
        assert!(normalize_embedding(&[0.0; DIMENSIONS]).is_err());
        let mut nan = [0.0; DIMENSIONS];
        nan[42] = f32::NAN;
        assert!(normalize_embedding(&nan).is_err());
        let mut values = [0.0; DIMENSIONS];
        values[0] = 3.0;
        values[1] = 4.0;
        let unit = normalize_embedding(&values).unwrap();
        assert!((unit[0] - 0.6).abs() < 1e-6);
        assert!((unit[1] - 0.8).abs() < 1e-6);
        assert!(normalize_embedding(&values[..255]).is_err());
    }
    #[test]
    fn model_fingerprint_changes_if_either_weight_changes() {
        let root = std::env::temp_dir().join(format!("reid-model-test-{}", std::process::id()));
        assert_eq!(
            model_paths(&root).0.parent().unwrap(),
            root.join("models/openvino")
        );
        let dir = root.join("models/openvino");
        std::fs::create_dir_all(&dir).unwrap();
        let (xml, bin) = model_paths(&root);
        std::fs::write(&xml, b"<net/>").unwrap();
        std::fs::write(&bin, b"weights-a").unwrap();
        let a = fingerprint(&root).unwrap();
        assert_eq!(a.len(), 64);
        std::fs::write(&bin, b"weights-b").unwrap();
        assert_ne!(a, fingerprint(&root).unwrap());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn actual_openvino_model_vector_if_native_library_configured() {
        let Ok(library) = std::env::var("VOLLEY_TEST_OPENVINO_C_LIB") else {
            return;
        };
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut encoder = Encoder::load(&root, Path::new(&library)).unwrap();
        let mut data = vec![0_u8; INPUT_LEN];
        for (pixel, rgb) in data.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            let x = (pixel % WIDTH) as u8;
            let y = (pixel / WIDTH) as u8;
            rgb.copy_from_slice(&[x, y, x.wrapping_add(y)]);
        }
        let input = bgr24_to_nchw(&data).unwrap();
        let a = encoder.infer(&input).unwrap();
        let b = encoder.infer(&input).unwrap();
        assert_eq!(a.len(), 256);
        assert!(a.iter().all(|v| v.is_finite()));
        let norm = a.iter().map(|x| (*x as f64).powi(2)).sum::<f64>();
        assert!((norm - 1.0).abs() < 0.00001);
        let dot = a
            .iter()
            .zip(&b)
            .map(|(x, y)| (*x as f64) * (*y as f64))
            .sum::<f64>();
        assert!(dot > 0.999);
    }
}
