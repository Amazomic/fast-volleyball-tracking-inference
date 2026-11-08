//! Shared safe versioned sidecar I/O and compare-and-swap helpers.
//! This adds no new persisted schema. Source validation and locking are
//! the responsibility of each legacy or native domain adapter.
use crate::storage;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::Read,
    path::Path,
};

pub(crate) fn revision(raw: Option<&[u8]>) -> String {
    raw.map(|bytes| format!("{:x}", Sha256::digest(bytes)))
        .unwrap_or_else(|| "absent".to_owned())
}

pub(crate) fn valid_revision(value: &str) -> bool {
    value == "absent"
        || (value.len() == 64
            && value
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()))
}

pub(crate) fn expect_revision(current: Option<&[u8]>, expected: &str) -> Result<(), &'static str> {
    if !valid_revision(expected) {
        return Err("Invalid revision format");
    }
    if revision(current) != expected {
        return Err("Stale sidecar revision; refresh and retry");
    }
    Ok(())
}

/// Never read a symlink, nonregular file or more bytes than the cap.
pub(crate) fn read_optional_bounded(path: &Path, limit: u64) -> Result<Option<Vec<u8>>, String> {
    if limit == 0 {
        return Err("Sidecar byte limit must be positive".into());
    }
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_file() || meta.len() > limit => {
            return Err("Unsafe or oversized sidecar file".into())
        }
        Ok(_) => (),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("Check sidecar file: {e}")),
    }
    let file = File::open(path).map_err(|e| format!("Open sidecar file: {e}"))?;
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Read bounded sidecar: {e}"))?;
    if bytes.len() as u64 > limit {
        return Err("Sidecar exceeds byte limit".into());
    }
    Ok(Some(bytes))
}

/// Write a *direct child* of a checked directory. The caller must hold
/// the project/feature lock and validate the immutable source manifest.
pub(crate) fn write_json_checked(
    parent: &Path,
    target: &Path,
    value: &Value,
    max_bytes: u64,
) -> Result<(), String> {
    if max_bytes == 0 || target.parent() != Some(parent) {
        return Err("Sidecar destination is not a direct child of guarded folder".into());
    }
    let encoded = serde_json::to_vec_pretty(value).map_err(|e| format!("Encode sidecar: {e}"))?;
    if encoded.len() as u64 > max_bytes {
        return Err("Sidecar serialization exceeds byte limit".into());
    }
    match fs::symlink_metadata(parent) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
            return Err("Unsafe sidecar parent directory".into())
        }
        Ok(_) => (),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(parent).map_err(|e| format!("Create sidecar parent: {e}"))?
        }
        Err(e) => return Err(format!("Check sidecar parent: {e}")),
    }
    match fs::symlink_metadata(target) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_file() => {
            return Err("Unsafe sidecar output target".into())
        }
        Ok(_) => (),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(format!("Check sidecar output: {e}")),
    }
    storage::write_json_atomic(target, value)
        .map_err(|e| format!("Atomic sidecar write: {e:?}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;
    fn fixture() -> std::path::PathBuf {
        let folder = std::env::temp_dir().join(format!("volley-sidecar-{}", Uuid::new_v4()));
        fs::create_dir_all(&folder).unwrap();
        folder
    }
    #[test]
    fn revision_cas_distinguishes_missing_stale_and_valid_snapshots() {
        assert_eq!(revision(None), "absent");
        assert!(expect_revision(None, "absent").is_ok());
        assert!(expect_revision(None, &"a".repeat(64)).is_err());
        assert!(!valid_revision("abc"));
        assert!(!valid_revision(&"A".repeat(64)));
        let raw = b"{\"schema\":1}";
        let sha = revision(Some(raw));
        assert_eq!(sha.len(), 64);
        assert!(expect_revision(Some(raw), "absent").is_err());
        assert!(expect_revision(Some(raw), &sha).is_ok());
    }
    #[test]
    fn bounded_io_roundtrip_preserves_legacy_json_and_exact_revision() {
        let root = fixture();
        let folder = root.join("records");
        let path = folder.join("rally_marks.json");
        assert!(read_optional_bounded(&path, 3000).unwrap().is_none());
        let doc = json!({"schema":"volley-rally-marks/1","marks":[{"run_id":"run_a"}]});
        write_json_checked(&folder, &path, &doc, 3000).unwrap();
        let bytes = read_optional_bounded(&path, 3000).unwrap().unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap(), doc);
        assert_eq!(
            revision(Some(&bytes)),
            format!("{:x}", Sha256::digest(&bytes))
        );
        assert!(write_json_checked(&folder, &path, &doc, 3).is_err());
        assert!(read_optional_bounded(&path, 3).is_err());
        assert!(write_json_checked(&root, &path, &doc, 3000).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn reject_symlink_sidecars_and_parents() {
        use std::os::unix::fs::symlink;
        let root = fixture();
        let source = root.join("source.json");
        fs::write(&source, "{}").unwrap();
        let folder = root.join("safe");
        fs::create_dir(&folder).unwrap();
        let path = folder.join("malicious.json");
        symlink(&source, &path).unwrap();
        assert!(read_optional_bounded(&path, 3000).is_err());
        assert!(write_json_checked(&folder, &path, &json!({}), 3000).is_err());
        let alias = root.join("alias");
        symlink(&folder, &alias).unwrap();
        assert!(write_json_checked(&alias, &alias.join("other.json"), &json!({}), 3000).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn sidecar_write_never_mutates_original_tracking_evidence() {
        let root = fixture();
        let source = root.join("tracked_frames.jsonl");
        fs::write(&source, b"{\"frame_index\":0}\n").unwrap();
        let original = fs::read(&source).unwrap();
        let folder = root.join("native_track_assignments");
        write_json_checked(
            &folder,
            &folder.join("review.json"),
            &json!({"assignments":{}}),
            3000,
        )
        .unwrap();
        assert_eq!(fs::read(&source).unwrap(), original);
        fs::remove_dir_all(root).unwrap();
    }
}
