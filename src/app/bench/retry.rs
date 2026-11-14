//! Decide only whether to replace a finished pair. The paired load tests still see every attempt.
use super::{BACKGROUND_PCT, BURST_RETRIES, Block, Burst, burst_pct, foreign_apart};

#[derive(Debug, PartialEq)]
pub enum Decision {
    Keep(Option<&'static str>),
    Replace(f64),
}

pub struct Schedule {
    threshold: f64,
    seen: [bool; 2],
    replacing: bool,
    busy_replacement: bool,
}

impl Schedule {
    pub fn resume(cores: Option<usize>, blocks: &[Block], pairs: &[(usize, usize)], discarded: &[Burst]) -> Self {
        let mut s = Self { threshold: burst_pct(cores), seen: [false; 2], replacing: false, busy_replacement: false };
        // A checkpoint may end between a discarded attempt and its replacement. Recover the
        // chronological block order, including every such attempt, without any new stored state.
        let mut bursts = discarded.iter().peekable();
        for pair in 0..=pairs.len() {
            while let Some(b) = bursts.next_if(|b| b.pair == pair) {
                s.seen[usize::from(b.apart > 0.)] = true;
                s.replacing = true;
            }
            if let Some(&(a, b)) = pairs.get(pair) {
                s.decide(&blocks[a], &blocks[b], 0, false, false);
            }
        }
        s
    }

    /// The quiet wait ended the budget: keep this observed pair, do not treat the next as its replacement.
    pub fn cancel_replace(&mut self) {
        self.replacing = false;
    }

    pub fn decide(&mut self, base: &Block, head: &Block, discarded: usize, moves: bool, time_left: bool) -> Decision {
        let pct = |b: &Block| b.values.get("cpu.foreign.max").copied().unwrap_or(f64::NAN);
        let busy = |b| pct(b) >= BACKGROUND_PCT;
        let quiet = |b: &Block| pct(b) < BACKGROUND_PCT && !super::unread_load(b) && !b.values.contains_key("background.subtick");
        if quiet(base) && quiet(head) {
            self.busy_replacement = false;
        }
        if std::mem::take(&mut self.replacing) && (busy(base) || busy(head)) {
            self.busy_replacement = true;
        }
        let Some(apart) = foreign_apart(base, head).filter(|d| d.abs() >= self.threshold) else { return Decision::Keep(None) };
        // Track isolated hits on each side, not a busy patch that has its own reset rule.
        let reason = if busy(base) && busy(head) {
            "other work ran on both sides of this pair"
        } else if self.busy_replacement {
            "a replacement was busy and no clean pair followed"
        } else if std::mem::replace(&mut self.seen[usize::from(apart > 0.)], true) {
            "repeated load on the same side"
        } else if discarded >= BURST_RETRIES {
            "retry limit reached"
        } else if !moves {
            "load does not move the compared metrics"
        } else if !time_left {
            "time budget reached"
        } else {
            self.replacing = true;
            return Decision::Replace(apart);
        };
        Decision::Keep(Some(reason))
    }
}
