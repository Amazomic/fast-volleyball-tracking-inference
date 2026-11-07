//! Deterministic CPU player statistics from saved volleyball action events.
//!
//! Identifies physical contacts by (confirmed athlete or raw track, source
//! time), or by contact_id when no track exists. Keeps the most informative
//! action among duplicates. Geometry/ReID candidates never become athletes.

use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};

pub const ACTION_TYPES: [&str; 7] = [
    "attack",
    "block",
    "serve",
    "dig",
    "overhead_pass",
    "low_touch",
    "unknown_touch",
];
pub const RECOGNIZED: [&str; 5] = ["attack", "block", "serve", "dig", "overhead_pass"];

#[derive(Clone, Debug)]
pub struct Action {
    pub id: String,
    pub contact_id: Option<String>,
    pub kind: String,
    /// Source time rounded to milliseconds (matches browser timeline).
    pub time_ms: i64,
    pub track_id: Option<String>,
    pub confidence: f64,
}

#[derive(Clone, Debug)]
pub struct Player {
    pub id: String,
    pub name: String,
    /// An explicit user-reviewed ID->player link. Never include speculative
    /// automatic ReID or long-gap geometry candidates here.
    pub track_ids: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct PlayerTotals {
    pub id: String,
    pub name: String,
    pub track_ids: Vec<String>,
    pub touches: u64,
    pub recognized_touches: u64,
    pub actions: BTreeMap<String, u64>,
    /// Touch activity relative to roster median. Null if median is zero.
    pub activity: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TrackTotals {
    pub player_track_id: String,
    pub player_id: Option<String>,
    pub touches: u64,
    pub recognized_touches: u64,
    pub actions: BTreeMap<String, u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Stats {
    pub total_touches: u64,
    pub unassigned_touches: u64,
    pub recognized_touches: u64,
    pub counts: BTreeMap<String, u64>,
    pub players: Vec<PlayerTotals>,
    pub raw: Vec<TrackTotals>,
    pub ignored_non_actions: u64,
    pub ignored_excluded: u64,
    pub duplicates_collapsed: u64,
    pub median_player_touches: f64,
    pub notes: &'static str,
}

#[derive(Clone, Debug)]
struct Contact {
    kind: String,
    confidence: f64,
    owner: Option<String>,
    track_id: Option<String>,
}

fn recognized(kind: &str) -> bool {
    RECOGNIZED.contains(&kind)
}

fn better(a: &Contact, b: &Contact) -> bool {
    (recognized(&a.kind), a.confidence).gt(&(recognized(&b.kind), b.confidence))
}

fn numeric_id(track: &str) -> bool {
    !track.is_empty() && track.len() <= 12 && track.bytes().all(|b| b.is_ascii_digit())
}
fn valid_player(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

/// All explicit owners must be unique and not manually excluded.
/// Unassigned contacts remain counted; uncertain player identities are not
/// silently attributed to a roster member.
pub fn summarize(
    actions: &[Action],
    roster: &[Player],
    excluded: &HashSet<String>,
) -> Result<Stats, &'static str> {
    if actions.len() > 1_000_000 || roster.len() > 200 || excluded.len() > 100_000 {
        return Err("Too many action events, roster players or excluded tracks");
    }
    let mut owners = HashMap::new();
    let mut names = HashSet::new();
    let mut by_id = BTreeMap::new();
    for player in roster {
        if !valid_player(&player.id) || !names.insert(player.id.clone()) || player.name.len() > 200
        {
            return Err("Invalid or duplicate player in project roster");
        }
        let mut ids = Vec::new();
        for raw in &player.track_ids {
            if !numeric_id(raw) {
                return Err("Invalid player Track ID");
            }
            if excluded.contains(raw) {
                continue;
            }
            if owners.insert(raw.clone(), player.id.clone()).is_some() {
                return Err("Confirmed Track ID belongs to two players");
            }
            ids.push(raw.clone());
        }
        ids.sort_by_key(|id| id.parse::<u64>().unwrap_or(u64::MAX));
        ids.dedup();
        by_id.insert(
            player.id.clone(),
            PlayerTotals {
                id: player.id.clone(),
                name: player.name.clone(),
                track_ids: ids,
                ..PlayerTotals::default()
            },
        );
    }

    let mut dedup: HashMap<String, Contact> = HashMap::new();
    let mut ignored_non_actions = 0_u64;
    let mut ignored_excluded = 0_u64;
    let mut duplicates = 0_u64;
    for (index, action) in actions.iter().enumerate() {
        if !ACTION_TYPES.contains(&action.kind.as_str()) {
            ignored_non_actions += 1;
            continue;
        }
        if !action.confidence.is_finite()
            || !(0.0..=1.0).contains(&action.confidence)
            || action.time_ms < 0
            || action.time_ms > 1000 * 60 * 60 * 48
        {
            continue;
        }
        let track = action.track_id.as_deref();
        if track.is_some_and(|id| excluded.contains(id)) {
            ignored_excluded += 1;
            continue;
        }
        let owner = track.and_then(|id| owners.get(id)).cloned();
        let identity = owner
            .as_deref()
            .map(|id| format!("p:{id}"))
            .or_else(|| track.map(|id| format!("t:{id}")));
        let key = if let (Some(identity), Some(track)) = (identity, track) {
            // A particular Track ID exists, so unknown owners must not
            // collapse independent players merely by sharing a frame.
            let _ = track;
            format!("time:{identity}:{}", action.time_ms)
        } else if let Some(contact) = &action.contact_id {
            format!("contact:{contact}")
        } else {
            let identity = if action.id.is_empty() {
                index.to_string()
            } else {
                action.id.clone()
            };
            format!("event:{identity}")
        };
        let next = Contact {
            kind: action.kind.clone(),
            confidence: action.confidence,
            owner,
            track_id: action.track_id.clone(),
        };
        if let Some(previous) = dedup.get_mut(&key) {
            duplicates += 1;
            if better(&next, previous) {
                *previous = next;
            }
        } else {
            dedup.insert(key, next);
        }
    }

    let mut counts = BTreeMap::new();
    let mut raw = BTreeMap::<String, TrackTotals>::new();
    let mut unassigned = 0_u64;
    let mut recognized_total = 0_u64;
    for (_, contact) in dedup {
        *counts.entry(contact.kind.clone()).or_insert(0) += 1;
        let is_recognized = recognized(&contact.kind);
        if is_recognized {
            recognized_total += 1;
        }
        if let Some(track) = &contact.track_id {
            let entry = raw.entry(track.clone()).or_insert_with(|| TrackTotals {
                player_track_id: track.clone(),
                player_id: contact.owner.clone(),
                touches: 0,
                recognized_touches: 0,
                actions: BTreeMap::new(),
            });
            entry.touches += 1;
            entry.recognized_touches += u64::from(is_recognized);
            *entry.actions.entry(contact.kind.clone()).or_insert(0) += 1;
        }
        if let Some(owner) = &contact.owner {
            if let Some(person) = by_id.get_mut(owner) {
                person.touches += 1;
                person.recognized_touches += u64::from(is_recognized);
                *person.actions.entry(contact.kind).or_insert(0) += 1;
            }
        } else {
            unassigned += 1;
        }
    }
    let mut players = by_id.into_values().collect::<Vec<_>>();
    let mut touches = players.iter().map(|r| r.touches).collect::<Vec<_>>();
    touches.sort_unstable();
    let len = touches.len();
    let median = if len == 0 {
        0.0
    } else if len % 2 == 0 {
        (touches[len / 2 - 1] as f64 + touches[len / 2] as f64) / 2.0
    } else {
        touches[len / 2] as f64
    };
    for p in &mut players {
        p.activity = (median > 0.0).then(|| p.touches as f64 / median);
    }
    players.sort_by(|a, b| b.touches.cmp(&a.touches).then_with(|| a.name.cmp(&b.name)));
    let mut tracks = raw.into_values().collect::<Vec<_>>();
    tracks.sort_by(|a, b| {
        b.touches
            .cmp(&a.touches)
            .then_with(|| a.player_track_id.cmp(&b.player_track_id))
    });
    let total = counts.values().sum();
    Ok(Stats{
        total_touches:total,unassigned_touches:unassigned,
        recognized_touches:recognized_total,counts,players,raw:tracks,
        ignored_non_actions,ignored_excluded,duplicates_collapsed:duplicates,
        median_player_touches:median,
        notes:"Counts are inferred from stored CPU contacts; an unassigned Track ID is not a real player identity.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(id: &str, kind: &str, time_ms: i64, confidence: f64) -> Action {
        Action {
            id: format!("evt-{kind}"),
            contact_id: None,
            kind: kind.into(),
            time_ms,
            track_id: Some(id.into()),
            confidence,
        }
    }
    fn roster() -> Vec<Player> {
        vec![Player {
            id: "athlete-a".into(),
            name: "A".into(),
            track_ids: vec!["1".into(), "2".into()],
        }]
    }
    #[test]
    fn one_contact_per_confirmed_player_per_instant() {
        let result = summarize(
            &[
                sample("1", "unknown_touch", 1000, 0.99),
                sample("2", "attack", 1000, 0.4),
                sample("2", "attack", 2000, 0.9),
            ],
            &roster(),
            &HashSet::new(),
        )
        .unwrap();
        assert_eq!(result.total_touches, 2);
        assert_eq!(result.players[0].actions["attack"], 2);
        assert_eq!(result.duplicates_collapsed, 1);
    }
    #[test]
    fn separate_unconfirmed_tracks_are_not_misidentified() {
        let result = summarize(
            &[
                sample("3", "attack", 1000, 0.9),
                sample("4", "block", 1000, 0.9),
            ],
            &roster(),
            &HashSet::new(),
        )
        .unwrap();
        assert_eq!(result.total_touches, 2);
        assert_eq!(result.unassigned_touches, 2);
    }
    #[test]
    fn manual_exclusion_is_a_hard_gate() {
        let result = summarize(
            &[
                sample("1", "serve", 1000, 0.9),
                sample("2", "attack", 2000, 0.9),
            ],
            &roster(),
            &HashSet::from(["1".into()]),
        )
        .unwrap();
        assert_eq!(result.total_touches, 1);
        assert_eq!(result.ignored_excluded, 1);
    }
    #[test]
    fn same_id_in_two_confirmed_players_is_rejected() {
        let mut roster = roster();
        roster.push(Player {
            id: "athlete-b".into(),
            name: "B".into(),
            track_ids: vec!["2".into()],
        });
        assert!(summarize(&[], &roster, &HashSet::new()).is_err());
    }
}
