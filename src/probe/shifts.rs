//! The largest layout shifts of an attribution block and the elements that moved (`layout.shift`'s
//! explanation, SPEC §8.1). Read from the block's trace (`loading` category), with each moved node
//! named over CDP while the page is still open.

use crate::engine;
use crate::model::Result;
use crate::target::chromium::Page;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

/// The `top` largest shifts in the block's measured runs (from run `first`): score, run, time into
/// the run and moved elements (largest move first) with their rects before and after (CSS px,
/// `[x, y, w, h]`). The shifts and scores `layout.shift` sums (`metrics::read`): weighted as it is (a
/// frame's shift by its share of the page), none in the runs' untimed stretches; shifts right after
/// input count, as they do there.
pub fn largest(trace: &Path, token: &str, page: &Page, first: usize, top: usize) -> Result<Value> {
    // The run windows as the trace views read them (each with every renderer the page was in, `_wp`).
    let sql = format!(
        r#"
{windows}
select s.id, w.k, (s.ts - w.t0) / 1e6,
  coalesce(extract_arg(s.arg_set_id, 'debug.data.weighted_score_delta'), extract_arg(s.arg_set_id, 'debug.data.score')), a.key, a.display_value
  from slice s join thread_track tt on s.track_id = tt.id join thread t using(utid)
  join _wp w on t.upid = w.upid and s.ts >= w.t0 and s.ts < w.t1
  join args a on a.arg_set_id = s.arg_set_id and a.key glob 'debug.data.impacted_nodes*'
  where s.name = 'LayoutShift' and not exists (select 1 from _u u where s.ts >= u.t0 and s.ts < u.t1);
"#,
        windows = crate::app::query::windows_sql(token, None)
    );
    let table = engine::query(trace, &sql)?;
    // id → (run, at ms, score, node index → (node id, old rect, new rect))
    type Moved = BTreeMap<usize, (i64, [f64; 4], [f64; 4])>;
    let mut shifts: BTreeMap<String, (String, f64, f64, Moved)> = BTreeMap::new();
    let num = |c: &Option<String>| c.as_deref().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
    for row in &table.rows {
        let id = row[0].clone().unwrap_or_default();
        let index = row[1].as_deref().unwrap_or("").rsplit_once(":run:").and_then(|(_, i)| i.parse::<usize>().ok());
        let Some(index) = index.filter(|i| *i >= first) else { continue };
        let run = format!("run {index}");
        let entry = shifts.entry(id).or_insert_with(|| (run, num(&row[2]), num(&row[3]), BTreeMap::new()));
        // `debug.data.impacted_nodes[3].new_rect[1]`
        let key = row[4].as_deref().unwrap_or("");
        let Some(rest) = key.strip_prefix("debug.data.impacted_nodes[") else { continue };
        let Some((index, field)) = rest.split_once("].") else { continue };
        let Ok(index) = index.parse::<usize>() else { continue };
        let node = entry.3.entry(index).or_insert((0, [0.0; 4], [0.0; 4]));
        let value = num(&row[5]);
        let corner = |f: &str| f.trim_end_matches(']').rsplit('[').next().and_then(|i| i.parse::<usize>().ok()).filter(|i| *i < 4);
        match field {
            "node_id" => node.0 = value as i64,
            f if f.starts_with("old_rect[") => {
                if let Some(i) = corner(f) {
                    node.1[i] = value;
                }
            }
            f if f.starts_with("new_rect[") => {
                if let Some(i) = corner(f) {
                    node.2[i] = value;
                }
            }
            _ => {}
        }
    }
    let mut list: Vec<(String, f64, f64, Moved)> = shifts.into_values().collect();
    list.sort_by(|a, b| b.2.total_cmp(&a.2));
    let out: Vec<Value> = list
        .into_iter()
        .take(top)
        .map(|(run, at, score, moved)| {
            let mut nodes: Vec<(i64, [f64; 4], [f64; 4])> = moved.into_values().collect();
            nodes.sort_by(|a, b| distance(b).total_cmp(&distance(a)));
            let nodes: Vec<Value> = nodes.iter().map(|(id, from, to)| json!({ "node": describe(page, *id), "from": from, "to": to, "moved": distance(&(*id, *from, *to)) })).collect();
            json!({ "run": run, "at_ms": at, "score": score, "nodes": nodes })
        })
        .collect();
    Ok(Value::Array(out))
}

fn distance(n: &(i64, [f64; 4], [f64; 4])) -> f64 {
    ((n.2[0] - n.1[0]).powi(2) + (n.2[1] - n.1[1]).powi(2)).sqrt()
}

/// `tag#id.class[data-*]` of a backend node, or `removed` once the page let it go.
fn describe(page: &Page, id: i64) -> String {
    let Ok(r) = page.send("DOM.describeNode", json!({ "backendNodeId": id })) else { return "removed".into() };
    let node = &r["node"];
    let attrs: Vec<&str> = node["attributes"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let mut out = node["localName"].as_str().filter(|n| !n.is_empty()).unwrap_or(node["nodeName"].as_str().unwrap_or("?")).to_string();
    for pair in attrs.chunks(2) {
        match pair {
            ["id", v] => out += &format!("#{v}"),
            ["class", v] => out += &v.split_whitespace().take(3).map(|c| format!(".{c}")).collect::<String>(),
            [k, v] if k.starts_with("data-") && v.len() <= 40 => out += &format!("[{k}={v}]"),
            _ => {}
        }
    }
    out
}

/// One line for the bench's why section: the largest shift and what moved most in it.
pub fn headline(shifts: &Value) -> Option<String> {
    let s = shifts.as_array()?.first()?;
    let node = &s["nodes"][0];
    let (from, to) = (&node["from"], &node["to"]);
    let dy = to[1].as_f64().unwrap_or(0.0) - from[1].as_f64().unwrap_or(0.0);
    let dx = to[0].as_f64().unwrap_or(0.0) - from[0].as_f64().unwrap_or(0.0);
    let way = if dy.abs() >= dx.abs() { format!("{} {:.0} px", if dy > 0.0 { "down" } else { "up" }, dy.abs()) } else { format!("{} {:.0} px", if dx > 0.0 { "right" } else { "left" }, dx.abs()) };
    Some(format!(
        "largest layout shift {:.3} ({}, {} in): {} moved {way}{}",
        s["score"].as_f64().unwrap_or(0.0),
        s["run"].as_str().unwrap_or(""),
        crate::fmt::time(s["at_ms"].as_f64().unwrap_or(0.0)),
        node["node"].as_str().unwrap_or("?"),
        match s["nodes"].as_array().map_or(0, Vec::len) {
            0 | 1 => String::new(),
            n => format!(" (+{} more)", n - 1),
        }
    ))
}

/// The `shifts` view of `query`.
pub fn text(shifts: &Value) -> String {
    let Some(list) = shifts.as_array().filter(|l| !l.is_empty()) else { return "no layout shifts in the measured runs\n".into() };
    // `at` as wide as its widest time (`18,309,038 µs`).
    let at: Vec<String> = list.iter().map(|s| crate::fmt::time(s["at_ms"].as_f64().unwrap_or(0.0))).collect();
    let w = at.iter().map(|a| a.chars().count()).max().unwrap_or(2).max(2);
    let mut out = format!("score   run      {:<w$}  moved\n", "at");
    for (s, at) in list.iter().zip(&at) {
        out += &format!("{:<7.4} {:<8} {at:>w$}", s["score"].as_f64().unwrap_or(0.0), s["run"].as_str().unwrap_or(""));
        for (i, n) in s["nodes"].as_array().into_iter().flatten().take(5).enumerate() {
            let r = |v: &Value| format!("{:.0},{:.0} {:.0}×{:.0}", v[0].as_f64().unwrap_or(0.0), v[1].as_f64().unwrap_or(0.0), v[2].as_f64().unwrap_or(0.0), v[3].as_f64().unwrap_or(0.0));
            out += &format!(
                "{}{} {:.0} px  ({} → {})\n",
                if i == 0 { "  ".to_string() } else { " ".repeat(19 + w) },
                n["node"].as_str().unwrap_or("?"),
                n["moved"].as_f64().unwrap_or(0.0),
                r(&n["from"]),
                r(&n["to"])
            );
        }
    }
    out
}
