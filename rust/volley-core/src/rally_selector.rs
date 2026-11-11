//! One source-typed whole-rally selector used by both saved CPU formats.
//! Only already-verified immutable original IDs/intervals may enter it.
use crate::event_evidence::Interval;
use std::collections::{BTreeSet, HashMap};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceFormat {
    NativeCpu,
    LegacyCpu,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GoldFilter {
    All,
    Marked,
    Unmarked,
}
impl GoldFilter {
    pub fn parse(input: &str) -> Result<Self, &'static str> {
        match input {
            "all" => Ok(Self::All),
            "marked" => Ok(Self::Marked),
            "unmarked" => Ok(Self::Unmarked),
            _ => Err("Unsupported gold filter"),
        }
    }
    pub fn permits(self, marked: bool) -> bool {
        match self {
            Self::All => true,
            Self::Marked => marked,
            Self::Unmarked => !marked,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct SourceRally<'a> {
    pub source: SourceFormat,
    pub stable_id: &'a str,
    pub source_scope: &'a str,
    pub interval: Interval,
    pub marked: bool,
}
pub enum Choice<'a> {
    Manual(&'a [String]),
    Page {
        offset: usize,
        limit: usize,
        gold: GoldFilter,
    },
    All {
        gold: GoldFilter,
    },
}
/// Return indices in source order except Manual, which preserves explicit
/// user order. No inferred IDs, no automatic rewrite of source formats.
pub fn select<'a>(
    rows: &[SourceRally<'a>],
    choice: Choice<'_>,
) -> Result<Vec<usize>, &'static str> {
    if rows.len() > 200_000 {
        return Err("Too many saved rally intervals");
    }
    let mut keys = HashMap::<(&str, &str), usize>::new();
    let source = rows.first().map(|r| r.source);
    for (i, row) in rows.iter().enumerate() {
        if row.stable_id.len() != 24
            || !row
                .stable_id
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || row.source_scope.is_empty()
            || row.source_scope.len() > 512
            || row.interval.start > row.interval.end
            || Some(row.source) != source
            || keys.insert((row.source_scope, row.stable_id), i).is_some()
        {
            return Err("Invalid, duplicated or incompatible source rally ID");
        }
    }
    match choice {
        Choice::Manual(ids) => {
            if ids.is_empty() || ids.len() > 80 {
                return Err("Manual selection requires 1..80 rally IDs");
            }
            let unique = ids.iter().collect::<BTreeSet<_>>();
            if unique.len() != ids.len() {
                return Err("Duplicate manual rally selection");
            }
            let mut selected = Vec::with_capacity(ids.len());
            for id in ids {
                let matches = rows
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| r.stable_id == id)
                    .map(|(i, _)| i)
                    .collect::<Vec<_>>();
                if matches.len() != 1 {
                    return Err("Saved rally ID missing or ambiguous across video sources");
                }
                selected.push(matches[0]);
            }
            Ok(selected)
        }
        Choice::Page {
            offset,
            limit,
            gold,
        } => {
            if limit == 0 || limit > 80 || offset > 200_000 {
                return Err("Invalid rally preview page request");
            }
            Ok(rows
                .iter()
                .enumerate()
                .filter(|(_, r)| gold.permits(r.marked))
                .skip(offset)
                .take(limit)
                .map(|(i, _)| i)
                .collect())
        }
        Choice::All { gold } => Ok(rows
            .iter()
            .enumerate()
            .filter(|(_, r)| gold.permits(r.marked))
            .map(|(i, _)| i)
            .collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rows<'a>() -> Vec<SourceRally<'a>> {
        vec![
            SourceRally {
                source: SourceFormat::NativeCpu,
                stable_id: "aabbccddeeff001122334455",
                source_scope: "video.mp4",
                interval: Interval { start: 10, end: 50 },
                marked: true,
            },
            SourceRally {
                source: SourceFormat::NativeCpu,
                stable_id: "bbccddeeff00112233445566",
                source_scope: "video.mp4",
                interval: Interval { start: 60, end: 90 },
                marked: false,
            },
            SourceRally {
                source: SourceFormat::NativeCpu,
                stable_id: "ccddeeff0011223344556677",
                source_scope: "video.mp4",
                interval: Interval {
                    start: 100,
                    end: 130,
                },
                marked: true,
            },
        ]
    }
    #[test]
    fn same_contract_supports_original_native_and_legacy_ids() {
        let native = rows();
        assert_eq!(
            select(
                &native,
                Choice::Page {
                    offset: 1,
                    limit: 1,
                    gold: GoldFilter::Marked
                }
            )
            .unwrap(),
            vec![2]
        );
        assert_eq!(
            select(
                &native,
                Choice::All {
                    gold: GoldFilter::Unmarked
                }
            )
            .unwrap(),
            vec![1]
        );
        let ids = vec![native[2].stable_id.into(), native[0].stable_id.into()];
        assert_eq!(select(&native, Choice::Manual(&ids)).unwrap(), vec![2, 0]);
        let legacy = native
            .iter()
            .map(|r| SourceRally {
                source: SourceFormat::LegacyCpu,
                ..*r
            })
            .collect::<Vec<_>>();
        assert_eq!(select(&legacy, Choice::Manual(&ids)).unwrap(), vec![2, 0]);
    }
    #[test]
    fn unknown_duplicate_cross_video_and_cross_format_fail_closed() {
        let native = rows();
        assert!(select(
            &native,
            Choice::Manual(&[native[0].stable_id.into(), native[0].stable_id.into()])
        )
        .is_err());
        assert!(select(&native, Choice::Manual(&["f".repeat(24)])).is_err());
        let mut mixed = rows();
        mixed[2].source = SourceFormat::LegacyCpu;
        assert!(select(
            &mixed,
            Choice::All {
                gold: GoldFilter::All
            }
        )
        .is_err());
        let mut ambiguous = rows();
        ambiguous[1].stable_id = ambiguous[0].stable_id;
        ambiguous[1].source_scope = "another.mp4";
        assert!(select(&ambiguous, Choice::Manual(&[ambiguous[0].stable_id.into()])).is_err());
        assert!(GoldFilter::parse("autocorrect").is_err());
    }
}
