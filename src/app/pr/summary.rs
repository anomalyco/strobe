//! `report --format summary` (SPEC §11.2): a result as a short GitHub-flavoured Markdown section for
//! a CI job's summary page (`$GITHUB_STEP_SUMMARY`). A check, a pixels run, a leak run and `bench
//! --blocks` are one table each; a comparison is its pull request block without images. A check runs
//! each case once and a leak run judges memory, so neither states a time: a check shows only exact
//! counters, which one run reads exactly.

use super::*;

/// The step summary of an experiment's result.
pub fn summary(r: &Value) -> String {
    let series: Vec<&Value> = r["series"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
    let aside = |s: &Value| s["kind"] == "refused" || s["not_run"] == true;
    let only = |kind: &str| series.iter().any(|s| s["kind"] == kind) && series.iter().all(|s| s["kind"] == kind || aside(s));
    let body = match r["kind"].as_str() {
        Some("check") => check(r, &series),
        Some("pixels") => pixels(r, &series),
        _ if only("leak") => leak(r, &series),
        _ if only("blocks") => blocks(r, &series),
        // A comparison: the PR block as `--format pr` writes it, with no card or badges (a step summary
        // attaches no images).
        _ => return block(r),
    };
    scrub_paths(&body)
}

/// `### strobe check · perf/app.bench.ts`: the section's heading, with the bench file.
fn heading(r: &Value, what: &str) -> String {
    let file = r["file_repo"].as_str().map(str::to_string).unwrap_or_else(|| repo_relative(r["file"].as_str().unwrap_or("")));
    if file.is_empty() { format!("### strobe {what}\n\n") } else { format!("### strobe {what} · {}\n\n", cell(&file)) }
}

/// The closing line: what the table does not say, the strobe that wrote it and the command that shows
/// the whole result.
fn footer(r: &Value, note: &str) -> String {
    let id = r["id"].as_str().unwrap_or("");
    format!("<sub>{note} {} · <code>strobe report {id}</code></sub>\n", version())
}

/// The series' case and target (`find · b` → `find`, `b`).
fn case_target(s: &Value) -> (String, String) {
    let label = s["label"].as_str().unwrap_or("");
    let case = s["case"].as_str().map(str::to_string).unwrap_or_else(|| short_label(label).to_string());
    let target = s["target"].as_str().map(str::to_string).or_else(|| label.rsplit_once(" · ").map(|(_, t)| t.to_string())).unwrap_or_default();
    (case, target)
}

/// Why a series did not pass, in a table cell: its refusal (`Validator: summary`), else its error.
fn why(s: &Value) -> String {
    let refusal = |r: &Value| Some(format!("{}: {}", r["validator"].as_str()?, r["summary"].as_str()?));
    let text = refusal(&s["refusal"])
        .or_else(|| s["error"].as_str().map(str::to_string))
        .or_else(|| s["gap"]["reason"].as_str().map(str::to_string))
        .or_else(|| refusal(&s["primary"]["refusal"]))
        .unwrap_or_default();
    cell(&first_clause(&text.replace(['\n', '\r'], " "), 180))
}

/// An exact counter: one run counts it exactly, so a check's single run reads it as any run would
/// (requests, bytes, the bench file's or page's counts, terminal frames, DOM nodes).
fn exact_counter(metric: &str) -> bool {
    metric == "dom.nodes" || model::metric(metric).is_some_and(|d| d.kind == model::Kind::Exact)
}

/// The warnings of the result worth a CI reader's look (warn level), each once, in a closed drawer.
fn warnings(r: &Value) -> String {
    let warns: Vec<&Value> = r["warnings"].as_array().into_iter().flatten().filter(|w| w["level"] == "warn").collect();
    if warns.is_empty() {
        return String::new();
    }
    let ids: Vec<&str> = warns.iter().filter_map(|w| w["id"].as_str()).fold(Vec::new(), |mut v, id| {
        if !v.contains(&id) {
            v.push(id);
        }
        v
    });
    let lines: Vec<String> = warns.iter().take(8).map(|w| format!("- **{}** {}", w["id"].as_str().unwrap_or(""), cell(&first_clause(w["text"].as_str().unwrap_or(""), 240)))).collect();
    let more = if warns.len() > 8 { format!("\n- and {} more", warns.len() - 8) } else { String::new() };
    format!("<details>\n<summary>{} warning{}: {}</summary>\n\n{}{more}\n\n</details>\n\n", warns.len(), if warns.len() == 1 { "" } else { "s" }, ids.join(", "), lines.join("\n"))
}

/// One row per case: whether it ran, its exact counters (base → head when both ran, with the
/// `--noworse` outcome), and why it did not pass. No times: one cold run measures none.
fn check(r: &Value, series: &[&Value]) -> String {
    let both = series.iter().any(|s| s["base_values"].is_object() || s["counters"].is_array());
    let (mut passed, mut failed, mut refused, mut other) = (0, 0, 0, 0);
    let mut rows = Vec::new();
    for s in series {
        let (case, target) = case_target(s);
        let state = s["state"].as_str().unwrap_or(match (s["kind"].as_str(), s["passed"].as_bool()) {
            (Some("refused"), _) => "refused",
            (Some("gap"), _) => "gap",
            (_, Some(true)) => "pass",
            (_, Some(false)) => "fail",
            _ => "",
        });
        let status = match state {
            "pass" => {
                passed += 1;
                "pass".to_string()
            }
            "fail" => {
                failed += 1;
                "**fail**".to_string()
            }
            "refused" => {
                refused += 1;
                "refused".to_string()
            }
            _ if s["not_run"] == true => {
                other += 1;
                "not run".to_string()
            }
            "gap" => {
                other += 1;
                "gap".to_string()
            }
            x => {
                other += 1;
                cell(x)
            }
        };
        let counters = counters_cell(s);
        let note = if state == "pass" { String::new() } else { why(s) };
        rows.push(format!("| {} | {} | {status} | {counters} | {note} |", cell(&case), cell(&target)));
    }
    let mut out = heading(r, "check");
    let total = series.len();
    let mut parts = vec![format!("**{passed} of {total} case{} passed**", if total == 1 { "" } else { "s" })];
    for (n, what) in [(failed, "failed"), (refused, "refused"), (other, "did not run")] {
        if n > 0 {
            parts.push(format!("{n} {what}"));
        }
    }
    let sides: Vec<String> = r["variants"].as_array().into_iter().flatten().filter_map(|v| Some(format!("{} {}", v["role"].as_str()?, code(v["label"].as_str()?)))).collect();
    if !sides.is_empty() {
        parts.push(sides.join(" · "));
    }
    out += &format!("{}\n\n", parts.join(" · "));
    let col = if both { "Exact counters (base → head)" } else { "Exact counters" };
    out += &format!("| Case | Target | Status | {col} | Notes |\n| --- | --- | --- | --- | --- |\n{}\n\n", rows.join("\n"));
    out += &warnings(r);
    out += &footer(r, "A check runs each case once to see that it works: its times are one cold run, not a measurement, so this summary states none; it shows the counters one run reads exactly.");
    out
}

/// A check series' exact counters: each `counters` row (base → head and its `--noworse` outcome)
/// when the check ran base and head, else head's values (with base's beside them when it has them).
fn counters_cell(s: &Value) -> String {
    let show = |m: &str, v: &Value| -> String { v.as_f64().map_or("—".into(), |x| disp(x, unit_of(m))) };
    // The metric and its value on one line.
    let pair = |m: &str, text: String| format!("{}\u{a0}{}", code(m), nb(&text));
    let mut parts: Vec<String> = Vec::new();
    if let Some(rows) = s["counters"].as_array() {
        for c in rows {
            let m = c["metric"].as_str().unwrap_or("");
            let mut p = pair(m, format!("{} → {}", show(m, &c["base"]), show(m, &c["head"])));
            if c["stable"] == false {
                p += " **unstable**";
            } else if c["passed"] == false {
                p += &format!(" **worse** (--noworse\u{a0}{})", cell(c["margin"].as_str().unwrap_or("")));
            }
            parts.push(p);
        }
    } else {
        let base = &s["base_values"];
        let mut keys: Vec<&String> = s["values"].as_object().into_iter().flat_map(|o| o.keys()).chain(base.as_object().into_iter().flat_map(|o| o.keys())).filter(|k| exact_counter(k)).collect();
        keys.sort();
        keys.dedup();
        for m in keys {
            let head = show(m, &s["values"][m.as_str()]);
            parts.push(if base.is_object() { pair(m, format!("{} → {head}", show(m, &base[m.as_str()]))) } else { pair(m, head) });
        }
    }
    const MAX: usize = 6;
    if parts.is_empty() {
        return "—".into();
    }
    let more = parts.len().saturating_sub(MAX);
    parts.truncate(MAX);
    if more > 0 {
        parts.push(format!("+{more} more"));
    }
    parts.join("<br>")
}

/// A pixels run: per case, how many of its frames differ beyond launch noise.
fn pixels(r: &Value, series: &[&Value]) -> String {
    let mut rows = Vec::new();
    let mut differ_cases = 0;
    for s in series {
        let (case, target) = case_target(s);
        let frames: Vec<&Value> = s["pixels"].as_array().into_iter().flatten().collect();
        let differ: Vec<&str> = frames.iter().filter(|p| p["state"] == "differ").filter_map(|p| p["name"].as_str()).collect();
        if !differ.is_empty() {
            differ_cases += 1;
        }
        let status = if aside_text(s).is_some() {
            why(s)
        } else if differ.is_empty() {
            format!("{} frame{} the same", frames.len(), if frames.len() == 1 { "" } else { "s" })
        } else {
            format!("**{} of {} differ**: {}", differ.len(), frames.len(), cell(&differ.iter().take(6).copied().collect::<Vec<_>>().join(", ")))
        };
        rows.push(format!("| {} | {} | {status} |", cell(&case), cell(&target)));
    }
    let mut out = heading(r, "pixels");
    out += &format!("**{differ_cases} of {} case{} {}**\n\n", series.len(), if series.len() == 1 { "" } else { "s" }, if differ_cases == 1 { "differs" } else { "differ" });
    out += &format!("| Case | Target | Frames |\n| --- | --- | --- |\n{}\n\n", rows.join("\n"));
    out += &warnings(r);
    out += &footer(r, "Frames compared pixel by pixel beyond each launch's own noise; no time is measured.");
    out
}

/// Why a series is beside the table's subject (refused before it ran, not run).
fn aside_text(s: &Value) -> Option<String> {
    (s["kind"] == "refused" || s["not_run"] == true || s["error"].is_string()).then(|| why(s))
}

/// A leak run: per case and side, the JS heap's (or a native app's host memory's) and the DOM's
/// growth per run with its interval and verdict, and the top suspect.
fn leak(r: &Value, series: &[&Value]) -> String {
    let mut rows = Vec::new();
    let (mut grows, mut judged) = (0, 0);
    for s in series {
        let (case, target) = case_target(s);
        if let Some(t) = aside_text(s).filter(|_| s["kind"] != "leak") {
            rows.push(format!("| {} | {} | {t} | — | — |", cell(&case), cell(&target)));
            continue;
        }
        let roles: Vec<&str> = ["base", "head"].into_iter().filter(|r| s["leak"][*r].is_object()).collect();
        if roles.is_empty() {
            rows.push(format!("| {} | {} | {} | — | — |", cell(&case), cell(&target), aside_text(s).unwrap_or_else(|| "not measured".into())));
            continue;
        }
        for role in &roles {
            let l = &s["leak"][*role];
            let target = if roles.len() > 1 { format!("{target} ({role})") } else { target.clone() };
            // A sign only on what shows a nonzero digit (`fmt::signed`).
            let signed = |x: f64, body: String| if !body.contains(|c: char| ('1'..='9').contains(&c)) { body } else if x < 0.0 { format!("−{body}") } else { format!("+{body}") };
            let bytes = |x: f64| signed(x, fmt::bytes(x.abs()));
            let nodes = |x: f64| signed(x, format!("{:.1}", x.abs()));
            // The verdict (it names a growth's slope), then the slope's 95% interval per run.
            let growth = |g: &Value, show: &dyn Fn(f64) -> String| -> String {
                if !g.is_object() {
                    return "—".into();
                }
                let verdict = g["verdict"].as_str().unwrap_or("");
                let verdict = if g["verdict_kind"] == "worse" { format!("**{}**", cell(verdict)) } else { cell(verdict) };
                format!("{verdict}<br>{}", nb(&format!("[{} … {}] per run", show(f(&g["lo"])), show(f(&g["hi"])))))
            };
            let (memory, dom, suspect) = if l["native_host"].is_object() {
                (growth(&l["native_host"], &bytes), "—".to_string(), "—".to_string())
            } else {
                let sus = &l["suspects"];
                let top = match sus["classes"].as_array().and_then(|c| c.first()) {
                    _ if sus["refused"].is_string() => "none named".to_string(),
                    Some(c) => format!("{} {} · {} retained", code(c["class"].as_str().unwrap_or("?")), nb(&format!("×{}", c["count"])), nb(&fmt::bytes(f(&c["retained_size"])))),
                    None => "none".to_string(),
                };
                (growth(&l["js_heap"], &bytes), growth(&l["dom_nodes"], &nodes), top)
            };
            let worse = ["native_host", "js_heap", "dom_nodes"].iter().any(|k| l[*k]["verdict_kind"] == "worse");
            judged += 1;
            if worse {
                grows += 1;
            }
            rows.push(format!("| {} | {} | {memory} | {dom} | {suspect} |", cell(&case), cell(&target)));
        }
    }
    let mut out = heading(r, "leak run");
    let cases = if judged == 1 { "case" } else { "cases" };
    out += &match grows {
        0 => format!("**No growth** in {judged} of {judged} {cases}\n\n"),
        n => format!("**Memory grows** in {n} of {judged} {cases}\n\n"),
    };
    out += &format!("| Case | Target | Memory per run | DOM nodes per run | Top suspect |\n| --- | --- | --- | --- | --- |\n{}\n\n", rows.join("\n"));
    out += &warnings(r);
    out += &footer(
        r,
        "A leak run judges memory left after each run (Theil–Sen slope over post-GC readings, Sen's 95% interval), not time; suspects are objects from the first half still alive at the end.",
    );
    out
}

/// `bench --blocks`: per case, the primary's median over the blocks with its 95% interval. One
/// build's values: nothing is compared.
fn blocks(r: &Value, series: &[&Value]) -> String {
    let mut rows = Vec::new();
    for s in series {
        let (case, target) = case_target(s);
        if s["kind"] != "blocks" {
            rows.push(format!("| {} | {} | — | {} | — |", cell(&case), cell(&target), aside_text(s).unwrap_or_default()));
            continue;
        }
        let p = &s["blocks_summary"]["primary"];
        let m = p["metric"].as_str().or(s["primary"]["metric"].as_str()).unwrap_or("");
        let u = unit_of(m);
        let got: Vec<f64> = p["values"].as_array().into_iter().flatten().filter_map(Value::as_f64).collect();
        let value = match (p["median"].as_f64(), p["lo"].as_f64(), p["hi"].as_f64()) {
            (Some(x), Some(lo), Some(hi)) => format!("{} {}", nb(&disp(x, u)), nb(&format!("[{} … {}]", disp(lo, u), disp(hi, u)))),
            _ if p["verdict"].is_string() => cell(p["verdict"].as_str().unwrap_or("")),
            _ if p["refused"].is_string() => format!("refused: {}", cell(&first_clause(p["refused"].as_str().unwrap_or(""), 120))),
            // One block: its value, with no interval.
            _ if got.len() == 1 => format!("{} (one block: no interval)", nb(&disp(got[0], u))),
            _ => "—".to_string(),
        };
        let n = s["blocks_summary"]["blocks"].as_u64().or_else(|| s["blocks"].as_array().map(|b| b.len() as u64)).unwrap_or(0);
        rows.push(format!("| {} | {} | {} | {value} | {n} |", cell(&case), cell(&target), code(m)));
    }
    let mut out = heading(r, "blocks");
    out += &format!("| Case | Target | Metric | Median [95% interval] | Blocks |\n| --- | --- | --- | --- | --- |\n{}\n\n", rows.join("\n"));
    out += &warnings(r);
    out += &footer(r, "One build's values over its blocks: nothing is compared, so no change is stated.");
    out
}
