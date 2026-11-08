//! `strobe stats` `simulate` (hidden, SPEC §5.11): synthetic paired series through the decision code a
//! bench runs (`bench::sequential`: looks, Pocock level, futility, run-on for Holm and the family,
//! closing verdict, gate and Holm; `family::Board` for a family of journeys), so the simulations SPEC
//! §9 quotes are reproducible: `scripts/dev/stats-sim.ts` runs its rows. Only the blocks are made up:
//! each is `100 × (1 + effect on head) × e^(noise + penalties)`.

use crate::app::bench::sequential::{self, Design, Driver, Step};
use crate::model::{self, Error, MetricDef, Result, VerdictKind};
use crate::stats::{self, Comparison};
use serde_json::{Value, json};

/// SplitMix64: a seeded stream per trial, so a row's counts do not depend on the threads.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn u(&mut self) -> f64 {
        ((self.next() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }
    fn n(&mut self) -> f64 {
        (-2.0 * self.u().ln()).sqrt() * (2.0 * std::f64::consts::PI * self.u()).cos()
    }
}

#[derive(Clone, Copy)]
enum Noise {
    Normal(f64),
    /// Student's t with 3 degrees of freedom, scaled to standard deviation σ.
    T3(f64),
    /// Normal σ, and with probability p a block +10% to +50% (a preempted VM).
    Spike(f64, f64),
}

impl Noise {
    fn of(v: &Value) -> Result<Noise> {
        let sigma = v["sigma"].as_f64().unwrap_or(0.01);
        Ok(match v["kind"].as_str().unwrap_or("normal") {
            "normal" => Noise::Normal(sigma),
            "t3" => Noise::T3(sigma),
            "spike" => Noise::Spike(sigma, v["p"].as_f64().unwrap_or(0.1)),
            other => return Err(Error::usage(format!("simulate: noise kind normal, t3 or spike, not {other}"))),
        })
    }

    /// One block's log deviation, and its standardised normal part (for correlated secondaries).
    fn draw(self, r: &mut Rng) -> (f64, f64) {
        match self {
            Noise::Normal(s) => {
                let z = r.n();
                (s * z, z)
            }
            Noise::T3(s) => {
                let c: f64 = (0..3).map(|_| r.n().powi(2)).sum();
                let t = r.n() / (c / 3.0).sqrt() / 3f64.sqrt();
                (s * t, t)
            }
            Noise::Spike(s, p) => {
                let z = r.n();
                (s * z + if r.u() < p { (1.1 + 0.4 * r.u()).ln() } else { 0.0 }, z)
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Secondary {
    effect: f64,
    sigma: f64,
    rho: f64,
}

struct Params {
    def: &'static MetricDef,
    name: String,
    design: Design,
    noise: Noise,
    /// Log penalty on the series' first measured block (pair 0's base): a cold first launch.
    first: f64,
    /// One discarded block per side before pair 0 (SPEC §9.1 warm-up blocks).
    discard: bool,
    /// Log penalty on the second block of every pair.
    order: f64,
    secondaries: Vec<Secondary>,
}

/// One simulated series: its primary as the bench closes it, its secondaries after the gate and Holm,
/// and its pairs.
struct Series {
    primary: Comparison,
    secondaries: Vec<Comparison>,
    pairs: Vec<(f64, f64)>,
}

fn series(r: &mut Rng, p: &Params, effect: f64) -> Series {
    let sdef = model::metric("run.duration").expect("catalog");
    let mut driver = Driver::new(p.design.clone());
    let (mut pp, mut sp) = (Vec::new(), vec![Vec::new(); p.secondaries.len()]);
    let mut first = true;
    let block = |r: &mut Rng, head: bool, second: bool, first: &mut bool| -> (f64, Vec<f64>) {
        let (dev, z) = p.noise.draw(r);
        let penalty = if std::mem::take(first) { p.first } else { 0.0 } + if second { p.order } else { 0.0 };
        let v = 100.0 * if head { (1.0 + effect).max(1e-9) } else { 1.0 } * (dev + penalty).exp();
        let s = p
            .secondaries
            .iter()
            .map(|s| {
                let e = s.rho * z.clamp(-6.0, 6.0) + (1.0 - s.rho * s.rho).sqrt() * r.n();
                50.0 * if head { 1.0 + s.effect } else { 1.0 } * (s.sigma * e + penalty).exp()
            })
            .collect();
        (v, s)
    };
    if p.discard {
        block(r, false, false, &mut first);
        block(r, true, true, &mut first);
    }
    for pair in 0..p.design.max_pairs() {
        // Base first in even pairs, head first in odd ones, as `run_series` orders them.
        let base_first = pair % 2 == 0;
        let (b, h) = if base_first {
            let b = block(r, false, false, &mut first);
            (b, block(r, true, true, &mut first))
        } else {
            let h = block(r, true, false, &mut first);
            (block(r, false, true, &mut first), h)
        };
        pp.push((b.0, h.0));
        for (j, s) in sp.iter_mut().enumerate() {
            s.push((b.1[j], h.1[j]));
        }
        let tested = p.secondaries.len();
        if let (_, Step::Stop) = driver.after_pair(&p.name, p.def, &pp, || tested) {
            break;
        }
    }
    if driver.decided_at.is_some() {
        driver.stopped = "primary decided".into();
    }
    let decided = driver.primary_decided(pp.len());
    let mut primary = driver.primary().expect("pairs");
    if !decided {
        let (kind, words) = match sequential::closing(&p.design, p.def, &mut primary, &driver.stopped, &pp) {
            Some(v) => v,
            None if driver.stopped == "saturated" => (VerdictKind::Saturated, "saturated".into()),
            None => (VerdictKind::Unclear, "unclear".into()),
        };
        primary.set_verdict(kind, words);
    }
    let mut secondaries: Vec<Comparison> = sp.iter().map(|s| stats::compare("run.duration", sdef, s, p.design.bound, 0.95)).collect();
    sequential::gate_and_holm(&mut secondaries, decided);
    Series { primary, secondaries, pairs: pp }
}

/// A series as result.json keeps it, for `family::Board`.
fn series_json(name: &str, s: &Series) -> Value {
    let blocks: Vec<Value> =
        s.pairs.iter().enumerate().flat_map(|(i, &(b, h))| [json!({ "pair": i, "variant": "base", "values": { name: b } }), json!({ "pair": i, "variant": "head", "values": { name: h } })]).collect();
    json!({ "label": name, "primary": s.primary, "blocks": blocks, "pairs": s.pairs.len() })
}

#[derive(Default, Clone)]
struct Tally {
    trials: usize,
    kinds: std::collections::BTreeMap<String, usize>,
    pairs: usize,
    covered: usize,
    estimate: f64,
    first_pair: f64,
    sec_null_tests: usize,
    sec_null_called: usize,
    sec_any_wrong: usize,
    sec_true: usize,
    sec_true_hit: usize,
    // Families.
    fam_null_better: usize,
    fam_null_worse: usize,
    fam_true: usize,
    fam_true_counted: usize,
    fam_all_no_worse: usize,
}

impl Tally {
    fn add(&mut self, o: &Tally) {
        self.trials += o.trials;
        for (k, v) in &o.kinds {
            *self.kinds.entry(k.clone()).or_default() += v;
        }
        self.pairs += o.pairs;
        self.covered += o.covered;
        self.estimate += o.estimate;
        self.first_pair += o.first_pair;
        self.sec_null_tests += o.sec_null_tests;
        self.sec_null_called += o.sec_null_called;
        self.sec_any_wrong += o.sec_any_wrong;
        self.sec_true += o.sec_true;
        self.sec_true_hit += o.sec_true_hit;
        self.fam_null_better += o.fam_null_better;
        self.fam_null_worse += o.fam_null_worse;
        self.fam_true += o.fam_true;
        self.fam_true_counted += o.fam_true_counted;
        self.fam_all_no_worse += o.fam_all_no_worse;
    }
}

fn kind_name(k: VerdictKind) -> String {
    serde_json::to_value(k).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

/// One trial: a series, or with `journeys` a family of them read as a bench's result.
fn trial(r: &mut Rng, p: &Params, effects: &[f64], family: Option<usize>, t: &mut Tally) {
    t.trials += 1;
    let Some(m) = family else {
        let effect = effects[0];
        let s = series(r, p, effect);
        let c = &s.primary;
        *t.kinds.entry(kind_name(c.kind)).or_default() += 1;
        t.pairs += s.pairs.len();
        t.covered += (c.lo <= effect && effect <= c.hi) as usize;
        t.estimate += c.estimate;
        t.first_pair += (s.pairs[0].1 / s.pairs[0].0).ln();
        let mut wrong = false;
        for (sec, c) in p.secondaries.iter().zip(&s.secondaries) {
            let called = matches!(c.kind, VerdictKind::Better | VerdictKind::Worse);
            if sec.effect == 0.0 {
                t.sec_null_tests += 1;
                t.sec_null_called += called as usize;
                wrong |= called;
            } else {
                t.sec_true += 1;
                let right = called && (c.kind == VerdictKind::Worse) == (sec.effect > 0.0);
                t.sec_true_hit += right as usize;
                wrong |= called && !right;
            }
        }
        t.sec_any_wrong += wrong as usize;
        return;
    };
    let runs: Vec<Series> = (0..m).map(|j| series(r, p, effects[j % effects.len()])).collect();
    let result = json!({
        "comparison": "implementations", "plan": { "looks": p.design.looks },
        "series": runs.iter().map(|s| series_json(&p.name, s)).collect::<Vec<_>>(),
    });
    let board = crate::app::family::Board::of(&result);
    let f = &board.family;
    let (mut nb, mut nw) = (false, false);
    for (j, s) in runs.iter().enumerate() {
        let e = effects[j % effects.len()];
        if e == 0.0 {
            nb |= board.win(j);
            nw |= board.regression(j);
        } else {
            t.fam_true += 1;
            t.fam_true_counted += f.holm.get(j).copied().unwrap_or(false) as usize;
        }
        *t.kinds.entry(kind_name(s.primary.kind)).or_default() += 1;
        t.pairs += s.pairs.len();
    }
    t.fam_null_better += nb as usize;
    t.fam_null_worse += nw as usize;
    t.fam_all_no_worse += (f.all_no_worse == Some(true)) as usize;
}

pub(super) fn run(q: &Value) -> Result<Value> {
    let name = q["metric"].as_str().unwrap_or("cpu.work").to_string();
    let def = if q["higher"] == true { &model::METRIC_HIGHER } else { model::metric(&name).ok_or_else(|| Error::usage(format!("simulate: no metric {name}")))? };
    let fixed = match &q["pairs"] {
        Value::Number(n) => n.as_u64().map(|n| n as usize),
        _ => None,
    };
    let family = q["journeys"].as_u64().map(|m| m as usize);
    let effects: Vec<f64> = match q["effects"].as_array() {
        Some(a) => a.iter().filter_map(Value::as_f64).collect(),
        None => vec![q["effect"].as_f64().unwrap_or(0.0)],
    };
    if effects.is_empty() {
        return Err(Error::usage("simulate: effects is empty"));
    }
    let mut design = Design::new(fixed, q["bound"].as_f64().unwrap_or(0.02), q["noworse"].as_f64(), family);
    // Another design to compare with (a look after every pair: `"looks": [2, 3, …, 40]`), at
    // `confidence`.
    if let Some(l) = q["looks"].as_array() {
        design.looks = l.iter().filter_map(|x| x.as_u64().map(|n| n as usize)).collect();
        design.confidence = q["confidence"].as_f64().unwrap_or(0.95);
        if design.looks.is_empty() {
            return Err(Error::usage("simulate: looks is empty"));
        }
    }
    let p = Params {
        def,
        name,
        design,
        noise: Noise::of(&q["noise"])?,
        first: q["first"].as_f64().unwrap_or(0.0),
        discard: q["discard"] == true,
        order: q["order"].as_f64().unwrap_or(0.0),
        secondaries: q["secondaries"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|s| Secondary { effect: s["effect"].as_f64().unwrap_or(0.0), sigma: s["sigma"].as_f64().unwrap_or(0.03), rho: s["rho"].as_f64().unwrap_or(0.0) })
            .collect(),
    };
    let trials = q["trials"].as_u64().unwrap_or(4000) as usize;
    let seed = q["seed"].as_u64().unwrap_or(1);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(trials.max(1));
    let total = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|k| {
                let (p, effects) = (&p, &effects);
                scope.spawn(move || {
                    let mut t = Tally::default();
                    for i in (k..trials).step_by(threads) {
                        let mut r = Rng(seed.wrapping_mul(0x2545_F491_4F6C_DD1D) ^ (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
                        trial(&mut r, p, effects, family, &mut t);
                    }
                    t
                })
            })
            .collect();
        let mut all = Tally::default();
        for h in handles {
            all.add(&h.join().unwrap_or_default());
        }
        all
    });
    let n = total.trials.max(1) as f64;
    let series_n: usize = total.kinds.values().sum();
    let mut out = json!({
        "trials": total.trials, "verdicts": total.kinds, "pairs_mean": total.pairs as f64 / series_n.max(1) as f64,
    });
    if family.is_none() {
        out["coverage"] = json!(total.covered as f64 / n);
        out["estimate_mean"] = json!(total.estimate / n);
        out["first_pair_mean"] = json!(total.first_pair / n);
        if !p.secondaries.is_empty() {
            out["secondaries"] = json!({
                "any_wrong": total.sec_any_wrong as f64 / n,
                "null_called": total.sec_null_called as f64 / total.sec_null_tests.max(1) as f64,
                "true_hit": total.sec_true_hit as f64 / total.sec_true.max(1) as f64,
            });
        }
    } else {
        out["family"] = json!({
            "any_null_better": total.fam_null_better as f64 / n, "any_null_worse": total.fam_null_worse as f64 / n,
            "true_counted": total.fam_true_counted as f64 / total.fam_true.max(1) as f64, "all_no_worse": total.fam_all_no_worse as f64 / n,
        });
    }
    Ok(out)
}
