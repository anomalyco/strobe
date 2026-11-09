//! `strobe query <rec|exp|a..b> http` (SPEC §11.1): what the HTTP probe read (SPEC §8.9). The idle control
//! against the measured runs, each probe's fate, the probe's own CPU, and the stalled cohorts: probes in a
//! row that the server held, which say when it stopped answering and for how long.

use super::{Output, fmt};
use crate::cli::Args;
use crate::model::{Error, Result};
use crate::probe::http_probe::{self, Data, Outcome, Sample, When};
use crate::store;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

/// A stall is probes in a row slower than this many times the idle control's p95, and than `STALL_MS`.
const STALL_IDLE: f64 = 5.0;
const STALL_MS: f64 = 50.0;

pub fn run(args: &Args, top: usize) -> Result<Output> {
    let handle = args.pos.first().ok_or_else(|| Error::usage("query needs a handle: strobe query rec_… http"))?;
    let phase = args.get("phase");
    if let Some((a, b)) = handle.split_once("..") {
        return pair(&store::resolve(a)?, &store::resolve(b)?, phase, top);
    }
    let dir = store::resolve(handle)?;
    if store::is_experiment(&dir) {
        return experiment(&dir, phase, top);
    }
    recording(&dir, phase, top)
}

fn data(dir: &Path) -> Result<Data> {
    http_probe::read(dir).ok_or_else(|| {
        Error::usage(format!(
            "{} has no HTTP probe: it comes from ctx.httpProbe in a bench file's setup, in an attribution block (--why, record --bench), a --check, or the probe blocks of a series that declares http.*",
            dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
        ))
    })
}

/// `ctx.phase` names as strobe marks them (letters, digits, `-`, `_`, `.`; the rest `_`).
fn mark_name(n: &str) -> String {
    n.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' }).collect()
}

/// One recording's probes as the view reads them.
struct View {
    id: String,
    data: Data,
    /// The probes it reads as the runs' (the measured ones, or those in `--phase`).
    measured: Vec<Sample>,
    idle: Vec<f64>,
    threshold: f64,
    /// `--phase`: its spans in the measured runs, which the cohorts are cut to as well.
    spans: Option<Vec<(u64, u64)>>,
}

impl View {
    fn open(dir: &Path, phase: Option<&str>) -> Result<View> {
        let data = data(dir)?;
        let id = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let idle: Vec<f64> = data.samples.iter().filter(|s| s.when == When::Idle).filter_map(Sample::latency_ms).collect();
        let idle_p95 = http_probe::censored_quantile(&idle, 0, 0.95).filter(|v| v.is_finite());
        let threshold = idle_p95.map_or(STALL_MS, |p| (STALL_IDLE * p).max(STALL_MS));
        let mut measured: Vec<Sample> = data.samples.iter().filter(|s| s.when == When::Measured).cloned().collect();
        let mut scope = None;
        if let Some(p) = phase {
            let spans: Vec<(u64, u64)> = data.runs.iter().filter(|r| r.measured).flat_map(|r| r.phases.iter()).filter(|(n, _, _)| n == p || mark_name(n) == p).map(|(_, a, b)| (*a, *b)).collect();
            if spans.is_empty() {
                let mut names: Vec<String> = data.runs.iter().flat_map(|r| r.phases.iter().map(|x| mark_name(&x.0))).collect();
                names.sort();
                names.dedup();
                return Err(Error::usage(format!(
                    "{id}: no measured run entered phase {p}{}",
                    if names.is_empty() { " (its runs marked no ctx.phase)".to_string() } else { format!(" (its phases: {})", names.join(", ")) }
                )));
            }
            measured.retain(|s| spans.iter().any(|(a, b)| s.scheduled_ns >= *a && s.scheduled_ns < *b));
            scope = Some(spans);
        }
        Ok(View { id, data, measured, idle, threshold, spans: scope })
    }

    /// Whether a probe was due where the view looks: anywhere but the idle control, or in `--phase`.
    fn in_scope(&self, s: &Sample) -> bool {
        s.when != When::Idle && self.spans.as_ref().is_none_or(|sp| sp.iter().any(|(a, b)| s.scheduled_ns >= *a && s.scheduled_ns < *b))
    }

    fn answers(&self) -> Vec<f64> {
        self.measured.iter().filter_map(Sample::latency_ms).collect()
    }

    fn count(&self, f: impl Fn(&Outcome) -> bool) -> usize {
        self.measured.iter().filter(|s| f(&s.outcome)).count()
    }

    fn timeouts(&self) -> usize {
        self.count(|o| *o == Outcome::Timeout)
    }

    /// A quantile of the runs' latencies, or None where it falls on a timeout (right-censored).
    fn quantile(&self, p: f64) -> Option<f64> {
        http_probe::censored_quantile(&self.answers(), self.timeouts(), p).filter(|v| v.is_finite())
    }

    /// When a probe stopped being out: its answer (or failure), its timeout's deadline, or never (still
    /// out when the probe stopped).
    fn end_ns(&self, s: &Sample) -> Option<u64> {
        match (&s.outcome, s.completed_ns, s.dispatch_ns) {
            (_, Some(c), _) => Some(c),
            (Outcome::Timeout, None, Some(d)) => Some(d + (self.data.config.timeout_ms * 1e6) as u64),
            _ => None,
        }
    }

    /// Probes in a row, by slot, that were slow (over `threshold`) or timed out: a stall the server held
    /// them through. A missed send or an error ends one, and so does a probe sent after every probe
    /// before it had its answer (nothing out bridged the gap: the server answered in between). A probe
    /// that timed out was never answered, so it bridges on (a timeout no longer than the cadence ends
    /// before the next is sent) unless one of the cohort's probes was answered after it was sent.
    fn cohorts(&self) -> Vec<Vec<&Sample>> {
        let mut out: Vec<Vec<&Sample>> = Vec::new();
        let mut cur: Vec<&Sample> = Vec::new();
        for s in self.data.samples.iter().filter(|s| self.in_scope(s)) {
            let stalled = s.outcome == Outcome::Timeout || s.latency_ms().is_some_and(|l| l > self.threshold);
            let sent = s.dispatch_ns.unwrap_or(s.scheduled_ns);
            let answered_after = |p: &Sample| cur.iter().any(|q| q.outcome == Outcome::Ok && q.completed_ns.zip(p.dispatch_ns).is_some_and(|(c, d)| c > d));
            let bridged = cur.iter().any(|p| self.end_ns(p).is_none_or(|e| e > sent) || (p.outcome == Outcome::Timeout && !answered_after(p)));
            if stalled && cur.last().is_none_or(|l| l.seq + 1 == s.seq && bridged) {
                cur.push(s);
            } else {
                if cur.len() >= 2 {
                    out.push(std::mem::take(&mut cur));
                }
                cur.clear();
                if stalled {
                    cur.push(s);
                }
            }
        }
        if cur.len() >= 2 {
            out.push(cur);
        }
        out
    }

    fn rel_s(&self, t: u64) -> f64 {
        t.saturating_sub(self.data.start_ns) as f64 / 1e9
    }

    fn cohort_json(&self, c: &[&Sample]) -> Value {
        let first = c[0];
        let began = first.dispatch_ns.unwrap_or(first.scheduled_ns);
        let mut answers: Vec<u64> = c.iter().filter(|s| s.outcome != Outcome::Timeout).filter_map(|s| s.completed_ns).collect();
        answers.sort_unstable();
        let deadlines: Vec<u64> = c.iter().filter(|s| s.outcome == Outcome::Timeout).filter_map(|s| self.end_ns(s)).collect();
        let deadline = deadlines.iter().copied().max();
        let last = answers.last().copied();
        // Every answer the server gave from the first dispatch to the cohort's end, its own probes' and
        // any other's in scope (a response read, whatever it said): a silence is none of them.
        let horizon = last.max(deadline).unwrap_or(began);
        let mut heard: Vec<u64> = self
            .data
            .samples
            .iter()
            .filter(|s| self.in_scope(s) && matches!(s.outcome, Outcome::Ok | Outcome::Bad { .. }))
            .filter_map(|s| s.completed_ns)
            .filter(|t| *t > began && *t <= horizon)
            .collect();
        heard.sort_unstable();
        // Past the cohort's last answer only a timeout's deadline says how long one was held, and only
        // at least, so long as no other probe was answered before that deadline.
        let after = last.unwrap_or(began);
        let past_timeout = deadline.is_some_and(|d| d > after && !heard.iter().any(|t| *t > after && *t <= d));
        let ended = if past_timeout { deadline } else { last.or_else(|| heard.iter().copied().find(|t| *t > began)) };
        // Silences: from the first dispatch to each answer, answer to answer, and the last answer to a
        // later deadline, longer than the stall threshold. Each is a stall the server held probes
        // through; more than one is stalls back to back with probes still out between them.
        let mut points: Vec<u64> = std::iter::once(began).chain(heard.iter().copied().filter(|t| ended.is_some_and(|e| *t <= e))).collect();
        if past_timeout && let Some(d) = deadline {
            points.push(d);
        }
        let gap = (self.threshold * 1e6) as u64;
        let silences: Vec<(u64, u64)> = points.windows(2).map(|w| (w[0], w[1])).filter(|(a, b)| b.saturating_sub(*a) > gap).collect();
        // A probe still out where a later silence began: it waited across the answers between them.
        let carried = c.iter().filter(|s| silences.iter().skip(1).any(|(t, _)| s.dispatch_ns.is_some_and(|d| d < *t) && self.end_ns(s).is_none_or(|e| e > *t))).count();
        let timeouts = c.iter().filter(|s| s.outcome == Outcome::Timeout).count();
        json!({
            "run": first.run, "when": first.when, "at_s": self.rel_s(first.scheduled_ns), "probes": c.len(),
            "timeouts": timeouts, "any_timeout": timeouts > 0, "answered": answers.len(),
            // Timeouts whose deadline ran past every answer of the cohort (when it held past one).
            "timeouts_past_answers": if past_timeout && last.is_some() { deadlines.iter().filter(|d| **d > after).count() } else { 0 },
            "latencies_ms": c.iter().map(|s| s.latency_ms()).collect::<Vec<_>>(),
            // From the first one's dispatch until the last answer, or a later timeout's deadline.
            "held_ms": ended.map(|e| e.saturating_sub(began) as f64 / 1e6),
            "held_past_timeout": past_timeout,
            "waves": silences.len(),
            "longest_silence_ms": silences.iter().map(|(a, b)| (b - a) as f64 / 1e6).fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| m.max(x)))),
            "carried": carried,
        })
    }

    fn json(&self, top: usize) -> Value {
        let mut statuses: BTreeMap<String, usize> = BTreeMap::new();
        for s in &self.measured {
            if let Some(c) = s.status {
                *statuses.entry(c.to_string()).or_default() += 1;
            }
        }
        let iq = |p: f64| http_probe::censored_quantile(&self.idle, 0, p).filter(|v| v.is_finite());
        let wall = self.data.end_ns.saturating_sub(self.data.start_ns) as f64 / 1e6;
        let cohorts = self.cohorts();
        let cpu_pct = self.data.cpu_ms.filter(|_| wall > 0.0).map(|c| 100.0 * c / wall);
        let mut j = json!({
            "recording": self.id, "config": self.data.config,
            "idle": { "n": self.idle.len(), "p50": iq(0.5), "p95": iq(0.95), "max": iq(1.0) },
            "measured": {
                "runs": self.data.runs.iter().filter(|r| r.measured).count(), "due": self.measured.len(), "ok": self.answers().len(), "timeouts": self.timeouts(),
                "errors": self.count(|o| matches!(o, Outcome::Bad { .. } | Outcome::Error { .. } | Outcome::Pending)), "missed": self.count(Outcome::missed), "statuses": statuses,
                "p50": self.quantile(0.5), "p95": self.quantile(0.95), "max": self.quantile(1.0),
            },
            "probe_cpu_ms": self.data.cpu_ms, "probe_cpu_pct": cpu_pct,
            "stall_threshold_ms": self.threshold,
            "cohorts_total": cohorts.len(),
            "cohorts": cohorts.iter().take(top).map(|c| {
                let mut j = self.cohort_json(c);
                j["says"] = json!(self.says(&j));
                j
            }).collect::<Vec<_>>(),
        });
        // A recording's own view: a check's or attribution block's, whose probe CPU refuses nothing (a timed
        // block's entry judges it again with its CPU: `entries`).
        judge(&mut j["measured"], cpu_pct, false);
        j
    }

    /// The setup line, the idle and measured lines, the probe's CPU.
    fn text(&self, j: &Value) -> String {
        let c = &self.data.config;
        let ms = |v: &Value| v.as_f64().map_or("—".to_string(), |x| fmt::value(x, "ms"));
        let censored = |k: &str| match j["measured"]["states"][k].as_str() {
            Some("censored") => format!("censored ({} timeouts at {} ms)", self.timeouts(), c.timeout_ms),
            _ => ms(&j["measured"][k]),
        };
        let auth = match (&c.authorization_header, c.has_authorization) {
            (Some(h), true) => format!(" · {h} sent, never kept"),
            _ => String::new(),
        };
        let conn = if c.connection == http_probe::Connection::KeepAlive { "kept connections" } else { "a new connection per probe" };
        let m = &j["measured"];
        let statuses: Vec<String> = m["statuses"].as_object().into_iter().flatten().map(|(k, n)| format!("HTTP {k} ×{n}")).collect();
        let mut t = format!(
            "HTTP probe  {} {} · every {} ms · timeout {} ms · {conn}{auth}\n",
            serde_json::to_value(c.method).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default(),
            c.url,
            c.cadence_ms,
            c.timeout_ms
        );
        t += &match self.idle.len() {
            0 => "idle      no idle control (idleMs 0)\n".to_string(),
            n => format!("idle      {n} probes after setup, before any run: p50 {} · p95 {} · max {}\n", ms(&j["idle"]["p50"]), ms(&j["idle"]["p95"]), ms(&j["idle"]["max"])),
        };
        t += &format!(
            "measured  {} due in {} measured run{} timed parts: {} answered · {} timeouts · {} errors · {} missed{}\n",
            m["due"],
            m["runs"],
            if m["runs"] == 1 { "'s" } else { "s'" },
            m["ok"],
            m["timeouts"],
            m["errors"],
            m["missed"],
            if statuses.is_empty() { String::new() } else { format!(" · {}", statuses.join(", ")) }
        );
        t += &format!("          p50 {} · p95 {} · max {}{}\n", censored("p50"), censored("p95"), censored("max"), m["unread_why"].as_str().map(|w| format!("  (unread: {w})")).unwrap_or_default());
        if let Some(p) = j["probe_cpu_pct"].as_f64() {
            t += &format!("probe     CPU {p:.2}% of a core over {} (budget {}%)\n", fmt::ms_duration(self.data.end_ns.saturating_sub(self.data.start_ns) / 1_000_000), http_probe::CPU_BUDGET_PCT);
        }
        t
    }

    fn cohort_text(&self, j: &Value) -> String {
        let cohorts = j["cohorts"].as_array().cloned().unwrap_or_default();
        if cohorts.is_empty() {
            return format!("stalled   none: no two probes in a row over {}\n", fmt::value(self.threshold, "ms"));
        }
        let mut t = format!("stalled   probes in a row over {} ({STALL_IDLE}× the idle p95, at least {STALL_MS} ms):\n", fmt::value(self.threshold, "ms"));
        for c in &cohorts {
            let lat: Vec<String> = c["latencies_ms"].as_array().into_iter().flatten().map(|l| l.as_f64().map_or("TIMEOUT".to_string(), |x| format!("{x:.0}"))).collect();
            // Where it began: a measured run, or a run that is not (warmup, the check's run after its reset).
            let at = match (c["run"].as_str(), c["when"].as_str().unwrap_or("")) {
                (Some(r), "measured") => r.to_string(),
                (Some(r), when) => format!("{r} ({})", if when == "warmup" { "not measured" } else { when }),
                (None, when) => when.to_string(),
            };
            t += &format!("  {at} +{:.2} s  {} probes: {} ms  · {}\n", c["at_s"].as_f64().unwrap_or(0.0), c["probes"], lat.join(" "), c["says"].as_str().unwrap_or(""));
        }
        if let Some(more) = j["cohorts_total"].as_u64().map(|n| n.saturating_sub(cohorts.len() as u64)).filter(|m| *m > 0) {
            t += &format!("  … {more} more (--top N shows more)\n");
        }
        t
    }

    /// What a cohort says of the server: one stall it answered none of them in, or stalls back to
    /// back that probes still out waited across; and its timeouts, wherever they fell in it.
    fn says(&self, c: &Value) -> String {
        let past = c["held_past_timeout"] == true;
        let least = if past { "at least " } else { "" };
        let waves = c["waves"].as_u64().unwrap_or(1);
        let mut held = match c["held_ms"].as_f64() {
            Some(h) if waves <= 1 => format!("the server answered none for {least}{}", fmt::value(h, "ms")),
            Some(h) => {
                let carried = c["carried"].as_u64().unwrap_or(0);
                format!(
                    "spanned {waves} stalls over {least}{} (longest silence {}; {carried} probe{} waited across a brief drain)",
                    fmt::value(h, "ms"),
                    c["longest_silence_ms"].as_f64().map_or("—".to_string(), |s| fmt::value(s, "ms")),
                    if carried == 1 { "" } else { "s" }
                )
            }
            None => "none of them was answered".to_string(),
        };
        // Their latencies are past the timeout, unknown.
        if let Some(n) = c["timeouts"].as_u64().filter(|n| *n > 0) {
            let still = match c["timeouts_past_answers"].as_u64().unwrap_or(0) {
                0 => String::new(),
                k if k == n && n == 1 => ", still out after every answer".to_string(),
                k if k == n => ", all still out after every answer".to_string(),
                1 => ", one of them still out after every answer".to_string(),
                k => format!(", {k} of them still out after every answer"),
            };
            held += &format!(" · {n} timed out at {} ms{still}", self.data.config.timeout_ms);
        }
        held
    }
}

fn recording(dir: &Path, phase: Option<&str>, top: usize) -> Result<Output> {
    let v = View::open(dir, phase)?;
    let j = v.json(top);
    let mut text = v.text(&j);
    if let Some(p) = phase {
        text = text.replacen("measured  ", &format!("phase {p}  "), 1);
    }
    text += &v.cohort_text(&j);
    text += &format!("next      strobe open {} (the probes beside the runs)\n", v.id);
    Ok(Output::new(text, j))
}

fn pair(a: &Path, b: &Path, phase: Option<&str>, top: usize) -> Result<Output> {
    let (va, vb) = (View::open(a, phase)?, View::open(b, phase)?);
    let (ja, jb) = (va.json(top), vb.json(top));
    let mut text = String::new();
    for (v, j) in [(&va, &ja), (&vb, &jb)] {
        text += &format!("{}\n", v.id);
        text += &v.text(j).lines().map(|l| format!("  {l}\n")).collect::<String>();
        text += &v.cohort_text(j).lines().map(|l| format!("  {l}\n")).collect::<String>();
    }
    Ok(Output::new(text, json!({ "a": ja, "b": jb })))
}

/// Whether each latency quantile of a `measured` summary reads, as `http_probe::reading` and the block
/// refuse it: unread when no probe counted (none due, or none answered or timed out), any got an error
/// or a bad answer, strobe missed more than max(1, due / 20) sends, or, in a timed block, the probe used
/// over `CPU_REFUSE_PCT` of a core; else censored where it falls on a timeout. A quantile that does not
/// read is null; `states` names each one's state and `unread_why` the reasons.
fn judge(m: &mut Value, cpu_pct: Option<f64>, timed: bool) {
    let n = |k: &str| m[k].as_u64().unwrap_or(0);
    let (due, ok, timeouts, errors, missed) = (n("due"), n("ok"), n("timeouts"), n("errors"), n("missed"));
    let mut why: Vec<String> = Vec::new();
    if due == 0 || ok + timeouts == 0 {
        why.push("no probe that counts".into());
    }
    if errors > 0 {
        why.push(format!("{errors} error{} or bad answer{}", if errors == 1 { "" } else { "s" }, if errors == 1 { "" } else { "s" }));
    }
    if missed > 1.max(due / 20) {
        why.push(format!("strobe missed {missed} of {due} sends"));
    }
    if let Some(p) = cpu_pct.filter(|p| timed && *p > http_probe::CPU_REFUSE_PCT) {
        why.push(format!("the probe used {p:.1}% of a core (refused over {}%)", http_probe::CPU_REFUSE_PCT));
    }
    let mut states = serde_json::Map::new();
    for k in ["p50", "p95", "max"] {
        let state = match (why.is_empty(), m[k].is_number()) {
            (false, _) => "unread",
            (true, true) => "ok",
            (true, false) if timeouts > 0 => "censored",
            (true, false) => "unread",
        };
        if state != "ok" {
            m[k] = Value::Null;
        }
        states.insert(k.into(), json!(state));
    }
    m["states"] = Value::Object(states);
    m["unread"] = json!(!why.is_empty());
    m["unread_why"] = if why.is_empty() { Value::Null } else { json!(why.join("; ")) };
}

/// One series' probes: each probe block's summary (`views`: with its whole view, stalled cohorts and
/// all, where its recording was kept), then the check's and attribution blocks' recordings.
fn entries(s: &Value, phase: Option<&str>, top: usize, views: bool) -> Vec<Value> {
    let label = s["label"].as_str().unwrap_or("");
    let mut out = Vec::new();
    // `--blocks N` runs blocks of one build, not pairs.
    let unit = if s["kind"] == "blocks" { "block" } else { "pair" };
    for (kind, list) in [("", &s["blocks"]), ("pass ", &s["pass"]["blocks"])] {
        for b in list.as_array().into_iter().flatten().filter(|b| b["http"].is_object()) {
            let what = format!("{kind}{unit} {}", b["pair"].as_u64().map_or("—".to_string(), |p| (p + 1).to_string()));
            let mut e = json!({ "series": label, "variant": b["variant"], "block": what, "http": b["http"] });
            // A timed block: its probe's CPU over budget refuses its latencies too.
            let cpu = b["http"]["cpu_pct"].as_f64();
            judge(&mut e["http"]["measured"], cpu, true);
            if views
                && let Some(r) = b["http"]["recording"].as_str()
                && let Ok(v) = store::resolve(r).and_then(|d| View::open(&d, phase))
            {
                e["view"] = v.json(top);
                judge(&mut e["view"]["measured"], cpu, true);
            }
            out.push(e);
        }
    }
    // A check's block, and attribution blocks, keep their probes in their recordings.
    let mut recs: Vec<(String, String)> = Vec::new();
    if let Some(r) = s["recording"].as_str() {
        recs.push(("check".into(), r.to_string()));
    }
    for side in ["base", "head"] {
        if let Some(r) = s["attribution"][side].as_str() {
            recs.push((format!("{side} attribution"), r.to_string()));
        }
    }
    if let Some(r) = s["attribution"].as_str() {
        recs.push(("attribution".into(), r.to_string()));
    }
    for (what, r) in recs {
        if out.iter().any(|e| e["http"]["recording"] == r.as_str()) {
            continue;
        }
        let Ok(d) = store::resolve(&r) else { continue };
        let Ok(v) = View::open(&d, phase) else { continue };
        out.push(json!({ "series": label, "block": what, "recording": r, "http": v.json(top) }));
    }
    out
}

/// What `report` shows of a series' probes (`entries` with each kept block's view), or None when it
/// ran none.
pub fn series_json(s: &Value, top: usize) -> Option<Value> {
    let e = entries(s, None, top, true);
    (!e.is_empty()).then(|| json!(e))
}

/// What `report` shows of a recording's probes: its view, or None without one.
pub fn recording_json(dir: &Path, top: usize) -> Option<Value> {
    View::open(dir, None).ok().map(|v| v.json(top))
}

/// An experiment's probe blocks: each one's summary, and where its probes are kept.
fn experiment(dir: &Path, phase: Option<&str>, top: usize) -> Result<Output> {
    let result = store::read_json(&dir.join("result.json")).map_err(|_| Error::usage("this experiment has no result yet: query it once it ends"))?;
    let mut rows: Vec<Vec<String>> = vec![["series", "side", "block", "p50", "p95", "max", "timeouts", "errors", "missed", "recording"].iter().map(|s| s.to_string()).collect()];
    let ms = |v: &Value| v.as_f64().map_or("—".to_string(), |x| fmt::value(x, "ms"));
    let out: Vec<Value> = result["series"].as_array().into_iter().flatten().flat_map(|s| entries(s, phase, top, false)).collect();
    for e in &out {
        let m = &e["http"]["measured"];
        rows.push(vec![
            e["series"].as_str().unwrap_or("").into(),
            e["variant"].as_str().unwrap_or("").into(),
            e["block"].as_str().unwrap_or("").into(),
            ms(&m["p50"]),
            ms(&m["p95"]),
            ms(&m["max"]),
            m["timeouts"].to_string(),
            m["errors"].to_string(),
            m["missed"].to_string(),
            e["recording"].as_str().or(e["http"]["recording"].as_str()).unwrap_or("").into(),
        ]);
    }
    if out.is_empty() {
        return Err(Error::usage("this experiment ran no HTTP probe: ctx.httpProbe in setup runs one in attribution blocks and --check, and in probe blocks of a series that declares http.*"));
    }
    let mut text = fmt::table(&rows, &[false, false, false, true, true, true, true, true, true, false], "", 2);
    text += "a quantile shown — is refused (censored by timeouts, a bad answer, missed sends) or unread; strobe query <recording> http shows a block's probes and stalls\n";
    Ok(Output::new(text, json!({ "id": result["id"], "blocks": out })))
}
