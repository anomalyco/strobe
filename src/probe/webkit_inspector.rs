//! WebKit Inspector attribution probes over `target::wir` (M0.3):
//! - `Timeline` (`RecalculateStyles`, `Layout`, `Paint`, `Composite`, `EvaluateScript`, `FunctionCall`,
//!   `RenderingFrame`, `TimeStamp` run marks) → `webkit.json` (`Part::Layout`, `Part::Phases`,
//!   `Part::LongTasks`, plus timeline spans for Perfetto).
//! - `ScriptProfiler` (`trackingUpdate` turns + `trackingComplete` `stackTraces`) → `cpu-0.cpuprofile`
//!   + `maps/` fetched through `Page.getResourceTree` and `Network.loadResource` (`query hotspots`).
//! - `Heap` (`garbageCollected` → `Part::Gc`; `Heap.snapshot` WebKit v3 → V8 `.heapsnapshot.gz` for
//!   `query heap` and `query leaks`) and `Memory` (`trackingUpdate` categories → `census` for
//!   `query memory`).
//! - `Network` (`requestWillBeSent`, `responseReceived`, `dataReceived`, `loadingFinished` metrics,
//!   `loadingFailed`, `requestServedFromMemoryCache`) + `Runtime` Performance Timing → owner-only
//!   `network.json.gz` (`query network`), never re-fetching URLs and never keeping request headers.

use crate::host::paths;
use crate::model::{Error, Result};
use crate::probe::cpu::Sampling;
use crate::store;
use crate::target::wir::{ConsoleEntry, Session, WirEvent, collect_console_events};
use flate2::Compression;
use flate2::write::GzEncoder;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;
use std::path::Path;
use std::time::Duration;

/// Stored summary rows for `query` (`Part::Layout`, `Gc`, `Phases`, `LongTasks`) and `perfetto`.
pub const READ: &str = "webkit.json";
pub const NETWORK_FILE: &str = "network.json.gz";
pub const CONSOLE_FILE: &str = "console.json.gz";
pub const DEVICE_LOG_FILE: &str = "device-log.json.gz";
pub const CRASHES_FILE: &str = "crashes.json.gz";

/// Maximum weight one JSC sample may take across idle gaps (10 ms; JSC only samples active JS turns).
const MAX_SAMPLE_WEIGHT_S: f64 = 0.010;

pub struct CaptureResult {
    pub values: BTreeMap<String, f64>,
    pub sampling: Option<Sampling>,
    pub census: Vec<Value>,
    pub console_count: usize,
    pub console_errors: usize,
    pub network_requests: usize,
    pub network_bytes: u64,
    pub timeline_records: usize,
    pub js_samples: u64,
    pub mapped_scripts: usize,
    pub heap_self_bytes: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NetworkEntry {
    pub request_id: String,
    pub url: String,
    pub method: String,
    pub resource_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    pub start_ms: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dns_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connect_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttfb_ms: Option<f64>,
    pub encoded_bytes: u64,
    pub decoded_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub canceled: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NetworkWaterfall {
    pub navigation: Value,
    pub paint: Vec<Value>,
    pub spans: Vec<Value>,
    pub requests: Vec<NetworkEntry>,
}

pub fn write_owner_gz<T: Serialize>(path: &Path, val: &T) -> Result<()> {
    let raw = serde_json::to_vec(val)?;
    let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
    gz.write_all(&raw)?;
    paths::write_owner_only(path, &gz.finish()?)?;
    Ok(())
}

pub fn read_gz_json(path: &Path) -> Result<Value> {
    let f = std::fs::File::open(path)?;
    let mut text = String::new();
    std::io::Read::read_to_string(&mut flate2::read::MultiGzDecoder::new(f), &mut text)?;
    Ok(serde_json::from_str(&text)?)
}

/// Runs WebKit Inspector attribution over `for_ms` on `session`, then takes a Heap v3 snapshot and
/// source maps before returning. Leaves no globals or performance marks on the user's page.
pub fn record_attribution(session: &mut Session, dir: &Path, token: &str, for_ms: u64, served: Option<&Path>, on_progress: impl Fn(&str)) -> Result<CaptureResult> {
    let _ = session.call("Runtime.enable", Value::Null, Duration::from_secs(5));
    let _ = session.call("Page.enable", Value::Null, Duration::from_secs(5));
    let _ = session.call("Console.enable", Value::Null, Duration::from_secs(5));
    let _ = session.call("Network.enable", Value::Null, Duration::from_secs(5));
    let _ = session.call("Heap.enable", Value::Null, Duration::from_secs(5));
    let memory_ok = session.call("Memory.enable", Value::Null, Duration::from_secs(3)).is_ok();
    let _ = session.call("Timeline.enable", Value::Null, Duration::from_secs(5));
    let _ = session.call("Timeline.setInstruments", json!({ "instruments": ["Timeline", "ScriptProfiler", "Memory", "Heap"] }), Duration::from_secs(5));

    // Flush pre-run buffered events (buffered console messages) before starting timers.
    session.pump_for(Duration::from_millis(50))?;
    let mut console: Vec<ConsoleEntry> = Vec::new();
    let pre_events = session.drain_events();

    if memory_ok {
        let _ = session.call("Memory.startTracking", Value::Null, Duration::from_secs(3));
    }
    session.call("Timeline.start", json!({ "maxCallStackDepth": 32 }), Duration::from_secs(5))?;
    session.call("ScriptProfiler.startTracking", json!({ "includeSamples": true }), Duration::from_secs(5))?;

    let start_mark = format!("strobe:{token}:run:0:start");
    let end_mark = format!("strobe:{token}:run:0:end");
    let t0 = crate::host::now_ns();
    collect_console_events(&pre_events, t0, &mut console);
    let _ = session.eval(&format!("try {{ performance.mark({:?}) }} catch {{}}", start_mark), Duration::from_secs(3));

    session.pump_for(Duration::from_millis(for_ms))?;

    let _ = session.eval(&format!("try {{ performance.mark({:?}) }} catch {{}}", end_mark), Duration::from_secs(3));
    let t1 = crate::host::now_ns();

    on_progress("stopping WebKit Timeline and ScriptProfiler");
    let _ = session.call("ScriptProfiler.stopTracking", Value::Null, Duration::from_secs(15));
    let _ = session.call("Timeline.stop", Value::Null, Duration::from_secs(15));
    if memory_ok {
        let _ = session.call("Memory.stopTracking", Value::Null, Duration::from_secs(5));
    }

    // Drain until ScriptProfiler.trackingComplete arrives (up to 3 s).
    let mut events = session.drain_events();
    let wait_complete = std::time::Instant::now() + Duration::from_secs(3);
    while !events.iter().any(|e| e.method == "ScriptProfiler.trackingComplete") && std::time::Instant::now() < wait_complete {
        session.pump_for(Duration::from_millis(80))?;
        events.extend(session.drain_events());
    }
    collect_console_events(&events, t0, &mut console);

    // Read in-page Navigation, Paint, Resource, User Timing (`performance.measure` spans) and DOM count,
    // then clear our two marks so the user's tab stays clean.
    let perf_js = format!(
        r#"(() => {{
  const nav = performance.getEntriesByType?.("navigation")?.[0]
  const paints = (performance.getEntriesByType?.("paint") ?? []).map((p) => ({{ name: p.name, start_ms: p.startTime }}))
  const measures = (performance.getEntriesByType?.("measure") ?? [])
    .filter((m) => !String(m.name).startsWith("strobe:"))
    .slice(-200)
    .map((m) => ({{ name: m.name, start_ms: m.startTime, duration_ms: m.duration }}))
  const dom = document.getElementsByTagName("*").length
  try {{ performance.clearMarks({start:?}); performance.clearMarks({end:?}) }} catch {{}}
  return JSON.stringify({{
    dom,
    nav: nav ? {{
      type: nav.type,
      protocol: nav.nextHopProtocol || null,
      ttfb_ms: nav.responseStart - nav.startTime,
      dom_content_loaded_ms: nav.domContentLoadedEventEnd - nav.startTime,
      load_ms: nav.loadEventEnd > 0 ? nav.loadEventEnd - nav.startTime : null,
      transfer_bytes: nav.transferSize ?? null,
      decoded_bytes: nav.decodedBodySize ?? null,
    }} : null,
    paints,
    measures,
  }})
}})()"#,
        start = start_mark,
        end = end_mark,
    );
    let page_perf: Value = session.eval(&perf_js, Duration::from_secs(5)).ok().and_then(|v| v.as_str().and_then(|s| serde_json::from_str(s).ok())).unwrap_or(Value::Null);

    on_progress("taking a WebKit heap snapshot");
    let mut heap_self_bytes = None;
    if let Ok(snap_res) = session.call("Heap.snapshot", Value::Null, Duration::from_secs(30))
        && let Some(snap_str) = snap_res["snapshotData"].as_str()
    {
        heap_self_bytes = Some(convert_webkit_heap(snap_str, &dir.join("heap-0.heapsnapshot.gz"))?);
    }

    // Convert Timeline + ScriptProfiler + GC + Memory + Network.
    let converted = convert_events(&events, token, &start_mark, &end_mark, t0, t1, &page_perf, heap_self_bytes, session.page.app_pid, dir)?;

    // Fetch source maps for profiled scripts through Page.getResourceTree + Network.loadResource.
    let mapped_scripts = if !converted.scripts.is_empty() {
        on_progress("loading source maps from WebKit");
        store_webkit_maps(session, &converted.scripts, &converted.source_map_hints, dir, served).unwrap_or(0)
    } else {
        0
    };

    write_owner_gz(&dir.join(CONSOLE_FILE), &console)?;
    let console_errors = console.iter().filter(|c| c.level == "error").count();

    Ok(CaptureResult {
        values: converted.values,
        sampling: converted.sampling,
        census: converted.census,
        console_count: console.len(),
        console_errors,
        network_requests: converted.network_requests,
        network_bytes: converted.network_bytes,
        timeline_records: converted.timeline_records,
        js_samples: converted.js_samples,
        mapped_scripts,
        heap_self_bytes,
    })
}

pub struct MemoryCapture {
    pub values: BTreeMap<String, f64>,
    pub census: Vec<Value>,
    pub leaks: Value,
    pub console: Vec<ConsoleEntry>,
}

/// `--probes memory` on `wir:`: three WebKit Heap v3 snapshots (S0, S1, S2) converted to V8
/// `.heapsnapshot.gz` so `heapsnap::leaks` finds objects allocated in the first half and alive in S2.
pub fn record_memory(session: &mut Session, dir: &Path, for_ms: u64, on_progress: impl Fn(&str)) -> Result<MemoryCapture> {
    let t0 = crate::host::now_ns();
    let _ = session.call("Runtime.enable", Value::Null, Duration::from_secs(5));
    let _ = session.call("Console.enable", Value::Null, Duration::from_secs(5));
    let _ = session.call("Heap.enable", Value::Null, Duration::from_secs(5));
    let started = std::time::Instant::now();
    let mut census = Vec::new();
    let mut console = Vec::new();

    for (i, name) in ["heap-s0", "heap-s1", "heap-0"].iter().enumerate() {
        if i > 0 {
            session.pump_for(Duration::from_millis(for_ms / 2))?;
        }
        on_progress(&format!("taking heap snapshot {} of 3", i + 1));
        let dom_nodes = session.eval("document.getElementsByTagName('*').length", Duration::from_secs(5)).ok().and_then(|v| v.as_u64()).unwrap_or(0);
        let snap_res = session.call("Heap.snapshot", Value::Null, Duration::from_secs(30))?;
        let snap_str = snap_res["snapshotData"].as_str().ok_or_else(|| Error::msg("Heap.snapshot returned no snapshotData"))?;
        let self_bytes = convert_webkit_heap(snap_str, &dir.join(format!("{name}.heapsnapshot.gz")))?;
        census.push(json!({
            "engine": "webkit",
            "at_ms": started.elapsed().as_millis() as u64,
            "taken": match i { 0 => "at the start (S0)", 1 => "at the middle (S1)", _ => "at the end (S2)" },
            "js_heap_bytes": self_bytes,
            "js_heap_total_bytes": self_bytes,
            "dom_nodes": dom_nodes,
            "dom": { "nodes": dom_nodes },
            "processes": [],
        }));
    }
    let evs = session.drain_events();
    collect_console_events(&evs, t0, &mut console);
    write_owner_gz(&dir.join(CONSOLE_FILE), &console)?;

    let leaks = crate::heapsnap::leaks(
        &crate::heapsnap::ids(&dir.join("heap-s0.heapsnapshot.gz"))?,
        &crate::heapsnap::ids(&dir.join("heap-s1.heapsnapshot.gz"))?,
        &crate::heapsnap::read(&dir.join("heap-0.heapsnapshot.gz"))?,
        25,
        false,
    );
    store::write_json(&dir.join("leaks.json"), &leaks)?;

    let at = |i: usize, k: &str| census[i][k].as_f64().unwrap_or(f64::NAN);
    let minutes = ((at(2, "at_ms") - at(0, "at_ms")) / 60_000.0).max(1e-6);
    let mut values = BTreeMap::new();
    values.insert("js.heap.used".into(), at(2, "js_heap_bytes") / (1024.0 * 1024.0));
    values.insert("js.heap.growth".into(), (at(2, "js_heap_bytes") - at(0, "js_heap_bytes")) / (1024.0 * 1024.0) / minutes);
    values.insert("dom.nodes".into(), at(2, "dom_nodes"));
    values.insert("dom.nodes.growth".into(), (at(2, "dom_nodes") - at(0, "dom_nodes")) / minutes);
    values.insert("leak.suspects.retained".into(), leaks["retained_size"].as_f64().unwrap_or(0.0) / (1024.0 * 1024.0));
    values.retain(|_, v| v.is_finite());
    Ok(MemoryCapture { values, census, leaks, console })
}

struct ConvertedEvents {
    values: BTreeMap<String, f64>,
    sampling: Option<Sampling>,
    census: Vec<Value>,
    scripts: BTreeSet<String>,
    source_map_hints: HashMap<String, String>,
    network_requests: usize,
    network_bytes: u64,
    timeline_records: usize,
    js_samples: u64,
}

type Row = Vec<Option<String>>;

fn cell(v: impl ToString) -> Option<String> {
    Some(v.to_string())
}

fn find_mark_sw(record: &Value, want: &str) -> Option<f64> {
    if record["type"].as_str() == Some("TimeStamp") && record["data"]["message"].as_str() == Some(want) {
        return record["startTime"].as_f64();
    }
    for kid in record["children"].as_array().into_iter().flatten() {
        if let Some(t) = find_mark_sw(kid, want) {
            return Some(t);
        }
    }
    None
}

/// Encodes a WebKit `Console.StackTrace` (`callFrames` with 1-based `lineNumber` and `columnNumber`)
/// into the `stackTrace[i].field\x1fval\x1e` format `query::layout` symbolizes.
fn encode_webkit_stack(st: &Value, scripts: &mut BTreeSet<String>) -> Option<String> {
    let frames = st["callFrames"].as_array()?;
    let mut out = String::new();
    let mut idx = 0usize;
    for cf in frames {
        let url = cf["url"].as_str().unwrap_or("");
        if url.is_empty() || url == "[native code]" {
            continue;
        }
        let name = cf["functionName"].as_str().unwrap_or("");
        let line = cf["lineNumber"].as_i64().unwrap_or(-1);
        let col = cf["columnNumber"].as_i64().unwrap_or(-1);
        if line <= 0 {
            continue;
        }
        if url.contains("://") {
            scripts.insert(url.to_string());
        }
        for (k, v) in [("url", url.to_string()), ("functionName", name.to_string()), ("lineNumber", line.to_string()), ("columnNumber", col.max(1).to_string())] {
            out += &format!("stackTrace[{idx}].{k}\u{1f}{v}\u{1e}");
        }
        idx += 1;
    }
    (!out.is_empty()).then_some(out)
}

#[derive(Clone, Serialize, Deserialize)]
pub struct TimelineSlice {
    pub kind: String,
    pub t0_ns: u64,
    pub t1_ns: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[allow(clippy::too_many_arguments)]
fn walk_timeline(
    rec: &Value,
    parent_stack: Option<&Value>,
    to_host_ns: &dyn Fn(f64) -> u64,
    win_key: &str,
    t0: u64,
    t1: u64,
    scripts: &mut BTreeSet<String>,
    layout_rows: &mut Vec<Row>,
    phase_rows: &mut Vec<Row>,
    busy_spans: &mut Vec<(u64, u64)>,
    long_tasks: &mut (u64, u64),
    slices: &mut Vec<TimelineSlice>,
    depth: usize,
) -> usize {
    let mut count = 1usize;
    let ty = rec["type"].as_str().unwrap_or("");
    let own_stack = rec.get("stackTrace").filter(|s| s.is_object()).or(parent_stack);
    if let Some(s) = rec["startTime"].as_f64() {
        let e = rec["endTime"].as_f64().unwrap_or(s).max(s);
        let a = to_host_ns(s);
        let b = to_host_ns(e).max(a);
        if b >= t0 && a <= t1 && ty != "RenderingFrame" {
            let ca = a.max(t0);
            let cb = b.min(t1);
            let dur = cb.saturating_sub(ca);
            if depth == 0 && dur > 0 {
                busy_spans.push((ca, cb));
                if dur > 50_000_000 {
                    long_tasks.0 += 1;
                    long_tasks.1 = long_tasks.1.max(dur);
                }
            }
            if dur > 0 || ty == "TimeStamp" {
                let detail = rec["data"]["url"]
                    .as_str()
                    .or_else(|| rec["data"]["scriptName"].as_str())
                    .or_else(|| rec["data"]["type"].as_str())
                    .or_else(|| rec["data"]["message"].as_str())
                    .filter(|m| !m.starts_with("strobe:"))
                    .map(str::to_string);
                if !ty.is_empty() && slices.len() < 20_000 {
                    slices.push(TimelineSlice { kind: ty.to_string(), t0_ns: ca, t1_ns: cb, detail });
                }
            }
            match ty {
                "RecalculateStyles" => {
                    let st = own_stack.and_then(|st| encode_webkit_stack(st, scripts));
                    layout_rows.push(vec![cell(win_key), cell("UpdateLayoutTree"), cell(ca), cell(dur), cell(1), st]);
                    phase_rows.push(vec![cell(win_key), cell("UpdateLayoutTree"), cell(ca), cell(dur), cell(0)]);
                }
                "Layout" => {
                    let st = own_stack.and_then(|st| encode_webkit_stack(st, scripts));
                    layout_rows.push(vec![cell(win_key), cell("Layout"), cell(ca), cell(dur), cell(1), st]);
                    phase_rows.push(vec![cell(win_key), cell("Layout"), cell(ca), cell(dur), cell(0)]);
                }
                "Paint" | "Composite" => {
                    let label = if ty == "Paint" { "Paint" } else { "PrePaint" };
                    phase_rows.push(vec![cell(win_key), cell(label), cell(ca), cell(dur), cell(0)]);
                }
                _ => {}
            }
        }
    }
    for kid in rec["children"].as_array().into_iter().flatten() {
        let next_depth = if ty == "RenderingFrame" { depth } else { depth + 1 };
        count += walk_timeline(kid, own_stack, to_host_ns, win_key, t0, t1, scripts, layout_rows, phase_rows, busy_spans, long_tasks, slices, next_depth);
    }
    count
}

#[allow(clippy::too_many_arguments)]
fn convert_events(
    events: &[WirEvent],
    token: &str,
    start_mark: &str,
    end_mark: &str,
    t0: u64,
    t1: u64,
    page_perf: &Value,
    heap_self_bytes: Option<u64>,
    app_pid: Option<u32>,
    dir: &Path,
) -> Result<ConvertedEvents> {
    // Establish WebKit's executionStopwatch anchor `sw0` corresponding to `t0`.
    let mut mark_sw0 = None;
    let mut mark_sw1 = None;
    let mut started_sw = None;
    for ev in events {
        match ev.method.as_str() {
            "Timeline.eventRecorded" => {
                let rec = &ev.params["record"];
                if mark_sw0.is_none() {
                    mark_sw0 = find_mark_sw(rec, start_mark);
                }
                if mark_sw1.is_none() {
                    mark_sw1 = find_mark_sw(rec, end_mark);
                }
            }
            "Timeline.recordingStarted" => {
                started_sw = started_sw.or_else(|| ev.params["startTime"].as_f64());
            }
            "ScriptProfiler.trackingStart" => {
                started_sw = started_sw.or_else(|| ev.params["timestamp"].as_f64());
            }
            _ => {}
        }
    }
    let sw0 = mark_sw0.or(started_sw).unwrap_or(0.0);
    let sw_end = mark_sw1.unwrap_or_else(|| sw0 + (t1.saturating_sub(t0) as f64 / 1e9));
    let to_host_ns = |sw: f64| -> u64 {
        let delta_ns = ((sw - sw0) * 1e9).round();
        if delta_ns >= 0.0 { t0.saturating_add(delta_ns as u64) } else { t0.saturating_sub((-delta_ns) as u64) }
    };

    let win_key = format!("strobe:{token}:run:0");
    let mut scripts = BTreeSet::new();
    let mut layout_rows: Vec<Row> = vec![vec![cell(&win_key), None, None, None, None, None]];
    let mut gc_rows: Vec<Row> = vec![vec![cell(&win_key), None, None, None, None, None, None]];
    let mut phase_rows: Vec<Row> = vec![vec![cell(&win_key), None, None, None, None]];
    let mut busy_spans: Vec<(u64, u64)> = Vec::new();
    let mut long_tasks = (0u64, 0u64);
    let mut slices: Vec<TimelineSlice> = Vec::new();
    let mut timeline_records = 0usize;
    let mut script_turns: Vec<(f64, f64)> = Vec::new();
    let mut stack_traces: Vec<Value> = Vec::new();
    let mut memory_events: Vec<Value> = Vec::new();

    // Network tracking state keyed by requestId.
    let mut net_map: HashMap<String, NetworkEntry> = HashMap::new();
    let mut net_order: Vec<String> = Vec::new();
    let mut source_map_hints: HashMap<String, String> = HashMap::new();

    for ev in events {
        let p = &ev.params;
        match ev.method.as_str() {
            "Timeline.eventRecorded" => {
                timeline_records += walk_timeline(&p["record"], None, &to_host_ns, &win_key, t0, t1, &mut scripts, &mut layout_rows, &mut phase_rows, &mut busy_spans, &mut long_tasks, &mut slices, 0);
            }
            "ScriptProfiler.trackingUpdate" => {
                if let (Some(s), Some(e)) = (p["event"]["startTime"].as_f64(), p["event"]["endTime"].as_f64()) {
                    let e = e.max(s);
                    script_turns.push((s, e));
                    let a = to_host_ns(s);
                    let b = to_host_ns(e);
                    if b >= t0 && a <= t1 {
                        let ca = a.max(t0);
                        let cb = b.min(t1);
                        let dur = cb.saturating_sub(ca);
                        if dur > 0 {
                            busy_spans.push((ca, cb));
                            if dur > 50_000_000 {
                                long_tasks.0 += 1;
                                long_tasks.1 = long_tasks.1.max(dur);
                            }
                        }
                    }
                }
            }
            "ScriptProfiler.trackingComplete" => {
                if let Some(traces) = p["samples"]["stackTraces"].as_array() {
                    stack_traces.extend(traces.iter().cloned());
                } else if let Some(traces) = p["samples"]["traces"].as_array() {
                    stack_traces.extend(traces.iter().cloned());
                }
            }
            "Heap.garbageCollected" => {
                let c = &p["collection"];
                if let (Some(s), Some(e)) = (c["startTime"].as_f64(), c["endTime"].as_f64()) {
                    let a = to_host_ns(s);
                    let b = to_host_ns(e.max(s));
                    if b >= t0 && a <= t1 {
                        let ca = a.max(t0);
                        let cb = b.min(t1);
                        let dur = cb.saturating_sub(ca);
                        let kind = if c["type"].as_str() == Some("full") { "MajorGC" } else { "MinorGC" };
                        gc_rows.push(vec![cell(&win_key), cell(kind), cell(ca), cell(dur), cell(1), cell(1), None]);
                        if dur > 0 {
                            busy_spans.push((ca, cb));
                        }
                        slices.push(TimelineSlice { kind: kind.into(), t0_ns: ca, t1_ns: cb, detail: None });
                    }
                }
            }
            "Memory.trackingUpdate" => {
                if p["event"].is_object() {
                    memory_events.push(p["event"].clone());
                }
            }
            "Network.requestWillBeSent" => {
                let Some(rid) = p["requestId"].as_str() else { continue };
                let url = p["request"]["url"].as_str().unwrap_or("").to_string();
                let method = p["request"]["method"].as_str().unwrap_or("GET").to_string();
                let rtype = p["type"].as_str().unwrap_or("Other").to_string();
                let start_ms = p["timestamp"].as_f64().map(|ts| ((ts - sw0) * 1e3).max(0.0)).unwrap_or(0.0);
                if !net_map.contains_key(rid) {
                    net_order.push(rid.to_string());
                }
                net_map.insert(
                    rid.to_string(),
                    NetworkEntry {
                        request_id: rid.to_string(),
                        url,
                        method,
                        resource_type: rtype,
                        status: None,
                        mime_type: None,
                        source: None,
                        protocol: None,
                        priority: None,
                        start_ms,
                        response_ms: None,
                        end_ms: None,
                        duration_ms: None,
                        dns_ms: None,
                        connect_ms: None,
                        tls_ms: None,
                        ttfb_ms: None,
                        encoded_bytes: 0,
                        decoded_bytes: 0,
                        error: None,
                        canceled: false,
                    },
                );
            }
            "Network.requestServedFromMemoryCache" => {
                let Some(rid) = p["requestId"].as_str() else { continue };
                let res = &p["resource"];
                let url = res["url"].as_str().unwrap_or("").to_string();
                let rtype = res["type"].as_str().unwrap_or("Other").to_string();
                let size = res["bodySize"].as_u64().unwrap_or(0);
                if let Some(sm) = res["sourceMapURL"].as_str().filter(|s| !s.is_empty()) {
                    source_map_hints.insert(url.clone(), sm.to_string());
                }
                let start_ms = p["timestamp"].as_f64().map(|ts| ((ts - sw0) * 1e3).max(0.0)).unwrap_or(0.0);
                if !net_map.contains_key(rid) {
                    net_order.push(rid.to_string());
                }
                net_map.insert(
                    rid.to_string(),
                    NetworkEntry {
                        request_id: rid.to_string(),
                        url,
                        method: "GET".into(),
                        resource_type: rtype,
                        status: res["response"]["status"].as_i64().or(Some(200)),
                        mime_type: res["response"]["mimeType"].as_str().map(str::to_string),
                        source: Some("memory-cache".into()),
                        protocol: None,
                        priority: None,
                        start_ms,
                        response_ms: Some(start_ms),
                        end_ms: Some(start_ms),
                        duration_ms: Some(0.0),
                        dns_ms: None,
                        connect_ms: None,
                        tls_ms: None,
                        ttfb_ms: None,
                        encoded_bytes: 0,
                        decoded_bytes: size,
                        error: None,
                        canceled: false,
                    },
                );
            }
            "Network.responseReceived" => {
                let Some(rid) = p["requestId"].as_str() else { continue };
                let resp = &p["response"];
                let at_ms = p["timestamp"].as_f64().map(|ts| ((ts - sw0) * 1e3).max(0.0));
                let e = net_map.entry(rid.to_string()).or_insert_with(|| {
                    net_order.push(rid.to_string());
                    NetworkEntry {
                        request_id: rid.to_string(),
                        url: resp["url"].as_str().unwrap_or("").to_string(),
                        method: "GET".into(),
                        resource_type: p["type"].as_str().unwrap_or("Other").to_string(),
                        status: None,
                        mime_type: None,
                        source: None,
                        protocol: None,
                        priority: None,
                        start_ms: at_ms.unwrap_or(0.0),
                        response_ms: None,
                        end_ms: None,
                        duration_ms: None,
                        dns_ms: None,
                        connect_ms: None,
                        tls_ms: None,
                        ttfb_ms: None,
                        encoded_bytes: 0,
                        decoded_bytes: 0,
                        error: None,
                        canceled: false,
                    }
                });
                if let Some(t) = p["type"].as_str() {
                    e.resource_type = t.to_string();
                }
                e.status = resp["status"].as_i64();
                e.mime_type = resp["mimeType"].as_str().map(str::to_string);
                e.source = resp["source"].as_str().map(str::to_string);
                e.response_ms = at_ms;
                let tm = &resp["timing"];
                if tm.is_object() {
                    let span = |a: &str, b: &str| match (tm[a].as_f64(), tm[b].as_f64()) {
                        (Some(x), Some(y)) if x >= 0.0 && y >= x => Some(y - x),
                        _ => None,
                    };
                    e.dns_ms = span("domainLookupStart", "domainLookupEnd");
                    e.connect_ms = span("connectStart", "connectEnd");
                    e.tls_ms = span("secureConnectionStart", "connectEnd");
                    e.ttfb_ms = span("requestStart", "responseStart");
                }
            }
            "Network.dataReceived" => {
                if let Some(rid) = p["requestId"].as_str()
                    && let Some(e) = net_map.get_mut(rid)
                {
                    e.decoded_bytes = e.decoded_bytes.saturating_add(p["dataLength"].as_u64().unwrap_or(0));
                    e.encoded_bytes = e.encoded_bytes.saturating_add(p["encodedDataLength"].as_u64().unwrap_or(0));
                }
            }
            "Network.loadingFinished" => {
                let Some(rid) = p["requestId"].as_str() else { continue };
                let end_ms = p["timestamp"].as_f64().map(|ts| ((ts - sw0) * 1e3).max(0.0));
                if let Some(e) = net_map.get_mut(rid) {
                    e.end_ms = end_ms;
                    if let Some(em) = end_ms {
                        e.duration_ms = Some((em - e.start_ms).max(0.0));
                    }
                    if let Some(sm) = p["sourceMapURL"].as_str().filter(|s| !s.is_empty()) {
                        source_map_hints.insert(e.url.clone(), sm.to_string());
                    }
                    let m = &p["metrics"];
                    if m.is_object() {
                        e.protocol = m["protocol"].as_str().map(str::to_string).or(e.protocol.clone());
                        e.priority = m["priority"].as_str().map(str::to_string);
                        let hdr = m["responseHeaderBytesReceived"].as_u64().unwrap_or(0);
                        let body = m["responseBodyBytesReceived"].as_u64();
                        if let Some(b) = body {
                            e.encoded_bytes = hdr.saturating_add(b);
                        }
                        if let Some(dec) = m["responseBodyDecodedSize"].as_u64() {
                            e.decoded_bytes = dec;
                        }
                    }
                }
            }
            "Network.loadingFailed" => {
                let Some(rid) = p["requestId"].as_str() else { continue };
                let end_ms = p["timestamp"].as_f64().map(|ts| ((ts - sw0) * 1e3).max(0.0));
                if let Some(e) = net_map.get_mut(rid) {
                    e.end_ms = end_ms;
                    if let Some(em) = end_ms {
                        e.duration_ms = Some((em - e.start_ms).max(0.0));
                    }
                    e.error = p["errorText"].as_str().map(str::to_string);
                    e.canceled = p["canceled"].as_bool().unwrap_or(false);
                }
            }
            _ => {}
        }
    }

    // Build `cpu-0.cpuprofile` from `ScriptProfiler` samples inside `[sw0, sw_end]`.
    let (js_samples, sampling) = build_cpu_profile(&stack_traces, &script_turns, (sw0, sw_end), (t0, t1), &mut scripts, dir)?;

    // Merge overlapping main-thread busy intervals so `main.work` never double-counts nested records.
    busy_spans.sort_by_key(|s| (s.0, s.1));
    let mut merged_busy_ns = 0u64;
    let mut cur: Option<(u64, u64)> = None;
    for (a, b) in busy_spans {
        match cur {
            Some((ca, cb)) if a <= cb => cur = Some((ca, cb.max(b))),
            Some((ca, cb)) => {
                merged_busy_ns += cb - ca;
                cur = Some((a, b));
            }
            None => cur = Some((a, b)),
        }
    }
    if let Some((ca, cb)) = cur {
        merged_busy_ns += cb - ca;
    }

    let dur_ms = (t1.saturating_sub(t0) as f64 / 1e6).max(1e-3);
    let main_work_ms = merged_busy_ns as f64 / 1e6;
    let mut values = BTreeMap::new();
    values.insert("run.duration".into(), dur_ms);
    values.insert("main.work".into(), main_work_ms);
    values.insert("main.busy".into(), (100.0 * main_work_ms / dur_ms).min(100.0));
    values.insert("main.tasks.long".into(), long_tasks.0 as f64);
    if let Some(dom) = page_perf["dom"].as_f64() {
        values.insert("dom.nodes".into(), dom);
    }

    // Latest Memory.trackingUpdate category breakdown + JS heap census.
    let mut layers_map = Map::new();
    let mut mem_samples = Vec::new();
    for m in &memory_events {
        if let Some(ts) = m["timestamp"].as_f64() {
            let at_ns = to_host_ns(ts);
            let mut cats = Map::new();
            for c in m["categories"].as_array().into_iter().flatten() {
                if let (Some(ty), Some(sz)) = (c["type"].as_str(), c["size"].as_f64()) {
                    cats.insert(ty.to_string(), json!(sz));
                }
            }
            if !cats.is_empty() {
                layers_map = cats.clone();
                mem_samples.push(json!({ "t_ns": at_ns, "categories": cats }));
            }
        }
    }
    let js_cat = layers_map.get("javascript").and_then(Value::as_f64);
    let jit_cat = layers_map.get("jit").and_then(Value::as_f64).unwrap_or(0.0);
    let js_used_bytes = heap_self_bytes.map(|b| b as f64).or(js_cat);
    if let Some(js_b) = js_used_bytes {
        values.insert("js.heap.used".into(), js_b / (1024.0 * 1024.0));
    }

    // Add user-timing `performance.measure` spans (`span.<name>`).
    let spans = page_perf["measures"].as_array().cloned().unwrap_or_default();
    for sp in &spans {
        if let (Some(name), Some(ms)) = (sp["name"].as_str(), sp["duration_ms"].as_f64())
            && !name.is_empty()
            && ms.is_finite()
        {
            values.insert(format!("span.{name}"), ms);
        }
    }

    let requests: Vec<NetworkEntry> = net_order.into_iter().filter_map(|id| net_map.remove(&id)).collect();
    let network_bytes: u64 = requests.iter().map(|r| if r.encoded_bytes > 0 { r.encoded_bytes } else { r.decoded_bytes }).sum();
    let network_requests = requests.len();
    values.insert("net.requests".into(), network_requests as f64);
    values.insert("net.bytes".into(), network_bytes as f64);

    let waterfall = NetworkWaterfall { navigation: page_perf["nav"].clone(), paint: page_perf["paints"].as_array().cloned().unwrap_or_default(), spans, requests };
    write_owner_gz(&dir.join(NETWORK_FILE), &waterfall)?;

    let pid_str = app_pid.unwrap_or(1).to_string();
    let mut census = Vec::new();
    if js_used_bytes.is_some() || !layers_map.is_empty() {
        let alloc_list: Vec<Value> = layers_map.iter().map(|(k, v)| json!({ "path": format!("webkit/{k}"), "effective": v, "size": v })).collect();
        let total_cat: f64 = layers_map.values().filter_map(Value::as_f64).sum();
        let mut allocators = Map::new();
        allocators.insert(pid_str.clone(), json!({ "layers": layers_map, "allocators": alloc_list }));
        census.push(json!({
            "engine": "webkit",
            "taken": "at the end of the recording",
            "page_pid": app_pid.unwrap_or(1),
            "js_heap_bytes": js_used_bytes,
            "js_heap_total_bytes": js_cat.map(|j| j + jit_cat).or(js_used_bytes),
            "dom": { "nodes": page_perf["dom"] },
            "processes": [{
                "pid": app_pid.unwrap_or(1),
                "type": "WebContent",
                "name": "com.apple.WebKit.WebContent",
                "private_bytes": if total_cat > 0.0 { json!(total_cat) } else { Value::Null },
                "working_set_bytes": Value::Null,
            }],
            "allocators": allocators,
        }));
    }

    let read_doc = json!({
        "engine": "webkit",
        "token": token,
        "sampling": sampling,
        "windows": [[win_key, t0 as i64, t1 as i64]],
        "parts": {
            "Layout": layout_rows,
            "Gc": gc_rows,
            "Phases": phase_rows,
            "LongTasks": [[cell(long_tasks.0), (long_tasks.0 > 0).then(|| long_tasks.1.to_string())]],
        },
        "slices": slices,
        "memory": mem_samples,
    });
    store::write_json(&dir.join(READ), &read_doc)?;

    Ok(ConvertedEvents { values, sampling, census, scripts, source_map_hints, network_requests, network_bytes, timeline_records, js_samples })
}

fn build_cpu_profile(stack_traces: &[Value], turns: &[(f64, f64)], (sw0, sw_end): (f64, f64), (t0, t1): (u64, u64), scripts: &mut BTreeSet<String>, dir: &Path) -> Result<(u64, Option<Sampling>)> {
    let mut ordered: Vec<(f64, &Value)> = stack_traces.iter().filter_map(|st| Some((st["timestamp"].as_f64()?, st))).filter(|(ts, _)| *ts >= sw0 - 0.002 && *ts <= sw_end + 0.002).collect();
    ordered.sort_by(|a, b| a.0.total_cmp(&b.0));

    let mut nodes: Vec<Value> = vec![json!({
        "id": 1,
        "callFrame": { "functionName": "(root)", "url": "", "lineNumber": -1, "columnNumber": -1, "scriptId": "0" },
    })];
    let mut children: Vec<Vec<u64>> = vec![Vec::new()];
    let mut node_index: HashMap<(usize, String), usize> = HashMap::new();
    let mut intern = |parent: usize, name: &str, url: &str, line0: i64, col0: i64| -> usize {
        let key = (parent, format!("{name}\u{1f}{url}\u{1f}{line0}\u{1f}{col0}"));
        if let Some(&idx) = node_index.get(&key) {
            return idx;
        }
        let idx = nodes.len();
        nodes.push(json!({
            "id": idx + 1,
            "callFrame": {
                "functionName": name,
                "url": url,
                "lineNumber": line0,
                "columnNumber": col0,
                "scriptId": "0",
            }
        }));
        children.push(Vec::new());
        children[parent].push(idx as u64 + 1);
        node_index.insert(key, idx);
        idx
    };
    let idle_id = intern(0, "(idle)", "", -1, -1) as u64 + 1;

    let mut active_gaps_us = Vec::new();
    for pair in ordered.windows(2) {
        let dt = pair[1].0 - pair[0].0;
        if dt > 0.0 && dt <= MAX_SAMPLE_WEIGHT_S {
            active_gaps_us.push(dt * 1e6);
        }
    }
    // JavaScriptCore's SamplingProfiler targets 1000 µs (1 ms) between active samples.
    let sampling = Sampling::of(1000.0, active_gaps_us);
    let default_s = sampling.map_or(0.001, |s| (s.median_us / 1e6).clamp(0.0002, MAX_SAMPLE_WEIGHT_S));

    let mut samples: Vec<u64> = Vec::new();
    let mut weights_us: Vec<i64> = Vec::new();
    let mut cursor_s = sw0;
    let mut js_count = 0u64;

    for (i, (ts, st)) in ordered.iter().enumerate() {
        let s = ts.clamp(sw0, sw_end);
        if s > cursor_s + 0.002 {
            let idle_us = ((s - cursor_s) * 1e6).round() as i64;
            if idle_us > 0 {
                samples.push(idle_id);
                weights_us.push(idle_us);
            }
        }
        let next_sample = ordered.get(i + 1).map(|x| x.0.min(sw_end)).unwrap_or(sw_end);
        let turn_end = turns.iter().find(|&&(a, b)| s >= a - 0.001 && s <= b + 0.001).map(|&(_, b)| b.min(sw_end));
        let limit_s = turn_end.unwrap_or(s + default_s).min(s + MAX_SAMPLE_WEIGHT_S).min(next_sample);
        let w_s = (limit_s - s).max(0.0001);
        let w_us = (w_s * 1e6).round() as i64;

        let frames = st["stackFrames"].as_array().or_else(|| st["frames"].as_array());
        let mut at = 0usize;
        for f in frames.into_iter().flatten() {
            let raw_name = f["name"].as_str().unwrap_or("");
            let url = f["url"].as_str().unwrap_or("");
            let line = f["line"].as_i64().unwrap_or(-1);
            let col = f["column"].as_i64().unwrap_or(-1);
            let valid_pos = line > 0 && line < i64::from(u32::MAX);
            let line0 = if valid_pos { line - 1 } else { -1 };
            let col0 = if valid_pos && col > 0 && col < i64::from(u32::MAX) { col - 1 } else { 0 };
            if !url.is_empty() && url.contains("://") {
                scripts.insert(url.to_string());
                at = intern(at, raw_name, url, line0, if line0 >= 0 { col0 } else { -1 });
            } else {
                let label = if raw_name.is_empty() { "(program)" } else { raw_name };
                at = intern(at, label, "", -1, -1);
            }
        }
        if at == 0 {
            at = intern(0, "(program)", "", -1, -1);
        }
        samples.push(at as u64 + 1);
        weights_us.push(w_us);
        cursor_s = (s + w_s).min(sw_end);
        js_count += 1;
    }

    if sw_end > cursor_s + 0.001 {
        let tail_us = ((sw_end - cursor_s) * 1e6).round() as i64;
        if tail_us > 0 {
            samples.push(idle_id);
            weights_us.push(tail_us);
        }
    }

    for (n, c) in nodes.iter_mut().zip(children) {
        n["children"] = json!(c);
    }
    // `Rec::load_cpu` reads sample `i`'s weight from `timeDeltas[i + 1]`.
    let mut time_deltas = vec![0i64];
    time_deltas.extend(weights_us);
    let start_us = (t0 / 1_000) as i64;
    let end_us = (t1 / 1_000) as i64;
    let profile = json!({
        "nodes": nodes,
        "startTime": start_us,
        "endTime": end_us,
        "samples": samples,
        "timeDeltas": time_deltas,
    });
    store::write_json(&dir.join("cpu-0.cpuprofile"), &profile)?;
    Ok((js_count, sampling))
}

/// Converts a WebKit Heap Snapshot v3 JSON string into V8's `.heapsnapshot` schema, gzipped and
/// owner-only (`0600`), so `heapsnap::read` and `heapsnap::leaks` compute dominators, retained sizes,
/// classes and retainer paths natively. Returns total `self_size` in bytes.
pub fn convert_webkit_heap(raw_json: &str, out_gz: &Path) -> Result<u64> {
    let mut de = serde_json::Deserializer::from_str(raw_json);
    de.disable_recursion_limit();
    let wk: Value = serde::Deserialize::deserialize(&mut de).map_err(|e| Error::msg(format!("WebKit heap snapshot JSON: {e}")))?;

    let snap_type = wk["type"].as_str().unwrap_or("Inspector");
    let stride = if snap_type == "GCDebugging" { 7usize } else { 4usize };
    let class_names: Vec<&str> = wk["nodeClassNames"].as_array().map(|a| a.iter().map(|v| v.as_str().unwrap_or("")).collect()).unwrap_or_default();
    let edge_types: Vec<&str> = wk["edgeTypes"].as_array().map(|a| a.iter().map(|v| v.as_str().unwrap_or("Internal")).collect()).unwrap_or_default();
    let edge_names: Vec<&str> = wk["edgeNames"].as_array().map(|a| a.iter().map(|v| v.as_str().unwrap_or("")).collect()).unwrap_or_default();
    let raw_nodes = wk["nodes"].as_array().ok_or_else(|| Error::msg("WebKit heap snapshot lacks nodes"))?;
    let raw_edges = wk["edges"].as_array().ok_or_else(|| Error::msg("WebKit heap snapshot lacks edges"))?;

    // V8 node_types: 0 hidden, 1 array, 2 string, 3 object, 4 code, 5 closure, 6 regexp, 7 number, 8 native, 9 synthetic, 10 object shape
    let mut strings: Vec<String> = vec![String::new()];
    let mut str_map: HashMap<String, u32> = HashMap::from([(String::new(), 0)]);
    let mut intern_str = |s: &str| -> u32 {
        if let Some(&idx) = str_map.get(s) {
            return idx;
        }
        let idx = strings.len() as u32;
        strings.push(s.to_string());
        str_map.insert(s.to_string(), idx);
        idx
    };

    // Parse nodes: (v8_ntype, name_str_idx, node_id, self_size, is_window_like)
    let mut nodes: Vec<(u8, u32, u32, u32, bool)> = Vec::with_capacity(raw_nodes.len() / stride + 1);
    let mut id_to_ord: HashMap<u64, u32> = HashMap::with_capacity(raw_nodes.len() / stride + 1);
    let mut total_self: u64 = 0;

    for (k, chunk) in raw_nodes.chunks_exact(stride).enumerate() {
        let nid = chunk[0].as_u64().unwrap_or(0);
        let sz = chunk[1].as_u64().unwrap_or(0).min(u32::MAX as u64) as u32;
        let cidx = chunk[2].as_u64().unwrap_or(0) as usize;
        let flags = chunk[3].as_u64().unwrap_or(0);
        let cname = class_names.get(cidx).copied().unwrap_or("Object");

        if k == 0 && nid != 0 {
            nodes.push((9, 0, 0, 0, false));
            id_to_ord.insert(0, 0);
        }
        if id_to_ord.contains_key(&nid) {
            continue;
        }
        let ord = nodes.len() as u32;
        id_to_ord.insert(nid, ord);

        let (v8_type, display_name) = if nid == 0 || cname == "<root>" {
            (9u8, "")
        } else if flags & 4 != 0 {
            // ElementSubtype: DOM node wrapper.
            (8, cname)
        } else if cname == "string" {
            (2, "")
        } else if cname == "Structure" {
            (10, cname)
        } else if matches!(cname, "FunctionExecutable" | "ScriptExecutable" | "EvalExecutable" | "ModuleProgramExecutable" | "CodeBlock" | "UnlinkedFunctionExecutable") {
            (4, cname)
        } else if flags & 1 != 0 {
            (0, cname)
        } else if cname == "Array" {
            (1, cname)
        } else if cname == "Function" || cname.ends_with("Function") {
            (5, if cname == "Function" { "" } else { cname })
        } else if cname == "RegExp" {
            (6, cname)
        } else {
            (3, cname)
        };
        let is_win = matches!(cname, "Window" | "DOMWindow" | "JSWindowProxy" | "GlobalObject");
        let name_idx = intern_str(display_name);
        total_self += u64::from(sz);
        nodes.push((v8_type, name_idx, nid.min(u32::MAX as u64) as u32, sz, is_win));
    }
    if nodes.is_empty() {
        nodes.push((9, 0, 0, 0, false));
        id_to_ord.insert(0, 0);
    }

    let window_name_idx = intern_str("window");
    // V8 edge_types: 0 context, 1 element, 2 property, 3 internal, 4 hidden, 5 shortcut, 6 weak
    let mut adj: Vec<Vec<(u8, u32, u32)>> = vec![Vec::new(); nodes.len()];
    for e in raw_edges.as_chunks::<4>().0 {
        let from_id = e[0].as_u64().unwrap_or(u64::MAX);
        let to_id = e[1].as_u64().unwrap_or(u64::MAX);
        let (Some(&from_ord), Some(&to_ord)) = (id_to_ord.get(&from_id), id_to_ord.get(&to_id)) else { continue };
        if from_ord == to_ord {
            continue;
        }
        let wk_kind = edge_types.get(e[2].as_u64().unwrap_or(0) as usize).copied().unwrap_or("Internal");
        let extra = e[3].as_u64().unwrap_or(0) as u32;
        let (etype, name_or_idx) = match wk_kind {
            "Property" => (2u8, intern_str(edge_names.get(extra as usize).copied().unwrap_or(""))),
            "Index" => (1u8, extra),
            "Variable" => (0u8, intern_str(edge_names.get(extra as usize).copied().unwrap_or(""))),
            _ if from_ord == 0 && nodes[to_ord as usize].4 => (5u8, window_name_idx),
            _ => (3u8, 0u32),
        };
        adj[from_ord as usize].push((etype, name_or_idx, to_ord));
    }

    let node_count = nodes.len();
    let edge_count: usize = adj.iter().map(Vec::len).sum();
    let meta = json!({
        "meta": {
            "node_fields": ["type", "name", "id", "self_size", "edge_count"],
            "node_types": [["hidden", "array", "string", "object", "code", "closure", "regexp", "number", "native", "synthetic", "object shape"]],
            "edge_fields": ["type", "name_or_index", "to_node"],
            "edge_types": [["context", "element", "property", "internal", "hidden", "shortcut", "weak"]],
            "trace_function_info_fields": ["function_id", "name", "script_name", "script_id", "line", "column"],
            "trace_node_fields": ["id", "function_info_index", "count", "size", "children"],
            "location_fields": ["object_index", "script_id", "line", "column"],
        },
        "node_count": node_count,
        "edge_count": edge_count,
        "root_index": 0,
    });

    let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
    write!(gz, "{{\"snapshot\":{},\"nodes\":[", serde_json::to_string(&meta)?)?;
    for (i, (nt, nm, id, sz, _)) in nodes.iter().enumerate() {
        if i > 0 {
            gz.write_all(b",")?;
        }
        write!(gz, "{nt},{nm},{id},{sz},{}", adj[i].len())?;
    }
    gz.write_all(b"],\"edges\":[")?;
    let mut first_e = true;
    for list in &adj {
        for &(et, nm, to_ord) in list {
            if !first_e {
                gz.write_all(b",")?;
            }
            first_e = false;
            write!(gz, "{et},{nm},{}", u64::from(to_ord) * 5)?;
        }
    }
    gz.write_all(b"],\"strings\":[")?;
    for (i, s) in strings.iter().enumerate() {
        if i > 0 {
            gz.write_all(b",")?;
        }
        gz.write_all(serde_json::to_string(s)?.as_bytes())?;
    }
    gz.write_all(b"]}")?;
    paths::write_owner_only(out_gz, &gz.finish()?)?;
    Ok(total_self)
}

fn collect_frame_resources(tree: &Value, default_frame: &mut Option<String>, out: &mut HashMap<String, (String, Option<String>)>) {
    if let Some(fid) = tree["frame"]["id"].as_str() {
        if default_frame.is_none() {
            *default_frame = Some(fid.to_string());
        }
        if let Some(u) = tree["frame"]["url"].as_str().filter(|u| !u.is_empty()) {
            out.entry(u.to_string()).or_insert_with(|| (fid.to_string(), None));
        }
        for r in tree["resources"].as_array().into_iter().flatten() {
            if let Some(u) = r["url"].as_str().filter(|u| !u.is_empty()) {
                let sm = r["sourceMapURL"].as_str().filter(|s| !s.is_empty()).map(str::to_string);
                out.insert(u.to_string(), (fid.to_string(), sm));
            }
        }
    }
    for kid in tree["childFrames"].as_array().into_iter().flatten() {
        collect_frame_resources(kid, default_frame, out);
    }
}

fn resolve_relative_url(base: &str, rel: &str) -> String {
    if rel.contains("://") || rel.starts_with("data:") {
        return rel.to_string();
    }
    let Some((scheme, rest)) = base.split_once("://") else { return rel.to_string() };
    let host_end = rest.find('/').unwrap_or(rest.len());
    let origin = format!("{scheme}://{}", &rest[..host_end]);
    if rel.starts_with('/') {
        return format!("{origin}{rel}");
    }
    let path = rest[host_end..].split(['?', '#']).next().unwrap_or("/");
    let dir = path.rsplit_once('/').map_or("", |(d, _)| d);
    format!("{origin}{dir}/{rel}")
}

fn decode_data_url(url: &str) -> Option<String> {
    let rest = url.strip_prefix("data:")?;
    let (meta, body) = rest.split_once(',')?;
    if meta.ends_with(";base64") {
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD.decode(body.trim()).ok()?;
        String::from_utf8(bytes).ok()
    } else {
        Some(body.to_string())
    }
}

fn load_webkit_text(session: &mut Session, frame_id: &str, url: &str) -> Option<String> {
    if url.starts_with("data:") {
        return decode_data_url(url);
    }
    if let Ok(r) = session.call("Network.loadResource", json!({ "frameId": frame_id, "url": url }), Duration::from_secs(8))
        && r["status"].as_i64().unwrap_or(200) < 400
        && let Some(c) = r["content"].as_str()
        && !c.is_empty()
    {
        return Some(c.to_string());
    }
    if let Ok(r) = session.call("Page.getResourceContent", json!({ "frameId": frame_id, "url": url }), Duration::from_secs(5))
        && let Some(c) = r["content"].as_str()
        && !c.is_empty()
    {
        if r["base64Encoded"].as_bool().unwrap_or(false) {
            use base64::Engine as _;
            return base64::engine::general_purpose::STANDARD.decode(c).ok().and_then(|b| String::from_utf8(b).ok());
        }
        return Some(c.to_string());
    }
    None
}

fn find_sourcemap_comment(script: &str) -> Option<&str> {
    let at = script.rfind("//# sourceMappingURL=").into_iter().chain(script.rfind("//@ sourceMappingURL=")).max()?;
    script[at + 21..].lines().next().and_then(|l| l.split_whitespace().next()).filter(|s| !s.is_empty())
}

fn store_webkit_maps(session: &mut Session, scripts: &BTreeSet<String>, hints: &HashMap<String, String>, dir: &Path, served: Option<&Path>) -> Result<usize> {
    let maps_dir = dir.join("maps");
    std::fs::create_dir_all(&maps_dir)?;
    let mut default_frame = None;
    let mut resources = HashMap::new();
    if let Ok(tree) = session.call("Page.getResourceTree", Value::Null, Duration::from_secs(5)) {
        collect_frame_resources(&tree["frameTree"], &mut default_frame, &mut resources);
    }
    let Some(fallback_frame) = default_frame else { return Ok(0) };

    let mut index = Map::new();
    for (i, url) in scripts.iter().take(48).enumerate() {
        let (frame_id, tree_sm) = resources.get(url).cloned().unwrap_or_else(|| (fallback_frame.clone(), None));
        let Some(script_text) = load_webkit_text(session, &frame_id, url) else { continue };
        let sm_ref = hints.get(url).map(String::as_str).or(tree_sm.as_deref()).or_else(|| find_sourcemap_comment(&script_text));
        let map_url = match sm_ref {
            Some(r) => resolve_relative_url(url, r),
            None => format!("{}.map", url.split(['?', '#']).next().unwrap_or(url)),
        };
        let Some(map_text) = load_webkit_text(session, &frame_id, &map_url) else { continue };
        if !map_text.trim_start_matches('\u{feff}').trim_start().starts_with('{') {
            continue;
        }
        let mut lines = vec![0i64];
        for (pos, b) in script_text.bytes().enumerate() {
            if b == b'\n' {
                lines.push(pos as i64 + 1);
            }
        }
        let map_rel = format!("maps/{i}.map.gz");
        let scopes_rel = format!("maps/{i}.scopes.json.gz");
        write_owner_raw_gz(&dir.join(&map_rel), map_text.as_bytes())?;
        let scopes_body = json!({ "lines": lines, "functions": [] });
        write_owner_raw_gz(&dir.join(&scopes_rel), scopes_body.to_string().as_bytes())?;

        let at_url = if map_url.starts_with("data:") { url.as_str() } else { map_url.as_str() };
        let url_path = at_url.split_once("://").and_then(|(_, r)| r.find('/').map(|j| &r[j..])).map(|p| p.split(['?', '#']).next().unwrap_or(p));
        let map_dir = served.zip(url_path).map(|(root, p)| root.join(p.trim_start_matches('/'))).and_then(|p| p.parent().map(|d| d.display().to_string()));
        index.insert(url.clone(), json!({ "map": map_rel, "map_url": map_url, "map_dir": map_dir, "scopes": scopes_rel }));
    }
    let n = index.len();
    store::write_json(&maps_dir.join("index.json"), &Value::Object(index))?;
    Ok(n)
}

fn write_owner_raw_gz(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
    gz.write_all(bytes)?;
    paths::write_owner_only(path, &gz.finish()?)?;
    Ok(())
}
