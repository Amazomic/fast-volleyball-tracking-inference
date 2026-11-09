//! One strict gold-mark mutation policy for native and historical CPU rallies.
//! Never rewrites source IDs, original records, run IDs or file schemas.
use std::collections::BTreeSet;

pub const MAX_GOLD_MARKS: usize = 5_000;

/// An existing original stable ID is the authority; do not regenerate it.
pub fn valid_gold_id(id: &str) -> bool {
    id.len() == 24
        && id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// A validated source-scoped set is used for both compatibility formats.
/// The caller remains responsible for CAS, origin validation, source membership
/// and writing only its own versioned store in the Rust sandbox.
pub fn edit_gold_set(
    before: &BTreeSet<String>,
    source_id: &str,
    marked: bool,
) -> Result<(BTreeSet<String>, bool), &'static str> {
    if !valid_gold_id(source_id)
        || before.len() > MAX_GOLD_MARKS
        || before
            .iter()
            .any(|id| id.len() != 24 || !id.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err("Gold mark set contains unsupported source IDs");
    }
    let mut after = before.clone();
    let changed = if marked {
        if !after.contains(source_id) && after.len() == MAX_GOLD_MARKS {
            return Err("Gold rally mark count exceeded");
        }
        after.insert(source_id.into())
    } else {
        after.remove(source_id)
    };
    Ok((after, changed))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_source_identity_is_preserved_and_idempotent() {
        let source = "abcdef012345abcdef012345";
        let a = BTreeSet::new();
        let (one, changed) = edit_gold_set(&a, source, true).unwrap();
        assert!(changed);
        assert!(one.contains(source));
        assert_eq!(
            edit_gold_set(&one, source, true).unwrap(),
            (one.clone(), false)
        );
        assert_eq!(
            edit_gold_set(&one, source, false).unwrap(),
            (a.clone(), true)
        );
        assert_eq!(edit_gold_set(&a, source, false).unwrap(), (a, false));
    }
    #[test]
    fn rejected_bad_source_or_over_limit_does_not_mutate_inputs() {
        let src = "0".repeat(24);
        let a = BTreeSet::from([src.clone()]);
        assert!(edit_gold_set(&a, "INVALID", true).is_err());
        assert_eq!(a.len(), 1);
        let full = (0..MAX_GOLD_MARKS)
            .map(|i| format!("{i:024x}"))
            .collect::<BTreeSet<_>>();
        assert!(edit_gold_set(&full, &"f".repeat(24), true).is_err());
        assert!(!edit_gold_set(&full, &src, true).unwrap().1);
    }
    #[test]
    fn separate_source_ids_never_collapse_into_another_version() {
        let previous = "1".repeat(24);
        let latest = "2".repeat(24);
        let (old, _) = edit_gold_set(&BTreeSet::new(), &previous, true).unwrap();
        let (next, _) = edit_gold_set(&old, &latest, true).unwrap();
        assert_eq!(next.len(), 2);
        assert!(next.contains(&previous) && next.contains(&latest));
    }
}
