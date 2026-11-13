//! CPU-only, bounded reconstruction of existing skeleton-confirmed Track IDs.
//! The immutable GPU/CPU replay is unchanged. Both ends must be independent
//! observed poses for the SAME existing ID; never create a new identity.
use super::{TouchFrame, TouchPlayer};
use std::collections::BTreeMap;

const MAX_MISSING_FRAMES: usize = 89;

fn overlap(a: [f64; 4], b: [f64; 4]) -> f64 {
    let area = |b: [f64; 4]| (b[2] - b[0]).max(0.0) * (b[3] - b[1]).max(0.0);
    let w = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let h = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let intersection = w * h;
    intersection / (area(a) + area(b) - intersection).max(1e-9)
}

/// Reconstruct missing *positions* only for an already observed skeleton ID.
/// Values have deliberately lower confidence; never change source evidence.
/// No one-sided extrapolation, invented identities, or gap > 3 seconds.
pub fn reconstruct(frames: &[TouchFrame]) -> Vec<TouchFrame> {
    let mut output = frames.to_vec();
    let mut observed = BTreeMap::<u64, Vec<(usize, &TouchPlayer)>>::new();
    for (index, frame) in frames.iter().enumerate() {
        for player in &frame.players {
            observed
                .entry(player.track_id)
                .or_default()
                .push((index, player));
        }
    }
    for (track_id, observations) in observed {
        for pair in observations.windows(2) {
            let [(left_frame, a), (right_frame, b)] = [pair[0], pair[1]];
            let missing = right_frame.saturating_sub(left_frame + 1);
            if missing == 0 || missing > MAX_MISSING_FRAMES {
                continue;
            }
            // Keep only stable tracks, not two boxes that happened to share an ID.
            let aw = a.bbox_xyxy_norm[2] - a.bbox_xyxy_norm[0];
            let bw = b.bbox_xyxy_norm[2] - b.bbox_xyxy_norm[0];
            let ah = a.bbox_xyxy_norm[3] - a.bbox_xyxy_norm[1];
            let bh = b.bbox_xyxy_norm[3] - b.bbox_xyxy_norm[1];
            if aw.min(bw) <= 0.005
                || ah.min(bh) <= 0.02
                || (aw.max(bw) / aw.min(bw)) > 1.8
                || (ah.max(bh) / ah.min(bh)) > 1.8
                || a.score.min(b.score) < 0.35
            {
                continue;
            }
            let center = |v: &[f64; 4]| [(v[0] + v[2]) * 0.5, (v[1] + v[3]) * 0.5];
            let ac = center(&a.bbox_xyxy_norm);
            let bc = center(&b.bbox_xyxy_norm);
            let distance = (ac[0] - bc[0]).hypot(ac[1] - bc[1]);
            if distance > (0.035 + (missing as f64) * 0.003).min(0.15) {
                continue;
            }
            for at in left_frame + 1..right_frame {
                let ratio = (at - left_frame) as f64 / (right_frame - left_frame) as f64;
                let bbox = std::array::from_fn(|i| {
                    a.bbox_xyxy_norm[i] * (1.0 - ratio) + b.bbox_xyxy_norm[i] * ratio
                });
                // Prefer actual observed joints/boxes if two players overlap.
                if output[at]
                    .players
                    .iter()
                    .any(|real| overlap(real.bbox_xyxy_norm, bbox) > 0.65)
                {
                    continue;
                }
                output[at].players.push(TouchPlayer {
                    track_id,
                    score: a.score.min(b.score) * 0.90,
                    bbox_xyxy_norm: bbox,
                    touch_probability: None,
                });
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player(id: u64, x: f64) -> TouchPlayer {
        TouchPlayer {
            track_id: id,
            score: 0.80,
            touch_probability: None,
            bbox_xyxy_norm: [x, 0.40, x + 0.05, 0.60],
        }
    }

    #[test]
    fn restores_only_bounded_same_id_motion_without_changing_source() {
        let mut frames = (0..96)
            .map(|i| TouchFrame {
                frame_index: i,
                players: Vec::new(),
                selected_ball: None,
            })
            .collect::<Vec<_>>();
        frames[5].players.push(player(4, 0.40));
        frames[45].players.push(player(4, 0.42));
        frames[90].players.push(player(5, 0.50));
        let result = reconstruct(&frames);
        assert!(frames[20].players.is_empty());
        assert_eq!(result[20].players.len(), 1);
        assert_eq!(result[20].players[0].track_id, 4);
        assert!(result[20].players[0].score < 0.8);
        assert!(result[70].players.is_empty());
    }

    #[test]
    fn does_not_fabricate_long_gaps_or_teleporting_or_weak_tracks() {
        let mut frames = (0..100)
            .map(|i| TouchFrame {
                frame_index: i,
                players: Vec::new(),
                selected_ball: None,
            })
            .collect::<Vec<_>>();
        frames[0].players.push(player(1, 0.20));
        frames[95].players.push(player(1, 0.22));
        frames[79].players.push(player(2, 0.20));
        frames[89].players.push(player(2, 0.70));
        frames[91].players.push(player(3, 0.20));
        let mut weak = player(3, 0.21);
        weak.score = 0.20;
        frames[99].players.push(weak);
        let out = reconstruct(&frames);
        assert!(out[15].players.is_empty());
        assert!(out[84].players.is_empty());
        assert!(out[96].players.is_empty());
    }
}
