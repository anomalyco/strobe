//! `strobe stats` (hidden, SPEC §5.11): the statistics and number formatting on numbers given on
//! stdin, one JSON request a line, one JSON answer a line. Known-answer checks of what a bench would
//! decide from those pairs (`scripts/dev/stats-check.ts`), and `simulate`: synthetic series through a
//! bench's own decision code (`scripts/dev/stats-sim.ts`). It reads and writes nothing else.

use super::{Output, fmt};
use crate::cli::Args;
use crate::model::{self, Error, Result};
use crate::stats;
use serde_json::{Value, json};
use std::io::BufRead;

mod simulate;

pub fn run(_args: &Args) -> Result<Output> {
    let (mut out, mut all) = (String::new(), Vec::new());
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let q: Value = serde_json::from_str(&line)?;
        let a = answer(&q).unwrap_or_else(|e| json!({ "error": e.message }));
        out += &a.to_string();
        out.push('\n');
        all.push(a);
    }
    // `--json`: the answers as one array, in order.
    Ok(Output::new(out, Value::Array(all)))
}

fn nums(v: &Value) -> Vec<f64> {
    v.as_array().into_iter().flatten().map(|x| x.as_f64().unwrap_or(f64::NAN)).collect()
}

fn answer(q: &Value) -> Result<Value> {
    let conf = q["confidence"].as_f64().unwrap_or(0.95);
    let looks: Vec<usize> = q["looks"].as_array().into_iter().flatten().filter_map(|x| x.as_u64().map(|n| n as usize)).collect();
    Ok(match q["op"].as_str().unwrap_or("") {
        "wilcoxon" => json!({ "p": stats::wilcoxon_p(&nums(&q["d"])) }),
        "hl" => {
            let (est, lo, hi, achieved) = stats::hodges_lehmann(&nums(&q["d"]), conf);
            json!({ "est": est, "lo": lo, "hi": hi, "achieved": achieved })
        }
        "theil_sen" => match stats::theil_sen(&nums(&q["ys"]), conf) {
            Some((m, lo, hi)) => json!({ "slope": m, "lo": lo, "hi": hi }),
            None => Value::Null,
        },
        "repeated_p" => json!({ "p": stats::repeated_p(&nums(&q["d"]), &looks) }),
        "family_min_pairs" => json!({ "pairs": stats::family_min_pairs(q["journeys"].as_u64().unwrap_or(1) as usize, &looks) }),
        "pairs_needed" => json!({ "pairs": stats::pairs_needed(&nums(&q["d"]), q["bound"].as_f64().unwrap_or(0.02), conf, q["power"].as_f64().unwrap_or(stats::FUTILITY_POWER)) }),
        "median_interval" => {
            let (m, lo, hi, achieved) = stats::median_interval(&nums(&q["v"]), conf);
            json!({ "median": m, "lo": lo, "hi": hi, "achieved": achieved })
        }
        "bound" => json!({ "text": fmt::bound(q["fraction"].as_f64().unwrap_or(f64::NAN), q["relative"] != false, q["abs"].as_f64().unwrap_or(f64::NAN), q["unit"].as_str().unwrap_or("")) }),
        "pct_ci" => {
            let (e, l, h) = fmt::pct_ci(q["est"].as_f64().unwrap_or(f64::NAN), q["lo"].as_f64().unwrap_or(f64::NAN), q["hi"].as_f64().unwrap_or(f64::NAN));
            json!({ "text": format!("{e} [{l} … {h}]") })
        }
        "pct_vs" => json!({ "text": fmt::pct_vs(q["r"].as_f64().unwrap_or(f64::NAN), q["threshold"].as_f64().unwrap_or(f64::NAN)) }),
        "pairs_for" => json!({ "pairs": stats::pairs_for(q["sigma"].as_f64().unwrap_or(f64::NAN), q["dist"].as_f64().unwrap_or(f64::NAN), conf, q["power"].as_f64().unwrap_or(0.8)) }),
        "compare" => {
            let name = q["metric"].as_str().unwrap_or("");
            let def = if q["higher"] == true { &model::METRIC_HIGHER } else { model::metric(name).ok_or_else(|| Error::usage(format!("no metric {name}")))? };
            let pairs: Vec<(f64, f64)> = q["pairs"].as_array().into_iter().flatten().map(|p| (p[0].as_f64().unwrap_or(f64::NAN), p[1].as_f64().unwrap_or(f64::NAN))).collect();
            let bound = q["bound"].as_f64().unwrap_or(0.02);
            let c = match q["noworse"].as_f64() {
                Some(m) => stats::compare_noworse(name, def, &pairs, m, conf),
                None => stats::compare(name, def, &pairs, bound, conf),
            };
            serde_json::to_value(&c)?
        }
        "simulate" => simulate::run(q)?,
        "pct" => json!({ "text": fmt::pct(q["r"].as_f64().unwrap_or(f64::NAN)) }),
        "value" => json!({ "text": fmt::value(q["v"].as_f64().unwrap_or(f64::NAN), q["unit"].as_str().unwrap_or("value")) }),
        other => return Err(Error::usage(format!("stats: no op {other}"))),
    })
}
