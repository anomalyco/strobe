//! `report experiment <exp> --format pr` (SPEC §11.2): an experiment as one delimited block of
//! GitHub-flavoured Markdown for a pull request body: one hero line, a caution for what got worse, one
//! line per series in plain words, at most one caveat, then a Details drawer (metrics, pairs,
//! profiles, method) and a Rerun drawer. The block sits between two HTML comments that carry a hash
//! of what strobe wrote, so a tool can
//! find it, replace it (`--update-pr`) and see when someone edited it by hand.

use super::fmt;

mod extract;
mod github;
mod implementations;
mod markdown;
mod summary;

use crate::model::{self, Better, Error, Result, VerdictKind};
use crate::stats;
use crate::store;
use extract::*;
pub(crate) use extract::{judged, public, public_value};
use github::*;
pub use github::{Updated, head_commit, hero_level, relevel, splice, update_pr};
pub use implementations::{claim, cpu_scope, factor_or_pct, fairness, range_x, ready_noun};
use markdown::*;
use serde_json::Value;
use std::collections::BTreeMap;
pub use summary::summary;

/// The blocks that measured `metric` in series `s`: a series that counts cpu.instructions apart from
/// its times (Linux perf events never run in a timing block, SPEC §7.5) keeps the other kind's blocks
/// in `pass` (`counters`: whether they are the counter blocks), and so does one that probes HTTP apart
/// from its times (SPEC §8.9; `http`: whether they are the probe blocks).
pub(crate) fn blocks_for<'a>(s: &'a Value, metric: &str) -> &'a Value {
    let pass = &s["pass"];
    let theirs = if pass["http"].is_boolean() { metric.starts_with("http.") == (pass["http"] == true) } else { (metric == "cpu.instructions") == (pass["counters"] == true) };
    if pass["blocks"].as_array().is_some_and(|b| !b.is_empty()) && theirs { &pass["blocks"] } else { &s["blocks"] }
}

/// The direction comparison `c` counted as better, as every report reads it: its own `better`; in a
/// result from before that field, for a `metric.<name>` the direction its blocks' lines gave (`s`, an
/// undecided one too); else `stats::better_in` (a better or worse verdict with its interval's side,
/// then the catalog).
pub(crate) fn better(s: Option<&Value>, c: &Value) -> Better {
    let m = c["metric"].as_str().unwrap_or("");
    if let Some(s) = s.filter(|_| m.starts_with("metric.") && !c["better"].is_string()) {
        let dirs: Vec<&str> = blocks_for(s, m).as_array().into_iter().flatten().filter_map(|b| b["directions"][m].as_str()).collect();
        match dirs.first() {
            Some(&"higher") if dirs.iter().all(|d| *d == "higher") => return Better::Higher,
            Some(&"lower") if dirs.iter().all(|d| *d == "lower") => return Better::Lower,
            _ => {}
        }
    }
    stats::better_in(c)
}

/// A bound or margin given as a fraction, as a verdict prints it (`2%`, `0.3%`, `1.5%`: never rounded
/// to another bound).
pub(crate) fn pct_bound(frac: f64) -> String {
    fmt::bound(frac, true, 0.0, "")
}

/// What a report calls base and head (SPEC §11.2), first that applies: `report --labels` (the
/// result's `side_names`); an implementation comparison's sides by name (SPEC §9.10); `Base (A/A)`
/// and `Head (A/A)` for an A/A run; "Before" and "This PR" when head is the working tree or `HEAD`
/// and, when the pull request is known (`--update-pr`: its `pr_head_commit`), its commit is the
/// pull request's head (uncommitted changes on top of it count); else each side's tag (`1982f5313f`, `origin/x@7bf4e2d566`),
/// or `Base` and `Head` when the tags do not tell them apart.
pub(crate) fn side_names(r: &Value) -> (String, String) {
    let pair = |b: &str, h: &str| (b.to_string(), h.to_string());
    if let Some([b, h]) = r["side_names"].as_array().map(Vec::as_slice)
        && let (Some(b), Some(h)) = (b.as_str(), h.as_str())
    {
        return pair(b, h);
    }
    let named = |i: usize| r["variants"][i]["name"].as_str().filter(|_| r["comparison"] == "implementations");
    if let (Some(b), Some(h)) = (named(0), named(1)) {
        return pair(b, h);
    }
    if r["plan"]["aa"] == true {
        return pair("Base (A/A)", "Head (A/A)");
    }
    if this_pr(r) {
        return pair("Before", "This PR");
    }
    let (b, h) = variants(r);
    let (tb, th) = (tag(&b), tag(&h));
    if tb.is_empty() || th.is_empty() || tb == th { pair("Base", "Head") } else { (tb, th) }
}

/// Whether head is the pull request's own change: the working tree (a folder, or a result from before
/// variants named their kind: `.` or a path) or `HEAD`, and, when the pull request is known, at its
/// head commit (uncommitted changes on top of it included; on another commit they are not its).
fn this_pr(r: &Value) -> bool {
    let (_, h) = variants(r);
    let spec = h["spec"].as_str().unwrap_or("");
    let tree = h["kind"] == "dir" || (h["kind"].is_null() && (spec == "." || local_path(spec))) || spec == "HEAD";
    let at_head = match r["pr_head_commit"].as_str() {
        None => true,
        Some(pr) => h["commit"].as_str().is_some_and(|c| c.len() >= 7 && pr.to_ascii_lowercase().starts_with(&c.to_ascii_lowercase())),
    };
    tree && at_head
}

/// The sides in a sentence: `before` and `this PR`, else the names `side_names` gives with the words
/// both start with left out (`compact_names`).
pub(crate) fn side_words(r: &Value) -> (String, String) {
    match side_names(r) {
        (b, h) if b == "Before" && h == "This PR" => ("before".into(), "this PR".into()),
        (b, h) => compact_names(&b, &h),
    }
}

/// `--labels <base>,<head>`: two distinct names, neither empty.
pub fn labels(arg: &str) -> Result<(String, String)> {
    let parts: Vec<&str> = arg.split(',').map(str::trim).collect();
    match parts.as_slice() {
        [b, h] if !b.is_empty() && !h.is_empty() && b != h => Ok((b.to_string(), h.to_string())),
        _ => Err(Error::usage(format!("--labels {arg:?}: two different names, base then head, separated by a comma (--labels v1.4,v1.5)"))),
    }
}

/// Side names for a label with little room: without the words both start with, and a rest that is
/// one parenthesis unwrapped (`Studio classic`, `Studio (native)` → `classic`, `native`). The full
/// names when they share no first word or that would leave either empty or both the same.
pub(crate) fn compact_names(base: &str, head: &str) -> (String, String) {
    let (b, h): (Vec<&str>, Vec<&str>) = (base.split_whitespace().collect(), head.split_whitespace().collect());
    let common = b.iter().zip(&h).take_while(|(x, y)| x == y).count();
    let rest = |w: &[&str]| {
        let s = w.join(" ");
        s.strip_prefix('(').and_then(|x| x.strip_suffix(')')).filter(|x| !x.contains(['(', ')'])).map(str::to_string).unwrap_or(s)
    };
    let full = (base.to_string(), head.to_string());
    if common == 0 || common >= b.len() || common >= h.len() {
        return full;
    }
    let (x, y) = (rest(&b[common..]), rest(&h[common..]));
    if x.is_empty() || y.is_empty() || x == y { full } else { (x, y) }
}

/// GitHub refuses a body over 65,536 characters (UTF-16 units); the block leaves most of it to the author.
pub const CAP: usize = 24_000;
/// How much a long report sheds, in order, until it fits `CAP` (`body` documents each step).
const SHED_MAX: u8 = 7;

/// The whole block: markers, the generated-by line, the report, the end line.
pub fn block(r: &Value) -> String {
    let id = r["id"].as_str().unwrap_or("");
    let mut content = String::new();
    for level in 0..=SHED_MAX {
        content = framed(r, level);
        if utf16(&content) <= CAP {
            break;
        }
    }
    if utf16(&content) > CAP {
        // Nothing left to shed but the series themselves: cut at a line, and say so.
        let mut cut = String::new();
        for line in content.lines() {
            if utf16(&cut) + utf16(line) > CAP - 400 {
                break;
            }
            cut += line;
            cut.push('\n');
        }
        content = format!("{cut}\n</details>\n\n<sub>The rest of this report did not fit a pull request: <code>strobe report experiment {id} --open</code>.</sub>");
    }
    let content = content.trim().to_string();
    format!("{}\n\n{content}\n\n{}\n", begin_marker(id, &digest(&content)), end_marker(id))
}

/// The report between its two quiet visible boundary lines.
fn framed(r: &Value, level: u8) -> String {
    let id = r["id"].as_str().unwrap_or("");
    // The bench file: a body with several blocks says which file each covers.
    let file = r["file_repo"].as_str().map(str::to_string).unwrap_or_else(|| repo_relative(r["file"].as_str().unwrap_or("")));
    let file = if file.is_empty() { String::new() } else { format!(" · {}", cell(&file)) };
    let top = format!("<sub>strobe · {id}{file} · generated</sub>");
    let bottom = format!("<sub>strobe · {id} · end of generated report</sub>");
    let body = if r["kind"] == "bisect" { bisect(r) } else { Doc::new(r, level).render() };
    // No machine's paths and no other process's name in a pull request, whatever a note or a refusal quoted.
    format!("{top}\n\n{}\n\n{bottom}", public(body.trim()))
}

/// `strobe 0.6.0 (a1b2c3d4e5)`: Cargo.toml's version and, for a build from a checkout, its commit, so
/// a build from main after a release does not pass for the release.
pub fn version() -> String {
    let commit = env!("STROBE_COMMIT");
    if commit.is_empty() { format!("strobe {}", env!("CARGO_PKG_VERSION")) } else { format!("strobe {}+{commit}", env!("CARGO_PKG_VERSION")) }
}

/// A bench file's path from its repository's root, with forward slashes; the file name when it is in
/// no repository (or no longer on this disk).
pub fn repo_relative(file: &str) -> String {
    let clean = file.trim_start_matches(r"\\?\").replace('\\', "/");
    let path = std::path::Path::new(&clean);
    // Looked for on this disk only: never another machine's path, never while a file a person named is
    // read (`store::may_look`); then its file name.
    let look = crate::store::may_look(file);
    for dir in path.ancestors().skip(1).filter(|_| look) {
        if dir.join(".git").exists()
            && let Ok(rel) = path.strip_prefix(dir)
        {
            return rel.to_string_lossy().replace('\\', "/");
        }
    }
    clean.rsplit('/').next().unwrap_or(&clean).to_string()
}

/// What the hero line and the caution say, for the card (`card.rs`) to draw the same: each a
/// (series, comparison).
pub struct Hero<'a> {
    /// The hero's wins: each celebrated series' main metric (a metric that won in several series is
    /// one phrase in the line and one row per series here), then the one extra tested metric.
    pub wins: Vec<(&'a Value, &'a Value)>,
    /// Everything decidedly worse: main and tested metrics.
    pub worse: Vec<(&'a Value, &'a Value)>,
    /// Late frames up by a point or more: (series, base share, head share, pairs worse, pairs).
    pub late: Vec<(&'a Value, f64, f64, usize, usize)>,
    /// The heading without a win or a regression.
    pub calm: String,
}

pub fn hero(r: &Value) -> Hero<'_> {
    let d = Doc::new(r, 0);
    let second = d.win_phrases().1;
    let mut keys: Vec<String> = Vec::new();
    let mut wins = Vec::new();
    for s in d.series.iter().copied() {
        let c = &s["primary"];
        if d.aa || !judged(s) || d.primary_out(s) != Out::Better || beyond_margin(c) {
            continue;
        }
        // The same groups as `win_phrases`, at most three.
        let key = format!("{}|{}", c["metric"].as_str().unwrap_or(""), d.hero_parts(c).1);
        if !keys.contains(&key) {
            if keys.len() == 3 {
                continue;
            }
            keys.push(key);
        }
        wins.push((s, c));
    }
    wins.extend(second);
    let (mut worse, mut late) = (Vec::new(), Vec::new());
    for s in d.series.iter().copied() {
        if d.aa || !judged(s) {
            continue;
        }
        if d.primary_out(s) == Out::Worse {
            worse.push((s, &s["primary"]));
        }
        worse.extend(tested(s).into_iter().skip(1).filter(|c| outcome(c, d.aa) == Out::Worse).map(|c| (s, c)));
        if let Some((b, h, k, n)) = late_frames(s, r)
            && h - b >= 0.01
        {
            late.push((s, b, h, k, n));
        }
    }
    Hero { wins, worse, late, calm: d.calm() }
}
