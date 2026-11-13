//! Streaming, conservative volleyball rally *proposals* from selected ball
//! observations and motion-gated Touch candidates. NOT a final action or
//! score classifier: a model prediction is not proof of a played point.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RallySettings {
    pub min_ball_score: f64,
    /// Brief tracking holes are permitted inside one rally; not indefinitely.
    pub max_ball_gap_frames: u64,
    pub max_idle_frames: u64,
    pub stationary_px_per_frame: f64,
    pub min_rally_frames: u64,
    pub min_ball_samples: u64,
    pub min_ball_coverage: f64,
    pub min_touch_candidates: u64,
    pub max_without_touch_frames: u64,
    pub suggested_context_frames: u64,
}
impl Default for RallySettings {
    fn default() -> Self {
        Self {
            min_ball_score: 0.35,
            max_ball_gap_frames: 75,
            max_idle_frames: 60,
            stationary_px_per_frame: 2.0,
            min_rally_frames: 45,
            min_ball_samples: 12,
            min_ball_coverage: 0.25,
            min_touch_candidates: 2,
            max_without_touch_frames: 120,
            suggested_context_frames: 45,
        }
    }
}
impl RallySettings {
    pub fn validate(self) -> Result<Self, String> {
        if !self.min_ball_score.is_finite()
            || !(0.01..=1.0).contains(&self.min_ball_score)
            || !(1..=120).contains(&self.max_ball_gap_frames)
            || !(15..=180).contains(&self.max_idle_frames)
            || !self.stationary_px_per_frame.is_finite()
            || !(0.1..=15.0).contains(&self.stationary_px_per_frame)
            || !(15..=900).contains(&self.min_rally_frames)
            || !(3..=900).contains(&self.min_ball_samples)
            || !self.min_ball_coverage.is_finite()
            || !(0.05..=1.0).contains(&self.min_ball_coverage)
            || !(1..=10).contains(&self.min_touch_candidates)
            || !(90..=900).contains(&self.max_without_touch_frames)
            || !(0..=180).contains(&self.suggested_context_frames)
        {
            return Err("Invalid native rally candidate thresholds".into());
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BallFrame {
    pub x_px: f64,
    pub y_px: f64,
    pub score: f64,
}
pub fn ground_bounce_candidate(points: &[BallFrame; 9], near_player: bool, on_court: bool) -> bool {
    if near_player
        || !on_court
        || points.iter().any(|p| {
            !p.x_px.is_finite() || !p.y_px.is_finite() || !p.score.is_finite() || p.score < 0.35
        })
    {
        return false;
    }
    if points
        .windows(2)
        .any(|pair| (pair[0].x_px - pair[1].x_px).hypot(pair[0].y_px - pair[1].y_px) > 70.0)
    {
        return false;
    }
    let incoming = (points[4].y_px - points[0].y_px) / 4.0;
    let outgoing = (points[8].y_px - points[4].y_px) / 4.0;
    incoming >= 2.0
        && outgoing <= -1.0
        && points[4].y_px >= points[0].y_px
        && points[4].y_px >= points[8].y_px
}
#[derive(Clone, Copy, Debug)]
pub struct ContactFrame {
    pub probability: f64,
    /// Provisional Track ID: never interpreted as an athlete identity.
    pub provisional_track_id: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RallyProposal {
    pub detected_start_frame: u64,
    pub detected_end_frame: u64,
    pub duration_sec: f64,
    pub ball_observations: u64,
    pub ball_coverage: f64,
    pub motion_supported_touch_proposals: u64,
    pub first_touch_frame: u64,
    pub last_touch_frame: u64,
    pub max_touch_probability: f64,
    pub provisional_track_ids: Vec<u64>,
    pub unidentified_contacts: u64,
    pub classification: &'static str,
    pub method: &'static str,
    pub authoritative_rally: bool,
    pub athlete_identities_verified: bool,
    pub serve_verified: bool,
    pub suggested_context_before_frames: u64,
    pub suggested_context_after_frames: u64,
    pub end_reason: &'static str,
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct RallyTotals {
    pub ball_observations: u64,
    pub skipped_low_score: u64,
    pub ignored_touch_proposals: u64,
    pub observed_segments: u64,
    pub accepted_proposals: u64,
    pub rejected_short: u64,
    pub rejected_low_ball_coverage: u64,
    pub rejected_missing_touches: u64,
    pub ended_on_ball_gap: u64,
    pub ended_on_idle: u64,
    pub ended_on_touch_gap: u64,
}
#[derive(Clone, Debug)]
struct ActiveSegment {
    start: u64,
    last_ball: u64,
    last_center: [f64; 2],
    samples: u64,
    idle: u64,
    // First stationary sample; end a rally *before* the prolonged idle.
    idle_start: Option<u64>,
    touches: u64,
    first_touch: Option<u64>,
    last_touch: Option<u64>,
    last_touch_track_id: Option<u64>,
    max_touch: f64,
    ids: BTreeSet<u64>,
    unknown: u64,
}
pub struct RallySegmenter {
    config: RallySettings,
    active: Option<ActiveSegment>,
    last_frame: Option<u64>,
    last_rest_center: Option<[f64; 2]>,
    waiting_motion: bool,
    waiting_contact_after_ground: bool,
    width: u32,
    height: u32,
    totals: RallyTotals,
}
impl RallySegmenter {
    pub fn new(config: RallySettings, width: u32, height: u32) -> Result<Self, String> {
        config.validate()?;
        if !(64..=16384).contains(&width) || !(64..=16384).contains(&height) {
            return Err("Invalid prepared-video dimensions for rally reconstruction".into());
        }
        Ok(Self {
            config,
            active: None,
            last_frame: None,
            last_rest_center: None,
            waiting_motion: false,
            waiting_contact_after_ground: false,
            width,
            height,
            totals: RallyTotals::default(),
        })
    }
    fn measured(&mut self, frame: u64, ball: BallFrame) -> Result<Option<BallFrame>, String> {
        if ![ball.x_px, ball.y_px, ball.score]
            .iter()
            .all(|x| x.is_finite())
            || !(0.0..=1.0).contains(&ball.score)
            || ball.x_px < 0.0
            || ball.y_px < 0.0
            || ball.x_px > self.width as f64
            || ball.y_px > self.height as f64
        {
            return Err("Invalid ball box center or confidence".into());
        }
        if ball.score < self.config.min_ball_score {
            self.totals.skipped_low_score += 1;
            return Ok(None);
        }
        self.totals.ball_observations += 1;
        // Track continuity is not trusted across a massive single-frame jump.
        // At 30 fps this is more likely a wrong ball/source candidate than
        // a physical trajectory. Never manufacture a rally from that jump.
        if let Some(active) = &self.active {
            let gap = frame.saturating_sub(active.last_ball);
            if gap == 1 {
                let step =
                    (ball.x_px - active.last_center[0]).hypot(ball.y_px - active.last_center[1]);
                if step > self.width.max(self.height) as f64 * 0.38 {
                    self.totals.skipped_low_score += 1;
                    return Ok(None);
                }
            }
        }
        Ok(Some(ball))
    }
    fn push_touch(
        active: &mut ActiveSegment,
        frame: u64,
        touch: ContactFrame,
    ) -> Result<bool, String> {
        if !touch.probability.is_finite()
            || !(0.0..=1.0).contains(&touch.probability)
            || touch
                .provisional_track_id
                .is_some_and(|id| id == 0 || id > i64::MAX as u64)
        {
            return Err("Invalid motion-gated contact proposal".into());
        }
        let Some(id) = touch.provisional_track_id else {
            return Ok(false);
        };
        if active.last_touch_track_id == Some(id) {
            // Repeated physical proposals for one player count as one Touch.
            return Ok(false);
        }
        if active.ids.len() > 512 {
            return Err("Too many distinct provisional tracks in one rally".into());
        }
        active.touches += 1;
        active.first_touch.get_or_insert(frame);
        active.last_touch = Some(frame);
        active.last_touch_track_id = Some(id);
        active.max_touch = active.max_touch.max(touch.probability);
        active.ids.insert(id);
        Ok(true)
    }
    fn finish_active(&mut self, reason: &'static str) -> Option<RallyProposal> {
        let mut active = self.active.take()?;
        self.totals.observed_segments += 1;
        let end = if reason == "prolonged_idle" {
            let start = active.idle_start.unwrap_or(active.last_ball);
            active.samples = active.samples.saturating_sub(active.idle);
            start.saturating_sub(1)
        } else {
            active.last_ball
        };
        match reason {
            "ball_missing" => self.totals.ended_on_ball_gap += 1,
            "prolonged_idle" => self.totals.ended_on_idle += 1,
            "touchless_timeout" => self.totals.ended_on_touch_gap += 1,
            _ => {}
        }
        if end < active.start {
            self.totals.rejected_short += 1;
            return None;
        }
        let frames = end - active.start + 1;
        if frames < self.config.min_rally_frames || active.samples < self.config.min_ball_samples {
            self.totals.rejected_short += 1;
            return None;
        }
        let coverage = (active.samples.min(frames) as f64 / frames as f64).min(1.0);
        if coverage < self.config.min_ball_coverage {
            self.totals.rejected_low_ball_coverage += 1;
            return None;
        }
        if active.touches < self.config.min_touch_candidates {
            self.totals.rejected_missing_touches += 1;
            return None;
        }
        let Some(first_touch) = active.first_touch else {
            self.totals.rejected_missing_touches += 1;
            return None;
        };
        let last_touch = active.last_touch.unwrap_or(first_touch);
        // A long post-contact stationary tail must not claim game activity.
        if first_touch > end || last_touch > end {
            self.totals.rejected_missing_touches += 1;
            return None;
        }
        self.totals.accepted_proposals += 1;
        Some(RallyProposal {
            detected_start_frame: active.start,
            detected_end_frame: end,
            duration_sec: ((frames as f64 / 30.0) * 10000.0).round() / 10000.0,
            ball_observations: active.samples,
            ball_coverage: (coverage * 10000.0).round() / 10000.0,
            motion_supported_touch_proposals: active.touches,
            first_touch_frame: first_touch,
            last_touch_frame: last_touch,
            max_touch_probability: active.max_touch,
            provisional_track_ids: active.ids.into_iter().collect(),
            unidentified_contacts: active.unknown,
            classification: "provisional_rally",
            method: "ball_observation_continuity_and_motion_gated_touch_proposals",
            authoritative_rally: false,
            athlete_identities_verified: false,
            serve_verified: false,
            suggested_context_before_frames: self.config.suggested_context_frames,
            suggested_context_after_frames: self.config.suggested_context_frames,
            end_reason: reason,
        })
    }
    /// One frame at a time. Can emit at most one completed provisional rally.
    /// An empty ball detection represents a frame with no reliable ball.
    pub fn push(
        &mut self,
        frame: u64,
        ball: Option<BallFrame>,
        contacts: &[ContactFrame],
    ) -> Result<Option<RallyProposal>, String> {
        if self.last_frame.is_some_and(|last| frame != last + 1)
            || frame > 48 * 3600 * 30
            || contacts.len() > 32
        {
            return Err("Native rally frames must be sequential and bounded".into());
        }
        self.last_frame = Some(frame);
        // Validate contacts first so bad data never gets silently discarded
        // when there is no ball or the current segment is closed.
        for contact in contacts {
            if !contact.probability.is_finite()
                || !(0.0..=1.0).contains(&contact.probability)
                || contact
                    .provisional_track_id
                    .is_some_and(|id| id == 0 || id > i64::MAX as u64)
            {
                return Err("Invalid motion-gated contact input".into());
            }
        }
        let valid = ball
            .map(|item| self.measured(frame, item))
            .transpose()?
            .flatten();
        if self.waiting_contact_after_ground {
            if contacts.is_empty() || valid.is_none() {
                self.totals.ignored_touch_proposals += contacts.len() as u64;
                return Ok(None);
            }
            self.waiting_contact_after_ground = false;
        }
        let mut emitted = None;
        if let Some(active) = &self.active {
            if frame.saturating_sub(active.last_ball) > self.config.max_ball_gap_frames {
                emitted = self.finish_active("ball_missing");
            } else if active.last_touch.is_some_and(|last| {
                frame.saturating_sub(last) > self.config.max_without_touch_frames.min(120)
            }) {
                emitted = self.finish_active("touchless_timeout");
            }
        }
        if let Some(ball) = valid {
            let point = [ball.x_px, ball.y_px];
            if self.waiting_motion {
                if let Some(rest) = self.last_rest_center {
                    if (rest[0] - point[0]).hypot(rest[1] - point[1])
                        < self.config.stationary_px_per_frame * 4.0
                    {
                        self.last_rest_center = Some(point);
                        self.totals.ignored_touch_proposals += contacts.len() as u64;
                        return Ok(emitted);
                    }
                }
                self.waiting_motion = false;
                self.last_rest_center = None;
            }
            if self.active.is_none() {
                self.active = Some(ActiveSegment {
                    start: frame,
                    last_ball: frame,
                    last_center: point,
                    samples: 1,
                    idle: 0,
                    idle_start: None,
                    touches: 0,
                    first_touch: None,
                    last_touch: None,
                    last_touch_track_id: None,
                    max_touch: 0.0,
                    ids: BTreeSet::new(),
                    unknown: 0,
                });
            } else {
                let t = self.active.as_mut().expect("set");
                let gap = frame - t.last_ball;
                let step = (point[0] - t.last_center[0]).hypot(point[1] - t.last_center[1]);
                // Only consecutive measurements count towards stationary
                // timeout. Dropouts reset the idle run.
                if gap == 1 && step < self.config.stationary_px_per_frame {
                    t.idle += 1;
                    t.idle_start.get_or_insert(frame);
                } else {
                    t.idle = 0;
                    t.idle_start = None;
                }
                t.last_center = point;
                t.last_ball = frame;
                t.samples += 1;
                if t.idle >= self.config.max_idle_frames {
                    let rest = point;
                    let previous = emitted;
                    let closed = self.finish_active("prolonged_idle");
                    self.last_rest_center = Some(rest);
                    self.waiting_motion = true;
                    // An existing emitted group and a separate idle group
                    // on the same frame would violate the single-result API.
                    // It cannot happen: only one active segment exists.
                    emitted = closed.or(previous);
                }
            }
        }
        if let Some(segment) = self.active.as_mut() {
            for &touch in contacts {
                if frame.saturating_sub(segment.last_ball) <= self.config.max_ball_gap_frames {
                    if !Self::push_touch(segment, frame, touch)? {
                        self.totals.ignored_touch_proposals += 1;
                    }
                } else {
                    self.totals.ignored_touch_proposals += 1;
                }
            }
        } else {
            self.totals.ignored_touch_proposals += contacts.len() as u64;
        }
        Ok(emitted)
    }
    pub fn finish(&mut self) -> Option<RallyProposal> {
        self.finish_active("end_of_video")
    }
    pub fn finish_at_possible_ground_bounce(&mut self, frame: u64) -> Option<RallyProposal> {
        let active = self.active.as_mut()?;
        if frame < active.start
            || frame > active.last_ball
            || active.last_touch.is_some_and(|touch| touch >= frame)
        {
            return None;
        }
        let trailing = active.last_ball - frame;
        let samples = active.samples.saturating_sub(trailing);
        let frames = frame - active.start + 1;
        if frames < self.config.min_rally_frames
            || samples < self.config.min_ball_samples
            || active.touches < self.config.min_touch_candidates
            || (samples.min(frames) as f64 / frames as f64) < self.config.min_ball_coverage
        {
            return None;
        }
        active.samples = samples;
        active.last_ball = frame;
        let result = self.finish_active("possible_ground_bounce");
        self.waiting_contact_after_ground = result.is_some();
        result
    }
    pub fn totals(&self) -> &RallyTotals {
        &self.totals
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ground_candidate_distinguishes_bounce_from_ballistic_apex_and_player_contact() {
        let mut points = [BallFrame {
            x_px: 300.0,
            y_px: 0.0,
            score: 0.9,
        }; 9];
        for (i, point) in points.iter_mut().enumerate() {
            point.y_px = 300.0 - (i as f64 - 4.0).abs() * 8.0;
        }
        assert!(ground_bounce_candidate(&points, false, true));
        assert!(!ground_bounce_candidate(&points, true, true));
        assert!(!ground_bounce_candidate(&points, false, false));
        let mut unreliable = points;
        unreliable[2].score = 0.1;
        assert!(!ground_bounce_candidate(&unreliable, false, true));
        let mut jumped = points;
        jumped[3].x_px = 1100.0;
        assert!(!ground_bounce_candidate(&jumped, false, true));
        for (i, point) in points.iter_mut().enumerate() {
            point.y_px = 200.0 + (i as f64 - 4.0).abs() * 8.0;
        }
        assert!(!ground_bounce_candidate(&points, false, true));
    }
    #[test]
    fn ground_candidate_ends_provisional_rally_at_impact_frame() {
        let mut segment = RallySegmenter::new(RallySettings::default(), 640, 360).unwrap();
        for frame in 0..110 {
            let touches = if frame == 20 {
                hit().to_vec()
            } else if frame == 70 {
                hit_with_id(4).to_vec()
            } else {
                vec![]
            };
            assert!(segment
                .push(frame, b(100.0 + frame as f64 * 3.0), &touches)
                .unwrap()
                .is_none());
        }
        let rally = segment.finish_at_possible_ground_bounce(105).unwrap();
        assert_eq!(rally.detected_end_frame, 105);
        assert_eq!(rally.end_reason, "possible_ground_bounce");
        assert!(!rally.authoritative_rally);
        for frame in 110..140 {
            assert!(segment
                .push(frame, b(100.0 + frame as f64 * 3.0), &[])
                .unwrap()
                .is_none());
        }
        assert!(segment.finish().is_none());
    }
    #[test]
    fn early_ground_candidate_does_not_discard_incomplete_rally() {
        let mut segment = RallySegmenter::new(RallySettings::default(), 640, 360).unwrap();
        for frame in 0..110 {
            let touches = if frame == 20 {
                hit().to_vec()
            } else if frame == 70 {
                hit_with_id(4).to_vec()
            } else {
                vec![]
            };
            segment
                .push(frame, b(100.0 + frame as f64 * 3.0), &touches)
                .unwrap();
            if frame == 50 {
                assert!(segment.finish_at_possible_ground_bounce(47).is_none());
            }
        }
        assert_eq!(
            segment.finish().unwrap().motion_supported_touch_proposals,
            2
        );
    }
    fn b(x: f64) -> Option<BallFrame> {
        Some(BallFrame {
            x_px: x,
            y_px: 100.0,
            score: 0.9,
        })
    }
    fn hit_with_id(id: u64) -> [ContactFrame; 1] {
        [ContactFrame {
            probability: 0.94,
            provisional_track_id: Some(id),
        }]
    }
    fn hit() -> [ContactFrame; 1] {
        hit_with_id(3)
    }
    fn run(
        frames: u64,
        ball: impl Fn(u64) -> Option<BallFrame>,
        touches: &[u64],
        settings: RallySettings,
    ) -> (Vec<RallyProposal>, RallyTotals) {
        let mut segment = RallySegmenter::new(settings, 640, 360).unwrap();
        let mut result = Vec::new();
        let mut contact_number = 0;
        for frame in 0..frames {
            let contacts = if touches.contains(&frame) {
                contact_number += 1;
                hit_with_id(contact_number).to_vec()
            } else {
                vec![]
            };
            if let Some(item) = segment.push(frame, ball(frame), &contacts).unwrap() {
                result.push(item);
            }
        }
        if let Some(item) = segment.finish() {
            result.push(item);
        }
        (result, segment.totals().clone())
    }
    #[test]
    fn short_ball_gaps_do_not_split_active_point() {
        let (r, _) = run(
            160,
            |i| {
                if (67..=84).contains(&i) {
                    None
                } else {
                    b(100.0 + i as f64 * 3.0)
                }
            },
            &[30, 120],
            RallySettings::default(),
        );
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].detected_start_frame, 0);
        assert_eq!(r[0].detected_end_frame, 159);
        assert_eq!(r[0].motion_supported_touch_proposals, 2);
        assert!(!r[0].authoritative_rally);
    }
    #[test]
    fn long_tracking_loss_yields_two_distinct_rallies() {
        let (r, _) = run(
            480,
            |i| {
                if (130..=240).contains(&i) {
                    None
                } else {
                    b(90.0 + (i % 130) as f64 * 3.0)
                }
            },
            &[15, 78, 270, 345],
            RallySettings::default(),
        );
        assert_eq!(r.len(), 2);
        assert!(r[0].detected_end_frame < 130);
        assert_eq!(r[1].detected_start_frame, 241);
        assert!(r[0].detected_end_frame < r[1].detected_start_frame);
    }
    #[test]
    fn high_ball_detection_without_two_motion_touches_is_not_a_rally() {
        let (r, totals) = run(
            100,
            |i| b(30.0 + i as f64 * 3.0),
            &[50],
            RallySettings::default(),
        );
        assert!(r.is_empty());
        assert_eq!(totals.rejected_missing_touches, 1);
    }
    #[test]
    fn prolonged_stationary_ball_ends_rally_before_break() {
        let (r, stats) = run(
            270,
            |i| {
                if i < 130 {
                    b(50.0 + i as f64 * 3.0)
                } else {
                    b(440.0)
                }
            },
            &[20, 85],
            RallySettings::default(),
        );
        assert_eq!(r.len(), 1);
        assert!(r[0].detected_end_frame <= 132);
        assert_eq!(r[0].end_reason, "prolonged_idle");
        assert!(stats.ended_on_idle >= 1);
    }
    #[test]
    fn two_points_with_continuously_visible_ball_split_on_touchless_timeout() {
        let (r, stats) = run(
            540,
            |i| {
                let t = i % 360;
                b(20.0 + t.min(360 - t) as f64 * 3.0)
            },
            &[25, 65, 340, 385],
            RallySettings::default(),
        );
        assert_eq!(r.len(), 2);
        assert!(r[0].detected_end_frame < r[1].detected_start_frame);
        assert!(stats.ended_on_touch_gap >= 1);
    }
    #[test]
    fn four_seconds_without_a_different_player_ends_rally_even_if_ball_visible() {
        let mut config = RallySettings::default();
        config.max_without_touch_frames = 900; // old saved profile cannot extend beyond 4s
        let mut segment = RallySegmenter::new(config, 640, 360).unwrap();
        let mut results = Vec::new();
        for frame in 0..270 {
            let contact = match frame {
                20 => Some(1),
                50 | 90 | 130 => Some(2),
                _ => None,
            };
            let contacts = contact
                .map(|id| {
                    vec![ContactFrame {
                        probability: 0.9,
                        provisional_track_id: Some(id),
                    }]
                })
                .unwrap_or_default();
            let observed = BallFrame {
                x_px: 100.0 + (frame % 40) as f64 * 5.0,
                y_px: 140.0,
                score: 0.95,
            };
            if let Some(rally) = segment.push(frame, Some(observed), &contacts).unwrap() {
                results.push(rally);
            }
        }
        if let Some(rally) = segment.finish() {
            results.push(rally);
        }
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].motion_supported_touch_proposals, 2);
        assert_eq!(results[0].end_reason, "touchless_timeout");
        assert!(results[0].detected_end_frame <= 170);
    }

    #[test]
    fn repeated_player_is_suppressed_within_each_rally_but_resets_at_boundary() {
        let mut segment = RallySegmenter::new(RallySettings::default(), 640, 360).unwrap();
        let mut rallies = Vec::new();
        for frame in 0..360 {
            let ball = if (145..=250).contains(&frame) {
                None
            } else {
                b(80.0 + (frame % 145) as f64 * 2.0)
            };
            let contact = match frame {
                20 | 60 | 280 => hit_with_id(3).to_vec(),
                100 => hit_with_id(4).to_vec(),
                310 => hit_with_id(5).to_vec(),
                320 => vec![ContactFrame {
                    probability: 0.9,
                    provisional_track_id: None,
                }],
                _ => vec![],
            };
            if let Some(rally) = segment.push(frame, ball, &contact).unwrap() {
                rallies.push(rally);
            }
        }
        if let Some(rally) = segment.finish() {
            rallies.push(rally);
        }
        assert_eq!(rallies.len(), 2);
        assert_eq!(rallies[0].motion_supported_touch_proposals, 2);
        assert_eq!(rallies[1].motion_supported_touch_proposals, 2);
        assert_eq!(rallies[0].unidentified_contacts, 0);
        assert_eq!(rallies[1].unidentified_contacts, 0);
        assert_eq!(segment.totals().ignored_touch_proposals, 2);
    }

    #[test]
    fn invalid_parameters_sequence_and_untrusted_contacts_rejected() {
        assert!(RallySegmenter::new(
            RallySettings {
                min_touch_candidates: 0,
                ..RallySettings::default()
            },
            640,
            360
        )
        .is_err());
        let mut r = RallySegmenter::new(RallySettings::default(), 640, 360).unwrap();
        r.push(0, b(10.0), &[]).unwrap();
        assert!(r.push(2, b(12.0), &[]).is_err());
        assert!(r
            .push(
                1,
                b(12.0),
                &[ContactFrame {
                    probability: f64::NAN,
                    provisional_track_id: None
                }]
            )
            .is_err());
    }
}
