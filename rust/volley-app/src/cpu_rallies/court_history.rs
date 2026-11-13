//! Bounded, append-only court-context evidence ring for accepted CPU rallies.
//! The authoritative Touch writer and RAVEL-frame reader remain unchanged.
use std::collections::VecDeque;
use volley_core::{
    rally_contact_context::{self, BallSample, ContactEvidence, RallyContext},
    rally_reconstruction::RallyProposal,
};

const HISTORY_FRAMES: u64 = 18_000; // 10 minutes at prepared 30 FPS.
const MAX_CONTEXT_CONTACTS: usize = 512;

#[derive(Default)]
pub(super) struct CourtHistory {
    balls: VecDeque<BallSample>,
    contacts: VecDeque<ContactEvidence>,
    // Only needed when a rally lasts so long that its beginning was discarded.
    truncated_before: u64,
}

impl CourtHistory {
    pub(super) fn push_ball(&mut self, sample: BallSample) {
        self.balls.push_back(sample);
    }

    pub(super) fn push_touch(&mut self, item: ContactEvidence) {
        self.contacts.push_back(item);
    }

    pub(super) fn trim(&mut self, frame: u64) {
        let before = frame.saturating_sub(HISTORY_FRAMES);
        while self
            .balls
            .front()
            .is_some_and(|sample| sample.frame < before)
        {
            self.balls.pop_front();
            self.truncated_before = before;
        }
        while self
            .contacts
            .front()
            .is_some_and(|item| item.frame < before)
        {
            self.contacts.pop_front();
            self.truncated_before = before;
        }
    }

    /// Compute possession from the exact accepted rally interval, not any
    /// player who happened to appear in the same long source video.
    pub(super) fn for_rally(&self, proposal: &RallyProposal) -> Option<RallyContext> {
        let start = proposal.detected_start_frame;
        let end = proposal.detected_end_frame;
        if start < self.truncated_before || end < start {
            return None;
        }
        let mut events = Vec::new();
        let mut last_track_id = None;
        for contact in self
            .contacts
            .iter()
            .filter(|c| (start..=end).contains(&c.frame))
        {
            let Some(track_id) = contact.track_id else {
                continue;
            };
            if last_track_id == Some(track_id) {
                continue;
            }
            last_track_id = Some(track_id);
            if events.len() == MAX_CONTEXT_CONTACTS {
                // Never publish a misleading partial analysis.
                return None;
            }
            events.push(*contact);
        }
        if !events.iter().any(|c| c.side.is_some()) {
            return None;
        }
        let ball = self
            .balls
            .iter()
            .filter(|sample| (start..=end).contains(&sample.frame))
            .copied()
            .collect::<Vec<_>>();
        Some(rally_contact_context::analyze(&events, &ball))
    }
}
