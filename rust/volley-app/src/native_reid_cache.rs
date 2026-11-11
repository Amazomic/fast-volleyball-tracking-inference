//! Single trusted read-through cache for verified native CPU Track ID vectors.
//! The cached data is not a player identity and never merges model spaces.
//! The canonical encoder remains volley-inference::reid_openvino.
use crate::versioned_store;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
};
use volley_inference::reid_openvino::{DIMENSIONS, MODEL_NAME, PREPROCESS};

const MAX_BYTES: u64 = 12 * 1024 * 1024;
const MAX_FEATURES: usize = 1024;
const SCHEMA: &str = "volley-native-reid-embeddings/1";

#[derive(Clone, Debug)]
pub(crate) struct CacheScope {
    pub project_id: String,
    pub video: String,
    pub replay_id: String,
    pub replay_sha256: String,
    pub model_fingerprint: String,
}
fn is_sha(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
fn is_hex_id(s: &str, count: usize) -> bool {
    s.len() == count
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
fn scope_valid(scope: &CacheScope) -> Result<(), String> {
    if !is_hex_id(&scope.project_id, 12)
        || !is_hex_id(&scope.replay_id, 32)
        || !is_sha(&scope.replay_sha256)
        || !is_sha(&scope.model_fingerprint)
        || scope.video.is_empty()
        || scope.video.len() > 255
        || scope.video == "."
        || scope.video == ".."
        || scope.video.contains('/')
        || scope.video.contains('\\')
    {
        return Err("Invalid ReID cache source/model scope".into());
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CacheDocument {
    schema: String,
    project_id: String,
    video: String,
    replay_id: String,
    source_replay_sha256: String,
    model: String,
    model_fingerprint: String,
    preprocessing: String,
    dimensions: usize,
    /// Unconfirmed vectors; a Track ID here is unique only within replay_id.
    features: BTreeMap<u64, Vec<f32>>,
}

#[derive(Default)]
pub(crate) struct CacheLookup {
    pub vectors: BTreeMap<u64, Vec<f32>>,
    pub invalidated: bool,
}
fn normal_vector(vector: &[f32]) -> bool {
    if vector.len() != DIMENSIONS || vector.iter().any(|v| !v.is_finite()) {
        return false;
    }
    let norm = vector.iter().map(|v| (*v as f64).powi(2)).sum::<f64>();
    norm.is_finite() && (norm - 1.0).abs() < 0.005
}

fn checked_path(root: &Path, scope: &CacheScope, create: bool) -> Result<PathBuf, String> {
    scope_valid(scope)?;
    let base = root.join("projects");
    let project = base.join(&scope.project_id);
    let folder = project.join("native_reid_cache");
    for path in [&base, &project, &folder] {
        match fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                return Err("Unsafe native ReID cache directory".into());
            }
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && create => {
                fs::create_dir(path).map_err(|e| format!("Create ReID cache folder: {e}"))?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => return Err(format!("Check ReID cache directory: {e}")),
        }
    }
    Ok(folder.join(format!("{}.json", scope.replay_id)))
}
fn read_document(path: &Path, scope: &CacheScope) -> Result<CacheLookup, String> {
    let Some(raw) = versioned_store::read_optional_bounded(path, MAX_BYTES)? else {
        return Ok(CacheLookup::default());
    };
    let doc: CacheDocument = serde_json::from_slice(&raw)
        .map_err(|e| format!("Invalid saved native ReID cache: {e}"))?;
    if doc.schema != SCHEMA
        || doc.project_id != scope.project_id
        || doc.video != scope.video
        || doc.replay_id != scope.replay_id
        || doc.model != MODEL_NAME
        || doc.preprocessing != PREPROCESS
        || doc.dimensions != DIMENSIONS
    {
        return Err("ReID cache schema or source owner is not supported".into());
    }
    // Invalidating a *whole* source/model snapshot is safer than trying to
    // translate vectors between different feature spaces.
    if doc.source_replay_sha256 != scope.replay_sha256
        || doc.model_fingerprint != scope.model_fingerprint
    {
        return Ok(CacheLookup {
            vectors: BTreeMap::new(),
            invalidated: true,
        });
    }
    if doc.features.len() > MAX_FEATURES
        || doc
            .features
            .iter()
            .any(|(id, v)| *id == 0 || !normal_vector(v))
    {
        return Err("Corrupt or unnormalized ReID vector in saved cache".into());
    }
    Ok(CacheLookup {
        vectors: doc.features,
        invalidated: false,
    })
}
pub(crate) fn read(root: &Path, scope: &CacheScope) -> Result<CacheLookup, String> {
    let file = checked_path(root, scope, false)?;
    read_document(&file, scope)
}
/// Merge only server-computed, normalized vectors. A project lock and on-disk
/// exclusive cache lock preserve concurrent requests. A stale model/source
/// resets the old document; no old vector is carried into the new feature space.
pub(crate) fn merge(
    root: &Path,
    scope: &CacheScope,
    generated: &[(u64, Vec<f32>)],
) -> Result<usize, String> {
    scope_valid(scope)?;
    if generated.is_empty()
        || generated.len() > 8
        || generated
            .iter()
            .any(|(id, v)| *id == 0 || !normal_vector(v))
    {
        return Err("Invalid native ReID batch for caching".into());
    }
    let file = checked_path(root, scope, true)?;
    let folder = file.parent().ok_or("ReID cache directory absent")?;
    let lockfile = folder.join(".embeddings.lock");
    if fs::symlink_metadata(&lockfile)
        .ok()
        .is_some_and(|m| m.file_type().is_symlink())
    {
        return Err("Unsafe native ReID cache lock".into());
    }
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lockfile)
        .map_err(|e| format!("Open ReID cache lock: {e}"))?;
    lock.lock_exclusive()
        .map_err(|e| format!("Lock ReID cache: {e}"))?;
    let previous = read_document(&file, scope)?;
    let mut combined = previous.vectors;
    for (id, vector) in generated {
        if !combined.contains_key(id) && combined.len() >= MAX_FEATURES {
            return Err("Native ReID cache track limit exceeded".into());
        }
        combined.insert(*id, vector.clone());
    }
    let n = combined.len();
    let value = serde_json::to_value(CacheDocument {
        schema: SCHEMA.into(),
        project_id: scope.project_id.clone(),
        video: scope.video.clone(),
        replay_id: scope.replay_id.clone(),
        source_replay_sha256: scope.replay_sha256.clone(),
        model: MODEL_NAME.into(),
        model_fingerprint: scope.model_fingerprint.clone(),
        preprocessing: PREPROCESS.into(),
        dimensions: DIMENSIONS,
        features: combined,
    })
    .map_err(|e| format!("Encode ReID feature cache: {e}"))?;
    versioned_store::write_json_checked(folder, &file, &value, MAX_BYTES)?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;
    fn folder() -> PathBuf {
        let path = std::env::temp_dir().join(format!("native-reid-cache-{}", Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        path
    }
    fn source() -> CacheScope {
        CacheScope {
            project_id: "123456abcdef".into(),
            video: "match.mp4".into(),
            replay_id: "0123456789abcdef0123456789abcdef".into(),
            replay_sha256: "a".repeat(64),
            model_fingerprint: "b".repeat(64),
        }
    }
    fn vector(channel: usize) -> Vec<f32> {
        let mut result = vec![0.0; DIMENSIONS];
        result[channel] = 1.0;
        result
    }
    #[test]
    fn checked_cache_roundtrip_invalidates_whole_model_or_replay_space() {
        let root = folder();
        let src = source();
        assert!(read(&root, &src).unwrap().vectors.is_empty());
        assert_eq!(merge(&root, &src, &[(7, vector(0))]).unwrap(), 1);
        let saved = read(&root, &src).unwrap();
        assert_eq!(saved.vectors.get(&7), Some(&vector(0)));
        assert!(!saved.invalidated);
        let mut new_model = src.clone();
        new_model.model_fingerprint = "c".repeat(64);
        let stale = read(&root, &new_model).unwrap();
        assert!(stale.vectors.is_empty());
        assert!(stale.invalidated);
        assert_eq!(merge(&root, &new_model, &[(7, vector(1))]).unwrap(), 1);
        assert_eq!(
            read(&root, &new_model).unwrap().vectors.get(&7),
            Some(&vector(1))
        );
        assert!(read(&root, &src).unwrap().invalidated);
        let mut new_replay = src.clone();
        new_replay.replay_sha256 = "d".repeat(64);
        assert!(read(&root, &new_replay).unwrap().invalidated);
        let mut new_run = src.clone();
        new_run.replay_id = "f".repeat(32);
        assert!(read(&root, &new_run).unwrap().vectors.is_empty());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn reid_cache_merges_but_rejects_forged_or_malformed_vectors() {
        let root = folder();
        let src = source();
        assert!(merge(&root, &src, &[(7, vec![0.0; DIMENSIONS])]).is_err());
        assert!(merge(&root, &src, &[(0, vector(0))]).is_err());
        assert_eq!(merge(&root, &src, &[(7, vector(0))]).unwrap(), 1);
        assert_eq!(merge(&root, &src, &[(8, vector(1))]).unwrap(), 2);
        assert_eq!(read(&root, &src).unwrap().vectors.len(), 2);
        let path = checked_path(&root, &src, false).unwrap();
        let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        raw["features"]["7"] = serde_json::json!([0.1, 0.1]);
        fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
        assert!(read(&root, &src).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn symlink_cache_folder_is_rejected() {
        use std::os::unix::fs::symlink;
        let root = folder();
        let src = source();
        let project = root.join("projects").join(&src.project_id);
        fs::create_dir_all(&project).unwrap();
        symlink(&root, project.join("native_reid_cache")).unwrap();
        assert!(read(&root, &src).is_err());
        assert!(merge(&root, &src, &[(7, vector(0))]).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
