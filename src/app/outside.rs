//! `strobe record pid:<n>|proc:<name>` (SPEC §13.6): any running process, from the outside. Timing:
//! what the OS put on the screen for its tree (present events), its CPU (cycle counters), its threads'
//! wakes and CPU, IO, GPU, and what holds its memory: the views a launched native app gets.
//! `--probes deep`: native stacks from the kernel's sampled profile (attribution, through the helper).

use super::{Output, fmt, progress};
use crate::cli::{Args, duration_ms};
use crate::host;
use crate::model::{Error, Refusal, Result, ValidatorId};
use crate::probe::present::{self, frames};
use crate::probe::stacks::StackCapture;
use crate::probe::{self, wakes};
use crate::store;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::Duration;

/// What this OS gives strobe no way to read for another process (SPEC §13.6), as the refusal that
/// says so (`NotOnThisOs`). None: the OS has a source, which the caller reads.
pub fn not_on_this_os(metric: &str) -> Option<Refusal> {
    let mac = cfg!(target_os = "macos");
    let why = match metric {
        "gpu.vram" | "gpu.shared" if mac => {
            "macOS: Apple GPUs share system memory, so a process's GPU allocations are part of its physical footprint (memory.app.private); there is no VRAM per process"
        }
        "memory.app.host" if mac => "macOS: memory by what holds it reads another process's regions, which takes its task port (debuggers only); memory.app.private is its physical footprint",
        "ui.wakes" if mac => "macOS counts context switches per process, not per thread (no public per-thread count): cpu.wakes has the process's",
        "cpu.foreign" | "cpu.foreign.max" if mac => "macOS has no CPU affinity, so no CPUs are the target's own to measure other work on",
        "memory.app.pss" if !cfg!(target_os = "linux") => {
            "the proportional set size (each shared page split between the processes that map it) is Linux's (smaps_rollup); memory.app.private is this OS's private memory"
        }
        // Every metric made of cycles over the cycle clock (`host::tsc`).
        "cpu.work" | "cpu.app.total" | "cpu.process.total" | "ui.cpu" | "cpu.renderer" | "cpu.gpu" | "cpu.browser" | "cpu.foreign" | "cpu.foreign.max" | "cpu.fixture" => {
            return host::cycles_refused().map(|why| Refusal::new(ValidatorId::NotOnThisOs, format!("{metric}: {why} (process and thread times move in 15.6 ms ticks, too coarse to use)")));
        }
        _ => return None,
    };
    Some(Refusal::new(ValidatorId::NotOnThisOs, format!("{metric}: {why}")))
}

/// Whether the OS sampler read the GPU of every process it followed: its per-process source opened and
/// no process's statistics were unreadable (otherwise the GPU fields are empty, never a partial sum).
pub fn gpu_read(s: &probe::os::Samples) -> bool {
    s.gpu_available && s.gpu_unreadable.is_empty()
}

/// Why the GPU fields are empty: the per-process source did not open, or some processes' statistics
/// could not be read (`unreadable`).
pub fn gpu_unread(metric: &str, unreadable: &[u32]) -> Refusal {
    let source = if cfg!(windows) {
        "the GPU's per-process statistics (D3DKMT)"
    } else if cfg!(target_os = "macos") {
        "the GPU's per-client statistics (IOAccelerator)"
    } else {
        "the GPU driver's per-client statistics (DRM fdinfo; NVIDIA's proprietary driver and virtual GPUs publish none)"
    };
    let why = match unreadable {
        [] => format!("{source} did not open"),
        pids => format!("{source} of processes {} could not be read, so a sum would leave them out", pids.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")),
    };
    Refusal::new(ValidatorId::NotOnThisOs, format!("{metric}: {why}"))
}

/// The refusals (`<validator>: <why>`) of what a process recording on this OS cannot hold, for its
/// meta and summary: a missing number is never shown as if nothing were there. `gpu`: None when the
/// sampler read every process's GPU, else the processes it could not read.
pub fn os_refusals(has: &dyn Fn(&str) -> bool, gpu: Option<&[u32]>) -> serde_json::Map<String, Value> {
    let mut out = serde_json::Map::new();
    for m in ["cpu.work", "cpu.app.total", "ui.cpu", "gpu.process.3d", "gpu.vram", "gpu.shared", "memory.app.host", "ui.wakes"] {
        // A UI thread's wakes are missing only beside its CPU: without a window it has neither.
        if has(m) || (m == "ui.wakes" && !has("ui.cpu")) {
            continue;
        }
        let r = not_on_this_os(m).or_else(|| gpu.filter(|_| m.starts_with("gpu.")).map(|u| gpu_unread(m, u)));
        if let Some(r) = r {
            out.insert(m.into(), json!(r.message()));
        }
    }
    out
}

/// `cpu.wakes` refused (`NoCounter`) in a recording's refusals when the OS stopped counting a
/// process's switches (`Span::uncounted`): never left out unsaid, never 0/s.
pub fn wakes_refused(span: &wakes::Span, refused: &mut serde_json::Map<String, Value>) {
    if let Some(why) = &span.uncounted {
        refused.insert("cpu.wakes".into(), json!(Refusal::new(ValidatorId::NoCounter, format!("cpu.wakes: {why}")).message()));
    }
}

/// `pid:<n>`, or `proc:<name>`: the one process of that name whose parent is not of that name.
pub fn resolve(target: &str) -> Result<u32> {
    if let Some(p) = target.strip_prefix("pid:") {
        let pid: u32 = p.parse().map_err(|_| Error::usage(format!("not a pid: {p}")))?;
        if !host::all_processes().iter().any(|x| x.pid == pid) {
            return Err(Error::usage(format!("no process {pid}")));
        }
        return Ok(pid);
    }
    let name = target.strip_prefix("proc:").unwrap_or(target).to_ascii_lowercase();
    let all = host::all_processes();
    let is = |n: &str| {
        let n = n.to_ascii_lowercase();
        n == name || n.strip_suffix(".exe") == Some(name.as_str())
    };
    let matching: Vec<&host::ProcInfo> = all.iter().filter(|p| is(&p.name)).collect();
    let roots: Vec<&&host::ProcInfo> = matching.iter().filter(|p| !matching.iter().any(|q| q.pid == p.parent)).collect();
    match roots.as_slice() {
        [one] => Ok(one.pid),
        [] => Err(Error::usage(format!("no process named {name}"))),
        many => Err(Error::usage(format!("{} processes named {name}: {}", many.len(), many.iter().map(|p| format!("pid:{}", p.pid)).collect::<Vec<_>>().join(", ")))),
    }
}

pub fn record(args: &Args, target: &str) -> Result<Output> {
    let root = resolve(target)?;
    let name = host::all_processes().into_iter().find(|p| p.pid == root).map(|p| p.name).unwrap_or_default();
    let deep = match args.get("probes").unwrap_or("timing") {
        "timing" => false,
        "deep" | "attribution" | "stacks-unprivileged" | "waits" => true,
        other => return Err(Error::usage(format!("--probes for a process: timing, deep, waits (deep with off-CPU waits) or stacks-unprivileged, not {other}"))),
    };
    let for_ms = match args.get("for") {
        Some(d) => duration_ms(d)?,
        None => 10_000,
    };
    let awake = host::Awake::start();
    let (id, draft) = super::record::claim(args)?;
    let dir = draft.dir().to_path_buf();
    // The lock screen covers every window: nothing reaches the display and presents stop. Stacks,
    // CPU and GPU work need no display, so only frames are refused then (SPEC §10.5).
    let locked = |awake: &host::Awake| awake.locks() > 0 || host::session_locked();
    if deep {
        let waits = args.get("probes") == Some("waits");
        // What the capture costs the target (rule 2): its threads' context switches and CPU over the
        // capture, then just after it (the capture starts at once: no wait before it). Sampling stacks
        // (and taking waits, at every switch) slows a busy target: a rate lower during the capture than
        // after it says the stacks describe a slower process than the one recorded.
        let tree = || host::process_set(root, None).iter().map(|p| p.pid).collect::<Vec<u32>>();
        // The tree's CPU around the capture, for what share of it the samples cover (unknown when a
        // process of it ended inside: a pid: target has no job or cgroup that keeps its CPU).
        let set = || super::record::read_set(&host::process_set(root, None), None);
        let (c0, w0) = (set(), host::now_ns());
        let cap = StackCapture::start_with(crate::probe::stacks::Targets::new(root, None), args.get("probes") == Some("stacks-unprivileged"), waits)?;
        progress(format!("sampling native stacks of {name} ({root}) for {}", fmt::ms_duration(for_ms)));
        let a = wakes::Snapshot::take(&tree());
        std::thread::sleep(Duration::from_millis(for_ms));
        let during = wakes::span(&a, &wakes::Snapshot::take(&tree()));
        let d = cap.stop()?;
        let (c1, w1) = (set(), host::now_ns());
        let set_cpu_ms = match (super::record::set_used(&c0, &c1), c0.2, c1.2) {
            (Some(used), Some(a), Some(b)) if b > a => Some(used as f64 / (b - a) as f64 * (w1 - w0) as f64 / 1e6),
            _ => None,
        };
        let after = {
            let a = wakes::Snapshot::take(&tree());
            std::thread::sleep(Duration::from_millis(for_ms.clamp(200, 500)));
            wakes::span(&a, &wakes::Snapshot::take(&tree()))
        };
        let cost = capture_cost(&during, &after, d.waits.is_some());
        // Waits asked for and not taken refuse the recording, saying why (SPEC §11.1).
        if waits && d.waits.is_none() {
            return Err(d.waits_refused.clone().unwrap_or_else(|| crate::probe::stacks::no_waits("no waits were taken", "try again")).into());
        }
        crate::symbol::write_gz(&dir.join("deep.json.gz"), &d)?;
        progress("symbolizing (PDBs: local, then symbol servers)");
        let native = crate::symbol::native(&dir, None, None)?;
        let facts = native.as_ref().map(crate::symbol::summary).unwrap_or(Value::Null);
        let meta = json!({
            "id": id, "kind": "record", "target": target, "pid": root, "process": name, "probes": "deep", "token": "", "duration_ms": for_ms,
            "env": host::env(), "values": {}, "native": facts, "samples": d.samples.len(), "events_lost": d.events_lost, "pids": d.pids,
            "timing": "perturbed", "created": super::record::now_iso(), "valid": true, "suspend": d.suspend,
            "waits": d.waits.as_ref().map(|w| w.waits.len()), "set_cpu_ms": set_cpu_ms,
            "capture_cost": cost,
        });
        store::write_json(&dir.join("meta.json"), &meta)?;
        let top = super::query::native_hotspots(&dir, "function", 8, None, None).map(|o| o.text).unwrap_or_default();
        draft.keep();
        let text = format!(
            "{id} · {target} ({name}) · {} · deep{}\n{top}{}next     strobe query {id} hotspots --by package · strobe query {id} hotspots --thread <name>{} · strobe report recording {id} --open\n",
            fmt::ms_duration(for_ms),
            if d.waits.is_some() { " with off-CPU waits" } else { "" },
            cost_lines(&meta["capture_cost"]),
            if d.waits.is_some() { format!(" · strobe query {id} waits") } else { String::new() }
        );
        return Ok(Output::new(text, meta));
    }
    // A process strobe did not launch has no home to find its detached processes by: its set is its
    // tree (`process_set` with no home).
    let set = move || host::process_set(root, None);
    let pids_now = move || set().iter().map(|p| p.pid).collect::<Vec<u32>>();
    let presents = present::PresentCapture::start().map(|mut p| {
        p.track(&pids_now());
        p
    });
    let via = presents.as_ref().map(|p| p.via()).unwrap_or("");
    let sampler = probe::os::Sampler::start(root, None);
    let cycles = || super::record::read_set(&set(), None);
    // The tree's threads around the window: wakes and CPU per thread, as for a launched native app.
    let snap0 = wakes::Snapshot::take(&pids_now());
    let timeline = wakes::Timeline::start(pids_now);
    let procs0 = (host::now_ns(), host::all_processes());
    // The process's main thread (`cpu.main`): the thread it started with, still running.
    let main = host::MainThread::attach(root);
    let read_main = || -> std::result::Result<host::MainCpu, host::MainLost> { main.as_ref().map_err(Clone::clone)?.read() };
    // The tree's processes, listed (milliseconds) outside the window: those listed are read again right
    // at its edges, each by itself, with the clocks, and the end's listing only finds those born
    // meanwhile (`record::read_known`).
    let mut c0 = cycles();
    timeline.mark_edge();
    // Read just outside the window's edges (on Linux a reading may wait up to a tick), so it never
    // lengthens the window: its share of the time between the two readings is `cpu.main.busy`.
    let m0 = read_main();
    let at0 = super::record::read_known(c0.0.keys());
    super::record::read_at(&mut c0.0, &at0);
    let clock0 = super::record::Clock::now();
    let t0 = clock0.ns;
    progress(format!("recording {name} ({root}) for {}", fmt::ms_duration(for_ms)));
    std::thread::sleep(Duration::from_millis(for_ms));
    let at1 = super::record::read_known(c0.0.keys());
    let clock1 = super::record::Clock::now();
    let t1 = clock1.ns;
    let m1 = read_main();
    timeline.mark_edge();
    let mut c1 = cycles();
    super::record::read_at(&mut c1.0, &at1);
    let screen = super::bench::screen_state(&procs0, &(host::now_ns(), host::all_processes()));
    let mut timeline = timeline.stop();
    timeline.marks.push(("recording".into(), t0, t1));
    crate::symbol::write_gz(&dir.join("timeline.json.gz"), &timeline)?;
    let tree = set();
    let pids: Vec<u32> = tree.iter().map(|p| p.pid).collect();
    let span = wakes::span(&snap0, &wakes::Snapshot::take(&pids));
    let samples = sampler.stop();
    let gpu_missing = (!gpu_read(&samples)).then(|| samples.gpu_unreadable.clone());
    let os = samples.window(t0, t1);
    let presents = presents.and_then(|p| p.stop());
    let session_locked = locked(&awake);
    let ms = (t1 - t0) as f64 / 1e6;
    let mut values: BTreeMap<String, f64> = BTreeMap::new();
    // A process of the tree that ended in the window took its CPU there with it: none then.
    let used = super::record::set_used(&c0, &c1);
    let lost = used.is_none();
    if let (Some(rate), Some(used)) = (clock0.per_ms(&clock1), used) {
        let cpu = used as f64 / rate;
        values.insert("cpu.app.total".into(), 100.0 * cpu / ms);
        values.insert("cpu.work".into(), cpu);
    }
    let main_lost = match m0.and_then(|a| m1.map(|b| [a, b])) {
        Ok(at) => match host::main_share(&at) {
            Some(share) => {
                values.insert("cpu.main".into(), share * ms);
                values.insert("cpu.main.busy".into(), 100.0 * share);
                None
            }
            None => Some(host::MainLost::Gone("the CPU clock gave no rate over the window".into())),
        },
        Err(lost) => Some(lost),
    };
    if !span.threads.is_empty() {
        if span.uncounted.is_none() {
            values.insert("cpu.wakes".into(), span.wakes_per_s());
        }
        values.insert("io.read".into(), span.io_read as f64);
        values.insert("io.write".into(), span.io_write as f64);
    }
    values.extend(wakes::counter_values(&span.counted).into_iter().map(|(m, v)| (m.to_string(), v)));
    // The UI thread: the thread that owns the tree's main window, when it has one.
    let ui_tid = host::main_window(&pids).and_then(|w| host::window_thread(w.hwnd));
    if let Some(t) = ui_tid.and_then(|tid| span.threads.iter().find(|t| t.tid == tid)) {
        if let Some(c) = t.cpu_ms {
            values.insert("ui.cpu".into(), c);
        }
        if let Some(n) = t.switches.filter(|_| not_on_this_os("ui.wakes").is_none()) {
            values.insert("ui.wakes".into(), n as f64 / (ms / 1e3));
        }
    }
    let names: BTreeMap<u32, String> = span.threads.iter().filter_map(|t| Some((t.tid, host::thread_name(t.tid)?))).collect();
    // The busiest threads by CPU, for the summary: per-thread CPU needs no permission on any OS.
    let mut busy: Vec<&wakes::ThreadSpan> = span.threads.iter().filter(|t| t.tid != 0 && t.cpu_ms.is_some_and(|c| c > 0.0)).collect();
    busy.sort_by(|a, b| b.cpu_ms.unwrap_or(0.0).total_cmp(&a.cpu_ms.unwrap_or(0.0)));
    let threads_top: Vec<Value> = busy.iter().take(6).map(|t| json!({ "tid": t.tid, "pid": t.pid, "name": names.get(&t.tid), "cpu_ms": t.cpu_ms })).collect();
    let spans = std::slice::from_ref(&span);
    store::write_json(&dir.join("threads.json"), &json!(wakes::by_thread(spans, &names, &wakes::modules(spans))))?;
    store::write_json(
        &dir.join("runs.json"),
        &json!([{ "run": "run:0", "ms": ms, "switches": span.uncounted.is_none().then_some(span.switches), "wakes_per_s": span.wakes_per_s(), "io_read": span.io_read, "io_write": span.io_write }]),
    )?;
    // What holds the tree's memory, read after the window.
    values.insert("memory.app.private".into(), tree.iter().map(|p| p.private_bytes).sum::<u64>() as f64 / (1024.0 * 1024.0));
    let reading = probe::memory::Reading::take(&tree, "after the recording");
    values.extend(reading.values().into_iter().map(|(k, v)| (k.to_string(), v)));
    store::write_json(&dir.join("memory.json"), &json!({ "readings": [&reading] }))?;
    if let Some(p) = os.gpu_3d_pct {
        values.insert("gpu.process.3d".into(), p);
    }
    if let Some(v) = os.vram_dedicated_mb {
        values.insert("gpu.vram".into(), v);
    }
    if let Some(v) = os.vram_shared_mb {
        values.insert("gpu.shared".into(), v);
    }
    // Presents far faster than the display refreshes, locked or not (`UnpacedSpin`).
    // The display the process shows on: its main window's, else any window's (an app that shows only
    // an overlay, such as a HUD, has no main window).
    let display_of = |pids: &[u32]| host::main_window(pids).map(|w| w.hwnd).or_else(|| host::top_windows(pids).first().map(|w| w.0)).and_then(host::display_of);
    // The adapter the tree drew on over the window, and the one driving its display (`screen.gpu`).
    let mut screen = screen;
    if let Some(g) = super::bench::drawing_gpu(&samples, &[&os], display_of(&pids).as_ref()) {
        screen["gpu"] = g;
    }
    let unpaced = presents.as_ref().ok().and_then(|p| {
        let pids = pids_now();
        let display = display_of(&pids);
        let hz = frames::refresh_ms(&p.vsyncs, t0, t1, display.as_ref()).map(|ms| 1000.0 / ms).or(display.map(|d| d.hz));
        frames::spin(&p.presents, &pids, &[frames::Timed::whole(t0, t1)], hz)
    });
    let mut session = Value::Null;
    let refused = match &presents {
        _ if session_locked => Some("SessionLocked: the session was locked while strobe recorded, so nothing reached the display".to_string()),
        Ok(p) => {
            let pids = pids_now();
            let display = display_of(&pids);
            let f = frames::native_values(p, &pids, &[frames::Timed::whole(t0, t1)], display.as_ref());
            match f.refused {
                Some(why) if display.is_none() && why.contains("several displays") => {
                    Some("NoPresentSource: several displays run at different refresh rates and the process has no window to say which it is on".to_string())
                }
                Some(why) => Some(why),
                None => {
                    values.extend(f.values);
                    crate::symbol::write_gz(&dir.join("presents.json.gz"), p)?;
                    session = json!({ "via": via, "events": p.events, "events_lost": p.events_lost, "consumer_cpu_ms": p.consumer_cpu_ms, "presents": p.presents.len(), "pids": pids, "display": display, "refused_metrics": f.partial });
                    None
                }
            }
        }
        Err(e) => Some(e.message.clone()),
    };
    let mut os_refused = os_refusals(&|m| values.contains_key(m), gpu_missing.as_deref());
    if lost {
        for m in ["cpu.work", "cpu.app.total"] {
            os_refused.insert(m.into(), json!(super::record::cpu_lost(m, 1, 1, "the process tree").message()));
        }
    }
    wakes_refused(&span, &mut os_refused);
    if let Some(lost) = &main_lost {
        for m in ["cpu.main", "cpu.main.busy"] {
            os_refused.insert(m.into(), json!(lost.refusal(m).message()));
        }
    }
    let meta = json!({
        "id": id, "kind": "record", "target": target, "pid": root, "process": name, "probes": "timing", "token": "", "duration_ms": for_ms,
        "env": host::env(), "values": values, "refused": os_refused, "os": os, "presents": session, "presents_refused": refused, "session_locked": session_locked, "ui_thread": ui_tid,
        "display_settings_hz": host::refresh_at(0, 0), "timing": "clean", "created": super::record::now_iso(), "valid": true, "screen": screen,
        "unpaced": unpaced.map(|(fps, hz)| json!({ "fps": fps, "hz": hz })), "threads_top": threads_top,
        "main_thread": main.as_ref().ok().map(|m| m.tid),
    });
    store::write_json(&dir.join("meta.json"), &meta)?;
    draft.keep();
    Ok(Output::new(super::record::summary_text(&meta), meta))
}

/// A deep capture's cost to the target (`capture_cost` in its meta): its tree's context switches a
/// second and CPU (% of a core) over the capture and just after it. A rate lower during the capture by
/// more than a quarter says the capture slowed the target (perf's or the helper's work at each switch
/// and sample).
fn capture_cost(during: &wakes::Span, after: &wakes::Span, waits: bool) -> Value {
    let cpu = |s: &wakes::Span| s.threads.iter().map(|t| t.cpu_ms).sum::<Option<f64>>().filter(|_| s.ms > 0.0).map(|c| 100.0 * c / s.ms);
    let rate = |s: &wakes::Span| Some(s.wakes_per_s()).filter(|x| x.is_finite());
    let side = |s: &wakes::Span| json!({ "ms": s.ms, "switches_per_s": rate(s), "cpu_pct": cpu(s) });
    let fell = rate(after).zip(rate(during)).filter(|(a, d)| *a >= 100.0 && *d < 0.75 * a).map(|(a, d)| 100.0 * (1.0 - d / a));
    json!({ "during": side(during), "after": side(after), "waits": waits, "switches_fell_pct": fell })
}

/// `capture_cost`'s lines: `cost`, and `! CaptureCost` when the rate fell.
pub fn cost_lines(c: &Value) -> String {
    if !c.is_object() {
        return String::new();
    }
    let shown = |s: &Value| {
        format!(
            "{} context switches, {} of a core",
            s["switches_per_s"].as_f64().map_or("?".to_string(), |r| fmt::value(r, "/s")),
            s["cpu_pct"].as_f64().map_or("?".to_string(), |p| fmt::value(p, "%"))
        )
    };
    let mut text =
        format!("cost     the tree during the capture: {} · after it ({}): {}\n", shown(&c["during"]), fmt::ms_duration(c["after"]["ms"].as_f64().unwrap_or(0.0) as u64), shown(&c["after"]));
    if let Some(pct) = c["switches_fell_pct"].as_f64() {
        let waits = c["waits"] == true;
        text += &format!(
            "! CaptureCost  the target switched {pct:.0}% less often a second during the capture than just after it: the capture slows a switch-heavy process (unless its own work changed then), and its stacks{} describe that slower process{}\n",
            if waits { " and waits" } else { "" },
            if waits { " (waits cost the target at every switch: --probes deep takes none)" } else { "" }
        );
    }
    text
}

pub fn text(meta: &Value) -> String {
    let v = &meta["values"];
    let get = |k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(f64::NAN);
    let id = meta["id"].as_str().unwrap_or("");
    let mut s = format!("{id} · {} ({}) · {} · timing\n", meta["target"].as_str().unwrap_or(""), meta["process"].as_str().unwrap_or(""), fmt::ms_duration(meta["duration_ms"].as_u64().unwrap_or(0)));
    match meta["presents_refused"].as_str() {
        Some(why) => s += &format!("frames   refused: {why}\n"),
        None => {
            s += &format!(
                "frames   {} displayed · {} dropped per run ({} of frames) · {} discarded · display {}\n",
                fmt::value(get("frames.n"), "count"),
                fmt::value(get("frames.dropped"), "count"),
                fmt::value(get("frames.dropped.pct"), "%"),
                fmt::value(get("frames.discarded"), "count"),
                if get("display.hz").is_finite() { format!("{:.1} Hz", get("display.hz")) } else { "?".into() },
            );
            s += &format!(
                "interval p50 {} · p95 {} · max {}   (gap between frames on screen while drawing)\n",
                fmt::value(get("frames.interval.p50"), "ms"),
                fmt::value(get("frames.interval.p95"), "ms"),
                fmt::value(get("frames.interval.max"), "ms"),
            );
            let p = &meta["presents"];
            let cpu = p["consumer_cpu_ms"].as_f64();
            let dur = meta["duration_ms"].as_f64().unwrap_or(1.0).max(1.0);
            s += &format!(
                "presents {} events · {} lost · {} · consumer {}\n",
                p["events"],
                p["events_lost"],
                p["via"].as_str().unwrap_or(""),
                cpu.map(|c| format!("{c:.1} ms CPU ({:.3}% of a core)", 100.0 * c / dur)).unwrap_or_else(|| "?".into())
            );
        }
    }
    if meta["session_locked"] == true {
        s += "session  locked: CPU and GPU numbers compare only with other locked recordings\n";
    }
    if let Some(line) = super::bench::screen_line(&meta["screen"]) {
        s += &format!("screen   {line}\n");
    }
    if get("cpu.app.total").is_finite() {
        s += &format!("cpu      {} of one core · {} in the window, process tree\n", fmt::value(get("cpu.app.total"), "%"), fmt::value(get("cpu.work"), "ms"));
    }
    if get("cpu.main").is_finite() {
        s += &format!("main     {} busy · {} on a CPU, the thread the process started with\n", fmt::value(get("cpu.main.busy"), "%"), fmt::value(get("cpu.main"), "ms"));
    }
    if get("cpu.wakes").is_finite() {
        s += &format!("wakes    {} context switches of its threads\n", fmt::value(get("cpu.wakes"), "/s"));
    }
    // Its own line: a refused `cpu.wakes` (`NoCounter`) leaves the IO it read shown.
    if get("io.read").is_finite() {
        s += &super::record::io_line(meta);
    }
    s += &fmt::counters(get);
    if get("ui.cpu").is_finite() {
        s += &super::record::ui_line(meta);
    }
    for (i, t) in meta["threads_top"].as_array().into_iter().flatten().enumerate() {
        s += &format!(
            "{}{} {}  {}\n",
            if i == 0 { "threads  " } else { "         " },
            fmt::pad(&fmt::value(t["cpu_ms"].as_f64().unwrap_or(f64::NAN), "ms"), 10),
            t["name"].as_str().map(str::to_string).unwrap_or_else(|| format!("thread {}", t["tid"])),
            if t["pid"] != meta["pid"] { format!("(pid {})", t["pid"]) } else { String::new() }
        );
    }
    s += &super::record::memory_lines(meta);
    s += &super::record::refused_lines(meta);
    s += &format!("next     strobe query {id} threads · strobe query {id} memory · strobe query {id} gpu · strobe record {} --probes deep (native stacks)\n", meta["target"].as_str().unwrap_or(""));
    s
}
