//! Other recordings named by an experiment's local summary. Keep their IDs as provenance;
//! only offer a command when the reader has the recording the renderer meant.

use super::*;

fn handle(id: &str, source: Option<&Path>, exp: &Value, verb: &str, args: &str) -> std::result::Result<String, &'static str> {
    let missing = if source.is_some() { "recording not in this copy (query its folder directly if copied)" } else { "recording no longer stored here" };
    if !id.starts_with("rec_") || !store::plain_id(id) {
        return Err(missing);
    }
    let exp = exp.as_str().filter(|e| !e.is_empty()).ok_or(missing)?;
    let mut note = missing;
    let mut answers = |folder: &Path, meta: Value| {
        if meta["id"] != id || meta["experiment"] != exp {
            return false;
        }
        note = "recording here lacks the data for this view";
        if verb == "report recording" {
            return super::super::report::film(folder).is_some();
        }
        let mut words = args.split_ascii_whitespace();
        let Some(view) = words.next() else { return false };
        if view == "http" {
            return crate::probe::http_probe::read(folder).is_some();
        }
        let Ok(a) = Args::parse(["query", id, view].into_iter().chain(words).map(str::to_string).collect()) else { return false };
        a.check_command().is_ok() && recording_view(Rec::with_meta(folder, meta, None), view, &a, 15, true).is_ok()
    };
    if let Some(dir) = source.and_then(Path::parent)
        && let Some(meta) = store::held_file(dir, &format!("{id}/meta.json"))
        && let Some(folder) = meta.parent()
        && store::named_folder(folder).is_ok()
        && recording_meta(&meta).is_ok_and(|m| answers(folder, m))
    {
        return Ok(path_word(&meta));
    }
    let dir = crate::host::paths::recordings().join(id);
    store::read_json(&dir.join("meta.json")).is_ok_and(|m| answers(&dir, m)).then(|| id.to_string()).ok_or(note)
}

pub(in crate::app) fn recording_links(text: &str, source: Option<&Path>, exp: &Value) -> String {
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
            body.rsplit_once("strobe ").filter(|(p, _)| *p == "       " || p.ends_with(": ")).map(|(p, c)| (p, "", c))
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
        out += prefix;
        out += separator;
        let mut previous = None;
        let mut note = "";
        let repeated = format!("strobe {verb} {id} ");
        for segment in args.split(" · ") {
            let tail = segment.strip_prefix(&repeated).unwrap_or(segment);
            match handles.entry((id, verb, tail)).or_insert_with(|| handle(id, source, exp, verb, tail)) {
                Ok(h) => {
                    if previous.is_some() {
                        out += " · ";
                    }
                    // Native views share an operand; leak queries repeat it. A promoted first view
                    // and a view from a different folder always need the full command.
                    if previous.as_ref() != Some(h) || tail != segment {
                        out += &format!("strobe {verb} {h} ");
                    }
                    out += tail;
                    previous = Some(h.clone());
                }
                Err(why) => note = why,
            }
        }
        if previous.is_none() {
            if !prefix.split_whitespace().any(|w| w == id) && store::plain_id(id) {
                out += id;
                out += ": ";
            }
            out += note;
        }
        if line.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}
