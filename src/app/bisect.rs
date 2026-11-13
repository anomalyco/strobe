//! `strobe bench <file> --bisect <good>..<bad>`: the commit that moved a case's primary (SPEC §9.7).

use super::{Output, fmt, progress};
use crate::cli::Args;
use crate::config;
use crate::host::git::{self, short, subject};
use crate::model::{Error, Result, VerdictKind};
use crate::store;
use serde_json::{Value, json};
use std::path::Path;
use std::time::{Duration, Instant};

/// Walks `git rev-list --first-parent good..bad`. First benches `bad` against `good` to learn which
/// way the primary moved; then each step benches the middle commit against `good` with the same
/// settings. A step that moves the primary the way `bad` did becomes `bad`, any other outcome becomes
/// `good`, until one commit is left: the culprit, once it also moves the primary against its own
/// parent (benched when no step did). A step without a verdict can hide the culprit behind it, and a
/// culprit that does not move against its parent did not do it alone (a change built up over
/// commits), so the result names them and is `unclear` instead of found.
pub fn run(args: &Args, range: &str) -> Result<Output> {
    run_from(args, range, None, None)
}

/// A bisect whose step 0 (`bad` against `good`) already ran: a tracked change's paired confirmation
/// (SPEC §25.4) is that bench, so the bisect does not run it again. `target`: the bench file's target
/// whose commits it walks, in that target's own repository (`(name, root)`, an implementation
/// comparison's head, SPEC §25.7): each step benches `<name>=git:<commit>` on both sides.
pub(crate) fn run_from(args: &Args, range: &str, first: Option<&Output>, target: Option<(&str, &Path)>) -> Result<Output> {
    let file = args.pos.first().ok_or_else(|| Error::usage("bench needs a bench file"))?;
    let file = std::fs::canonicalize(file).map_err(|_| Error::usage(format!("no such bench file: {file}")))?;
    let root = target.map_or_else(|| config::project_root(file.parent().unwrap_or(Path::new("."))), |(_, r)| r.to_path_buf());
    let side = target.map(|(t, _)| t);
    let (good, bad) = range.split_once("..").ok_or_else(|| Error::usage("--bisect <good>..<bad>, for example --bisect v1.2..main"))?;
    let (mut good, mut bad) = (rev_parse(&root, good)?, rev_parse(&root, bad)?);
    // One seed for every step (`ctx.seed`, `ctx.random()`): each commit runs the same workload. `--seed`,
    // else step 0's when it already ran (a tracked change's confirmation), else one drawn now.
    let seed = match (args.get("seed"), first) {
        (None, Some(out)) => match out.json["plan"]["seed"].as_u64().and_then(|n| u32::try_from(n).ok()) {
            Some(n) => n,
            None => super::bench::seed_of(args)?,
        },
        _ => super::bench::seed_of(args)?,
    };
    let mut seeded = args.clone();
    seeded.set("seed", &seed.to_string());
    let args = &seeded;
    let id = args.get("id").map(str::to_string).unwrap_or_else(|| store::new_id("exp"));
    let dir = store::experiment_dir(&id)?;
    // Its steps' own scopes stack on it: `bench --stop <bisect>` stops the step that runs.
    let _scope = super::experiment_scope(&dir);
    let mut candidates = git::run(&root, &["rev-list", "--first-parent", "--reverse", &format!("{good}..{bad}")])?.lines().map(str::to_string).collect::<Vec<_>>();
    if candidates.is_empty() {
        return Err(Error::usage(format!("{range}: no commits from good to bad on the first-parent line")));
    }
    progress(format!("strobe bisect {range} · {} commits on the first-parent line · {id} · seed {seed}", candidates.len()));
    let line: Vec<Value> = candidates.iter().map(|c| json!({ "commit": c, "short": short(c), "subject": subject(&root, c) })).collect();
    let first_good = good.clone();
    let deadline = args.get("max-time").map(crate::cli::duration_ms).transpose()?.map(|ms| Instant::now() + Duration::from_millis(ms));
    let mut retries = 0;
    // Keep the first experiment (including its warnings) even when its retry settles the step.
    let mut settle = |mut s: Value, sign: Option<i8>| -> Result<Value> {
        let remaining = || deadline.map_or(Duration::MAX, |d| d.saturating_duration_since(Instant::now()));
        if retry_allowed(blocked(&s, sign), false, retries) && !remaining().is_zero() && !super::stop_requested() {
            let k = s["step"].as_u64().unwrap_or_default() as usize;
            retries += 1;
            progress(format!("step {k}: Background made the step inconclusive; waiting up to 60 s for quieter CPUs and retrying ({retries} of 2)"));
            crate::validity::wait_quiet(None, remaining().min(Duration::from_secs(60)));
            if super::stop_requested() {
                return Err(super::stop_error());
            }
            if remaining().as_millis() > 0 {
                let mut retry = args.clone();
                if deadline.is_some() {
                    retry.set("max-time", &format!("{}ms", remaining().as_millis().max(1)));
                }
                let mut next = step(&retry, &format!("{id}_retry"), &root, side, k, s["good"].as_str().unwrap(), s["commit"].as_str().unwrap())?;
                let warnings = s["warnings"].as_array().into_iter().flatten().chain(next["warnings"].as_array().into_iter().flatten()).cloned().collect();
                next["previous"] = s;
                next["warnings"] = Value::Array(warnings);
                s = next;
            }
        }
        s["blocked"] = json!(blocked(&s, sign));
        Ok(s)
    };
    let first = match first {
        Some(out) => step_json(out, &root, 0, &good, &bad)?,
        None => step(args, &id, &root, side, 0, &good, &bad)?,
    };
    let first = settle(first, None)?;
    let direction = moved(&first).filter(|_| first["blocked"] != true);
    let mut steps = vec![first];
    // Steps without a verdict, with the `good` each was benched against: the culprit may hide there.
    let mut undecided: Vec<(String, String)> = Vec::new();
    if let Some(sign) = direction {
        while candidates.len() > 1 {
            // The middle commit, never `bad` itself: `bad` is already known to have moved.
            let at = (candidates.len() - 1) / 2;
            let mid = candidates[at].clone();
            let s = settle(step(args, &id, &root, side, steps.len(), &good, &mid)?, Some(sign))?;
            if s["blocked"] == true {
                steps.push(s);
                break;
            }
            if moved(&s) == Some(sign) {
                bad = mid;
                candidates.truncate(at + 1);
            } else {
                if s["decided"] != true {
                    undecided.push((mid.clone(), good.clone()));
                }
                good = mid;
                candidates.drain(..=at);
            }
            steps.push(s);
        }
    }
    // A step that did not move the primary hands `good` on to its commit, so each later step is benched
    // against a newer `good`: a change that built up over several commits, each under what one step
    // resolves (a `same (±2%)`), lands on whichever commit tipped the sum, an innocent one. So the
    // culprit must move the primary against its own parent (the last `good`) too; when no step benched
    // that pair, one more does. Not moved there: no one commit did it, and the bisect is `unclear`.
    let mut not_alone: Option<Value> = None;
    if let Some(sign) = direction.filter(|_| !steps.iter().any(|s| s["blocked"] == true)) {
        let direct = steps.iter().any(|st| st["commit"] == bad.as_str() && st["good"] == good.as_str() && moved(st) == Some(sign));
        if !direct {
            progress(format!("\n{} moved it against an earlier commit: benched against its parent {} to check it did it alone", short(&bad), short(&good)));
            let s = settle(step(args, &id, &root, side, steps.len(), &good, &bad)?, Some(sign))?;
            if moved(&s) != Some(sign) {
                if s["decided"] == true {
                    not_alone = Some(s.clone());
                } else {
                    // No verdict against its parent: not known either way, like any undecided step.
                    undecided.push((bad.clone(), good.clone()));
                }
            }
            steps.push(s);
        }
    }
    let dirty = steps.iter().find(|s| s["blocked"] == true);
    let culprit = direction.filter(|_| not_alone.is_none() && dirty.is_none()).map(|_| json!({ "commit": bad, "short": short(&bad), "subject": subject(&root, &bad) }));
    // Where the move is known to be when no one commit did it: from the `good` the culprit last moved against.
    let moved_from =
        steps.iter().rev().find(|st| st["commit"] == bad.as_str() && direction.is_some_and(|d| moved(st) == Some(d))).and_then(|st| st["good"].as_str()).unwrap_or(&first_good).to_string();
    // With steps that had no verdict, the move may also hide behind the first of them: the range starts
    // at whichever `good` is earlier on the line (the first `good` is before every commit).
    let at = |c: &str| line.iter().position(|v| v["commit"] == c).map_or(-1, |p| p as i64);
    let from = match undecided.first() {
        Some((_, g)) if at(g) < at(&moved_from) => g.clone(),
        _ => moved_from.clone(),
    };
    // What the bisect found, for code (`track`, `--require-verdict`): the words below are for people.
    let outcome = match (&culprit, undecided.is_empty()) {
        _ if dirty.is_some() || steps[0]["decided"] != true => Outcome::Unclear,
        _ if not_alone.is_some() => Outcome::Unclear,
        (None, _) => Outcome::NoChange,
        (Some(_), true) => Outcome::Found,
        (Some(_), false) => Outcome::Unclear,
    };
    let verdict = match (&culprit, undecided.is_empty()) {
        _ if let Some(s) = dirty => format!("unclear: Background at step {}; the step could not settle {}..{} under load; rerun when quieter", s["step"], short(&good), short(&bad)),
        _ if steps[0]["decided"] != true => "unclear: the endpoints did not give a verdict".into(),
        // Not against its parent, but steps without a verdict: where the move is stays open.
        _ if not_alone.is_some() && !undecided.is_empty() => format!(
            "unclear: {} {} moved {} against {} but not against its parent {} ({}), and {} step(s) had no verdict ({}), so the move may be anywhere in {}..{}; more pairs (--pairs auto) settle it",
            short(&bad),
            subject(&root, &bad),
            steps[0]["metric"].as_str().unwrap_or(""),
            short(&moved_from),
            short(&good),
            not_alone.as_ref().map(crate::stats::verdict_text).unwrap_or_default(),
            undecided.len(),
            undecided.iter().map(|c| short(&c.0)).collect::<Vec<_>>().join(", "),
            short(&from),
            short(&bad)
        ),
        _ if not_alone.is_some() => format!(
            "unclear: {} {} moved {} against {} but not against its parent {} ({}): no one commit did it; it built up over {}..{} (each step under what one bench resolves), or more pairs settle it",
            short(&bad),
            subject(&root, &bad),
            steps[0]["metric"].as_str().unwrap_or(""),
            short(&moved_from),
            short(&good),
            not_alone.as_ref().map(crate::stats::verdict_text).unwrap_or_default(),
            short(&moved_from),
            short(&bad)
        ),
        (None, _) => format!("no change: {} did not move between good and bad ({})", steps[0]["metric"].as_str().unwrap_or(""), crate::stats::verdict_text(&steps[0])),
        (Some(_), true) => format!("found: {} {}", short(&bad), subject(&root, &bad)),
        (Some(_), false) => format!(
            "unclear: {} {} is the likeliest culprit (it moved, and no step before it did), but {} step(s) had no verdict ({}), so the culprit may be anywhere in {}..{}; more pairs (--pairs auto) settle it",
            short(&bad),
            subject(&root, &bad),
            undecided.len(),
            undecided.iter().map(|c| short(&c.0)).collect::<Vec<_>>().join(", "),
            short(&undecided[0].1),
            short(&bad)
        ),
    };
    // Where the culprit may still be: from the first undecided step's `good` (or the last `good`) to `bad`.
    // No one commit: the range the move built up over.
    let remaining = match &not_alone {
        _ if dirty.is_some() || steps[0]["decided"] != true => Some(json!({"good":good,"bad":bad})),
        Some(_) => Some(json!({ "good": from, "bad": bad })),
        None => culprit.as_ref().map(|_| json!({ "good": undecided.first().map_or(&good, |u| &u.1), "bad": bad })),
    };
    // Good against bad, as step 0 found it; a culprit that more pairs must still settle, or no one
    // commit, is unclear.
    let verdict_kind = if outcome == Outcome::Unclear { json!(VerdictKind::Unclear) } else { steps[0]["verdict_kind"].clone() };
    let result = json!({
        "warnings":steps.iter().flat_map(|s|s["warnings"].as_array().into_iter().flatten()).collect::<Vec<_>>(),
        "id": id, "kind": "bisect", "file": file, "range": range, "created": super::record::now_iso(),
        "metric": steps[0]["metric"], "series": steps[0]["series"], "steps": steps, "culprit": culprit, "verdict": verdict, "verdict_kind": verdict_kind,
        "outcome": outcome, "good": first_good, "commits": line, "remaining": remaining, "seed": seed,
    });
    let text = text(&result);
    let mut result = result;
    result["text"] = json!(text);
    store::write_json(&dir.join("result.json"), &result)?;
    let mut out = Output::new(text, result);
    if args.has("require-verdict") && outcome == Outcome::Unclear {
        out.code = 5;
    }
    Ok(out)
}

/// What a bisect found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Outcome {
    /// The one commit that moved the primary.
    Found,
    /// Good and bad did not differ: there is no culprit.
    NoChange,
    /// The likeliest culprit, behind steps without a verdict.
    Unclear,
}

impl Outcome {
    /// A bisect result's outcome.
    pub(crate) fn of(r: &Value) -> Option<Outcome> {
        serde_json::from_value(r["outcome"].clone()).ok()
    }
}

/// One bisect step: a full experiment of `head` against `base` with the bisect's own settings (of
/// `target` alone, on both sides, when it walks one target's commits).
fn step(args: &Args, id: &str, root: &Path, target: Option<&str>, k: usize, base: &str, head: &str) -> Result<Value> {
    let mut a = args.clone();
    a.remove("bisect");
    let side = target.map(|t| format!("{t}=")).unwrap_or_default();
    a.set("base", &format!("{side}git:{base}"));
    a.set("head", &format!("{side}git:{head}"));
    a.set("id", &format!("{id}_s{k}"));
    if !args.has("why") {
        // Profiled launches explain a move; a bisect only needs to know whether one happened.
        a.set("why", "never");
    }
    progress(format!("\nstep {k}  {} against {}", short(head), short(base)));
    let out = super::bench::run(&a).and_then(super::unless_halted)?;
    step_json(&out, root, k, base, head)
}

/// A step as the bisect keeps it, from the bench that ran it.
fn step_json(out: &Output, root: &Path, k: usize, base: &str, head: &str) -> Result<Value> {
    let series = out.json["series"].as_array().cloned().unwrap_or_default();
    if series.len() != 1 {
        return Err(Error::usage(format!("--bisect follows one series, and the file has {} here: narrow it with --filter, --target or --params", series.len())));
    }
    let p = &series[0]["primary"];
    let warnings = series[0]["warnings"].as_array().into_iter().flatten().chain(out.json["warnings"].as_array().into_iter().flatten()).filter(|w| w["level"] == "warn").collect::<Vec<_>>();
    for w in &warnings {
        progress(format!("  step {k}: ! {}  {}", w["id"].as_str().unwrap_or(""), w["text"].as_str().unwrap_or("")));
    }
    let s = json!({
        "series_background":contaminated(&series[0]), "unread":unread(&series[0]),
        "warnings":warnings,
        "step": k, "good": base, "commit": head, "short": short(head), "subject": subject(root, head),
        "experiment": out.json["id"], "series": series[0]["label"], "metric": p["metric"], "relative": p["relative"],
        "estimate": p["estimate"], "lo": p["lo"], "hi": p["hi"], "verdict": p["verdict"], "verdict_kind": p["verdict_kind"], "decided": p["decided"], "pairs": series[0]["pairs"],
        // So a reader draws an absolute time bound again in its own unit (`stats::verdict_text`).
        "bound": p["bound"], "noworse": p["noworse"],
    });
    progress(format!("  {}  {}", change(&s), crate::stats::verdict_text(&s)));
    Ok(s)
}

/// +1 or −1 when the step's primary moved with a verdict (its interval excludes 0), else None.
fn moved(s: &Value) -> Option<i8> {
    let (lo, hi) = (s["lo"].as_f64()?, s["hi"].as_f64()?);
    (s["decided"] == true).then_some(())?;
    if lo > 0.0 {
        Some(1)
    } else if hi < 0.0 {
        Some(-1)
    } else {
        None
    }
}

pub fn contaminated(s: &Value) -> bool {
    s["warnings"].as_array().into_iter().flatten().any(|w| w["id"] == "Background" && w["level"] == "warn")
}

pub(super) fn unread(series: &Value) -> bool {
    [&series["blocks"], &series["pass"]["blocks"]]
        .into_iter()
        .flat_map(|b| b.as_array().into_iter().flatten())
        .any(|b| b["values"]["background.unread"].is_number() || !b["values"]["cpu.foreign.max"].is_number())
}

pub fn blocked(s: &Value, sign: Option<i8>) -> bool {
    s["series_background"] == true && (s["unread"] == true || !matches!(s["verdict_kind"].as_str(), Some("better" | "worse")) || moved(s).is_none_or(|d| sign.is_some_and(|wanted| wanted != d)))
}

pub fn retry_allowed(blocked: bool, retried: bool, used: usize) -> bool {
    blocked && !retried && used < 2
}

pub fn inconclusive_confirmation(series: &Value, sign: i8) -> bool {
    let mut step = series["primary"].clone();
    if !step.is_object() {
        return contaminated(series);
    }
    step["series_background"] = json!(contaminated(series));
    step["unread"] = json!(unread(series));
    blocked(&step, Some(sign))
}

fn change(s: &Value) -> String {
    let f = |k: &str| s[k].as_f64().unwrap_or(f64::NAN);
    if s["relative"] == true {
        let (e, lo, hi) = fmt::pct_ci(f("estimate"), f("lo"), f("hi"));
        format!("{e} [{lo} … {hi}]")
    } else {
        // In the metric's unit: a time in the unit times print in (it printed bare `+0.42` before).
        let unit = crate::model::metric(s["metric"].as_str().unwrap_or("")).map_or("count", |d| if d.unit == "%" { "pp" } else { d.unit });
        let v = [f("estimate"), f("lo"), f("hi")];
        let v: Vec<String> = if unit == "ms" { fmt::times(&v, true) } else { v.iter().map(|x| fmt::signed(*x, unit)).collect() };
        format!("{} [{} … {}]", v[0], v[1], v[2])
    }
}

/// A bisect's text from its result.json (`query`, `report --format md` draw it again in the unit times
/// print in now).
pub(crate) fn text(r: &Value) -> String {
    let mut t = format!("strobe bisect {} · {} · {}\n", r["range"].as_str().unwrap_or(""), r["series"].as_str().unwrap_or(""), r["metric"].as_str().unwrap_or(""));
    // Columns as wide as their widest cell: a change in µs is 45 characters.
    let rows: Vec<Vec<String>> = r["steps"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| {
            vec![
                format!("step {}", s["step"]),
                format!("{} vs {}", s["short"].as_str().unwrap_or(""), short(s["good"].as_str().unwrap_or(""))),
                change(s),
                crate::stats::verdict_text(s),
                s["experiment"].as_str().unwrap_or("").to_string(),
                s["subject"].as_str().unwrap_or("").to_string(),
            ]
        })
        .collect();
    t += &crate::app::fmt::table(&rows, &[], "", 2);
    for s in r["steps"].as_array().into_iter().flatten() {
        for w in s["warnings"].as_array().into_iter().flatten() {
            t += &format!("  step {}: ! {}  {}\n", s["step"], w["id"].as_str().unwrap_or(""), w["text"].as_str().unwrap_or(""));
        }
    }
    t += &format!("\n{}\n", verdict_text(r));
    t
}

/// A bisect's conclusion (`no change: … (same (±40.0 µs))`) with each step verdict it quotes drawn again
/// as the step's own line draws it (`stats::verdict_text`), in the unit times print in now.
pub(crate) fn verdict_text(r: &Value) -> String {
    let mut v = r["verdict"].as_str().unwrap_or("").to_string();
    for s in r["steps"].as_array().into_iter().flatten() {
        let (old, new) = (s["verdict"].as_str().unwrap_or(""), crate::stats::verdict_text(s));
        if !old.is_empty() && old != new {
            v = v.replace(&format!("({old})"), &format!("({new})"));
        }
    }
    v
}

fn rev_parse(root: &Path, rev: &str) -> Result<String> {
    git::commit(root, rev).ok_or_else(|| Error::usage(format!("--bisect: {} is not a commit in {}", rev.strip_prefix("git:").unwrap_or(rev), root.display())))
}
