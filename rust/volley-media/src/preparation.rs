//! Shared content-addressed prepared media, compatible with Python schema 2.
//! Pure planning logic; Rust job orchestration owns FFmpeg execution/cancellation.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
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
            return Some(PreparationReuse::Original);
        }
        let raw = fs::read(&self.manifest).ok()?;
        let value = serde_json::from_slice::<Value>(&raw).ok()?;
        if value["status"].as_str() != Some("ready")
            || value.get("identity") != serde_json::to_value(&self.identity).ok().as_ref()
        {
            return None;
        }
        if self
            .output
            .metadata()
            .is_ok_and(|metadata| metadata.len() > 0 && metadata.is_file())
        {
            Some(PreparationReuse::Cached)
        } else {
            None
        }
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
