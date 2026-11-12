//! `query <exp> insights` (SPEC §11.3): per series, what the experiment found (a comparison's verdict,
//! a check's, a `--blocks` run's or a leak run's status), the suspects its kept recordings show,
//! ranked, the warnings that bear on that result, and the commands that go further. Facts and
//! pointers only: each line is a number the experiment holds, a rule's hit over a recording it kept,
//! or a command that shows more.

use super::{Output, Rec, fmt, hits_text, insights, mark_sides, series_insights};
use crate::model::{Result, VerdictKind};
use crate::stats;
use serde_json::{Value, json};

pub fn run(r: &Value) -> Result<Output> {
    let id = r["id"].as_str().unwrap_or("");
    if r["kind"] == "bisect" {
        return Ok(bisect(r));
    }
    let series: Vec<&Value> = r["series"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
    let cmd = Command::of(r, series.len());
    let mut text = format!("insights · {id} · {}\n", what(r, &series));
    // The experiment's own warnings bear on every series: said once.
    let shared = warnings(&r["warnings"]);
    if !shared.is_empty() {
        text += &format!("warnings (the experiment's)\n{}", warnings_text(&shared));
    }
    if let Some(st) = r["stopped"].as_object() {
        text += &format!("stopped  {}\n", st.get("why").and_then(Value::as_str).unwrap_or("the bench stopped before every series ran"));
    }
    let mut out = Vec::new();
    for s in series {
        let sec = section(r, s, &cmd)?;
        text += &sec.text();
        out.push(sec.json());
    }
    Ok(Output::new(text, json!({ "experiment": id, "kind": r["kind"], "comparison": r["comparison"], "warnings": shared, "series": out })))
}

/// One series' insights.
#[derive(Default)]
struct Section {
    label: String,
    kind: String,
    /// What it found: a verdict, a check's state, the blocks' median, each side's growth.
    result: Vec<String>,
    /// Where the suspects come from, and each source's hits, ranked.
    suspects: Vec<(String, Vec<Value>)>,
    /// Why there are no suspects, when there are none.
    none: Option<String>,
    warnings: Vec<Value>,
    /// (why, command): what to run next and what it shows.
    next: Vec<(String, String)>,
    /// A comparison's movers, as `series_insights` gives them.
    extra: Value,
}

impl Section {
    fn text(&self) -> String {
        let mut t = format!("\n{}\n", self.label);
        for (i, l) in self.result.iter().enumerate() {
            t += &format!("{}{l}\n", if i == 0 { "result    " } else { "          " });
        }
        for (source, hits) in &self.suspects {
            t += &format!("suspects  {source}\n");
            if hits.is_empty() {
                t += "  nothing stood out: no rule crossed its threshold\n";
            }
            t += &hits_text(hits).lines().map(|l| format!("  {l}\n")).collect::<String>();
        }
        if let Some(n) = &self.none {
            t += &format!("suspects  none: {n}\n");
        }
        if !self.warnings.is_empty() {
            t += &format!("warnings\n{}", warnings_text(&self.warnings));
        }
        if !self.next.is_empty() {
            t += "next\n";
            for (why, c) in &self.next {
                if !why.is_empty() {
                    t += &format!("  {why}\n");
                }
                if !c.is_empty() {
                    t += &format!("    {c}\n");
                }
            }
        }
        t
    }

    fn json(&self) -> Value {
        let mut j = json!({
            "series": self.label, "kind": self.kind, "result": self.result,
            "suspects": self.suspects.iter().map(|(s, h)| json!({ "source": s, "hits": h })).collect::<Vec<_>>(),
            "no_suspects": self.none, "warnings": self.warnings,
            "next": self.next.iter().map(|(w, c)| json!({ "why": w, "command": (!c.is_empty()).then_some(c) })).collect::<Vec<_>>(),
        });
        if let Some(o) = self.extra.as_object() {
            for (k, v) in o {
                j[k] = v.clone();
            }
        }
        j
    }
}

/// The header's words for what the experiment ran.
fn what(r: &Value, series: &[&Value]) -> String {
    let label = |i: usize| r["variants"][i]["label"].as_str().or(r["variants"][i]["name"].as_str()).unwrap_or("?").to_string();
    let n = r["variants"].as_array().map_or(0, Vec::len);
    let only = |k: &str| series.iter().any(|s| s["kind"] == k) && series.iter().all(|s| s["kind"] == k || s["kind"] == "refused" || s["not_run"] == true);
    let count = format!("{} series", series.len());
    let what = match r["kind"].as_str() {
        Some("check") if n >= 2 => format!("a check of base {} and head {}", label(0), label(n - 1)),
        Some("check") if n == 1 => format!("a check of {}", label(0)),
        Some("check") => "a check".to_string(),
        Some("pixels") => "a pixels run".to_string(),
        _ if only("blocks") => format!("--blocks of {}", label(n.saturating_sub(1))),
        _ if only("leak") && n >= 2 => format!("a leak run of base {} and head {}", label(0), label(1)),
        _ if only("leak") => format!("a leak run of {}", label(0)),
        _ if r["comparison"] == "implementations" => format!("{} against {}", r["variants"][1]["name"].as_str().unwrap_or("head"), r["variants"][0]["name"].as_str().unwrap_or("base")),
        _ if r["plan"]["aa"] == true => format!("an A/A of {}", label(1)),
        _ => format!("base {} → head {}", label(0), label(1)),
    };
    format!("{what} · {count}")
}

fn section(r: &Value, s: &Value, cmd: &Command) -> Result<Section> {
    let mut sec = Section { label: s["label"].as_str().unwrap_or("").to_string(), kind: s["kind"].as_str().unwrap_or("").to_string(), ..Default::default() };
    sec.warnings = warnings(&s["warnings"]);
    match s["kind"].as_str() {
        _ if s["not_run"] == true => {
            sec.result.push("not run: the bench stopped before this series".into());
            if let Some(id) = r["id"].as_str().filter(|id| crate::store::resolve(id).is_ok_and(|d| d.join("state.json").is_file())) {
                sec.next.push(("go on with the series not run".into(), format!("strobe bench --resume {id}")));
            }
        }
        Some("refused" | "gap") => {
            let f = if s["refusal"].is_object() { &s["refusal"] } else { &s["primary"]["refusal"] };
            let what = if s["kind"] == "gap" { "not compared" } else { "not run" };
            sec.result.push(format!("{what}: {} ({})", f["summary"].as_str().or(s["error"].as_str()).unwrap_or("refused"), f["validator"].as_str().unwrap_or("refused")));
            if let Some(fix) = f["fix"].as_str() {
                sec.next.push((format!("the refusal's fix: {fix}"), String::new()));
            }
        }
        Some("check") => check(r, s, cmd, &mut sec)?,
        Some("blocks") => blocks(r, s, cmd, &mut sec)?,
        Some("leak") => leak(r, s, cmd, &mut sec)?,
        Some("pixels") => pixels(r, s, &mut sec),
        _ => comparison(r, s, cmd, &mut sec)?,
    }
    Ok(sec)
}

/// The warnings that bear on a result: every warn-level one, and why an attribution block is missing.
fn warnings(list: &Value) -> Vec<Value> {
    list.as_array().into_iter().flatten().filter(|w| w["level"] == "warn" || w["id"] == "Attribution").cloned().collect()
}

fn warnings_text(list: &[Value]) -> String {
    list.iter()
        .map(|w| {
            let text = w["text"].as_str().unwrap_or("");
            let text = if text.chars().count() > 240 { format!("{}…", text.chars().take(239).collect::<String>()) } else { text.to_string() };
            format!("  {} {:<20}{text}\n", if w["level"] == "warn" { "!" } else { "i" }, w["id"].as_str().unwrap_or(""))
        })
        .collect()
}

/// A recording's rule hits, or none when it cannot be read (pruned, moved).
fn hits_of(id: &str) -> Option<Vec<Value>> {
    let rec = Rec::resolve(id).ok()?;
    insights(&rec).ok().map(|o| o.json["insights"].as_array().cloned().unwrap_or_default())
}

/// Warn-level hits first, each level in the rules' own order.
fn ranked(mut hits: Vec<Value>) -> Vec<Value> {
    hits.sort_by_key(|h| h["level"] != "warn");
    hits
}

/// A comparison's primary as one line: `★ cpu.work more: 244 ms → 256 ms, +4.6% [+4.1% … +5.8%] over 8 pairs`.
fn verdict_line(c: &Value, aa: bool) -> String {
    let m = c["metric"].as_str().unwrap_or("");
    let kind = stats::kind_of(c);
    let verdict = c["verdict"].as_str().unwrap_or("");
    if kind == VerdictKind::Refused {
        return format!("{m} {verdict}");
    }
    let unit = crate::model::metric(m).map_or("", |d| d.unit);
    let n = |k: &str| c[k].as_f64().filter(|x| x.is_finite());
    let mut line = format!("{m} {verdict}");
    if let (Some(b), Some(h)) = (n("base"), n("head")) {
        line += &format!(": {} → {}", fmt::value(b, unit), fmt::value(h, unit));
    }
    if let Some(est) = n("estimate") {
        let abs = if unit == "%" { "pp" } else { unit };
        let change = match (c["relative"] == true, n("lo"), n("hi")) {
            _ if c["exact"] == true => format!("{} (exact)", if c["relative"] == true { fmt::pct(est) } else { fmt::signed(est, abs) }),
            (true, Some(lo), Some(hi)) => {
                let (e, l, h) = fmt::pct_ci(est, lo, hi);
                format!("{e} [{l} … {h}]")
            }
            (false, Some(lo), Some(hi)) => format!("{} [{} … {}]", fmt::signed(est, abs), fmt::signed(lo, abs), fmt::signed(hi, abs)),
            (true, _, _) => fmt::pct(est),
            (false, _, _) => fmt::signed(est, abs),
        };
        line += &format!(", {change}");
    }
    if let Some(p) = c["n"].as_u64() {
        line += &format!(" over {p} pair{}", if p == 1 { "" } else { "s" });
    }
    if aa && matches!(kind, VerdictKind::Better | VerdictKind::Worse) {
        line += " ← a false positive: both sides ran the same build (A/A)";
    }
    line
}

/// Why a comparison series kept no attribution recordings, from what the result says.
fn why_none(r: &Value, s: &Value, differs: bool) -> (String, bool) {
    let kinds = [&s["target_kind"], &s["target_kinds"]["base"], &s["target_kinds"]["head"]];
    if kinds.iter().any(|k| *k == "cli") {
        return ("a command ends inside each run: there is nothing to profile it with".into(), false);
    }
    if kinds.iter().any(|k| *k == "tty") {
        return ("a terminal program has no profiler in strobe yet (strobe record pid:<n> --probes deep profiles a running one)".into(), false);
    }
    let label = s["label"].as_str().unwrap_or("");
    let said = s["warnings"].as_array().into_iter().flatten().chain(r["warnings"].as_array().into_iter().flatten()).find(|w| {
        let t = w["text"].as_str().unwrap_or("");
        (w["id"] == "Attribution" || w["id"] == "MaxTime") && t.contains("attribution") && (t.starts_with(label) || s["warnings"].as_array().is_some_and(|a| a.contains(w)))
    });
    if let Some(w) = said {
        let t = w["text"].as_str().unwrap_or("");
        let profilable = !t.contains("no profilers") && !t.contains("has no profile");
        return (format!("{} {t}", w["id"].as_str().unwrap_or("")), profilable);
    }
    if s["halt"].is_object() || s["halt"].is_string() {
        return ("the bench stopped before its attribution blocks".into(), true);
    }
    let flags = &r["args"]["flags"];
    if flags["why"] == "never" || (flags["why"].is_null() && flags["probes"] == "timing") {
        return (format!("{} skipped them", if flags["why"] == "never" { "--why never" } else { "--probes timing" }), true);
    }
    if !differs {
        return ("nothing to explain: --why auto profiles a series only when its primary moved (decided, its interval clear of 0)".into(), true);
    }
    ("no attribution recording was kept".into(), true)
}

fn comparison(r: &Value, s: &Value, cmd: &Command, sec: &mut Section) -> Result<()> {
    sec.kind = "comparison".into();
    let p = &s["primary"];
    let kind = stats::kind_of(p);
    let aa = r["plan"]["aa"] == true;
    sec.result.push(format!("★ {}", verdict_line(p, aa)));
    if let Some(k) = s["primary_pairs"].as_u64().filter(|k| Some(*k) < s["pairs"].as_u64()) {
        sec.result.push(format!("the primary's verdict is its first {k} pairs' (the look that decided it); {} pairs ran", s["pairs"]));
    }
    // The secondaries that decided a direction.
    for c in s["secondaries"].as_array().into_iter().flatten().filter(|c| matches!(stats::kind_of(c), VerdictKind::Better | VerdictKind::Worse)) {
        sec.result.push(format!("· {}", verdict_line(c, aa)));
    }
    let (lo, hi) = (p["lo"].as_f64().unwrap_or(0.0), p["hi"].as_f64().unwrap_or(0.0));
    let differs = p["decided"] == true && (lo > 0.0 || hi < 0.0);
    // The way the primary moved, for a lower-is-better time or count: movers that went the same way
    // explain it, the others moved against it.
    let way = match kind {
        VerdictKind::Better | VerdictKind::Worse if p["better"] != "higher" && !aa => p["estimate"].as_f64().unwrap_or(0.0).signum(),
        _ => 0.0,
    };
    let attr = &s["attribution"];
    let (b, h) = (attr["base"].as_str(), attr["head"].as_str());
    let id = r["id"].as_str().unwrap_or("");
    if r["comparison"] == "implementations" {
        // Two implementations share no code: each side's own hits, under its name.
        for (i, role) in ["base", "head"].into_iter().enumerate() {
            let Some(rec) = attr[role].as_str() else { continue };
            let name = r["variants"][i]["name"].as_str().unwrap_or(role);
            if let Some(hits) = hits_of(rec) {
                sec.suspects.push((format!("{name}'s attribution block {rec} (on its own: two implementations share no code)"), ranked(hits)));
            }
        }
        if sec.suspects.is_empty() {
            sec.none = Some(format!("no attribution blocks: {}", why_none(r, s, differs).0));
        } else {
            sec.next.push(("both sides side by side".into(), format!("strobe report experiment {id} --open")));
        }
    } else if let (Some(b), Some(h)) = (b, h) {
        let (base, head) = (Rec::resolve(b)?, Rec::resolve(h)?);
        let o = series_insights(&base, &head, r["plan"]["runs_per_block"].as_u64())?;
        let movers = o.json["movers"].as_array().cloned().unwrap_or_default();
        let along = |m: &Value| way == 0.0 || (m["head_per_run"].as_f64().unwrap_or(0.0) - m["base_per_run"].as_f64().unwrap_or(0.0)).signum() == way;
        let as_hit = |m: &Value, against: bool| json!({ "rule": "mover", "level": "info", "title": format!("{}{}", m["title"].as_str().unwrap_or(""), if against { " (against the primary's change)" } else { "" }), "evidence": m, "handles": m["handles"] });
        let heads = o.json["head"].as_array().cloned().unwrap_or_default();
        let (only, both): (Vec<Value>, Vec<Value>) = heads.into_iter().partition(|h| h["sides"] == "head only");
        let mut hits: Vec<Value> = movers.iter().filter(|m| along(m)).map(|m| as_hit(m, false)).collect();
        hits.extend(ranked(only));
        hits.extend(movers.iter().filter(|m| !along(m)).map(|m| as_hit(m, true)));
        hits.extend(ranked(both));
        hits.extend(o.json["base_only"].as_array().into_iter().flatten().map(|x| {
            let mut x = x.clone();
            x["title"] = json!(format!("base only: {}", x["title"].as_str().unwrap_or("")));
            x["level"] = json!("info");
            x
        }));
        sec.suspects.push((format!("the attribution blocks, one profiled launch per side: base {b} · head {h}"), hits));
        sec.extra = json!({ "movers": o.json["movers"], "head": o.json["head"], "base_only": o.json["base_only"] });
        let views: Vec<&str> = ["hotspots", "layout", "alloc"].into_iter().filter(|v| movers.iter().any(|m| m["handles"][0].as_str().is_some_and(|c| c.ends_with(&format!(" {v}"))))).collect();
        for v in if views.is_empty() { vec!["hotspots"] } else { views } {
            sec.next.push((format!("what moved, by function ({v}, per run, with the stacks it moved in)"), format!("strobe query {b}..{h} {v}")));
        }
        sec.next.push(("both sides and the diff flame".into(), format!("strobe report experiment {id} --open")));
    } else {
        let (why, profilable) = why_none(r, s, differs);
        // Without a profile, the timed blocks' kept recordings still hold their counters and trace.
        let timed: Vec<(&str, &str)> = s["timing_recordings"].as_array().into_iter().flatten().filter_map(|t| Some((t["variant"].as_str().unwrap_or("head"), t["recording"].as_str()?))).collect();
        let pick = |role: &str| timed.iter().find(|t| t.0 == role).map(|t| t.1);
        if let Some(hr) = pick("head")
            && let Some(mut hits) = hits_of(hr)
        {
            let base_only = pick("base").and_then(hits_of).map(|bh| mark_sides(&mut hits, bh)).unwrap_or_default();
            hits = ranked(hits);
            hits.extend(base_only.into_iter().map(|mut x| {
                x["title"] = json!(format!("base only: {}", x["title"].as_str().unwrap_or("")));
                x
            }));
            sec.suspects.push((format!("head's timed block recording {hr} (no profiler: its counters and trace; no attribution blocks: {why})"), hits));
        } else {
            sec.none = Some(format!("no attribution blocks: {why}"));
        }
        if differs && profilable && !aa {
            let case = s["case"].as_str().unwrap_or("");
            sec.next.push((
                format!("profile {case} on each side (one attribution block each: CPU and allocation stacks, forced layouts, GC), then diff them with strobe query <base rec>..<head rec> hotspots"),
                String::new(),
            ));
            for role in ["base", "head"] {
                if let Some(c) = cmd.record(s, role) {
                    sec.next.push((String::new(), c));
                }
            }
        }
    }
    match kind {
        VerdictKind::Refused => {
            if let Some(fix) = p["refusal"]["fix"].as_str() {
                sec.next.push((format!("the refusal's fix: {fix}"), String::new()));
            }
        }
        VerdictKind::Underpowered | VerdictKind::Unclear => {
            let pairs = s["pairs"].as_u64().unwrap_or(0);
            if let Some(n) = p["needed_pairs"].as_u64().filter(|n| *n > pairs) {
                sec.next.push((format!("~{n} pairs for an 80% chance to decide the primary (projected from these pairs' spread)"), cmd.bench(s, &[("pairs", Some(&n.to_string()))])));
            }
        }
        VerdictKind::Stopped if crate::store::resolve(id).is_ok_and(|d| d.join("state.json").is_file()) => {
            sec.next.push(("the bench stopped before a verdict: go on with it".into(), format!("strobe bench --resume {id}")));
        }
        _ => {}
    }
    // Secondaries the gate kept untested for want of pairs (Holm).
    let holm = s["holm_pairs"].as_u64().unwrap_or(0);
    if s["pairs"].as_u64().unwrap_or(0) < holm && s["decided"] == true {
        sec.next.push((format!("the secondaries can only clear Holm's correction from {holm} pairs"), cmd.bench(s, &[("pairs", Some(&holm.to_string()))])));
    }
    if sec.suspects.is_empty() && sec.none.is_none() {
        sec.none = Some("no recording kept".into());
    }
    Ok(())
}

fn check(r: &Value, s: &Value, cmd: &Command, sec: &mut Section) -> Result<()> {
    let m = s["primary"]["metric"].as_str().unwrap_or("");
    let unit = crate::model::metric(m).map_or("", |d| d.unit);
    let runs = s["runs"].as_u64().unwrap_or(1);
    let how = match (s["repeat"]["launches"].as_u64(), runs) {
        (Some(l), _) => format!("the median of {l} cold launches"),
        (None, 1) => "one cold run".to_string(),
        (None, n) => format!("the median of {n} runs in one launch, the first cold"),
    };
    let passed = s["passed"] == true;
    if passed {
        let value = match (s["values"][m].as_f64(), s["refused"][m].as_str()) {
            (Some(v), _) => format!("{} ({how})", fmt::value(v, unit)),
            (None, Some(why)) => format!("refused: {why}"),
            (None, None) => "no value".to_string(),
        };
        sec.result.push(format!("passed · ★ {m} {value}"));
    } else {
        let error = s["error"].as_str().unwrap_or("failed");
        sec.result.push(format!("FAILED: {}", error.lines().next().unwrap_or("")));
        if let Some(h) = s["hint"].as_str() {
            sec.next.push((format!("the failure's hint: {h}"), String::new()));
        }
        let seed = (r["plan"]["used_random"] == true).then(|| r["plan"]["seed"].to_string());
        sec.next.push(("run this case alone again".into(), cmd.bench(s, &[("seed", seed.as_deref())])));
    }
    for c in s["counters"].as_array().into_iter().flatten().filter(|c| c["passed"] == false) {
        sec.result.push(format!("--noworse {}: {}", c["metric"].as_str().unwrap_or(""), c["text"].as_str().or(c["state"].as_str()).unwrap_or("")));
    }
    if let Some(rec) = s["attribution"].as_str()
        && let Some(hits) = hits_of(rec)
    {
        sec.suspects.push((format!("the attribution block {rec} (warmed, its measured runs under the profilers, untimed)"), ranked(hits)));
        sec.next.push(("where its time goes, by package and function".into(), format!("strobe query {rec} hotspots --by package")));
        sec.next.push(("the recording's report".into(), format!("strobe report recording {rec} --open")));
    }
    if let Some(rec) = s["recording"].as_str()
        && let Some(hits) = hits_of(rec)
    {
        sec.suspects.push((format!("the check run's recording {rec} ({how}, no profiler)"), ranked(hits)));
    }
    let kind = s["target_kind"].as_str().unwrap_or("");
    if passed && s["attribution"].is_null() {
        if matches!(kind, "cli" | "tty") {
            sec.none = Some(format!("a {} case has no attribution block in strobe", if kind == "cli" { "cli()" } else { "tty()" }));
        } else {
            if sec.suspects.is_empty() {
                sec.none = Some("a check keeps no profile (--why always adds an attribution block of each case)".into());
            }
            if let Some(c) = cmd.record(s, "head") {
                sec.next.push(("profile this case (one attribution block: CPU and allocation stacks, forced layouts, GC)".into(), c));
            }
        }
    }
    if passed {
        sec.next.push(("time it against another build (pairs of fresh launches, a verdict)".into(), cmd.compare(s)));
    }
    Ok(())
}

fn blocks(_r: &Value, s: &Value, cmd: &Command, sec: &mut Section) -> Result<()> {
    let m = s["primary"]["metric"].as_str().unwrap_or("");
    let unit = crate::model::metric(m).map_or("", |d| d.unit);
    let all = s["blocks"].as_array().map(Vec::as_slice).unwrap_or_default();
    let values: Vec<f64> = all.iter().filter_map(|b| b["values"][m].as_f64()).filter(|v| v.is_finite()).collect();
    let line = if stats::kind_of(&s["primary"]) == VerdictKind::Refused {
        format!("★ {m} {}", s["primary"]["verdict"].as_str().unwrap_or("refused"))
    } else if values.len() >= 2 {
        let (median, lo, hi, conf) = stats::median_interval(&values, 0.95);
        format!("★ {m} median {} [{} … {}] ({:.0}% over {} blocks); nothing compared", fmt::value(median, unit), fmt::value(lo, unit), fmt::value(hi, unit), 100.0 * conf, values.len())
    } else if let [v] = values[..] {
        format!("★ {m} {} (1 block: no interval); nothing compared", fmt::value(v, unit))
    } else {
        format!("★ {m} no value")
    };
    sec.result.push(line);
    if let Some(rec) = s["attribution"]["head"].as_str()
        && let Some(hits) = hits_of(rec)
    {
        sec.suspects.push((format!("head's attribution block {rec} (after its timed blocks, profiled, untimed)"), ranked(hits)));
        sec.next.push(("where its time goes, by package and function".into(), format!("strobe query {rec} hotspots --by package")));
    } else {
        let timed: Vec<&str> = s["timing_recordings"].as_array().into_iter().flatten().filter_map(|t| t["recording"].as_str()).collect();
        if let Some(hits) = timed.first().and_then(|t| hits_of(t)) {
            sec.suspects.push((format!("the first timed block's recording {} (no profiler: its counters and trace)", timed[0]), ranked(hits)));
        } else {
            sec.none = Some("a timed block runs no profiler, and no attribution block ran (--why always adds one)".into());
        }
        if !matches!(s["target_kind"].as_str(), Some("cli" | "tty")) {
            sec.next.push(("profile head after its timed blocks (one attribution block)".into(), cmd.bench(s, &[("why", Some("always"))])));
        }
    }
    sec.next.push(("time it against another build (pairs of fresh launches, a verdict)".into(), cmd.compare(s)));
    Ok(())
}

fn leak(r: &Value, s: &Value, cmd: &Command, sec: &mut Section) -> Result<()> {
    let roles: Vec<&str> = ["base", "head"].into_iter().filter(|role| s["leak"][*role].is_object()).collect();
    let mut unclear = false;
    for role in &roles {
        let l = &s["leak"][*role];
        let side = if roles.len() > 1 { format!("{role} ") } else { String::new() };
        let range = |g: &Value, show: &dyn Fn(f64) -> String| match (g["lo"].as_f64(), g["hi"].as_f64()) {
            (Some(lo), Some(hi)) => format!(" (95%: {} … {} per run)", show(lo), show(hi)),
            _ => String::new(),
        };
        let bytes = |x: f64| fmt::signed(x, "bytes");
        let nodes = |x: f64| fmt::signed(x, "count");
        for (part, what, show) in [("native_host", "host memory", &bytes as &dyn Fn(f64) -> String), ("js_heap", "js heap", &bytes), ("dom_nodes", "dom nodes", &nodes)] {
            let g = &l[part];
            if g.is_object() {
                unclear |= growth(g) == VerdictKind::Unclear;
                // A leak's verdict names its slope; its interval is beside it (an unclear one says its own).
                sec.result.push(format!("{side}{what}: {}{}", g["verdict"].as_str().unwrap_or(""), if growth(g) == VerdictKind::Worse { range(g, show) } else { String::new() }));
            }
        }
        let rec = l["recording"].as_str();
        let kinds: Vec<VerdictKind> = ["native_host", "js_heap", "dom_nodes"].iter().filter(|k| l[**k].is_object()).map(|k| growth(&l[*k])).collect();
        let grows = kinds.contains(&VerdictKind::Worse);
        let mut hits: Vec<Value> = Vec::new();
        if let Some(why) = l["suspects"]["refused"].as_str() {
            sec.result.push(format!("{side}suspects: {why}"));
        }
        // What stayed alive is a suspect only while memory grows or may (no growth: nothing to explain).
        let suspect = grows || kinds.contains(&VerdictKind::Unclear);
        for c in l["suspects"]["classes"].as_array().into_iter().flatten().filter(|_| suspect).take(3) {
            let at = c["location"]["url"].as_str().filter(|u| !u.is_empty()).map(|u| format!(", its constructor at {u}:{}:{}", c["location"]["line"], c["location"]["column"])).unwrap_or_default();
            // The page's own objects (their constructor has a source location) over V8's internals.
            hits.push(json!({
                "rule": "leak-suspect", "level": if grows && c["location"]["url"].as_str().is_some_and(|u| !u.is_empty()) { "warn" } else { "info" },
                "title": format!("{} ×{} from the first half still alive at the end, retaining {}{at}; its largest held by {}", c["class"].as_str().unwrap_or("?"), c["count"], fmt::bytes(c["retained_size"].as_f64().unwrap_or(0.0)), super::path_tail(&c["largest"]["path"])),
                "evidence": c, "handles": rec.map(|r| vec![format!("strobe query {r} leaks")]).unwrap_or_default(),
            }));
        }
        // The recording's other rules (its growth is the result above).
        if let Some(rest) = rec.and_then(hits_of) {
            hits.extend(ranked(rest.into_iter().filter(|h| h["rule"] != "leak").collect()));
        }
        if let Some(rec) = rec {
            let what = if suspect { " (objects from the first half alive at the end, largest retained first)" } else { "" };
            sec.suspects.push((format!("{side}memory block {rec}{what}"), hits));
            if grows {
                let whose = if roles.len() > 1 { format!("{role}: ") } else { String::new() };
                sec.next.push((format!("{whose}retainer paths of what stayed alive"), format!("strobe query {rec} leaks")));
                sec.next.push((format!("{whose}the live heap at the end, by class"), format!("strobe query {rec} heap")));
            }
        }
    }
    if let [b, h] = roles[..]
        && let (Some(b), Some(h)) = (s["leak"][b]["recording"].as_str(), s["leak"][h]["recording"].as_str())
    {
        sec.next.push(("the two live heaps by class, side by side".into(), format!("strobe query {b}..{h} heap")));
    }
    if unclear {
        let n = s["runs_per_half"].as_u64().or(r["args"]["flags"]["repeat"].as_str().and_then(|x| x.parse().ok())).unwrap_or(5);
        sec.next.push((format!("an unclear growth narrows with more runs (this ran {n} per half)"), cmd.bench(s, &[("repeat", Some(&(2 * n).to_string()))])));
    }
    Ok(())
}

/// A growth verdict's kind: its `verdict_kind`, or in results from before it, what its words say.
fn growth(g: &Value) -> VerdictKind {
    if g["verdict_kind"].is_string() {
        return stats::kind_of(g);
    }
    match g["verdict"].as_str().unwrap_or("") {
        v if v.starts_with("leaks") => VerdictKind::Worse,
        v if v.starts_with("unclear") => VerdictKind::Unclear,
        _ => VerdictKind::Same,
    }
}

fn pixels(r: &Value, s: &Value, sec: &mut Section) {
    let frames: Vec<&Value> = s["pixels"].as_array().into_iter().flatten().collect();
    let differ: Vec<&str> = frames.iter().filter(|p| p["state"] == "differ").filter_map(|p| p["name"].as_str()).collect();
    sec.result.push(if differ.is_empty() {
        format!("{} frames the same", frames.len())
    } else {
        format!("{} of {} frames differ beyond launch noise: {}", differ.len(), frames.len(), differ.join(", "))
    });
    sec.none = Some("a pixels run compares screenshots; it times nothing to explain".into());
    if !differ.is_empty() {
        sec.next.push(("the frames and their diffs side by side".into(), format!("strobe report experiment {} --open", r["id"].as_str().unwrap_or(""))));
    }
}

fn bisect(r: &Value) -> Output {
    let id = r["id"].as_str().unwrap_or("");
    let mut text = format!("insights · {id} · a bisect of {} · {}\n", r["series"].as_str().unwrap_or(""), r["metric"].as_str().unwrap_or(""));
    text += &format!("result    {}\n", r["verdict"].as_str().unwrap_or(""));
    let mut next = Vec::new();
    let culprit = r["culprit"]["commit"].as_str();
    if let Some(step) = r["steps"].as_array().into_iter().flatten().find(|s| culprit.is_some() && s["commit"].as_str() == culprit)
        && let Some(e) = step["experiment"].as_str()
    {
        next.push(json!({ "why": "the culprit against the good commit: its suspects", "command": format!("strobe query {e} insights") }));
    }
    if !next.is_empty() {
        text += "next\n";
        for n in &next {
            text += &format!("  {}\n    {}\n", n["why"].as_str().unwrap_or(""), n["command"].as_str().unwrap_or(""));
        }
    }
    Output::new(text, json!({ "experiment": id, "kind": "bisect", "verdict": r["verdict"], "culprit": r["culprit"], "next": next }))
}

/// The bench command that ran the experiment, to run again with a change: its file and flags as given
/// (a check from before results kept them: its variants), less what made that run one of a kind.
struct Command {
    file: String,
    flags: Vec<(String, String)>,
    /// More than one series ran: a rerun of one names its case.
    several: bool,
}

/// Flags a rerun of one series leaves out: its own seed, cap and page, and what it asks for itself.
const DROP: [&str; 9] = ["seed", "pairs", "filter", "why", "max-time", "resume", "json", "quiet", "plan"];

impl Command {
    fn of(r: &Value, series: usize) -> Command {
        let path = r["args"]["pos"][0].as_str().or(r["file"].as_str()).unwrap_or("<file.bench.ts>");
        let file = path.strip_prefix(r"\\?\").unwrap_or(path).to_string();
        let mut flags: Vec<(String, String)> = r["args"]["flags"]
            .as_object()
            .map(|o| o.iter().filter(|(k, _)| !DROP.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.as_str().map_or_else(|| v.to_string(), str::to_string))).collect())
            .unwrap_or_default();
        if flags.is_empty() && r["kind"] == "check" {
            let v = r["variants"].as_array().cloned().unwrap_or_default();
            if let [b, _, ..] = v.as_slice() {
                flags.push(("base".into(), b["spec"].as_str().unwrap_or(".").into()));
            }
            flags.push(("head".into(), v.last().and_then(|h| h["spec"].as_str()).unwrap_or(".").into()));
            flags.push(("check".into(), "true".into()));
        }
        Command { file, flags, several: series > 1 }
    }

    fn get(&self, k: &str) -> Option<&str> {
        self.flags.iter().find(|(f, _)| f == k).map(|(_, v)| v.as_str())
    }

    /// `strobe bench <file> …` for this series alone, with `set` (None: the flag left out).
    fn bench(&self, s: &Value, set: &[(&str, Option<&str>)]) -> String {
        let mut flags: Vec<(String, String)> = self.flags.iter().filter(|(k, _)| !set.iter().any(|(s, _)| s == k)).cloned().collect();
        flags.extend(set.iter().filter_map(|(k, v)| Some((k.to_string(), (*v)?.to_string()))));
        if self.several {
            flags.push(("filter".into(), s["case"].as_str().unwrap_or("").into()));
        }
        format!("strobe bench {}{}", quote(&self.file), flag_text(&flags))
    }

    /// The command that times this series' case against another build: `--base <ref>` for the ref
    /// to compare with.
    fn compare(&self, s: &Value) -> String {
        let keep = |k: &str| !matches!(k, "check" | "blocks" | "base" | "runs" | "repeat" | "batch" | "aa");
        let mut flags: Vec<(String, String)> = vec![("base".into(), "<ref>".into())];
        flags.extend(self.flags.iter().filter(|(k, _)| keep(k)).cloned());
        if self.several {
            flags.push(("filter".into(), s["case"].as_str().unwrap_or("").into()));
        }
        format!("strobe bench {}{}", quote(&self.file), flag_text(&flags))
    }

    /// `strobe record --bench <file>#<case> --head <variant>`: one attribution block of the case on a side.
    fn record(&self, s: &Value, role: &str) -> Option<String> {
        let spec = match role {
            "base" if self.get("aa").is_some() => self.get("head"),
            other => self.get(other),
        }
        .unwrap_or(".");
        let mut flags: Vec<(String, String)> = vec![("head".into(), spec.into())];
        for k in ["target", "headless", "mobile", "engine", "wayland"] {
            if let Some(v) = self.get(k) {
                flags.push((k.into(), v.into()));
            }
        }
        if self.get("target").is_none()
            && let Some(t) = s["target"].as_str()
        {
            flags.push(("target".into(), t.into()));
        }
        if let Some(p) = s["params"].as_object().filter(|p| !p.is_empty()) {
            flags.push(("params".into(), p.iter().map(|(k, v)| format!("{k}={}", v.as_str().map_or_else(|| v.to_string(), str::to_string))).collect::<Vec<_>>().join(",")));
        }
        let case = s["case"].as_str()?;
        Some(format!("strobe record --bench {}{}", quote(&format!("{}#{case}", self.file)), flag_text(&flags)))
    }
}

fn flag_text(flags: &[(String, String)]) -> String {
    flags.iter().map(|(k, v)| if v == "true" { format!(" --{k}") } else { format!(" --{k} {}", quote(v)) }).collect()
}

/// A word as a shell takes it whole: in double quotes when it holds a space, `#` or a quote.
fn quote(w: &str) -> String {
    if w.is_empty() || w.contains([' ', '"', '\'', '&', '|', ';', '#', '(', ')']) && !(w.starts_with('<') && w.ends_with('>')) { format!("\"{}\"", w.replace('"', "\\\"")) } else { w.to_string() }
}
