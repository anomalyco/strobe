//! Other recordings named by an experiment's local summary. Keep their IDs as provenance;
//! only offer a command when the reader has the recording the renderer meant.

use super::*;

/// Try the result's own copy, then the exact store folder. A reader may decline a matching
/// recording (e.g. a copy without the requested view). Neither lookup is a use of the recording.
pub(in crate::app) fn owned_recording<T>(id: &str, source: Option<&Path>, exp: &str, mut read: impl FnMut(&Path, Value, bool) -> Option<T>) -> Option<T> {
    if exp.is_empty() || !id.starts_with("rec_") || !store::plain_id(id) {
        return None;
    }
    let mut take = |file: &Path, copy: bool| {
        let folder = file.parent()?;
        if copy {
            store::named_folder(folder).ok()?;
        }
        let meta = recording_meta(file).ok()?;
        (meta["id"] == id && meta["experiment"] == exp).then(|| read(folder, meta, copy)).flatten()
    };
    source
        .and_then(Path::parent)
        .and_then(|d| store::held_file(d, &format!("{id}/meta.json")))
        .and_then(|p| take(&p, true))
        .or_else(|| take(&crate::host::paths::state_path().join("recordings").join(id).join("meta.json"), false))
}

fn handle(id: &str, source: Option<&Path>, exp: &Value, verb: &str, args: &str) -> std::result::Result<String, &'static str> {
    let mut note = if source.is_some() { "recording not in this copy (query its folder directly if copied)" } else { "recording no longer stored here" };
    let answers = |folder: &Path, meta: Value| {
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
    owned_recording(id, source, exp.as_str().unwrap_or(""), |folder, meta, copy| {
        note = "recording here lacks the data for this view";
        answers(folder, meta).then(|| if copy { path_word(&folder.join("meta.json")) } else { id.to_string() })
    })
    .ok_or(note)
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
