//! `strobe query`: views over recordings and experiments (SPEC §11.1).

mod actions;
mod diff;
mod exp_insights;
pub(super) mod http;
mod inputs;
mod rec;
mod sources;
pub(crate) use rec::windows_sql;
mod waits;
mod wir;

use super::{Output, fmt};
use crate::cli::Args;
use crate::engine;
use crate::metrics;
use crate::model::{Error, Result};
use crate::probe::symbols::Frame;
use crate::store;
use rec::{NativeStacks, cached};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

pub use diff::{ATTRIBUTION_PARTS, diff_tree, series_insights, side_tree};
pub use rec::{Part, Rec, Stacks, Weight};

/// A result or a recording named by its path rather than its handle, as `report` reads them: read as it
/// is, from anywhere (a copy, a fixture), and nothing is written beside it.
pub(super) enum ByPath {
    /// An experiment's result.json (named, or in the folder named), what it holds (read once), and, for
    /// one strobe does not keep, the hold while it is read (`store::Named`).
    Experiment(std::path::PathBuf, Value, Option<store::Named>),
    /// A recording's folder (named, or its meta.json named), its meta.json (read once and checked), and
    /// the hold.
    Recording(std::path::PathBuf, Value, Option<store::Named>),
}

/// `handle` as a path a person named: a result.json (a file holding an experiment's `series`, or a
/// bisect's), a meta.json, or a folder holding either: the one resolver `query`, `report` and MCP's
/// tools read a path with. None when no file or folder of that name is here (a handle, `a..b`), or for a
/// plain id (`store::plain_id`) that names a folder holding neither. A file that is no result or
/// meta.json strobe can read (too large, not JSON, nested past serde_json's limit, a token or run key
/// strobe does not write) is refused with why (`store::read_named_json`, `rec::sql_words`), and so is a
/// named folder holding a link out of it (`store::named_folder`). Nothing read from it is ever joined to
/// a path: the recordings a result names resolve by plain id in the store alone.
pub(super) fn by_path(handle: &str) -> Result<Option<ByPath>> {
    let Ok(p) = std::path::absolute(handle) else { return Ok(None) };
    let p = p.as_path();
    let held = |dir: &Path| (!store::kept(dir)).then(store::Named::hold);
    if p.is_dir() {
        let (result, meta) = (p.join("result.json"), p.join("meta.json"));
        if !result.is_file() && !meta.is_file() {
            if store::plain_id(handle) {
                return Ok(None);
            }
            return Err(Error::usage(format!("{handle}: a folder with no result.json or meta.json in it: not an experiment's or a recording's folder")));
        }
        store::named_folder(p)?;
        let named = held(p);
        return Ok(Some(if result.is_file() { ByPath::Experiment(result.clone(), experiment_json(&result)?, named) } else { ByPath::Recording(p.to_path_buf(), recording_meta(&meta)?, named) }));
    }
    if !p.is_file() {
        return Ok(None);
    }
    let dir = p.parent().unwrap_or(Path::new(".")).to_path_buf();
    if p.file_name().is_some_and(|n| n == "meta.json") {
        store::named_folder(&dir)?;
        let named = held(&dir);
        return Ok(Some(ByPath::Recording(dir, recording_meta(p)?, named)));
    }
    // A result.json alone: what is read beside it (a check's actions) is the folder's own regular file
    // by a plain name (`store::held_file`).
    let named = held(&dir);
    match experiment_json(p) {
        Ok(v) => Ok(Some(ByPath::Experiment(p.to_path_buf(), v, named))),
        // A plain id that happens to name a file here is still a handle.
        Err(_) if store::plain_id(handle) => Ok(None),
        Err(e) => Err(e),
    }
}

/// An experiment's result named by its path: a comparison's, a check's (`series` a list) or a bisect's.
fn experiment_json(file: &Path) -> Result<Value> {
    let v = store::read_named_json(file)?;
    if v["series"].is_array() || v["kind"] == "bisect" {
        Ok(v)
    } else {
        Err(Error::usage(format!("{}: not an experiment's result.json (no series, not a bisect) or a recording's meta.json: strobe reads only those by their path", file.display())))
    }
}

/// A recording's meta.json named by its path (or in the folder named): read as any named file is, its
/// refusals said (never a blank recording), and the words of it that go into a trace query checked.
fn recording_meta(file: &Path) -> Result<Value> {
    let meta = store::read_named_json(file)?;
    rec::sql_words(&meta, file)?;
    Ok(meta)
}

/// A path as a printed command's word: forward slashes (which Windows reads too), a canonical path's
/// verbatim prefix taken off (`\\?\C:\x` → `C:/x`, `\\?\UNC\srv\share` → `//srv/share`), quoted for the
/// shells (`exp_insights::quote`).
pub(super) fn path_word(p: &Path) -> String {
    exp_insights::quote(&plain_path(&p.display().to_string()))
}

/// What a printed command names the recording in `dir` by: its id when strobe keeps it, else the path it
/// was read from, quoted (`path_word`), as `Rec::handle` does.
pub(super) fn handle_of(dir: &Path) -> String {
    match dir.file_name().filter(|_| dir.parent().is_some_and(|p| p == crate::host::paths::recordings())) {
        Some(id) => id.to_string_lossy().to_string(),
        None => path_word(dir),
    }
}

/// Two recordings as one word of a printed command (`a..b`), each named as `Rec::handle` names it.
pub(super) fn pair_word(a: &Rec, b: &Rec) -> String {
    let raw = |r: &Rec| if r.stored() { r.id.clone() } else { plain_path(&r.dir.display().to_string()) };
    exp_insights::quote(&format!("{}..{}", raw(a), raw(b)))
}

/// A local path as a person types it, in forward slashes (Windows reads them too).
pub(super) fn plain_path(s: &str) -> String {
    match s.strip_prefix(r"\\?\UNC\") {
        Some(unc) => format!("//{}", unc.replace('\\', "/")),
        None => s.strip_prefix(r"\\?\").unwrap_or(s).replace('\\', "/"),
    }
}

pub fn run(args: &Args) -> Result<Output> {
    let handle = args.pos.first().ok_or_else(|| Error::usage("query needs a handle: strobe query rec_… <view>"))?;
    let view = args.pos.get(1).map(String::as_str).unwrap_or("summary");
    let top = args.num("top")?.unwrap_or(15.0) as usize;
    // Only two views read `--source`: one that ignored it would answer a question not asked.
    if let Some(s) = args.get("source").filter(|_| !matches!(view, "hotspots" | "log")) {
        return Err(Error::usage(format!("--source {s}: only hotspots (host, startup or setup of a script recording) and log (console, device or crash) take --source, not {view}")));
    }
    // Tracked history (SPEC §25.5): `query history`, `query changes`, `query <ser_…> history|changes`.
    if handle == "history" || handle == "changes" || handle.starts_with("ser_") {
        return super::history::query(args);
    }
    // The HTTP probe (SPEC §8.9) of a recording, an experiment's blocks, or two recordings.
    if view == "http" {
        return http::run(args, top);
    }
    let path = match by_path(handle)? {
        Some(ByPath::Experiment(file, result, _named)) => return experiment(result, Some(file.as_path()), args, view, top),
        other => other,
    };
    // (`store::resolve` marks what it resolves used: never a folder named by its path.)
    if let Some(dir) = path.is_none().then(|| store::find(handle).ok()).flatten().filter(|d| store::is_experiment(d)) {
        if !dir.join("result.json").exists() {
            let log = std::fs::read_to_string(dir.join("progress.log")).unwrap_or_default();
            let tail: Vec<&str> = log.lines().rev().take(12).collect::<Vec<_>>().into_iter().rev().collect();
            // Running only while a bench on this device has it (itself, or a bisect's step of it):
            // one that died or was refused before it wrote a result has ended.
            let id = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let ours = |e: &str| e == id || e.strip_prefix(id.as_str()).and_then(|r| r.strip_prefix("_s")).is_some_and(|k| !k.is_empty() && k.bytes().all(|c| c.is_ascii_digit()));
            if !super::bench::slot::list().iter().any(|b| b["exp"].as_str().is_some_and(ours)) {
                let resume = if dir.join("state.json").is_file() { format!("; strobe bench --resume {id} goes on") } else { String::new() };
                let text = format!("{id} ended without a result (no bench runs it{resume}); its last lines:\n{}\n", tail.join("\n"));
                return Ok(Output::new(text, json!({ "id": id, "state": "ended", "progress": tail })));
            }
            let text = format!("{handle} is running\n{}\n", tail.join("\n"));
            return Ok(Output::new(text, json!({ "id": handle, "state": "running", "progress": tail })));
        }
        let answer = experiment(store::read_json(&dir.join("result.json"))?, None, args, view, top);
        if answer.is_ok() {
            store::used(&dir);
        }
        return answer;
    }
    // `a..b`: one view of two recordings, side by side and per run.
    if let Some((a, b)) = handle.split_once("..").filter(|_| path.is_none()) {
        return pair(a, b, view, args, top);
    }
    let rec = match path {
        Some(ByPath::Recording(dir, meta, named)) => Rec::with_meta(&dir, meta, named),
        _ => Rec::resolve(handle)?,
    };
    recording(rec, view, args, top)
}

/// An experiment's view, from its result (stored, or read from a path).
fn experiment(mut result: Value, source: Option<&Path>, args: &Args, view: &str, top: usize) -> Result<Output> {
    // The folder that holds the result: what it keeps beside itself (a check's actions) is read there.
    let home = match source {
        Some(f) => f.parent().map(Path::to_path_buf),
        None => result["id"].as_str().and_then(|id| store::find(id).ok()),
    };
    match view {
        "summary" | "progress" => {
            // A result read from its path keeps the ledger it holds: this machine's index is not its history.
            let text = live_ledger(&mut result, source);
            Ok(Output::new(text, result))
        }
        "insights" => exp_insights::run(&result, source),
        "actions" => actions::experiment(&result, home.as_deref(), args, top),
        other => {
            if result["kind"] == "check" {
                return sources::check(&result, view, args, top);
            }
            let mut recs: Vec<String> = result["series"].as_array().into_iter().flatten().filter_map(|s| s["recording"].as_str().map(str::to_string)).collect();
            // `--blocks N` compares nothing: its attribution recordings are head's alone (`--why
            // always`), one per series, and one answers for the experiment.
            let series = result["series"].as_array().cloned().unwrap_or_default();
            // Beside series refused before they ran, or not run (the bench stopped).
            let aside = |s: &Value| s["kind"] == "refused" || s["not_run"] == true;
            if series.iter().any(|s| s["kind"] == "blocks") && series.iter().all(|s| s["kind"] == "blocks" || aside(s)) {
                let heads: Vec<String> = series.iter().filter_map(|s| s["attribution"]["head"].as_str().map(str::to_string)).collect();
                let id = result["id"].as_str().unwrap_or("this experiment");
                return match heads.as_slice() {
                    [one] => sources::forward(
                        Rec::resolve(one)?,
                        series.iter().find(|s| s["attribution"]["head"].as_str() == Some(one)).and_then(|s| s["label"].as_str()).unwrap_or("head"),
                        view,
                        args,
                        top,
                    ),
                    [] => {
                        let timed: Vec<&str> = series.iter().flat_map(|s| s["timing_recordings"].as_array().into_iter().flatten()).filter_map(|t| t["recording"].as_str()).collect();
                        let others = if timed.is_empty() { String::new() } else { format!("; its timed blocks' recordings (no profile): {}", timed.join(", ")) };
                        Err(Error::usage(format!(
                            "{id} kept no attribution recording: a timed block runs no profiler (it would weigh on its times), and bench --blocks N --why always adds one attribution block of head per series{others}"
                        )))
                    }
                    _ => Err(Error::usage(format!("{id} kept one attribution recording per series: strobe query <rec> {other} with one of {}", heads.join(", ")))),
                };
            }
            // A view of two recordings answers for the experiment: each series' base..head.
            if diff::VIEWS.contains(&other) {
                // What a printed command names it by: its id, or the path it was read from.
                let exp = source.map_or_else(|| result["id"].as_str().unwrap_or("").to_string(), path_word);
                return diff::experiment(&result, &exp, other, args, top);
            }
            for s in result["series"].as_array().into_iter().flatten() {
                recs.extend(["base", "head"].iter().filter_map(|r| s["attribution"][r].as_str().map(str::to_string)));
                recs.extend(s["attribution"].as_str().map(str::to_string));
            }
            Err(Error::usage(format!(
                "experiments support summary, progress, insights, actions, http and the base..head views ({}); {other} reads a recording{}",
                diff::VIEWS.join(" "),
                if recs.is_empty() { " (this experiment kept none)".to_string() } else { format!(": strobe query <rec> {other} with one of {}", recs.join(", ")) }
            )))
        }
    }
}

/// `a..b`: one view of two recordings, side by side and per run.
fn pair(a: &str, b: &str, view: &str, args: &Args, top: usize) -> Result<Output> {
    let (a, b) = (Rec::named(a)?, Rec::named(b)?);
    let mut out = diff::run(&a, &b, view, args, top)?;
    // A full trace costs the page main-thread time (SPEC §8.1): a lean and a full-trace recording's
    // times differ by that alone. The level is the trace's probes: a bench block's lean set, or the
    // full timing set of `--trace full` blocks and `strobe record` of a page.
    let traced = |r: &Rec| -> Option<bool> {
        let chromium = r.meta["engine"].as_str().is_none_or(|e| e == "chromium") && r.meta["native"] != true && r.dir.join("trace.pftrace").is_file();
        match r.meta["probes"].as_str() {
            Some("lean") if chromium => Some(false),
            Some("timing") if chromium && r.meta["timing"] != "perturbed" => Some(true),
            _ => None,
        }
    };
    if let (Some(fa), Some(fb)) = (traced(&a), traced(&b))
        && fa != fb
    {
        let (f, l) = if fa { (&a.id, &b.id) } else { (&b.id, &a.id) };
        let note = format!("{f} traced the full timing set and {l} the lean one: the full set costs the page main-thread time, so their times do not compare");
        out.text = format!("! ProbeOverhead  {note}\n{}", out.text);
        if let Some(o) = out.json.as_object_mut() {
            let w = serde_json::json!({ "id": "ProbeOverhead", "level": "warn", "text": note });
            match o.get_mut("warnings").and_then(Value::as_array_mut) {
                Some(all) => all.push(w),
                None => {
                    o.insert("warnings".into(), serde_json::json!([w]));
                }
            }
        }
    }
    Ok(out)
}

/// A scratch folder's name for a folder named by its path: 12 hex of the SHA-256 of its full path and
/// its meta.json's modification time (two copies never share one; an edited copy gets its own).
pub(super) fn folder_key(dir: &Path) -> String {
    use sha2::Digest;
    let full = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let at = std::fs::metadata(dir.join("meta.json")).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
    sha2::Sha256::digest(format!("{}\u{0}{at}", full.display()).as_bytes()).iter().take(6).map(|b| format!("{b:02x}")).collect()
}

/// A recording's view.
fn recording(rec: Rec, view: &str, args: &Args, top: usize) -> Result<Output> {
    recording_view(rec, view, args, top, false)
}

fn available() -> Result<Output> {
    Ok(Output::new(String::new(), Value::Null))
}

fn recording_view(mut rec: Rec, view: &str, args: &Args, top: usize, probe: bool) -> Result<Output> {
    let dir = rec.dir.clone();
    let case = args.get("case").map(|c| one_case(&mut rec, c, view)).transpose()?;
    let meta = &rec.meta;
    let trace = dir.join("trace.pftrace");
    // A page in Firefox or WebKit: no Chrome trace.
    let other_engine = meta["engine"].as_str().is_some_and(|e| e != "chromium");
    match view {
        "summary" => {
            let mut text = super::record::summary_text(meta);
            // A copied recording's stored id may name something else here. Its report is always a
            // useful next step; unlike `open`, `report` accepts the actual folder.
            if !rec.stored() {
                text = path_text(&text, meta, &dir, true);
            }
            if let Some(c) = &case {
                text += &format!("case     {}: the values are its runs' ({}); a duration above is the whole launch's\n", meta["label"].as_str().unwrap_or(&c.name), c.runs.join(" "));
            }
            let mut json = meta.clone();
            if let Some((t, rows)) = run_phases_cached(&rec) {
                text += &t;
                json["per_run"] = rows;
            }
            Ok(Output::new(text, json))
        }
        "style" if probe => {
            firefox_gap(&rec, Gap::Style)?;
            attribution_meta(&rec, "style")?;
            rec.require_part(Part::Style)?;
            if trace.is_file() {
                // Old traces do not index which measured runs have invalidations. Never start
                // trace_processor merely to list choices, nor infer tracking from recalcs.
                let measured = rec::measured_keys(meta);
                match meta["style_runs"].as_array().map(|runs| runs.iter().any(|r| measured.as_ref().is_none_or(|m| m.iter().any(|k| r == k.as_str())))) {
                    Some(true) => available(),
                    Some(false) => Err(no_tracking()),
                    None => Err(Error::usage(format!("{}: tracking in its measured runs is not indexed; read the recording directly to inspect its trace", rec.id))),
                }
            } else {
                if !tracked(&rec, rec.part(Part::Style)?.as_slice()) {
                    return Err(no_tracking());
                }
                available()
            }
        }
        "style" => style(&rec, top),
        "runs" if probe => runs_read(&rec, true),
        "runs" => runs_view(&rec),
        "sql" => {
            let sql = args.pos.get(2).ok_or_else(|| Error::usage("query <h> sql \"select …\""))?;
            if probe {
                return available();
            } // Every recording can build a trace. Do not execute the person's SQL.
            // Without a Chrome trace (a script recording, a native one, a page in another engine): the
            // Perfetto trace `strobe open` builds from what the recording holds, built now when it is not yet.
            let file = if trace.is_file() {
                trace
            } else {
                // Beside a stored recording; one named by its path is left as it is (the run folder).
                let built = if rec.stored() {
                    dir.join("perfetto.pftrace")
                } else {
                    // Named by a hash of the folder and its meta.json's time, never by anything the file says.
                    let d = crate::host::paths::run().join("query").join(folder_key(&dir));
                    std::fs::create_dir_all(&d)?;
                    d.join("perfetto.pftrace")
                };
                if !built.is_file() {
                    super::progress(format!("building {}'s Perfetto trace (what strobe open shows) to query", rec.id));
                    crate::perfetto::build(&dir, &built).map_err(|e| {
                        Error::usage(format!(
                            "this recording has no trace to query, and its Perfetto trace could not be built: {}{}",
                            e.message,
                            if other_engine { " (a page in another engine than Chromium is measured without a trace)" } else { "" }
                        ))
                    })?;
                }
                built
            };
            sql_view(&file, sql, top.max(50), false)
        }
        "frames" if meta["wir"] == true => Err(Error::refused(
            "EngineUnsupported",
            "WebKit's Web Inspector reports main-thread RenderingFrame records, not screen presentation times or dropped refreshes",
            format!("strobe query {} hotspots · strobe query {} layout", rec.handle(), rec.handle()),
        )),
        "threads" if meta["wir"] == true => {
            Err(Error::refused("NotOnThisOs", "iOS gives an unprivileged USB client no per-thread CPU or wake counters", format!("strobe query {} hotspots", rec.handle())))
        }
        "gpu" if meta["wir"] == true => {
            Err(Error::refused("NotOnThisOs", "iOS gives an unprivileged USB client no GPU utilization or VRAM counters", format!("strobe query {} memory --by layer", rec.handle())))
        }
        "frames" if dir.join("frames.json").exists() || meta["native"] == true => {
            if probe {
                inputs::frame_runs(&dir)?;
                inputs::presents_path(&dir)?;
                return available();
            }
            native_frames(&dir, top, case.as_ref().map(|c| c.runs.as_slice()))
        }
        "inputs" => inputs::inputs(&dir, meta, args.num("top")?.map(|t| t as usize), case.as_ref().map(|c| c.runs.as_slice()), probe),
        "threads" if case.is_some() && (args.has("timeline") || dir.join("threads.json").exists()) => {
            Err(Error::usage("--case: a native recording's threads count every case of its launch together (threads.json has no per-run rows); its frames take --case"))
        }
        "threads" if args.has("timeline") => thread_timeline(&dir, top, probe),
        "threads" if probe && dir.join("threads.json").is_file() => available(),
        "threads" if dir.join("threads.json").exists() => native_threads(&dir, top),
        // A page in another engine has no trace: threads from Firefox's profile (attribution), frames
        // from the OS's presents (headed).
        "threads" if !trace.exists() && dir.join(crate::probe::gecko::READ).is_file() => gecko_threads(&rec, top, probe),
        "frames" | "threads" if !trace.exists() && other_engine => Err(Error::usage(format!(
            "this {} recording has no {view}: {}",
            meta["engine"].as_str().unwrap_or(""),
            if view == "frames" {
                meta["os_frames"]["refused"].as_str().unwrap_or("its frames come from the OS's presents of a headed page").to_string()
            } else {
                "threads come from Firefox's profiler, in Firefox attribution recordings (--probes attribution)".to_string()
            }
        ))),
        // A Chromium page block that traced nothing (`traced: false`: a headless page's lean block traces
        // only for a metric read from the trace) has no frames to read: said, never a trace engine's error.
        "frames" if !trace.exists() => Err(match meta["frames_refused"].as_str().and_then(crate::model::Refusal::from_line) {
            Some(r) => r.fix("bench --trace full, or a headed run (no --headless), keeps a trace with the page's frames").into(),
            None if meta["traced"] == false => Error::usage("this block took no trace: nothing it measured needs one; record or bench with --trace full, or headed, to keep one"),
            None => Error::usage("this recording kept no trace, so it has no frames: bench --trace full, or a headed run (no --headless), keeps one"),
        }),
        "frames" if probe => available(),
        "frames" => frames(&trace, meta, top),
        "threads" if !trace.exists() => Err(Error::usage(format!(
            "this recording kept no thread counters (no trace, no threads.json){}",
            if dir.join("timeline.json.gz").is_file() {
                format!(": its threads over time are in strobe query {} threads --timeline", rec.handle())
            } else if dir.join("deep.json.gz").is_file() {
                format!(": a native stack capture answers strobe query {} hotspots --by package", rec.handle())
            } else {
                String::new()
            }
        ))),
        "threads" => threads(&trace, meta, top, probe),
        "gpu" if !trace.exists() => native_gpu(&dir, meta, probe),
        "gpu" if probe => available(),
        "gpu" => gpu(&trace, meta, top),
        "hotspots" | "alloc" => {
            let by = by_arg(args)?;
            let (pick, sort) = (Pick::of(args), Sort::of(args)?);
            if view == "alloc" {
                if probe {
                    rec.alloc_files()?;
                    return available();
                }
                alloc(&rec, by, top)
            } else if let Some(scope) = Scope::of(&rec, args)? {
                if probe {
                    return probe_cpu(&rec, scope);
                }
                hotspots_scoped(&rec, by, top, sort, scope)
            } else if args.has("native") || pick.any() {
                if probe {
                    pick.phase.map(|p| phase_spans(&dir, p)).transpose()?;
                    if !dir.join("deep.json.gz").is_file() {
                        return Err(no_native_samples(&rec));
                    }
                    return available();
                }
                native_hotspots_of(&rec, by, top, pick, sort)
            } else {
                if probe {
                    return probe_cpu(&rec, Scope::Measured);
                }
                hotspots_sorted(&rec, by, top, sort)
            }
        }
        "waits" => waits::waits(&rec, by_arg(args)?, top, args.get("thread"), args.get("phase"), probe),
        "layout" | "gc" if probe => {
            attribution_meta(&rec, view)?;
            rec.require_part(if view == "gc" { Part::Gc } else { Part::Layout })?;
            available()
        }
        "layout" => layout(&rec, top),
        "shifts" => {
            let v =
                store::read_json(&dir.join("shifts.json")).map_err(|_| Error::usage("no layout shifts in this recording: they come from attribution blocks of a series that declares layout.shift"))?;
            Ok(Output::new(crate::probe::shifts::text(&v), v))
        }
        "gc" => gc(&rec),
        "heap" if probe => {
            firefox_gap(&rec, Gap::Heap)?;
            heap_file(&dir)?;
            available()
        }
        "heap" => heap(&rec, top, args.has("reveal")),
        "leaks" if probe => {
            leak_files(&rec)?;
            available()
        }
        "leaks" => leaks(&rec, top, args.has("reveal")),
        "insights" => {
            exp_insights::quoting_reset();
            let mut out = insights(&rec)?;
            if let Some((shell, note)) = exp_insights::quoting_note() {
                out.text = format!("quoting  {note}\n{}", out.text);
                out.json["quoted_for"] = json!(shell);
            }
            Ok(out)
        }
        "actions" => actions::recording(&dir, args, top, case.as_ref().map(|c| (c.name.as_str(), c.seq == 1))),
        "log" => wir::log(&dir, meta, args, top, probe),
        "network" => wir::network(&dir, meta, args, top, probe),
        "memory" => {
            let by = args.get("by").unwrap_or("process");
            if !matches!(by, "process" | "layer" | "allocator") {
                return Err(Error::usage(format!("--by: process, layer or allocator, not {by}")));
            }
            if dir.join("memory.json").exists() {
                if probe {
                    reading_at(&dir, args.get("at"))?;
                    return available();
                }
                return native_memory(&dir, meta, by, top, args.get("at"));
            }
            if meta["native"] == true || (meta["pid"].is_u64() && meta["script"] != true) {
                return Err(Error::usage(format!("this native recording has no memory reading: {}", crate::probe::memory::why_empty())));
            }
            if meta["script"] == true {
                return Err(Error::usage(
                    "a script recording keeps no memory census: the host's memory at the last measured run's end is the block's memory.app.private, and what each run kept js.heap.retained",
                ));
            }
            if probe {
                census_at(&dir, args.get("at"))?;
                return available();
            }
            memory(&dir, by, top, args.get("at"))
        }
        other => Err(Error::usage(format!(
            "unknown view {other}: summary runs frames inputs threads gpu hotspots waits alloc layout style shifts gc heap leaks memory http log network insights actions sql"
        ))),
    }
}

/// An experiment's text with its rerun ledger as of now (SPEC §14.2): the text froze the ledger when
/// the bench ended, and experiments on the same variants since then count too, as the report's do.
/// A comparison's or a check's text is drawn again from its numbers, in the unit times print in now.
fn live_ledger(result: &mut Value, source: Option<&Path>) -> String {
    let mut text = result["text"].as_str().unwrap_or("").to_string();
    if let Some(source) = source {
        let rendered = super::bench::rendered(result).unwrap_or(text);
        text = path_text(&rendered, result, source, false);
        result["text"] = json!(text);
        return text;
    }
    if !result["identity"].is_object() {
        if let Some(t) = super::bench::rendered(result) {
            result["text"] = json!(t);
            return t;
        }
        return text;
    }
    let was = store::ledger::line(result);
    result["ledger"] = store::ledger::for_experiment(result["id"].as_str().unwrap_or(""), result["created"].as_str().unwrap_or(""), &result["identity"]);
    let now = store::ledger::line(result);
    if let Some(t) = super::bench::rendered(result) {
        // Said so: the bench printed the other line when it ended.
        text = match now.as_ref().filter(|_| was != now) {
            Some(n) => t.replacen(&format!("ledger  {n}\n"), &format!("ledger  {n} (as of now)\n"), 1),
            None => t,
        };
        result["text"] = json!(text);
        return text;
    }
    if was == now {
        return text;
    }
    // Said so: the bench printed the other line when it ended.
    let line = now.map(|t| format!("ledger  {t} (as of now)\n")).unwrap_or_default();
    match was.and_then(|w| text.find(&format!("ledger  {w}\n")).map(|at| (at, w.len() + 9))) {
        Some((at, len)) => text.replace_range(at..at + len, &line),
        // None before: the line goes where the bench puts it, above the closing `<id> · strobe …` line.
        None => {
            let id = result["id"].as_str().unwrap_or("");
            let at = text.rfind(&format!("{id} · strobe ")).filter(|_| !id.is_empty()).unwrap_or(text.len());
            text.insert_str(at, &line);
        }
    }
    // The JSON (`--json`, MCP) carries the same text.
    result["text"] = json!(text);
    text
}

/// Local text about a copy: rewrite command operands, not its name in prose or the IDs of other
/// recordings. Used after rendering both the terminal summary and Markdown (including saved text).
pub(super) fn path_text(text: &str, data: &Value, path: &Path, recording: bool) -> String {
    let word = path_word(path);
    let id = data["id"].as_str().unwrap_or("");
    let closing = format!("{id} · ");
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        if !recording && line.trim_end().strip_prefix("resume  strobe bench --resume").is_some_and(|arg| arg.trim() == id) {
            continue;
        }
        if recording && line.starts_with("next     strobe ") {
            if !id.is_empty() {
                out += &format!("next     strobe report recording {word} --open\n");
            }
            continue;
        }
        // Only these fields contain commands about this artifact. A log, warning or fix can
        // quote an entire command verbatim. A check's rec/why/video fields name *other* artifacts.
        let prefix = if recording && line.starts_with("network  ") && line.trim_end().ends_with(&format!("· query {id} network)")) {
            Some("· query ")
        } else if recording && line.starts_with("video    ") {
            Some("· strobe report recording ")
        } else if !recording && line.starts_with("      actions  ") {
            Some("· strobe query ")
        } else if !recording && line.starts_with("       strobe report experiment ") {
            Some("       strobe report experiment ")
        } else if !recording && let Some(cmd) = line.strip_prefix(&closing) {
            ["strobe query ", "strobe report experiment "].into_iter().find(|v| cmd.starts_with(v))
        } else {
            None
        };
        if let Some(prefix) = prefix
            && let Some((before, cmd)) = line.rsplit_once(prefix)
            && let Some(after) = cmd.strip_prefix(id)
            && (after.is_empty() || after.starts_with(char::is_whitespace))
        {
            if !id.is_empty() {
                out.push_str(before);
                out.push_str(prefix);
                out.push_str(&word);
                out.push_str(after);
            } else if prefix.starts_with('·') {
                // No operand to rewrite. Drop the unusable command, not the action count or
                // other information on its line (the inline network note ends in a parenthesis).
                out.push_str(before.trim_end());
                if prefix == "· query " {
                    out.push(')');
                }
                out.push('\n');
            }
        } else {
            out.push_str(line);
        }
    }
    out
}

/// One case of a `--batch` recording (`--case`): its name, its place in the launch and its runs.
struct Case {
    name: String,
    seq: u64,
    runs: Vec<String>,
}

/// The views whose rows come from a recording's runs, so `--case` can keep one case's.
const CASE_VIEWS: [&str; 9] = ["summary", "frames", "inputs", "threads", "gpu", "layout", "style", "gc", "actions"];

/// Narrows a batch recording to one case (by its name, its label or its place in the launch): its
/// measured runs are the only runs every view reads.
fn one_case(rec: &mut Rec, want: &str, view: &str) -> Result<Case> {
    let id = rec.id.clone();
    let cases = rec.meta["batch"].as_array().filter(|a| !a.is_empty()).cloned().ok_or_else(|| Error::usage(format!("--case picks one case of a --batch recording; {id} ran one")))?;
    if !CASE_VIEWS.contains(&view) {
        return Err(Error::usage(format!("--case: {view} reads the whole launch; one case's runs are in {}", CASE_VIEWS.join(", "))));
    }
    let names = |c: &Value| [c["case"].as_str().unwrap_or("").to_string(), c["label"].as_str().unwrap_or("").to_string(), c["seq"].to_string()];
    let found = cases.iter().find(|c| names(c).iter().any(|n| n == want)).ok_or_else(|| {
        let all: Vec<String> = cases.iter().map(|c| format!("{} ({})", c["case"].as_str().unwrap_or("?"), c["seq"])).collect();
        Error::usage(format!("no case {want:?} in {id}; its cases: {}", all.join(", ")))
    })?;
    let runs: Vec<String> = found["runs"].as_array().into_iter().flatten().filter_map(|r| r.as_str().map(str::to_string)).collect();
    rec.meta["measured"] = json!(runs);
    rec.meta["runs"] = json!(runs.len());
    // The case's own values and refusals, as its block gave them (the launch's are every case's).
    rec.meta["values"] = found["values"].clone();
    rec.meta["refused"] = found["refused"].clone();
    rec.meta["label"] = found["label"].clone();
    rec.meta["query_case"] = found["label"].clone();
    rec.meta["batch"] = json!([found]);
    Ok(Case { name: found["case"].as_str().unwrap_or(want).to_string(), seq: found["seq"].as_u64().unwrap_or(0), runs })
}

fn by_arg(args: &Args) -> Result<&str> {
    let by = args.get("by").unwrap_or("function");
    if !matches!(by, "function" | "file" | "package") {
        return Err(Error::usage(format!("--by: function, file or package, not {by}")));
    }
    Ok(by)
}

/// The recording's heap snapshot: live objects by class with retained sizes, detached DOM, the largest
/// owners and their retainer paths. String values stay hidden unless `--reveal` (SPEC §8.6).
pub fn heap(rec: &Rec, top: usize, reveal: bool) -> Result<Output> {
    let v = crate::heapsnap::summary(rec.snapshot()?, top, reveal);
    Ok(Output::new(crate::heapsnap::summary_text(&v, top), v))
}

/// Live objects in `b` against `a` by class: count, self and retained size changes, and what only `b` has.
/// Two recordings are two processes, so new objects group by class, never by node id.
pub fn heap_diff(a: &Rec, b: &Rec, top: usize, reveal: bool) -> Result<Output> {
    let v = crate::heapsnap::diff(a.snapshot()?, b.snapshot()?, false, top, reveal);
    Ok(Output::new(crate::heapsnap::diff_text(&v, top), v))
}

/// What a Firefox recording holds no source for.
#[derive(Clone, Copy)]
enum Gap {
    Heap,
    Alloc,
    Style,
}

/// Refuses a view a Firefox or iOS WebKit recording cannot answer (`EngineUnsupported`), with what answers instead.
fn firefox_gap(rec: &Rec, gap: Gap) -> Result<()> {
    // A Bun script host: Bun's `--heap-prof` writes a heap snapshot, not sampled allocations (strobe
    // passes it to Node alone; an older recording kept Bun's snapshot as `alloc-0.heapprofile`).
    let snapshot = || {
        let mut head = [0u8; 16];
        std::fs::File::open(rec.dir.join("alloc-0.heapprofile")).and_then(|mut f| std::io::Read::read(&mut f, &mut head)).is_ok_and(|n| head[..n].starts_with(b"{\"snapshot\""))
    };
    if matches!(gap, Gap::Alloc) && rec.meta["script"] == true && (rec.meta["runtime"]["name"] == "bun" || snapshot()) {
        return Err(crate::model::Refusal::new(
            crate::model::ValidatorId::EngineUnsupported,
            "sampled allocation stacks: Bun's --heap-prof writes a heap snapshot at its exit, not allocation samples",
        )
        .fix("run a runtime: \"node\" target for sampled allocation stacks, or declare js.heap.retained")
        .into());
    }
    if rec.meta["wir"] == true {
        let (summary, fix) = match gap {
            Gap::Heap => return Ok(()),
            Gap::Alloc => (
                "allocation samples: WebKit's Heap instrument snapshots live objects and GC pauses rather than allocation-site stacks",
                "query heap and query gc show live retained memory and garbage collections",
            ),
            Gap::Style => (
                "style invalidation tracking: WebKit's Timeline records RecalculateStyles spans without per-element selector invalidation causes",
                "query layout ranks WebKit's RecalculateStyles and Layout work by the script that forced them",
            ),
        };
        return Err(crate::model::Refusal::new(crate::model::ValidatorId::EngineUnsupported, summary).fix(fix).into());
    }
    if rec.meta["engine"] != "firefox" {
        return Ok(());
    }
    let (summary, fix) = match gap {
        Gap::Heap => (
            "heap snapshots: Firefox takes them inside the content process (ChromeUtils.saveHeapSnapshot), which strobe's WebDriver BiDi chrome scope does not reach",
            "query memory reads Firefox's about:memory by process, layer and allocator; a leak run gives the page's JS memory growth; heap snapshots and retainers come from Chromium",
        ),
        Gap::Alloc => ("allocation samples: strobe reads none from Firefox's profiler yet", "query hotspots and query gc show where time and collections go; sampled allocations come from Chromium"),
        Gap::Style => (
            "style invalidation tracking: Chromium's trace records each invalidation; Firefox's profiler records only the first cause of each restyle",
            "query layout ranks Firefox's forced style flushes and reflows by the script that forced them",
        ),
    };
    Err(crate::model::Refusal::new(crate::model::ValidatorId::EngineUnsupported, summary).fix(fix).into())
}

/// Insights (SPEC §11.3): rules over one recording's own data, each hit with its numbers and the
/// commands that show the evidence. Facts with thresholds, never a verdict strobe cannot back.
pub fn insights(rec: &Rec) -> Result<Output> {
    // The trace's rules read one pass, not three; the profiles and the snapshot load beside it.
    rec.preload(&[Part::Layout, Part::Gc, Part::LongTasks], true, true, true);
    let dir = rec.dir.as_path();
    let meta = &rec.meta;
    let id = rec.id.clone();
    let runs = meta["runs"].as_f64().unwrap_or(1.0).max(1.0);
    let v = &meta["values"];
    let work = v["main.work"].as_f64().unwrap_or(f64::NAN);
    let mut hits: Vec<Value> = Vec::new();
    // The rules whose source this recording holds, hit or not: a comparison says a rule fired on one
    // side only where the other side was read for it too.
    let mut checked: Vec<&str> = Vec::new();
    let mut hit =
        |rule: &str, level: &str, title: String, evidence: Value, handles: Vec<String>| hits.push(json!({ "rule": rule, "level": level, "title": title, "evidence": evidence, "handles": handles }));
    // What the commands name it by: its id, or the path it was read from (`strobe open` reads strobe's
    // own recordings only).
    let handle = rec.handle();
    let q = |view: &str| format!("strobe query {handle} {view}");
    let open: Vec<String> = rec.stored().then(|| format!("strobe open {id}")).into_iter().collect();
    // Forced layout: geometry read after a DOM write makes the browser lay out mid-script.
    if let Ok(l) = layout(rec, 3).map(|o| o.json) {
        checked.push("forced-layout");
        let ms = l["forced"]["layout"]["ms_per_run"].as_f64().unwrap_or(0.0) + l["forced"]["style"]["ms_per_run"].as_f64().unwrap_or(0.0);
        let share = if work > 0.0 { ms / work } else { f64::NAN };
        if ms >= 5.0 || share >= 0.05 {
            let top = &l["rows"][0];
            hit(
                "forced-layout",
                if share >= 0.1 { "warn" } else { "info" },
                format!(
                    "forced layout and style: {:.0} layouts and {:.0} style recalcs ({}) per run{}, most from {} at {}",
                    l["forced"]["layout"]["count_per_run"].as_f64().unwrap_or(0.0),
                    l["forced"]["style"]["count_per_run"].as_f64().unwrap_or(0.0),
                    fmt::value(ms, "ms"),
                    if share.is_finite() { format!(", {:.0}% of main-thread work", share * 100.0) } else { String::new() },
                    top["name"].as_str().unwrap_or("?"),
                    top["where"].as_str().unwrap_or("?")
                ),
                json!({ "ms_per_run": ms, "share_of_work": share, "forced": l["forced"], "top": top }),
                vec![q("layout")],
            );
        }
    }
    // GC pressure: collections pause the main thread; allocation sites say who causes them.
    if let Ok(g) = gc(rec).map(|o| o.json) {
        checked.push("gc-pressure");
        let ms = g["values"]["main_ms_per_run"].as_f64().unwrap_or(0.0);
        let share = if work > 0.0 { ms / work } else { f64::NAN };
        if share >= 0.05 || ms >= 10.0 {
            let sites: Vec<String> = alloc(rec, "function", 3)
                .map(|o| o.json["rows"].as_array().into_iter().flatten().map(|r| format!("{} {}", r["name"].as_str().unwrap_or(""), fmt::bytes(r["weight"].as_f64().unwrap_or(0.0) / runs))).collect())
                .unwrap_or_default();
            hit(
                "gc-pressure",
                if share >= 0.1 { "warn" } else { "info" },
                format!(
                    "garbage collection: {} of main-thread pauses per run{} ({:.0} minor, {:.0} major); top allocation sites per run: {}",
                    fmt::value(ms, "ms"),
                    if share.is_finite() { format!(", {:.0}% of main-thread work", share * 100.0) } else { String::new() },
                    g["values"]["minor_per_run"].as_f64().unwrap_or(0.0),
                    g["values"]["major_per_run"].as_f64().unwrap_or(0.0),
                    if sites.is_empty() { "no allocation profile".into() } else { sites.join(", ") }
                ),
                json!({ "gc": g["values"], "share_of_work": share, "sites": sites }),
                std::iter::once(q("gc")).chain(if meta["engine"] == "firefox" { rec.cpu().is_ok().then(|| q("hotspots")) } else { rec.alloc().is_ok().then(|| q("alloc")) }).collect(),
            );
        }
    }
    // A native block whose measured runs posted no input and whose app used under a quarter of a core:
    // an idle case (an app at rest), as a recording with no input is. A busy one (a spin, a compile)
    // is working, and its threads' switches are preemptions, not wakes for nothing.
    let idle_block = meta["native"] == true
        && meta["kind"] == "block"
        && v["cpu.app.total"].as_f64().is_some_and(|c| c < 25.0)
        && store::read_json(&dir.join("frames.json"))
            .ok()
            .and_then(|f| f["runs"].as_object().map(|r| !r.is_empty() && r.values().all(|r| r["inputs"].as_array().is_some_and(Vec::is_empty))))
            .unwrap_or(false);
    let all_threads: Vec<Value> = store::read_json(&dir.join("threads.json")).ok().and_then(|t| t.as_array().cloned()).unwrap_or_default();
    // Wakes while idle: a native app recorded with no input, or a block whose runs gave it none, whose
    // threads keep waking (a polling loop, a short timer) costs power and CPU all day; the threads that
    // wake most are named.
    let wakes = v["cpu.wakes"].as_f64().unwrap_or(0.0);
    if meta["native"] == true && (meta["kind"] == "record" || idle_block) && v["cpu.wakes"].is_number() {
        checked.push("wakes-while-idle");
    }
    if meta["native"] == true && (meta["kind"] == "record" || idle_block) && wakes >= 50.0 {
        // Rows with a wake count: macOS counts wakes per process, and its thread rows carry none.
        let threads: Vec<Value> = all_threads.iter().filter(|t| t["wakes_per_s"].is_number()).cloned().collect();
        let line = |t: &Value| {
            let module = t["module"].as_str().map(|m| format!(" ({m})")).unwrap_or_default();
            format!("{}{module} {}", t["name"].as_str().unwrap_or("(unnamed)"), fmt::value(t["wakes_per_s"].as_f64().unwrap_or(0.0), "/s"))
        };
        // The app's own threads are what its code can change; a driver's or the OS's threads inside
        // its processes wake for what the app asked of them (a GPU device, an audio stream), so they
        // are named apart.
        let mine = |t: &&Value| t["origin"] != "driver" && t["origin"] != "os";
        let sum = |own: bool| threads.iter().filter(|t| mine(t) == own).map(|t| t["wakes_per_s"].as_f64().unwrap_or(0.0)).sum::<f64>();
        let (app, other) = (sum(true), sum(false));
        let top: Vec<String> = threads.iter().filter(mine).take(3).map(line).collect();
        let theirs: Vec<String> = threads.iter().filter(|t| !mine(t)).take(3).map(line).collect();
        let idle = if idle_block { "in runs that gave it no input" } else { "with nothing to do" };
        let mut title = format!("the app's threads woke {} {idle}; most: {}", fmt::value(app, "/s"), if top.is_empty() { "none".into() } else { top.join(", ") });
        if other > 0.0 {
            title += &format!("; driver and OS threads in its processes woke {} more: {}", fmt::value(other, "/s"), theirs.join(", "));
        }
        hit(
            "wakes-while-idle",
            if app >= 200.0 { "warn" } else { "info" },
            title,
            json!({ "wakes_per_s": wakes, "app_wakes_per_s": app, "driver_os_wakes_per_s": other, "top": top, "driver_os_top": theirs, "idle_block": idle_block }),
            vec![q("threads")],
        );
    }
    let per = if meta["kind"] == "record" { "over the recording" } else { "per run" };
    // One thread doing most of the work off the UI (or main) thread: a worker, a pool thread, a
    // driver's compiler, which a UI-thread metric never shows.
    // Every part is a mean over the runs, as the threads' rows are (`wakes::by_thread`): a median beside
    // a mean reads a thread that was busy in one run alone as most of a typical run's CPU.
    let runs = store::read_json(&dir.join("runs.json")).unwrap_or(Value::Null);
    let column = |k: &str| -> Vec<f64> { runs.as_array().into_iter().flatten().filter_map(|r| r[k].as_f64()).collect() };
    let mean = |x: &[f64]| (!x.is_empty()).then(|| x.iter().sum::<f64>() / x.len() as f64);
    let (runs_cpu, main_cpu) = (column("cpu_ms"), column("main_cpu_ms"));
    // The threads that exited before the end are in the processes' total alone: the larger is the whole,
    // so no thread reads over 100% of it.
    let listed: f64 = all_threads.iter().filter_map(|t| t["cpu_ms_per_run"].as_f64()).sum();
    let total = mean(&runs_cpu).or(v["cpu.work"].as_f64()).unwrap_or(f64::NAN).max(listed);
    let ui_tid = meta["ui_thread"].as_u64();
    let ui_row = ui_tid.and_then(|tid| all_threads.iter().find(|t| t["tid"].as_u64() == Some(tid))).and_then(|t| t["cpu_ms_per_run"].as_f64());
    let (ui_name, ui_ms) = match (ui_row.or(v["ui.cpu"].as_f64()), mean(&main_cpu).or(v["cpu.main"].as_f64())) {
        (Some(u), _) => ("the UI thread", u),
        (None, Some(m)) => ("the main thread", m),
        (None, None) => ("", f64::NAN),
    };
    let busiest = all_threads
        .iter()
        .filter(|t| t["cpu_ms_per_run"].is_number() && (ui_tid.is_none() || t["tid"].as_u64() != ui_tid))
        .max_by(|a, b| a["cpu_ms_per_run"].as_f64().unwrap_or(0.0).total_cmp(&b["cpu_ms_per_run"].as_f64().unwrap_or(0.0)));
    if busiest.is_some() && total > 0.0 && ui_ms.is_finite() {
        checked.push("dominant-thread");
    }
    if let Some(t) = busiest
        && total > 0.0
        && ui_ms.is_finite()
    {
        let ms = t["cpu_ms_per_run"].as_f64().unwrap_or(0.0);
        let (share, ui_share) = (ms / total, ui_ms / total);
        if share >= 0.5 && ms >= 50.0 && ui_share < 0.25 {
            let name = t["name"].as_str().unwrap_or("(unnamed)");
            let module = t["module"].as_str().map(|m| format!(" in {m}")).unwrap_or_default();
            let origin = t["origin"].as_str().unwrap_or("?");
            let whose = match origin {
                "app" => "the app's own thread",
                "driver" => "a driver's thread in the app's process",
                "os" => "an OS thread in the app's process",
                _ => "a thread strobe could not place",
            };
            hit(
                "dominant-thread",
                if share >= 0.75 { "warn" } else { "info" },
                format!(
                    "one thread did {:.0}% of the processes' CPU ({} {per}): {name} (thread {}{module}), {whose}, while {ui_name} used {:.0}% ({}): the work is off it, where {ui_name}'s metrics never show it",
                    100.0 * share,
                    fmt::value(ms, "ms"),
                    t["tid"],
                    100.0 * ui_share,
                    fmt::value(ui_ms, "ms")
                ),
                json!({
                    "thread": name, "tid": t["tid"], "pid": t["pid"], "module": t["module"], "origin": origin, "cpu_ms": ms, "share": share, "processes_cpu_ms": total, "ui_thread": ui_name, "ui_cpu_ms": ui_ms, "ui_share": ui_share,
                    // Which thread, as two launches share it: its name and start module (ids differ).
                    "subject": format!("{name} {}", t["module"].as_str().unwrap_or("")),
                }),
                std::iter::once(q("threads")).chain(dir.join("deep.json.gz").is_file().then(|| q(&format!("hotspots --thread {}", t["tid"])))).collect(),
            );
        }
    }
    // Off-CPU: a block's runs that took their time waiting (on a server, the network, a timer, a lock,
    // I/O) rather than computing; an idle native block waits by design. A recording has no runs: its
    // `run.duration` is the window it was asked to record.
    // A script host under its profiler spends a core on the sampler (its cpu.work is mostly that): its
    // main thread's CPU is the script's own.
    let profiled_script = meta["script"] == true && meta["timing"] == "perturbed";
    let (cpu_metric, whose) = if profiled_script { ("cpu.main", "the host's main thread ran") } else { ("cpu.work", "the target's processes used") };
    let (duration, cpu) = (v["run.duration"].as_f64().unwrap_or(f64::NAN), v[cpu_metric].as_f64().unwrap_or(f64::NAN));
    // Its main thread alone says nothing of threads the script started: when the host used more than
    // the sampler's one core besides, other threads were on a CPU, and the rule cannot tell.
    let workers = profiled_script && v["cpu.work"].as_f64().is_some_and(|w| w - duration >= 0.25 * duration);
    if meta["kind"] == "block" && duration.is_finite() && cpu.is_finite() && !workers {
        checked.push("off-cpu-bound");
    }
    if meta["kind"] == "block" && duration >= 100.0 && cpu.is_finite() && cpu < 0.25 * duration && !idle_block && !workers {
        let mut handles = Vec::new();
        if crate::probe::http_probe::read(dir).is_some() {
            handles.push(q("http"));
        }
        if dir.join(crate::probe::webkit_inspector::NETWORK_FILE).is_file() {
            handles.push(q("network"));
        }
        // Where the threads waited and who woke them comes with a native capture alone; a native
        // block without one has its threads, a page's trace its requests and timers, a script host its
        // threads over time.
        if dir.join("deep.json.gz").is_file() {
            handles.push(q("waits"));
        } else if meta["native"] == true && dir.join("threads.json").is_file() {
            handles.push(q("threads"));
        } else if handles.is_empty() && dir.join("trace.pftrace").is_file() {
            handles.extend(open.clone());
        } else if handles.is_empty() && dir.join("timeline.json.gz").is_file() {
            handles.push(q("threads --timeline"));
        }
        hit(
            "off-cpu-bound",
            "info",
            format!(
                "off-CPU: each run took {} while {whose} {} of CPU ({:.0}%){}: most of the run was not on a CPU (waiting on a server, the network, a timer, a lock or I/O)",
                fmt::value(duration, "ms"),
                fmt::value(cpu, "ms"),
                100.0 * cpu / duration,
                if profiled_script { ", the profiler's own thread left out" } else { "" }
            ),
            json!({ "run_duration_ms": duration, "cpu_work_ms": v["cpu.work"], "cpu_ms": cpu, "cpu_metric": cpu_metric, "cpu_share": cpu / duration }),
            handles,
        );
    }
    // Style and layout over script: the page's time goes to the browser's rendering of what script
    // changed (many elements, costly selectors), not to the script itself.
    let (style, layout_ms, script) = (v["main.style"].as_f64(), v["main.layout"].as_f64(), v["main.script"].as_f64().unwrap_or(0.0));
    if style.is_some() || layout_ms.is_some() {
        checked.push("style-layout-dominant");
    }
    if let Some(sl) = style.zip(layout_ms).map(|(s, l)| s + l).or(style).or(layout_ms)
        && sl >= 20.0
        && sl > script
    {
        // Only the views this recording answers: forced layouts come from an attribution trace, what
        // invalidated style from one that tracked invalidations too. A timed block's lean trace has
        // neither: its trace in the Perfetto UI shows the restyles.
        let mut restyle_handles: Vec<String> = Vec::new();
        if layout(rec, 1).is_ok() {
            restyle_handles.push(q("layout"));
        }
        if meta["probes"] == "attribution" && invalidation(rec).is_ok() {
            restyle_handles.insert(0, q("style"));
        }
        if restyle_handles.is_empty() && dir.join("trace.pftrace").is_file() {
            restyle_handles.extend(open.clone());
        }
        hit(
            "style-layout-dominant",
            if sl >= 2.0 * script.max(1.0) && sl >= 50.0 { "warn" } else { "info" },
            format!(
                "style and layout: {} {per} ({} style, {} layout) against {} of script: the page's time goes to restyling and laying out rather than running script",
                fmt::value(sl, "ms"),
                fmt::value(style.unwrap_or(f64::NAN), "ms"),
                fmt::value(layout_ms.unwrap_or(f64::NAN), "ms"),
                fmt::value(script, "ms")
            ),
            json!({ "style_ms": style, "layout_ms": layout_ms, "script_ms": script }),
            restyle_handles.clone(),
        );
    }
    // Long tasks: one task over 50 ms blocks input for that long.
    if let Ok(rows) = rec.part(Part::LongTasks) {
        checked.push("long-tasks");
        let cell = |i: usize| rows.first().and_then(|r| r.get(i).cloned().flatten()).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
        let (n, worst) = (cell(0), cell(1) / 1e6);
        if n > 0.0 {
            hit(
                "long-tasks",
                "warn",
                format!("long tasks: {n:.0} main-thread tasks over 50 ms in the recording, the longest {}: input waits that long", fmt::value(worst, "ms")),
                json!({ "count": n, "longest_ms": worst }),
                if dir.join("trace.pftrace").is_file() {
                    std::iter::once(q("sql \"select ts, dur / 1e6 as ms from slice where name = 'RunTask' and depth = 0 and dur > 50e6 order by dur desc limit 20\""))
                        .chain(rec.cpu().is_ok().then(|| q("hotspots")))
                        .collect()
                } else {
                    rec.cpu().is_ok().then(|| q("hotspots")).into_iter().chain(open.clone()).collect()
                },
            );
        }
    }
    // Work while idle: a recording with no input that still presents frames or keeps the main thread busy
    // (skipped on `wir:`, where a developer inspects their live Safari tab and may touch/scroll it).
    if meta["kind"] == "record" && meta["wir"] != true && v["interaction.n"].is_null() && v["scroll.n"].is_null() {
        checked.push("work-while-idle");
        let seconds = meta["duration_ms"].as_f64().unwrap_or(0.0) / 1000.0;
        let fps = v["frames.n"].as_f64().unwrap_or(0.0) / seconds.max(1e-9);
        let busy = v["main.busy"].as_f64().unwrap_or(0.0);
        if fps >= 1.0 || busy >= 5.0 {
            hit(
                "work-while-idle",
                "warn",
                format!("work while idle: with no input, the page presented {fps:.1} frames per second and kept its main thread {busy:.1}% busy: an animation, a timer or polling"),
                json!({ "frames_per_second": fps, "main_busy_pct": busy }),
                (!meta["frames_refused"].is_string() && meta["traced"] != false && (dir.join("frames.json").is_file() || dir.join("trace.pftrace").is_file()))
                    .then(|| q("frames"))
                    .into_iter()
                    .chain(rec.cpu().is_ok().then(|| q("hotspots")))
                    .collect(),
            );
        }
    }
    // Detached DOM: removed from the document but kept alive by script.
    if let Ok(h) = heap(rec, 3, false).map(|o| o.json) {
        checked.push("detached-dom");
        let d = &h["detached"];
        if d["count"].as_u64().unwrap_or(0) > 0 {
            let largest = &d["classes"][0];
            hit(
                "detached-dom",
                if d["retained_size"].as_f64().unwrap_or(0.0) > 1e6 { "warn" } else { "info" },
                format!(
                    "detached DOM: {} nodes removed from the document are still alive, retaining {}; the largest, {}, is held by {}",
                    d["count"],
                    fmt::bytes(d["retained_size"].as_f64().unwrap_or(0.0)),
                    largest["class"].as_str().unwrap_or("?"),
                    path_tail(&largest["largest"]["path"])
                ),
                json!({ "detached": d["count"], "retained_size": d["retained_size"], "largest": largest }),
                vec![q("heap")],
            );
        }
    }
    // Stalled HTTP probes in the measured runs: probes in a row the server held past the stall threshold
    // (`query http`). A cohort counts where any of its probes was due in a measured run's timed part
    // (one may open just before the run); stalls in warmup, untimed parts or between runs are no suspect.
    if let Some(h) = http::recording_json(dir, usize::MAX) {
        checked.push("http-stalls");
        let all = h["cohorts"].as_array().cloned().unwrap_or_default();
        let measured: Vec<&Value> = all.iter().filter(|c| c["in_measured"] == true).collect();
        if let Some(c) = measured.first() {
            let n = measured.len();
            let timeouts = measured.iter().filter_map(|c| c["measured_timeouts"].as_u64()).sum::<u64>();
            let others = all.len() - n;
            hit(
                "http-stalls",
                if timeouts > 0 { "warn" } else { "info" },
                format!(
                    "HTTP probe: {n} stalled cohort{} in the measured runs (probes in a row over {}{}); the first, {} +{:.2} s, {} probes: {}",
                    if n == 1 { "" } else { "s" },
                    fmt::value(h["stall_threshold_ms"].as_f64().unwrap_or(f64::NAN), "ms"),
                    if others > 0 { format!("; {others} more outside them") } else { String::new() },
                    c["measured_run"].as_str().or(c["run"].as_str()).unwrap_or("?"),
                    c["at_s"].as_f64().unwrap_or(0.0),
                    c["probes"],
                    c["says"].as_str().unwrap_or("")
                ),
                json!({ "cohorts": n, "outside_measured": others, "timeouts": timeouts, "first": c, "stall_threshold_ms": h["stall_threshold_ms"], "measured": h["measured"] }),
                [vec![q("http")], open.clone()].concat(),
            );
        }
    }
    // Background load on the measured CPUs.
    if v["cpu.foreign.max"].is_number() {
        checked.push("background");
    }
    if v["cpu.foreign.max"].as_f64().is_some_and(|f| f >= 25.0) {
        hit(
            "background",
            "warn",
            format!("background load: other processes used up to {:.0}% of a core on the CPUs the target ran on", v["cpu.foreign.max"].as_f64().unwrap_or(0.0)),
            json!({ "cpu_foreign_max_pct": v["cpu.foreign.max"], "busiest": meta["background"] }),
            vec![],
        );
    }
    // Unresolved time: script without source maps, and native time strobe cannot name without deep.
    if let Ok(h) = hotspots_of(rec, "function", 400).map(|o| o.json) {
        checked.extend(["unresolved", "native-time"]);
        if h["native"] == true && h["resolved_share"].as_f64().is_some_and(|s| s < 0.95) {
            let share = h["resolved_share"].as_f64().unwrap_or(0.0);
            hit(
                "unresolved",
                "info",
                format!("unresolved native: {:.0}% of native samples have no symbol (no matching PDB), mostly in {}", 100.0 * (1.0 - share), h["unsymbolized"][0]["module"].as_str().unwrap_or("?")),
                json!({ "resolved_share": share, "unsymbolized": h["unsymbolized"], "subject": "native" }),
                vec![q("hotspots --by package")],
            );
        }
        let (scripts, mapped) = (h["script_samples"].as_f64().unwrap_or(0.0), h["mapped_samples"].as_f64().unwrap_or(0.0));
        // Only minified scripts lack a map: a page's inline scripts and a script written as it runs
        // name their own functions and lines. A script target's profile names its own source files.
        let minified = h["minified_unmapped_samples"].as_f64().unwrap_or(0.0);
        if scripts > 0.0 && minified / scripts > 0.05 && meta["script"] != true {
            hit(
                "unresolved",
                "info",
                format!("unresolved script: {:.0}% of script samples are in minified scripts with no source map, so names and files stay minified there", 100.0 * minified / scripts),
                json!({ "script_samples": scripts, "mapped_samples": mapped, "minified_unmapped_samples": minified, "subject": "script" }),
                vec![q("hotspots --by file")],
            );
        }
        let rows = h["rows"].as_array().cloned().unwrap_or_default();
        // strobe's page driver at work (its frames' location `strobe-driver.js:43:11`, or the browser's
        // work it called): never the target's time.
        let driver = |r: &Value| {
            let at = r["where"].as_str().unwrap_or("");
            at == VIA_DRIVER || at == crate::driver::SOURCE_URL || at.starts_with(&format!("{}:", crate::driver::SOURCE_URL))
        };
        let busy: f64 = rows.iter().filter(|r| r["name"] != "(idle)" && !driver(r)).map(|r| r["weight"].as_f64().unwrap_or(0.0)).sum();
        // No symbol: a frame named by its module (`known.exe`, listed as `ntdll.dll (windows)`) or
        // `module+0x…`. A symbol with no line info (a release build's symbol table, a public PDB) is a
        // function strobe can name, though it has no file.
        let unsymbolized = |r: &Value| {
            let name = r["name"].as_str().unwrap_or("");
            h["native"] == true
                && (name.contains("+0x")
                    || name.starts_with('[')
                    || ["unsymbolized", "unsymbolized_any_frame"]
                        .iter()
                        .flat_map(|k| h[k].as_array().into_iter().flatten())
                        .filter_map(|u| u["module"].as_str())
                        .any(|m| m.split(" (").next().is_some_and(|m| m.eq_ignore_ascii_case(name))))
        };
        // One function holding a large part of the sampled CPU: where a change pays first. Not from
        // untimed snapshots (macOS `sample`: every thread, waiting ones too, no CPU time), and only a
        // named function of the target's with enough samples to say so (20, and 1 ms a run in a block).
        let per_run = (meta["kind"] == "block" && h["scope"] != "whole_host").then(|| rec.runs()).flatten().filter(|n| *n > 0);
        // Each such function over the threshold (at most four can be), so a comparison reads one side's
        // function against the same function on the other, never against whichever came first.
        let named = rows.iter().filter(|_| h["untimed"] != true && busy > 0.0).filter(|r| {
            let w = r["weight"].as_f64().unwrap_or(0.0);
            !r["name"].as_str().unwrap_or("(").starts_with('(')
                && !driver(r)
                && !unsymbolized(r)
                && r["samples"].as_u64().unwrap_or(0) >= 20
                && per_run.is_none_or(|n| w / 1e3 / n as f64 >= 1.0)
                && w / busy >= 0.25
        });
        if h["untimed"] != true {
            checked.push("hot-function");
        }
        for r in named {
            let (w, samples) = (r["weight"].as_f64().unwrap_or(0.0), r["samples"].as_u64().unwrap_or(0));
            {
                // Weights are µs; a recording's window, and a script profile of the whole host, have no
                // runs to divide by.
                let amount = match per_run {
                    Some(n) => format!("{} per run", fmt::value(w / 1e3 / n as f64, "ms")),
                    None if h["scope"] == "whole_host" => format!("{} over the whole host process", fmt::value(w / 1e3, "ms")),
                    None => format!("{} over the recording", fmt::value(w / 1e3, "ms")),
                };
                // Its file from its checkout's root, as two builds share it (the hotspots view keeps the
                // whole location and line).
                let file = r["where"].as_str().map(|w| diff::portable_location(w, diff::ran_from(meta))).filter(|w| !w.is_empty() && w != "(native)");
                let at = file.as_deref().map(|w| format!(" ({w})")).unwrap_or_default();
                hit(
                    "hot-function",
                    "info",
                    format!("hot function: {}{at} holds {:.0}% of the sampled CPU ({amount}, {samples} samples)", r["name"].as_str().unwrap_or("?"), 100.0 * w / busy),
                    json!({
                        "function": r["name"], "where": r["where"], "share": w / busy, "weight_us": w, "samples": samples, "runs": per_run,
                        // Which function, as two launches of two builds share it (its file from the
                        // checkout's root, no line).
                        "subject": format!("{} {}", r["name"].as_str().unwrap_or(""), file.as_deref().unwrap_or("")),
                    }),
                    vec![q("hotspots"), q("hotspots --by package")],
                );
            }
        }
        let program = rows.iter().find(|r| r["name"] == "(program)").and_then(|r| r["weight"].as_f64()).unwrap_or(0.0);
        if busy > 0.0 && program / busy >= 0.3 {
            // A script host has no browser: its (program) is the runtime's own work, and its threads are
            // in the timeline (no trace).
            let (whose, next) = if meta["script"] == true {
                ("the runtime's own work outside JavaScript (its event loop, I/O, parsing, compiling)", q("threads --timeline"))
            } else {
                ("the browser's own work outside JavaScript (rendering, IPC, parsing)", q("threads"))
            };
            hit(
                "native-time",
                "info",
                format!("native time: {:.0}% of busy samples are (program), {whose}; naming it needs native stacks", 100.0 * program / busy),
                json!({ "program_share": program / busy }),
                vec![next],
            );
        }
    }
    // Memory recordings and leak runs: what grew and what stayed (facts only). A `record --probes memory`
    // keeps its suspects' classes in leaks.json, not in its meta.
    let leaks = if meta["leaks"]["classes"].is_array() { meta["leaks"].clone() } else { store::read_json(&dir.join("leaks.json")).unwrap_or(Value::Null) };
    if meta["leak"]["js_heap"].is_object() || meta["probes"] == "memory" {
        checked.push("leak");
    }
    if let Some(l) = meta["leak"]["js_heap"].as_object() {
        let grows = crate::stats::kind_of(&meta["leak"]["js_heap"]) == crate::model::VerdictKind::Worse;
        hit(
            "leak",
            if grows { "warn" } else { "info" },
            format!("js heap: {}{}", l["verdict"].as_str().unwrap_or(""), if grows { retainer(&leaks).map(|r| format!("; {r}")).unwrap_or_default() } else { String::new() }),
            json!(l),
            // Firefox keeps about:memory censuses, no heap snapshots to list leaks from.
            if meta["engine"] == "firefox" { vec![q("memory --at 0"), q("memory")] } else { vec![q("leaks"), q("memory")] },
        );
    } else if meta["probes"] == "memory" && meta["leaks"]["count"].as_u64().unwrap_or(0) > 0 {
        hit(
            "leak",
            "info",
            format!(
                "{} objects from the first half are still alive at the end, retaining {}{}",
                meta["leaks"]["count"],
                fmt::bytes(meta["leaks"]["retained_size"].as_f64().unwrap_or(0.0)),
                retainer(&leaks).map(|r| format!("; {r}")).unwrap_or_default()
            ),
            meta["leaks"].clone(),
            vec![q("leaks"), q("memory")],
        );
    }
    // Crash or Jetsam caught on an iOS device during `[t0, t1 + 5s]`.
    if meta["crashes"].is_array() {
        checked.extend(["crash", "jetsam"]);
    }
    for c in meta["crashes"].as_array().into_iter().flatten() {
        let rule = if c["kind"] == "jetsam" { "jetsam" } else { "crash" };
        hit(rule, "warn", c["summary"].as_str().unwrap_or("target process crashed or was jetsam-killed").to_string(), c.clone(), vec![q("log --source crash")]);
    }
    let mut text = format!("insights · {id} · {} hit(s)\n", hits.len());
    text += &hits_text(&hits);
    if hits.is_empty() {
        text += "nothing stood out: no rule crossed its threshold\n";
    }
    Ok(Output::new(text, json!({ "recording": id, "insights": hits, "checked": checked })))
}

/// Insight hits as text: `! title` (warn) or `i title`, then the commands that show its evidence. A hit
/// of a comparison's head says what base's recording showed for the same rule and subject (`sides`).
fn hits_text(hits: &[Value]) -> String {
    let mut text = String::new();
    for h in hits {
        let sides = match h["sides"].as_str() {
            Some("both") => " (over its threshold on base too)",
            Some("head only") => " (over its threshold on head only)",
            Some("base unread") => " (base not read for it)",
            _ => "",
        };
        text += &format!("{} {}{sides}\n", if h["level"] == "warn" { "!" } else { "i" }, h["title"].as_str().unwrap_or(""));
        for c in h["handles"].as_array().into_iter().flatten() {
            text += &format!("    {}\n", c.as_str().unwrap_or(""));
        }
    }
    text
}

/// A hit's identity across two recordings: its rule and what it is about (`evidence.subject`: which
/// function, which thread, native or script), so one function's hit never stands for another's.
fn hit_key(h: &Value) -> (String, String) {
    (h["rule"].as_str().unwrap_or("").to_string(), h["evidence"]["subject"].as_str().unwrap_or("").to_string())
}

/// Marks each of head's hits by what base's recording showed (`sides`): the same rule over its
/// threshold on the same subject (`both`), base read for that rule and not over it there (`head only`),
/// or base not read for it (`base unread`: its recording lacks that rule's source). Returns base's hits
/// head has none of. `base` is base's insights JSON (`insights`, `checked`).
fn mark_sides(head: &mut [Value], base: &Value) -> Vec<Value> {
    let hits = base["insights"].as_array().cloned().unwrap_or_default();
    let read = |rule: &str| base["checked"].as_array().is_some_and(|c| c.iter().any(|r| r == rule));
    for h in head.iter_mut() {
        let k = hit_key(h);
        h["sides"] = json!(if hits.iter().any(|b| hit_key(b) == k) {
            "both"
        } else if read(&k.0) {
            "head only"
        } else {
            "base unread"
        });
    }
    hits.into_iter().filter(|b| !head.iter().any(|h| hit_key(h) == hit_key(b))).collect()
}

/// One class of objects from the first half still alive at the end, as a leak's suspects list it:
/// `Retained ×10000 (its constructor at <url>:4:29), 1.03 MB, held by … › kept Array › 14000 Retained`,
/// at the location `query leaks` prints (V8's line and column).
fn leak_class(c: &Value) -> String {
    let at = c["location"]["url"].as_str().filter(|u| !u.is_empty()).map(|u| format!(" (its constructor at {u}:{}:{})", c["location"]["line"], c["location"]["column"])).unwrap_or_default();
    format!("{} ×{}{at}, {}, held by {}", c["class"].as_str().unwrap_or("?"), c["count"], fmt::bytes(c["retained_size"].as_f64().unwrap_or(0.0)), path_tail(&c["largest"]["path"]))
}

/// A leak's classes, the page's own first (their constructor has a source location: its script made
/// them) over V8's internals, each group in its retained order.
fn leak_classes(leaks: &Value) -> Vec<&Value> {
    let all: Vec<&Value> = leaks["classes"].as_array().into_iter().flatten().collect();
    let own = |c: &&Value| c["location"]["url"].as_str().is_some_and(|u| !u.is_empty());
    all.iter().copied().filter(own).chain(all.iter().copied().filter(|c| !own(c))).collect()
}

/// The suspect a leak's census names first (`leak_classes`).
fn retainer(leaks: &Value) -> Option<String> {
    leak_classes(leaks).first().map(|c| format!("most retained: {}", leak_class(c)))
}

/// The last two steps of a retainer path, as `… › parent › object`.
fn path_tail(path: &Value) -> String {
    let steps: Vec<String> =
        path.as_array().into_iter().flatten().map(|s| format!("{} {}", s["edge"].as_str().map(str::to_string).unwrap_or_else(|| s["edge"].to_string()), s["name"].as_str().unwrap_or(""))).collect();
    match steps.len() {
        0 => "an unknown retainer".into(),
        n => format!("… › {}", steps[n.saturating_sub(2)..].join(" › ")),
    }
}

/// Memory by process, layer or allocator at one census (`--at` index, default the last): OS private
/// bytes and working set per process, the page's JS heap and DOM, and Chromium's allocators by
/// effective size (SPEC §8.6). `unexplained` is OS private bytes no allocator accounts for: an
/// estimate, since shared memory and the OS account differently.
pub fn memory(dir: &Path, by: &str, top: usize, at: Option<&str>) -> Result<Output> {
    let (c, label) = census_at(dir, at)?;
    let (procs, from_dump) = census_processes(&c);
    // A phone's page: no process of this computer's is its, so only Chromium's dump names them.
    let phone = c["processes"].is_null();
    let alloc = &c["allocators"];
    let mb = |b: f64| fmt::bytes(b);
    // The trace that took the dump allocates its own buffers: strobe's memory, not the target's.
    let layers = |pid: &str| -> BTreeMap<String, f64> {
        alloc[pid]["layers"].as_object().map(|o| o.iter().filter(|(k, _)| k.as_str() != "tracing").map(|(k, v)| (k.clone(), v.as_f64().unwrap_or(0.0))).collect()).unwrap_or_default()
    };
    let inflated = c["os_after_snapshot"] == true;
    // The page's renderer: the one whose V8 heap matches the page's (other renderers serve workers or
    // extensions). Only it was snapshotted, so only its OS readings are inflated after a snapshot.
    let js_used = c["js_heap_bytes"].as_f64().filter(|u| u.is_finite() && *u > 0.0);
    // The census names the page's process: Firefox's content process, and a Chromium census from the
    // mark the page made in the dump's trace. An older Chromium census only when sizes leave one
    // choice: the only renderer, or the only renderer whose V8 can hold the heap the page used (read
    // after a GC just before the dump, so 10% under it still can). Nearest by size misnamed a 1 MB V8
    // beside a page whose 21 MB V8 held its 10 MB heap; with two that could, the page is not known.
    let page_pid = c["page_pid"].as_u64().map(|p| p.to_string()).or_else(|| {
        let renderers: Vec<(String, f64)> = procs.iter().filter(|p| p["type"] == "renderer").filter_map(|p| Some((p["pid"].to_string(), *layers(&p["pid"].to_string()).get("v8")?))).collect();
        if let [only] = renderers.as_slice() {
            return Some(only.0.clone());
        }
        let used = js_used?;
        match renderers.iter().filter(|r| r.1 >= 0.9 * used).collect::<Vec<_>>().as_slice() {
            [one] => Some(one.0.clone()),
            _ => None,
        }
    });
    // A utility process by its service (`network.mojom.NetworkService` → network); the tracing service
    // runs because strobe traces, so it is strobe's, not the target's (rule 2).
    let kind = |p: &Value| -> String {
        let ty = p["type"].as_str().unwrap_or("other");
        match ty.split_once(".mojom.") {
            Some(("tracing", _)) => "tracing (strobe's)".into(),
            Some((service, _)) => service.to_string(),
            None if page_pid.as_deref() == Some(p["pid"].to_string().as_str()) => "renderer (page)".into(),
            None => ty.to_string(),
        }
    };
    let procs: Vec<Value> = procs
        .into_iter()
        .map(|mut p| {
            p["kind"] = json!(kind(&p));
            p
        })
        .collect();
    let target_procs: Vec<&Value> = procs.iter().filter(|p| p["kind"] != "tracing (strobe's)").collect();
    let mut text = format!(
        "memory by {by} · {label}{}{}\n",
        c["taken"].as_str().map(|t| format!(" · taken {t}")).unwrap_or_default(),
        if alloc.is_object() && alloc.get("error").is_none() { "" } else { " · no allocator dump" }
    );
    let note = match (phone, from_dump) {
        (false, _) => None,
        (true, true) => {
            Some("the page's processes are on the phone: listed from Chromium's memory dump, with no OS private bytes or working set (strobe reads no process memory of the phone's)".to_string())
        }
        (true, false) => Some(format!(
            "the page's processes are on the phone, and this census has no Chromium memory dump to list them{}: strobe reads no process memory of the phone's, so only the page's JS heap and DOM are here",
            alloc["error"].as_str().map(|e| format!(" ({e})")).unwrap_or_default()
        )),
    };
    if let Some(n) = &note {
        text += &format!("{n}\n");
    }
    let mut json = match by {
        "layer" => {
            let mut sums: BTreeMap<String, f64> = BTreeMap::new();
            for p in &target_procs {
                for (k, v) in layers(&p["pid"].to_string()) {
                    *sums.entry(k).or_default() += v;
                }
            }
            let mut rows: Vec<(String, f64)> = sums.into_iter().collect();
            rows.sort_by(|a, b| b.1.total_cmp(&a.1));
            let has_private = target_procs.iter().any(|p| p["private_bytes"].is_f64());
            let private: f64 = target_procs.iter().filter_map(|p| p["private_bytes"].as_f64()).sum();
            if has_private {
                let left_out = if c["engine"] == "firefox" { "" } else { " (strobe's tracing service left out)" };
                text += &format!("  {:>10}  OS private, all {} target processes{left_out}\n", mb(private), target_procs.len());
            } else if c["engine"] == "webkit" {
                text += "     refused  OS private memory (NotOnThisOs: WebKit's page categories follow below)\n";
            }
            let what = if c["engine"] == "firefox" {
                "JavaScript memory of the page (SpiderMonkey's reporters, after GC and CC)"
            } else if c["engine"] == "webkit" {
                "JavaScript heap of the page (WebKit Heap)"
            } else {
                "JS heap of the page (used, after GC)"
            };
            text += &format!("  {:>10}  {what}\n", mb(c["js_heap_bytes"].as_f64().unwrap_or(f64::NAN)));
            for (k, v) in rows.iter().take(top) {
                text += &format!("  {:>10}  {k}\n", mb(*v));
            }
            json!({
                "census": label,
                "os_private_bytes": if has_private { json!(private) } else { Value::Null },
                "js_heap_bytes": c["js_heap_bytes"],
                "layers": rows.iter().map(|(k, v)| json!({ "layer": k, "effective_bytes": v })).collect::<Vec<_>>(),
            })
        }
        "allocator" => {
            let mut rows: Vec<(String, String, f64, f64)> = target_procs
                .iter()
                .flat_map(|p| {
                    let ty = p["kind"].as_str().unwrap_or("").to_string();
                    alloc[p["pid"].to_string()]["allocators"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .map(move |a| (ty.clone(), a["path"].as_str().unwrap_or("").to_string(), a["effective"].as_f64().unwrap_or(0.0), a["size"].as_f64().unwrap_or(0.0)))
                })
                .filter(|r| !r.1.starts_with("tracing/"))
                .collect();
            rows.sort_by(|a, b| b.2.total_cmp(&a.2));
            if !rows.is_empty() || !phone {
                text += "  effective        size  process           allocator\n";
            }
            for (ty, path, e, s) in rows.iter().take(top) {
                text += &format!("  {:>9}  {:>10}  {:<16}  {path}\n", mb(*e), mb(*s), ty);
            }
            json!({ "census": label, "allocators": rows.iter().take(top).map(|(t, p, e, s)| json!({ "process": t, "path": p, "effective_bytes": e, "size_bytes": s })).collect::<Vec<_>>() })
        }
        _ => {
            if !procs.is_empty() || !phone {
                text += "  process               pid      private  working set    allocators  unexplained\n";
            }
            let mut rows = Vec::new();
            let mut over = false;
            for p in &procs {
                let pid = p["pid"].to_string();
                let private = p["private_bytes"].as_f64().unwrap_or(f64::NAN);
                let tracked: f64 = layers(&pid).iter().filter(|(k, _)| k.as_str() != "shared_memory").map(|(_, v)| v).sum();
                let has = alloc[&pid].is_object();
                // Allocators over the OS's private bytes leave no estimate: never a negative size.
                let unexplained = (has && private.is_finite() && private >= tracked).then_some(private - tracked);
                over |= has && private.is_finite() && private < tracked;
                let ty = p["kind"].as_str().unwrap_or("other");
                let flag = if inflated && page_pid.as_deref() == Some(pid.as_str()) { " *" } else { "" };
                text += &format!(
                    "  {:<18} {:>6}  {:>11}  {:>11}  {:>12}  {:>11}{flag}\n",
                    ty,
                    p["pid"],
                    mb(private),
                    mb(p["working_set_bytes"].as_f64().unwrap_or(f64::NAN)),
                    if has { mb(tracked) } else { "—".into() },
                    unexplained.map_or_else(|| "—".into(), mb)
                );
                rows.push(json!({
                    "pid": p["pid"],
                    "type": ty,
                    "strobe_own": ty == "tracing (strobe's)",
                    "page": page_pid.as_deref() == Some(pid.as_str()),
                    "name": p["name"],
                    "private_bytes": if private.is_finite() { json!(private) } else { Value::Null },
                    "working_set_bytes": p["working_set_bytes"],
                    "allocator_bytes": if has { json!(tracked) } else { Value::Null },
                    "unexplained_bytes": unexplained,
                    "layers": layers(&pid),
                }));
            }
            if c["engine"] == "firefox" {
                text += &format!(
                    "page      JavaScript {} used of {} (about:memory) · {} DOM nodes in the document · DOM {} ({} in nodes no document holds)\n",
                    mb(c["js_heap_bytes"].as_f64().unwrap_or(f64::NAN)),
                    mb(c["js_heap_total_bytes"].as_f64().unwrap_or(f64::NAN)),
                    c["dom"]["nodes"],
                    mb(c["dom"]["bytes"].as_f64().unwrap_or(f64::NAN)),
                    mb(c["dom"]["orphan_bytes"].as_f64().unwrap_or(f64::NAN))
                );
            } else if c["engine"] == "webkit" {
                let jit_part = c["js_heap_total_bytes"].as_f64().map(|t| format!("; {} with JIT", mb(t))).unwrap_or_default();
                text += &format!(
                    "page      JavaScript {} (WebKit Heap{jit_part}) · {} DOM nodes (WebCore categories; OS private memory refused on iOS)\n",
                    mb(c["js_heap_bytes"].as_f64().unwrap_or(f64::NAN)),
                    c["dom"]["nodes"],
                );
            } else {
                text += &format!(
                    "page      JS heap {} used of {} · {} DOM nodes · {} documents · {} listeners\n",
                    mb(c["js_heap_bytes"].as_f64().unwrap_or(f64::NAN)),
                    mb(c["js_heap_total_bytes"].as_f64().unwrap_or(f64::NAN)),
                    c["dom"]["nodes"],
                    c["dom"]["documents"],
                    c["dom"]["listeners"]
                );
            }
            match (inflated, page_pid.is_some()) {
                (true, true) => text += "* read after a heap snapshot of this process, which raises its memory (CensusContaminated)\n",
                (true, false) => text += "read after a heap snapshot of the page's renderer, which raises its memory; which renderer that is, this census does not say (CensusContaminated)\n",
                _ => {}
            }
            let chromium = !matches!(c["engine"].as_str(), Some("firefox" | "webkit"));
            if page_pid.is_none() && chromium && procs.iter().any(|p| p["type"] == "renderer") {
                text += "the page's renderer is not known in this census (it predates the page's mark, and its sizes leave more than one renderer)\n";
            }
            if over {
                text += "unexplained — where the allocators count more than the OS's private bytes (memory the OS charges elsewhere, such as shared or mapped): no estimate\n";
            }
            json!({ "census": label, "processes": rows, "page": { "pid": page_pid.as_deref().and_then(|p| p.parse::<u64>().ok()), "js_heap_bytes": c["js_heap_bytes"], "js_heap_total_bytes": c["js_heap_total_bytes"], "dom": c["dom"] }, "os_after_snapshot": inflated })
        }
    };
    if phone {
        json["phone"] = json!(true);
        json["processes_from"] = json!(if from_dump { "dump" } else { "none" });
        json["note"] = json!(note);
    }
    Ok(Output::new(text, json))
}

/// A census's processes (pid, name, Chromium type, OS private bytes and working set), and whether they
/// come from Chromium's memory dump: a phone's census reads no process of this computer's
/// (`processes: null`), so its processes are the dump's, typed from the names its trace gives them,
/// with no OS reading. Pid 0 is the dump's global part, no process.
fn census_processes(c: &Value) -> (Vec<Value>, bool) {
    if let Some(p) = c["processes"].as_array() {
        return (p.clone(), false);
    }
    let Some(dump) = c["allocators"].as_object().filter(|a| a.get("error").is_none()) else { return (Vec::new(), false) };
    let procs: Vec<Value> = dump
        .iter()
        .filter_map(|(pid, a)| {
            let pid: u64 = pid.parse().ok().filter(|p| *p > 0)?;
            let name = a["name"].as_str().unwrap_or("");
            let ty = match name {
                "Browser" => "browser".to_string(),
                "Renderer" => "renderer".to_string(),
                "GPU Process" => "GPU".to_string(),
                n if n.starts_with("Service: ") => n["Service: ".len()..].to_string(),
                // A census from before the dump kept names: a renderer by its V8 heap.
                "" if a["layers"]["v8"].is_number() => "renderer".to_string(),
                "" => "other".to_string(),
                n => n.to_lowercase(),
            };
            Some(json!({ "pid": pid, "name": name, "type": ty, "private_bytes": Value::Null, "working_set_bytes": Value::Null }))
        })
        .collect();
    (procs, true)
}

/// A native recording's memory by what holds it (`memory.json`): one reading (`--at`, default the last).
pub fn native_memory(dir: &Path, meta: &Value, by: &str, top: usize, at: Option<&str>) -> Result<Output> {
    let (r, label) = reading_at(dir, at)?;
    let (text, json) = crate::probe::memory::view(&r, &label, if by == "process" { "process" } else { "layer" }, top, &meta["values"]);
    Ok(Output::new(text, json))
}

/// A native recording's memory reading by index (`--at 0`, negative from the end) or its last.
fn reading_at(dir: &Path, at: Option<&str>) -> Result<(Value, String)> {
    let all = store::read_json(&dir.join("memory.json"))?["readings"].as_array().cloned().unwrap_or_default();
    if all.is_empty() {
        return Err(Error::usage(format!("this recording has no memory reading: {}", crate::probe::memory::why_empty())));
    }
    let i = match at.map(|a| a.parse::<i64>()) {
        None => all.len() - 1,
        Some(Ok(i)) if i < 0 => all.len().saturating_sub(i.unsigned_abs() as usize),
        Some(Ok(i)) => (i as usize).min(all.len() - 1),
        Some(Err(_)) => return Err(Error::usage("--at: a reading index (0 is the first, -1 the last)")),
    };
    Ok((all[i].clone(), format!("reading {} of {}", i + 1, all.len())))
}

/// Memory of `b` against `a` by process type and layer, at each recording's last census (native
/// recordings: by what holds it, at each one's last reading).
pub fn memory_diff(a: &Path, b: &Path, top: usize) -> Result<Output> {
    // A phone's census (no OS reading: its processes are the dump's) is named under the table.
    let phone = std::cell::Cell::new(false);
    let totals = |dir: &Path| -> Result<BTreeMap<String, f64>> {
        if dir.join("memory.json").exists() {
            return Ok(crate::probe::memory::totals(&reading_at(dir, None)?.0).into_iter().collect());
        }
        let (c, _) = census_at(dir, None)?;
        phone.set(phone.get() || c["processes"].is_null());
        let mut m: BTreeMap<String, f64> = BTreeMap::new();
        for p in &census_processes(&c).0 {
            let ty = p["type"].as_str().unwrap_or("other");
            let ty = match ty.split_once(".mojom.") {
                // strobe's own tracing service (rule 2).
                Some(("tracing", _)) => continue,
                Some((service, _)) => service,
                None => ty,
            };
            if let Some(pb) = p["private_bytes"].as_f64() {
                *m.entry(format!("OS private · {ty}")).or_default() += pb;
            }
            for (k, v) in c["allocators"][p["pid"].to_string()]["layers"].as_object().into_iter().flatten().filter(|(k, _)| k.as_str() != "tracing") {
                *m.entry(format!("{k} · {ty}")).or_default() += v.as_f64().unwrap_or(0.0);
            }
        }
        if let Some(jh) = c["js_heap_bytes"].as_f64() {
            m.insert("JS heap · page".into(), jh);
        }
        Ok(m)
    };
    let (base, head) = (totals(a)?, totals(b)?);
    let mut rows: Vec<(String, f64, f64)> =
        base.keys().chain(head.keys()).collect::<std::collections::BTreeSet<_>>().into_iter().map(|k| (k.clone(), base.get(k).copied().unwrap_or(0.0), head.get(k).copied().unwrap_or(0.0))).collect();
    rows.sort_by(|x, y| (y.2 - y.1).abs().total_cmp(&(x.2 - x.1).abs()));
    let mut text = String::from("memory diff · last census of each · by layer and process type\n        base        head        Δ  layer · process\n");
    for (k, x, y) in rows.iter().take(top) {
        text += &format!("  {:>10}  {:>10}  {:>9}  {k}\n", fmt::bytes(*x), fmt::bytes(*y), fmt::signed(y - x, "bytes"));
    }
    if phone.get() {
        text += "a phone's page: its processes are on the phone, from Chromium's memory dump, with no OS private bytes (strobe reads no process memory of the phone's)\n";
    }
    Ok(Output::new(text, json!({ "rows": rows.iter().map(|(k, x, y)| json!({ "layer": k, "base_bytes": x, "head_bytes": y, "delta_bytes": y - x })).collect::<Vec<_>>(), "phone": phone.get() })))
}

/// A recording's census by index (`--at 0`, negative from the end) or its last, with a label.
fn census_at(dir: &Path, at: Option<&str>) -> Result<(Value, String)> {
    let meta = store::read_json(&dir.join("meta.json")).unwrap_or(Value::Null);
    let all = meta["census"].as_array().cloned().unwrap_or_default();
    if all.is_empty() {
        return Err(Error::usage("this recording has no census; record --probes memory, a bench leak run (--probes memory) or an attribution block takes one"));
    }
    let i = match at.map(|a| a.parse::<i64>()) {
        None => all.len() - 1,
        Some(Ok(i)) if i < 0 => all.len().saturating_sub(i.unsigned_abs() as usize),
        Some(Ok(i)) => (i as usize).min(all.len() - 1),
        Some(Err(_)) => return Err(Error::usage("--at: a census index (0 is the first, -1 the last)")),
    };
    Ok((all[i].clone(), format!("census {} of {}", i + 1, all.len())))
}

/// Leak suspects of a memory recording: objects allocated between its first and middle snapshots that
/// are still alive in the last, by class, with retainer paths (SPEC §8.6).
fn leak_files(rec: &Rec) -> Result<[std::path::PathBuf; 2]> {
    firefox_gap(rec, Gap::Heap)?;
    let s = |name: &str| rec.dir.join(format!("{name}.heapsnapshot.gz"));
    if !s("heap-s0").exists() || !s("heap-s1").exists() || !s("heap-0").exists() {
        return Err(Error::usage("this recording has no leak snapshots; record with --probes memory"));
    }
    Ok([s("heap-s0"), s("heap-s1")])
}

pub fn leaks(rec: &Rec, top: usize, reveal: bool) -> Result<Output> {
    let [a, b] = leak_files(rec)?;
    let v = crate::heapsnap::leaks(&crate::heapsnap::ids(&a)?, &crate::heapsnap::ids(&b)?, rec.snapshot()?, top, reveal);
    Ok(Output::new(crate::heapsnap::leaks_text(&v, top), v))
}

/// The recording's heap snapshot: `heap-0.heapsnapshot.gz` (the last one), or an imported `.heapsnapshot`.
fn heap_file(dir: &Path) -> Result<std::path::PathBuf> {
    if dir.join("heap-0.heapsnapshot.gz").is_file() {
        return Ok(dir.join("heap-0.heapsnapshot.gz"));
    }
    [dir.to_path_buf(), dir.join("import")]
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flat_map(|rd| rd.flatten().map(|e| e.path()))
        .find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().contains(".heapsnapshot")))
        .ok_or_else(|| Error::usage("this recording has no heap snapshot; bench attribution blocks and record --probes attribution|memory take one"))
}

/// A float trace_processor printed as `%f`, six decimals (`71.000000`), as it is (`71`, `1507.64`):
/// only trailing zeros go, so no digit it gave is lost; `-0.000000` is `0`. Any other cell (a string
/// such as `0.20`) is left as it came.
fn sql_number(cell: &str) -> &str {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    match cell.strip_prefix('-').unwrap_or(cell).split_once('.') {
        Some((int, frac)) if digits(int) && frac.len() == 6 && digits(frac) => match cell.trim_end_matches('0').trim_end_matches('.') {
            "-0" => "0",
            short => short,
        },
        _ => cell,
    }
}

fn sql_view(trace: &Path, sql: &str, limit: usize, times: bool) -> Result<Output> {
    let mut t = engine::query(trace, sql)?;
    for c in t.rows.iter_mut().flatten().flatten() {
        let short = sql_number(c);
        if short.len() < c.len() {
            *c = short.to_string();
        }
    }
    let shown = t.rows.len().min(limit);
    let mut text = t.columns.join(" | ") + "\n";
    // A view's own query (`gpu`, `threads`): its `*_ms` columns are times, printed as times are
    // (`--unit`), in columns as wide as their widest cell. The JSON keeps the query's cells.
    if times {
        fn ms(c: &str) -> Option<&str> {
            c.strip_suffix("_ms")
        }
        let mut table = vec![t.columns.iter().map(|c| ms(c).unwrap_or(c).to_string()).collect::<Vec<_>>()];
        for r in &t.rows[..shown] {
            table.push(
                r.iter()
                    .zip(&t.columns)
                    .map(|(c, col)| match (ms(col), c.as_deref().and_then(|x| x.parse::<f64>().ok())) {
                        (Some(_), Some(v)) => fmt::time(v),
                        _ => c.clone().unwrap_or_else(|| "NULL".into()),
                    })
                    .collect(),
            );
        }
        let right: Vec<bool> = t.columns.iter().map(|c| ms(c).is_some() || ["n", "pid", "tid", "tasks"].contains(&c.as_str())).collect();
        text = fmt::table(&table, &right, "", 2);
    } else {
        for r in &t.rows[..shown] {
            text += &r.iter().map(|c| c.clone().unwrap_or_else(|| "NULL".into())).collect::<Vec<_>>().join(" | ");
            text.push('\n');
        }
    }
    if t.rows.len() > shown {
        text += &format!("… {} more rows (narrow the query or raise --top)\n", t.rows.len() - shown);
    }
    let rows: Vec<Value> = t.rows[..shown].iter().map(|r| json!(r)).collect();
    Ok(Output::new(text, json!({ "columns": t.columns, "rows": rows, "total": t.rows.len() })))
}

fn frames(trace: &Path, meta: &Value, top: usize) -> Result<Output> {
    let token = meta["token"].as_str().unwrap_or("");
    let mut data = metrics::read(trace, token)?;
    // Only the measured runs: a block's trace marks its warmup runs, a check's run after its reset and
    // every case of a batch too.
    if let Some(keys) = rec::measured_keys(meta) {
        data.measured_where(|i| keys.iter().any(|k| *k == format!("run:{i}")));
    }
    let values = metrics::block_values(&data, "run", &BTreeMap::new());
    let g = |k: &str| values.get(k).copied().unwrap_or(f64::NAN);
    let mut all: Vec<metrics::Vsync> = data
        .frames
        .iter()
        .filter(|(k, _)| k.contains(":run:") && !k.contains(":phase:"))
        .flat_map(|(_, f)| metrics::vsyncs(f))
        .filter(|s| matches!(s.outcome, metrics::Outcome::Presented | metrics::Outcome::Partial))
        .collect();
    let ms = metrics::run_frames(&data);
    let dropped = g("frames.dropped.pct");
    let q = |p: f64| metrics::percentile(&ms, p);
    let mut text = format!(
        "frames    {} presented · {} of vsyncs dropped · display {}\nlatency   p50 {} · p90 {} · p95 {} · p99 {} · max {}\ninterval  p50 {} · p95 {} · p99 {} · max {}\n",
        ms.len(),
        fmt::value(dropped, "%"),
        if g("display.hz").is_finite() { format!("{:.0} Hz", g("display.hz")) } else { "?".into() },
        fmt::value(q(0.5), "ms"),
        fmt::value(q(0.9), "ms"),
        fmt::value(q(0.95), "ms"),
        fmt::value(q(0.99), "ms"),
        fmt::value(*ms.last().unwrap_or(&f64::NAN), "ms"),
        fmt::value(g("frames.interval.p50"), "ms"),
        fmt::value(g("frames.interval.p95"), "ms"),
        fmt::value(g("frames.interval.p99"), "ms"),
        fmt::value(g("frames.interval.max"), "ms"),
    );
    let phases = metrics::phase_frames(&data);
    if !phases.is_empty() {
        text += "phases\n";
        for (name, v) in &phases {
            let mut v = v.clone();
            v.sort_by(|a, b| a.total_cmp(b));
            text += &format!("  {:<14} {:>5} frames · p50 {} · p95 {}\n", name, v.len(), fmt::value(metrics::percentile(&v, 0.5), "ms"), fmt::value(metrics::percentile(&v, 0.95), "ms"));
        }
    }
    if g("frames.renderer.p95").is_finite() {
        text += &format!(
            "stages    renderer p95 {} · GPU process (raster, draw to swap) p95 {} · {:.0} of {} frames carry both (stage spans, not busy time)\n",
            fmt::value(g("frames.renderer.p95"), "ms"),
            fmt::value(g("frames.viz.p95"), "ms"),
            g("frames.split.n"),
            ms.len()
        );
    } else if !ms.is_empty() {
        text += &format!("stages    refused: {}\n", metrics::split_refusal("frames.renderer.p95", &values).summary);
    }
    all.sort_by_key(|f| std::cmp::Reverse(f.cost));
    let t0 = data.windows.first().map(|w| w.t0).unwrap_or(0);
    let worst_frames: Vec<&metrics::Vsync> = all.iter().take(top.min(10)).collect();
    let (stages, procs) = frame_stages(trace, meta, &worst_frames).unwrap_or_default();
    // Frames by process: a page's second renderer, or the browser's own UI, presents frames the
    // page's numbers above leave out.
    if procs.len() > 1 || procs.iter().any(|p| !p.4) {
        text += &format!(
            "by process  {}   (vsyncs presented / reported; * the page's)\n",
            procs.iter().map(|(name, pid, shown, all, page)| format!("{name} {pid}{}: {shown} / {all}", if *page { "*" } else { "" })).collect::<Vec<_>>().join(" · ")
        );
    }
    text += "worst\n";
    let mut worst: Vec<Value> = Vec::new();
    let to_ms = |ns: i64| ns as f64 / 1e6;
    // Columns as wide as their widest time (`18,309,038 µs`).
    let cw = worst_frames.iter().map(|f| fmt::time(to_ms(f.cost)).chars().count()).max().unwrap_or(0);
    let aw = worst_frames.iter().map(|f| fmt::time(to_ms(f.ts - t0)).chars().count()).max().unwrap_or(0);
    for f in &worst_frames {
        let ms = to_ms;
        let at_ms = ms(f.ts - t0);
        text += &format!("  {:>cw$} at {:>aw$}", fmt::time(ms(f.cost)), fmt::time(at_ms));
        if let Some((c, gp)) = f.split {
            text += &format!("  renderer {} · GPU process {}", fmt::value(ms(c), "ms"), fmt::value(ms(gp), "ms"));
        }
        text.push('\n');
        let mine = stages.get(&f.report).cloned().unwrap_or_default();
        if !mine.is_empty() {
            let parts: Vec<String> = mine
                .iter()
                .map(|(name, dur, kids)| {
                    let inner = if kids.is_empty() { String::new() } else { format!(" ({})", kids.iter().map(|(n, d)| format!("{n} {}", fmt::time(ms(*d)))).collect::<Vec<_>>().join(", ")) };
                    format!("{name} {}{inner}", fmt::time(ms(*dur)))
                })
                .collect();
            text += &format!("             {}\n", parts.join(" · "));
        }
        worst.push(json!({
            "at_ms": at_ms, "ms": ms(f.cost),
            "renderer_ms": f.split.map(|s| ms(s.0)), "viz_ms": f.split.map(|s| ms(s.1)),
            "stages": mine.iter().map(|(n, d, kids)| json!({ "name": n, "ms": ms(*d), "stages": kids.iter().map(|(k, kd)| json!({ "name": k, "ms": ms(*kd) })).collect::<Vec<_>>() })).collect::<Vec<_>>(),
        }));
    }
    let phase_json: BTreeMap<&String, usize> = phases.iter().map(|(k, v)| (k, v.len())).collect();
    Ok(Output::new(
        text,
        json!({
            "presented": ms.len(), "dropped_pct": dropped, "latency": { "p50": q(0.5), "p95": q(0.95), "p99": q(0.99) }, "values": values, "worst": worst, "phases": phase_json,
            "processes": procs.iter().map(|(name, pid, shown, all, page)| json!({ "process": name, "pid": pid, "presented": shown, "reported": all, "page": page })).collect::<Vec<_>>(),
        }),
    ))
}

/// A frame's report stage by stage (Chromium's `PipelineReporter` children, each with its own children).
type Stages = Vec<(String, i64, Vec<(String, i64)>)>;

/// Frames by process over the measured runs: (process, pid, vsyncs it presented, vsyncs it reported,
/// the page's renderer). The page's frames above are its renderers'; another process's (a second
/// renderer, the browser's own UI) are here only.
type ByProcess = Vec<(String, i64, i64, i64, bool)>;

/// The stages of these frames' presented reports, by the report's slice id, and the frames of every
/// process with frame reports in the measured runs (one load of the trace for both).
fn frame_stages(trace: &Path, meta: &Value, frames: &[&metrics::Vsync]) -> Result<(BTreeMap<i64, Stages>, ByProcess)> {
    let ids: Vec<String> = frames.iter().filter(|f| f.report >= 0).map(|f| f.report.to_string()).collect();
    let (pre, _, _) = in_measured_runs(meta);
    // Without strobe's run marks (an imported trace) the whole trace, and no page.
    let (runs, page) = if pre.is_empty() { ("", "0") } else { (" join (select distinct t0, t1 from _w) w on s.ts >= w.t0 and s.ts < w.t1", "pr.upid in (select upid from _wp)") };
    let seq = "extract_arg(s.arg_set_id, 'frame_reporter.frame_source') || ':' || extract_arg(s.arg_set_id, 'frame_reporter.frame_sequence')";
    let sql = format!(
        "{pre}select 'stage', p.id, c.ts, c.id, c.name, c.dur, g.ts, g.name, g.dur from slice p join slice c on c.parent_id = p.id left join slice g on g.parent_id = c.id
         where {}
         union all
         select 'proc', pr.pid, null, null, coalesce(pr.name, '?'),
           count(distinct case when extract_arg(s.arg_set_id, 'frame_reporter.state') in ('STATE_PRESENTED_ALL', 'STATE_PRESENTED_PARTIAL') then {seq} end),
           count(distinct {seq}), {page}, null
         from slice s join track tr on s.track_id = tr.id join process pr on pr.upid = extract_arg(tr.dimension_arg_set_id, 'upid'){runs}
         where s.name = 'PipelineReporter' group by pr.upid
         order by 1, 2, 3, 4, 7",
        if ids.is_empty() { "0".to_string() } else { format!("p.id in ({})", ids.join(",")) },
    );
    let t = engine::query(trace, &sql)?;
    let num = |c: &Option<String>| c.as_deref().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0) as i64;
    let mut out: BTreeMap<i64, Stages> = BTreeMap::new();
    let mut procs: ByProcess = Vec::new();
    let mut last_stage: Option<i64> = None;
    for r in &t.rows {
        if r[0].as_deref() == Some("proc") {
            procs.push((r[4].clone().unwrap_or_default(), num(&r[1]), num(&r[5]), num(&r[6]), num(&r[7]) == 1));
            continue;
        }
        let stages = out.entry(num(&r[1])).or_default();
        if last_stage != Some(num(&r[3])) {
            stages.push((r[4].clone().unwrap_or_default(), num(&r[5]), Vec::new()));
            last_stage = Some(num(&r[3]));
        }
        if let (Some(kid), Some(last)) = (r[7].clone(), stages.last_mut()) {
            last.2.push((kid, num(&r[8])));
        }
    }
    procs.sort_by(|a, b| b.2.cmp(&a.2).then(a.1.cmp(&b.1)));
    Ok((out, procs))
}

/// The GPU process's work by name: its named tasks below the task loop on every thread (WebGPU command
/// decoding, shader and pipeline compiles from attribution traces, compositing), heaviest first. A shader
/// compile that froze a game's first frame for seconds showed here and nowhere in the page's own threads.
fn gpu(trace: &Path, meta: &Value, top: usize) -> Result<Output> {
    let (pre, runs, said) = in_measured_runs(meta);
    let sql = format!(
        "{pre}select t.name as thread, s.name, count(*) as n, sum(s.dur) / 1e6 as total_ms, max(s.dur) / 1e6 as max_ms
         from slice s join thread_track tt on s.track_id = tt.id join thread t using(utid) join process p using(upid)
         where p.name glob 'GPU*' and s.dur > 0 and s.depth between 1 and 4
           and s.name not in ('RunTask', 'ThreadControllerImpl::RunTask', 'ThreadPool_RunTask', 'SequenceManager::DoIdleWork'){runs}
         group by 1, 2 order by total_ms desc limit {top}"
    );
    let mut out = sql_view(trace, &sql, top, true)?;
    out.text += &said;
    // The adapter the page drew on (the OS's per-process GPU statistics), where the recording read it.
    if let Some(line) = super::bench::gpu_line(&meta["screen"]["gpu"]) {
        out.text += &format!("adapter  {line}\n");
        out.json["adapter"] = meta["screen"]["gpu"].clone();
    }
    Ok(out)
}

/// A recording without a trace (a native app, a process from outside): per adapter the target's
/// engine use and GPU memory from the OS's GPU statistics, the device's own counters, and what
/// reached the screen.
fn native_gpu(dir: &Path, meta: &Value, probe: bool) -> Result<Output> {
    let os = if meta["os"].is_object() { &meta["os"] } else { &meta["gpu"] };
    if !os.is_object() {
        return Err(Error::usage("this recording has no GPU readings: they come from timing recordings (record, bench blocks kept with --keep or attribution)"));
    }
    if probe {
        return available();
    }
    let mut text = String::from("gpu      the target's processes, from the OS's per-process GPU statistics, over the recording\n");
    if let Some(line) = super::bench::gpu_line(&meta["screen"]["gpu"]) {
        text += &format!("adapter  {line}\n");
    }
    if os["gpu_3d_pct"].is_null() && os["gpu_memory"].is_null() && os["vram_dedicated_mb"].is_null() {
        text += "         unavailable: no per-process GPU statistics opened on this machine (Windows D3DKMT, Linux DRM client statistics, macOS IORegistry)\n";
    }
    let engines = os["gpu_adapters"].as_object().cloned().unwrap_or_default();
    let memory = os["gpu_memory"].as_object().cloned().unwrap_or_default();
    let mut adapters: Vec<&String> = engines.keys().chain(memory.keys()).collect();
    adapters.sort();
    adapters.dedup();
    for a in &adapters {
        text += &format!("  {a}\n");
        if let Some(e) = engines.get(*a).and_then(Value::as_object) {
            let mut e: Vec<(&String, f64)> = e.iter().map(|(k, v)| (k, v.as_f64().unwrap_or(0.0))).collect();
            e.sort_by(|x, y| y.1.total_cmp(&x.1));
            text += &format!("    engines  {}   (% of one engine)\n", e.iter().map(|(k, v)| format!("{k} {}", fmt::value(*v, "%"))).collect::<Vec<_>>().join(" · "));
        }
        if let Some(m) = memory.get(*a) {
            let mb = |i: usize| fmt::bytes(m[i].as_f64().unwrap_or(f64::NAN) * 1048576.0);
            text += &format!("    memory   {} dedicated · {} shared (system memory)\n", mb(0), mb(1));
        }
    }
    if adapters.is_empty() && !os["gpu_3d_pct"].is_null() {
        text += &format!(
            "  no engine time or GPU memory on any adapter · VRAM {} · shared {}\n",
            fmt::value(os["vram_dedicated_mb"].as_f64().unwrap_or(f64::NAN), "MB"),
            fmt::value(os["vram_shared_mb"].as_f64().unwrap_or(f64::NAN), "MB")
        );
    }
    if let Some(u) = os["gpu_unreadable_pids"].as_array().filter(|u| !u.is_empty()) {
        text += &format!("  unreadable: pids {} (left out of the sums)\n", u.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", "));
    }
    let d = &os["device"];
    if d.is_object() {
        let f = |k: &str, unit: &str| d[k].as_f64().map(|v| format!("{} {}", k.split('_').next().unwrap_or(k), fmt::value(v, unit)));
        let parts: Vec<String> = [f("gpu_pct", "%"), f("mem_pct", "%"), f("gfx_mhz", "MHz"), f("power_w", "W"), f("temp_c", "°C"), f("vram_used_mb", "MB")].into_iter().flatten().collect();
        text += &format!("device   {}: {}   (the whole device, every process on it)\n", d["name"].as_str().unwrap_or("?"), parts.join(" · "));
    }
    let v = &meta["values"];
    match (v["frames.n"].as_f64().filter(|_| dir.join("presents.json.gz").exists()), meta["presents_refused"].as_str().or(meta["os_frames"]["refused"].as_str())) {
        (Some(n), _) => text += &format!("frames   {n:.0} on screen (OS present events): strobe query {} frames\n", handle_of(dir)),
        (None, Some(why)) => text += &format!("frames   refused: {why}\n"),
        _ => text += "frames   none recorded (no window, or no OS present events)\n",
    }
    if meta["session_locked"] == true {
        text += "session  locked: the compositor drew nothing, so engine use describes no screen (GPU memory does not depend on it)\n";
    }
    text += "no trace: a browser GPU process's tasks by name need a Chromium attribution recording\n";
    Ok(Output::new(text, json!({ "gpu": os, "adapter": meta["screen"]["gpu"], "frames": v["frames.n"], "session_locked": meta["session_locked"] })))
}

/// A native recording's threads, busiest waker first: context switches per second over the measured
/// runs, CPU per run from each thread's cycle counter, and its name.
pub fn native_threads(dir: &Path, top: usize) -> Result<Output> {
    let rows = store::read_json(&dir.join("threads.json"))?;
    let mut all = rows.as_array().cloned().unwrap_or_default();
    let meta = store::read_json(&dir.join("meta.json")).unwrap_or(Value::Null);
    // A recording of a window (`record`) has no runs: its one span is the whole recording.
    let whole = meta["kind"] == "record";
    let (cpu_col, per) = if whole { ("cpu", "over the recording") } else { ("cpu/run", "per run") };
    // Without per-thread wakes (macOS counts them per process), the busiest threads come first.
    if all.iter().all(|r| r["wakes_per_s"].as_f64().is_none()) {
        all.sort_by(|a, b| b["cpu_ms_per_run"].as_f64().unwrap_or(0.0).total_cmp(&a["cpu_ms_per_run"].as_f64().unwrap_or(0.0)));
    }
    let mut text = format!("{:>10}  {:>13}  {:>7}  {:>7}  {:<6}  thread (started in)\n", "wakes/s", cpu_col, "pid", "tid", "whose");
    for r in all.iter().take(top) {
        let started = r["module"].as_str().map(|m| format!(" ({m})")).unwrap_or_default();
        text += &format!(
            "{:>10}  {:>13}  {:>7}  {:>7}  {:<6}  {}{started}\n",
            fmt::value(r["wakes_per_s"].as_f64().unwrap_or(f64::NAN), "count"),
            fmt::value(r["cpu_ms_per_run"].as_f64().unwrap_or(f64::NAN), "ms"),
            r["pid"].as_u64().unwrap_or(0),
            r["tid"].as_u64().unwrap_or(0),
            r["origin"].as_str().unwrap_or("?"),
            r["name"].as_str().unwrap_or("(unnamed)")
        );
    }
    if all.len() > top {
        text += &format!("… {} more (--top)\n", all.len() - top);
    }
    // Every thread's wakes by whose it is, those strobe could not place (`?`: no start module) too, so
    // the parts add up to the total.
    let sum = |o: Option<&str>| all.iter().filter(|r| r["origin"].as_str().filter(|s| *s != "?") == o).map(|r| r["wakes_per_s"].as_f64().unwrap_or(0.0)).sum::<f64>();
    if all.iter().any(|r| r["wakes_per_s"].is_number()) {
        let unknown = sum(None);
        text += &format!(
            "by whose threads: app {} · driver {} · os {}{} (wakes/s)\n",
            fmt::value(sum(Some("app")), "count"),
            fmt::value(sum(Some("driver")), "count"),
            fmt::value(sum(Some("os")), "count"),
            if unknown > 0.0 { format!(" · unknown (?) {}", fmt::value(unknown, "count")) } else { String::new() }
        );
    }
    // What the processes spent that no listed thread did: threads that exited during a run (a worker
    // that ran 4 of a run's 6 s once hid that way).
    let runs = store::read_json(&dir.join("runs.json")).unwrap_or(Value::Null);
    let per_run: Vec<f64> = runs.as_array().into_iter().flatten().filter_map(|r| r["cpu_ms"].as_f64()).collect();
    // Where cycles are not time, no thread has CPU: say why rather than show blanks.
    let cpu_refused = meta["refused"]["ui.cpu"].as_str().or(meta["refused"]["cpu.work"].as_str()).filter(|r| r.starts_with("NotOnThisOs")).map(str::to_string);
    if let Some(why) = &cpu_refused {
        text += &format!("{cpu_col} refused: {why}\n");
    }
    let total = if per_run.is_empty() { meta["values"]["cpu.work"].as_f64().unwrap_or(f64::NAN) } else { metrics::median(&per_run) };
    let listed: f64 = all.iter().filter_map(|r| r["cpu_ms_per_run"].as_f64()).sum();
    let gone = total - listed;
    if gone.is_finite() && gone > (0.05 * total).max(5.0) {
        let end = if whole { "the recording's end (they exited during it)" } else { "the run's end (they exited during it)" };
        text +=
            &format!("{} {per} in threads no longer there at {end}: the processes spent {} {per}, the threads above {}\n", fmt::value(gone, "ms"), fmt::value(total, "ms"), fmt::value(listed, "ms"));
    }
    // Threads that exited before the end, from the timeline (sampled every half second from the launch).
    let mut exited_json = Vec::new();
    if let Ok(Some(t)) = read_timeline(dir) {
        let end = t.ticks.last().copied().unwrap_or(t.t0_ns);
        let listed: std::collections::BTreeSet<u64> = all.iter().filter_map(|r| r["tid"].as_u64()).collect();
        let mut gone: Vec<(&crate::probe::wakes::TimelineThread, f64)> = t
            .threads
            .iter()
            .enumerate()
            .filter(|(_, th)| th.last_ns < end && !listed.contains(&(th.tid as u64)))
            .map(|(i, th)| (th, t.cells.iter().filter(|c| c.1 as usize == i).map(|c| c.2 as f64).sum::<f64>()))
            .filter(|(_, cpu)| *cpu >= 1.0)
            .collect();
        gone.sort_by(|a, b| b.1.total_cmp(&a.1));
        if !gone.is_empty() {
            text += "exited before the end (CPU over the whole recording):\n";
        }
        let s = |ns: u64| ns.saturating_sub(t.t0_ns) as f64 / 1e9;
        for (th, cpu) in gone.iter().take(top) {
            let module = th.module.as_deref().map(|m| m.rsplit(['\\', '/']).next().unwrap_or(m));
            text += &format!(
                "  {:>10}  {:>7}  {:<6}  {}{}  ran {:.1}-{:.1} s\n",
                fmt::value(*cpu, "ms"),
                th.tid,
                th.module.as_deref().map(crate::probe::wakes::origin).unwrap_or("?"),
                th.name.as_deref().unwrap_or("(unnamed)"),
                module.map(|m| format!(" ({m})")).unwrap_or_default(),
                s(th.first_ns),
                s(th.last_ns)
            );
            exited_json.push(json!({ "pid": th.pid, "tid": th.tid, "name": th.name, "module": module, "cpu_ms": cpu, "first_s": s(th.first_ns), "last_s": s(th.last_ns) }));
        }
    }
    text += &format!(
        "wakes: context switches of the thread, each a switch in to run after a wait or a preemption; whose: app (started in the app's own code), driver (a device driver's user-mode part, such as GPU driver workers), os (the OS's thread pool and services; a pool thread may run the app's callbacks).{}\n",
        if dir.join("timeline.json.gz").exists() { " When each ran: --timeline" } else { " A thread that exited during a run is not listed" }
    );
    Ok(Output::new(
        text,
        json!({ "threads": all.into_iter().take(top).collect::<Vec<_>>(), "cpu_ms_per_run": total, "cpu_refused": cpu_refused, "exited_ms_per_run": gone.is_finite().then_some(gone.max(0.0)), "exited": exited_json }),
    ))
}

/// The thread timeline, None without one; a file outside the store that expands past its bound is
/// refused with why (`store::gunzip`).
fn read_timeline(dir: &Path) -> Result<Option<crate::probe::wakes::TimelineData>> {
    let path = dir.join("timeline.json.gz");
    if !path.is_file() {
        return Ok(None);
    }
    let text = store::gunzip_string(&path).map_err(|e| Error::usage(e.to_string()))?;
    Ok(serde_json::from_str(&text).ok())
}

/// `query <rec> threads --timeline`: each thread's CPU per second (or coarser for long recordings)
/// from the launch, threads that exited included, with the runs marked, so work that starts late
/// and stops shows when it ran.
fn thread_timeline(dir: &Path, top: usize, probe: bool) -> Result<Output> {
    let t = read_timeline(dir)?
        .ok_or_else(|| Error::usage("this recording has no thread timeline: native recordings, kept native bench blocks and script attribution recordings have one (from this strobe on)"))?;
    if probe {
        return available();
    }
    let meta = store::read_json(&dir.join("meta.json")).unwrap_or(Value::Null);
    let end = t.ticks.last().copied().unwrap_or(t.t0_ns);
    let span_s = end.saturating_sub(t.t0_ns) as f64 / 1e9;
    // One column per second, coarser past 120 s.
    let bin_s = (span_s / 120.0).ceil().max(1.0);
    let cols = ((span_s / bin_s).ceil() as usize).max(1);
    let col = |ns: u64| (((ns.saturating_sub(t.t0_ns)) as f64 / 1e9 / bin_s) as usize).min(cols - 1);
    // Spans to mark: the measured runs (a native block's frames.json, a script recording's runs.json) or
    // the recording's own window.
    let mut marks: Vec<(String, u64, u64)> = t.marks.clone();
    let runs = store::read_json(&dir.join("frames.json")).ok().and_then(|f| f["runs"].as_object().cloned()).map(|o| o.into_iter().collect::<Vec<_>>()).or_else(|| {
        store::read_json(&dir.join(crate::probe::cpu::RUNS)).ok().and_then(|f| f["runs"].as_array().cloned()).map(|a| a.into_iter().map(|r| (r["run"].as_str().unwrap_or("").to_string(), r)).collect())
    });
    for (name, r) in runs.into_iter().flatten() {
        marks.push((name, r["t0_ns"].as_u64().unwrap_or(0), r["t1_ns"].as_u64().unwrap_or(0)));
    }
    let n = t.threads.len();
    // Each tick spread over the columns it covers: a late tick fills them, never one column at its middle.
    let (cpu, wakes) = t.binned((bin_s * 1e9) as u64, cols);
    // Threads whose CPU was not read (all of them where the OS refused it): their cells hold wakes only.
    let unknown = |i: usize| t.cpu_refused.is_some() || t.cpu_unknown.contains(&(i as u32));
    let total: Vec<f64> = cpu.iter().map(|r| r.iter().sum()).collect();
    let woke: Vec<f64> = wakes.iter().map(|r| r.iter().sum()).collect();
    let mut order: Vec<usize> = (0..n).filter(|&i| total[i] > 0.0 || woke[i] > 0.0 || unknown(i)).collect();
    order.sort_by(|&a, &b| unknown(a).cmp(&unknown(b)).then(total[b].total_cmp(&total[a])).then(woke[b].total_cmp(&woke[a])));
    let ramp = |pct: f64| match pct {
        p if p < 1.0 => ' ',
        p if p < 5.0 => '.',
        p if p < 15.0 => ':',
        p if p < 30.0 => '-',
        p if p < 50.0 => '=',
        p if p < 75.0 => '+',
        p if p < 95.0 => '*',
        _ => '#',
    };
    let mut axis = vec![' '; cols];
    for c in (0..cols).step_by(10) {
        for (k, ch) in format!("{}s", (c as f64 * bin_s) as u64).chars().enumerate() {
            if c + k < cols {
                axis[c + k] = ch;
            }
        }
    }
    let mut runs = vec![' '; cols];
    for (_, a, b) in &marks {
        if *b > *a {
            for c in runs.iter_mut().take(col(*b) + 1).skip(col(*a)) {
                *c = '~';
            }
        }
    }
    let mut text = format!(
        "threads over time · one column per {} from the launch (a process recorded from outside: from the start) · CPU of one core: ' ' <1%  . <5%  : <15%  - <30%  = <50%  + <75%  * <95%  # more\n",
        if bin_s == 1.0 { "second".to_string() } else { format!("{bin_s:.0} s") }
    );
    // Where the OS gave no thread's CPU, no strip: a blank one would read as idle.
    match &t.cpu_refused {
        Some(why) => text += &format!("! no thread's CPU: {why}; wakes only\n"),
        None => text += &format!("|{}|\n|{}|  {}\n", axis.iter().collect::<String>(), runs.iter().collect::<String>(), if marks.is_empty() { "" } else { "~ runs" }),
    }
    let s = |ns: u64| ns.saturating_sub(t.t0_ns) as f64 / 1e9;
    // The recorded process's main thread, where the recording names it (`cpu.main`).
    let main = meta["pid"].as_u64().zip(meta["main_tid"].as_u64().or_else(|| meta["main_thread"].as_u64()));
    let mut rows = Vec::new();
    for &i in order.iter().take(top) {
        let th = &t.threads[i];
        let strip: String = match (&t.cpu_refused, unknown(i)) {
            (Some(_), _) => String::new(),
            (None, true) => format!("|{}|  ", "?".repeat(cols)),
            (None, false) => format!("|{}|  ", cpu[i].iter().map(|ms| ramp(100.0 * ms / (bin_s * 1e3))).collect::<String>()),
        };
        let exited = th.last_ns < end;
        let module = th.module.as_deref().map(|m| m.rsplit(['\\', '/']).next().unwrap_or(m));
        let peak = wakes[i].iter().copied().fold(0.0, f64::max) / bin_s;
        let is_main = main == Some((u64::from(th.pid), u64::from(th.tid)));
        text += &format!(
            "{strip}{:>13}  {:>6} wakes/s peak  {:.1}-{:.1} s{}  {:<6} {}{} {}{}\n",
            if unknown(i) { "?".to_string() } else { fmt::value(total[i], "ms") },
            // macOS counts wakes per process, not per thread.
            if cfg!(target_os = "macos") { "—".to_string() } else { fmt::value(peak, "count") },
            s(th.first_ns),
            s(th.last_ns),
            if exited { " exited" } else { "" },
            th.module.as_deref().map(crate::probe::wakes::origin).unwrap_or("?"),
            th.name.as_deref().unwrap_or("(unnamed)"),
            module.map(|m| format!(" ({m})")).unwrap_or_default(),
            th.tid,
            if is_main { " main thread" } else { "" }
        );
        let known = !unknown(i);
        rows.push(json!({
            "pid": th.pid, "tid": th.tid, "name": th.name, "module": module, "first_s": s(th.first_ns), "last_s": s(th.last_ns), "exited": exited, "main_thread": is_main,
            "cpu_ms": known.then_some(total[i]), "cpu_ms_per_bin": known.then_some(&cpu[i]), "wakes_per_bin": wakes[i],
        }));
    }
    if order.len() > top {
        text += &format!("… {} more (--top)\n", order.len() - top);
    }
    // Windows keeps an exited thread's last count through the handle strobe holds; elsewhere the CPU
    // after its last sample went with it.
    let windows = meta["env"]["os"].as_str().map_or(cfg!(windows), |os| os.starts_with("Windows"));
    text += &format!(
        "sampled every {} ms ({} of strobe's own CPU){}{}\n",
        t.interval_ms,
        t.cpu_ms.map(|c| fmt::value(c, "ms")).unwrap_or_else(|| "?".into()),
        if windows || t.cpu_refused.is_some() { "" } else { "; a thread's CPU between its last sample and its exit is missing" },
        if t.cpu_unknown.is_empty() || t.cpu_refused.is_some() { "" } else { "; ? a thread whose CPU strobe could not read" }
    );
    Ok(Output::new(
        text,
        json!({ "bin_s": bin_s, "span_s": span_s, "cpu_refused": t.cpu_refused, "marks": marks.iter().map(|(n, a, b)| json!({ "name": n, "from_s": s(*a), "to_s": s(*b) })).collect::<Vec<_>>(), "threads": rows }),
    ))
}

/// A native recording's frames: per measured run, what reached the screen (OS present events), the
/// inputs strobe gave the app, and each input's latency to the frame that showed it (SPEC §7.4).
fn native_frames(dir: &Path, top: usize, only: Option<&[String]>) -> Result<Output> {
    let f = inputs::frame_runs(dir)?;
    let presents = inputs::presents(dir)?;
    let pids: Vec<u32> = f["pids"].as_array().into_iter().flatten().filter_map(|p| p.as_u64().map(|p| p as u32)).collect();
    let refresh = inputs::refresh_ms(&f, &presents);
    let quiet = crate::probe::present::frames::quiet_ns(refresh.unwrap_or(1000.0 / 60.0));
    let mut text = String::new();
    let mut runs_json = Vec::new();
    let mut slow: Vec<(f64, String, &'static str)> = Vec::new();
    // Frames on screen by process over the runs: which of the app's processes drew them.
    let mut by_pid: BTreeMap<u32, usize> = BTreeMap::new();
    for (run, r) in f["runs"].as_object().into_iter().flatten().filter(|(k, _)| only.is_none_or(|o| o.contains(k))) {
        let (timed, sent, changes) = inputs::run_of(r);
        for (t0, t1) in timed.segments() {
            for p in presents.presents.iter().filter(|p| p.state == "displayed" && pids.contains(&p.pid) && p.start_ns >= t0 && p.start_ns < t1) {
                *by_pid.entry(p.pid).or_default() += 1;
            }
        }
        let shown = crate::probe::present::frames::from_timed(&presents.presents, &pids, &timed);
        let mut screens: Vec<(usize, u64)> = shown.shown.iter().map(|s| (s.seg, s.screen_ns)).collect();
        screens.sort_unstable();
        let mut gaps: Vec<f64> = screens.windows(2).filter(|w| w[0].0 == w[1].0).map(|w| (w[1].1 - w[0].1) as f64 / 1e6).collect();
        gaps.sort_by(f64::total_cmp);
        let all = r["inputs"].as_array().cloned().unwrap_or_default();
        let kind = |k: &str| all.iter().filter(|i| i["kind"] == k).count();
        let got = crate::probe::present::frames::reactions(&sent, &changes, &presents.presents, &pids, quiet);
        let pressed = |e: crate::probe::present::frames::Edge| if e == crate::probe::present::frames::Edge::Press { "press" } else { "input" };
        slow.extend(got.interaction.iter().map(|(ms, e)| (*ms, run.clone(), pressed(*e))));
        slow.extend(got.scroll.iter().map(|ms| (*ms, run.clone(), "scroll")));
        let untimed_ms = timed.untimed_in(timed.t0, timed.t1) as f64 / 1e6;
        text += &format!(
            "{run:<8} {:>5} frames on screen · {} discarded · interval p50 {} · p95 {} · inputs {} ({} pointer, {} action) · latency p50 {} ({} at a press){}{}\n",
            shown.shown.len(),
            shown.discarded,
            fmt::value(metrics::percentile(&gaps, 0.5), "ms"),
            fmt::value(metrics::percentile(&gaps, 0.95), "ms"),
            kind("discrete") + kind("scroll"),
            kind("pointer"),
            kind("pattern"),
            fmt::value(
                {
                    let mut v: Vec<f64> = got.interaction.iter().map(|x| x.0).collect();
                    v.sort_by(f64::total_cmp);
                    metrics::percentile(&v, 0.5)
                },
                "ms"
            ),
            got.presses(),
            if got.unchanged + got.busy.0 + got.unshown.0 > 0 {
                format!(" · not counted: {} changed no tree, {} came mid-drawing or before an earlier input showed, {} showed no frame", got.unchanged, got.busy.0, got.unshown.0)
            } else {
                String::new()
            },
            if timed.untimed.is_empty() { String::new() } else { format!(" · {} untimed left out ({} stretches)", fmt::value(untimed_ms, "ms"), timed.untimed.len()) },
        );
        runs_json.push(json!({
            "run": run, "frames": shown.shown.len(), "discarded": shown.discarded, "interval_p95": metrics::percentile(&gaps, 0.95),
            "latencies_ms": got.interaction.iter().map(|x| x.0).collect::<Vec<_>>(), "edges": got.interaction.iter().map(|x| x.1).collect::<Vec<_>>(),
            "scroll_latencies_ms": got.scroll, "unchanged": got.unchanged, "busy": got.busy, "unshown": got.unshown, "untimed_ms": untimed_ms,
        }));
    }
    slow.sort_by(|a, b| b.0.total_cmp(&a.0));
    if by_pid.len() > 1 {
        let mut rows: Vec<(&u32, &usize)> = by_pid.iter().collect();
        rows.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        text += &format!("by process  {}   (frames on screen over the runs above)\n", rows.iter().map(|(pid, n)| format!("pid {pid}: {n}")).collect::<Vec<_>>().join(" · "));
    }
    text += "slowest inputs (posted to the screen time of the frame that showed them; press: a click the app acted on at its press, timed from it)\n";
    for (ms, run, k) in slow.iter().take(top.min(10)) {
        text += &format!("  {:>13}  {run} {k}\n", fmt::value(*ms, "ms"));
    }
    text += &format!(
        "an input counts when the app changed its accessibility tree after it and was not drawing already or still to show an earlier input (SPEC §7.4); each input, and why one gave no sample: strobe query {} inputs\n",
        handle_of(dir)
    );
    let processes: Vec<Value> = by_pid.iter().map(|(pid, n)| json!({ "pid": pid, "frames": n })).collect();
    Ok(Output::new(text, json!({ "runs": runs_json, "refresh_ms": refresh, "processes": processes })))
}
/// Busy time by thread from its task loop's tasks. A thread's other tracks hold spans that overlap
/// its tasks (animation frames, GC sweeping): counting every top-level slice once read 70 s of
/// "busy" time for one renderer thread in a 14 s trace. Tasks on several tracks of one thread can
/// still overlap (one thread read 33.8 s of tasks in a shorter trace), so busy time is the union of
/// its tasks' spans, per thread (its tid), never more than the time they cover.
fn threads(trace: &Path, meta: &Value, top: usize, probe: bool) -> Result<Output> {
    // A lean trace holds no task loop's slices (`toplevel` is not in it): its threads' top-level slices
    // are the few its categories carry (a 50 ms click task read 0.2 ms, the renderer's main thread
    // missing), so it has no busy time by thread.
    if meta["probes"] == "lean" {
        return Err(Error::usage(
            "this recording's trace is lean (a timed block's default): it holds no task loop's slices, so it has no busy time by thread; its main-thread time is main.work (strobe query <id> summary), and a block with --trace full, or an attribution block (bench --why always), has threads",
        ));
    }
    if probe {
        return available();
    }
    let tasks = " and s.name in ('RunTask', 'ThreadControllerImpl::RunTask', 'ThreadPool_RunTask')";
    let (pre, runs, said) = in_measured_runs(meta);
    let sql = |filter: &str| {
        format!(
            "{pre}with x as (select tt.utid, s.ts, s.ts + s.dur as e, s.dur from slice s join thread_track tt on s.track_id = tt.id where s.depth = 0 and s.dur > 0{filter}{runs}),
             o as (select utid, ts, e, dur, max(e) over (partition by utid order by ts, e rows between unbounded preceding and 1 preceding) as before from x)
             select p.name as process, p.pid, t.tid, t.name as thread, count(*) as tasks, sum(max(0, e - max(ts, coalesce(before, ts)))) / 1e6 as busy_ms, max(dur) / 1e6 as max_ms
             from o join thread t using(utid) left join process p using(upid) group by o.utid order by busy_ms desc limit {top}"
        )
    };
    let mut out = sql_view(trace, &sql(tasks), top, true)?;
    if out.json["total"].as_u64().unwrap_or(0) == 0 {
        if meta["probes"] == "import" {
            // An imported trace without a task loop's slices: every top-level slice, said so.
            out = sql_view(trace, &sql(""), top, true)?;
            out.text += "no task-loop slices (RunTask) in this trace: every top-level slice of a thread counted, overlaps once\n";
        } else {
            out.text += "no task-loop slices (RunTask) in this trace's measured runs\n";
        }
    }
    out.text += &said;
    Ok(out)
}

/// What limits a whole-trace view to a recording's measured runs: the run windows' tables (`rec::windows_sql`)
/// to put before the query, the condition on a slice `s` (started in a measured run, outside its untimed
/// stretches) and the line that says so. A trace without strobe's run marks (an imported one) is read
/// whole.
fn in_measured_runs(meta: &Value) -> (String, &'static str, String) {
    let Some(token) = meta["token"].as_str().filter(|t| !t.is_empty()) else { return (String::new(), "", "over the whole trace\n".into()) };
    let keys = rec::measured_keys(meta);
    let said = match &keys {
        Some(k) => format!("the {} measured run{} (warmup runs and anything outside the runs left out)\n", k.len(), if k.len() == 1 { "" } else { "s" }),
        None => "the recording's runs\n".to_string(),
    };
    (
        format!("{}\n", rec::windows_sql(token, keys.as_deref())),
        " and exists (select 1 from _w w where s.ts >= w.t0 and s.ts < w.t1) and not exists (select 1 from _u u where s.ts >= u.t0 and s.ts < u.t1)",
        format!("over {said}"),
    )
}

/// A Firefox attribution recording's threads by CPU in its measured runs, every process's, from the
/// CPU each profile sample carries (`threadCPUDelta`).
fn gecko_threads(rec: &Rec, top: usize, probe: bool) -> Result<Output> {
    let g = store::read_json(&rec.dir.join(crate::probe::gecko::READ))?;
    let rows = g["thread_cpu"].as_array().cloned().unwrap_or_default();
    if rows.is_empty() {
        return Err(Error::usage("this Firefox profile carries no CPU per thread (its samples give CPU in cycles, not time)"));
    }
    if probe {
        return available();
    }
    let runs = rec.meta["runs"].as_f64().unwrap_or(1.0).max(1.0);
    // A sample carries its thread's CPU since the one before: the first sample in a run brings up to one
    // gap of CPU from before it, and the CPU after its last sample goes to no run.
    let edges = rec.sampling().map(|s| format!(" · {}: each run's CPU is exact to one gap at its edges", s.text())).unwrap_or_default();
    let mut text = format!(
        "threads by CPU per run · the threads Firefox's profiler sampled (main, compositor, renderer, DOM workers) · {runs:.0} measured run{}{edges}\n  {:>13}  {:<28}  thread\n",
        if runs == 1.0 { "" } else { "s" },
        "cpu/run",
        "process"
    );
    let mut out = Vec::new();
    for r in rows.iter().take(top) {
        let ms = r["cpu_ms"].as_f64().unwrap_or(0.0) / runs;
        let process = format!("{}{} {}", r["process"].as_str().unwrap_or(""), if r["page"] == true { " (page)" } else { "" }, r["pid"]);
        text += &format!("  {:>13}  {:<28}  {} {}\n", fmt::value(ms, "ms"), process, r["thread"].as_str().unwrap_or(""), r["tid"]);
        let mut row = r.clone();
        row["cpu_ms_per_run"] = json!(ms);
        out.push(row);
    }
    if rows.len() > top {
        text += &format!("  … {} more threads (--top)\n", rows.len() - top);
    }
    Ok(Output::new(text, json!({ "runs": runs, "threads": out, "sampling": rec.sampling() })))
}

/// Self time per function, file or package from the recording's CPU profiles (attribution only).
pub fn hotspots(dir: &Path, by: &str, top: usize) -> Result<Output> {
    hotspots_of(&Rec::open(dir), by, top)
}

/// What a sampler coarser than asked costs a profile (`probe::cpu::Sampling::coarse`).
pub const COARSE: &str = "coarser than asked: every time is its samples' own gaps, but a function that runs for less than one gap shows only when a sample lands in it (± per row: how sure)";

/// `hotspots` of an opened recording: its JavaScript profile, else its native samples.
pub fn hotspots_of(rec: &Rec, by: &str, top: usize) -> Result<Output> {
    hotspots_sorted(rec, by, top, Sort::Own)
}

/// The columns' order as the header says it.
fn columns(sort: Sort) -> &'static str {
    match sort {
        Sort::Own => "self time, then total time (with what it called)",
        Sort::Total => "self time, then total time (with what it called), sorted by total",
    }
}

/// `hotspots_of`, its rows in `sort`'s order.
pub fn hotspots_sorted(rec: &Rec, by: &str, top: usize, sort: Sort) -> Result<Output> {
    hotspots_scoped(rec, by, top, sort, Scope::Measured)
}

/// Which samples of a script recording's CPU profile a view reads (SPEC §8.3): its measured runs'
/// timed spans (`cpu-<run>.cpuprofile`), the whole host process (`--source host`), the host's startup
/// or the bench file's setup alone (`--source startup`, `--source setup`), or one phase's stretches
/// of the measured runs (`--phase`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scope<'a> {
    Measured,
    Host,
    Startup,
    Setup,
    Phase(&'a str),
}

impl<'a> Scope<'a> {
    /// What `--source` and `--phase` ask of a recording's JavaScript profile: a scope for a script
    /// recording, None for any other (whose `--phase` selects native samples).
    pub fn of(rec: &Rec, args: &'a Args) -> Result<Option<Scope<'a>>> {
        let (source, phase) = (args.get("source"), args.get("phase"));
        let script = rec.meta["script"] == true;
        let asked = match source {
            None => None,
            Some("host") => Some(Scope::Host),
            Some("startup") => Some(Scope::Startup),
            Some("setup") => Some(Scope::Setup),
            Some(other) => {
                return Err(Error::usage(format!(
                    "--source for hotspots: host (a script target's whole host process), startup or setup (the host's startup or the bench file's setup alone), not {other}"
                )));
            }
        };
        if let (Some(s), false) = (source, script) {
            return Err(Error::usage(format!("--source {s} reads a script target's host process; {} is no script recording", rec.id)));
        }
        if let Some(s) = source.filter(|_| args.has("native") || args.get("thread").is_some() || args.get("pid").is_some()) {
            return Err(Error::usage(format!("--source {s} reads the host's JavaScript profile; --native, --thread and --pid read native samples: one or the other")));
        }
        if !script || args.has("native") || args.get("thread").is_some() || args.get("pid").is_some() {
            return Ok(None);
        }
        Ok(Some(match (asked, phase) {
            (Some(_), Some(_)) => return Err(Error::usage("--source reads the host process or one part of its life; --phase reads a phase of the measured runs: one or the other")),
            (Some(s), None) => s,
            (None, Some(p)) => Scope::Phase(p),
            (None, None) => Scope::Measured,
        }))
    }

    /// The part of the host's life a scope reads alone (its `lifecycle_spans` name), if one.
    fn life(self) -> Option<&'static str> {
        match self {
            Scope::Startup => Some("startup"),
            Scope::Setup => Some("setup"),
            Scope::Measured | Scope::Host | Scope::Phase(_) => None,
        }
    }

    /// What a scope that no run count divides reads, for a..b's header (`each side's …`): the whole
    /// host process, or one part of its life; None for the measured runs or a phase of them.
    fn per_recording(self) -> Option<&'static str> {
        match self {
            Scope::Host => Some("whole host process (startup, setup, warmup, runs, resets)"),
            Scope::Startup => Some("host's startup alone"),
            Scope::Setup => Some("bench file's setup alone"),
            Scope::Measured | Scope::Phase(_) => None,
        }
    }

    /// The scope as `--json` says it (`scope`) and the header's words for it. A script recording from
    /// before strobe cut its profile to the runs holds the whole host in its one profile.
    fn named(self, rec: &Rec) -> (String, String) {
        let wall = || life_spans(rec, self.life().unwrap_or("")).iter().map(|(a, b)| b.saturating_sub(*a) as f64 / 1e6).sum::<f64>();
        match self {
            Scope::Measured if !rec.dir.join(crate::probe::cpu::RUNS).is_file() => {
                ("whole_host".into(), "whole host process: recorded before strobe cut script profiles to the runs, so its startup, setup and warmup are in it".into())
            }
            Scope::Measured => ("measured_runs".into(), format!("{} measured runs, each its timed span alone (--source host: the whole host process)", rec.runs().unwrap_or(0))),
            Scope::Host => ("whole_host".into(), format!("{WHOLE_HOST_TEXT}: per recording, not per run")),
            Scope::Startup => (
                "startup".into(),
                format!(
                    "the host's startup, {} wall (its spawn until it said ready: the runtime's boot, strobe's host and the target's static imports): per recording, not per run",
                    fmt::value(wall(), "ms")
                ),
            ),
            Scope::Setup => (
                "setup".into(),
                format!(
                    "the bench file's setup, {} wall (the whole setup hook, which runs in strobe's runner; the samples are the host's: what the hook called in the target, imports it made among them): per recording, not per run",
                    fmt::value(wall(), "ms")
                ),
            ),
            Scope::Phase(p) => ("phase".into(), format!("phase {p} of the measured runs")),
        }
    }
}

/// How every view names a script recording's whole host profile.
pub const WHOLE_HOST_TEXT: &str = "Whole host process (startup, setup, warmup, runs, resets)";

/// A script recording's CPU stacks in `scope`; a phase's or a part of the host's life's are computed
/// for the question (`owned`).
pub(super) fn scoped_cpu<'r>(rec: &'r Rec, scope: Scope, owned: &'r mut Option<Stacks>) -> Result<&'r Stacks> {
    match scope {
        Scope::Measured => rec.cpu(),
        Scope::Host => rec.host_cpu(),
        Scope::Startup | Scope::Setup => {
            let name = scope.life().unwrap_or("");
            let spans = life_spans(rec, name);
            if spans.is_empty() {
                return Err(no_life_span(rec, name));
            }
            Ok(owned.insert(rec.phase_cpu(&spans, &[])?))
        }
        Scope::Phase(p) => {
            let (spans, untimed) = script_phase(rec, p)?;
            Ok(owned.insert(rec.phase_cpu(&spans, &untimed)?))
        }
    }
}

/// Stretches of time, ns on strobe's clock.
type Spans = Vec<(u64, u64)>;

/// A script recording's stretches of one part of its host's life (`meta.lifecycle_spans`): those named
/// `name`, or `name:<run>` (`warm:0`, `reset:3`).
fn life_spans(rec: &Rec, name: &str) -> Spans {
    rec.meta["lifecycle_spans"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|l| l["name"].as_str().is_some_and(|n| n.split(':').next() == Some(name)))
        .filter_map(|l| Some((l["t0_ns"].as_u64()?, l["t1_ns"].as_u64()?)))
        .filter(|(a, b)| b > a)
        .collect()
}

fn probe_cpu(rec: &Rec, scope: Scope) -> Result<Output> {
    match scope {
        Scope::Measured => {
            if !rec.dir.join("deep.json.gz").is_file() {
                rec.cpu_files()?;
            }
        }
        Scope::Phase(p) => {
            script_phase(rec, p)?;
            rec.host_file()?;
        }
        Scope::Startup | Scope::Setup => {
            let name = scope.life().unwrap_or("");
            if life_spans(rec, name).is_empty() {
                return Err(no_life_span(rec, name));
            }
            rec.host_file()?;
        }
        Scope::Host => {
            rec.host_file()?;
        }
    }
    available()
}

fn no_life_span(rec: &Rec, name: &str) -> Error {
    Error::usage(format!("--source {name}: {} keeps no {name} span (a script recording from before strobe kept its host's lifecycle_spans)", rec.id))
}

/// A phase's stretches in a script recording's measured runs and those runs' untimed stretches (ns on
/// strobe's clock), from `runs.json`.
fn script_phase(rec: &Rec, phase: &str) -> Result<(Spans, Spans)> {
    let f = store::read_json(&rec.dir.join(crate::probe::cpu::RUNS))
        .map_err(|_| Error::usage(format!("--phase: {} keeps no runs.json (a script recording from before strobe kept its runs' phase marks)", rec.id)))?;
    let measured = rec::measured_keys(&rec.meta);
    let (mut spans, mut untimed) = (Vec::new(), Vec::new());
    for r in f["runs"].as_array().into_iter().flatten().filter(|r| measured.as_ref().is_none_or(|m| m.iter().any(|k| r["run"] == k.as_str()))) {
        let marks: Vec<(String, u64)> = r["marks"].as_array().into_iter().flatten().filter_map(|m| Some((m[0].as_str()?.to_string(), m[1].as_u64()?))).collect();
        spans.extend(metrics::phase_stretches(&marks, phase));
        untimed.extend(r["untimed"].as_array().into_iter().flatten().filter_map(|u| Some((u[0].as_u64()?, u[1].as_u64()?))));
    }
    if spans.is_empty() {
        return Err(Error::usage(format!("no phase {phase:?} in {}'s measured runs", rec.id)));
    }
    Ok((spans, untimed))
}

/// `hotspots_sorted` of one scope of a script recording's profile (any other recording: `Measured`).
pub fn hotspots_scoped(rec: &Rec, by: &str, top: usize, sort: Sort, scope: Scope) -> Result<Output> {
    let mut owned = None;
    let stacks = match scoped_cpu(rec, scope, &mut owned) {
        Ok(s) => s,
        Err(_) if scope == Scope::Measured && rec.dir.join("deep.json.gz").is_file() => return native_hotspots_of(rec, by, top, Pick::default(), sort),
        Err(e) => return Err(e),
    };
    let named = (rec.meta["script"] == true).then(|| scope.named(rec));
    // The interval the sampler kept, not the one asked: Firefox 160 and older on Windows sample every
    // 2 ms when asked for 1 ms (`probe::gecko::interval_us`).
    let sampling = rec.sampling();
    let at = sampling.map(|s| format!(", {}", s.text())).unwrap_or_default();
    let what = named.as_ref().map(|n| format!(" · {}", n.1)).unwrap_or_default();
    let mut header = format!("hotspots by {by}{what} · {} · {{samples}} samples{at}{{mapped}}", columns(sort));
    // Each sample weighs the gap it kept, so the times hold; what a coarser sampler loses is the short.
    if sampling.is_some_and(|s| s.coarse()) {
        header += &format!("\n! {COARSE}");
    }
    // The measured runs' main-thread CPU the profile holds no sample of (`cpu.main` against the samples).
    let runs = matches!(scope, Scope::Measured | Scope::Phase(_));
    let missed = (runs && rec.meta["missed"].is_object()).then(|| &rec.meta["missed"]);
    for t in missed.map(|m| crate::probe::cpu::missed_texts(m, rec.meta["runtime"]["name"].as_str())).unwrap_or_default() {
        header += &format!("\n! {t}");
    }
    if stacks.weights.is_empty() && runs && named.is_some() {
        header += &format!("\n! no sample in these runs: the host ran no JavaScript there that its sampler saw; strobe query {} hotspots --source host has the whole host process", rec.handle());
    }
    let mut out = rank(stacks, by, top, &header, |us| fmt::value(us / 1000.0, "ms"), Some(sort));
    if let Some(m) = missed {
        out.json["missed"] = m.clone();
    }
    if named.is_some() && !runs {
        host_trailer(rec, scope, stacks, &mut out);
    }
    if named.is_some() && scope == Scope::Host {
        by_lifecycle(rec, &mut out);
    }
    out.json["interval_us"] = json!(sampling.map(|s| s.median_us));
    out.json["sampling"] = json!(sampling);
    if let Some((key, text)) = named {
        out.json["scope"] = json!(key);
        out.json["scope_text"] = json!(text);
    }
    ms_columns(&mut out.json);
    Ok(out)
}

/// Under this share of a part's wall time sampled, a host's part says what the rest is.
const UNSAMPLED: f64 = 0.5;

/// What a part's wall time without a JavaScript sample is, for the runtime the host ran on: never
/// work missing from the rows alone.
fn unsampled_note(rec: &Rec) -> String {
    let why = match rec.meta["runtime"]["name"].as_str() {
        Some("bun") => "; Bun's --cpu-prof samples JavaScript only, so its native work (transpiling TypeScript, resolving and loading modules) is in it too",
        Some("node") => "; Node samples its native work as (program), and its (idle) samples are left out of this count",
        _ => "",
    };
    format!("i wall time with no JavaScript sample is mostly not work missing from the rows: the host waiting (idle, I/O, timers) and the runner's own steps between its calls{why}")
}

/// The JavaScript a script profile's stacks hold (µs): every weight but the profile's own `(root)`
/// and `(idle)`, a thread that waited.
fn sampled_js(stacks: &Stacks) -> f64 {
    stacks.weights.iter().filter(|w| w.stack.last().is_some_and(|&i| !matches!(stacks.frames[i as usize].name.as_str(), "(root)" | "(idle)"))).map(|w| w.w).sum()
}

/// What a script host's profile outside its measured runs says beside its rows: for its startup or the
/// bench file's setup, the JavaScript sampled against the wall time (on Bun, why the rest has no row);
/// and the modules it evaluated, each its own root stack (`(module)`), which rank low one by one.
fn host_trailer(rec: &Rec, scope: Scope, stacks: &Stacks, out: &mut Output) {
    if let Some(name) = scope.life() {
        let wall = life_spans(rec, name).iter().map(|(a, b)| b.saturating_sub(*a) as f64 / 1e6).sum::<f64>();
        let sampled = sampled_js(stacks) / 1e3;
        out.text +=
            &format!("{name}: {} of JavaScript sampled in {} of wall time ({:.0}% of wall time, not CPU)\n", fmt::value(sampled, "ms"), fmt::value(wall, "ms"), 100.0 * sampled / wall.max(1e-9));
        if sampled < UNSAMPLED * wall {
            out.text += &unsampled_note(rec);
            out.text += "\n";
        }
        out.json["wall_ms"] = json!(wall);
        out.json["sampled_ms"] = json!(sampled);
    }
    // Each module's evaluation as one row: its file, its weight with what it called.
    let mut modules: BTreeMap<&str, f64> = BTreeMap::new();
    for w in &stacks.weights {
        if let Some(f) = w.stack.first().map(|&i| &stacks.frames[i as usize]).filter(|f| f.name == "(module)") {
            *modules.entry(f.file.as_str()).or_default() += w.w;
        }
    }
    if modules.len() < 2 {
        return;
    }
    let all: f64 = modules.values().sum();
    let (file, most) = modules.iter().max_by(|a, b| a.1.total_cmp(b.1)).map(|(f, w)| (*f, *w)).unwrap_or_default();
    out.text += &format!(
        "{} modules evaluated, each its own stack ((module) at its root): {} in all, the most {} ({file}); --by file or --by package sums them\n",
        modules.len(),
        fmt::value(all / 1e3, "ms"),
        fmt::value(most / 1e3, "ms")
    );
    out.json["modules"] = json!({ "count": modules.len(), "ms": all / 1e3, "max_ms": most / 1e3, "max_file": file });
}

/// `--source host`: the host's life by part (its startup, the bench file's setup, warmup runs, the
/// measured runs' timed stretches and their untimed ones, resets, teardown, and the time between
/// them), each its wall time on strobe's clock against the JavaScript the whole host profile sampled
/// in it. The measured runs' row is what `hotspots` reads by default (and `meta.missed` counts).
/// Nothing for a recording from before strobe kept `lifecycle_spans`.
fn by_lifecycle(rec: &Rec, out: &mut Output) {
    const ORDER: [(&str, &str); 8] = [
        ("startup", "startup"),
        ("setup", "setup"),
        ("warm", "warmup runs"),
        ("measured", "measured runs"),
        ("untimed", "untimed (in runs)"),
        ("unmeasured", "unmeasured runs"),
        ("reset", "resets"),
        ("teardown", "teardown"),
    ];
    let mut names: Vec<String> = ORDER.iter().map(|(n, _)| n.to_string()).collect();
    for l in rec.meta["lifecycle_spans"].as_array().into_iter().flatten() {
        let n = l["name"].as_str().unwrap_or("").split(':').next().unwrap_or("");
        if !n.is_empty() && !names.iter().any(|m| m == n) {
            names.push(n.to_string());
        }
    }
    // The measured runs as their own profiles cut them: timed stretches, `ctx.untimed` apart.
    let runs: Vec<metrics::Timed> = store::read_json(&rec.dir.join(crate::probe::cpu::RUNS))
        .ok()
        .and_then(|f| f["runs"].as_array().cloned())
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let (t0, t1) = (r["t0_ns"].as_u64()?, r["t1_ns"].as_u64()?);
            let untimed = r["untimed"].as_array().into_iter().flatten().filter_map(|u| Some((u[0].as_u64()?.max(t0), u[1].as_u64()?.min(t1)))).filter(|(a, b)| b > a).collect();
            (t1 > t0).then_some(metrics::Timed { t0, t1, untimed })
        })
        .collect();
    let parts: Vec<(String, Spans)> = names
        .into_iter()
        .map(|n| {
            let spans = match n.as_str() {
                "measured" => runs.iter().flat_map(metrics::Timed::segments).collect(),
                "untimed" => runs.iter().flat_map(|r| r.untimed.iter().copied()).collect(),
                _ => life_spans(rec, &n),
            };
            (n, spans)
        })
        .filter(|(_, s)| !s.is_empty())
        .collect();
    if !parts.iter().any(|(n, _)| n != "measured" && n != "untimed") {
        return;
    }
    let groups: Vec<Spans> = parts.iter().map(|(_, s)| s.clone()).collect();
    let Ok((sampled, all)) = rec.host_sampled(&groups) else { return };
    let wall_of = |s: &Spans| s.iter().map(|(a, b)| (b - a) as f64 / 1e6).sum::<f64>();
    let label = |n: &str| ORDER.iter().find(|(k, _)| *k == n).map_or(n.to_string(), |(_, l)| l.to_string());
    out.text += "by lifecycle · each part's wall time on strobe's clock and the JavaScript sampled in it, as a share of wall time, not CPU (--source startup or setup ranks one part's rows; --phase a phase of the measured runs):\n";
    let mut rows = Vec::new();
    let mut short = false;
    let line = |l: &str, wall: Option<f64>, ms: f64| {
        let w = wall.map(|w| format!("{} wall", fmt::value(w, "ms"))).unwrap_or_default();
        let pct = wall.filter(|w| *w > 0.0).map(|w| format!("{:>4.0}%", 100.0 * ms / w)).unwrap_or_default();
        format!("  {l:<17} {w:>19}  {:>13} sampled  {pct}\n", fmt::value(ms, "ms"))
    };
    for ((n, spans), us) in parts.iter().zip(&sampled) {
        let (wall, ms) = (wall_of(spans), us / 1e3);
        short |= wall > 10.0 && ms < UNSAMPLED * wall;
        out.text += &line(&label(n), Some(wall), ms);
        rows.push(json!({ "part": n, "label": label(n), "spans": spans.len(), "wall_ms": wall, "sampled_ms": ms }));
    }
    // The rest: strobe's clock-sync spins and its calls between the parts and runs, and the exit.
    let rest = (all - sampled.iter().sum::<f64>()).max(0.0) / 1e3;
    out.text += &format!("{}  (between the parts: strobe's clock-sync spins, its calls between runs; the exit)\n", line("other", None, rest).trim_end());
    rows.push(json!({ "part": "other", "label": "between the parts and the exit", "sampled_ms": rest }));
    if short {
        out.text += &unsampled_note(rec);
        out.text += "\n";
    }
    out.json["lifecycle"] = json!(rows);
}

/// `total_ms` beside `total`, `self_ms` and `total_ms` beside each row's and caller's µs.
fn ms_columns(j: &mut Value) {
    j["total_ms"] = json!(j["total"].as_f64().unwrap_or(0.0) / 1000.0);
    for key in ["rows", "callers"] {
        for r in j[key].as_array_mut().into_iter().flatten() {
            r["self_ms"] = json!(r["weight"].as_f64().unwrap_or(0.0) / 1000.0);
            r["total_ms"] = json!(r["inclusive"].as_f64().unwrap_or(0.0) / 1000.0);
        }
    }
}

/// Which native samples `hotspots` reads: one thread's (`--thread`), one process's (`--pid`, its id
/// or its executable's name), one phase's (`--phase`).
#[derive(Clone, Copy, Default)]
pub struct Pick<'a> {
    pub thread: Option<&'a str>,
    pub pid: Option<&'a str>,
    pub phase: Option<&'a str>,
}

impl<'a> Pick<'a> {
    pub fn of(args: &'a Args) -> Pick<'a> {
        Pick { thread: args.get("thread"), pid: args.get("pid"), phase: args.get("phase") }
    }

    fn any(&self) -> bool {
        self.thread.is_some() || self.pid.is_some() || self.phase.is_some()
    }
}

/// Self time per function, file or package from the recording's native samples (the kernel's
/// sampled profile, symbolized from PDBs), in the shape of `hotspots`. `thread`: a thread name or id.
/// `phase`: only samples inside that phase of each measured run (`phase(name, fn)` in the bench file).
pub fn native_hotspots(dir: &Path, by: &str, top: usize, thread: Option<&str>, phase: Option<&str>) -> Result<Output> {
    native_hotspots_of(&Rec::open(dir), by, top, Pick { thread, phase, pid: None }, Sort::Own)
}

fn no_native_samples(rec: &Rec) -> Error {
    if rec.meta["kind"] == "block" && rec.meta["probes"] != "attribution" {
        no_profile(rec, "native samples", "hotspots")
    } else {
        Error::usage("this recording has no native samples; record with --probes deep")
    }
}

pub fn native_hotspots_of(rec: &Rec, by: &str, top: usize, pick: Pick, sort: Sort) -> Result<Output> {
    let dir = rec.dir.as_path();
    let missing = || no_native_samples(rec);
    // A thread, process or phase selects samples: symbolized for that question alone; all samples are shared.
    let filtered;
    let n: &NativeStacks = if pick.any() {
        filtered = native_filtered(dir, pick)?.ok_or_else(missing)?;
        &filtered
    } else {
        rec.native()?.ok_or_else(missing)?
    };
    let share = if n.samples > 0 { 100.0 * n.resolved as f64 / n.samples as f64 } else { 0.0 };
    let facts = &n.facts;
    // A leaf in a named system library resolves while its callers in an unsymbolized binary do not:
    // both shares, so a high leaf share never reads as named stacks.
    let frame_share = 100.0 * facts["resolved_frame_share"].as_f64().unwrap_or(0.0);
    let processes = facts["processes"].as_array().cloned().unwrap_or_default();
    let of_procs = if processes.len() > 1 { format!(" of {} processes", processes.len()) } else { String::new() };
    // Samples that carry no time (macOS `sample`'s snapshots of every thread, running or not) are
    // counts, never CPU: their rows say samples, not samples × interval as ms.
    let (header, interval) = if n.untimed {
        (
            format!(
                "native hotspots by {by} · {} · {{samples}} samples{of_procs}, untimed (snapshots of every thread, not CPU time: rows count samples) · resolved: {share:.1}% of leaf frames, {frame_share:.1}% of all frames{{mapped}}",
                columns(sort)
            ),
            n.interval_us.max(1.0),
        )
    } else {
        (
            format!(
                "native hotspots by {by} · {} · {{samples}} samples{of_procs} at {} (sampled: samples × interval) · resolved: {share:.1}% of leaf frames, {frame_share:.1}% of all frames{{mapped}}",
                columns(sort),
                fmt::time(n.interval_us / 1e3)
            ),
            n.interval_us,
        )
    };
    let untimed = n.untimed;
    let show = move |us: f64| if untimed { format!("{:.0} smp", us / interval) } else { fmt::value(us / 1000.0, "ms") };
    let mut out = rank(&n.stacks, by, top, &header, show, Some(sort));
    out.json["native"] = json!(true);
    out.json["untimed"] = json!(n.untimed);
    out.json["interval_us"] = json!(n.interval_us);
    out.json["resolved_samples"] = facts["resolved_samples"].clone();
    out.json["resolved_share"] = facts["resolved_share"].clone();
    out.json["resolved_frame_share"] = facts["resolved_frame_share"].clone();
    out.json["unsymbolized"] = facts["unsymbolized"].clone();
    out.json["unsymbolized_any_frame"] = facts["unsymbolized_any_frame"].clone();
    out.json["processes"] = json!(processes);
    if !n.untimed {
        ms_columns(&mut out.json);
    }
    // The rows above pool every sampled process: by process, so a tree's children are not read as its root's.
    if processes.len() > 1 {
        let shown = processes.len().min(top.max(8));
        out.text += &format!(
            "by process (the rows above pool them; --pid <pid|exe> reads one): samples{}, share of the {} samples\n",
            if n.untimed { "" } else { ", sampled time (samples × interval)" },
            n.samples
        );
        for p in processes.iter().take(shown) {
            let k = p["samples"].as_f64().unwrap_or(0.0);
            let ms = if n.untimed { String::new() } else { format!("{:>13}  ", fmt::value(p["sampled_ms"].as_f64().unwrap_or(0.0), "ms")) };
            out.text += &format!("  {k:>7.0}  {ms}{:>5.1}%  {:>7}  {}\n", 100.0 * k / (n.samples.max(1) as f64), p["pid"], p["exe"].as_str().unwrap_or("?"));
        }
        if processes.len() > shown {
            out.text += &format!("  … {} more processes (--json lists every one)\n", processes.len() - shown);
        }
    }
    let deep: Option<crate::probe::stacks::Deep> = store::gunzip(&dir.join("deep.json.gz")).ok().and_then(|f| serde_json::from_reader(f).ok());
    // What share of the processes' CPU the samples stand for: the tree's own cycles around the capture
    // where strobe read them (`record pid:`), else what strobe's own sampler saw its threads use. Both
    // span the whole capture, so the samples counted are all the capture took (its own count: a block
    // keeps the samples of its measured runs only). Only for a timed capture read whole.
    if !n.untimed && !pick.any() {
        let stats = deep.as_ref().and_then(|d| d.suspend.as_ref());
        let set = rec.meta["set_cpu_ms"].as_f64().filter(|c| *c > 0.0);
        let seen = stats.and_then(|s| s.cpu_ms).filter(|c| *c > 0.0);
        if let Some(cpu) = set.or(seen) {
            let source = if set.is_some() { "the processes' cycles around the capture" } else { "the CPU the sampler saw the threads use" };
            let taken = stats.map(|s| s.samples).or_else(|| deep.as_ref().map(|d| d.samples.len() as u64)).unwrap_or(n.samples);
            let every_ms = n.interval_us / 1e3;
            let sampled = taken as f64 * every_ms;
            let share = sampled / cpu;
            // Where the rest went, as far as the sampler counted it. Threads it never opened only show as
            // the set's own cycles above what it saw.
            let capped_ms = stats.map_or(0.0, |s| s.capped as f64 * every_ms);
            let forgone_ms = stats.map_or(0.0, |s| s.forgone_ms);
            let late_ms = stats.map_or(0.0, |s| s.late_ms);
            let unseen_ms = set.zip(seen).map(|(s, v)| s - v).filter(|d| *d > 1.0);
            let unopened = stats.map_or(0, |s| s.unopened);
            let lost = deep.as_ref().map_or(0, |d| d.events_lost);
            out.json["cpu_coverage"] = json!({
                "share": share, "sampled_ms": sampled, "cpu_ms": cpu, "source": source, "samples": taken, "samples_asked": cpu / every_ms,
                "capped_ms": capped_ms, "forgone_ms": forgone_ms, "late_ms": late_ms, "unseen_ms": unseen_ms, "unopened_threads": unopened, "events_lost": lost,
            });
            if share < 0.8 {
                let mut why: Vec<String> = Vec::new();
                if late_ms > 0.5 {
                    why.push(format!("{} ran in threads before strobe opened them", fmt::value(late_ms, "ms")));
                }
                if let Some(u) = unseen_ms {
                    why.push(format!("{} ran outside the threads strobe followed (begun and ended between its looks, or around the capture's edges)", fmt::value(u, "ms")));
                }
                if forgone_ms > 0.5 {
                    why.push(format!("{} ran past two intervals between looks at a thread (waking between set refreshes, or a late tick)", fmt::value(forgone_ms, "ms")));
                }
                if capped_ms > 0.0 {
                    why.push(format!("{} went over its rate cap", fmt::value(capped_ms, "ms")));
                }
                if unopened > 0 {
                    why.push(format!("{unopened} threads could not be opened"));
                }
                if lost > 0 {
                    why.push(format!("the OS lost {lost} events"));
                }
                out.text += &format!(
                    "! Coverage: {taken} samples of the {:.0} the processes' {} of CPU asks for at one per {} ({:.0}%; {source}){}\n",
                    cpu / every_ms,
                    fmt::value(cpu, "ms"),
                    fmt::value(every_ms, "ms"),
                    100.0 * share,
                    if why.is_empty() { "; the sampler counted no cause for the rest".to_string() } else { format!("; {}", why.join("; ")) }
                );
            }
        }
    }
    if let (0, true, Some(d)) = (n.samples, pick.thread.is_some() || pick.pid.is_some(), deep.as_ref())
        && !d.samples.is_empty()
    {
        // The thread or process matched none of the sampled ones: name those, busiest first.
        let busiest = |processes: bool, name: &dyn Fn(u32) -> String| {
            let mut by: BTreeMap<u32, u64> = BTreeMap::new();
            for s in &d.samples {
                *by.entry(if processes { s.1 } else { s.2 }).or_default() += 1;
            }
            let mut list: Vec<(u32, u64)> = by.into_iter().collect();
            list.sort_by_key(|a| std::cmp::Reverse(a.1));
            list.iter().take(8).map(|(id, k)| format!("{} ({k})", name(*id))).collect::<Vec<_>>().join(", ")
        };
        if let Some(t) = pick.thread {
            let names = busiest(false, &|tid| d.threads.get(&tid).cloned().unwrap_or_else(|| format!("thread {tid}")));
            out.text += &format!("! no samples of a thread named or numbered {t:?}{}; the sampled threads (samples): {names}\n", if pick.pid.is_some() { " in that process" } else { "" });
        }
        if let Some(p) = pick.pid {
            let names = busiest(true, &|pid| format!("{} {pid}", crate::symbol::process_name(d, pid)));
            out.text += &format!("! no samples of a process numbered or named {p:?}; the sampled processes (samples): {names}\n");
        }
    } else if n.samples == 0 {
        // Nothing sampled is not the same as nothing resolved: say what the sampler gave.
        let why = if cfg!(target_os = "macos") {
            "/usr/bin/sample gave no stacks for these runs (the app exited or was not sampled in the measured runs' span; macOS samples carry no times, so a span with no sample stays empty)"
        } else if cfg!(target_os = "linux") {
            "perf events gave no samples for these runs (perf_event_paranoid, or the threads never ran in the measured span)"
        } else {
            "the kernel's sampled profile held no samples of the app in the measured runs (the deep helper, or the threads never ran)"
        };
        out.text += &format!("! no native samples: {why}\n");
    } else if share < 95.0 || frame_share < 95.0 {
        let list =
            |k: &str| -> String { facts[k].as_array().into_iter().flatten().take(3).map(|m| format!("{} ({})", m["module"].as_str().unwrap_or(""), m["samples"])).collect::<Vec<_>>().join(", ") };
        let what = if cfg!(windows) { "no PDB found" } else { "no symbols or DWARF found (a dSYM or an unstripped binary)" };
        if share < 95.0 {
            out.text += &format!("! Unresolved: {:.1}% of samples have no symbol at the leaf ({what}): {}\n", 100.0 - share, list("unsymbolized"));
        }
        if frame_share < 95.0 {
            out.text += &format!("! Unresolved: {:.1}% of all frames have no symbol ({what}); samples with one in: {}\n", 100.0 - frame_share, list("unsymbolized_any_frame"));
        }
    }
    // Sampled by strobe itself without the helper: how, and what it cost the threads.
    // Processes of the target the sampler gave no stacks of: their CPU is in no row above.
    if let Some(d) = deep.as_ref().filter(|d| !d.unsampled.is_empty()) {
        let list: Vec<&str> = d.unsampled.iter().map(|(_, w)| w.as_str()).collect();
        out.text += &format!("! Unsampled: no stacks of {} of the target's processes, so their CPU is missing from these rows: {}\n", list.len(), list.join("; "));
        out.json["unsampled"] = json!(d.unsampled);
    }
    if let Some(d) = deep.filter(|d| d.suspend.is_some()) {
        out.text += &format!("i native stacks{}\n", crate::probe::stacks::stacks_how(&d));
        out.json["suspend"] = json!(d.suspend);
    }
    Ok(out)
}

/// One thread's, one process's or one phase's native samples (symbolized for that question alone).
fn native_filtered(dir: &Path, pick: Pick) -> Result<Option<NativeStacks>> {
    let spans = pick.phase.map(|p| phase_spans(dir, p)).transpose()?;
    NativeStacks::read(dir, &crate::symbol::Select { thread: pick.thread, process: pick.pid, spans: spans.as_deref() })
}

/// The host-clock spans of a phase in a native recording's measured runs, from its marks.
fn phase_spans(dir: &Path, phase: &str) -> Result<Vec<(u64, u64)>> {
    let f = store::read_json(&dir.join("frames.json")).map_err(|_| Error::usage("--phase needs a native block's recording (its frames.json holds the phase marks)"))?;
    let mut out = Vec::new();
    for r in f["runs"].as_object().into_iter().flatten().map(|(_, r)| r) {
        let marks: Vec<(String, u64)> = r["marks"].as_array().into_iter().flatten().filter_map(|m| Some((m[0].as_str()?.to_string(), m[1].as_u64()?))).collect();
        // Every time the run entered it, as `phase.<name>` counts them.
        out.extend(metrics::phase_stretches(&marks, phase));
    }
    if out.is_empty() {
        return Err(Error::usage(format!("no phase {phase:?} in this recording's runs")));
    }
    Ok(out)
}

/// Bytes allocated per function, file or package from the recording's sampling heap profiles
/// (attribution only): who allocates, with full stacks, including objects already collected.
pub fn alloc(rec: &Rec, by: &str, top: usize) -> Result<Output> {
    let header = format!("allocations by {by} · sampled every {} KB on average · {{samples}} samples{{mapped}}", crate::probe::alloc::INTERVAL_BYTES / 1024);
    let mut out = rank(rec.alloc()?, by, top, &header, fmt::bytes, None);
    out.json["interval_bytes"] = json!(crate::probe::alloc::INTERVAL_BYTES);
    Ok(out)
}

/// A recording's forced layouts and style recalculations in its measured runs: the stacks of the
/// calls that forced them (µs each), and [forced, in the frame lifecycle] × [layout, style] counts
/// and ms.
pub struct Forced {
    pub stacks: Stacks,
    totals: [[(u64, f64); 2]; 2],
    /// [layout, style] that strobe's driver forced (a wait's check reading a box), apart from `totals`.
    driver: [(u64, f64); 2],
    pub runs: usize,
    runs_text: String,
    /// Call sites located at their function's start: Firefox gave no running line for them.
    at_start: u64,
}

fn forced(rec: &Rec) -> Result<&Forced> {
    cached(rec.forced.get_or_init(|| {
        attribution_meta(rec, "layout")?;
        let rows = rec.part(Part::Layout)?;
        let measured = measured_runs(&rec.meta, &rows);
        let sym = rec.sym.lock().unwrap_or_else(|e| e.into_inner());
        let mut sites: BTreeMap<TraceFrame, u32> = BTreeMap::new();
        let mut stacks = Stacks::default();
        let mut totals = [[(0u64, 0.0f64); 2]; 2];
        let mut driver = [(0u64, 0.0f64); 2];
        let mut outer_end: BTreeMap<String, i64> = BTreeMap::new();
        let mut at_start = 0u64;
        for row in rows.iter().filter(|r| r.get(1).is_some_and(Option::is_some)) {
            let cell = |i: usize| row.get(i).cloned().flatten().unwrap_or_default();
            if !measured.contains(&cell(0)) {
                continue;
            }
            let (ts, dur) = (cell(2).parse::<i64>().unwrap_or(0), cell(3).parse::<i64>().unwrap_or(0));
            // Count only outermost events: nested ones are already inside their parent's time.
            let end = outer_end.entry(cell(4)).or_insert(i64::MIN);
            if ts < *end {
                continue;
            }
            *end = ts + dur;
            let stack = trace_stack(&cell(5));
            at_start += stack.first().is_some_and(|f| f.4 > 0 && (f.1, f.2) == (f.4, f.5)) as u64;
            let kind = usize::from(cell(1) == "UpdateLayoutTree");
            // strobe's driver forced it (a wait's check read a box): never the app's.
            let by_driver = stack.iter().find(|f| !f.0.is_empty()).is_some_and(|f| f.0 == crate::driver::SOURCE_URL);
            let lifecycle = usize::from(stack.is_empty());
            let into = if by_driver { &mut driver[kind] } else { &mut totals[lifecycle][kind] };
            into.0 += 1;
            into.1 += dur as f64 / 1e6;
            if stack.is_empty() {
                continue;
            }
            // Trace stacks are innermost first with 1-based lines and columns; `rank` wants root first.
            let ids: Vec<u32> = stack
                .into_iter()
                .rev()
                .map(|f| match sites.get(&f) {
                    Some(&i) => i,
                    None => {
                        let i = stacks.intern(site(&sym, &f));
                        sites.insert(f, i);
                        i
                    }
                })
                .collect();
            // Traced, not sampled: each forced layout is counted, with no sampling variance.
            stacks.weights.push(Weight { stack: ids, w: dur as f64 / 1e3, n: 1, var: 0.0 });
        }
        Ok(Forced { stacks, totals, driver, runs: measured.len(), runs_text: runs_text(&rec.meta, measured.len(), &rows), at_start })
    }))
}

/// Forced (synchronous) layouts and style recalculations in the measured runs, by the JavaScript call
/// that forced them (attribution traces carry their stacks): reading geometry after a DOM write makes
/// the browser lay out mid-script instead of once per frame.
pub fn layout(rec: &Rec, top: usize) -> Result<Output> {
    let f = forced(rec)?;
    let runs = f.runs.max(1) as f64;
    let per_run = |(n, ms): (u64, f64)| format!("{:.0} ({}) per run", n as f64 / runs, fmt::value(ms / runs, "ms"));
    let header = format!(
        "forced layout and style by the call that forced them · {}{{mapped}}\n  forced     layout {} · style {}\n  in frames  layout {} · style {}",
        f.runs_text,
        per_run(f.totals[0][0]),
        per_run(f.totals[0][1]),
        per_run(f.totals[1][0]),
        per_run(f.totals[1][1]),
    );
    let mut out = rank(&f.stacks, "function", top, &header, |us| fmt::value(us / 1000.0, "ms"), None);
    let summary = |(n, ms): (u64, f64)| json!({ "count": n, "ms": ms, "count_per_run": n as f64 / runs, "ms_per_run": ms / runs });
    out.json["runs"] = json!(f.runs);
    out.json["forced"] = json!({ "layout": summary(f.totals[0][0]), "style": summary(f.totals[0][1]) });
    out.json["lifecycle"] = json!({ "layout": summary(f.totals[1][0]), "style": summary(f.totals[1][1]) });
    out.json["driver"] = json!({ "layout": summary(f.driver[0]), "style": summary(f.driver[1]) });
    if f.driver.iter().any(|d| d.0 > 0) {
        out.text += &format!("i strobe's driver forced layout {} · style {} (its waits' checks; not in the counts above, its rows say {DRIVER})\n", per_run(f.driver[0]), per_run(f.driver[1]));
    }
    for r in out.json["rows"].as_array_mut().into_iter().flatten() {
        r["ms"] = json!(r["weight"].as_f64().unwrap_or(0.0) / 1000.0);
    }
    if f.at_start > 0 {
        out.text += &format!(
            "i {} forced call sites are located at their function's start, not the call: Firefox gives no running line for the innermost JIT-compiled frame of a marker's stack\n",
            f.at_start
        );
        out.json["located_at_function_start"] = json!(f.at_start);
    }
    Ok(out)
}

/// A recording's style invalidation in its measured runs, as counts (`style` divides them per run).
pub struct Invalidation {
    pub runs: usize,
    runs_text: String,
    pub recalcs: u64,
    pub recalc_ms: f64,
    pub elements: f64,
    pub largest: f64,
    pub tracked: u64,
    /// What changed → (count, the nodes it hit).
    pub what: BTreeMap<String, (u64, BTreeMap<String, u64>)>,
    pub selectors: BTreeMap<String, u64>,
    /// The script line that made the change (its name resolved), by call site.
    pub sources: BTreeMap<Frame, u64>,
}

fn invalidation(rec: &Rec) -> Result<Invalidation> {
    firefox_gap(rec, Gap::Style)?;
    attribution_meta(rec, "style")?;
    let rows = rec.part(Part::Style)?;
    let measured = measured_runs(&rec.meta, &rows);
    let sym = rec.sym.lock().unwrap_or_else(|e| e.into_inner());
    let mut v = Invalidation {
        runs: measured.len(),
        runs_text: runs_text(&rec.meta, measured.len(), &rows),
        recalcs: 0,
        recalc_ms: 0.0,
        elements: 0.0,
        largest: 0.0,
        tracked: 0,
        what: BTreeMap::new(),
        selectors: BTreeMap::new(),
        sources: BTreeMap::new(),
    };
    for row in rows.iter().filter(|r| r.get(1).is_some_and(Option::is_some)) {
        let cell = |i: usize| row.get(i).cloned().flatten().unwrap_or_default();
        if !measured.contains(&cell(0)) {
            continue;
        }
        let name = cell(1);
        if name == "UpdateLayoutTree" {
            v.recalcs += 1;
            v.recalc_ms += cell(2).parse::<f64>().unwrap_or(0.0) / 1e6;
            let n = cell(3).parse::<f64>().unwrap_or(0.0);
            v.elements += n;
            v.largest = v.largest.max(n);
            continue;
        }
        v.tracked += 1;
        let kind = name.trim_end_matches("InvalidationTracking");
        let change = Some(cell(5)).filter(|c| !c.is_empty()).or_else(|| Some(cell(4)).filter(|r| !r.is_empty())).unwrap_or_else(|| "(no reason given)".into());
        let e = v.what.entry(format!("{change} ({kind})")).or_default();
        e.0 += 1;
        *e.1.entry(Some(cell(6)).filter(|n| !n.is_empty()).unwrap_or_else(|| "?".into())).or_default() += 1;
        for sel in cell(7).split('\u{1e}').filter(|s| !s.is_empty()) {
            *v.selectors.entry(sel.to_string()).or_default() += 1;
        }
        if let Some(f) = trace_stack(&cell(8)).into_iter().next() {
            let mut fr = site(&sym, &f);
            if fr.name.is_empty() {
                fr.name = "(anonymous)".into();
            }
            *v.sources.entry(fr).or_default() += 1;
        }
    }
    if v.tracked == 0 {
        return Err(no_tracking());
    }
    Ok(v)
}

fn no_tracking() -> Error {
    Error::usage(
        "this recording has no style invalidation tracking: attribution blocks trace it for a series that declares main.style or style.count (or with --style); record --probes attribution --style for a page",
    )
}

fn tracked(rec: &Rec, rows: &[Vec<Option<String>>]) -> bool {
    let measured = measured_runs(&rec.meta, rows);
    rows.iter().any(|r| r.get(1).and_then(Option::as_deref).is_some_and(|n| n != "UpdateLayoutTree") && r.first().and_then(Option::as_ref).is_some_and(|k| measured.contains(k)))
}

pub(super) use sources::index_check;

/// What invalidated style in the measured runs, from the trace's invalidation tracking (attribution
/// blocks of a series that declares `main.style` or `style.count`, or `--style`): the recalcs with
/// the elements each restyled, and the invalidations by what changed (a class, an id, an attribute,
/// a pseudo-class such as `:hover`, an inline style, a sheet), by the node it hit, by the selector
/// that matched, and by the script line that made the change.
pub fn style(rec: &Rec, top: usize) -> Result<Output> {
    let v = invalidation(rec)?;
    let runs = v.runs.max(1) as f64;
    let per = |n: u64| n as f64 / runs;
    let mut text = format!(
        "style invalidation · {}\n  recalcs      {:.1} per run · {} per run · {} elements restyled per run (largest recalc {} elements)\n",
        v.runs_text,
        per(v.recalcs),
        fmt::value(v.recalc_ms / runs, "ms"),
        fmt::value(v.elements / runs, "count"),
        fmt::value(v.largest, "count"),
    );
    let ranked = |m: &BTreeMap<String, u64>| {
        let mut r: Vec<(String, u64)> = m.iter().map(|(k, n)| (k.clone(), *n)).collect();
        r.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        r.truncate(top);
        r
    };
    let mut changes: Vec<_> = v.what.iter().collect();
    changes.sort_by_key(|c| std::cmp::Reverse(c.1.0));
    text += "  what changed (per run, the nodes it hit most)\n";
    let mut changes_json = Vec::new();
    for (k, (n, nodes)) in changes.into_iter().take(top) {
        let nodes = ranked(nodes);
        text += &format!("    {:>8.1}  {k}  · {}\n", per(*n), nodes.iter().take(3).map(|(nd, c)| format!("{nd} ×{c}")).collect::<Vec<_>>().join(", "));
        changes_json.push(json!({ "what": k, "per_run": per(*n), "count": n, "nodes": nodes }));
    }
    let sels = ranked(&v.selectors);
    if !sels.is_empty() {
        text += "  by selector (invalidation sets that matched)\n";
        for (s, n) in &sels {
            text += &format!("    {:>8.1}  {s}\n", per(*n));
        }
    }
    let sources: BTreeMap<String, u64> = v.sources.iter().fold(BTreeMap::new(), |mut m, (f, n)| {
        *m.entry(format!("{} ({})", f.name, f.location())).or_default() += n;
        m
    });
    let srcs = ranked(&sources);
    if !srcs.is_empty() {
        text += "  by source (the script line that made the change)\n";
        for (s, n) in &srcs {
            text += &format!("    {:>8.1}  {s}\n", per(*n));
        }
    }
    Ok(Output::new(
        text,
        json!({
            "runs": v.runs, "recalcs_per_run": per(v.recalcs), "recalc_ms_per_run": v.recalc_ms / runs, "elements_per_run": v.elements / runs, "largest_recalc": v.largest,
            "invalidations_per_run": per(v.tracked), "changes": changes_json, "selectors": sels, "sources": srcs,
        }),
    ))
}

/// The recording's per-run table (`run_phases`) kept beside its trace: a recording never changes, and
/// reading the table loads the whole trace (15 s for a 950 MB one) at every `query <rec> summary`.
const PHASES_CACHE: &str = "phases-cache.json";

/// `run_phases`, read from the recording's cache when this strobe wrote it for these runs; else
/// computed and cached. A trace that fails to load is not cached.
fn run_phases_cached(rec: &Rec) -> Option<(String, Value)> {
    if !rec.has(Part::Phases) {
        return None;
    }
    let path = rec.dir.join(PHASES_CACHE);
    // By this build (the table's reading may change between any two), the source it read (its size
    // and time: an imported or re-read file is another), the run marks' token and the runs it
    // covers (`--case` picks some).
    let source = ["trace.pftrace", crate::probe::gecko::READ, crate::probe::webkit_inspector::READ]
        .iter()
        .find_map(|f| {
            let m = std::fs::metadata(rec.dir.join(f)).ok()?;
            let ns = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
            Some(format!("{f}:{}:{ns}", m.len()))
        })
        .unwrap_or_default();
    let key =
        format!("{} {source} {} {} {}", env!("STROBE_BUILD_ID"), rec.meta["token"].as_str().unwrap_or(""), rec.meta["runs"], rec::measured_keys(&rec.meta).map(|k| k.join(",")).unwrap_or_default());
    let mut cache = store::read_json(&path).ok().filter(Value::is_object).unwrap_or_else(|| json!({}));
    if let Some(hit) = cache.get(&key) {
        return hit["text"].as_str().map(|t| (t.to_string(), hit["per_run"].clone()));
    }
    let got = run_phases(rec).ok()?;
    // Another build's rows are never read again.
    if let Some(o) = cache.as_object_mut() {
        o.retain(|k, _| k.starts_with(env!("STROBE_BUILD_ID")));
    }
    cache[key.as_str()] = match &got {
        Some((t, rows)) => json!({ "text": t, "per_run": rows }),
        None => json!({ "text": null }),
    };
    // A recording named by its path is read as it is: nothing is written beside it.
    if rec.stored()
        && let Ok(bytes) = serde_json::to_vec(&cache)
    {
        let _ = store::write_atomic(&path, &bytes);
    }
    got
}

/// `query <rec> runs`: every run of a block recording, warmup and unmeasured ones too, with its time
/// and work: from the run list the block kept (`meta.run_list`), else its measured runs (`runs.json`:
/// a native or script block's), else the run marks in its trace (a page's runs, by its main thread's
/// tasks). Where a block's first runs went (warmup), and which runs its values are.
pub fn runs_view(rec: &Rec) -> Result<Output> {
    runs_read(rec, false)
}

fn runs_read(rec: &Rec, probe: bool) -> Result<Output> {
    let measured: Option<Vec<String>> = rec::measured_keys(&rec.meta);
    let is_measured = |k: &str| measured.as_ref().is_none_or(|m| m.iter().any(|x| x == k));
    let num = |v: &Value, unit: &str| v.as_f64().filter(|x| x.is_finite()).map_or("—".to_string(), |x| fmt::value(x, unit));
    let run_no = |k: &str| k.rsplit(':').next().and_then(|i| i.parse::<u64>().ok());
    let label = |k: &str| run_no(k).map_or_else(|| k.to_string(), |i| format!("run {i}"));
    let (from, mut rows): (&str, Vec<Value>) = if let Some(list) = rec.meta["run_list"].as_array().filter(|l| !l.is_empty()) {
        ("the block's run list", list.clone())
    } else if let Some(list) = store::read_json(&rec.dir.join(crate::probe::cpu::RUNS)).ok().and_then(|v| v.as_array().or(v["runs"].as_array()).cloned()).filter(|l| !l.is_empty()) {
        // Measured runs only (a native block's list, a script block's `runs`): no warmup run's readings.
        let rows = list.iter().map(|r| json!({ "run": r["run"], "measured": true, "ms": r["ms"], "cpu_work_ms": r["cpu_ms"], "main_cpu_ms": r["main_cpu_ms"] })).collect();
        let from =
            if rec.meta["kind"] == "record" { "runs.json (a recording of a window: its one span is the whole recording)" } else { "runs.json (measured runs only: this recording kept no warmup run)" };
        (from, rows)
    } else if probe && rec.meta["kind"] != "record" && rec.dir.join("trace.pftrace").is_file() && rec.meta["token"].as_str().is_some_and(|t| !t.is_empty()) {
        return Err(Error::usage(format!("{}: run marks are not indexed; read this recording directly to inspect its trace", rec.id)));
    } else if let Some(marked) = (rec.meta["kind"] != "record" && rec.dir.join("trace.pftrace").is_file() && rec.meta["token"].as_str().is_some_and(|t| !t.is_empty()))
        .then(|| trace_runs(rec))
        .transpose()?
        .filter(|r| !r.is_empty())
    {
        // Not run.duration (the driver's wall time of the run) nor main.work: the page's own marks around
        // each run and the main thread's top-level tasks between them, read alike for every run. A
        // recording of a page (`record <url>`) marks its window, no runs: none here.
        (
            "the trace's run marks: each run's span between its marks (wider than run.duration, the driver's time, by the marks' round trips) and its main thread's top-level tasks (no CPU times: a trace has none)",
            marked.into_iter().map(|(k, ms, task)| json!({ "run": k, "measured": is_measured(&k), "marked_ms": ms, "task_ms": task })).collect(),
        )
    } else {
        return Err(Error::usage(format!("{} keeps no runs to list: a block recording keeps its runs (its run list, runs.json, or run marks in its trace); a recording of a window has none", rec.id)));
    };
    if probe {
        return available();
    }
    rows.sort_by_key(|r| r["run"].as_str().and_then(run_no));
    let cols: [(&str, &str, &str); 5] =
        [("ms", "run.duration", "ms"), ("marked_ms", "marked span", "ms"), ("cpu_work_ms", "cpu.work", "ms"), ("task_ms", "main tasks", "ms"), ("main_cpu_ms", "main CPU", "ms")];
    let shown: Vec<&(&str, &str, &str)> = cols.iter().filter(|(k, _, _)| rows.iter().any(|r| r[*k].as_f64().is_some())).collect();
    let mut table = vec![std::iter::once("run".to_string()).chain(std::iter::once(String::new())).chain(shown.iter().map(|c| c.1.to_string())).collect::<Vec<_>>()];
    for r in &rows {
        let k = r["run"].as_str().unwrap_or("");
        let what = match (r["measured"].as_bool().unwrap_or_else(|| is_measured(k)), r["cpu_lost"] == true) {
            (true, true) => "measured · CPU lost",
            (true, false) => "measured",
            (false, true) => "not measured · CPU lost",
            (false, false) => "not measured",
        };
        table.push(std::iter::once(label(k)).chain(std::iter::once(what.to_string())).chain(shown.iter().map(|c| num(&r[c.0], c.2))).collect());
    }
    let n_measured = rows.iter().filter(|r| r["measured"].as_bool().unwrap_or_else(|| is_measured(r["run"].as_str().unwrap_or("")))).count();
    let mut text = format!("runs  {} · {} run{}, {n_measured} measured · from {from}\n", rec.id, rows.len(), if rows.len() == 1 { "" } else { "s" });
    let right: Vec<bool> = std::iter::repeat_n(false, 2).chain(std::iter::repeat_n(true, shown.len())).collect();
    text += &fmt::table(&table, &right, "  ", 2);
    if rows.len() > n_measured {
        text += "  not measured: warmup until two runs agree, and a check's run after its reset; the values are the measured runs'\n";
    }
    Ok(Output::new(text, json!({ "recording": rec.id, "from": from, "runs": rows })))
}

/// A page recording's runs from the run marks in its trace, warmup ones too, in order: (key, ms
/// outside the run's untimed stretches, the renderer main thread's top-level task time in it: NaN
/// when the trace holds no tasks, as a lean block's does not).
fn trace_runs(rec: &Rec) -> Result<Vec<(String, f64, f64)>> {
    let token = rec.meta["token"].as_str().unwrap_or("");
    let sql = format!(
        "{}
select w.k, (w.t1 - w.t0) - coalesce((select sum(min(u.t1, w.t1) - u.t0) from _u u where u.t0 >= w.t0 and u.t0 < w.t1), 0),
  case when exists (select 1 from slice where name = 'RunTask') then
  (select coalesce(sum(min(s.ts + s.dur, w.t1) - max(s.ts, w.t0)), 0) from slice s join thread_track tt on s.track_id = tt.id join thread t using(utid)
    where s.depth = 0 and s.dur > 0 and s.name = 'RunTask' and t.name = 'CrRendererMain' and s.ts < w.t1 and s.ts + s.dur > w.t0
      and t.upid in (select p.upid from _wp p where p.k = w.k and p.t0 = w.t0)) end
from _w w order by w.t0;",
        rec::windows_sql(token, None)
    );
    let table = crate::engine::query(&rec.dir.join("trace.pftrace"), &sql)?;
    let ns = |c: Option<&Option<String>>| c.cloned().flatten().and_then(|v| v.parse::<f64>().ok()).unwrap_or(f64::NAN) / 1e6;
    Ok(table
        .rows
        .iter()
        .filter_map(|r| {
            let k = r.first().cloned().flatten()?;
            // `strobe:<token>:run:<i>`, as the block's `measured` names them.
            let key = k.strip_prefix(&format!("strobe:{token}:")).unwrap_or(&k).to_string();
            key.starts_with("run:").then(|| (key, ns(r.get(1)), ns(r.get(2))))
        })
        .collect())
}

/// Per measured run, the renderer main thread's style, layout and paint time and the elements style
/// recalcs touched, from a trace that has them (timing with `--trace full`, attribution): the numbers
/// a person would otherwise pull out with SQL over the run marks.
fn run_phases(rec: &Rec) -> Result<Option<(String, Value)>> {
    let meta = &rec.meta;
    let rows = rec.part(Part::Phases)?;
    let measured = measured_runs(meta, &rows);
    // [style, layout, paint] ms and elements restyled, per run; nested slices of one kind count once.
    let mut per: BTreeMap<String, ([f64; 3], f64)> = measured.iter().map(|k| (k.clone(), ([0.0; 3], 0.0))).collect();
    let mut end: BTreeMap<String, i64> = BTreeMap::new();
    let mut any = false;
    for row in rows.iter().filter(|r| r.get(1).is_some_and(Option::is_some)) {
        let cell = |i: usize| row.get(i).cloned().flatten().unwrap_or_default();
        let Some(run) = per.get_mut(&cell(0)) else { continue };
        let i = match cell(1).as_str() {
            "UpdateLayoutTree" => 0,
            "Layout" => 1,
            _ => 2,
        };
        let (ts, dur) = (cell(2).parse::<i64>().unwrap_or(0), cell(3).parse::<i64>().unwrap_or(0));
        let e = end.entry(format!("{} {i}", cell(0))).or_insert(i64::MIN);
        if ts < *e {
            continue;
        }
        *e = ts + dur;
        run.0[i] += dur as f64 / 1e6;
        if i == 0 {
            run.1 += cell(4).parse::<f64>().unwrap_or(0.0);
        }
        any = true;
    }
    if !any {
        return Ok(None);
    }
    let mut text = format!("per run  {} · renderer main thread\n", runs_text(meta, measured.len(), &rows));
    let mut out = Vec::new();
    // In run order: `run:10` after `run:9`.
    let mut ordered: Vec<(&String, &([f64; 3], f64))> = per.iter().collect();
    ordered.sort_by_key(|(k, _)| k.rsplit(':').next().and_then(|i| i.parse::<u64>().ok()));
    let mut table = vec![vec![String::new(), "style".into(), "(elements restyled)".into(), "layout".into(), "paint".into()]];
    for (k, (ms, el)) in ordered {
        let run = k.rsplit(':').next().map(|i| format!("run {i}")).unwrap_or_default();
        table.push(vec![run, fmt::time(ms[0]), format!("({})", fmt::value(*el, "count")), fmt::time(ms[1]), fmt::time(ms[2])]);
        out.push(json!({ "run": k, "style_ms": ms[0], "elements_restyled": el, "layout_ms": ms[1], "paint_ms": ms[2] }));
    }
    text += &fmt::table(&table, &[false, true, true, true, true], "  ", 2);
    Ok(Some((text, json!(out))))
}

/// Garbage collection in the measured runs: collections, the main-thread pause they cost (incremental
/// marking steps included) and the work they did on helper threads, from attribution traces.
pub fn gc(rec: &Rec) -> Result<Output> {
    attribution_meta(rec, "gc")?;
    let rows = rec.part(Part::Gc)?;
    let measured = measured_runs(&rec.meta, &rows);
    #[derive(Default, Clone, Copy)]
    struct Run {
        minor: u64,
        major: u64,
        main_ms: f64,
        helper_ms: f64,
        freed: f64,
    }
    let mut runs: BTreeMap<String, Run> = measured.iter().map(|k| (k.clone(), Run::default())).collect();
    // Each run's span and untimed stretches (a trace's `(run)` and `(untimed)` rows; a Firefox profile's
    // rows come cut to them already): a slice counts only its time inside the run and outside them, and
    // a collection only where it began there.
    let num = |row: &[Option<String>], i: usize| row.get(i).cloned().flatten().and_then(|c| c.parse::<f64>().ok()).unwrap_or(0.0) as i64;
    type Cut = ((i64, i64), Vec<(i64, i64)>);
    let mut cut: BTreeMap<String, Cut> = BTreeMap::new();
    for row in rows.iter().filter(|r| r.get(4).is_none_or(Option::is_none)) {
        let (k, a) = (row.first().cloned().flatten().unwrap_or_default(), num(row, 2));
        let b = a + num(row, 3);
        match row.get(1).cloned().flatten().as_deref() {
            Some("(run)") => cut.entry(k).or_insert(((i64::MIN, i64::MAX), Vec::new())).0 = (a, b),
            Some("(untimed)") => cut.entry(k).or_insert(((i64::MIN, i64::MAX), Vec::new())).1.push((a, b)),
            _ => {}
        }
    }
    let timed = |k: &str, a: i64, b: i64| -> i64 {
        let Some(((t0, t1), gaps)) = cut.get(k) else { return (b - a).max(0) };
        let (a, b) = (a.max(*t0), b.min(*t1));
        (b - a).max(0) - gaps.iter().map(|&(x, y)| (y.min(b) - x.max(a)).max(0)).sum::<i64>()
    };
    let began = |k: &str, ts: i64| cut.get(k).is_none_or(|((t0, t1), gaps)| ts >= *t0 && ts < *t1 && !gaps.iter().any(|&(x, y)| ts >= x && ts < y));
    // Union of GC intervals per run and thread: GC phases nest inside their collection, and a slice
    // across two runs is a row in each.
    let mut covered: BTreeMap<(String, String), i64> = BTreeMap::new();
    for row in rows.iter().filter(|r| r.get(4).is_some_and(Option::is_some)) {
        let cell = |i: usize| row.get(i).cloned().flatten().unwrap_or_default();
        let Some(run) = runs.get_mut(&cell(0)) else { continue };
        let (ts, dur) = (cell(2).parse::<i64>().unwrap_or(0), cell(3).parse::<i64>().unwrap_or(0));
        if began(&cell(0), ts) {
            match cell(1).as_str() {
                "MinorGC" => run.minor += 1,
                "MajorGC" => run.major += 1,
                _ => {}
            }
            run.freed += cell(6).parse::<f64>().unwrap_or(0.0).max(0.0);
        }
        let end = covered.entry((cell(0), cell(4))).or_insert(i64::MIN);
        let new = timed(&cell(0), ts.max(*end), ts + dur) as f64 / 1e6;
        *end = (*end).max(ts + dur);
        if cell(5) == "1" {
            run.main_ms += new;
        } else {
            run.helper_ms += new;
        }
    }
    let med = |f: fn(&Run) -> f64| metrics::median(&runs.values().map(f).collect::<Vec<_>>());
    let values = json!({
        "minor_per_run": med(|r| r.minor as f64),
        "major_per_run": med(|r| r.major as f64),
        "main_ms_per_run": med(|r| r.main_ms),
        "helper_ms_per_run": med(|r| r.helper_ms),
        "freed_bytes_per_run": med(|r| r.freed),
    });
    // Firefox's markers time the page's main thread only: its GC helper threads are not sampled.
    let firefox = rec.meta["engine"] == "firefox";
    let mut values = values;
    if firefox {
        values["helper_ms_per_run"] = Value::Null;
    }
    let g = |k: &str| values[k].as_f64().unwrap_or(f64::NAN);
    let helpers = if firefox {
        "not recorded (Firefox's profiler marks no GC helper-thread work)".to_string()
    } else {
        format!("{} per run (concurrent and parallel work)", fmt::value(g("helper_ms_per_run"), "ms"))
    };
    let mut text = format!(
        "garbage collection · median of {}\n  collections   {:.0} minor · {:.0} major per run\n  main thread   {} per run (pauses and incremental marking{})\n  helpers       {helpers}\n  freed         {} per run\n",
        runs_text(&rec.meta, runs.len(), &rows),
        g("minor_per_run"),
        g("major_per_run"),
        fmt::value(g("main_ms_per_run"), "ms"),
        if firefox { " slices" } else { "" },
        fmt::bytes(g("freed_bytes_per_run")),
    );
    text += &format!("  per run       minor major  {:>13}  {:>13}\n", "main", "helpers");
    let mut ordered: Vec<(&String, &Run)> = runs.iter().collect();
    ordered.sort_by_key(|(k, _)| k.rsplit(':').next().and_then(|i| i.parse::<u64>().ok()));
    let per_run: Vec<Value> = ordered
        .into_iter()
        .map(|(k, r)| {
            let helper = if firefox { "—".to_string() } else { fmt::time(r.helper_ms) };
            text += &format!("  {:<12}  {:>5} {:>5}  {:>13}  {:>13}\n", k.rsplit(':').next().map(|i| format!("run {i}")).unwrap_or_default(), r.minor, r.major, fmt::time(r.main_ms), helper);
            json!({ "run": k, "minor": r.minor, "major": r.major, "main_ms": r.main_ms, "helper_ms": if firefox { Value::Null } else { json!(r.helper_ms) }, "freed_bytes": r.freed })
        })
        .collect();
    Ok(Output::new(text, json!({ "values": values, "runs": per_run })))
}

fn attribution_meta(rec: &Rec, view: &str) -> Result<()> {
    if rec.meta["probes"] != "attribution" {
        return Err(no_profile(rec, "attribution stacks or GC phases (this view needs an attribution recording)", view));
    }
    Ok(())
}

/// A recording without the profile a view reads (`what`), and where one is: a bench's timed block
/// never runs a profiler (it would weigh on the times the block measures), its experiment's
/// attribution blocks do (`--why always`, or a series whose primary differs); anything else is
/// recorded again with `--probes attribution`.
pub(crate) fn no_profile(rec: &Rec, what: &str, view: &str) -> Error {
    let meta = &rec.meta;
    match meta["experiment"].as_str().filter(|_| meta["kind"] == "block" && meta["probes"] != "attribution") {
        Some(exp) => {
            // A copied recording's experiment belongs to its source machine. For a stored recording,
            // read the exact parent, not a prefix, and match its own series (not another case's profile).
            let result = (rec.stored() && store::plain_id(exp)).then(|| store::read_json(&crate::host::paths::experiments().join(exp).join("result.json")).ok()).flatten();
            let check = result.as_ref().is_some_and(|r| r["kind"] == "check") || meta["check"] == true;
            let kind = if check {
                "check"
            } else if result.is_some() {
                "timed"
            } else {
                "bench"
            };
            let hint = result.as_ref().and_then(|r| sources::hint(r, exp, rec, view)).unwrap_or_else(|| "run the bench again with --why always to keep an attribution recording".into());
            Error::usage(format!("this recording has no {what}: it is a {kind} block of {exp}, recorded without the profiler; {hint}"))
        }
        // It ran under the profilers already: recording it again the same way adds nothing (a Bun
        // script host's missing allocation samples are refused before this, `firefox_gap`).
        None if meta["probes"] == "attribution" => Error::usage(format!("this recording has no {what}, though it ran with --probes attribution: its target gave none, or its profiler failed")),
        None => Error::usage(format!("this recording has no {what}; record with --probes attribution")),
    }
}

/// The window keys of the measured runs, from a result whose window rows (`_w`) have only their key
/// set: the last `runs` of the run windows (a block warms up first; its attribution profilers start
/// at the steady point).
fn measured_runs(meta: &Value, rows: &[Vec<Option<String>>]) -> Vec<String> {
    let mut keys = run_keys(rows);
    // A bench block says which they are (`measured`; a Firefox recording's windows from its profile hold
    // its warmup runs and a check's run after its reset too).
    if let Some(m) = rec::measured_keys(meta) {
        return keys.into_iter().map(|k| k.1).filter(|k| m.iter().any(|x| k == x || k.ends_with(&format!(":{x}")))).collect();
    }
    let n = meta["runs"].as_u64().map_or(keys.len(), |n| n as usize).min(keys.len());
    keys.split_off(keys.len() - n).into_iter().map(|k| k.1).collect()
}

fn run_keys(rows: &[Vec<Option<String>>]) -> Vec<(usize, String)> {
    let mut keys: Vec<(usize, String)> =
        rows.iter().filter(|r| r.get(1).is_none_or(Option::is_none)).filter_map(|r| r.first().cloned().flatten()).filter_map(|k| Some((k.rsplit(':').next()?.parse().ok()?, k))).collect();
    keys.sort();
    keys.dedup();
    keys
}

/// `5 measured runs (7 in the trace: 2 warmup before them)`: a trace's run marks include warmup. One
/// case of a batch (`--case`) says whose runs the others are.
fn runs_text(meta: &Value, measured: usize, rows: &[Vec<Option<String>>]) -> String {
    let all = run_keys(rows).len();
    let s = if measured == 1 { "" } else { "s" };
    match meta["query_case"].as_str() {
        Some(case) if all > measured => format!("{measured} measured run{s} of case {case} ({all} in the trace: the launch's other cases and warmup)"),
        Some(case) => format!("{measured} measured run{s} of case {case}"),
        None if all > measured => format!("{measured} measured run{s} ({all} in the trace: {} warmup before)", all - measured),
        None => format!("{measured} measured run{s}"),
    }
}

/// A frame of a trace stack: url, 1-based line and column of the call, the function's name, and the
/// function's own start where the stack says it (Firefox's markers), else 0.
type TraceFrame = (String, i64, i64, String, i64, i64);

/// The call site of a trace stack frame: named after the function its stack says it is in, else after
/// the innermost function around it in the stored maps.
fn site(sym: &crate::probe::symbols::Symbolizer, f: &TraceFrame) -> Frame {
    if f.4 > 0 { sym.call_in(&f.0, f.1 - 1, f.2 - 1, f.4 - 1, f.5 - 1, &f.3) } else { sym.call_site(&f.0, f.1 - 1, f.2 - 1, &f.3) }
}

/// Frames of a trace event's `beginData.stackTrace` from `key \x1f value` pairs joined by `\x1e`,
/// innermost first.
fn trace_stack(raw: &str) -> Vec<TraceFrame> {
    let mut frames: BTreeMap<usize, TraceFrame> = BTreeMap::new();
    for pair in raw.split('\u{1e}').filter(|p| !p.is_empty()) {
        let Some((key, value)) = pair.split_once('\u{1f}') else { continue };
        let Some((index, field)) = key.split_once('[').and_then(|(_, r)| r.split_once("].")) else { continue };
        let Ok(i) = index.parse::<usize>() else { continue };
        let f = frames.entry(i).or_default();
        match field {
            "url" => f.0 = value.to_string(),
            "lineNumber" => f.1 = value.parse().unwrap_or(0),
            "columnNumber" => f.2 = value.parse().unwrap_or(0),
            "functionName" => f.3 = value.to_string(),
            "functionLine" => f.4 = value.parse().unwrap_or(0),
            "functionColumn" => f.5 = value.parse().unwrap_or(0),
            _ => {}
        }
    }
    frames.into_values().collect()
}

/// `(strobe driver)`: what strobe's page driver did in the page (a wait's checks), and the browser's own
/// work it called (a layout its checks measured), apart from the app's.
pub(crate) const DRIVER: &str = "(strobe driver)";
/// The browser's own work (a layout, a DOM call) that strobe's page driver called.
pub(crate) const VIA_DRIVER: &str = "(native, via strobe driver)";

/// What strobe's page driver had to do with a stack (root first): its leaf is the driver's script, or
/// the browser's own code that the driver's script called (the nearest frame with a script is the
/// driver's). None: the app's.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Driven {
    Js,
    Native,
}

impl Driven {
    pub(crate) fn of(stack: &[u32], frames: &[Frame]) -> Option<Driven> {
        let leaf = &frames[*stack.last()? as usize];
        let nearest = stack.iter().rev().map(|&i| &frames[i as usize]).find(|f| !f.url.is_empty())?;
        (nearest.url == crate::driver::SOURCE_URL).then_some(if leaf.url.is_empty() { Driven::Native } else { Driven::Js })
    }

    /// Its row by file or package.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Driven::Js => DRIVER,
            Driven::Native => VIA_DRIVER,
        }
    }
}

/// The row a leaf frame counts toward by function (its name), file or package.
fn row_key(f: &Frame, by: &str, packages: &mut BTreeMap<String, String>) -> String {
    match by {
        "package" if f.package.is_some() => f.package.clone().unwrap_or_default(),
        // A frame with no script: its file when it has one (the script host's), the profile's own
        // names (`(program)`, `(garbage collector)`, `(module)`) as themselves, any other one native code.
        "file" | "package" if f.url.is_empty() && !f.file.is_empty() => f.file.clone(),
        "file" | "package" if f.url.is_empty() && f.name.starts_with('(') => f.name.clone(),
        "file" | "package" if f.url.is_empty() => "(native)".into(),
        "file" => f.file.clone(),
        "package" => {
            let at = if f.mapped { &f.file } else { &f.url };
            packages.entry(at.clone()).or_insert_with(|| package(at)).clone()
        }
        _ => f.name.clone(),
    }
}

/// How `hotspots` orders its rows (`--sort`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    /// By self time: where the time was spent.
    Own,
    /// By total time: what the time was spent under, callers included.
    Total,
}

impl Sort {
    pub fn of(args: &Args) -> Result<Sort> {
        match args.get("sort") {
            None | Some("self") => Ok(Sort::Own),
            Some("total") => Ok(Sort::Total),
            Some(other) => Err(Error::usage(format!("--sort: self or total, not {other}"))),
        }
    }
}

/// Callers listed under rows sorted by self time when they are not among them (`rank`).
const CALLERS: usize = 8;

/// Ranks weighted stacks (leaf = the frame that spent the time or allocated the bytes) by function, file
/// or package. Rows keep complete locations; by function, each row keeps its three heaviest full stacks
/// and the text prints the heaviest in full. `header` may use `{samples}` and `{mapped}`. `total`
/// (CPU time): each row's total too, every stack its frame is in, once a stack; a function that only
/// calls is a row by its total, after the rows with time of their own (or among them, `Sort::Total`).
/// Sorted by self time, the heaviest callers by total time that missed the rows follow them.
fn rank(stacks: &Stacks, by: &str, top: usize, header: &str, show: impl Fn(f64) -> String, total_by: Option<Sort>) -> Output {
    let with_total = total_by.is_some();
    type ByStack<'a> = BTreeMap<&'a [u32], (f64, u64)>;
    /// (name, where) → (weight, samples, stacks).
    type Row<'a> = ((String, String), (f64, u64, ByStack<'a>));
    let frame = |i: u32| &stacks.frames[i as usize];
    let mut rows: BTreeMap<(String, String), (f64, u64, ByStack)> = BTreeMap::new();
    let (mut total, mut mapped, mut scripts, mut minified_unmapped) = (0.0, 0u64, 0u64, 0u64);
    let minified = minified_scripts(&stacks.frames);
    let samples: u64 = stacks.weights.iter().map(|w| w.n).sum();
    let mut packages: BTreeMap<String, String> = BTreeMap::new();
    let mut lines: BTreeMap<(String, String), BTreeMap<i64, f64>> = BTreeMap::new();
    // Inclusive weight: each row whose frame is anywhere in a stack, once per stack (recursion too).
    let mut inclusive: BTreeMap<(String, String), f64> = BTreeMap::new();
    for Weight { stack, w, n: hits, .. } in &stacks.weights {
        let Some(f) = stack.last().map(|&i| frame(i)) else { continue };
        // strobe's page driver at work: its own rows, never a share of the app's scripts.
        let driven = Driven::of(stack, &stacks.frames);
        if f.mapped && driven.is_none() {
            mapped += hits;
        }
        if !f.url.is_empty() && driven.is_none() {
            scripts += hits;
            if !f.mapped && minified.contains(&f.url) {
                minified_unmapped += hits;
            }
        }
        let key = match driven {
            Some(d) if by != "function" => d.label().to_string(),
            _ => row_key(f, by, &mut packages),
        };
        // A native frame is located at the sampled line: its function's row is the function in its
        // file, and the lines it spent the time on are kept beside it.
        let native = f.package.is_some() && f.line > 0;
        let detail = match by {
            "function" if driven == Some(Driven::Native) => VIA_DRIVER.to_string(),
            "function" if native => f.file.clone(),
            "function" => f.location(),
            _ => String::new(),
        };
        if native && by == "function" {
            *lines.entry((key.clone(), detail.clone())).or_default().entry(f.line).or_default() += w;
        }
        if with_total {
            let mut within: std::collections::BTreeSet<(String, String)> = std::collections::BTreeSet::new();
            within.insert((key.clone(), detail.clone()));
            for g in stack[..stack.len() - 1].iter().map(|&i| frame(i)) {
                let d = match by {
                    "function" if g.package.is_some() && g.line > 0 => g.file.clone(),
                    "function" => g.location(),
                    _ => String::new(),
                };
                // strobe's page driver by file or package is its own row, never the app's.
                let k = if by != "function" && g.url == crate::driver::SOURCE_URL { DRIVER.to_string() } else { row_key(g, by, &mut packages) };
                within.insert((k, d));
            }
            for k in within {
                *inclusive.entry(k).or_default() += w;
            }
        }
        let e = rows.entry((key, detail)).or_insert((0.0, 0, ByStack::new()));
        e.0 += w;
        e.1 += hits;
        if by == "function" {
            let st = e.2.entry(stack.as_slice()).or_insert((0.0, 0));
            st.0 += w;
            st.1 += hits;
        }
        total += w;
    }
    // Callers that spent no time themselves (a function that only calls) are rows too, by their total.
    for k in inclusive.keys() {
        rows.entry(k.clone()).or_insert((0.0, 0, ByStack::new()));
    }
    let mut rows: Vec<Row> = rows.into_iter().collect();
    let incl_of = |k: &(String, String), own: f64| inclusive.get(k).copied().unwrap_or(own);
    if total_by == Some(Sort::Total) {
        rows.sort_by(|a, b| incl_of(&b.0, b.1.0).total_cmp(&incl_of(&a.0, a.1.0)).then_with(|| b.1.0.total_cmp(&a.1.0)));
    } else {
        rows.sort_by(|a, b| b.1.0.total_cmp(&a.1.0).then_with(|| inclusive.get(&b.0).unwrap_or(&0.0).total_cmp(inclusive.get(&a.0).unwrap_or(&0.0))));
    }
    let mapped_text = if mapped > 0 { format!(" · {:.0}% of script samples source-mapped", 100.0 * mapped as f64 / scripts.max(1) as f64) } else { String::new() };
    let mut text = header.replace("{samples}", &samples.to_string()).replace("{mapped}", &mapped_text) + "\n";
    let mut json_rows = Vec::new();
    let n = (samples as f64).max(1.0);
    // The weight columns as wide as their widest cell (`1,234,567 µs`).
    let wd = rows.iter().take(top).flat_map(|(k, (w, _, _))| [show(*w), show(incl_of(k, *w))]).map(|s| s.chars().count()).max().unwrap_or(0).max(9);
    for ((k, detail), (w, hits, row_stacks)) in rows.iter().take(top) {
        let share = if total > 0.0 { w / total } else { 0.0 };
        let ci = 1.96 * (share * (1.0 - share) / n).sqrt();
        let incl = inclusive.get(&(k.clone(), detail.clone())).copied().unwrap_or(*w);
        let incl_share = if total > 0.0 { incl / total } else { 0.0 };
        let total_col = if with_total { format!("{:>wd$} {:>5.1}%  ", show(incl), incl_share * 100.0) } else { String::new() };
        text += &format!("  {:>wd$}  {:>5.1}% ±{:>4.2}  {total_col}{}  {}\n", show(*w), share * 100.0, ci * 100.0, k, detail);
        let mut hot: Vec<(i64, f64)> = lines.get(&(k.clone(), detail.clone())).map(|l| l.iter().map(|(a, b)| (*a, *b)).collect()).unwrap_or_default();
        hot.sort_by(|a, b| b.1.total_cmp(&a.1));
        if !hot.is_empty() {
            text += &format!("             lines {}\n", hot.iter().take(4).map(|(l, x)| format!(":{l} {:.0}%", 100.0 * x / w.max(1e-9))).collect::<Vec<_>>().join(" · "));
        }
        let mut heavy: Vec<(&&[u32], &(f64, u64))> = row_stacks.iter().collect();
        // Equal weights in frame order, root first.
        heavy.sort_by(|a, b| b.1.0.total_cmp(&a.1.0).then_with(|| a.0.iter().map(|&i| frame(i)).cmp(b.0.iter().map(|&i| frame(i)))));
        if let Some((stack, (sw, _))) = heavy.first().filter(|(s, _)| s.len() > 1) {
            text += &format!("             heaviest stack ({:.0}% of this row):\n", 100.0 * sw / w.max(1e-9));
            for fr in stack.iter().rev().skip(1).map(|&i| frame(i)) {
                text += &format!("               ← {}  {}\n", fr.name, fr.location());
            }
        }
        let stacks_json: Vec<Value> =
            heavy.iter().take(3).map(|(stack, (sw, sn))| json!({ "weight": sw, "samples": sn, "frames": stack.iter().map(|&i| frame(i).json()).collect::<Vec<_>>() })).collect();
        let mut row = json!({ "name": k, "weight": w, "samples": hits, "share": share, "share_ci": ci, "where": detail, "stacks": stacks_json });
        if with_total {
            row["inclusive"] = json!(incl);
            row["total_share"] = json!(incl_share);
        }
        if !hot.is_empty() {
            row["line"] = json!(hot[0].0);
            row["lines"] = json!(hot.iter().take(10).map(|(l, x)| json!({ "line": l, "weight": x })).collect::<Vec<_>>());
        }
        json_rows.push(row);
    }
    // Sorted by self time, a caller whose time is all in what it called (a function that only
    // calls) ranks under every row with time of its own: the heaviest by total that missed the rows.
    let mut callers_json = Vec::new();
    if total_by == Some(Sort::Own) && total > 0.0 {
        let mut hidden: Vec<(&(String, String), f64, f64)> = rows.iter().skip(top).map(|(k, (w, _, _))| (k, *w, incl_of(k, *w))).filter(|(_, _, incl)| incl / total >= 0.05).collect();
        hidden.sort_by(|a, b| b.2.total_cmp(&a.2));
        if !hidden.is_empty() {
            text += "  callers by total time, not in the rows above (their time is mostly in what they called; --sort total ranks every row by it):\n";
        }
        for ((k, detail), own, incl) in hidden.into_iter().take(CALLERS) {
            text += &format!("  {:>wd$} {:>5.1}%  self {:>wd$}  {k}  {detail}\n", show(incl), 100.0 * incl / total, show(own));
            callers_json.push(json!({ "name": k, "where": detail, "weight": own, "inclusive": incl, "total_share": incl / total }));
        }
    }
    let mut out = json!({ "by": by, "samples": samples, "script_samples": scripts, "mapped_samples": mapped, "minified_unmapped_samples": minified_unmapped, "total": total, "rows": json_rows });
    if let Some(s) = total_by {
        out["sort"] = json!(if s == Sort::Total { "total" } else { "self" });
        out["callers"] = json!(callers_json);
    }
    Output::new(text, out)
}

/// The scripts whose frames read minified: a script file (not an HTML document, whose inline scripts
/// are its own source, nor a WebAssembly module, whose column is a byte offset) whose unmapped frames
/// have one past column 300 (a bundle's long lines) or mostly one-letter function names (a minifier
/// spends every one-letter name of a scope before any two-letter one; a person writes `on`, `el`, `px`).
/// A source map is what they lack; a script written as it runs lacks none. Mapped frames (which carry
/// their original names and columns) say nothing of the bundle around them.
fn minified_scripts(frames: &[Frame]) -> std::collections::BTreeSet<String> {
    // url → (largest column, named frames, one-letter-named frames)
    let mut seen: BTreeMap<&str, (i64, u64, u64)> = BTreeMap::new();
    for f in frames.iter().filter(|f| !f.url.is_empty() && f.package.is_none() && !f.mapped) {
        let e = seen.entry(f.url.as_str()).or_default();
        e.0 = e.0.max(f.column);
        if !f.name.is_empty() && !f.name.starts_with('(') {
            e.1 += 1;
            e.2 += u64::from(f.name.chars().count() == 1);
        }
    }
    let document = |url: &str| {
        let path = url.split(['?', '#']).next().unwrap_or(url);
        let last = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
        url.starts_with("wasm:") || last.is_empty() || [".html", ".htm", ".xhtml", ".wasm"].iter().any(|x| last.ends_with(x))
    };
    seen.into_iter().filter(|(url, (col, named, short))| !document(url) && (*col > 300 || (*named > 0 && 2 * short >= *named))).map(|(url, _)| url.to_string()).collect()
}

fn short_url(url: &str) -> String {
    if url.is_empty() {
        return "(native)".into();
    }
    let path = url.split("://").nth(1).map(|r| r.split_once('/').map(|x| x.1).unwrap_or(r)).unwrap_or(url);
    path.split('?').next().unwrap_or(path).to_string()
}

/// The package a script or file belongs to: the folder after the last `node_modules/` (with its scope),
/// else for a file on disk the `name` in its nearest `package.json`, else the URL's first path segment.
/// A runtime's own scripts (`internal:…`) are `(runtime)`.
pub fn package(url: &str) -> String {
    if url.is_empty() {
        return "(native)".into();
    }
    if url.starts_with("internal:") {
        return "(runtime)".into();
    }
    let url = url.replace('\\', "/");
    // A file URL as the path on disk: file:///C:/… is C:/…, file:///… is /….
    let url = match url.strip_prefix("file://") {
        Some(rest) => rest.strip_prefix('/').filter(|r| r.as_bytes().get(1) == Some(&b':')).unwrap_or(rest),
        None => &url,
    };
    if let Some(i) = url.rfind("node_modules/") {
        let rest = &url[i + 13..];
        let mut parts = rest.split('/');
        let first = parts.next().unwrap_or("");
        return if first.starts_with('@') { format!("{first}/{}", parts.next().unwrap_or("")) } else { first.to_string() };
    }
    // Never another machine's path (`//host/…`), and never while a folder a person named is read: its
    // frames' files are not this disk's (`store::may_look`).
    let declared =
        store::may_look(url).then(|| Path::new(url).ancestors().skip(1).take(12).find_map(|d| store::read_json(&d.join("package.json")).ok()?["name"].as_str().map(str::to_string))).flatten();
    if let Some(name) = declared {
        return name;
    }
    // A file on disk outside any package: the app's own code.
    if url.starts_with('/') || url.as_bytes().get(1) == Some(&b':') {
        return "(app)".into();
    }
    let path = short_url(url);
    // A relative script URL (`../chunk.js`) names its folder, never `..`.
    let mut parts = path.split('/').filter(|s| !s.is_empty() && *s != "." && *s != "..");
    match parts.next() {
        Some(scope) if scope.starts_with('@') => format!("{scope}/{}", parts.next().unwrap_or("")),
        Some(first) => first.to_string(),
        None => "(app)".into(),
    }
}

/// `strobe record file:<path>`: turns an existing capture into a recording (SPEC §8.7).
pub fn import(path: &str) -> Result<Output> {
    let src = Path::new(path);
    if !src.exists() {
        return Err(Error::usage(format!("{path} does not exist")));
    }
    let (id, draft) = store::new_recording()?;
    let dir = draft.dir().to_path_buf();
    let mut kinds = Vec::new();
    let copy = |from: &Path, name: &str| -> Result<()> { std::fs::copy(from, dir.join(name)).map(|_| ()).map_err(Error::from) };
    if src.is_dir() {
        let manifest = src.join("manifest.json");
        for e in std::fs::read_dir(src)?.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if name.ends_with(".heapsnapshot") || name.ends_with(".json") {
                std::fs::create_dir_all(dir.join("import"))?;
                copy(&p, &format!("import/{name}"))?;
                kinds.push(name);
            }
        }
        if !manifest.exists() && kinds.is_empty() {
            return Err(Error::usage("the folder has nothing strobe can read (no manifest.json, heap snapshots or traces)"));
        }
    } else {
        let name = src.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if name.ends_with(".cpuprofile") {
            copy(src, "cpu-0.cpuprofile")?;
        } else if name.ends_with(".heapsnapshot") {
            std::fs::create_dir_all(dir.join("import"))?;
            copy(src, &format!("import/{name}"))?;
        } else {
            copy(src, "trace.pftrace")?;
        }
        kinds.push(name);
    }
    let meta = json!({ "id": id, "kind": "import", "target": format!("file:{path}"), "probes": "import", "token": "", "valid": "unknown", "files": kinds, "values": {}, "duration_ms": 0, "created": super::record::now_iso() });
    store::write_json(&dir.join("meta.json"), &meta)?;
    draft.keep();
    let text = format!(
        "{id} · imported {} · valid: unknown (nothing proves how it was taken)\nnext     strobe query {id} sql \"select name, count(*) from slice group by 1 order by 2 desc limit 20\"\n",
        path
    );
    Ok(Output::new(text, meta))
}
