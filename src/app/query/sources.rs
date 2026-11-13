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
    let active: Vec<_> = r["series"].as_array().into_iter().flatten().filter(|s| !matches!(s["kind"].as_str(), Some("refused" | "gap")) && s["not_run"] != true).collect();
    let mut ids = BTreeSet::new();
    for s in &active {
        if !profile {
            ids.extend(launches(s).filter(|id| stored(id).is_some()));
        }
        if profile || launches(s).next().is_none() {
            ids.extend(s["attribution"].as_str().filter(|id| stored(id).is_some()));
        }
    }
    if let ([s], Some(&id)) = (active.as_slice(), ids.first().filter(|_| ids.len() == 1)) {
        let rec = Rec::resolve(id)?;
        if rec.meta["batch"].as_array().is_none_or(|b| b.len() <= 1) {
            return forward(rec, s["label"].as_str().or(s["case"].as_str()).unwrap_or("check"), view, args, top);
        }
    }
    if ids.is_empty() {
        return Err(Error::usage(if profile { "this check has no kept attribution recording here; run the check with --why always to keep one" } else { "this check has no kept recording here" }));
    }
    let commands = ids
        .into_iter()
        .map(|id| {
            let next = if matches!(view, "hotspots" | "alloc" | "layout" | "gc" | "style") && !answers(id, view) { "summary" } else { view };
            format!("strobe query {id} {next}")
        })
        .collect::<Vec<_>>()
        .join("\n  ");
    Err(Error::usage(format!("choose a recording of this check:\n  {commands}")))
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
        return Some(format!("this launch covers several cases: strobe query {exp} summary"));
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
