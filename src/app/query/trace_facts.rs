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
    let dir = trace.parent().ok_or_else(|| Error::msg("trace has no recording folder"))?;
    let root = crate::host::paths::run().join("probe");
    let key = Sha256::digest(std::fs::canonicalize(dir)?.as_os_str().as_encoded_bytes());
    let file = root.join(format!("{key:x}.json"));
    let stamp = || -> Result<String> { Ok(format!("v1:{}:{}:{}", crate::host::disk::generation(trace)?, crate::host::disk::generation(&dir.join("meta.json"))?, meta["token"])) };
    let generation = stamp()?;
    let cached = || std::fs::read(&file).ok().and_then(|b| serde_json::from_slice::<Facts>(&b).ok()).filter(|f| f.generation == generation);
    if let Some(facts) = cached() {
        return Ok(facts);
    }
    std::fs::create_dir_all(&root)?;
    let lock = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(file.with_extension("lock"))?;
    lock.lock()?;
    if let Some(facts) = cached() {
        return Ok(facts);
    }
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
    if generation != stamp()? {
        return Err(Error::msg("the recording changed while its trace was being read; query it again"));
    }
    let facts = Facts { generation, runs };
    store::write_atomic(&file, &serde_json::to_vec(&facts)?)?;
    Ok(facts)
}
