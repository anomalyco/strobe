//! Decide only whether to replace a finished pair. The paired load tests still see every attempt.
use super::{BURST_RETRIES, Block, Burst, burst_pct, foreign_apart};

pub enum Decision {
    Keep(Option<&'static str>),
    Replace(f64),
}

pub struct Schedule {
    threshold: f64,
    seen: [bool; 2],
}

impl Schedule {
    pub fn resume(cores: Option<usize>, blocks: &[Block], pairs: &[(usize, usize)], discarded: &[Burst]) -> Self {
        let mut s = Self { threshold: burst_pct(cores), seen: [false; 2] };
        for apart in discarded.iter().map(|b| b.apart).chain(pairs.iter().filter_map(|&(a,b)| foreign_apart(&blocks[a], &blocks[b]))) {
            if apart.abs() >= s.threshold { s.seen[usize::from(apart > 0.)] = true; }
        }
        s
    }

    pub fn decide(&mut self, base: &Block, head: &Block, discarded: usize, moves: bool, time_left: bool) -> Decision {
        let Some(apart) = foreign_apart(base, head).filter(|d| d.abs() >= self.threshold) else { return Decision::Keep(None) };
        let seen = std::mem::replace(&mut self.seen[usize::from(apart > 0.)], true);
        let reason = if seen { "repeated load on the same side" } else if discarded >= BURST_RETRIES { "retry limit reached" } else if !moves { "load does not move the compared metrics" } else if !time_left { "time budget reached" } else { return Decision::Replace(apart) };
        Decision::Keep(Some(reason))
    }
}
