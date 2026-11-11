//! Model-independent CPU fusion of independently persisted 9-frame GPU
//! windows. Keep every RAVEL player proposal and both ball alternatives;
//! a selected ball is a *visualization candidate*, not a verified trajectory.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub frame_index: u64,
    pub class_name: String,
    pub score: f64,
    pub bbox_xyxy_norm: [f64; 4],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_index: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub touch_probability: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_point_norm: Option<[f64; 2]>,
}

impl Candidate {
    pub fn validate(&self, first: u64, model: &str) -> Result<(), String> {
        if self.frame_index < first || self.frame_index >= first + 9 {
            return Err("GPU detection frame is outside its nine-frame window".into());
        }
        if !self.score.is_finite() || !(0.0..=1.0).contains(&self.score) {
            return Err("GPU detection score is invalid".into());
        }
        let [x1, y1, x2, y2] = self.bbox_xyxy_norm;
        if ![x1, y1, x2, y2]
            .iter()
            .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
            || x1 >= x2
            || y1 >= y2
        {
            return Err("GPU detection rectangle is invalid".into());
        }
        if self
            .touch_probability
            .is_some_and(|p| !p.is_finite() || !(0.0..=1.0).contains(&p))
            || self
                .head_point_norm
                .is_some_and(|p| p.iter().any(|v| !v.is_finite() || !(0.0..=1.0).contains(v)))
        {
            return Err("GPU detection touch/head score is invalid".into());
        }
        match (model, self.class_name.as_str()) {
            ("players", "player") if self.query_index.is_some_and(|q| q < 1024) => Ok(()),
            ("players", "ball")
                if self.query_index.is_none() && self.touch_probability.is_none() =>
            {
                Ok(())
            }
            ("ball", "ball") if self.query_index.is_none() && self.touch_probability.is_none() => {
                Ok(())
            }
            _ => Err("GPU detection source/class/query combination is invalid".into()),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct BallCandidate {
    pub source_model: &'static str,
    pub detection: Candidate,
}
#[derive(Clone, Debug, Serialize)]
pub struct FusedFrame {
    pub frame_index: u64,
    pub player_candidates: Vec<Candidate>,
    pub ball_candidates: Vec<BallCandidate>,
    pub selected_ball: Option<BallCandidate>,
    pub player_track_ids_confirmed: bool,
    pub touches_confirmed: bool,
}

/// Exactly one disjoint pair of same-index 9-frame windows. Overlap is
/// deliberately rejected by the caller for now; silent dedup could lose
/// differently-scored raw neural evidence needed for CPU reprocessing.
pub fn fuse_window(
    first: u64,
    ravel: Vec<Candidate>,
    vball: Vec<Candidate>,
) -> Result<Vec<FusedFrame>, String> {
    if first > u64::MAX - 9 {
        return Err("GPU window start overflows frame range".into());
    }
    let mut frames = (0..9)
        .map(|i| FusedFrame {
            frame_index: first + i,
            player_candidates: Vec::new(),
            ball_candidates: Vec::new(),
            selected_ball: None,
            player_track_ids_confirmed: false,
            touches_confirmed: false,
        })
        .collect::<Vec<_>>();
    if ravel.len() > 9 * 33 || vball.len() > 9 {
        return Err("GPU window candidate count exceeds neural decoder bounds".into());
    }
    for candidate in ravel {
        candidate.validate(first, "players")?;
        let slot = (candidate.frame_index - first) as usize;
        if candidate.class_name == "player" {
            frames[slot].player_candidates.push(candidate);
        } else {
            frames[slot].ball_candidates.push(BallCandidate {
                source_model: "ravel",
                detection: candidate,
            });
        }
    }
    for candidate in vball {
        candidate.validate(first, "ball")?;
        let slot = (candidate.frame_index - first) as usize;
        frames[slot].ball_candidates.push(BallCandidate {
            source_model: "vballnet",
            detection: candidate,
        });
    }
    for frame in &mut frames {
        frame.player_candidates.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.query_index.cmp(&b.query_index))
        });
        frame.ball_candidates.sort_by(|a, b| {
            (a.source_model != "vballnet")
                .cmp(&(b.source_model != "vballnet"))
                .then_with(|| b.detection.score.total_cmp(&a.detection.score))
        });
        // VballNet preferred whenever it has a candidate. RAVEL is only a
        // fallback; neither candidate becomes a confirmed physical track.
        frame.selected_ball = frame.ball_candidates.first().cloned();
    }
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ball(frame: u64, score: f64) -> Candidate {
        Candidate {
            frame_index: frame,
            class_name: "ball".into(),
            score,
            bbox_xyxy_norm: [0.4, 0.4, 0.5, 0.5],
            query_index: None,
            touch_probability: None,
            head_point_norm: None,
        }
    }
    fn player(frame: u64, query: usize, touch: f64) -> Candidate {
        Candidate {
            frame_index: frame,
            class_name: "player".into(),
            score: 0.8,
            bbox_xyxy_norm: [0.1, 0.2, 0.3, 0.7],
            query_index: Some(query),
            touch_probability: Some(touch),
            head_point_norm: Some([0.2, 0.3]),
        }
    }
    #[test]
    fn preserves_player_candidates_touch_and_both_ball_sources() {
        let frames = fuse_window(
            9,
            vec![player(9, 3, 0.94), ball(9, 0.99), player(9, 2, 0.50)],
            vec![ball(9, 0.31)],
        )
        .unwrap();
        assert_eq!(frames.len(), 9);
        let first = &frames[0];
        assert_eq!(first.player_candidates.len(), 2);
        assert_eq!(first.player_candidates[0].touch_probability, Some(0.5));
        assert_eq!(first.player_candidates[1].touch_probability, Some(0.94));
        assert_eq!(first.ball_candidates.len(), 2);
        assert_eq!(
            first.selected_ball.as_ref().unwrap().source_model,
            "vballnet"
        );
        assert_eq!(first.ball_candidates[1].source_model, "ravel");
        assert!(!first.player_track_ids_confirmed);
        assert!(!first.touches_confirmed);
        assert!(frames[1].selected_ball.is_none());
    }
    #[test]
    fn falls_back_only_if_preferred_ball_missing() {
        let frames = fuse_window(0, vec![ball(2, 0.65)], vec![]).unwrap();
        assert_eq!(
            frames[2].selected_ball.as_ref().unwrap().source_model,
            "ravel"
        );
    }
    #[test]
    fn rejects_cross_window_invalid_boxes_and_fabricated_ids() {
        assert!(fuse_window(u64::MAX, vec![], vec![]).is_err());
        assert!(fuse_window(0, vec![player(9, 0, 0.5)], vec![]).is_err());
        let mut invalid = player(0, 3, 0.4);
        invalid.bbox_xyxy_norm[0] = 2.0;
        assert!(fuse_window(0, vec![invalid], vec![]).is_err());
        let mut invalid = ball(0, 0.3);
        invalid.query_index = Some(7);
        assert!(fuse_window(0, vec![], vec![invalid]).is_err());
    }
}
