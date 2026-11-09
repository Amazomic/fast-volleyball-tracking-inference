//! Shared content-addressed prepared media, compatible with Python schema 2.
//! Pure planning logic; Rust job orchestration owns FFmpeg execution/cancellation.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use crate::{analysis_ready_probe, ffprobe, inspect_probe, VideoInfo};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PreparedIdentity {
    pub source: String,
    pub size: u64,
    pub mtime_ns: u128,
    pub width: u32,
    pub fps: u32,
    pub schema: u8,
}

#[derive(Clone, Debug)]
pub struct PreparationPlan {
    pub source: PathBuf,
    pub output: PathBuf,
    pub manifest: PathBuf,
    pub identity: PreparedIdentity,
    pub width: u32,
    pub duration_sec: f64,
    pub already_analysis_ready: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreparationReuse {
    Original,
    Cached,
}

// Default Python JSON ensure_ascii=True is part of schema 2 hashing.
// serde_json leaves Unicode unescaped, so convert it to ASCII here.
fn python_ascii_string(input: &str) -> String {
    let encoded = serde_json::to_string(input).expect("String is always valid JSON");
    let mut output = String::new();
    for ch in encoded.chars() {
        let code = ch as u32;
        if code > 0xffff {
            let scalar = code - 0x10000;
            let high = 0xd800 + (scalar >> 10);
            let low = 0xdc00 + (scalar & 0x3ff);
            output.push_str(&format!("\\u{high:04x}\\u{low:04x}"));
        } else if code >= 128 {
            output.push_str(&format!("\\u{code:04x}"));
        } else {
            output.push(ch);
        }
    }
    output
}

pub fn python_fingerprint_json(item: &PreparedIdentity) -> String {
    // Sorted Python dict keys: fps, mtime_ns, schema, size, source, width.
    format!(
        r#"{{"fps": {}, "mtime_ns": {}, "schema": {}, "size": {}, "source": {}, "width": {}}}"#,
        item.fps,
        item.mtime_ns,
        item.schema,
        item.size,
        python_ascii_string(&item.source),
        item.width
    )
}

pub fn cache_id(item: &PreparedIdentity) -> String {
    let hash = Sha256::digest(python_fingerprint_json(item).as_bytes());
    format!("{hash:x}")[..28].to_owned()
}

/// Streaming SHA-256: a one-megabyte buffer bounds additional RAM even for
/// multi-hour matches. Refuse links and nonregular files before reading.
pub fn prepared_file_sha256(path: &Path) -> io::Result<(u64, String)> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Unsafe prepared video",
        ));
    }
    let mut file = fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut total = 0u64;
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        total = total.checked_add(read as u64).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "Prepared MP4 length overflow")
        })?;
    }
    if total != metadata.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Prepared MP4 changed while hashing",
        ));
    }
    Ok((total, format!("{:x}", digest.finalize())))
}
impl PreparationPlan {
    pub fn new(source: &Path, shared_root: &Path, width: Option<u32>) -> Result<Self, String> {
        let source = source
            .canonicalize()
            .map_err(|error| format!("Source video inaccessible: {error}"))?;
        if !source.is_file() {
            return Err("Source video is not a file".into());
        }
        let probed: Value = ffprobe(&source)?;
        let info: VideoInfo = inspect_probe(&probed)?;
        let selected_width = width.unwrap_or(info.auto_width);
        if selected_width == 0 || selected_width % 2 != 0 || selected_width > 1920 {
            return Err("Неправильная ширина нормализации".into());
        }
        let metadata =
            fs::metadata(&source).map_err(|error| format!("Cannot stat video source: {error}"))?;
        let mtime_ns = metadata
            .modified()
            .map_err(|error| format!("Cannot read source modification: {error}"))?
            .duration_since(UNIX_EPOCH)
            .map_err(|error| format!("Source modification time predates Unix epoch: {error}"))?
            .as_nanos();

        let identity = PreparedIdentity {
            source: source.to_string_lossy().into_owned(),
            size: metadata.len(),
            mtime_ns,
            width: selected_width,
            fps: 30,
            schema: 2,
        };
        let id = cache_id(&identity);
        let root = shared_root.join("prepared");
        let output = root.join(format!("{id}.mp4"));
        let manifest = root.join(format!("{id}.json"));
        let already_analysis_ready = analysis_ready_probe(&probed, &source, selected_width);
        Ok(Self {
            source,
            output,
            manifest,
            identity,
            width: selected_width,
            duration_sec: info.duration_sec,
            already_analysis_ready,
        })
    }

    pub fn reused(&self) -> Option<PreparationReuse> {
        if self.already_analysis_ready {
            // An original file cannot be reused after its recorded source
            // stamp changed, even when a caller holds an earlier plan.
            let source_meta = fs::metadata(&self.source).ok()?;
            let stamp = source_meta
                .modified()
                .ok()?
                .duration_since(UNIX_EPOCH)
                .ok()?
                .as_nanos();
            return (source_meta.is_file()
                && source_meta.len() == self.identity.size
                && stamp == self.identity.mtime_ns)
                .then_some(PreparationReuse::Original);
        }
        let manifest_info = fs::symlink_metadata(&self.manifest).ok()?;
        if manifest_info.file_type().is_symlink()
            || !manifest_info.is_file()
            || manifest_info.len() > 64 * 1024
        {
            return None;
        }
        let raw = fs::read(&self.manifest).ok()?;
        let value = serde_json::from_slice::<Value>(&raw).ok()?;
        if value["status"].as_str() != Some("ready")
            || value.get("identity") != serde_json::to_value(&self.identity).ok().as_ref()
        {
            return None;
        }
        // Old ready manifests (Python schema 2 or early Rust without a
        // content receipt) are NOT trusted; rebuild rather than silently
        // accepting a same-size corrupted or replaced video.
        let declared_size = value["cache_size_bytes"].as_u64()?;
        let declared_hash = value["cache_sha256"].as_str()?;
        if declared_hash.len() != 64
            || !declared_hash
                .bytes()
                .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase())
            || declared_size == 0
        {
            return None;
        }
        let (size, digest) = prepared_file_sha256(&self.output).ok()?;
        (size == declared_size && digest == declared_hash).then_some(PreparationReuse::Cached)
    }
    pub fn ready_path(&self) -> Option<&Path> {
        match self.reused()? {
            PreparationReuse::Original => Some(&self.source),
            PreparationReuse::Cached => Some(&self.output),
        }
    }

    /// Matches original Python FFmpeg video preparation settings.
    pub fn ffmpeg_arguments(&self, partial: &Path) -> Vec<String> {
        ["-hide_banner", "-loglevel", "error", "-nostdin", "-y", "-i"]
            .iter()
            .map(|v| (*v).to_owned())
            .chain(std::iter::once(self.source.to_string_lossy().into_owned()))
            .chain([
                "-vf".to_owned(),
                format!("fps=30,scale={}:-2,setsar=1", self.width),
                "-map".into(),
                "0:v:0".into(),
                "-map".into(),
                "0:a:0?".into(),
                "-c:a".into(),
                "aac".into(),
                "-b:a".into(),
                "128k".into(),
                "-c:v".into(),
                "libx264".into(),
                "-preset".into(),
                "veryfast".into(),
                "-crf".into(),
                "20".into(),
                "-threads".into(),
                "4".into(),
                "-movflags".into(),
                "+faststart".into(),
                "-progress".into(),
                "pipe:1".into(),
                "-nostats".into(),
                partial.to_string_lossy().into_owned(),
            ])
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{cache_id, python_fingerprint_json, PreparedIdentity};

    #[test]
    fn schema2_fingerprint_has_python_key_order_and_ascii_escapes() {
        let identity = PreparedIdentity {
            source: "/media/мяч⚽.mp4".into(),
            size: 123,
            mtime_ns: 1730000000000000000,
            width: 1280,
            fps: 30,
            schema: 2,
        };
        assert_eq!(
            python_fingerprint_json(&identity),
            r#"{"fps": 30, "mtime_ns": 1730000000000000000, "schema": 2, "size": 123, "source": "/media/\u043c\u044f\u0447\u26bd.mp4", "width": 1280}"#
        );
        assert_eq!(cache_id(&identity).len(), 28);
    }

    #[test]
    fn sha_cache_detects_same_size_corruption_missing_receipt_and_symlink() {
        use super::{prepared_file_sha256, PreparationPlan, PreparationReuse};
        let root = std::env::temp_dir().join(format!("volley-cache-83-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let input = root.join("original.mp4");
        let cached = root.join("prepared.mp4");
        let receipt = root.join("prepared.json");
        std::fs::write(&input, b"original unchanged bytes").unwrap();
        let meta = std::fs::metadata(&input).unwrap();
        let stamp = meta
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let identity = PreparedIdentity {
            source: input.to_string_lossy().to_string(),
            size: meta.len(),
            mtime_ns: stamp,
            width: 640,
            fps: 30,
            schema: 2,
        };
        let plan = PreparationPlan {
            source: input.clone(),
            output: cached.clone(),
            manifest: receipt.clone(),
            identity: identity.clone(),
            width: 640,
            duration_sec: 1.0,
            already_analysis_ready: false,
        };
        std::fs::write(&cached, b"this-is-prepared-video").unwrap();
        let (size, digest) = prepared_file_sha256(&cached).unwrap();
        let entry = serde_json::json!({"status":"ready","identity":identity,
            "cache_size_bytes":size,"cache_sha256":digest,
            "cache_verification":"sha256-stream-v1"});
        std::fs::write(&receipt, serde_json::to_vec(&entry).unwrap()).unwrap();
        assert_eq!(plan.reused(), Some(PreparationReuse::Cached));
        assert_eq!(plan.ready_path(), Some(cached.as_path()));
        // The same-length bitflip used to pass all existing cache reuse checks.
        let mut corrupted = std::fs::read(&cached).unwrap();
        corrupted[0] ^= 1;
        std::fs::write(&cached, corrupted).unwrap();
        assert_eq!(
            plan.reused(),
            None,
            "Same-size tampering must invalidate cache"
        );
        std::fs::write(&cached, b"this-is-prepared-video").unwrap();
        assert_eq!(plan.reused(), Some(PreparationReuse::Cached));
        let mut old = entry.clone();
        old.as_object_mut().unwrap().remove("cache_sha256");
        std::fs::write(&receipt, serde_json::to_vec(&old).unwrap()).unwrap();
        assert_eq!(
            plan.reused(),
            None,
            "Unsigned old cache entries require rebuilding"
        );
        let mut wrong = entry.clone();
        wrong["cache_sha256"] = serde_json::json!("x".repeat(64));
        std::fs::write(&receipt, serde_json::to_vec(&wrong).unwrap()).unwrap();
        assert_eq!(plan.reused(), None);
        std::fs::write(&receipt, serde_json::to_vec(&entry).unwrap()).unwrap();
        #[cfg(unix)]
        {
            let out = root.join("external.mp4");
            std::fs::write(&out, b"external protected").unwrap();
            std::fs::remove_file(&cached).unwrap();
            std::os::unix::fs::symlink(&out, &cached).unwrap();
            assert!(prepared_file_sha256(&cached).is_err());
            assert_eq!(
                plan.reused(),
                None,
                "Symlinked cache output cannot be reused"
            );
            std::fs::remove_file(&cached).unwrap();
            std::fs::write(&cached, b"this-is-prepared-video").unwrap();
            std::fs::remove_file(&receipt).unwrap();
            std::os::unix::fs::symlink(&out, &receipt).unwrap();
            assert_eq!(
                plan.reused(),
                None,
                "Symlinked manifest cannot authorize cache"
            );
            assert_eq!(std::fs::read(&out).unwrap(), b"external protected");
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unchanged_original_can_be_reused_but_modified_source_cannot() {
        use super::{PreparationPlan, PreparationReuse};
        let root = std::env::temp_dir().join(format!("volley-source-83-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("match.mp4");
        std::fs::write(&source, b"real analysis ready example").unwrap();
        let info = std::fs::metadata(&source).unwrap();
        let stamp = info
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let plan = PreparationPlan {
            source: source.clone(),
            output: root.join("cached.mp4"),
            manifest: root.join("cached.json"),
            identity: PreparedIdentity {
                source: source.to_string_lossy().to_string(),
                size: info.len(),
                mtime_ns: stamp,
                width: 640,
                fps: 30,
                schema: 2,
            },
            width: 640,
            duration_sec: 1.0,
            already_analysis_ready: true,
        };
        assert_eq!(plan.reused(), Some(PreparationReuse::Original));
        std::fs::write(&source, b"modified different size").unwrap();
        assert_eq!(plan.reused(), None);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn stable_id_for_same_source_fingerprint() {
        let fp = PreparedIdentity {
            source: "/tmp/a.mp4".into(),
            size: 3,
            mtime_ns: 100,
            width: 720,
            fps: 30,
            schema: 2,
        };
        assert_eq!(cache_id(&fp), cache_id(&fp));
        let mut other = fp.clone();
        other.width = 1280;
        assert_ne!(cache_id(&fp), cache_id(&other));
    }
}
