//! Bounded, pure-Rust reader for NumPy .npz ReID cached embeddings.
//! Handles .npy v1/v2/v3, little-endian float16/float32, Unicode ID lists.
//! Rejects pickle/object, mixed dimensions, huge allocations and stale input.

use half::f16;
use std::{
    fs::File,
    io::{Read, Seek},
    path::Path,
};
use volley_core::reid::{Feature, FeatureMode};
use zip::ZipArchive;

struct Npy {
    dtype: String,
    shape: Vec<usize>,
    data: Vec<u8>,
}

fn extract_field<'a>(header: &'a str, name: &str) -> Result<&'a str, String> {
    let start = header
        .find(&format!("'{name}'"))
        .ok_or_else(|| format!("Missing NumPy header field {name}"))?;
    Ok(header[start..]
        .split_once(':')
        .ok_or_else(|| format!("Malformed NumPy header {name}"))?
        .1
        .trim_start())
}

fn parse_npy(bytes: Vec<u8>) -> Result<Npy, String> {
    if bytes.len() < 10 || &bytes[..6] != b"\x93NUMPY" {
        return Err("Not a NumPy .npy array".into());
    }
    let major = bytes[6];
    let (head_len, start) = match major {
        1 => (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10),
        2 | 3 if bytes.len() >= 12 => (
            u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize,
            12,
        ),
        _ => return Err("Unsupported NumPy header version".into()),
    };
    if head_len > 65_536 || bytes.len() < start + head_len {
        return Err("Truncated or oversized NumPy header".into());
    }
    let header = std::str::from_utf8(&bytes[start..start + head_len])
        .map_err(|_| "Invalid NumPy header encoding")?;
    let descr = extract_field(header, "descr")?;
    let quote = descr.chars().next().ok_or("Empty NumPy dtype")?;
    if quote != '\'' && quote != '"' {
        return Err("Invalid NumPy dtype".into());
    }
    let dtype = descr[1..]
        .split(quote)
        .next()
        .ok_or("Missing NumPy dtype")?
        .to_owned();
    if !matches!(dtype.chars().next(), Some('<' | '|' | '=')) {
        return Err("Only native/little-endian NumPy arrays supported".into());
    }
    let order = extract_field(header, "fortran_order")?;
    if !order.starts_with("False") {
        return Err("Fortran-order NumPy arrays unsupported".into());
    }
    let dimensions = extract_field(header, "shape")?;
    let tuple = dimensions
        .strip_prefix('(')
        .and_then(|s| s.split_once(')').map(|(row, _)| row))
        .ok_or("Invalid NumPy shape")?;
    let shape = tuple
        .split(',')
        .filter_map(|part| {
            let text = part.trim();
            if text.is_empty() {
                None
            } else {
                Some(text.parse::<usize>())
            }
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "Non-numeric NumPy shape")?;
    if shape.len() > 2 || shape.iter().any(|size| *size > 20_000) {
        return Err("Unsupported NumPy dimensions".into());
    }
    Ok(Npy {
        dtype,
        shape,
        data: bytes[start + head_len..].to_vec(),
    })
}

fn read_zip_array<R: Read + Seek>(archive: &mut ZipArchive<R>, name: &str) -> Result<Npy, String> {
    let mut file = archive
        .by_name(name)
        .map_err(|_| format!("NPZ is missing {name}"))?;
    const LIMIT: u64 = 75 * 1024 * 1024;
    if file.size() > LIMIT {
        return Err(format!("Uncompressed NPZ array too large: {name}"));
    }
    let mut bytes = Vec::with_capacity(file.size() as usize);
    file.by_ref()
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Failed decoding NPZ {name}: {e}"))?;
    if bytes.len() as u64 > LIMIT {
        return Err("NPZ array size limit exceeded".into());
    }
    parse_npy(bytes)
}

fn unicode_strings(array: &Npy) -> Result<Vec<String>, String> {
    let kind = array.dtype.get(..2).ok_or("Invalid numpy unicode dtype")?;
    if kind != "<U" && kind != "|S" {
        return Err("Expected NumPy Unicode/ascii string array".into());
    }
    let chars = array.dtype[2..]
        .parse::<usize>()
        .map_err(|_| "Invalid NumPy string width")?;
    if chars == 0 || chars > 500 {
        return Err("NPZ string width outside supported limits".into());
    }
    let count = array
        .shape
        .iter()
        .try_fold(1_usize, |a, b| a.checked_mul(*b))
        .ok_or("NPZ array element count overflow")?;
    if count > 10_000 {
        return Err("NPZ too many string elements".into());
    }
    let unit = if kind == "<U" {
        chars.checked_mul(4).ok_or("NPZ unicode overflow")?
    } else {
        chars
    };
    if array.data.len() != count.checked_mul(unit).ok_or("NPZ string array overflow")? {
        return Err("NPZ string data length mismatch".into());
    }
    let mut values = Vec::with_capacity(count);
    for chunk in array.data.chunks_exact(unit) {
        let text = if kind == "<U" {
            let mut letters = String::new();
            for chars in chunk.as_chunks::<4>().0.iter() {
                let code = u32::from_le_bytes(*chars);
                if code == 0 {
                    break;
                }
                let c = char::from_u32(code).ok_or("Invalid Unicode codepoint")?;
                letters.push(c);
            }
            letters
        } else {
            std::str::from_utf8(chunk)
                .map_err(|_| "Invalid ASCII strings")?
                .trim_end_matches('\0')
                .to_owned()
        };
        values.push(text);
    }
    Ok(values)
}

fn numeric_vectors(array: &Npy) -> Result<Vec<Vec<f32>>, String> {
    if array.shape.len() != 2 {
        return Err("Expected 2D NPZ vectors".into());
    }
    let rows = array.shape[0];
    let cols = array.shape[1];
    if rows > 2500 || cols == 0 || cols > 4096 {
        return Err("Unsupported embedding matrix shape".into());
    }
    let (element, kind) = match array.dtype.as_str() {
        "<f2" | "=f2" => (2_u64, 2),
        "<f4" | "=f4" => (4, 4),
        "<f8" | "=f8" => (8, 8),
        _ => return Err("ReID vectors must be little-endian float16/32/64".into()),
    };
    if (array.data.len() as u64) != rows as u64 * cols as u64 * element {
        return Err("Invalid embedding matrix byte count".into());
    }
    let mut vectors = Vec::with_capacity(rows);
    for row in array.data.chunks_exact(cols * element as usize) {
        let mut vec = Vec::with_capacity(cols);
        for cell in row.chunks_exact(element as usize) {
            let val = match kind {
                2 => f16::from_bits(u16::from_le_bytes(cell.try_into().unwrap())).to_f32(),
                4 => f32::from_le_bytes(cell.try_into().unwrap()),
                _ => f64::from_le_bytes(cell.try_into().unwrap()) as f32,
            };
            if !val.is_finite() {
                return Err("Embedding contains NaN/infinity".into());
            }
            vec.push(val);
        }
        vectors.push(vec);
    }
    Ok(vectors)
}

pub struct EmbeddingCache {
    pub features: Vec<Feature>,
    pub mode: FeatureMode,
    pub fingerprint: String,
    pub dimensions: usize,
}

/// Returns saved embeddings, never unpickles or runs image recognition.
pub fn read_embeddings(path: &Path) -> Result<EmbeddingCache, String> {
    let file = File::open(path).map_err(|e| format!("Cannot open ReID .npz: {e}"))?;
    if file.metadata().map_err(|e| e.to_string())?.len() > 100 * 1024 * 1024 {
        return Err("Compressed NPZ exceeds 100 MiB".into());
    }
    let mut zipped = ZipArchive::new(file).map_err(|e| format!("Not a ZIP NPZ: {e}"))?;
    if zipped.len() < 4 || zipped.len() > 16 {
        return Err("Unexpected NPZ archive structure".into());
    }
    let ids = unicode_strings(&read_zip_array(&mut zipped, "ids.npy")?)?;
    let vecs = numeric_vectors(&read_zip_array(&mut zipped, "vectors.npy")?)?;
    let modes = unicode_strings(&read_zip_array(&mut zipped, "mode.npy")?)?;
    let mut stamps = unicode_strings(&read_zip_array(&mut zipped, "fingerprint.npy")?)?;
    if ids.len() != vecs.len() || modes.len() != 1 || stamps.len() != 1 || stamps[0].len() != 64 {
        return Err("NPZ vector/ID count or cache fingerprint is inconsistent".into());
    }
    let mode = match modes[0].as_str() {
        "openvino_reid" => FeatureMode::OpenVino,
        "color_fallback" => FeatureMode::ColorFallback,
        _ => return Err("Unknown cached ReID embedding model".into()),
    };
    let dimensions = vecs.first().map_or(0, Vec::len);
    let mut seen = std::collections::HashSet::new();
    let mut features = Vec::with_capacity(ids.len());
    for (id, vector) in ids.into_iter().zip(vecs) {
        if id.len() > 12 || id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
            return Err("Invalid Track ID in embedding cache".into());
        }
        let id = id.parse::<i64>().map_err(|_| "Track ID out of range")?;
        if !seen.insert(id) {
            return Err("Duplicate Track IDs in embedding cache".into());
        }
        features.push(Feature { id, vector });
    }
    Ok(EmbeddingCache {
        features,
        mode,
        fingerprint: stamps.remove(0),
        dimensions,
    })
}

pub struct ReferenceCache {
    pub vectors: Vec<Vec<f32>>,
    pub origins: Vec<[String; 4]>,
    pub stamp: String,
}

/// Read precomputed embeddings from reviewed player photo albums.
/// The caller MUST verify the source album manifest + model fingerprint
/// before trusting them; this function never treats a cached label as verified.
pub fn read_reference_embeddings(path: &Path) -> Result<ReferenceCache, String> {
    let file = File::open(path).map_err(|e| format!("Cannot open reference NPZ: {e}"))?;
    if file.metadata().map_err(|e| e.to_string())?.len() > 40 * 1024 * 1024 {
        return Err("Reference NPZ exceeds 40 MiB".into());
    }
    let mut zipped = ZipArchive::new(file).map_err(|e| format!("Malformed reference NPZ: {e}"))?;
    if zipped.len() < 3 || zipped.len() > 12 {
        return Err("Unexpected reference NPZ structure".into());
    }
    let vectors = numeric_vectors(&read_zip_array(&mut zipped, "vectors.npy")?)?;
    let origin_array = read_zip_array(&mut zipped, "origins.npy")?;
    if origin_array.shape.len() != 2
        || origin_array.shape[1] != 4
        || origin_array.shape[0] != vectors.len()
    {
        return Err("Reference origins must be an N x 4 array".into());
    }
    let origin_strings = unicode_strings(&origin_array)?;
    let origins = origin_strings
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| {
            [
                chunk[0].clone(),
                chunk[1].clone(),
                chunk[2].clone(),
                chunk[3].clone(),
            ]
        })
        .collect::<Vec<_>>();
    let stamp = unicode_strings(&read_zip_array(&mut zipped, "stamp.npy")?)?;
    if stamp.len() != 1 || stamp[0].len() != 24 || !stamp[0].bytes().all(|c| c.is_ascii_hexdigit())
    {
        return Err("Invalid reference NPZ fingerprint".into());
    }
    Ok(ReferenceCache {
        vectors,
        origins,
        stamp: stamp[0].clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_npy_bytes_rejected() {
        assert!(parse_npy(vec![1, 2, 3, 4]).is_err());
    }

    #[test]
    fn rejects_object_npy_shape() {
        let mut data = b"\x93NUMPY\x01\x00".to_vec();
        let header = b"{'descr': '|O8', 'fortran_order': False, 'shape': (1,), }";
        data.extend_from_slice(&(header.len() as u16).to_le_bytes());
        data.extend_from_slice(header);
        let parsed = parse_npy(data).unwrap();
        assert!(numeric_vectors(&parsed).is_err());
    }
}
