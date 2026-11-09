//! `query <rec> inputs`: each input strobe gave the target in its measured runs, on one line: its run
//! and time in it, the change it made, the frames around it, and its latency or why it has none (and
//! how much later it had to come to count). From the OS's presents (`frames.json` with
//! `presents.json.gz`: a native app, a headed page where the OS tells presents), else from a Chromium
//! trace's own input latencies and frame reports.

use super::super::{Output, fmt};
use crate::metrics::Timed;
use crate::model::{Error, Result};
use crate::probe::present::Presents;
use crate::probe::present::frames::{self, BusyWhy, Change, Edge, Outcome, Sent};
use crate::store;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

/// The OS's presents a recording kept (`presents.json.gz`).
pub(super) fn presents(dir: &Path) -> Result<Presents> {
    let path = dir.join("presents.json.gz");
    let mut text = String::new();
    std::io::Read::read_to_string(
        &mut flate2::read::GzDecoder::new(std::fs::File::open(&path).map_err(|_| Error::usage("this recording has no OS present events (no access, or a block without a window)"))?),
        &mut text,
    )?;
    Ok(serde_json::from_str(&text)?)
}

/// The refresh period (ms) `frames.json` was measured at: the block's own display rate when it kept
/// one (macOS and X11 sessions have no vertical blanks), else the presents' blanks.
pub(super) fn refresh_ms(f: &Value, p: &Presents) -> Option<f64> {
    f["display_hz"].as_f64().filter(|hz| *hz > 1.0).map(|hz| 1000.0 / hz).or_else(|| frames::refresh_ms(&p.vsyncs, p.t0_ns, p.t1_ns, None))
}

/// One run of `frames.json`: its span without its untimed stretches (none in a recording from before
/// them), the inputs a latency is measured from (a click's release with its press, a key, a scroll's
/// first step, those in untimed stretches left out), and the target's changes as their earliest and
/// latest times (a recording from before kept one time, the latest).
pub(super) fn run_of(r: &Value) -> (Timed, Vec<Sent>, Vec<Change>) {
    let timed = Timed {
        t0: r["t0_ns"].as_u64().unwrap_or(0),
        t1: r["t1_ns"].as_u64().unwrap_or(0),
        untimed: r["untimed"].as_array().into_iter().flatten().filter_map(|g| Some((g[0].as_u64()?, g[1].as_u64()?))).collect(),
    };
    let sent = r["inputs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|i| i["kind"] == "discrete" || (i["kind"] == "scroll" && i["first"] == true))
        .map(|i| Sent {
            t_ns: i["t_ns"].as_u64().unwrap_or(0),
            scroll: i["kind"] == "scroll",
            press: i["press_ns"].as_u64(),
            acted: i["acted"].as_bool(),
            burst: i["burst"] == true,
            pixel: i["pixel"][0].as_u64().zip(i["pixel"][1].as_u64()),
        })
        .filter(|s| !timed.untimed_at(s.t_ns) && !s.press.is_some_and(|p| timed.untimed_at(p)))
        .collect();
    let changes = r["changes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| match (c.as_u64(), c[0].as_u64(), c[1].as_u64()) {
            (Some(t), _, _) => Some(frames::notified(t, false)),
            (None, Some(lo), Some(hi)) => Some(Change(lo, hi, c[2].as_bool().unwrap_or(false))),
            _ => None,
        })
        .collect();
    (timed, sent, changes)
}

/// The lines an inputs view shows without `--top`.
const SHOWN: usize = 100;

/// `top`: the lines to show (`--top`; all of them are in `--json`), `SHOWN` when not asked.
pub fn inputs(dir: &Path, meta: &Value, top: Option<usize>, only: Option<&[String]>) -> Result<Output> {
    let top = top.unwrap_or(SHOWN);
    if dir.join("frames.json").is_file() && dir.join("presents.json.gz").is_file() {
        return from_presents(dir, top, only);
    }
    let trace = dir.join("trace.pftrace");
    if trace.is_file() && meta["engine"].as_str().is_none_or(|e| e == "chromium") {
        return from_trace(&trace, meta, top, only);
    }
    Err(Error::usage(
        "this recording has no inputs to show: they come from a block's OS presents (a native app's, or a headed page's where the OS tells them: frames.json and presents.json.gz) or a Chromium page's trace",
    ))
}

/// The first `top` of `lines`, and a line for the rest.
fn first_lines(lines: &[String], top: usize) -> String {
    let mut text: String = lines.iter().take(top).map(String::as_str).collect();
    if lines.len() > top {
        text += &format!("  … {} more (--top N shows more; --json lists every one)\n", lines.len() - top);
    }
    text
}

/// The runs an inputs view read, for its header: `in 5 measured runs`, and their range when the first
/// is not `run:0` (the runs before it were warmup, or another case's with `--case`) or some between
/// were not measured (a batch's cases' warmups between their runs).
fn runs_text<'a>(keys: impl Iterator<Item = &'a str>, case: bool) -> String {
    let mut n: Vec<u64> = keys.filter_map(|k| k.strip_prefix("run:")?.parse().ok()).collect();
    n.sort_unstable();
    let count = n.len();
    let plural = if count == 1 { "" } else { "s" };
    match (n.first(), n.last()) {
        (Some(&a), Some(&b)) if a > 0 || (b - a + 1) as usize != count => {
            let range = if a == b { format!("run:{a}") } else { format!("run:{a}..run:{b}") };
            let left = if case {
                "warmup and other cases' runs left out".to_string()
            } else if (b - a + 1) as usize != count {
                "non-measured runs left out".to_string()
            } else {
                format!("warmup before run:{a} left out")
            };
            format!("in {count} measured run{plural} ({range}; {left})")
        }
        _ => format!("in {count} measured run{plural}"),
    }
}

/// What an input was, for its line.
fn kind(s: &Sent) -> &'static str {
    match (s.scroll, s.press.is_some(), s.pixel.is_some()) {
        (true, _, _) => "scroll",
        (false, true, _) => "click",
        (false, false, true) => "input (pixels)",
        (false, false, false) => "input",
    }
}

fn why_key(w: &BusyWhy) -> &'static str {
    match w {
        BusyWhy::MidDrawing { .. } => "mid_drawing",
        BusyWhy::Behind { .. } => "behind",
        BusyWhy::AmbiguousPress => "ambiguous_press",
        BusyWhy::MultiFramePixel { .. } => "multi_frame_pixel",
    }
}

fn from_presents(dir: &Path, top: usize, only: Option<&[String]>) -> Result<Output> {
    let f = store::read_json(&dir.join("frames.json"))?;
    let p = presents(dir)?;
    let pids: Vec<u32> = f["pids"].as_array().into_iter().flatten().filter_map(|p| p.as_u64().map(|p| p as u32)).collect();
    let refresh = refresh_ms(&f, &p);
    let quiet = frames::quiet_ns(refresh.unwrap_or(1000.0 / 60.0));
    let quiet_ms = quiet as f64 / 1e6;
    // The target's frames that reached the screen, by screen time: what each input's line shows around it.
    let mut shown: Vec<&crate::probe::present::Present> = p.presents.iter().filter(|x| pids.contains(&x.pid) && x.state == "displayed").collect();
    shown.sort_by_key(|x| x.screen_ns);
    let mut rows: Vec<Value> = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    let (mut counts, mut whys): (BTreeMap<&str, usize>, BTreeMap<&str, usize>) = (BTreeMap::new(), BTreeMap::new());
    let mut needed: Vec<f64> = Vec::new();
    // Runs in time order (their keys sort as text: run:10 before run:2).
    let mut ordered: Vec<(&String, &Value)> = f["runs"].as_object().into_iter().flatten().filter(|(k, _)| only.is_none_or(|o| o.contains(k))).collect();
    ordered.sort_by_key(|(_, r)| r["t0_ns"].as_u64().unwrap_or(0));
    let runs = ordered.len();
    let runs_said = runs_text(ordered.iter().map(|(k, _)| k.as_str()), only.is_some());
    for (run, r) in ordered {
        let (timed, sent, changes) = run_of(r);
        let got = frames::reactions(&sent, &changes, &p.presents, &pids, quiet);
        let outcome: BTreeMap<u64, Outcome> = got.each.iter().copied().collect();
        let mut sent = sent;
        sent.sort_by_key(|s| s.t_ns);
        for (k, s) in sent.iter().enumerate() {
            let from = s.press.unwrap_or(s.t_ns);
            let next = sent.get(k + 1).map_or(u64::MAX, |n| n.press.unwrap_or(n.t_ns));
            let at_ms = (s.t_ns as f64 - timed.t0 as f64) / 1e6;
            // What proved it changed: the first capture that had its pixels, or the first change after it
            // and before the next input that is no press feedback, by when it was known to have happened.
            let change = match s.pixel {
                Some((_, had)) => Some(("pixels", (had as f64 - s.t_ns as f64) / 1e6)),
                // One that may have happened before the next input (its earliest, when known) is this one's.
                None => changes
                    .iter()
                    .filter(|c| c.1 > from && (if c.0 == 0 { c.1 } else { c.0 }) <= (from + frames::HORIZON_NS).min(next) && !c.2)
                    .map(|c| c.1)
                    .min()
                    .map(|c| ("change", (c as f64 - s.t_ns as f64) / 1e6)),
            };
            // The last frame on the screen before it and the next three, as their screen time from it;
            // one shown after it but begun before its first edge is marked (it cannot hold its reaction).
            let at = shown.partition_point(|x| x.screen_ns <= s.t_ns);
            let around: Vec<(f64, bool)> = shown[at.saturating_sub(1)..(at + 3).min(shown.len())]
                .iter()
                .filter(|x| x.screen_ns + frames::HORIZON_NS > s.t_ns && x.screen_ns < s.t_ns + frames::HORIZON_NS)
                .map(|x| ((x.screen_ns as f64 - s.t_ns as f64) / 1e6, x.screen_ns > s.t_ns && x.start_ns <= from))
                .collect();
            let o = if s.burst { None } else { outcome.get(&s.t_ns).copied() };
            let mut row = json!({
                "run": run, "t_ns": s.t_ns, "at_ms": at_ms, "kind": kind(s), "press_ms": s.press.map(|p| (s.t_ns as f64 - p as f64) / 1e6),
                "change_ms": change.map(|c| c.1), "proved_by": change.map(|c| c.0),
                "frames": around.iter().map(|(ms, before)| json!({ "screen_ms": ms, "begun_before": before })).collect::<Vec<_>>(),
            });
            let said = match o {
                None => {
                    *counts.entry("burst").or_default() += 1;
                    row["outcome"] = json!("burst");
                    "a key typed in a burst: no one's sample".to_string()
                }
                Some(Outcome::Timed(edge, ms)) => {
                    *counts.entry("timed").or_default() += 1;
                    row["outcome"] = json!("timed");
                    row["latency_ms"] = json!(ms);
                    row["edge"] = json!(edge);
                    format!("latency {}{}", fmt::value(ms, "ms"), if edge == Edge::Press { " from its press" } else { "" })
                }
                Some(Outcome::Scroll(ms)) => {
                    *counts.entry("timed").or_default() += 1;
                    row["outcome"] = json!("scroll");
                    row["latency_ms"] = json!(ms);
                    format!("scroll latency {}", fmt::value(ms, "ms"))
                }
                Some(Outcome::Unchanged) => {
                    *counts.entry("unchanged").or_default() += 1;
                    row["outcome"] = json!("unchanged");
                    "changed nothing in a second: no sample".to_string()
                }
                Some(Outcome::Unshown) => {
                    *counts.entry("unshown").or_default() += 1;
                    row["outcome"] = json!("unshown");
                    "no frame showed its change within a second".to_string()
                }
                Some(Outcome::Busy(why)) => {
                    *counts.entry("busy").or_default() += 1;
                    *whys.entry(why_key(&why)).or_default() += 1;
                    let need = why.needed_ms(quiet_ms);
                    needed.extend(need);
                    row["outcome"] = json!("busy");
                    row["busy"] = json!(why);
                    row["needed_ms"] = json!(need);
                    if let BusyWhy::Behind { prev_input, .. } = why {
                        row["busy"]["prev_input_at_ms"] = json!((prev_input as f64 - timed.t0 as f64) / 1e6);
                    }
                    format!("busy, {}{}", why.text(), need.map(|n| format!(" · needed +{n:.1} ms later")).unwrap_or_default())
                }
            };
            let frames_text = if around.is_empty() {
                "no frame within a second".to_string()
            } else {
                around.iter().map(|(ms, before)| format!("{ms:+.1}{}", if *before { "^" } else { "" })).collect::<Vec<_>>().join(" ")
            };
            lines.push(format!(
                "  {run:<8} {:>10}  {:<14} {:<15} frames {frames_text:<28} {said}\n",
                format!("+{at_ms:.1} ms"),
                kind(s),
                change.map(|(by, ms)| format!("{by} {ms:+.1} ms")).unwrap_or_else(|| "no change".into()),
            ));
            rows.push(row);
        }
    }
    if runs == 0 {
        return Err(Error::usage("no measured run of this recording matched (--case picks one case's runs)"));
    }
    let total = rows.len();
    let mut text = format!(
        "inputs · {total} measured inputs {runs_said} · the app rests {} before an input counts (two refreshes{}) · {}\n",
        fmt::value(quiet_ms, "ms"),
        refresh.map(|r| format!(" of {}", fmt::value(r, "ms"))).unwrap_or_else(|| ", refresh unknown: 60 Hz assumed".into()),
        counts.iter().map(|(k, n)| format!("{n} {k}")).collect::<Vec<_>>().join(", ")
    );
    text += "  run      at (in run)  input          proved by       frames (screen time from the input; ^ begun before it)  outcome\n";
    text += &first_lines(&lines, top);
    needed.sort_by(f64::total_cmp);
    if !needed.is_empty() {
        text += &format!(
            "busy: {} · they would have counted coming later by {} (median) to {} (max): a wait of about that before such an input lets the app rest\n",
            whys.iter().map(|(k, n)| format!("{n} {}", k.replace('_', "-"))).collect::<Vec<_>>().join(", "),
            fmt::value(crate::metrics::percentile(&needed, 0.5), "ms"),
            fmt::value(*needed.last().unwrap_or(&0.0), "ms")
        );
    } else if !whys.is_empty() {
        text += &format!("busy: {}\n", whys.iter().map(|(k, n)| format!("{n} {}", k.replace('_', "-"))).collect::<Vec<_>>().join(", "));
    }
    text += "an input counts when the app changed after it, was at rest when it came, and every earlier input's reaction was on the screen (SPEC §7.4)\n";
    let json = json!({
        "source": "presents", "refresh_ms": refresh, "quiet_ms": quiet_ms, "runs": runs, "inputs": rows, "outcomes": counts, "busy": whys,
        "needed_ms": if needed.is_empty() { Value::Null } else { json!({ "p50": crate::metrics::percentile(&needed, 0.5), "max": needed.last() }) },
    });
    Ok(Output::new(text, json))
}

/// A Chromium page's inputs as its trace saw them: each input event's latency (`EventLatency`, from
/// the event to the frame that presented it, as Chromium measures `interaction.*`) and the frames the
/// page presented around it (`PipelineReporter`), per measured run, on the trace's clock.
fn from_trace(trace: &Path, meta: &Value, top: usize, only: Option<&[String]>) -> Result<Output> {
    let token = meta["token"].as_str().unwrap_or("");
    if token.is_empty() {
        return Err(Error::usage("this recording's trace has no run marks of strobe's, so no run is known in it"));
    }
    let measured: Option<Vec<String>> = only.map(<[String]>::to_vec).or_else(|| super::rec::measured_keys(meta));
    let sql = format!(
        "{}
select 'win' as kind, k, t0 as a, t1 as b, null as c from _w
union all
select 'lat', w.k, s.ts, s.dur, extract_arg(s.arg_set_id, 'event_latency.event_type')
  from slice s join track tr on s.track_id = tr.id
  join _wp w on extract_arg(tr.dimension_arg_set_id, 'upid') = w.upid and s.ts >= w.t0 and s.ts < w.t1
  where s.name = 'EventLatency' and s.dur > 0 and not exists (select 1 from _u u where s.ts >= u.t0 and s.ts < u.t1)
union all
select 'frame', w.k, s.ts, s.dur, extract_arg(s.arg_set_id, 'frame_reporter.state')
  from slice s join track tr on s.track_id = tr.id
  join _wp w on extract_arg(tr.dimension_arg_set_id, 'upid') = w.upid and s.ts >= w.t0 - 1000000000 and s.ts < w.t1 + 1000000000
  where s.name = 'PipelineReporter';",
        super::rec::windows_sql(token, measured.as_deref())
    );
    let table = crate::engine::query(trace, &sql)?;
    let num = |c: &Option<String>| c.as_deref().and_then(|s| s.parse::<f64>().ok());
    let mut wins: BTreeMap<String, f64> = BTreeMap::new();
    let mut lats: Vec<(String, f64, f64, String)> = Vec::new();
    let mut presented: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for row in &table.rows {
        let k = row[1].clone().unwrap_or_default().rsplit(':').take(2).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(":");
        match (row[0].as_deref(), num(&row[2]), num(&row[3])) {
            (Some("win"), Some(t0), _) => {
                wins.insert(k, t0);
            }
            (Some("lat"), Some(ts), Some(dur)) => lats.push((k, ts, dur, row[4].clone().unwrap_or_default())),
            // A frame that put something on the screen: at its report's end.
            (Some("frame"), Some(ts), Some(dur)) if row[4].as_deref().is_some_and(|s| s.contains("PRESENTED")) => presented.entry(k).or_default().push(ts + dur),
            _ => {}
        }
    }
    if wins.is_empty() {
        return Err(Error::usage("no measured run window in this recording's trace (--case picks one case's runs)"));
    }
    for v in presented.values_mut() {
        v.sort_by(f64::total_cmp);
        v.dedup();
    }
    // Runs in time order; in each, the events in time order.
    lats.sort_by(|a, b| wins.get(&a.0).unwrap_or(&a.1).total_cmp(wins.get(&b.0).unwrap_or(&b.1)).then(a.1.total_cmp(&b.1)));
    // Only the events `interaction.*` and `scroll.latency.*` read (a key, a mouse button, a touch's
    // edges, a scroll's updates and wheel turns): pointer moves, characters and the rest are left out,
    // counted.
    let read = |ty: &str| {
        let ty = ty.trim_start_matches("EVENT_TYPE_");
        COUNTED.contains(&ty) || ty.contains("SCROLL_UPDATE") || ty == "MOUSE_WHEEL"
    };
    let other = lats.iter().filter(|l| !read(&l.3)).count();
    let mut rows = Vec::new();
    let mut lines = Vec::new();
    for (k, ts, dur, kind) in lats.iter().filter(|l| read(&l.3)) {
        let t0 = wins.get(k).copied().unwrap_or(*ts);
        let frames: &[f64] = presented.get(k).map(Vec::as_slice).unwrap_or_default();
        let at = frames.partition_point(|f| *f <= *ts);
        let around: Vec<f64> = frames[at.saturating_sub(1)..(at + 3).min(frames.len())].iter().map(|f| (f - ts) / 1e6).filter(|ms| ms.abs() < 1000.0).collect();
        let kind = kind.trim_start_matches("EVENT_TYPE_").to_ascii_lowercase();
        lines.push(format!(
            "  {k:<8} {:>10}  {kind:<22} latency {:<10} frames {}\n",
            format!("+{:.1} ms", (ts - t0) / 1e6),
            fmt::value(dur / 1e6, "ms"),
            if around.is_empty() { "none presented within a second".to_string() } else { around.iter().map(|ms| format!("{ms:+.1}")).collect::<Vec<_>>().join(" ") }
        ));
        rows.push(json!({ "run": k, "at_ms": (ts - t0) / 1e6, "kind": kind, "latency_ms": dur / 1e6, "frames": around.iter().map(|ms| json!({ "screen_ms": ms })).collect::<Vec<_>>() }));
    }
    let total = rows.len();
    let mut text = format!(
        "inputs · {total} input events {}, as Chromium's trace saw them (each event's latency to the frame that presented it; frames: presented, from the event)\n",
        runs_text(wins.keys().map(String::as_str), only.is_some())
    );
    text += &first_lines(&lines, top);
    if total == 0 {
        text += "  the trace holds no key, button, touch or scroll input in the measured runs' timed stretches\n";
    }
    if other > 0 {
        text += &format!("  {other} other input events (pointer moves, characters, a gesture's begin and end) left out: no interaction.* or scroll.latency.* sample comes from them\n");
    }
    text += "Chromium decides which event counts for interaction.* (its own EventLatency); strobe's at-rest rule applies to OS presents only\n";
    Ok(Output::new(text, json!({ "source": "trace", "runs": wins.len(), "inputs": rows, "other_events": other })))
}

/// The discrete `EventLatency` types a page's `interaction.*` reads (`metrics`: a key, a mouse button,
/// a touch's edges); `scroll.latency.*` reads scroll updates and wheel turns.
const COUNTED: [&str; 8] = ["MOUSE_PRESSED", "MOUSE_RELEASED", "KEY_PRESSED", "KEY_RELEASED", "TOUCH_PRESSED", "GESTURE_TAP_DOWN", "TOUCH_RELEASED", "GESTURE_TAP"];
