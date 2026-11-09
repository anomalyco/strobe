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
}

impl View {
    fn open(dir: &Path, phase: Option<&str>) -> Result<View> {
        let data = data(dir)?;
        let id = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let idle: Vec<f64> = data.samples.iter().filter(|s| s.when == When::Idle).filter_map(Sample::latency_ms).collect();
        let idle_p95 = http_probe::censored_quantile(&idle, 0, 0.95).filter(|v| v.is_finite());
        let threshold = idle_p95.map_or(STALL_MS, |p| (STALL_IDLE * p).max(STALL_MS));
        let mut measured: Vec<Sample> = data.samples.iter().filter(|s| s.when == When::Measured).cloned().collect();
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
        }
        Ok(View { id, data, measured, idle, threshold })
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

    /// Probes in a row, by slot, that were slow (over `threshold`) or timed out: a stall the server held
    /// them through. A missed send or an error ends one.
    fn cohorts(&self) -> Vec<Vec<&Sample>> {
        let mut out: Vec<Vec<&Sample>> = Vec::new();
        let mut cur: Vec<&Sample> = Vec::new();
        for s in self.data.samples.iter().filter(|s| s.when != When::Idle) {
            let stalled = s.outcome == Outcome::Timeout || s.latency_ms().is_some_and(|l| l > self.threshold);
            if stalled && cur.last().is_none_or(|l| l.seq + 1 == s.seq) {
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
        let ended = c.iter().filter_map(|s| s.completed_ns).max();
        let began = first.dispatch_ns.unwrap_or(first.scheduled_ns);
        json!({
            "run": first.run, "when": first.when, "at_s": self.rel_s(first.scheduled_ns), "probes": c.len(),
            "timeouts": c.iter().filter(|s| s.outcome == Outcome::Timeout).count(),
            "latencies_ms": c.iter().map(|s| s.latency_ms()).collect::<Vec<_>>(),
            // The server answered none of them from the first one's dispatch until the last answer.
            "held_ms": ended.map(|e| e.saturating_sub(began) as f64 / 1e6),
            "held_past_timeout": c.last().is_some_and(|s| s.outcome == Outcome::Timeout),
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
        json!({
            "recording": self.id, "config": self.data.config,
            "idle": { "n": self.idle.len(), "p50": iq(0.5), "p95": iq(0.95), "max": iq(1.0) },
            "measured": {
                "runs": self.data.runs.iter().filter(|r| r.measured).count(), "due": self.measured.len(), "ok": self.answers().len(), "timeouts": self.timeouts(),
                "errors": self.count(|o| matches!(o, Outcome::Bad { .. } | Outcome::Error { .. } | Outcome::Pending)), "missed": self.count(Outcome::missed), "statuses": statuses,
                "p50": self.quantile(0.5), "p95": self.quantile(0.95), "max": self.quantile(1.0),
            },
            "probe_cpu_ms": self.data.cpu_ms, "probe_cpu_pct": self.data.cpu_ms.filter(|_| wall > 0.0).map(|c| 100.0 * c / wall),
            "stall_threshold_ms": self.threshold,
            "cohorts": self.cohorts().iter().take(top).map(|c| self.cohort_json(c)).collect::<Vec<_>>(),
        })
    }

    /// The setup line, the idle and measured lines, the probe's CPU.
    fn text(&self, j: &Value) -> String {
        let c = &self.data.config;
        let ms = |v: &Value| v.as_f64().map_or("—".to_string(), |x| fmt::value(x, "ms"));
        let censored = |v: &Value| if v.is_null() && self.timeouts() > 0 { format!("censored ({} timeouts at {} ms)", self.timeouts(), c.timeout_ms) } else { ms(v) };
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
            "measured  {} due in {} measured run{}' timed parts: {} answered · {} timeouts · {} errors · {} missed{}\n",
            m["due"],
            m["runs"],
            if m["runs"] == 1 { "" } else { "s" },
            m["ok"],
            m["timeouts"],
            m["errors"],
            m["missed"],
            if statuses.is_empty() { String::new() } else { format!(" · {}", statuses.join(", ")) }
        );
        t += &format!("          p50 {} · p95 {} · max {}\n", censored(&m["p50"]), censored(&m["p95"]), censored(&m["max"]));
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
            let held = match (c["held_ms"].as_f64(), c["held_past_timeout"] == true) {
                (Some(h), false) => format!("the server answered none for {}", fmt::value(h, "ms")),
                (Some(h), true) => format!("the server answered none for at least {} (the last timed out)", fmt::value(h, "ms")),
                (None, _) => "none of them was answered".to_string(),
            };
            t += &format!("  {} +{:.2} s  {} probes: {} ms  · {held}\n", c["run"].as_str().unwrap_or(c["when"].as_str().unwrap_or("")), c["at_s"].as_f64().unwrap_or(0.0), c["probes"], lat.join(" "));
        }
        t
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

/// An experiment's probe blocks: each one's summary, and where its probes are kept.
fn experiment(dir: &Path, phase: Option<&str>, top: usize) -> Result<Output> {
    let result = store::read_json(&dir.join("result.json")).map_err(|_| Error::usage("this experiment has no result yet: query it once it ends"))?;
    let mut rows: Vec<Vec<String>> = vec![["series", "side", "block", "p50", "p95", "max", "timeouts", "errors", "missed", "recording"].iter().map(|s| s.to_string()).collect()];
    let mut out = Vec::new();
    let ms = |v: &Value| v.as_f64().map_or("—".to_string(), |x| fmt::value(x, "ms"));
    for s in result["series"].as_array().into_iter().flatten() {
        let label = s["label"].as_str().unwrap_or("");
        let mut blocks: Vec<(String, &Value)> = Vec::new();
        for (kind, list) in [("", &s["blocks"]), ("pass ", &s["pass"]["blocks"])] {
            for b in list.as_array().into_iter().flatten().filter(|b| b["http"].is_object()) {
                blocks.push((format!("{kind}pair {}", b["pair"].as_u64().map_or("—".to_string(), |p| (p + 1).to_string())), b));
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
        for (what, b) in &blocks {
            let m = &b["http"]["measured"];
            rows.push(vec![
                label.into(),
                b["variant"].as_str().unwrap_or("").into(),
                what.clone(),
                ms(&m["p50"]),
                ms(&m["p95"]),
                ms(&m["max"]),
                m["timeouts"].to_string(),
                m["errors"].to_string(),
                m["missed"].to_string(),
                b["http"]["recording"].as_str().unwrap_or("").into(),
            ]);
            out.push(json!({ "series": label, "variant": b["variant"], "block": what, "http": b["http"] }));
        }
        for (what, r) in recs {
            let Ok(d) = store::resolve(&r) else { continue };
            let Ok(v) = View::open(&d, phase) else { continue };
            let j = v.json(top);
            let m = &j["measured"];
            if blocks.iter().any(|(_, b)| b["http"]["recording"] == r.as_str()) {
                continue;
            }
            rows.push(vec![
                label.into(),
                String::new(),
                what.clone(),
                ms(&m["p50"]),
                ms(&m["p95"]),
                ms(&m["max"]),
                m["timeouts"].to_string(),
                m["errors"].to_string(),
                m["missed"].to_string(),
                r.clone(),
            ]);
            out.push(json!({ "series": label, "block": what, "recording": r, "http": j }));
        }
    }
    if out.is_empty() {
        return Err(Error::usage("this experiment ran no HTTP probe: ctx.httpProbe in setup runs one in attribution blocks and --check, and in probe blocks of a series that declares http.*"));
    }
    let mut text = fmt::table(&rows, &[false, false, false, true, true, true, true, true, true, false], "", 2);
    text += "a quantile shown — is refused (censored by timeouts, a bad answer, missed sends) or unread; strobe query <recording> http shows a block's probes and stalls\n";
    Ok(Output::new(text, json!({ "id": result["id"], "blocks": out })))
}
