//! The three source choices that need trace contents. A cold listing reads the trace once, not
//! once per view or case. A hot listing reads only the files' generation and this small result.

use super::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Serialize, Deserialize)]
pub(super) struct Facts {
    generation: String,
    runs: BTreeMap<String, bool>, // run key → a tracked invalidation in its timed part
}

impl Facts {
    pub(super) fn runs(&self) -> bool {
        self.runs.keys().any(|k| k.starts_with("run:"))
    }

    pub(super) fn measured(&self, meta: &Value, style: bool) -> bool {
        let explicit = rec::measured_keys(meta).is_some();
        select_runs(meta, self.runs.keys().filter(|k| explicit || k.starts_with("run:")).cloned()).iter().any(|k| !style || self.runs[k])
    }
}

pub(super) fn read(trace: &Path, meta: &Value) -> Result<Facts> {
    let Some(dir) = trace.parent().and_then(|p| p.canonicalize().ok()) else { return compute(trace, meta) };
    let root = crate::host::paths::run().join("probe");
    let key = Sha256::digest(dir.as_os_str().as_encoded_bytes());
    let file = root.join(format!("{key:x}.json"));
    let stamp = || Some(format!("v2:{}:{}:{}:{}", crate::host::disk::generation(trace)?, crate::host::disk::generation(&dir.join("meta.json"))?, meta["token"], meta["probes"]));
    let lock = || -> std::io::Result<std::fs::File> {
        std::fs::create_dir_all(&root)?;
        let f = std::fs::File::options().read(true).write(true).create(true).truncate(false).open(file.with_extension("lock"))?;
        f.lock()?;
        Ok(f)
    };
    let (Ok(_lock), Some(generation)) = (lock(), stamp()) else { return compute(trace, meta) };
    if let Some(facts) = std::fs::read(&file).ok().and_then(|b| serde_json::from_slice::<Facts>(&b).ok()).filter(|f| f.generation == generation) {
        return Ok(facts);
    }
    let mut facts = compute(trace, meta)?;
    if let Some(after) = stamp() {
        if generation != after {
            return Err(Error::msg("the recording changed while its trace was being read; query it again"));
        }
        facts.generation = generation;
        if let Ok(bytes) = serde_json::to_vec(&facts) {
            let _ = store::write_atomic(&file, &bytes);
        }
    }
    Ok(facts)
}

fn compute(trace: &Path, meta: &Value) -> Result<Facts> {
    let token = meta["token"].as_str().unwrap_or("");
    let prefix = format!("strobe:{token}:");
    let tracked = if meta["probes"] == "attribution" { rec::TRACKED_RUN } else { "0" };
    let windows = rec::all_run_windows_sql(token);
    let table = engine::query(trace, &format!("{windows}select k, max({tracked}) from _w group by k"))?;
    let mut runs = BTreeMap::new();
    for row in table.rows {
        if let Some(k) = row[0].as_deref().and_then(|s| s.strip_prefix(&prefix)) {
            runs.insert(k.to_owned(), row[1].as_deref() == Some("1"));
        }
    }
    Ok(Facts { generation: String::new(), runs })
}
