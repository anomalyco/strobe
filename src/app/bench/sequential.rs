//! What a paired series decides after each pair and at its end (SPEC §9.1 stop rule, §9.3 gate and
//! Holm): the planned looks at a Pocock level, the futility projection, the pairs a decided primary
//! runs on for its secondaries and its journeys' family, the closing verdict of a series that never
//! decided, and the secondaries' gate and Holm. `run_series` drives it with blocks it measures;
//! `strobe stats` (`simulate`, SPEC §5.11) drives the same code with synthetic pairs, so the
//! simulations SPEC §9 quotes run through what a bench runs.

use crate::model::{MetricDef, VerdictKind};
use crate::stats::{self, Comparison};

/// Pairs before a series may stop as underpowered.
pub(crate) const FUTILITY_FROM: usize = 12;

/// Planned looks (pairs) for `--pairs auto`. Verdicts happen only here.
pub(crate) const LOOKS: [usize; 5] = [8, 12, 18, 26, 40];

/// Pocock's constant nominal level for 5 looks at 5% overall, two-sided: each look's interval is
/// 98.4%. Looking after every pair at 95% called A/A runs different 10% of the time (simulated, and
/// once on adais); five Pocock looks keep it at 1.5–3.7% and still catch a 3% effect by pair 8–9.
pub(crate) const POCOCK_ALPHA: f64 = 0.0158;

/// A series' design: its looks and their level, its bound or margin, and its journeys' family.
#[derive(Clone, Debug)]
pub(crate) struct Design {
    pub looks: Vec<usize>,
    pub confidence: f64,
    /// `--pairs N`: one look at N, run exactly N.
    pub fixed: bool,
    pub bound: f64,
    /// Non-inferiority's margin for the primary (SPEC §9.3).
    pub margin: Option<f64>,
    /// An implementation comparison's compared journeys: a primary decided better or worse runs on to
    /// `stats::family_min_pairs` of them.
    pub journeys: Option<usize>,
}

impl Design {
    /// `--pairs auto` (None) or `--pairs N`.
    pub fn new(pairs: Option<usize>, bound: f64, margin: Option<f64>, journeys: Option<usize>) -> Design {
        match pairs {
            None => Design { looks: LOOKS.to_vec(), confidence: 1.0 - POCOCK_ALPHA, fixed: false, bound, margin, journeys },
            Some(n) => Design { looks: vec![n.max(2)], confidence: 0.95, fixed: true, bound, margin, journeys },
        }
    }

    pub fn max_pairs(&self) -> usize {
        *self.looks.last().unwrap_or(&40)
    }

    pub fn min_pairs(&self) -> usize {
        self.looks[0]
    }

    /// The primary's comparison of these pairs at the design's level.
    pub fn compare(&self, name: &str, def: &MetricDef, pp: &[(f64, f64)]) -> Comparison {
        match self.margin {
            Some(m) => stats::compare_noworse(name, def, pp, m, self.confidence),
            None => stats::compare(name, def, pp, self.bound, self.confidence),
        }
    }

    /// The pairs a primary decided at a look runs on to: those its `tested` secondaries need to clear
    /// Holm's first step, and its journeys' family when it is better or worse (never with `--pairs N`).
    pub fn run_on(&self, c: &Comparison, tested: usize) -> (usize, usize) {
        let fam = match self.journeys {
            Some(m) if matches!(c.kind, VerdictKind::Better | VerdictKind::Worse) => stats::family_min_pairs(m, &self.looks),
            _ => 0,
        };
        (holm_pairs(tested), fam)
    }
}

/// The fewest pairs whose exact signed-rank test can clear Holm's first step across `tests` secondaries.
pub(crate) fn holm_pairs(tests: usize) -> usize {
    (2..=60).find(|n| 2.0 / 2f64.powi(*n) <= 0.05 / tests.max(1) as f64).unwrap_or(60) as usize
}

/// What a series learns after its `n`-th pair (`pp` holds every pair so far).
pub(crate) struct Look {
    pub cmp: Comparison,
    /// A planned look.
    pub look: bool,
    /// Decided at this look.
    pub decided: bool,
    /// At a look from `FUTILITY_FROM`, its projection needs more pairs than the last look: give up.
    pub hopeless: bool,
    /// A share primary at its ceiling on both sides at a look without a verdict.
    pub saturated: bool,
    /// The next planned look.
    pub next: Option<usize>,
}

/// After a pair: the comparison, whether this is a look that decides it, and whether to give up.
/// Pairs needed come from a robust spread of the differences (`stats::pairs_needed`), at the even
/// chance (`FUTILITY_POWER`): an early interval is [min, max] of few Walsh averages, so one odd pair
/// would stop a series that more pairs decide.
pub(crate) fn look(design: &Design, name: &str, def: &MetricDef, pp: &[(f64, f64)]) -> Look {
    let n = pp.len();
    let look = design.looks.contains(&n);
    let mut cmp = design.compare(name, def, pp);
    let decided = look && cmp.decided;
    if !cmp.decided {
        let d: Vec<f64> = pp.iter().map(|&(a, b)| if cmp.relative { (b / a).ln() } else { b - a }).collect();
        cmp.needed_pairs = Some(match cmp.noworse {
            Some(m) => stats::pairs_needed_noworse(&d, stats::margin_shift(def, cmp.relative, m), design.confidence, stats::FUTILITY_POWER),
            None => stats::pairs_needed(&d, if cmp.relative { (1.0 + design.bound).ln() } else { cmp.bound }, design.confidence, stats::FUTILITY_POWER),
        });
    }
    let hopeless = !design.fixed && look && n >= FUTILITY_FROM && cmp.needed_pairs.is_some_and(|need| need > design.max_pairs());
    // A share at its ceiling on both sides (91% vs 92% dropped frames at background priority) moves
    // with nothing the builds do: more pairs only narrow an interval around "same at the ceiling".
    let median = |side: usize| crate::metrics::median(&pp.iter().map(|p| if side == 0 { p.0 } else { p.1 }).collect::<Vec<_>>());
    let saturated = look && !decided && def.unit == "%" && !name.starts_with("cpu.") && median(0) >= 80.0 && median(1) >= 80.0;
    Look { cmp, look, decided, hopeless, saturated, next: design.looks.iter().find(|&&l| l > n).copied() }
}

/// What the series does after a pair (`Driver::after_pair`).
pub(crate) enum Step {
    /// More pairs: no look decided it, and nothing says to stop.
    Continue,
    /// Decided at a look (`now`) or earlier, and running on to `need` pairs: `holm` for the
    /// secondaries, `fam` for the journeys' family.
    RunOn { need: usize, holm: usize, fam: usize, now: bool },
    /// Stop: `Driver::stopped` says why.
    Stop,
}

/// One series' sequential state: the look that decided its primary (it then runs on for its
/// secondaries and family, keeping that look's verdict), its last comparison, and why it stopped
/// (`stopped`, as result.json keeps it: `primary decided`, `saturated`, `more pairs cannot decide
/// it`, `max pairs`; the bench adds `max time`, `stopped`, `failed: …`, `journeys differ`).
pub(crate) struct Driver {
    pub design: Design,
    pub decided_at: Option<Comparison>,
    /// The pairs at that look (`primary_pairs`; a checkpoint keeps it for `--resume`).
    pub decided_n: Option<usize>,
    pub last: Option<Comparison>,
    pub stopped: String,
}

impl Driver {
    pub fn new(design: Design) -> Driver {
        Driver { design, decided_at: None, decided_n: None, last: None, stopped: "max pairs".into() }
    }

    /// After the pair that made `pp` (every pair so far): the look's comparison when the primary was
    /// not yet decided, and what to do. `tested` counts the secondaries that will be tested (the Holm
    /// family the run-on pairs are for).
    pub fn after_pair(&mut self, name: &str, def: &MetricDef, pp: &[(f64, f64)], tested: impl Fn() -> usize) -> (Option<Look>, Step) {
        let n = pp.len();
        if let Some(c) = &self.decided_at {
            let (holm, fam) = self.design.run_on(c, tested());
            let need = holm.max(fam);
            if n >= need {
                self.stopped = "primary decided".into();
                return (None, Step::Stop);
            }
            return (None, Step::RunOn { need, holm, fam, now: false });
        }
        let lk = look(&self.design, name, def, pp);
        self.last = Some(lk.cmp.clone());
        if lk.decided {
            // Secondaries clear Holm only from enough pairs: run on to them rather than leave them blank.
            let (holm, fam) = self.design.run_on(&lk.cmp, tested());
            let need = holm.max(fam);
            if n < need && n < self.design.max_pairs() && !self.design.fixed {
                self.decided_at = Some(lk.cmp.clone());
                self.decided_n = Some(n);
                return (Some(lk), Step::RunOn { need, holm, fam, now: true });
            }
            self.stopped = "primary decided".into();
            return (Some(lk), Step::Stop);
        }
        let stop = if lk.saturated {
            Some("saturated")
        } else if lk.hopeless {
            Some("more pairs cannot decide it")
        } else {
            None
        };
        match stop {
            Some(why) => {
                self.stopped = why.into();
                (Some(lk), Step::Stop)
            }
            None => (Some(lk), Step::Continue),
        }
    }

    /// The primary as the series ends: the look that decided it, else the last comparison.
    pub fn primary(&self) -> Option<Comparison> {
        self.decided_at.clone().or_else(|| self.last.clone())
    }

    /// Decided: at a look, with at least the first look's pairs.
    pub fn primary_decided(&self, pairs: usize) -> bool {
        (self.decided_at.is_some() || self.stopped == "primary decided") && pairs >= self.design.min_pairs()
    }
}

/// The verdict of a primary that was not decided, for the statistical stops (`stopped`: `more pairs
/// cannot decide it`, `max pairs`, `max time`): `underpowered` with the pairs an 80% chance needs
/// (`PLAN_POWER`, as `plan` says them) when more would decide it, else `unclear`. Other stops (a
/// stop, a failure, saturation) say their own words; this returns None for them.
pub(crate) fn closing(design: &Design, def: &MetricDef, primary: &mut Comparison, stopped: &str, pp: &[(f64, f64)]) -> Option<(VerdictKind, String)> {
    if !matches!(stopped, "more pairs cannot decide it" | "max pairs" | "max time") {
        return None;
    }
    Some(match primary.needed_pairs {
        Some(n) if n != usize::MAX && n > pp.len() => {
            let d: Vec<f64> = pp.iter().map(|&(a, b)| if primary.relative { (b / a).ln() } else { b - a }).collect();
            let n80 = match primary.noworse {
                Some(m) => stats::pairs_needed_noworse(&d, stats::margin_shift(def, primary.relative, m), design.confidence, stats::PLAN_POWER),
                None => stats::pairs_needed(&d, if primary.relative { (1.0 + design.bound).ln() } else { primary.bound }, design.confidence, stats::PLAN_POWER),
            };
            let n80 = if n80 == usize::MAX { n } else { n80.max(n) };
            primary.needed_pairs = Some(n80);
            (VerdictKind::Underpowered, format!("underpowered: ~{n80} pairs needed (for an 80% chance)"))
        }
        _ if stopped == "max pairs" && !design.fixed => (
            VerdictKind::Unclear,
            format!("unclear after {} pairs, the last planned look: the difference sits on the {}", pp.len(), if design.margin.is_some() { "margin" } else { "equivalence bound" }),
        ),
        _ => (VerdictKind::Unclear, "unclear".into()),
    })
}

/// Secondaries: gated on the primary, Holm-corrected at 5% across those actually tested. An exact
/// comparison (a stable counter) is a fact, not a test, and a refused one tests nothing: neither
/// joins the family (a refused row's p once sized it, so a valid secondary's verdict was wiped). A
/// non-inferiority secondary's test is against its margin: rejected, its interval clears it. Returns
/// how many were tested.
pub(crate) fn gate_and_holm(secondaries: &mut [Comparison], primary_decided: bool) -> usize {
    let tested: Vec<usize> = (0..secondaries.len()).filter(|&i| !secondaries[i].exact && secondaries[i].kind != VerdictKind::Refused).collect();
    let ps: Vec<f64> = tested.iter().map(|&i| secondaries[i].p).collect();
    let rejected = stats::holm(&ps, 0.05);
    for (&i, rej) in tested.iter().zip(rejected) {
        let c = &mut secondaries[i];
        let directional = if c.noworse.is_some() { c.decided } else { c.lo > 0.0 || c.hi < 0.0 };
        let keep = primary_decided && ((rej && directional) || c.kind == VerdictKind::Same);
        if !keep {
            c.set_verdict(VerdictKind::Untested, "—");
            c.decided = false;
        }
    }
    tested.len()
}
