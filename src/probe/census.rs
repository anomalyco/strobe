//! A census: what memory the target holds at one moment, by process and layer (SPEC §8.6).

use super::trace;
use crate::engine;
use crate::host;
use crate::model::{Error, Result};
use crate::target::chromium::{Browser, Page};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

/// The page's live JS heap (after a forced GC, so garbage does not count) and DOM counters, and every
/// target process's OS memory with its Chromium process type. Take it before a heap snapshot of the
/// same process: serializing a snapshot raises that process's memory.
pub fn take(browser: &Browser, page: &Page) -> Value {
    take_at(browser, page).0
}

/// The least undisturbed time `js.heap.growth` and `dom.nodes.growth` are measured over: under it a
/// rate per minute would stretch what the page did more than 30 times.
pub const MIN_GROWTH: Duration = Duration::from_secs(2);

/// Why the growth rates of a recording whose page ran `ran` undisturbed are refused, if they are.
pub fn growth_refusal(ran: Duration) -> Option<crate::model::Refusal> {
    (ran < MIN_GROWTH).then(|| {
        crate::model::Refusal::new(
            crate::model::ValidatorId::RunIncomplete,
            format!("the page ran {:.2} s undisturbed between strobe's snapshots, under {} s: a rate per minute would stretch that more than 30 times", ran.as_secs_f64(), MIN_GROWTH.as_secs()),
        )
        .fix("record with --for 2s or longer: the page's undisturbed time is the recording's length")
    })
}

/// The page's live JS heap (bytes, after a forced GC) and DOM nodes, and the moment after they were
/// read (the page runs again from there): a census's page part alone, a growth window's start.
pub fn page_reading(page: &Page) -> (Option<f64>, Option<f64>, Instant) {
    let _ = page.send("HeapProfiler.collectGarbage", json!({}));
    let heap = page.send("Runtime.getHeapUsage", json!({})).ok().and_then(|h| h["usedSize"].as_f64());
    let nodes = page.send("Memory.getDOMCounters", json!({})).ok().and_then(|d| d["nodes"].as_f64());
    (heap, nodes, Instant::now())
}

/// [`take`], with the moment its page readings stand for: just before its forced collection, which
/// stops the page until they are read, so a growth window ends there.
pub fn take_at(browser: &Browser, page: &Page) -> (Value, Instant) {
    let _ = page.send("HeapProfiler.enable", json!({}));
    // strobe's own listeners in the page (its input counters, `window.__strobeListeners`), read before
    // the collection: the DOM's listener count is the page's alone without them.
    let own = page.send("Runtime.evaluate", json!({ "expression": "window.__strobeListeners ?? 0", "returnByValue": true })).ok().and_then(|r| r["result"]["value"].as_u64()).unwrap_or(0);
    let read = Instant::now();
    let _ = page.send("HeapProfiler.collectGarbage", json!({}));
    let heap = page.send("Runtime.getHeapUsage", json!({})).unwrap_or(Value::Null);
    let mut dom = page.send("Memory.getDOMCounters", json!({})).unwrap_or(Value::Null);
    if let Some(n) = dom["jsEventListeners"].as_u64() {
        dom["jsEventListeners"] = json!(n.saturating_sub(own));
    }
    // A phone's Chrome runs on the phone: no process of this computer's is its (null, never none).
    let processes = if browser.phone.is_some() {
        Value::Null
    } else {
        let types: BTreeMap<u64, String> = browser
            .cdp
            .send("SystemInfo.getProcessInfo", json!({}), None)
            .ok()
            .and_then(|r| r["processInfo"].as_array().map(|a| a.iter().filter_map(|p| Some((p["id"].as_u64()?, p["type"].as_str()?.to_string()))).collect()))
            .unwrap_or_default();
        let list: Vec<Value> = browser
            .processes()
            .iter()
            .map(|p| json!({ "pid": p.pid, "name": p.name, "type": types.get(&(p.pid as u64)).map_or("other", String::as_str), "private_bytes": p.private_bytes, "working_set_bytes": p.working_set }))
            .collect();
        Value::Array(list)
    };
    let v = json!({
        "at_ns": host::now_ns(),
        "js_heap_bytes": heap["usedSize"], "js_heap_total_bytes": heap["totalSize"],
        "dom": { "documents": dom["documents"], "nodes": dom["nodes"], "listeners": dom["jsEventListeners"] },
        "processes": processes,
    });
    (v, read)
}

/// Chromium's own accounting by allocator (the census's "Chromium allocators" layer): a detailed
/// memory-infra dump of every process, through a short trace of its own written to `path`. Sizes are
/// effective sizes, with the dump's ownership edges resolved, so allocators that overlap (cc, skia and
/// gpu inside malloc, discardable or shared memory) never count the same bytes twice. Per pid:
/// `layers` (top-level allocators) and `allocators` (their children, largest first), into the census's
/// `allocators`; and its `page_pid`, the page's renderer: a mark the page makes in the dump's trace
/// names its process, so `query memory` never guesses it from sizes.
pub fn add_allocators(c: &mut Value, browser: &Browser, page: &Page, path: &Path) {
    match dump(browser, page, path) {
        Ok((allocators, pid)) => {
            c["allocators"] = allocators;
            if let Some(pid) = pid {
                c["page_pid"] = json!(pid);
            }
        }
        Err(e) => c["allocators"] = json!({ "error": e.message }),
    }
}

fn dump(browser: &Browser, page: &Page, path: &Path) -> Result<(Value, Option<u64>)> {
    // The mark goes into the dump's own trace from the page's main thread: its process is the page's.
    let mark = format!("strobe:census:{}", crate::store::new_id("m"));
    let tracing = trace::start_categories(&browser.cdp, &["disabled-by-default-memory-infra", "blink.user_timing"], 64_000)?;
    let _ = page.send("Runtime.evaluate", json!({ "expression": format!("performance.mark({mark:?}), performance.clearMarks({mark:?}), true") }));
    let dumped = browser.cdp.send_timeout("Tracing.requestMemoryDump", json!({ "deterministic": true, "levelOfDetail": "detailed" }), None, Duration::from_secs(60));
    tracing.stop(path)?;
    if !dumped?["success"].as_bool().unwrap_or(false) {
        return Err(Error::msg("Chromium did not complete the memory dump"));
    }
    // One pass over the trace: the dump's nodes, and the mark's process as a row of its own (`PAGE`).
    const PAGE: &str = "(strobe: page mark)";
    let table = engine::query(
        path,
        &format!(
            "select p.pid, n.path, n.size, n.effective_size, coalesce(p.name, '')
           from memory_snapshot_node n
           join process_memory_snapshot s on n.process_snapshot_id = s.id
           join process p using(upid)
          where p.pid is not null
         union all
         select distinct p.pid, '{PAGE}', null, null, null
           from slice s join thread_track tt on s.track_id = tt.id join thread t using(utid) join process p using(upid)
          where s.name = '{mark}' and p.pid is not null"
        ),
    )?;
    let marked: Vec<u64> = table.rows.iter().filter(|r| r.get(1).cloned().flatten().as_deref() == Some(PAGE)).filter_map(|r| r.first().cloned().flatten()?.parse().ok()).collect();
    // Exactly one process made the mark, or the census names none.
    let page_pid = match marked.as_slice() {
        [p] => Some(*p),
        _ => None,
    };
    /// Per pid: top-level allocators (layers) by effective size, their children as (path, size,
    /// effective), and the process's name in the dump's trace (`Browser`, `Renderer`, `GPU Process`,
    /// `Service: <name>.mojom.<Service>`): a phone's census has no other.
    type Dump = (BTreeMap<String, f64>, Vec<(String, f64, f64)>, String);
    let mut by_pid: BTreeMap<String, Dump> = BTreeMap::new();
    for r in table.rows.iter().filter(|r| r.get(1).cloned().flatten().as_deref() != Some(PAGE)) {
        let cell = |i: usize| r.get(i).cloned().flatten().unwrap_or_default();
        let (pid, node) = (cell(0), cell(1));
        let (size, effective) = (cell(2).parse::<f64>().unwrap_or(0.0), cell(3).parse::<f64>().unwrap_or(0.0));
        let entry = by_pid.entry(pid).or_default();
        if entry.2.is_empty() {
            entry.2 = cell(4);
        }
        match node.matches('/').count() {
            0 => {
                entry.0.insert(node, effective);
            }
            1 => entry.1.push((node, size, effective)),
            _ => {}
        }
    }
    let out: serde_json::Map<String, Value> = by_pid
        .into_iter()
        .map(|(pid, (layers, mut children, name))| {
            children.sort_by(|a, b| b.2.total_cmp(&a.2));
            children.truncate(40);
            let children: Vec<Value> = children.into_iter().map(|(p, s, e)| json!({ "path": p, "size": s, "effective": e })).collect();
            (pid, json!({ "name": name, "layers": layers, "allocators": children }))
        })
        .collect();
    Ok((Value::Object(out), page_pid))
}
