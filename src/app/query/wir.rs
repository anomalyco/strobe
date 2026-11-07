//! `query <rec> log` and `query <rec> network` for iOS Web Inspector (`wir:`) recordings.

use crate::app::{Output, fmt};
use crate::cli::Args;
use crate::host::ios::logs::{CrashReport, DeviceLogs, redact_text, scrub_url};
use crate::model::{Error, Result};
use crate::probe::webkit_inspector::{self as wki, NetworkWaterfall};
use crate::target::wir::ConsoleEntry;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

pub fn log(dir: &Path, meta: &Value, args: &Args, top: usize) -> Result<Output> {
    let source = args.get("source");
    if let Some(s) = source
        && !matches!(s, "console" | "device" | "crash")
    {
        return Err(Error::usage(format!("--source: console, device or crash, not {s}")));
    }
    let has_any = dir.join(wki::CONSOLE_FILE).is_file() || dir.join(wki::DEVICE_LOG_FILE).is_file() || dir.join(wki::CRASHES_FILE).is_file();
    if !has_any {
        return Err(Error::usage("this recording kept no logs (strobe record wir: captures console, Safari/WebKit device logs and crash reports)"));
    }

    let reveal = args.has("reveal");
    let id = meta["id"].as_str().unwrap_or("");
    let want_crash = source.is_none() || source == Some("crash");
    let want_console = source.is_none() || source == Some("console");
    let want_device = source.is_none() || source == Some("device");

    let raw_crashes: Vec<CrashReport> = wki::read_gz_json(&dir.join(wki::CRASHES_FILE)).ok().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    let raw_console: Vec<ConsoleEntry> = wki::read_gz_json(&dir.join(wki::CONSOLE_FILE)).ok().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    let raw_device: DeviceLogs = wki::read_gz_json(&dir.join(wki::DEVICE_LOG_FILE)).ok().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();

    let crashes: Vec<Value> = raw_crashes
        .iter()
        .map(|c| {
            json!({
                "kind": c.kind,
                "process": c.process,
                "pid": c.pid,
                "timestamp": c.timestamp,
                "summary": redact_text(&c.summary, reveal),
                "exception": c.exception.as_deref().map(|x| redact_text(x, reveal)),
                "termination": c.termination.as_deref().map(|x| redact_text(x, reveal)),
                "memory_pages": c.memory_pages,
                "frames": c.frames,
            })
        })
        .collect();

    let console: Vec<Value> = raw_console
        .iter()
        .map(|c| {
            json!({
                "at_ms": c.at_ms,
                "source": c.source,
                "level": c.level,
                "text": redact_text(&c.text, reveal),
                "url": c.url.as_deref().map(|u| scrub_url(u, reveal)),
                "line": c.line,
                "column": c.column,
                "repeat": c.repeat_count,
            })
        })
        .collect();

    let dev_entries: Vec<Value> = raw_device
        .entries
        .iter()
        .map(|e| {
            json!({
                "at_ms": e.at_ms,
                "pid": e.pid,
                "process": e.process,
                "level": e.level,
                "subsystem": e.subsystem,
                "category": e.category,
                "message": redact_text(&e.message, reveal),
            })
        })
        .collect();

    let mut text = format!(
        "log · {id} · {} crashes/jetsams · {} console · {} Safari/WebKit device logs{}\n",
        crashes.len(),
        console.len(),
        dev_entries.len(),
        if reveal { " · revealed" } else { " · secrets scrubbed (--reveal shows raw)" }
    );

    if want_crash && (!crashes.is_empty() || source == Some("crash")) {
        text += &format!("crashes ({})\n", crashes.len());
        if crashes.is_empty() {
            text += "  none in [t0, t1 + 5s]\n";
        }
        for c in crashes.iter().take(top) {
            text += &format!("  ! {:<7} {}\n", c["kind"].as_str().unwrap_or("crash"), c["summary"].as_str().unwrap_or(""));
            for fr in c["frames"].as_array().into_iter().flatten().take(5) {
                if let Some(f) = fr.as_str() {
                    text += &format!("            {f}\n");
                }
            }
        }
    }

    if want_console && (!console.is_empty() || source == Some("console")) {
        let errs = console.iter().filter(|c| c["level"] == "error").count();
        text += &format!("console ({} messages, {errs} errors)\n", console.len());
        for c in console.iter().take(top) {
            let loc = match (c["url"].as_str(), c["line"].as_i64()) {
                (Some(u), Some(l)) => format!(" ({u}:{l})"),
                (Some(u), None) => format!(" ({u})"),
                _ => String::new(),
            };
            let rep = c["repeat"].as_u64().filter(|&r| r > 1).map(|r| format!(" ×{r}")).unwrap_or_default();
            let line1 = c["text"].as_str().unwrap_or("").lines().next().unwrap_or("");
            text += &format!("  {:>7.1} ms  {:<8} {line1}{rep}{loc}\n", c["at_ms"].as_f64().unwrap_or(0.0), c["level"].as_str().unwrap_or("log"));
        }
        if console.len() > top {
            text += &format!("  … {} more (--top N or --source console)\n", console.len() - top);
        }
    }

    if want_device && (!dev_entries.is_empty() || source == Some("device")) {
        let via = if raw_device.source.is_empty() { "device" } else { raw_device.source.as_str() };
        text += &format!("device ({} entries via {via}, Safari + WebKit allowlist only)\n", dev_entries.len());
        for e in dev_entries.iter().take(top) {
            let pid = e["pid"].as_u64().map(|p| format!("[{p}]")).unwrap_or_default();
            let label = match (e["subsystem"].as_str(), e["category"].as_str()) {
                (Some(s), Some(c)) => format!("{s}:{c} "),
                (Some(s), None) => format!("{s} "),
                _ => String::new(),
            };
            let line1 = e["message"].as_str().unwrap_or("").lines().next().unwrap_or("");
            text += &format!("  {:>7.1} ms  {:<7} {}{pid}  {label}{line1}\n", e["at_ms"].as_f64().unwrap_or(0.0), e["level"].as_str().unwrap_or("notice"), e["process"].as_str().unwrap_or(""));
        }
        if dev_entries.len() > top {
            text += &format!("  … {} more (--top N or --source device)\n", dev_entries.len() - top);
        }
    }

    let out_json = json!({
        "recording": id,
        "source": source,
        "revealed": reveal,
        "crashes": if want_crash { Value::Array(crashes) } else { Value::Null },
        "console": if want_console { Value::Array(console) } else { Value::Null },
        "device": if want_device {
            json!({
                "relay": raw_device.source,
                "dropped": raw_device.dropped,
                "entries": dev_entries,
            })
        } else {
            Value::Null
        },
    });
    Ok(Output::new(text, out_json))
}

pub fn network(dir: &Path, meta: &Value, args: &Args, top: usize) -> Result<Output> {
    let path = dir.join(wki::NETWORK_FILE);
    if !path.is_file() {
        return Err(Error::usage("this recording has no network waterfall (strobe record wir: records WebKit's Network waterfall)"));
    }
    let wf: NetworkWaterfall = serde_json::from_value(wki::read_gz_json(&path)?)?;
    let reveal = args.has("reveal");
    let id = meta["id"].as_str().unwrap_or("");

    let mut wire_total = 0u64;
    let mut decoded_total = 0u64;
    let mut cached = 0usize;
    let mut failed = 0usize;
    let mut by_proto: BTreeMap<String, usize> = BTreeMap::new();
    let mut rows = Vec::with_capacity(wf.requests.len());

    for r in &wf.requests {
        wire_total = wire_total.saturating_add(r.encoded_bytes);
        decoded_total = decoded_total.saturating_add(r.decoded_bytes);
        if matches!(r.source.as_deref(), Some("memory-cache" | "disk-cache")) {
            cached += 1;
        }
        if r.error.is_some() || r.status.is_some_and(|s| s >= 400) {
            failed += 1;
        }
        if let Some(p) = &r.protocol {
            *by_proto.entry(p.clone()).or_default() += 1;
        }
        let mut val = serde_json::to_value(r).unwrap_or(Value::Null);
        val["url"] = json!(scrub_url(&r.url, reveal));
        rows.push(val);
    }

    let protos: Vec<String> = by_proto.iter().map(|(p, n)| format!("{p} ×{n}")).collect();
    let mut text = format!(
        "network · {id} · {} requests · {} wire ({} decoded) · {} cached · {} failed{}{}\n",
        wf.requests.len(),
        fmt::bytes(wire_total as f64),
        fmt::bytes(decoded_total as f64),
        cached,
        failed,
        if protos.is_empty() { String::new() } else { format!(" · {}", protos.join(", ")) },
        if reveal { " · revealed" } else { "" }
    );

    if wf.navigation.is_object() {
        let n = &wf.navigation;
        let ttfb = n["ttfb_ms"].as_f64().unwrap_or(f64::NAN);
        let dcl = n["dom_content_loaded_ms"].as_f64().unwrap_or(f64::NAN);
        let load = n["load_ms"].as_f64().unwrap_or(f64::NAN);
        text += &format!("nav      TTFB {} · DOMContentLoaded {} · load {}\n", fmt::value(ttfb, "ms"), fmt::value(dcl, "ms"), fmt::value(load, "ms"));
    }
    if !wf.paint.is_empty() {
        let pts: Vec<String> = wf.paint.iter().filter_map(|p| Some(format!("{} {}", p["name"].as_str()?, fmt::value(p["start_ms"].as_f64()?, "ms")))).collect();
        if !pts.is_empty() {
            text += &format!("paint    {}\n", pts.join(" · "));
        }
    }
    if !wf.spans.is_empty() {
        let sp: Vec<String> = wf.spans.iter().take(8).filter_map(|s| Some(format!("{} {}", s["name"].as_str()?, fmt::value(s["duration_ms"].as_f64()?, "ms")))).collect();
        if !sp.is_empty() {
            text += &format!("spans    {}\n", sp.join(" · "));
        }
    }

    if !rows.is_empty() {
        text += "      start        dur       ttfb  status  proto     type         wire  url\n";
        for r in rows.iter().take(top) {
            let start = r["start_ms"].as_f64().unwrap_or(0.0);
            let dur = r["duration_ms"].as_f64().map_or_else(|| "—".into(), |d| format!("{d:>6.1} ms"));
            let ttfb = r["ttfb_ms"].as_f64().map_or_else(|| "—".into(), |d| format!("{d:>6.1} ms"));
            let status = match (r["status"].as_i64(), r["error"].as_str()) {
                (Some(code), _) => format!("{code:>6}"),
                (None, Some(_)) => "   ERR".into(),
                _ => "     —".into(),
            };
            let proto = r["protocol"].as_str().or_else(|| r["source"].as_str()).unwrap_or("—");
            let rtype = r["resource_type"].as_str().unwrap_or("Other");
            let wire = fmt::bytes(r["encoded_bytes"].as_f64().unwrap_or(0.0));
            let url = r["url"].as_str().unwrap_or("");
            let short_url: String = if url.chars().count() > 80 { format!("{}…", url.chars().take(79).collect::<String>()) } else { url.to_string() };
            text += &format!("  {start:>6.1} ms  {dur:>9}  {ttfb:>9}  {status}  {proto:<8}  {rtype:<10}  {wire:>7}  {short_url}\n");
        }
        if rows.len() > top {
            text += &format!("  … {} more requests (--top N)\n", rows.len() - top);
        }
    }

    let out_json = json!({
        "recording": id,
        "revealed": reveal,
        "summary": {
            "requests": wf.requests.len(),
            "encoded_bytes": wire_total,
            "decoded_bytes": decoded_total,
            "cached": cached,
            "failed": failed,
            "protocols": by_proto,
        },
        "navigation": wf.navigation,
        "paint": wf.paint,
        "spans": wf.spans,
        "requests": rows,
    });
    Ok(Output::new(text, out_json))
}
