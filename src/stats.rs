//! Paired statistics (SPEC §9.3): Wilcoxon signed-rank, Hodges–Lehmann, Holm, verdicts, mValue.

use crate::model::{Better, Kind, MetricDef, Refusal, ValidatorId, VerdictKind};
use serde::Serialize;

/// Signed-rank null distribution: counts of rank sums for n pairs (exact as f64 up to n = 50).
fn signed_rank_counts(n: usize) -> Vec<f64> {
    let max = n * (n + 1) / 2;
    let mut c = vec![0.0f64; max + 1];
    c[0] = 1.0;
    for k in 1..=n {
        for s in (k..=max).rev() {
            c[s] += c[s - k];
        }
    }
    c
}

/// Two-sided p-value of the signed-rank test for differences `d` (zeros dropped). Tied |d| share their
/// mid-rank, and the null distribution is that of those ranks (quantized metrics tie often: counts,
/// whole milliseconds): exact up to n = 400 from the distribution of the doubled ranks (whole numbers)
/// over every sign assignment, built as probabilities (each rank's sign halves them, so no count
/// outgrows an f64's 53 bits); above, the normal approximation, its variance less the ties'
/// Σ(t³ − t)/48. The untied distribution with a rounded statistic once gave p 0.039 where the exact
/// one is 0.055 (`d = [1, 0, 1, 3, 1, 0, 1, 1, −1, 2]`), and a normal tail with 0.5 continuity
/// correction is too small when every rank ties (a step of 30.5, not 1).
pub fn wilcoxon_p(d: &[f64]) -> f64 {
    let nz: Vec<f64> = d.iter().copied().filter(|x| *x != 0.0 && x.is_finite()).collect();
    let n = nz.len();
    if n == 0 {
        return 1.0;
    }
    let mut idx: Vec<usize> = (0..n).collect();
    idx.sort_by(|&a, &b| nz[a].abs().total_cmp(&nz[b].abs()));
    // Doubled mid-ranks (i + j + 2 for a tie from i to j, 0-based): whole numbers.
    let mut rank2 = vec![0usize; n];
    let mut ties = 0.0;
    let mut i = 0;
    while i < n {
        let mut j = i;
        while j + 1 < n && nz[idx[j + 1]].abs() == nz[idx[i]].abs() {
            j += 1;
        }
        for k in i..=j {
            rank2[idx[k]] = i + j + 2;
        }
        let t = (j - i + 1) as f64;
        ties += t * t * t - t;
        i = j + 1;
    }
    let t2: usize = (0..n).filter(|&k| nz[k] > 0.0).map(|k| rank2[k]).sum();
    if n <= 400 {
        let max: usize = rank2.iter().sum();
        let mut c = vec![0.0f64; max + 1];
        c[0] = 1.0;
        let mut reach = 0;
        for &r in &rank2 {
            reach += r;
            for s in (0..=reach).rev() {
                c[s] = 0.5 * (c[s] + if s >= r { c[s - r] } else { 0.0 });
            }
        }
        let lower: f64 = c[..=t2].iter().sum();
        let upper: f64 = c[t2..].iter().sum();
        return (2.0 * lower.min(upper)).min(1.0);
    }
    let nf = n as f64;
    let t = t2 as f64 / 2.0;
    let mean = nf * (nf + 1.0) / 4.0;
    let sd = (nf * (nf + 1.0) * (2.0 * nf + 1.0) / 24.0 - ties / 48.0).sqrt();
    if sd <= 0.0 {
        return 1.0;
    }
    let z = (((t - mean).abs() - 0.5) / sd).max(0.0);
    two_tail(z).min(1.0)
}

pub(crate) fn normal_quantile(p: f64) -> f64 {
    // Acklam's approximation
    let a = [-39.69683028665376, 220.9460984245205, -275.9285104469687, 138.357_751_867_269, -30.66479806614716, 2.506628277459239];
    let b = [-54.47609879822406, 161.5858368580409, -155.6989798598866, 66.80131188771972, -13.28068155288572];
    let c = [-0.007784894002430293, -0.3223964580411365, -2.400758277161838, -2.549732539343734, 4.374664141464968, 2.938163982698783];
    let d = [0.007784695709041462, 0.3224671290700398, 2.445134137142996, 3.754408661907416];
    let pl = 0.02425;
    if p < pl {
        let q = (-2.0 * p.ln()).sqrt();
        (((((c[0] * q + c[1]) * q + c[2]) * q + c[3]) * q + c[4]) * q + c[5]) / ((((d[0] * q + d[1]) * q + d[2]) * q + d[3]) * q + 1.0)
    } else if p <= 1.0 - pl {
        let q = p - 0.5;
        let r = q * q;
        (((((a[0] * r + a[1]) * r + a[2]) * r + a[3]) * r + a[4]) * r + a[5]) * q / (((((b[0] * r + b[1]) * r + b[2]) * r + b[3]) * r + b[4]) * r + 1.0)
    } else {
        -normal_quantile(1.0 - p)
    }
}

/// Growth per step of a series taken at equal steps (a census after every run): the Theil–Sen slope,
/// the median of all pairwise slopes, with Sen's distribution-free interval from Kendall's tau at
/// `confidence`. Robust to a few outlying readings (a late GC). `None` below 3 points.
pub fn theil_sen(ys: &[f64], confidence: f64) -> Option<(f64, f64, f64)> {
    let pts: Vec<(f64, f64)> = ys.iter().enumerate().filter(|(_, y)| y.is_finite()).map(|(i, y)| (i as f64, *y)).collect();
    let n = pts.len();
    if n < 3 {
        return None;
    }
    let mut slopes: Vec<f64> = (0..n).flat_map(|i| (i + 1..n).map(move |j| (i, j))).map(|(i, j)| (pts[j].1 - pts[i].1) / (pts[j].0 - pts[i].0)).collect();
    slopes.sort_by(|a, b| a.total_cmp(b));
    let m = slopes.len();
    let median = if m % 2 == 1 { slopes[m / 2] } else { (slopes[m / 2 - 1] + slopes[m / 2]) / 2.0 };
    let z = normal_quantile(1.0 - (1.0 - confidence) / 2.0);
    let c = z * ((n * (n - 1) * (2 * n + 5)) as f64 / 18.0).sqrt();
    // Sen's interval is the M1-th to the (M2 + 1)-th smallest slope (1-based), M1 = (m − C)/2 and
    // M2 = (m + C)/2, rounded outward: 0-based ⌊M1⌋ − 1 and ⌈M2⌉, which sum to m − 1 (symmetric).
    let lo = (((m as f64 - c) / 2.0).floor() - 1.0).max(0.0) as usize;
    let hi = ((((m as f64 + c) / 2.0).ceil()) as usize).min(m - 1);
    Some((median, slopes[lo.min(m - 1)], slopes[hi]))
}

/// Hodges–Lehmann estimate and its distribution-free interval (Walsh averages) at `confidence`.
pub fn hodges_lehmann(d: &[f64], confidence: f64) -> (f64, f64, f64, f64) {
    let v: Vec<f64> = d.iter().copied().filter(|x| x.is_finite()).collect();
    let n = v.len();
    if n == 0 {
        return (f64::NAN, f64::NAN, f64::NAN, 0.0);
    }
    let mut walsh = Vec::with_capacity(n * (n + 1) / 2);
    for i in 0..n {
        for j in i..n {
            walsh.push((v[i] + v[j]) / 2.0);
        }
    }
    walsh.sort_by(|a, b| a.total_cmp(b));
    let m = walsh.len();
    let est = if m % 2 == 1 { walsh[m / 2] } else { (walsh[m / 2 - 1] + walsh[m / 2]) / 2.0 };
    let alpha = 1.0 - confidence;
    // The interval [W(c), W(m-1-c)] (0-based, sorted Walsh averages) covers 1 - 2·P(T <= c), with c
    // the largest value where P(T <= c) <= alpha/2. When even c = 0 is too likely (fewer than 6
    // pairs at 95%), no interval reaches the level: [min, max] covers only 1 - 2·P(T = 0).
    let (c, achieved) = if n <= 50 {
        let counts = signed_rank_counts(n);
        let total = 2f64.powi(n as i32);
        let mut acc = 0.0;
        let mut c: i64 = -1;
        for (k, cnt) in counts.iter().enumerate() {
            if (acc + cnt) / total > alpha / 2.0 {
                break;
            }
            acc += cnt;
            c = k as i64;
        }
        if c < 0 { (0, 1.0 - 2.0 * counts[0] / total) } else { (c, 1.0 - 2.0 * acc / total) }
    } else {
        let nf = n as f64;
        let mean = nf * (nf + 1.0) / 4.0;
        let sd = (nf * (nf + 1.0) * (2.0 * nf + 1.0) / 24.0).sqrt();
        ((mean - normal_quantile(1.0 - alpha / 2.0) * sd - 0.5).floor().max(0.0) as i64, confidence)
    };
    let lo_i = c.clamp(0, m as i64 - 1) as usize;
    let hi_i = (m as i64 - 1 - c).clamp(0, m as i64 - 1) as usize;
    (est, walsh[lo_i.min(hi_i)], walsh[hi_i.max(lo_i)], achieved)
}

/// The median of `v` and its distribution-free interval (order statistics, the sign test's) at
/// `confidence`: `[x(r), x(n+1−r)]` (1-based, sorted) with r the largest where P(B ≤ r − 1) ≤ α/2 for
/// B ~ Binomial(n, ½), covering 1 − 2·P(B ≤ r − 1). The median always lies inside it, which a
/// Hodges–Lehmann interval of the same values does not promise. Below 6 values (at 95%) no r reaches
/// the level: `[min, max]`, covering 1 − 2^(1−n). Returns (median, lo, hi, achieved).
pub fn median_interval(v: &[f64], confidence: f64) -> (f64, f64, f64, f64) {
    let mut s: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
    let n = s.len();
    if n == 0 {
        return (f64::NAN, f64::NAN, f64::NAN, 0.0);
    }
    s.sort_by(|a, b| a.total_cmp(b));
    let med = if n % 2 == 1 { s[n / 2] } else { (s[n / 2 - 1] + s[n / 2]) / 2.0 };
    let alpha = 1.0 - confidence;
    // P(B ≤ k) for k = 0, 1, …: the binomial's lower tail, built term by term.
    let total = 2f64.powi(n as i32);
    let (mut term, mut tail, mut r) = (1.0f64, 0.0f64, 0usize);
    for k in 0..n.div_ceil(2) {
        if k > 0 {
            term = term * (n - k + 1) as f64 / k as f64;
        }
        if (tail + term) / total > alpha / 2.0 {
            break;
        }
        tail += term;
        r = k + 1;
    }
    if r == 0 {
        return (med, s[0], s[n - 1], 1.0 - 2.0 / total);
    }
    (med, s[r - 1], s[n - r], 1.0 - 2.0 * tail / total)
}

/// Holm step-down: which of `p` are rejected at `alpha`.
pub fn holm(p: &[f64], alpha: f64) -> Vec<bool> {
    let mut idx: Vec<usize> = (0..p.len()).collect();
    idx.sort_by(|&a, &b| p[a].total_cmp(&p[b]));
    let mut out = vec![false; p.len()];
    for (rank, &i) in idx.iter().enumerate() {
        if p[i] <= alpha / (p.len() - rank) as f64 {
            out[i] = true;
        } else {
            break;
        }
    }
    out
}

/// Complementary error function with a small relative error in the tails (Numerical Recipes
/// `erfcc`, fractional error under 1.2e-7 everywhere), so a family's tiny p-values keep their digits.
fn erfc(x: f64) -> f64 {
    let z = x.abs();
    let t = 1.0 / (1.0 + 0.5 * z);
    let r = t
        * (-z * z - 1.26551223
            + t * (1.00002368 + t * (0.37409196 + t * (0.09678418 + t * (-0.18628806 + t * (0.27886807 + t * (-1.13520398 + t * (1.48851587 + t * (-0.82215223 + t * 0.17087277)))))))))
            .exp();
    if x >= 0.0 { r } else { 2.0 - r }
}

/// P(|N(0, 1)| ≥ z), from the tail itself: `2·(1 − Φ(z))` loses every digit once Φ(z) rounds to 1.
pub(crate) fn two_tail(z: f64) -> f64 {
    erfc(z / std::f64::consts::SQRT_2)
}

/// The chance that a series with no true difference crosses ±`c` (in standard-normal units) at any
/// of the analyses at `pairs`, the information growing with the pairs (a Brownian motion seen at
/// those times). Summed crossing by crossing (Armitage, McPherson and Rowe), each the tail mass past
/// the boundary of the paths that had not crossed before, so a small answer keeps its digits.
fn crossing(c: f64, pairs: &[usize]) -> f64 {
    let last = *pairs.last().unwrap_or(&1) as f64;
    let t: Vec<f64> = pairs.iter().map(|&n| n as f64 / last).collect();
    // Simpson's rule on an odd grid: the step stays under a tenth of the smallest step's spread
    // (√(1/40) for five looks), so the sums are good to about 1e-7.
    const N: usize = 161;
    let simpson = |i: usize| {
        if i == 0 || i + 1 == N {
            1.0 / 3.0
        } else if i % 2 == 1 {
            4.0 / 3.0
        } else {
            2.0 / 3.0
        }
    };
    // The paths that have not crossed yet, as a density over B(t_k) on a grid of the continuation interval.
    let mut grid: Vec<f64> = Vec::new();
    let mut dens: Vec<f64> = Vec::new();
    let mut total = 0.0;
    for k in 0..t.len() {
        let dt = if k == 0 { t[0] } else { t[k] - t[k - 1] };
        if dt <= 0.0 {
            continue;
        }
        let sd = dt.sqrt();
        let edge = c * t[k].sqrt();
        // Crossing now: the mass of the surviving paths that lands beyond ±edge.
        total += if k == 0 || grid.is_empty() {
            two_tail(edge / sd)
        } else {
            let h = grid[1] - grid[0];
            grid.iter()
                .zip(&dens)
                .enumerate()
                .map(|(i, (s, f))| simpson(i) * h * f * 0.5 * (erfc((edge - s) / (sd * std::f64::consts::SQRT_2)) + erfc((edge + s) / (sd * std::f64::consts::SQRT_2))))
                .sum::<f64>()
        };
        // The survivors' density at this analysis, for the next.
        let next: Vec<f64> = (0..N).map(|i| -edge + 2.0 * edge * i as f64 / (N - 1) as f64).collect();
        let phi = |x: f64| (-0.5 * x * x).exp() / (2.0 * std::f64::consts::PI).sqrt();
        let nd: Vec<f64> = if k == 0 || grid.is_empty() {
            next.iter().map(|y| phi(y / sd) / sd).collect()
        } else {
            let h = grid[1] - grid[0];
            next.iter().map(|y| grid.iter().zip(&dens).enumerate().map(|(i, (s, f))| simpson(i) * h * f * phi((y - s) / sd) / sd).sum()).collect()
        };
        grid = next;
        dens = nd;
    }
    total.min(1.0)
}

/// The repeated-confidence-interval p-value of a sequential series (Jennison and Turnbull): the
/// smallest overall level α at which its Pocock interval at the analysis where it stopped excludes
/// 0. `d` holds its pairs' differences (log ratios or differences); `looks` the pairs at which the
/// design checks (one look for `--pairs N`). The nominal two-sided signed-rank p at the stop is the
/// per-look level of some Pocock boundary c; α is the chance of crossing that boundary at any of
/// the planned looks, with the stop counted as a look of its own when it is not one (a series run on
/// past a look for its secondaries or its family). One look: α is the nominal p.
pub fn repeated_p(d: &[f64], looks: &[usize]) -> f64 {
    let n = d.iter().filter(|x| x.is_finite()).count();
    repeated_p_at(wilcoxon_p(d), n, looks)
}

/// `repeated_p` from the nominal p at `n` pairs.
pub(crate) fn repeated_p_at(nominal: f64, n: usize, looks: &[usize]) -> f64 {
    if n == 0 || !nominal.is_finite() {
        return 1.0;
    }
    if nominal >= 1.0 {
        return 1.0;
    }
    let mut at: Vec<usize> = looks.iter().copied().filter(|&l| l > 0).collect();
    at.push(n);
    at.sort_unstable();
    at.dedup();
    if at.len() == 1 {
        return nominal;
    }
    // From the lower tail: 1 − p/2 rounds to 1 once p is tiny (n ≥ 54 pairs all one way).
    let c = -normal_quantile(nominal / 2.0);
    crossing(c, &at).max(nominal).min(1.0)
}

/// The fewest pairs at which a series whose every pair went one way (the strongest result its
/// signed-rank test can give) clears the first step of Holm across `journeys` series at 5%, on its
/// repeated p (`repeated_p`): a series decided at an early look runs on to these pairs so a family
/// claim ("better in K of M") can count it. 8 of 8 pairs (p 0.0078 at the first of five looks)
/// clears Holm only for one series.
pub fn family_min_pairs(journeys: usize, looks: &[usize]) -> usize {
    let first = looks.first().copied().unwrap_or(2);
    (first..=60).find(|&n| repeated_p_at(2.0 / 2f64.powi(n as i32), n, looks) <= 0.05 / journeys.max(1) as f64).unwrap_or(60)
}

/// One journey of a family, as the scoreboard reads it.
#[derive(Clone, Copy, Debug)]
pub struct Journey {
    /// The journey's own primary verdict, unchanged by the family.
    pub kind: VerdictKind,
    /// Its repeated p (`repeated_p`).
    pub p: f64,
    /// A parity gap: one implementation could not run it, so it was not compared.
    pub gap: bool,
}

/// What a family of journeys supports (SPEC §9.3). A journey decided better or worse on its own
/// counts that way only when its repeated p also clears Holm across the compared journeys at 5%, the
/// same rule both ways, so swapping the sides swaps `better` and `worse`; one that does not clear it
/// counts as `unclear` (`better_alone`, `worse_alone`), and reports still list an unconfirmed
/// regression as one. A single compared journey has no family: its own verdict stands. `same` counts
/// `same` and `no worse`. `all_no_worse` (non-inferiority) is an intersection–union claim: every
/// compared journey `no worse` at its own 2.5% one-sided, which needs no multiplicity correction
/// (Berger 1982); a gap leaves it standing over the journeys that were compared, and names itself
/// beside it. `better + same + worse + unclear == compared`.
#[derive(Clone, Debug, Serialize)]
pub struct Family {
    pub journeys: usize,
    pub compared: usize,
    pub better: usize,
    pub same: usize,
    pub worse: usize,
    pub unclear: usize,
    pub gaps: usize,
    /// Better on their own, not after Holm (counted in `unclear`).
    pub better_alone: usize,
    /// Worse on their own, not after Holm (counted in `unclear`).
    pub worse_alone: usize,
    pub family: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub all_no_worse: Option<bool>,
    /// Per journey, in order: its better or worse verdict confirmed across the family.
    #[serde(skip)]
    pub holm: Vec<bool>,
}

pub fn family(js: &[Journey], noworse: bool) -> Family {
    let compared: Vec<usize> = (0..js.len()).filter(|&i| !js[i].gap).collect();
    let ps: Vec<f64> = compared.iter().map(|&i| if js[i].p.is_finite() { js[i].p } else { 1.0 }).collect();
    let rejected = holm(&ps, 0.05);
    let directional = |i: usize| matches!(js[i].kind, VerdictKind::Better | VerdictKind::Worse);
    let mut confirmed = vec![false; js.len()];
    for (&i, r) in compared.iter().zip(rejected) {
        confirmed[i] = directional(i) && (r || compared.len() == 1);
    }
    let count = |f: &dyn Fn(usize) -> bool| compared.iter().filter(|&&i| f(i)).count();
    let better = count(&|i| confirmed[i] && js[i].kind == VerdictKind::Better);
    let worse = count(&|i| confirmed[i] && js[i].kind == VerdictKind::Worse);
    let better_alone = count(&|i| !confirmed[i] && js[i].kind == VerdictKind::Better);
    let worse_alone = count(&|i| !confirmed[i] && js[i].kind == VerdictKind::Worse);
    let same = count(&|i| matches!(js[i].kind, VerdictKind::Same | VerdictKind::NoWorse));
    Family {
        journeys: js.len(),
        compared: compared.len(),
        better,
        same,
        worse,
        unclear: compared.len() - better - same - worse,
        gaps: js.len() - compared.len(),
        better_alone,
        worse_alone,
        family: "holm",
        all_no_worse: noworse.then(|| !compared.is_empty() && compared.iter().all(|&i| js[i].kind == VerdictKind::NoWorse)),
        holm: confirmed,
    }
}

/// mValue (Brendan Gregg), as BenchmarkDotNet uses it: above 3.2 the values likely have several modes.
pub fn mvalue(values: &[f64]) -> Option<f64> {
    let v: Vec<f64> = values.iter().copied().filter(|x| x.is_finite()).collect();
    let n = v.len();
    if n < 10 {
        return None;
    }
    let mean = v.iter().sum::<f64>() / n as f64;
    let sd = (v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1) as f64).sqrt();
    if sd == 0.0 {
        return Some(2.0);
    }
    let width = 3.49 * sd * (n as f64).powf(-1.0 / 3.0);
    let min = v.iter().copied().fold(f64::INFINITY, f64::min);
    let max = v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let bins = (((max - min) / width).ceil() as usize).max(1);
    let mut counts = vec![0usize; bins + 2];
    for x in &v {
        let b = (((x - min) / width).floor() as usize).min(bins - 1);
        counts[b + 1] += 1;
    }
    let peak = *counts.iter().max().unwrap_or(&1) as f64;
    let sum: f64 = counts.windows(2).map(|w| (w[1] as f64 - w[0] as f64).abs()).sum();
    Some(sum / peak)
}

/// The comparison of one metric between base and head over pairs.
#[derive(Clone, Debug, Serialize)]
pub struct Comparison {
    pub metric: String,
    pub n: usize,
    pub base: f64,
    pub head: f64,
    /// Relative change for ratio metrics, absolute difference for counts.
    pub estimate: f64,
    pub lo: f64,
    pub hi: f64,
    pub relative: bool,
    pub confidence: f64,
    pub p: f64,
    pub bound: f64,
    /// The direction this comparison counted as better: the metric's, or for a `metric.<name>` the one
    /// its command's lines gave. Readers take it from here (`better_in`), not from the catalog.
    pub better: Better,
    /// The words for the better and worse directions it used (`faster`, `slower`; `more`, `less` for a
    /// `metric.<name>` better higher), as `words_in` reads them.
    pub words: (&'static str, &'static str),
    /// What it decided, in words for people (`faster (small)`, `refused (TailTooShort): …`).
    /// `set_verdict` and `refuse` change it together with `kind` and `refusal`.
    pub verdict: String,
    /// What it decided, for code and result.json readers.
    #[serde(rename = "verdict_kind")]
    pub kind: VerdictKind,
    pub decided: bool,
    pub needed_pairs: Option<usize>,
    /// A counter that held one value on each side in every block: compared exactly (SPEC §10.1).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub exact: bool,
    /// Non-inferiority (SPEC §9.3): the margin by which head may be worse (a fraction for ratio metrics,
    /// the metric's unit otherwise); `p` then tests the difference against the margin.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub noworse: Option<f64>,
    /// The validator that refused this metric (`kind` is `Refused`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<Refusal>,
}

impl Comparison {
    /// Says what it decided, in words and as a kind at once (a gate, a stop, a descriptive metric).
    pub fn set_verdict(&mut self, kind: VerdictKind, verdict: impl Into<String>) {
        self.kind = kind;
        self.verdict = verdict.into();
        self.refusal = None;
    }

    /// Refuses the metric: `refused (<validator>): <summary>`, undecided, with no numbers (they print —):
    /// a refused comparison's values are not a measurement anyone should read.
    pub fn refuse(&mut self, r: Refusal) {
        (self.base, self.head, self.estimate, self.lo, self.hi) = (f64::NAN, f64::NAN, f64::NAN, f64::NAN, f64::NAN);
        // Nor its p: a refused metric is tested by no one (it never joins a Holm family).
        self.p = f64::NAN;
        self.needed_pairs = None;
        self.verdict = format!("refused ({}): {}", r.validator, r.summary);
        self.kind = VerdictKind::Refused;
        self.decided = false;
        self.refusal = Some(r);
    }
}

/// The direction a comparison as result.json keeps it counted as better: its own `better` (a
/// `metric.<name>` whose command's lines said `higher` is better higher, whatever the catalog says);
/// in files from before that field, what its `verdict_kind` and interval say together (`better` with
/// an interval below 0 is lower-is-better, above 0 higher-is-better; `worse` the other way round);
/// else the catalog's.
pub fn better_in(c: &serde_json::Value) -> Better {
    match c["better"].as_str() {
        Some("higher") => return Better::Higher,
        Some("lower") => return Better::Lower,
        _ => {}
    }
    let (lo, hi) = (c["lo"].as_f64().unwrap_or(f64::NAN), c["hi"].as_f64().unwrap_or(f64::NAN));
    let kind = c.get("verdict_kind").and_then(|k| serde_json::from_value::<VerdictKind>(k.clone()).ok());
    if matches!(kind, Some(VerdictKind::Better | VerdictKind::Worse)) && c["noworse"].is_null() && (hi < 0.0 || lo > 0.0) {
        return if (kind == Some(VerdictKind::Better)) == (hi < 0.0) { Better::Lower } else { Better::Higher };
    }
    crate::model::metric(c["metric"].as_str().unwrap_or("")).map(|m| m.better).unwrap_or(Better::Lower)
}

/// The definition a comparison as result.json was made under: a `metric.<name>` better higher
/// (`better_in`) is `METRIC_HIGHER`, every other the catalog's (`model::metric`).
pub fn def_in(c: &serde_json::Value) -> Option<&'static MetricDef> {
    let name = c["metric"].as_str().unwrap_or("");
    let def = crate::model::metric(name);
    if name.starts_with("metric.") && better_in(c) == Better::Higher { Some(&crate::model::METRIC_HIGHER) } else { def }
}

/// The words for comparison `c`'s better and worse directions (`faster`, `slower`): its own `words`, else
/// its definition's (`def_in`), else `less`/`more` turned by `better_in`.
pub fn words_in(c: &serde_json::Value) -> (String, String) {
    if let Some([a, b]) = c["words"].as_array().map(Vec::as_slice)
        && let (Some(a), Some(b)) = (a.as_str(), b.as_str())
    {
        return (a.to_string(), b.to_string());
    }
    match def_in(c) {
        Some(d) => (d.words.0.to_string(), d.words.1.to_string()),
        None if better_in(c) == Better::Higher => ("more".into(), "less".into()),
        None => ("less".into(), "more".into()),
    }
}

/// What a verdict decided, from its words, for result.json files from before `verdict_kind`: new
/// comparisons carry their kind. `def` is the metric's (or its family's) definition, whose words
/// say which direction is better.
pub fn verdict_kind(verdict: &str, def: Option<&MetricDef>) -> VerdictKind {
    let v = verdict.trim();
    let first = v.split([' ', ':', ',']).next().unwrap_or("");
    match first {
        "—" => VerdictKind::Untested,
        "·" => VerdictKind::Descriptive,
        "refused" => VerdictKind::Refused,
        "no" if v.starts_with("no worse") => VerdictKind::NoWorse,
        "worse" => VerdictKind::Worse,
        "same" => VerdictKind::Same,
        "underpowered" => VerdictKind::Underpowered,
        "saturated" => VerdictKind::Saturated,
        "stopped" => VerdictKind::Stopped,
        // A leak run's growth (`leaks 120 KB/run`).
        "leaks" => VerdictKind::Worse,
        w if def.is_some_and(|d| w == d.words.0) => VerdictKind::Better,
        w if def.is_some_and(|d| w == d.words.1) => VerdictKind::Worse,
        _ => VerdictKind::Unclear,
    }
}

/// What a comparison as result.json keeps it decided: its `verdict_kind`, or, in files from before
/// that field, what its words say.
pub fn kind_of(c: &serde_json::Value) -> VerdictKind {
    c.get("verdict_kind").and_then(|k| serde_json::from_value(k.clone()).ok()).unwrap_or_else(|| verdict_kind(c["verdict"].as_str().unwrap_or(""), def_in(c)))
}

/// The refusal a `refused (<validator>): <why>` verdict names, for result.json files from before
/// `refusal`.
pub fn refusal_of(verdict: &str) -> Option<Refusal> {
    let rest = verdict.trim().strip_prefix("refused (")?;
    let (id, why) = rest.split_once(')')?;
    Some(Refusal::new(ValidatorId::from_name(id)?, why.trim_start_matches(':').trim()))
}

/// Per-pair differences of a metric: `ln(head / base)` when every value is positive and the metric is a
/// ratio metric, else `head − base`. Returns the finite pairs, the differences and whether they are ratios.
fn diffs(def: &MetricDef, pairs: &[(f64, f64)]) -> (Vec<(f64, f64)>, Vec<f64>, bool) {
    let finite: Vec<(f64, f64)> = pairs.iter().copied().filter(|(a, b)| a.is_finite() && b.is_finite()).collect();
    // Counts compare by difference: a zero on either side (no fetches, no shift) has no ratio.
    let relative = def.kind != Kind::Exact && def.unit != "count" && def.unit != "score" && finite.iter().all(|(a, b)| *a > 0.0 && *b > 0.0);
    let d = finite.iter().map(|(a, b)| if relative { (b / a).ln() } else { b - a }).collect();
    (finite, d, relative)
}

/// The margin as a difference in `diffs`' scale: where "worse by exactly the margin" sits. `margin_abs`
/// is the margin in the comparison's scale (a fraction for ratios, the unit otherwise).
pub fn margin_shift(def: &MetricDef, relative: bool, margin_abs: f64) -> f64 {
    let worse = if def.better == Better::Lower { 1.0 } else { -1.0 };
    if relative { (1.0 + worse * margin_abs).max(1e-9).ln() } else { worse * margin_abs }
}

/// Non-inferiority (SPEC §9.3): is head at most `margin` worse than base? The same interval as `compare`
/// (at `confidence`, a planned look's level for a primary), judged against the margin instead of 0:
/// `no worse (≤X)` when its worse end lies inside the margin, `worse (>X)` when its better end lies
/// beyond it, else `unclear`. Each claim is a one-sided test at (1 − confidence)/2; `p` is the two-sided
/// signed-rank test of the differences against the margin, so Holm treats it like any other secondary.
/// A `no worse` whose interval also excludes 0 says which way (`, faster` or `, but slower`).
pub fn compare_noworse(name: &str, def: &MetricDef, pairs: &[(f64, f64)], margin: f64, confidence: f64) -> Comparison {
    let mut c = compare(name, def, pairs, margin, confidence);
    let (_, d, relative) = diffs(def, pairs);
    let margin_abs = if relative { margin } else { in_unit(def, margin, c.base, c.head) };
    let label = fmt_bound(def, margin, relative, margin_abs);
    let good_low = def.better == Better::Lower;
    c.noworse = Some(margin_abs);
    c.bound = margin_abs;
    // Oriented so that larger is worse.
    let (w_est, w_lo, w_hi) = if good_low { (c.estimate, c.lo, c.hi) } else { (-c.estimate, -c.hi, -c.lo) };
    if c.exact {
        if w_est <= margin_abs {
            c.set_verdict(VerdictKind::NoWorse, format!("no worse (≤{label}) (exact)"));
        } else {
            c.set_verdict(VerdictKind::Worse, format!("worse (>{label}) (exact)"));
        }
        c.p = 0.0;
        c.decided = true;
        return c;
    }
    let shift = margin_shift(def, relative, margin_abs);
    c.p = wilcoxon_p(&d.iter().map(|x| x - shift).collect::<Vec<_>>());
    let covered = c.confidence + 1e-9 >= confidence;
    let n = d.len();
    let (kind, verdict) = if n < 2 || !w_est.is_finite() || !covered {
        (VerdictKind::Unclear, "unclear".to_string())
    } else if w_hi < margin_abs {
        let tag = if w_hi < 0.0 {
            format!(", {}", def.words.0)
        } else if w_lo > 0.0 {
            format!(", but {}", def.words.1)
        } else {
            String::new()
        };
        (VerdictKind::NoWorse, format!("no worse (≤{label}{tag})"))
    } else if w_lo > margin_abs {
        (VerdictKind::Worse, format!("worse (>{label})"))
    } else {
        (VerdictKind::Unclear, "unclear".to_string())
    };
    let decided = kind.decided();
    c.set_verdict(kind, verdict);
    c.decided = decided;
    c.needed_pairs = (!decided && n >= 2).then(|| {
        let hw = (c.hi - c.lo) / 2.0;
        let dist = (margin_abs - w_est).abs();
        let estimate = if dist <= 0.0 || hw <= 0.0 { usize::MAX } else { ((n as f64) * (hw / dist).powi(2)).ceil() as usize };
        estimate.max(min_pairs(confidence))
    });
    c
}

/// `pairs_needed` for a non-inferiority comparison: the distance that decides is the one from the
/// differences' median to the margin (`shift`, in the differences' scale), either way.
pub fn pairs_needed_noworse(d: &[f64], shift: f64, confidence: f64, power: f64) -> usize {
    let v: Vec<f64> = d.iter().copied().filter(|x| x.is_finite()).collect();
    if v.len() < 2 {
        return usize::MAX;
    }
    let dist = (crate::metrics::median(&v) - shift).abs();
    if dist <= 0.0 {
        return usize::MAX;
    }
    pairs_for(spread(&v), dist, confidence, power).ceil() as usize
}

/// A robust standard deviation: 1.4826 × the median absolute deviation from the median.
pub fn robust_sd(v: &[f64]) -> f64 {
    let med = crate::metrics::median(v);
    1.4826 * crate::metrics::median(&v.iter().map(|x| (x - med).abs()).collect::<Vec<_>>())
}

/// The spread a projection of pairs uses: `robust_sd`, or where over half the differences are one
/// value (a count or a quantized time: its MAD is 0) the mean absolute deviation from the median
/// × √(π/2) (the normal's σ): a MAD of 0 once projected 7 pairs for any series, so a series whose
/// other half spread wide never stopped as underpowered.
pub fn spread(v: &[f64]) -> f64 {
    let r = robust_sd(v);
    if r > 0.0 || v.is_empty() {
        return r;
    }
    let med = crate::metrics::median(v);
    1.2533 * v.iter().map(|x| (x - med).abs()).sum::<f64>() / v.len() as f64
}

/// Planning (SPEC §9.1 `plan`): pairs for a `power` chance that an interval at `confidence` clears a
/// boundary `dist` away from the true difference, for per-pair differences with spread `sigma` (same
/// scale). Normal approximation with the Hodges–Lehmann efficiency 0.955; at least the pairs whose
/// interval can reach `confidence` at all (`min_pairs`: 6 at 95%, 7 at a Pocock look's 98.42%).
pub fn pairs_for(sigma: f64, dist: f64, confidence: f64, power: f64) -> f64 {
    if dist <= 0.0 || !sigma.is_finite() {
        return f64::INFINITY;
    }
    let z = normal_quantile(1.0 - (1.0 - confidence) / 2.0) + normal_quantile(power);
    ((z * sigma / dist).powi(2) / 0.955).max(min_pairs(confidence) as f64)
}

/// The smallest distance from a boundary that `n` pairs clear with `power` (the minimum detectable effect).
pub fn detectable(sigma: f64, n: usize, confidence: f64, power: f64) -> f64 {
    let z = normal_quantile(1.0 - (1.0 - confidence) / 2.0) + normal_quantile(power);
    z * sigma / (0.955 * n as f64).sqrt()
}

/// Compares paired block values of metric `name` (`def` is its definition, or its family's).
/// `pairs` holds (base, head) per pair.
pub fn compare(name: &str, def: &MetricDef, pairs: &[(f64, f64)], bound: f64, confidence: f64) -> Comparison {
    let (finite, d, relative) = diffs(def, pairs);
    let base = crate::metrics::median(&finite.iter().map(|p| p.0).collect::<Vec<_>>());
    let head = crate::metrics::median(&finite.iter().map(|p| p.1).collect::<Vec<_>>());
    // An exact counter that is stable on both sides (every block the same, from `EXACT_MIN_PAIRS`) has
    // no noise to test: its difference is the answer. One that varies is compared like a timed metric.
    if def.kind == Kind::Exact && finite.len() >= EXACT_MIN_PAIRS && finite.iter().all(|p| p.0 == finite[0].0 && p.1 == finite[0].1) {
        let diff = head - base;
        let improved = (diff < 0.0) == (def.better == Better::Lower);
        let (kind, verdict) = match (diff == 0.0, improved) {
            (true, _) => (VerdictKind::Same, "same (exact)".to_string()),
            (false, true) => (VerdictKind::Better, format!("{} (exact)", def.words.0)),
            (false, false) => (VerdictKind::Worse, format!("{} (exact)", def.words.1)),
        };
        let p = if diff == 0.0 { 1.0 } else { 0.0 };
        return Comparison {
            metric: name.to_string(),
            n: finite.len(),
            base,
            head,
            estimate: diff,
            lo: diff,
            hi: diff,
            relative: false,
            confidence: 1.0,
            p,
            bound: 0.0,
            better: def.better,
            words: def.words,
            verdict,
            kind,
            decided: true,
            needed_pairs: None,
            exact: true,
            noworse: None,
            refusal: None,
        };
    }
    let (est, lo, hi, achieved) = hodges_lehmann(&d, confidence);
    let p = wilcoxon_p(&d);
    let (est, lo, hi) = if relative { (est.exp() - 1.0, lo.exp() - 1.0, hi.exp() - 1.0) } else { (est, lo, hi) };
    let bound_abs = if relative { bound } else { in_unit(def, bound, base, head) };
    let good_low = def.better == Better::Lower;
    let n = d.len();
    // Below 6 pairs no signed-rank interval reaches 95%: its [min, max] covers only 1 − 2^(1−n).
    let covered = achieved + 1e-9 >= confidence;
    let (kind, verdict) = if n < 2 || !est.is_finite() || !covered {
        (VerdictKind::Unclear, "unclear".to_string())
    } else if hi < 0.0 || lo > 0.0 {
        let improved = (hi < 0.0) == good_low;
        let (kind, word) = if improved { (VerdictKind::Better, def.words.0) } else { (VerdictKind::Worse, def.words.1) };
        let small = if hi < 0.0 { hi > -bound_abs } else { lo < bound_abs };
        (kind, if small { format!("{word} (small)") } else { word.to_string() })
    } else if (lo > -bound_abs && hi < bound_abs) || (lo == 0.0 && hi == 0.0) {
        (VerdictKind::Same, format!("same (±{})", fmt_bound(def, bound, relative, bound_abs)))
    } else {
        (VerdictKind::Unclear, "unclear".to_string())
    };
    let decided = kind.decided();
    let needed_pairs = (!decided && n >= 2).then(|| {
        let hw = (hi - lo) / 2.0;
        let dist_diff = est.abs();
        let dist_same = bound_abs - est.abs();
        let dist = dist_diff.max(dist_same);
        let estimate = if dist <= 0.0 || hw <= 0.0 { usize::MAX } else { ((n as f64) * (hw / dist).powi(2)).ceil() as usize };
        estimate.max(min_pairs(confidence))
    });
    Comparison {
        metric: name.to_string(),
        n,
        base,
        head,
        estimate: est,
        lo,
        hi,
        relative,
        confidence: achieved,
        p,
        bound: bound_abs,
        better: def.better,
        words: def.words,
        verdict,
        kind,
        decided,
        needed_pairs,
        exact: false,
        noworse: None,
        refusal: None,
    }
}

/// Pairs a verdict needs, projected from a robust spread of the differences so far (`spread`, HL
/// efficiency 0.955) for a `power` chance that the interval clears its boundary (`pairs_for`): a
/// small-n interval is [min, max] of Walsh averages, so one odd pair would make a width-based
/// projection give up early. `d` and `bound` are in the comparison's scale (log ratios for ratio
/// metrics). The bench's futility rule asks for the even chance (`FUTILITY_POWER`: give up only when
/// even the likely case needs more than the last look); what it prints as needed, and `plan`, the
/// 80% chance (`PLAN_POWER`), so the two never disagree by the factor (z + z₀.₈)²/z² ≈ 1.8.
pub fn pairs_needed(d: &[f64], bound: f64, confidence: f64, power: f64) -> usize {
    let v: Vec<f64> = d.iter().copied().filter(|x| x.is_finite()).collect();
    if v.len() < 2 {
        return usize::MAX;
    }
    let med = crate::metrics::median(&v);
    let dist = med.abs().max(bound - med.abs());
    if dist <= 0.0 {
        return usize::MAX;
    }
    pairs_for(spread(&v), dist, confidence, power).ceil() as usize
}

/// The chance a bench's futility projection asks for (`pairs_needed`): the likely case.
pub const FUTILITY_POWER: f64 = 0.5;
/// The chance `plan` and a verdict's "pairs needed" ask for (`pairs_needed`, `pairs_for`).
pub const PLAN_POWER: f64 = 0.8;

/// The fewest pairs over which an exact counter must hold one value on each side to be compared
/// exactly: the fewest at which any metric gets a verdict (6, `min_pairs(0.95)`), and more than the 3
/// an A/A needs to call a counter stable (SPEC §10.4). Two pairs once decided `more (exact)` in a
/// `--pairs 2` smoke run, which promises no verdicts, from a counter that had not shown it holds still.
pub const EXACT_MIN_PAIRS: usize = 6;

/// The fewest pairs whose signed-rank interval can reach `confidence` (6 at 95%).
pub fn min_pairs(confidence: f64) -> usize {
    (1..=50).find(|&n| 1.0 - 2f64.powi(1 - n as i32) >= confidence).unwrap_or(50)
}

fn fmt_bound(def: &MetricDef, bound: f64, relative: bool, abs: f64) -> String {
    crate::fmt::bound(bound, relative, abs, def.unit)
}

/// A bound or margin `fraction` in the metric's own unit, for a metric compared by difference: the
/// fraction of base's median (of head's when base's is 0, so a score that went from 0 to 0.3 is not
/// within a bound of 0). Only a count (a `count` unit or an exact counter) is floored at half a count:
/// a score or a duration has no smallest step, and a floor of 0.5 once made a layout shift of 0 → 0.30
/// `same (±0.5)`.
pub fn in_unit(def: &MetricDef, fraction: f64, base: f64, head: f64) -> f64 {
    let scale = if base.is_finite() && base != 0.0 { base.abs() } else { head.abs() };
    let abs = fraction * if scale.is_finite() { scale } else { 0.0 };
    if def.unit == "count" || def.kind == Kind::Exact { abs.max(0.5) } else { abs }
}
