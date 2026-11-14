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
    last_busy: bool,
    sustained: bool,
}

impl Schedule {
    pub fn resume(cores: Option<usize>, blocks: &[Block], pairs: &[(usize, usize)], discarded: &[Burst]) -> Self {
        let mut s = Self { threshold: burst_pct(cores), seen: [false; 2], last_busy: false, sustained: false };
        // A checkpoint may end between a discarded attempt and its replacement. Recover the
        // chronological block order, including every such attempt, without any new stored state.
        let mut bursts = discarded.iter().peekable();
        for pair in 0..=pairs.len() {
            while let Some(b) = bursts.next_if(|b| b.pair == pair) {
                s.observe(pair, b.base, b.head);
                s.seen[usize::from(b.apart > 0.)] = true;
            }
            if let Some(&(a, b)) = pairs.get(pair) {
                s.decide(&blocks[a], &blocks[b], 0, false, false);
            }
        }
        s
    }

    fn observe(&mut self, pair: usize, base: f64, head: f64) {
        for pct in if pair.is_multiple_of(2) { [base, head] } else { [head, base] } {
            let busy = pct >= BACKGROUND_PCT;
            self.sustained |= self.last_busy && busy;
            self.last_busy = busy;
        }
    }

    pub fn decide(&mut self, base: &Block, head: &Block, discarded: usize, moves: bool, time_left: bool) -> Decision {
        let pct = |b: &Block| b.values.get("cpu.foreign.max").copied().unwrap_or(f64::NAN);
        self.observe(base.pair, pct(base), pct(head));
        let Some(apart) = foreign_apart(base, head).filter(|d| d.abs() >= self.threshold) else { return Decision::Keep(None) };
        let seen = std::mem::replace(&mut self.seen[usize::from(apart > 0.)], true);
        let reason = if self.sustained {
            "other work continued across consecutive blocks"
        } else if seen {
            "repeated load on the same side"
        } else if discarded >= BURST_RETRIES {
            "retry limit reached"
        } else if !moves {
            "load does not move the compared metrics"
        } else if !time_left {
            "time budget reached"
        } else {
            return Decision::Replace(apart);
        };
        Decision::Keep(Some(reason))
    }
}
