//! The only authorities on device identity are phone records and explicit mobile selectors.
//! Neither a user parameter named `serial` nor a metric, commit or case label is a selector.
use serde_json::Value;
use std::collections::BTreeSet;

const HIDDEN: &str = "[device]";

pub(super) fn omit_device_ids(data: &mut Value) {
    let mut ids = BTreeSet::new();
    let mut take = |v: &mut Value| {
        if let Value::String(s) = v
            && !s.is_empty()
            && s != HIDDEN
        {
            let id = std::mem::replace(s, HIDDEN.into());
            ids.insert(crate::host::adb::key_of(&id, None));
            ids.insert(id);
        }
    };
    fn phone(v: &mut Value, take: &mut impl FnMut(&mut Value)) {
        for k in ["serial", "hardware_serial", "hw_serial", "udid", "short_udid"] {
            if let Some(id) = v.get_mut(k) {
                take(id);
            }
        }
    }
    if let Some(p) = data.get_mut("phone") {
        phone(p, &mut take);
    }
    for ptr in ["/plan/mobile", "/args/flags/mobile"] {
        if let Some(v) = data.pointer_mut(ptr) {
            take(v);
        }
    }
    for s in data.get_mut("series").and_then(Value::as_array_mut).into_iter().flatten() {
        if let Some(v) = s.get_mut("mobile") {
            take(v);
        }
        for ptr in ["/blocks", "/pass/blocks"] {
            for b in s.pointer_mut(ptr).and_then(Value::as_array_mut).into_iter().flatten() {
                if let Some(p) = b.get_mut("phone") {
                    phone(p, &mut take);
                }
            }
        }
    }
    if !ids.is_empty() {
        display_fields(data, &ids);
    }
}

// Match whole tokens, never part of a word, path or SHA. The slash in a WIR handle is a
// separator only after `wir:`. Scan the original once so the replacement is never input.
fn tokens(s: &str, ids: &BTreeSet<String>) -> String {
    let sep = |c: char| c.is_whitespace() || "\"'`()[]=,;".contains(c);
    let mut out = String::new();
    let mut at = 0;
    while at < s.len() {
        let rest = &s[at..];
        let wir = s[..at].ends_with("wir:");
        let android = s[..at].ends_with("android:");
        if (at == 0 || wir || android || s[..at].ends_with(sep))
            && let Some(id) = ids.iter().filter(|id| rest.starts_with(id.as_str())).find(|id| {
                let tail = &rest[id.len()..];
                tail.is_empty() || tail.starts_with(sep) || tail.starts_with(": ") || tail.starts_with(". ") || tail == "." || (wir && tail.starts_with('/')) || (android && tail.starts_with(':'))
            })
        {
            out.push_str(HIDDEN);
            at += id.len();
        } else {
            let c = rest.chars().next().unwrap();
            out.push(c);
            at += c.len_utf8();
        }
    }
    out
}

fn display_fields(v: &mut Value, ids: &BTreeSet<String>) {
    fn text(v: &mut Value, ids: &BTreeSet<String>) {
        if let Value::String(s) = v {
            *s = tokens(s, ids);
        }
    }
    match v {
        Value::Array(a) => a.iter_mut().for_each(|x| display_fields(x, ids)),
        Value::Object(o) => {
            for (k, x) in o {
                match k.as_str() {
                    // User and measurement data: even their nested keys can look like diagnostic fields.
                    "params" | "values" | "counts" | "metrics" | "env" | "flags" => {}
                    "target" if x.as_str().is_some_and(|s| s.starts_with("wir:")) => text(x, ids),
                    "text" | "detail" | "summary" | "message" | "why" | "fix" | "hint" | "error" | "note" | "care_note" | "browser" | "handle" | "command" | "rerun" | "frames_refused"
                    | "presents_refused"
                        if x.is_string() =>
                    {
                        text(x, ids)
                    }
                    "notes" | "handles" | "next" | "changed" => {
                        if let Some(a) = x.as_array_mut() {
                            for item in a {
                                if item.is_string() { text(item, ids) } else { display_fields(item, ids) }
                            }
                        } else {
                            text(x, ids)
                        }
                    }
                    "refused" => {
                        if let Some(o) = x.as_object_mut() {
                            for item in o.values_mut() {
                                if item.is_string() { text(item, ids) } else { display_fields(item, ids) }
                            }
                        } else {
                            text(x, ids)
                        }
                    }
                    _ => display_fields(x, ids),
                }
            }
        }
        _ => {}
    }
}
