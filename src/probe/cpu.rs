//! V8 CPU profiles (attribution blocks only), with line ticks.

use crate::model::{Error, Refusal, Result, ValidatorId};
use crate::target::chromium::Page;
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};

/// Sampling interval (µs) on macOS and Linux, and what every recording before 2026-11-07 asked for.
/// At 50 µs the profiler itself raised a JS-heavy run's main-thread work by 35–45%, enough that one
/// profiled launch could move against the timed pairs it was meant to explain; 200 µs still gives
/// 5,000 samples a second.
pub const INTERVAL_US: u64 = 200;

/// The interval strobe asks a V8 sampler for: 400 µs on Windows, else `INTERVAL_US`. Windows' timers
/// go no finer than about 0.5 ms, and V8's sampler there wakes on the timer tick after its interval:
/// Chromium on adais kept 0.52 ms when asked for 200 to 450 µs, 1.0 ms when asked for 500 µs and
/// 1.5 ms for 1 ms. 400 µs keeps the finest it can without asking for much more than it gives.
pub fn interval_us() -> u64 {
    if cfg!(windows) { 400 } else { INTERVAL_US }
}

/// A gap longer than this is no sampling interval: the thread was not sampled (an idle runtime that
/// skips ticks, a process that paused), as Firefox's `MAX_GAP_NS`.
pub const MAX_GAP_US: f64 = 50_000.0;

pub fn start(page: &Page) -> Result<()> {
    page.send("Profiler.enable", json!({}))?;
    page.send("Profiler.setSamplingInterval", json!({ "interval": interval_us() }))?;
    page.send("Profiler.start", json!({}))?;
    Ok(())
}

/// Stops the profiler and returns the `.cpuprofile` JSON.
pub fn stop(page: &Page) -> Result<Value> {
    let r = page.send("Profiler.stop", json!({}))?;
    let _ = page.send("Profiler.disable", json!({}));
    Ok(r["profile"].clone())
}

/// The gaps between consecutive samples of a V8 `.cpuprofile` (µs): `timeDeltas[i]` is the gap from
/// sample i − 1 to sample i.
pub fn gaps(profile: &Value) -> Vec<f64> {
    profile["timeDeltas"].as_array().map(|d| d.iter().skip(1).filter_map(Value::as_f64).collect()).unwrap_or_default()
}

/// What a JavaScript sampler was asked for and what it gave: the median gap between consecutive
/// samples of the page's main thread. A sampler need not keep the interval asked of it (Firefox 160
/// and older on Windows sample every 2 ms when asked for 1 ms; Windows' timers go no finer than about
/// 0.5 ms), and a sample weighs the gap it kept, so this is a profile's resolution.
#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct Sampling {
    pub asked_us: f64,
    pub median_us: f64,
    /// The gaps the median is of.
    pub gaps: u64,
}

impl Sampling {
    /// The median of `gaps` by time (µs, each between two consecutive samples; those over 50 ms are
    /// no interval and left out): the gap half the sampled time lies in gaps up to, which a regular
    /// sampler's plain median equals. By count, bursts of extra samples decide it: a V8 profile on
    /// Windows asked 200 µs held 915,000 gaps of 5 µs (11% of its time) beside 60,000 of 0.3-1 ms
    /// (78%), and read `sampled every 0.01 ms` where its time was sampled every 0.53 ms. None without
    /// a gap.
    pub fn of(asked_us: f64, mut gaps: Vec<f64>) -> Option<Sampling> {
        gaps.retain(|g| *g > 0.0 && *g <= MAX_GAP_US);
        if gaps.is_empty() {
            return None;
        }
        gaps.sort_by(f64::total_cmp);
        let half = gaps.iter().sum::<f64>() / 2.0;
        let mut acc = 0.0;
        let median_us = gaps
            .iter()
            .find(|g| {
                acc += **g;
                acc >= half
            })
            .copied()
            .unwrap_or(gaps[gaps.len() - 1]);
        Some(Sampling { asked_us, median_us, gaps: gaps.len() as u64 })
    }

    /// Coarser than asked: by more than half the interval asked (timer slack is a few per cent).
    pub fn coarse(&self) -> bool {
        self.median_us > 1.5 * self.asked_us
    }

    /// `sampled every 0.52 ms (asked 0.5 ms)`, `sampled every 2.0 ms (asked 1 ms: 2.0× coarser)`.
    pub fn text(&self) -> String {
        let got = if self.median_us >= 1000.0 { format!("{:.1} ms", self.median_us / 1000.0) } else { format!("{:.2} ms", self.median_us / 1000.0) };
        let coarser = if self.coarse() { format!(": {:.1}× coarser", self.median_us / self.asked_us) } else { String::new() };
        format!("sampled every {got} (asked {} ms{coarser})", self.asked_us / 1000.0)
    }
}

/// A script host's runtime (SPEC §9.6), and how its CPU profile (`--cpu-prof`) keeps time against the
/// monotonic clock `host.mjs` reads. Node's profile counts in that clock (`process.hrtime`, V8's
/// `TimeTicks`), its gaps exact, and V8 samples an idle thread as `(idle)`. Bun's counts in wall-clock
/// µs: each sample's monotonic time made wall time as the host exits and writes it (JSC's
/// `approximateWallTime`), so offset by how far the wall clock led the monotonic one then, and each gap
/// truncated to whole µs while the time it is taken from moves on exactly (Bun 1.4,
/// `BunCPUProfiler.cpp`): 0.5 µs a sample on vps, which put a run's samples 0.8 ms early after 20 s
/// there, 0.37 µs on adais; its `endTime`, the last sample's time untruncated, says how much in all. Bun's sampler takes no sample while the VM runs no JavaScript: a 1 s sleep is one 1 s
/// gap, where V8 samples `(idle)` every interval.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Runtime {
    Bun,
    Node,
}

impl Runtime {
    pub fn of(name: &str) -> Option<Runtime> {
        match name {
            "bun" => Some(Runtime::Bun),
            "node" => Some(Runtime::Node),
            _ => None,
        }
    }
}

/// Over this share of a script host's main-thread CPU in its measured runs with no sample of it
/// (`missed` in meta.json), the recording and every hotspots view say its rows undercount.
pub const MISSED_SHARE: f64 = 0.05;

/// A script recording's `missed` (meta.json): the measured runs' main-thread CPU (`cpu.main`, ms) the
/// profile holds no sample of: that CPU minus the samples' weight there (`Rebased::sampled_in`), at
/// least 0, and its share of that CPU.
pub fn missed(main_ms: f64, sampled_ms: f64) -> Value {
    let ms = (main_ms - sampled_ms).max(0.0);
    json!({ "ms": ms, "share": if main_ms > 0.0 { ms / main_ms } else { 0.0 }, "main_ms": main_ms, "sampled_ms": sampled_ms })
}

/// What a script recording's `missed` costs its rows, when over `MISSED_SHARE`; `runtime` is its
/// runtime's name (`meta.runtime.name`), which says why.
pub fn missed_text(missed: &Value, runtime: Option<&str>) -> Option<String> {
    let share = missed["share"].as_f64().filter(|s| *s > MISSED_SHARE)?;
    let why = match runtime {
        Some("node") => "Node's profiler labels most of a short timer callback (idle), so timer-driven work undercounts",
        Some("bun") => "Bun's sampler takes a sample only while JavaScript runs, never of its event loop's own work (timers, I/O)",
        _ => "the runtime's sampler took no sample of it",
    };
    Some(format!(
        "Missed: the main thread ran {:.0} ms on a CPU in the measured runs (cpu.main) and the profile holds samples of {:.0} ms of it: {:.0} ms ({:.0}%) has no sample, so rows undercount. {why}",
        missed["main_ms"].as_f64().unwrap_or(0.0),
        missed["sampled_ms"].as_f64().unwrap_or(0.0),
        missed["ms"].as_f64().unwrap_or(0.0),
        100.0 * share
    ))
}

/// A script recording's whole host profile, on strobe's clock (`Rebased::whole`).
pub const WHOLE_HOST: &str = "whole-host.json.gz";

/// A script recording's measured runs: spans, untimed stretches and phase marks on strobe's clock.
pub const RUNS: &str = "runs.json";

/// The longest round trip a clock reading may take: a reading is the trip's midpoint, off by at most
/// half of it, and samples are placed within 1 ms (SPEC §6) or not at all.
const MAX_RTT_NS: u64 = 2_000_000;

/// How far apart two placements of one moment may be (SPEC §6).
const SYNC_NS: f64 = 1_000_000.0;

/// The function `host.mjs` spins in while it reads its clocks: its samples are where the profile puts
/// a span strobe timed.
const MARK: &str = "strobeClockMark";

/// One reading of a script host's clocks against strobe's (`host::now_ns`): its `clock` op, just after
/// it is ready and again before it exits.
#[derive(Clone, Copy, Debug)]
pub struct Sync {
    /// strobe's clock (ns) minus the runtime's monotonic one (µs × 1000), at the midpoint of the
    /// quickest of a few round trips, and that trip (ns): the reading is off by at most half of it.
    pub offset_ns: f64,
    pub rtt_ns: u64,
    /// The runtime's wall clock minus its monotonic one (µs), at a `Date.now()` millisecond edge.
    pub wall_lead_us: f64,
    /// strobe's clock around the request in which `strobeClockMark` spun and read that edge.
    pub mark: (u64, u64),
}

impl Sync {
    /// From the marked request (strobe's clock around it, and its answer) and quick round trips
    /// (strobe's clock around each, and the runtime's monotonic µs it answered).
    pub fn of(mark: (u64, u64, &Value), trips: &[(u64, u64, f64)]) -> Result<Sync> {
        let best = trips.iter().filter(|t| t.2.is_finite() && t.1 >= t.0).min_by_key(|t| t.1 - t.0).ok_or_else(|| Error::msg("the script host gave no clock reading"))?;
        let (wall, at) = (mark.2["wall_us"].as_f64().unwrap_or(0.0), mark.2["wall_mono_us"].as_f64().unwrap_or(0.0));
        if wall <= 0.0 {
            return Err(Error::msg("the script host read no wall-clock edge"));
        }
        Ok(Sync { offset_ns: (best.0 as f64 + best.1 as f64) / 2.0 - best.2 * 1e3, rtt_ns: best.1 - best.0, wall_lead_us: wall - at, mark: (mark.0, mark.1) })
    }
}

/// A script host's CPU profile with each sample on strobe's clock (µs: `host::now_ns` / 1000).
pub struct Rebased {
    pub start_us: f64,
    pub times_us: Vec<f64>,
    /// What a sample weighs at most (µs): `MAX_GAP_US`; for a sampler that skips an idle VM (Bun) four
    /// of its median gaps while busy (in `strobeClockMark`'s spins), past which the VM was idle, not
    /// running the sampled function (busy gaps on adais: p99.9 1.5× the median).
    pub cap_us: f64,
    /// What a sample before a gap over the cap weighs: the cap (V8), one busy interval (Bun).
    pub over_us: f64,
    /// What the last sample weighs: the median gap under the cap.
    pub last_us: f64,
}

/// Places a script host's CPU profile `p` on strobe's clock from the host's clock readings at its
/// start and before its exit (`Runtime` says how each runtime keeps time). Refuses `ClockUnsynced` a
/// reading that took over 2 ms, a monotonic clock that moved over 1 ms against strobe's between the
/// two, and a profile that puts the samples of either reading's spin more than 1 ms outside strobe's
/// span around it, or has none of them.
pub fn rebase(p: &Value, rt: Runtime, ready: &Sync, exit: &Sync) -> Result<Rebased> {
    let unsynced = |what: String| -> Error { Refusal::new(ValidatorId::ClockUnsynced, format!("the script host's CPU profile: {what}")).fix("run it again on a machine with CPUs to spare").into() };
    let readings = [("at its start", ready), ("before its exit", exit)];
    for (when, s) in readings {
        if s.rtt_ns > MAX_RTT_NS {
            return Err(unsynced(format!(
                "reading the host's clock {when} took {:.2} ms there and back (over {} ms), so strobe knows it only within half that",
                s.rtt_ns as f64 / 1e6,
                MAX_RTT_NS / 1_000_000
            )));
        }
    }
    let drift = exit.offset_ns - ready.offset_ns;
    if drift.abs() > SYNC_NS {
        return Err(unsynced(format!("the runtime's monotonic clock moved {:+.2} ms against strobe's between the host's start and its exit", drift / 1e6)));
    }
    let deltas: Vec<f64> = p["timeDeltas"].as_array().into_iter().flatten().map(|d| d.as_f64().unwrap_or(0.0)).collect();
    let samples = p["samples"].as_array().map_or(0, Vec::len);
    let start = p["startTime"].as_f64().ok_or_else(|| Error::msg("the script host's CPU profile has no startTime"))?;
    // Bun's `endTime` is its last sample's time, not truncated: what the truncated gaps lost in all, spread
    // over the samples (0.37 µs a sample on adais, where gaps are not spread evenly over a µs).
    let truncated = rt == Runtime::Bun && deltas.iter().all(|d| d.fract() == 0.0);
    let lost = match p["endTime"].as_f64() {
        Some(end) if truncated && samples > 0 => Some((end - start - deltas.iter().sum::<f64>()) / samples as f64).filter(|l| (0.0..=1.0).contains(l)).unwrap_or(0.5),
        _ if truncated => 0.5,
        _ => 0.0,
    };
    let lead = if rt == Runtime::Bun { exit.wall_lead_us } else { 0.0 };
    let base = exit.offset_ns / 1e3 - lead;
    let mut t = start;
    let times_us: Vec<f64> = (0..samples)
        .map(|i| {
            t += deltas.get(i).copied().unwrap_or(0.0).max(0.0) + lost;
            t + base
        })
        .collect();
    // Bun read its start as wall time when the profiler started, with the wall clock's lead then.
    let start_us = start - if rt == Runtime::Bun { ready.wall_lead_us } else { 0.0 } + ready.offset_ns / 1e3;
    let gaps: Vec<f64> = times_us.windows(2).map(|w| w[1] - w[0]).filter(|g| *g > 0.0 && *g <= MAX_GAP_US).collect();
    let marked = under(p, MARK);
    // Bun's interval, from the gaps inside `strobeClockMark`'s spins, where the VM is busy throughout:
    // the gaps of the whole profile hold every short await (a handler that works 100 µs every 10 ms
    // has a median gap of 10 ms). Without two marked samples in a row, gaps up to four asked intervals.
    let busy = |mut g: Vec<f64>| {
        g.sort_by(f64::total_cmp);
        g.get(g.len() / 2).copied()
    };
    let spun = times_us.windows(2).zip(marked.windows(2)).filter(|(_, m)| m[0] && m[1]).map(|(t, _)| t[1] - t[0]).filter(|g| *g > 0.0 && *g <= MAX_GAP_US).collect();
    // A Bun gap over the cap is a VM that went idle: its sample stands for one interval, as a sample
    // does (four, the cap, put a 100 µs handler's sample at 1.7 ms). A V8 gap over 50 ms is cut to it.
    let (cap_us, over_us) = match rt {
        Runtime::Bun => busy(spun).or_else(|| busy(gaps.iter().copied().filter(|g| *g <= 4.0 * interval_us() as f64).collect())).map_or((MAX_GAP_US, MAX_GAP_US), |m| ((4.0 * m).min(MAX_GAP_US), m)),
        Runtime::Node => (MAX_GAP_US, MAX_GAP_US),
    };
    let last_us = Sampling::of(0.0, gaps.into_iter().filter(|g| *g <= cap_us).collect()).map_or(0.0, |s| s.median_us);
    // Each spin's samples, by the reading nearer them: inside strobe's span around it, within SYNC_NS.
    let mut seen = [0usize; 2];
    for at in times_us.iter().zip(&marked).filter(|(_, m)| **m).map(|(t, _)| t) {
        let ns = at * 1e3;
        let k = usize::from((ns - exit.mark.0 as f64).abs() < (ns - ready.mark.0 as f64).abs());
        let (a, b) = (readings[k].1.mark.0 as f64, readings[k].1.mark.1 as f64);
        let off = if ns < a { ns - a } else { (ns - b).max(0.0) };
        if off.abs() > SYNC_NS {
            return Err(unsynced(format!(
                "it puts a sample of the 5 ms the host spent reading its clock {} {:+.2} ms outside strobe's span around that: its clock does not match strobe's",
                readings[k].0,
                off / 1e6
            )));
        }
        seen[k] += 1;
    }
    if let Some(k) = seen.iter().position(|n| *n == 0) {
        return Err(unsynced(format!("it holds no sample of the 5 ms the host spent reading its clock {}, so where it puts that moment cannot be checked", readings[k].0)));
    }
    Ok(Rebased { start_us, times_us, cap_us, over_us, last_us })
}

/// Per sample of `p`: whether its stack holds a function named `name`.
fn under(p: &Value, name: &str) -> Vec<bool> {
    let nodes = p["nodes"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let mut parent: HashMap<u64, u64> = HashMap::new();
    for n in nodes {
        let id = n["id"].as_u64().unwrap_or(0);
        for c in n["children"].as_array().into_iter().flatten().filter_map(Value::as_u64) {
            parent.insert(c, id);
        }
    }
    let named: BTreeSet<u64> = nodes.iter().filter(|n| n["callFrame"]["functionName"] == name).filter_map(|n| n["id"].as_u64()).collect();
    let mut memo: HashMap<u64, bool> = HashMap::new();
    let mut hit = |id: u64| -> bool {
        if let Some(&h) = memo.get(&id) {
            return h;
        }
        let mut at = Some(id);
        let mut h = false;
        while let Some(x) = at {
            if named.contains(&x) {
                h = true;
                break;
            }
            at = parent.get(&x).copied();
        }
        memo.insert(id, h);
        h
    };
    p["samples"].as_array().into_iter().flatten().map(|s| s.as_u64().is_some_and(&mut hit)).collect()
}

impl Rebased {
    /// The whole profile on strobe's clock (`whole-host.json.gz`): each time rounded to a whole µs and
    /// the gaps taken between rounded times, so rounding never accumulates; `capUs` is `cap_us`.
    pub fn whole(&self, p: &Value) -> Value {
        let us = |x: f64| x.round() as i64;
        let start = us(self.start_us).min(self.times_us.first().map_or(i64::MAX, |t| us(*t)));
        let mut last = start;
        let deltas: Vec<i64> = self
            .times_us
            .iter()
            .map(|t| {
                let d = us(*t) - last;
                last = us(*t);
                d
            })
            .collect();
        json!({ "nodes": p["nodes"], "startTime": start, "endTime": last, "samples": p["samples"], "timeDeltas": deltas, "capUs": self.cap_us, "overUs": self.over_us })
    }

    /// The samples of one span (a measured run, µs on strobe's clock) as a profile of their own
    /// (`cpu-<run>.cpuprofile`): each weighs its time inside the span and outside `untimed`, a gap over
    /// `cap_us` weighing `over_us` (`weights`), the sample before the span for the span's first moments. Its
    /// timeline starts at the span's start and its gaps are those weights, so where a gap was cut the
    /// timeline closes it and never runs past the span, as Firefox's per-run profiles do
    /// (`gecko::convert`). Nodes the samples do not reach are left out, and so are `hitCount` and
    /// `positionTicks`, which count the whole profile. None without a sample there.
    pub fn clip(&self, p: &Value, span: (f64, f64), untimed: &[(f64, f64)]) -> Option<Value> {
        let w = weights(&self.times_us, self.cap_us, self.over_us, self.last_us, &[span], untimed);
        let picked: Vec<(usize, f64)> = w.into_iter().enumerate().filter(|x| x.1 > 0.0).collect();
        let (&(first, _), all) = (picked.first()?, p["samples"].as_array()?);
        let us = |x: f64| x.round() as i64;
        let mut at = self.times_us[first].max(span.0);
        let mut deltas = vec![us(at) - us(span.0)];
        let mut last = us(at);
        for (_, w) in &picked {
            at += w;
            deltas.push(us(at) - last);
            last = us(at);
        }
        let ids: Vec<Value> = picked.iter().map(|(i, _)| all[*i].clone()).collect();
        Some(json!({ "nodes": pruned(p, &ids), "startTime": us(span.0), "endTime": us(span.1), "samples": ids, "timeDeltas": deltas }))
    }

    /// The main-thread CPU the profile holds samples of inside `segments` (µs, strobe's clock; sorted and
    /// apart): every sample's weight there (`weights`) but the profile's own `(root)` and `(idle)`, a
    /// thread that waited. `(program)` and `(garbage collector)` are the thread on a CPU, rows of their
    /// own in every view. Against `cpu.main` it is what the sampler missed (`missed`).
    pub fn sampled_in(&self, p: &Value, segments: &[(f64, f64)]) -> f64 {
        let idle: BTreeSet<u64> =
            p["nodes"].as_array().into_iter().flatten().filter(|n| matches!(n["callFrame"]["functionName"].as_str(), Some("(root)" | "(idle)"))).filter_map(|n| n["id"].as_u64()).collect();
        let ids = p["samples"].as_array().map(Vec::as_slice).unwrap_or(&[]);
        weights(&self.times_us, self.cap_us, self.over_us, self.last_us, segments, &[]).iter().zip(ids).filter(|(_, id)| id.as_u64().is_none_or(|id| !idle.contains(&id))).map(|(w, _)| w).sum()
    }

    /// The gaps (µs) between consecutive samples inside one of `segments` (µs, strobe's clock), up to
    /// the cap: the interval the sampler kept where the runs were timed.
    pub fn gaps_in(&self, segments: &[(f64, f64)]) -> Vec<f64> {
        self.times_us.windows(2).filter(|w| segments.iter().any(|s| w[0] >= s.0 && w[1] <= s.1)).map(|w| w[1] - w[0]).filter(|g| *g > 0.0 && *g <= self.cap_us).collect()
    }
}

/// What each sample at `times` weighs inside `spans` and outside `untimed` (µs on one clock; spans
/// sorted and apart, untimed too): a sample stands for the time to the next one (the last: `last`); a
/// gap over `cap` stands for `over` instead; and only the part of that inside a span and not in an
/// untimed stretch counts.
pub fn weights(times: &[f64], cap: f64, over: f64, last: f64, spans: &[(f64, f64)], untimed: &[(f64, f64)]) -> Vec<f64> {
    let overlap = |a: f64, b: f64, list: &[(f64, f64)]| -> f64 {
        let from = list.partition_point(|s| s.1 <= a);
        list[from..].iter().take_while(|s| s.0 < b).map(|s| (b.min(s.1) - a.max(s.0)).max(0.0)).sum()
    };
    times
        .iter()
        .enumerate()
        .map(|(i, &a)| {
            let gap = times.get(i + 1).map_or(last, |n| (n - a).max(0.0));
            let b = a + if gap > cap { over } else { gap };
            let from = spans.partition_point(|s| s.1 <= a);
            spans[from..].iter().take_while(|s| s.0 < b).map(|s| (a.max(s.0), b.min(s.1))).filter(|(x, y)| y > x).map(|(x, y)| (y - x) - overlap(x, y, untimed)).sum::<f64>().max(0.0)
        })
        .collect()
}

/// A profile's nodes that `samples` reach and their ancestors, child lists cut to them, without the
/// whole profile's `hitCount` and `positionTicks`.
fn pruned(p: &Value, samples: &[Value]) -> Vec<Value> {
    let nodes = p["nodes"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let mut parent: HashMap<u64, u64> = HashMap::new();
    for n in nodes {
        let id = n["id"].as_u64().unwrap_or(0);
        for c in n["children"].as_array().into_iter().flatten().filter_map(Value::as_u64) {
            parent.insert(c, id);
        }
    }
    let mut keep: BTreeSet<u64> = BTreeSet::new();
    for s in samples.iter().filter_map(Value::as_u64) {
        let mut at = Some(s);
        while let Some(id) = at {
            if !keep.insert(id) {
                break;
            }
            at = parent.get(&id).copied();
        }
    }
    nodes
        .iter()
        .filter(|n| n["id"].as_u64().is_some_and(|id| keep.contains(&id)))
        .map(|n| {
            let mut n = n.clone();
            if let Some(o) = n.as_object_mut() {
                o.remove("hitCount");
                o.remove("positionTicks");
                let children: Vec<Value> = o.get("children").and_then(Value::as_array).into_iter().flatten().filter(|c| c.as_u64().is_some_and(|id| keep.contains(&id))).cloned().collect();
                o.insert("children".into(), json!(children));
            }
            n
        })
        .collect()
}
