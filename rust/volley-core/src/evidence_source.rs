//! Typed provenance contract across legacy CPU caches and the standalone
//! native GPU -> CPU pipeline. This is a domain identity, NOT persistence
//! migration and NOT a permission to merge numeric Track IDs across runs.
//! Legacy frame rates and SHA hashes must never be fabricated.

use serde::{Deserialize, Serialize};

/// Byte count and modification timestamp of an independently validated file.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceStamp {
    pub bytes: u64,
    /// Nanoseconds since Unix epoch; constrained to u64 for JSON roundtrips.
    pub mtime_ns: u64,
}

impl SourceStamp {
    pub fn check(self) -> Result<Self, String> {
        if self.bytes == 0 || self.mtime_ns == 0 {
            return Err("Missing or zero source fingerprint".into());
        }
        Ok(self)
    }
}

/// An unknown legacy rate is explicitly unknown; it is NOT assumed to be 30.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FrameRate {
    Unknown,
    Exact { numerator: u32, denominator: u32 },
}
impl FrameRate {
    pub fn check(self) -> Result<Self, String> {
        match self {
            Self::Unknown => Ok(self),
            Self::Exact {
                numerator,
                denominator,
            } if numerator > 0
                && denominator > 0
                && numerator as u64 <= 240 * denominator as u64 =>
            {
                Ok(self)
            }
            _ => Err("Invalid or unsupported evidence frame rate".into()),
        }
    }
}

/// Version-scoped identity of a whole *source of detections*, not an athlete.
/// The native SHA is the digest of the fully verified tracked_frames.jsonl.
/// Legacy only has the independently verified players.json metadata stamp;
/// do not present that as a SHA-256 content verification.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(tag = "source_kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EvidenceSource {
    LegacyCpuCache {
        project_id: String,
        video: String,
        run_id: String,
        base_cache_id: String,
        players_json_stamp: SourceStamp,
        frame_rate: FrameRate,
    },
    NativeGpuCpuReplay {
        project_id: String,
        video: String,
        replay_id: String,
        tracked_frames_sha256: String,
        prepared_video_stamp: SourceStamp,
        frame_rate: FrameRate,
    },
}

fn valid_project(id: &str) -> bool {
    id.len() == 12
        && id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn valid_video(video: &str) -> bool {
    video.len() <= 255
        && !video.is_empty()
        && video != "."
        && video != ".."
        && !video.chars().any(|c| matches!(c, '/' | '\\' | '\0'))
        && video.rsplit('.').next().is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "mp4" | "mov" | "mkv" | "avi" | "webm" | "m4v" | "mts"
            )
        })
}

fn valid_legacy_run(id: &str) -> bool {
    (10..=104).contains(&id.len())
        && id.starts_with("run_")
        && id.as_bytes()[4..]
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-')
}

fn valid_cache(id: &str) -> bool {
    // Legacy cached ID is a base digest (16-64 lowercase hex), optionally
    // followed by a separate CPU version digest.
    let hex = |part: &str| {
        (16..=64).contains(&part.len())
            && part
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    };
    id.split_once('.').map_or_else(
        || hex(id),
        |(base, cpu_version)| hex(base) && hex(cpu_version),
    )
}

fn valid_native_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn valid_sha256(sha: &str) -> bool {
    sha.len() == 64
        && sha
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

impl EvidenceSource {
    pub fn legacy(
        project_id: &str,
        video: &str,
        run_id: &str,
        base_cache_id: &str,
        players_json_stamp: SourceStamp,
        frame_rate: FrameRate,
    ) -> Result<Self, String> {
        Self::LegacyCpuCache {
            project_id: project_id.to_owned(),
            video: video.to_owned(),
            run_id: run_id.to_owned(),
            base_cache_id: base_cache_id.to_owned(),
            players_json_stamp,
            frame_rate,
        }
        .checked()
    }

    pub fn native(
        project_id: &str,
        video: &str,
        replay_id: &str,
        tracked_frames_sha256: &str,
        prepared_video_stamp: SourceStamp,
        frame_rate: FrameRate,
    ) -> Result<Self, String> {
        Self::NativeGpuCpuReplay {
            project_id: project_id.to_owned(),
            video: video.to_owned(),
            replay_id: replay_id.to_owned(),
            tracked_frames_sha256: tracked_frames_sha256.to_owned(),
            prepared_video_stamp,
            frame_rate,
        }
        .checked()
    }

    /// Also validates deserialized sources; serde parsing alone is not a
    /// provenance check. Call this at every untrusted domain boundary.
    pub fn checked(self) -> Result<Self, String> {
        match &self {
            Self::LegacyCpuCache {
                project_id,
                video,
                run_id,
                base_cache_id,
                players_json_stamp,
                frame_rate,
            } => {
                if !valid_project(project_id)
                    || !valid_video(video)
                    || !valid_legacy_run(run_id)
                    || !valid_cache(base_cache_id)
                {
                    return Err("Invalid legacy evidence identity".into());
                }
                players_json_stamp.check()?;
                frame_rate.check()?;
            }
            Self::NativeGpuCpuReplay {
                project_id,
                video,
                replay_id,
                tracked_frames_sha256,
                prepared_video_stamp,
                frame_rate,
            } => {
                if !valid_project(project_id)
                    || !valid_video(video)
                    || !valid_native_id(replay_id)
                    || !valid_sha256(tracked_frames_sha256)
                    || *frame_rate
                        != (FrameRate::Exact {
                            numerator: 30,
                            denominator: 1,
                        })
                {
                    return Err("Invalid native evidence identity/SHA-256/30 FPS".into());
                }
                prepared_video_stamp.check()?;
            }
        }
        Ok(self)
    }

    pub fn source_kind(&self) -> &'static str {
        match self {
            Self::LegacyCpuCache { .. } => "legacy_cpu_cache",
            Self::NativeGpuCpuReplay { .. } => "native_gpu_cpu_replay",
        }
    }

    /// A numeric tracking ID is meaningful only inside its checked source.
    /// Same ID from two caches, runs, videos, or model snapshots is NOT a join.
    pub fn track(self, track_id: u64) -> Result<ScopedTrackId, String> {
        let checked = self.checked()?;
        if matches!(checked, Self::NativeGpuCpuReplay { .. }) && track_id == 0 {
            return Err("Native provisional Track ID zero is invalid".into());
        }
        Ok(ScopedTrackId {
            source: checked,
            track_id,
        })
    }

    pub fn ensure_same_scope(&self, other: &Self) -> Result<(), String> {
        self.clone().checked()?;
        other.clone().checked()?;
        if self != other {
            return Err("Track IDs originate in different evidence sources".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopedTrackId {
    pub source: EvidenceSource,
    pub track_id: u64,
}

impl ScopedTrackId {
    pub fn checked(self) -> Result<Self, String> {
        self.source.clone().track(self.track_id)?;
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn stamp() -> SourceStamp {
        SourceStamp {
            bytes: 984_321,
            mtime_ns: 1_782_000_000_000_000_000,
        }
    }
    fn legacy() -> EvidenceSource {
        EvidenceSource::legacy(
            "abcdef123456",
            "Матч 1.mp4",
            "run_20261108_141500",
            "a".repeat(32).as_str(),
            stamp(),
            FrameRate::Unknown,
        )
        .unwrap()
    }
    fn native() -> EvidenceSource {
        EvidenceSource::native(
            "abcdef123456",
            "Матч 1.mp4",
            "0123456789abcdef0123456789abcdef",
            &"b".repeat(64),
            stamp(),
            FrameRate::Exact {
                numerator: 30,
                denominator: 1,
            },
        )
        .unwrap()
    }
    #[test]
    fn source_roundtrip_is_explicit_and_never_invents_legacy_fps_or_sha() {
        let first = legacy();
        let encoded = serde_json::to_value(&first).unwrap();
        assert_eq!(encoded["source_kind"], "legacy_cpu_cache");
        assert_eq!(encoded["frame_rate"]["kind"], "unknown");
        assert!(encoded.get("tracked_frames_sha256").is_none());
        let decoded = serde_json::from_value::<EvidenceSource>(encoded).unwrap();
        assert_eq!(decoded.checked().unwrap(), first);
        let second = native();
        let encoded = serde_json::to_value(&second).unwrap();
        assert_eq!(encoded["source_kind"], "native_gpu_cpu_replay");
        assert_eq!(encoded["frame_rate"]["numerator"], 30);
        assert_eq!(
            serde_json::from_value::<EvidenceSource>(encoded)
                .unwrap()
                .checked()
                .unwrap(),
            second
        );
    }
    #[test]
    fn reject_cross_source_and_cross_run_numeric_id_collisions() {
        let old = legacy();
        let new = native();
        assert!(old.ensure_same_scope(&new).is_err());
        let old_id = old.clone().track(7).unwrap();
        let new_id = new.clone().track(7).unwrap();
        assert_ne!(old_id, new_id);
        assert_ne!(old_id.source, new_id.source);
        assert!(new.track(0).is_err());
        let other_legacy = EvidenceSource::legacy(
            "abcdef123456",
            "Матч 1.mp4",
            "run_20261108_141501",
            "a".repeat(32).as_str(),
            stamp(),
            FrameRate::Unknown,
        )
        .unwrap();
        assert_ne!(old.track(7).unwrap(), other_legacy.track(7).unwrap());
    }
    #[test]
    fn reject_native_wrong_sha_fps_and_changed_stamp() {
        for digest in ["A".repeat(64), "b".repeat(63), "../escape".to_string()] {
            assert!(EvidenceSource::native(
                "abcdef123456",
                "Матч 1.mp4",
                "0123456789abcdef0123456789abcdef",
                &digest,
                stamp(),
                FrameRate::Exact {
                    numerator: 30,
                    denominator: 1
                }
            )
            .is_err());
        }
        for rate in [
            FrameRate::Unknown,
            FrameRate::Exact {
                numerator: 30000,
                denominator: 1001,
            },
            FrameRate::Exact {
                numerator: 60,
                denominator: 1,
            },
        ] {
            assert!(EvidenceSource::native(
                "abcdef123456",
                "Матч 1.mp4",
                "0123456789abcdef0123456789abcdef",
                &"b".repeat(64),
                stamp(),
                rate
            )
            .is_err());
        }
        assert!(EvidenceSource::native(
            "abcdef123456",
            "Матч 1.mp4",
            "0123456789abcdef0123456789abcdef",
            &"b".repeat(64),
            SourceStamp {
                bytes: 0,
                mtime_ns: 1,
            },
            FrameRate::Exact {
                numerator: 30,
                denominator: 1,
            }
        )
        .is_err());
        let first = native();
        let mut altered = first.clone();
        if let EvidenceSource::NativeGpuCpuReplay {
            prepared_video_stamp,
            ..
        } = &mut altered
        {
            prepared_video_stamp.bytes += 1;
        }
        assert!(first.ensure_same_scope(&altered).is_err());
    }
    #[test]
    fn unknown_legacy_fps_and_untrusted_deserialization_are_not_certified() {
        assert!(EvidenceSource::legacy(
            "abcdef123456",
            "../bad.mp4",
            "run_20261108_141500",
            &"a".repeat(32),
            stamp(),
            FrameRate::Unknown,
        )
        .is_err());
        let invalid = r#"{"source_kind":"native_gpu_cpu_replay","project_id":"abcdef123456","video":"match.mp4","replay_id":"0123456789abcdef0123456789abcdef","tracked_frames_sha256":"a","prepared_video_stamp":{"bytes":12,"mtime_ns":3},"frame_rate":{"kind":"exact","numerator":30,"denominator":1}}"#;
        let untrusted: EvidenceSource = serde_json::from_str(invalid).unwrap();
        assert!(untrusted.checked().is_err());
        let legacy_zero = legacy().track(0).unwrap();
        assert_eq!(legacy_zero.track_id, 0);
        assert_eq!(
            ScopedTrackId::checked(legacy_zero.clone()).unwrap(),
            legacy_zero
        );
    }
}
