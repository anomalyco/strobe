//! Which recording answers a view of a bench result. A check's recorded launches (possibly shared
//! across cases or repeated) are not its per-case attribution recordings.

use super::*;
use std::collections::BTreeSet;

fn launches(s: &Value) -> impl Iterator<Item = &str> {
    s["recording"].as_str().into_iter().chain(s["repeat"]["recordings"].as_array().into_iter().flatten().filter_map(Value::as_str))
}

fn stored(id: &str) -> Option<std::path::PathBuf> {
    if !store::plain_id(id) {
        return None;
    }
    let dir = crate::host::paths::recordings().join(id);
    dir.join("meta.json").is_file().then_some(dir)
}

/// After the check (never in a measured block or a listing), index precisely the run windows a
/// trace has, and whether a measured run has a style invalidation. The trace by itself proves
/// neither. One small result per run, no stacks, symbols or event payloads.
pub(in crate::app) fn index_check(series: &[Value]) {
    let ids: BTreeSet<_> = series.iter().flat_map(|s| launches(s).chain(s["attribution"].as_str())).collect();
    for id in ids {
        let Some(dir) = stored(id) else { continue };
        let file = dir.join("meta.json");
        let Ok(mut meta) = store::read_json(&file) else { continue };
        if dir.join("deep.json.gz").is_file() {
            if let Ok(status) = waits::header(&dir) {
                meta["wait_capture"] = status;
            }
        } else if dir.join("trace.pftrace").is_file()
            && let Some(token) = meta["token"].as_str().filter(|s| store::plain_id(s))
        {
            let sql = rec::windows_sql(token, None);
            let tracked = if meta["probes"] == "attribution" { rec::TRACKED_RUN } else { "0" };
            let Ok(table) = engine::query(&dir.join("trace.pftrace"), &format!("{sql}select k, {tracked} from _w")) else { continue };
            let prefix = format!("strobe:{token}:");
            let mut runs = Vec::new();
            let mut tracking = Vec::new();
            for row in &table.rows {
                if let Some(k) = row[0].as_deref().and_then(|k| k.strip_prefix(&prefix)) {
                    runs.push(k);
                    if row[1].as_deref() == Some("1") {
                        tracking.push(k);
                    }
                }
            }
            meta["trace_runs"] = json!(runs);
            if meta["probes"] == "attribution" {
                meta["style_runs"] = json!(tracking);
            }
        } else {
            continue;
        }
        // An index is optional. Its absence never invalidates the check or its direct views.
        let _ = store::write_json(&file, &meta);
    }
}

pub(super) fn forward(rec: Rec, label: &str, view: &str, args: &Args, top: usize) -> Result<Output> {
    let id = rec.id.clone();
    let mut out = recording(rec, view, args, top).map_err(|mut e| {
        e.message = format!("{label} · {id}: {}", e.message);
        e
    })?;
    out.text = format!("{label} · {id}\n{}", out.text);
    if !out.json.is_object() {
        out.json = json!({"data": out.json});
    }
    out.json["recording_source"] = json!({"series": label, "recording": id});
    Ok(out)
}

pub(super) fn check(r: &Value, view: &str, args: &Args, top: usize) -> Result<Output> {
    let profile = matches!(view, "hotspots" | "alloc" | "gc" | "layout" | "style" | "heap" | "shifts" | "waits");
    let mut active: Vec<_> = r["series"].as_array().into_iter().flatten().filter(|s| !matches!(s["kind"].as_str(), Some("refused" | "gap")) && s["not_run"] != true).collect();
    let mut args = args.clone();
    if profile && let Some(want) = args.get("case") {
        active.retain(|s| s["label"] == want || s["case"] == want || s["batch_seq"].as_u64().is_some_and(|n| n.to_string() == want));
        if active.is_empty() {
            return Err(Error::usage(format!("no active case {want:?} in this check")));
        }
        // The selected attribution recording is already that case alone, not its shared launch.
        args.remove("case");
    }
    let mut seen = BTreeSet::new();
    let mut candidates = Vec::new();
    for s in &active {
        let launch = !profile && launches(s).next().is_some();
        for id in launches(s).filter(|_| launch).chain(s["attribution"].as_str().filter(|_| !launch)) {
            if seen.insert(id)
                && let Some(dir) = stored(id)
            {
                candidates.push((id, s["label"].as_str().or(s["case"].as_str()).unwrap_or("check"), dir));
            }
        }
    }
    if active.len() == 1
        && let [(_, label, dir)] = candidates.as_slice()
    {
        let rec = Rec::open(dir);
        if rec.meta["batch"].as_array().is_none_or(|b| b.len() <= 1) {
            store::used(dir);
            return forward(rec, label, view, &args, top);
        }
    }
    if candidates.is_empty() {
        return Err(Error::usage(if profile { "this check has no kept attribution recording here; run the check with --why always to keep one" } else { "this check has no kept recording here" }));
    }
    // A file alone does not establish that a view works: its runs, engine, optional probes and
    // query arguments matter. Use the same reader as a direct query and keep its typed refusal.
    let mut commands = Vec::new();
    let mut reasons: Vec<(&str, &str, Error)> = Vec::new();
    let tail = query_tail(&args);
    for (id, label, dir) in candidates {
        match recording_view(Rec::open(&dir), view, &args, top, true) {
            Ok(_) => commands.push(format!("strobe query {id} {tail}")),
            Err(e) => {
                if !reasons.iter().any(|(_, _, r)| (r.code, r.validator(), &r.message, &r.hint) == (e.code, e.validator(), &e.message, &e.hint)) {
                    reasons.push((label, id, e));
                }
            }
        }
    }
    if commands.is_empty() {
        if reasons.len() == 1 {
            return Err(reasons.remove(0).2);
        }
        let code = if reasons.iter().all(|(_, _, e)| e.code == 4) { 4 } else { 2 };
        let mut text = format!("no recording of this check answers {view}:");
        let mut rows = Vec::new();
        for (label, id, e) in reasons {
            text += &format!("\n  {label} · {id}: {}", e.message);
            if let Some(h) = &e.hint {
                text += &format!("\n    fix: {h}");
            }
            rows.push(json!({"series":label,"recording":id,"code":e.code,"validator":e.validator(),"error":e.message,"hint":e.hint}));
        }
        let mut e = Error::new(code, text);
        e.recording_errors = Some(Box::new(json!(rows)));
        return Err(e);
    }
    Err(Error::usage(format!("choose a recording of this check:\n  {}", commands.join("\n  "))))
}

/// Keep the question intact, including a SQL statement or `threads --timeline`.
fn query_tail(args: &Args) -> String {
    use crate::cli::{FLAGS, Kind};
    let mut words: Vec<_> = args.pos.iter().skip(1).map(|w| exp_insights::quote(w)).collect();
    for &(name, kind, _) in FLAGS {
        if let Some(value) = args.get(name) {
            words.push(format!("--{name}"));
            if !matches!(kind, Kind::Switch) {
                words.push(exp_insights::quote(value));
            }
        }
    }
    words.join(" ")
}

/// Only offer a profile query when its source exists. Invalidation tracking is optional even in
/// an attribution trace. A summary remains useful when that particular probe was absent.
fn answers(id: &str, view: &str) -> bool {
    let Some(dir) = stored(id) else { return false };
    let r = Rec::open(&dir);
    if r.meta["probes"] != "attribution" {
        return false;
    }
    match view {
        "hotspots" => r.cpu().is_ok() || dir.join("deep.json.gz").is_file(),
        "alloc" => r.alloc().is_ok(),
        "layout" => r.has(Part::Layout),
        "gc" => r.has(Part::Gc),
        "style" => invalidation(&r).is_ok(),
        _ => false,
    }
}

pub(super) fn hint(result: &Value, exp: &str, rec: &Rec, view: &str) -> Option<String> {
    let series = result["series"].as_array()?;
    let qc = rec.meta["query_case"].as_str();
    let check = result["kind"] == "check";
    let matches: Vec<_> = series
        .iter()
        .filter(|s| {
            if check {
                launches(s).any(|id| id == rec.id) && qc.is_none_or(|q| s["label"] == q)
            } else {
                s["timing_recordings"].as_array().into_iter().flatten().any(|b| b["recording"] == rec.id) || rec.meta["series"].as_str().is_some_and(|l| s["label"] == l)
            }
        })
        .collect();
    if matches.len() > 1 {
        return matches.iter().any(|s| s["attribution"].as_str().is_some_and(|id| stored(id).is_some())).then(|| format!("this launch covers several cases: strobe query {exp} summary"));
    }
    let s = *matches.first()?;
    let a = &s["attribution"];
    let ids: Vec<_> = [a.as_str(), a["base"].as_str(), a["head"].as_str()].into_iter().flatten().filter(|id| *id != rec.id && stored(id).is_some()).collect();
    if ids.is_empty() {
        return None;
    }
    let blocks = s["kind"] == "blocks";
    let comparable = result["comparison"] != "implementations" && diff::VIEWS.contains(&view) && ids.len() == 2;
    if !check && ((blocks && series.iter().filter(|s| s["attribution"]["head"].is_string()).count() == 1) || comparable) && ids.iter().all(|id| answers(id, view)) {
        return Some(format!("read the kept attribution with strobe query {exp} {view}"));
    }
    Some(ids.into_iter().map(|id| format!("strobe query {id} {}", if answers(id, view) { view } else { "summary" })).collect::<Vec<_>>().join("\n  "))
}
