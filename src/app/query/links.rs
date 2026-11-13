//! Other recordings named by an experiment's local summary. Keep their IDs as provenance;
//! only offer a command when the reader has the recording the renderer meant.

use super::*;

fn handle(id: &str, source: Option<&Path>) -> Option<String> {
    if !id.starts_with("rec_") || !store::plain_id(id) {
        return None;
    }
    if let Some(dir) = source.and_then(Path::parent)
        && let Some(meta) = store::held_file(dir, &format!("{id}/meta.json"))
        && let Some(folder) = meta.parent()
        && store::named_folder(folder).is_ok()
        && recording_meta(&meta).is_ok_and(|m| m["id"] == id)
    {
        return Some(path_word(&meta));
    }
    crate::host::paths::recordings().join(id).join("meta.json").is_file().then(|| id.to_string())
}

pub(in crate::app) fn recording_links(text: &str, source: Option<&Path>) -> String {
    let mut handles = BTreeMap::new();
    let mut out = String::with_capacity(text.len());
    let mut attribution = false;
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches('\n');
        // Match only renderer fields, not e.g. a log or warning containing the same command.
        let field = if ["      rec  ", "      why  ", "      video    ", "  rec  "].iter().any(|p| body.starts_with(p)) {
            body.rsplit_once(" · strobe ").map(|(p, c)| (p, " · ", c))
        } else if let Some(c) = body.strip_prefix("  next      strobe ") {
            Some(("  next      ", "", c))
        } else if attribution && body.starts_with("       ") {
            body.split_once("strobe ").filter(|(p, _)| *p == "       " || p.ends_with(": ")).map(|(p, c)| (p, "", c))
        } else {
            None
        };
        attribution = body.starts_with("why    attribution block") || attribution && field.is_some();
        let Some((prefix, separator, cmd)) = field else {
            out.push_str(line);
            continue;
        };
        let Some((verb, rest)) = ["query", "report recording"].iter().find_map(|v| Some((*v, cmd.strip_prefix(v)?.strip_prefix(' ')?))) else {
            out.push_str(line);
            continue;
        };
        let Some((id, args)) = rest.split_once(' ') else {
            out.push_str(line);
            continue;
        };
        let h = handles.entry(id).or_insert_with(|| handle(id, source));
        out += prefix;
        out += separator;
        if let Some(h) = h {
            // A leak line gives two queries of this same recording.
            let args = args.replace(&format!(" · strobe query {id} "), &format!(" · strobe query {h} "));
            out += &format!("strobe {verb} {h} {args}");
        } else {
            if !prefix.split_whitespace().any(|w| w == id) && store::plain_id(id) {
                out += id;
                out += ": ";
            }
            out += if source.is_some() { "recording not in this copy; supply its folder separately" } else { "recording no longer stored here; supply its folder separately" };
        }
        if line.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}
