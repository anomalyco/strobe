//! `query <exp> insights` (SPEC §11.3): per series, what the experiment found (a comparison's verdict,
//! a check's, a `--blocks` run's or a leak run's status), the suspects its kept recordings show,
//! ranked, the warnings that bear on that result, and the commands that go further. Facts and
//! pointers only: each line is a number the experiment holds, a rule's hit over a recording it kept,
//! or a command that shows more. When in doubt it says less: what is pooled across cases, untimed or
//! read from one profiled launch beside a "no change" verdict is no suspect.

use super::{Output, Rec, fmt, hits_text, insights, leak_class, leak_classes, mark_sides, series_insights};
use crate::model::{Result, VerdictKind};
use crate::stats;
use serde_json::{Value, json};

/// `source`: the result.json it was read from when named by its path (`query <path> insights`).
pub fn run(r: &Value, source: Option<&std::path::Path>) -> Result<Output> {
    let id = r["id"].as_str().unwrap_or("");
    if r["kind"] == "bisect" {
        return Ok(bisect(r));
    }
    let series: Vec<&Value> = r["series"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
    let mut cmd = Command::of(r);
    // A result read from a path that strobe does not keep: its commands name that path.
    if let Some(f) = source.filter(|_| !crate::store::resolve(id).is_ok_and(|d| crate::store::is_experiment(&d) && d.join("result.json").is_file())) {
        cmd.exp = super::path_word(f);
    }
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
        let sec = section(r, s, &cmd);
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
    /// Why there are no suspects (from a source, or at all).
    none: Vec<String>,
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
        for n in &self.none {
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

    /// A kept recording's hits under `source`, or why it gives none.
    fn read(&mut self, source: String, rec: &str) {
        match kept(rec) {
            Kept::Hits(hits) => self.suspects.push((source, ranked(hits))),
            Kept::Gone => self.none.push(format!("{source}: not on this machine (pruned, or the result came from another)")),
            Kept::Pooled(n) => self.none.push(format!("{source}: {rec} holds the runs of {n} cases of one --batch launch, so its rules would pool them (record --bench profiles one case alone)")),
        }
    }
}

/// What a kept recording gives the insights.
enum Kept {
    Hits(Vec<Value>),
    /// Not on this machine any more (pruned), or never was (a result from another machine).
    Gone,
    /// One launch's recording shared by this many `--batch` cases: its views pool every case's runs.
    Pooled(usize),
}

fn kept(id: &str) -> Kept {
    let Ok(rec) = Rec::resolve(id) else { return Kept::Gone };
    if let Some(n) = rec.meta["batch"].as_array().map(Vec::len).filter(|n| *n > 1) {
        return Kept::Pooled(n);
    }
    match insights(&rec) {
        Ok(o) => Kept::Hits(o.json["insights"].as_array().cloned().unwrap_or_default()),
        Err(_) => Kept::Gone,
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

fn section(r: &Value, s: &Value, cmd: &Command) -> Section {
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
        Some("check") => check(r, s, cmd, &mut sec),
        Some("blocks") => blocks(r, s, cmd, &mut sec),
        Some("leak") => leak(r, s, cmd, &mut sec),
        Some("pixels") => pixels(s, cmd, &mut sec),
        _ => comparison(r, s, cmd, &mut sec),
    }
    sec
}

/// The warnings that bear on a result: every warn-level one, and why an attribution block is missing
/// or failed; each once.
fn warnings(list: &Value) -> Vec<Value> {
    let bears = |w: &Value| w["level"] == "warn" || (w["id"] == "Attribution" && w["text"].as_str().is_some_and(|t| t.contains("attribution block")));
    list.as_array().into_iter().flatten().filter(|w| bears(w)).fold(Vec::new(), |mut all, w| {
        if !all.iter().any(|x: &Value| x["id"] == w["id"] && x["text"] == w["text"]) {
            all.push(w.clone());
        }
        all
    })
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

/// Warn-level hits first, each level in the rules' own order.
fn ranked(mut hits: Vec<Value>) -> Vec<Value> {
    hits.sort_by_key(|h| h["level"] != "warn");
    hits
}

/// Base's hits head has none of: facts of base, never suspects of head's change (info, said as base's).
fn base_only(hits: impl IntoIterator<Item = Value>) -> Vec<Value> {
    hits.into_iter()
        .map(|mut x| {
            x["title"] = json!(format!("base only: {}", x["title"].as_str().unwrap_or("")));
            x["level"] = json!("info");
            x
        })
        .collect()
}

/// A comparison's primary as one line: `★ cpu.work more: 244 ms → 256 ms, +4.6% [+4.1% … +5.8%] over 8 pairs`.
fn verdict_line(c: &Value, aa: bool) -> String {
    let m = c["metric"].as_str().unwrap_or("");
    let kind = stats::kind_of(c);
    // A time's absolute bound drawn in the unit times print in (`stats::verdict_text`).
    let verdict = stats::verdict_text(c);
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
            (false, Some(lo), Some(hi)) => {
                let v = fmt::signed_values(&[est, lo, hi], abs);
                format!("{} [{} … {}]", v[0], v[1], v[2])
            }
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

/// Why strobe has no attribution block for a series' target, when it has none to give: a command ends
/// inside each run, a terminal program and a WebKit page have no profiler in strobe, a Firefox page's
/// launch has no profile (`bench::no_profiler`).
fn unprofiled(s: &Value) -> Option<String> {
    let kinds = [&s["target_kind"], &s["target_kinds"]["base"], &s["target_kinds"]["head"]];
    let launch = s["kind"] == "launch" || !s["until"].is_null();
    let engines = [&s["engine"], &s["engines"]["base"], &s["engines"]["head"]];
    if kinds.iter().any(|k| *k == "cli") {
        Some("a command ends inside each run: there is nothing to profile it with".into())
    } else if kinds.iter().any(|k| *k == "tty") {
        Some("a terminal program has no profiler in strobe yet".into())
    } else if engines.iter().any(|e| *e == "webkit") {
        Some("WebKit pages have no profilers in strobe yet".into())
    } else if launch && engines.iter().any(|e| *e == "firefox") {
        Some("a Firefox page's launch has no profile in strobe yet".into())
    } else {
        None
    }
}

/// The `Attribution` or `MaxTime` warning that says why a series' attribution block is missing.
fn attribution_note(r: &Value, s: &Value) -> Option<String> {
    let label = s["label"].as_str().unwrap_or("");
    let own = s["warnings"].as_array().into_iter().flatten();
    let theirs = r["warnings"].as_array().into_iter().flatten().filter(|w| w["text"].as_str().is_some_and(|t| t.starts_with(label) || t.starts_with("base") || t.starts_with("head")));
    own.chain(theirs)
        .find(|w| (w["id"] == "Attribution" || w["id"] == "MaxTime") && w["text"].as_str().is_some_and(|t| t.contains("attribution block")))
        .map(|w| format!("{} {}", w["id"].as_str().unwrap_or(""), w["text"].as_str().unwrap_or("")))
}

/// Why a comparison series kept no attribution recordings, from what the result says, and whether
/// `record --bench` could profile it.
fn why_none(r: &Value, s: &Value, differs: bool) -> (String, bool) {
    if let Some(no) = unprofiled(s) {
        return (no, false);
    }
    if let Some(note) = attribution_note(r, s) {
        return (note, true);
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

fn comparison(r: &Value, s: &Value, cmd: &Command, sec: &mut Section) {
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
    let differs = p["decided"] == true && (lo > 0.0 || hi < 0.0) && !aa;
    // Which way head's cost went: a worse primary (slower, more, fewer frames) means more of it, a
    // better one less, whichever way the metric counts. A mover (CPU, layout, allocations: costs) that
    // went that way can explain the change; one that went the other way cannot.
    let way = match kind {
        VerdictKind::Worse if differs => 1.0,
        VerdictKind::Better if differs => -1.0,
        _ => 0.0,
    };
    let attr = &s["attribution"];
    let (b, h) = (attr["base"].as_str(), attr["head"].as_str());
    let id = cmd.exp.as_str();
    let profile = "untimed, under the profilers; one launch per side, so an interval covers sampling alone, not launch to launch";
    if r["comparison"] == "implementations" {
        // Two implementations share no code: each side's own hits, under its name.
        for (i, role) in ["base", "head"].into_iter().enumerate() {
            let Some(rec) = attr[role].as_str() else { continue };
            let name = r["variants"][i]["name"].as_str().unwrap_or(role);
            sec.read(format!("{name}'s attribution block {rec} (on its own: two implementations share no code; {profile})"), rec);
        }
        if b.is_none() && h.is_none() {
            sec.none.push(format!("no attribution blocks: {}", why_none(r, s, differs).0));
        } else {
            sec.next.push(("both sides side by side".into(), format!("strobe report experiment {id} --open")));
        }
    } else if let (Some(b), Some(h)) = (b, h) {
        match (Rec::resolve(b), Rec::resolve(h)) {
            (Ok(base), Ok(head)) => match series_insights(&base, &head, r["plan"]["runs_per_block"].as_u64()) {
                Ok(o) => attributed(o.json, (b, h), way, differs, profile, id, sec),
                Err(e) => sec.none.push(format!("the attribution blocks {b} and {h} could not be read: {}", e.message)),
            },
            (base, head) => {
                // One side's recording is gone: the other's hits alone, said as that side's.
                for (role, rec, ok) in [("base", b, base.is_ok()), ("head", h, head.is_ok())] {
                    if ok {
                        sec.read(format!("{role}'s attribution block {rec} alone ({profile})"), rec);
                    } else {
                        sec.none.push(format!("{role}'s attribution block {rec} is not on this machine (pruned, or the result came from another)"));
                    }
                }
            }
        }
    } else {
        let (why, profilable) = why_none(r, s, differs);
        // One side's attribution block ran and the other's did not: its hits, and the missing side's command.
        if let Some((role, rec)) = b.map(|x| ("base", x)).or(h.map(|x| ("head", x))) {
            sec.read(format!("{role}'s attribution block {rec} alone (the other side's: {why}; {profile})"), rec);
        } else {
            // Without a profile, the timed blocks' kept recordings still hold their counters and trace.
            let timed: Vec<(&str, &str)> = s["timing_recordings"].as_array().into_iter().flatten().filter_map(|t| Some((t["variant"].as_str().unwrap_or("head"), t["recording"].as_str()?))).collect();
            let pick = |role: &str| timed.iter().find(|t| t.0 == role).map(|t| t.1);
            match pick("head").map(|hr| (hr, kept(hr))) {
                Some((hr, Kept::Hits(mut hits))) => {
                    let theirs = pick("base").and_then(|br| Rec::resolve(br).ok()).and_then(|r| insights(&r).ok());
                    let only = theirs.map(|t| mark_sides(&mut hits, &t.json)).unwrap_or_default();
                    hits = ranked(hits);
                    hits.extend(base_only(only));
                    sec.suspects.push((format!("head's timed block recording {hr} (no profiler: its counters and trace; no attribution blocks: {why})"), hits));
                }
                Some((hr, _)) => sec.none.push(format!("no attribution blocks ({why}), and head's timed block recording {hr} is not on this machine")),
                None => sec.none.push(format!("no attribution blocks: {why}")),
            }
        }
        if differs && profilable {
            let case = s["case"].as_str().unwrap_or("");
            let roles: Vec<&str> = ["base", "head"].into_iter().filter(|r| attr[*r].is_null()).collect();
            let cmds: Vec<String> = roles.iter().filter_map(|role| cmd.record(s, role)).collect();
            if !cmds.is_empty() {
                sec.next.push((
                    format!(
                        "profile {case} on {} (one attribution block each: CPU and allocation stacks, forced layouts, GC), then compare the two with strobe query <base rec>..<head rec> hotspots",
                        roles.join(" and ")
                    ),
                    String::new(),
                ));
                for c in cmds {
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
    if sec.suspects.is_empty() && sec.none.is_empty() {
        sec.none.push("no recording kept".into());
    }
}

/// A comparison's two attribution blocks: the movers that can explain a decided change first (those
/// along it, then head's hits over their threshold there alone, the movers against it, the hits base
/// has too, base's alone). Beside no decided change (same, unclear, A/A), the one profiled launch per
/// side differs by sampling and launch to launch alone: no mover is a suspect.
fn attributed(o: Value, (b, h): (&str, &str), way: f64, differs: bool, profile: &str, id: &str, sec: &mut Section) {
    let movers = o["movers"].as_array().cloned().unwrap_or_default();
    let along = |m: &Value| (m["head_per_run"].as_f64().unwrap_or(0.0) - m["base_per_run"].as_f64().unwrap_or(0.0)).signum() == way;
    let as_hit = |m: &Value, against: bool| json!({ "rule": "mover", "level": "info", "title": format!("{}{}", m["title"].as_str().unwrap_or(""), if against { " (against the primary's change)" } else { "" }), "evidence": m, "handles": m["handles"] });
    let heads = o["head"].as_array().cloned().unwrap_or_default();
    let (only, both): (Vec<Value>, Vec<Value>) = heads.into_iter().partition(|h| h["sides"] != "both");
    let mut hits: Vec<Value> = Vec::new();
    if differs {
        hits.extend(movers.iter().filter(|m| along(m)).map(|m| as_hit(m, false)));
    }
    hits.extend(ranked(only));
    if differs {
        hits.extend(movers.iter().filter(|m| !along(m)).map(|m| as_hit(m, true)));
    }
    hits.extend(ranked(both));
    hits.extend(base_only(o["base_only"].as_array().cloned().unwrap_or_default()));
    let skipped = if !differs && !movers.is_empty() {
        format!("; no movers: the timed pairs decided no change, so what the two launches differ by is no suspect (strobe query {b}..{h} hotspots shows it)")
    } else {
        String::new()
    };
    sec.suspects.push((format!("the attribution blocks, base {b} · head {h} ({profile}){skipped}"), hits));
    sec.extra = json!({ "movers": if differs { o["movers"].clone() } else { json!([]) }, "head": o["head"], "base_only": o["base_only"] });
    let views: Vec<&str> = ["hotspots", "layout", "alloc"].into_iter().filter(|v| differs && movers.iter().any(|m| m["handles"][0].as_str().is_some_and(|c| c.ends_with(&format!(" {v}"))))).collect();
    for v in if views.is_empty() { vec!["hotspots"] } else { views } {
        sec.next.push((format!("the two launches by function ({v}, per run, with the stacks each row moved in)"), format!("strobe query {b}..{h} {v}")));
    }
    sec.next.push(("both sides and the diff flame".into(), format!("strobe report experiment {id} --open")));
}

fn check(r: &Value, s: &Value, cmd: &Command, sec: &mut Section) {
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
    if let Some(rec) = s["attribution"].as_str() {
        sec.read(format!("the attribution block {rec} (warmed, its measured runs under the profilers, untimed)"), rec);
        if sec.suspects.iter().any(|(src, _)| src.contains(rec)) {
            sec.next.push(("where its time goes, by package and function".into(), format!("strobe query {rec} hotspots --by package")));
            sec.next.push(("the recording's report".into(), format!("strobe report recording {rec} --open")));
        }
    }
    if let Some(rec) = s["recording"].as_str() {
        sec.read(format!("the check run's recording {rec} ({how}, no profiler)"), rec);
    }
    let pooled = sec.none.iter().any(|n| n.contains("--batch"));
    if passed && (s["attribution"].is_null() || pooled) {
        match (unprofiled(s), attribution_note(r, s)) {
            (Some(no), _) => sec.none.push(no),
            // `--why always` ran and its block failed or was skipped: why, never "add --why always".
            (None, Some(note)) => sec.none.push(note),
            (None, None) => {
                if sec.suspects.is_empty() && sec.none.is_empty() {
                    sec.none.push("a check keeps no profile (--why always adds an attribution block of each case)".into());
                }
                if let Some(c) = cmd.record(s, cmd.side_of(s, "head")) {
                    sec.next.push(("profile this case alone (one attribution block: CPU and allocation stacks, forced layouts, GC)".into(), c));
                }
            }
        }
    }
    if passed && let Some(c) = cmd.compare(s) {
        sec.next.push(("time it against base (pairs of fresh launches, a verdict)".into(), c));
    }
}

fn blocks(r: &Value, s: &Value, cmd: &Command, sec: &mut Section) {
    let m = s["primary"]["metric"].as_str().unwrap_or("");
    let unit = crate::model::metric(m).map_or("", |d| d.unit);
    let all = s["blocks"].as_array().map(Vec::as_slice).unwrap_or_default();
    let values: Vec<f64> = all.iter().filter_map(|b| b["values"][m].as_f64()).filter(|v| v.is_finite()).collect();
    let line = if stats::kind_of(&s["primary"]) == VerdictKind::Refused {
        format!("★ {m} {}", s["primary"]["verdict"].as_str().unwrap_or("refused"))
    } else if values.len() >= 2 {
        let (median, lo, hi, conf) = stats::median_interval(&values, 0.95);
        let v = fmt::values(&[median, lo, hi], unit);
        format!("★ {m} median {} [{} … {}] ({:.0}% over {} blocks); nothing compared", v[0], v[1], v[2], 100.0 * conf, values.len())
    } else if let [v] = values[..] {
        format!("★ {m} {} (1 block: no interval); nothing compared", fmt::value(v, unit))
    } else {
        format!("★ {m} no value")
    };
    sec.result.push(line);
    if let Some(rec) = s["attribution"]["head"].as_str() {
        sec.read(format!("head's attribution block {rec} (after its timed blocks, profiled, untimed)"), rec);
        sec.next.push(("where its time goes, by package and function".into(), format!("strobe query {rec} hotspots --by package")));
        return;
    }
    let timed: Vec<&str> = s["timing_recordings"].as_array().into_iter().flatten().filter_map(|t| t["recording"].as_str()).collect();
    if let Some(t) = timed.first() {
        sec.read(format!("the first timed block's recording {t} (no profiler: its counters and trace)"), t);
    }
    match (unprofiled(s), attribution_note(r, s)) {
        (Some(no), _) => sec.none.push(format!("no attribution block: {no}")),
        (None, Some(note)) => sec.none.push(format!("no attribution block: {note}")),
        (None, None) if r["args"]["flags"]["why"] == "always" => sec.none.push("no attribution block was kept".into()),
        (None, None) => {
            if timed.is_empty() {
                sec.none.push("a timed block runs no profiler, and no attribution block ran".into());
            }
            sec.next.push(("profile head after its timed blocks (one attribution block)".into(), cmd.bench(s, &[("why", Some("always"))])));
        }
    }
}

fn leak(r: &Value, s: &Value, cmd: &Command, sec: &mut Section) {
    let roles: Vec<&str> = ["base", "head"].into_iter().filter(|role| s["leak"][*role].is_object()).collect();
    let implementations = r["comparison"] == "implementations";
    let mut unclear = false;
    // Snapshots: a page in Chromium. A native app's host memory and a Firefox page's about:memory name
    // no object.
    let mut snapshots = true;
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
        let named = !l["native_host"].is_object() && !l["suspects"]["refused"].is_string();
        snapshots &= named;
        if let Some(why) = l["suspects"]["refused"].as_str() {
            sec.result.push(format!("{side}suspects: {why}"));
        }
        // What stayed alive is a suspect only while memory grows or may (no growth: nothing to explain);
        // the page's own objects first (`leak_classes`).
        let suspect = named && (grows || kinds.contains(&VerdictKind::Unclear));
        let mut hits: Vec<Value> = Vec::new();
        for c in leak_classes(&l["suspects"]).into_iter().filter(|_| suspect).take(3) {
            let own = c["location"]["url"].as_str().is_some_and(|u| !u.is_empty());
            hits.push(json!({
                "rule": "leak-suspect", "level": if grows && own { "warn" } else { "info" },
                "title": format!("{} from the first half still alive at the end", leak_class(c)),
                "evidence": c, "handles": rec.map(|r| vec![format!("strobe query {r} leaks")]).unwrap_or_default(),
            }));
        }
        let Some(rec) = rec else { continue };
        // The recording's other rules (its growth is the result above).
        match kept(rec) {
            Kept::Hits(rest) => hits.extend(ranked(rest.into_iter().filter(|h| h["rule"] != "leak").collect())),
            _ => sec.none.push(format!("{side}memory block {rec} is not on this machine (pruned, or the result came from another)")),
        }
        if hits.is_empty() && !suspect && !sec.none.iter().any(|n| n.contains(rec)) {
            sec.none.push(format!(
                "{side}memory block {rec}: {}",
                if grows || kinds.contains(&VerdictKind::Unclear) { "its objects are not named (no heap snapshot)" } else { "no memory growth to explain" }
            ));
        } else if !hits.is_empty() {
            let what = if suspect { " (objects from the first half alive at the end, the page's own first)" } else { "" };
            sec.suspects.push((format!("{side}memory block {rec}{what}"), hits));
        }
        let whose = if roles.len() > 1 { format!("{role}: ") } else { String::new() };
        if grows && named {
            sec.next.push((format!("{whose}retainer paths of what stayed alive"), format!("strobe query {rec} leaks")));
            sec.next.push((format!("{whose}the live heap at the end, by class"), format!("strobe query {rec} heap")));
        } else if grows {
            // No heap snapshot: the memory census by process and layer, first and last.
            sec.next.push((format!("{whose}memory at the first census and at the end"), format!("strobe query {rec} memory --at 0")));
            sec.next.push((String::new(), format!("strobe query {rec} memory")));
        }
    }
    if let [b, h] = roles[..]
        && let (Some(b), Some(h)) = (s["leak"][b]["recording"].as_str(), s["leak"][h]["recording"].as_str())
        && !implementations
    {
        let view = if snapshots { "heap" } else { "memory" };
        sec.next.push((format!("the two sides' {}, side by side", if snapshots { "live heaps by class" } else { "memory by process" }), format!("strobe query {b}..{h} {view}")));
    }
    if unclear {
        let n = s["runs_per_half"].as_u64().or(r["args"]["flags"]["repeat"].as_str().and_then(|x| x.parse().ok())).unwrap_or(5);
        sec.next.push((format!("an unclear growth narrows with more runs (this ran {n} per half)"), cmd.bench(s, &[("repeat", Some(&(2 * n).to_string()))])));
    }
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

fn pixels(s: &Value, cmd: &Command, sec: &mut Section) {
    let frames: Vec<&Value> = s["pixels"].as_array().into_iter().flatten().collect();
    let differ: Vec<&str> = frames.iter().filter(|p| p["state"] == "differ").filter_map(|p| p["name"].as_str()).collect();
    sec.result.push(if differ.is_empty() {
        format!("{} frames the same", frames.len())
    } else {
        format!("{} of {} frames differ beyond launch noise: {}", differ.len(), frames.len(), differ.join(", "))
    });
    sec.none.push("a pixels run compares screenshots; it times nothing to explain".into());
    if !differ.is_empty() {
        sec.next.push(("the frames and their diffs side by side".into(), format!("strobe report experiment {} --open", cmd.exp)));
    }
}

fn bisect(r: &Value) -> Output {
    let id = r["id"].as_str().unwrap_or("");
    let mut text = format!("insights · {id} · a bisect of {} · {}\n", r["series"].as_str().unwrap_or(""), r["metric"].as_str().unwrap_or(""));
    // The step verdicts it quotes drawn in the unit times print in now.
    text += &format!("result    {}\n", crate::app::bisect::verdict_text(r));
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

/// The command that ran the experiment, to run again with a change: a bench file and its flags as
/// given (results that kept no flags: their variants), or a `strobe run` of two commands, less what
/// made that run one of a kind. Every command it prints parses as strobe's own parser reads it
/// (`scripts/dev/next-commands.ts` checks them under `sh` and PowerShell).
struct Command {
    /// The bench file; None for `strobe run`, whose file is strobe's own.
    file: Option<String>,
    flags: Vec<(String, String)>,
    implementations: bool,
    /// What a command names the experiment by: its id, or the path of a result strobe does not keep.
    exp: String,
}

/// Flags a rerun leaves out: its own id and cap, its output's form (`--unit` too: how it printed, not
/// what it measured), and what it asks for itself. `--filter` stays (a rerun of a series of several
/// names its case instead); `--seed` is the run's own seed when its hooks drew on it
/// (`plan.used_random`: the workload), else left out.
const DROP: [&str; 13] = ["id", "seed", "pairs", "why", "max-time", "resume", "json", "quiet", "plan", "verbose", "progress", "stop", "unit"];

impl Command {
    fn of(r: &Value) -> Command {
        let mut flags: Vec<(String, String)> = r["args"]["flags"]
            .as_object()
            .map(|o| o.iter().filter(|(k, _)| !DROP.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.as_str().map_or_else(|| v.to_string(), str::to_string))).collect())
            .unwrap_or_default();
        if r["plan"]["used_random"] == true
            && let Some(seed) = r["plan"]["seed"].as_u64().map(|s| s.to_string()).or_else(|| r["args"]["flags"]["seed"].as_str().map(str::to_string))
        {
            flags.push(("seed".into(), seed));
        }
        // A result that kept no flags (a check, an older comparison): each side from its variant.
        let variants = r["variants"].as_array().cloned().unwrap_or_default();
        let spec = |v: &Value| v["spec"].as_str().map(str::to_string);
        if !flags.iter().any(|(k, _)| k == "head")
            && let Some(h) = variants.last().and_then(spec)
        {
            if r["plan"]["aa"] == true {
                flags.push(("aa".into(), "true".into()));
            } else if let [b, _, ..] = variants.as_slice()
                && let Some(b) = spec(b)
            {
                flags.push(("base".into(), b));
            }
            flags.push(("head".into(), h));
            if r["kind"] == "check" {
                flags.push(("check".into(), "true".into()));
            }
        }
        // `strobe run --base "<command>" --head "<command>"`: its bench file is strobe's own, its sides
        // `cmd:` variants.
        let run = flags.iter().any(|(k, v)| k == "head" && v.starts_with("cmd:"));
        for (k, v) in &mut flags {
            if k != "head" && k != "base" {
                continue;
            }
            if let Some(line) = v.strip_prefix("cmd:") {
                *v = line.to_string();
            } else {
                // A folder as the bench kept it (its full path) in forward slashes, as the file below: no
                // git ref holds a `\`.
                *v = v.strip_prefix(r"\\?\").unwrap_or(v).replace('\\', "/");
            }
        }
        let path = r["args"]["pos"][0].as_str().or(r["file"].as_str()).unwrap_or("");
        // Forward slashes, which Windows reads too: a path with a `'` then needs no `\` beside it in
        // double quotes, so PowerShell reads it as `sh` does.
        let file = (!run && !path.is_empty()).then(|| path.strip_prefix(r"\\?\").unwrap_or(path).replace('\\', "/"));
        // Sides that name targets of the file (`--head <target>[=<variant>]`, `--base <target> --head
        // <target>`): a comparison of implementations, a run of one target (its variant names it), or a
        // check of two targets (`--check --base <target> --head <target>`, whose one variant is the bench
        // file's own folder: both flags name targets its series ran). Not a parity matrix alone: two
        // targets of one case checked on one build (`--head <folder> --check`) name builds, not targets.
        let targets: Vec<&str> = r["series"].as_array().into_iter().flatten().filter_map(|s| s["target"].as_str()).collect();
        let names_target = |k: &str| flags.iter().any(|(f, v)| f == k && targets.contains(&v.as_str()));
        let implementations = r["comparison"] == "implementations" || variants.iter().any(|v| v["target"].is_string()) || (r["kind"] == "check" && names_target("base") && names_target("head"));
        Command { file, flags, implementations, exp: r["id"].as_str().unwrap_or("").to_string() }
    }

    /// The side (`base` or `head`) whose target a check's series ran, for two targets; else `role`.
    fn side_of<'a>(&self, s: &Value, role: &'a str) -> &'a str {
        let target = |side: &str| self.get(side).map(|x| x.split_once('=').map_or(x, |(t, _)| t).trim_start_matches("target:").to_string());
        match s["target"].as_str() {
            Some(t) if self.implementations && target("base").as_deref() == Some(t) => "base",
            Some(t) if self.implementations && target("head").as_deref() == Some(t) => "head",
            _ => role,
        }
    }

    fn get(&self, k: &str) -> Option<&str> {
        self.flags.iter().find(|(f, _)| f == k).map(|(_, v)| v.as_str())
    }

    /// This series alone: `--filter <case>` (exact), its target and its params, whatever else the file holds.
    fn narrow(&self, s: &Value, flags: &mut Vec<(String, String)>) {
        if self.file.is_none() || s["case"].as_str().is_none_or(str::is_empty) {
            return;
        }
        flags.retain(|(k, _)| k != "filter");
        flags.push(("filter".into(), s["case"].as_str().unwrap_or("").into()));
        if self.get("target").is_none()
            && !self.implementations
            && let Some(t) = s["target"].as_str()
        {
            flags.push(("target".into(), t.into()));
        }
        // This series' own values, whatever the experiment swept (`--params all`, `n=1,n=3`).
        if let Some(p) = params(s) {
            flags.retain(|(k, _)| k != "params");
            flags.push(("params".into(), p));
        }
    }

    /// The experiment again for this series alone, with `set` (None: the flag left out).
    fn bench(&self, s: &Value, set: &[(&str, Option<&str>)]) -> String {
        let mut flags: Vec<(String, String)> = self.flags.iter().filter(|(k, _)| !set.iter().any(|(s, _)| s == k)).cloned().collect();
        flags.extend(set.iter().filter_map(|(k, v)| Some((k.to_string(), (*v)?.to_string()))));
        self.narrow(s, &mut flags);
        match &self.file {
            Some(f) => line("bench", Some(f), &flags),
            None => line("run", None, &flags),
        }
    }

    /// A check of base and head (`--check --base`) timed as a comparison of the same two: pairs of fresh
    /// launches and a verdict. None when the check ran one side (no base to compare with).
    fn compare(&self, s: &Value) -> Option<String> {
        self.get("base")?;
        let mut flags: Vec<(String, String)> = self.flags.iter().filter(|(k, _)| !matches!(k.as_str(), "check" | "repeat" | "batch" | "noworse")).cloned().collect();
        self.narrow(s, &mut flags);
        Some(match &self.file {
            Some(f) => line("bench", Some(f), &flags),
            None => line("run", None, &flags),
        })
    }

    /// `strobe record --bench <file>#<case>` on one side: that side's variant (an implementation's
    /// `target[=variant]`: its target and variant). None for `strobe run` (commands have no profile).
    fn record(&self, s: &Value, role: &str) -> Option<String> {
        let file = self.file.as_ref()?;
        let case = s["case"].as_str()?;
        let side = match role {
            "base" if self.get("aa").is_some() => self.get("head"),
            other => self.get(other),
        };
        let mut flags: Vec<(String, String)> = Vec::new();
        let target = if self.implementations {
            // The side's target and its variant (`target=variant`) when it names one; a result that kept
            // no flags, the series' target.
            let (t, v) = side.map(|x| x.split_once('=').map_or((x, None), |(t, v)| (t, Some(v)))).unwrap_or(("", None));
            if let Some(v) = v {
                flags.push(("head".into(), v.into()));
            }
            Some(t.trim_start_matches("target:").to_string()).filter(|t| !t.is_empty() && t != ".").or_else(|| s["target"].as_str().map(str::to_string))
        } else {
            // A side `<target>=<variant>` of this series' own target: its variant on its target.
            let side = side.unwrap_or(".");
            let side = side.split_once('=').filter(|(t, _)| s["target"].as_str() == Some(t.trim_start_matches("target:"))).map_or(side, |(_, v)| v);
            flags.push(("head".into(), side.into()));
            s["target"].as_str().map(str::to_string).or_else(|| self.get("target").map(str::to_string))
        };
        if let Some(t) = target {
            flags.push(("target".into(), t));
        }
        // What chose how it ran and what was measured: the same in the profile.
        for k in ["headless", "mobile", "engine", "wayland", "style", "runs", "pin", "power", "seed"] {
            if let Some(v) = self.get(k) {
                flags.push((k.into(), v.into()));
            }
        }
        // A native case's off-CPU waits go in its profile too (`record --bench` takes `waits` alone).
        if self.get("probes") == Some("waits") {
            flags.push(("probes".into(), "waits".into()));
        }
        if let Some(p) = params(s).or_else(|| self.get("params").map(str::to_string)) {
            flags.push(("params".into(), p));
        }
        Some(line("record", None, &[vec![("bench".to_string(), format!("{file}#{case}"))], flags].concat()))
    }
}

/// A series' params as `--params` takes them: `k=v,k2=v2`.
fn params(s: &Value) -> Option<String> {
    let p = s["params"].as_object().filter(|p| !p.is_empty())?;
    Some(p.iter().map(|(k, v)| format!("{k}={}", v.as_str().map_or_else(|| v.to_string(), str::to_string))).collect::<Vec<_>>().join(","))
}

/// `strobe <command> [<file>] --flag value …`: only the flags that command reads (`cli::FLAGS`), each as
/// its kind is written (a switch bare, `--mobile` or `--mobile=<serial>`), every word quoted for `sh`
/// and PowerShell alike.
fn line(command: &str, file: Option<&str>, flags: &[(String, String)]) -> String {
    let mut out = format!("strobe {command}");
    if let Some(f) = file {
        out += &format!(" {}", quote(f));
    }
    for (k, v) in flags {
        let Some((_, kind, commands)) = crate::cli::FLAGS.iter().find(|(n, _, _)| n == k) else { continue };
        if *commands != "*" && !commands.split(' ').any(|c| c == command) {
            continue;
        }
        out += &match kind {
            crate::cli::Kind::Switch if v == "false" => continue,
            crate::cli::Kind::Switch => format!(" --{k}"),
            crate::cli::Kind::Optional if v.is_empty() || v == "true" => format!(" --{k}"),
            crate::cli::Kind::Optional => format!(" {}", quote(&format!("--{k}={v}"))),
            _ => format!(" --{k} {}", quote(v)),
        };
    }
    out
}

/// A word both `sh` and PowerShell pass whole and as written: bare when it holds only letters, digits
/// and `_-./:=+%^`; else in single quotes (literal in both); a word with a `'` in double quotes, where
/// it holds none of `"$\` and backtick (each shell's escapes), else as `sh` writes it.
pub(super) fn quote(w: &str) -> String {
    if !w.is_empty() && w.chars().all(|c| c.is_alphanumeric() || "_-./:=+%^".contains(c)) {
        w.to_string()
    } else if !w.contains('\'') {
        format!("'{w}'")
    } else if !w.contains(['"', '$', '\\', '`']) {
        format!("\"{w}\"")
    } else {
        format!("'{}'", w.replace('\'', r"'\''"))
    }
}
