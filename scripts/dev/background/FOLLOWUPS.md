# Boundaries after the fifth review

| Consumer | Keep | Do not substitute |
|---|---|---|
| PR: experiment warning, series note, unread or refused result | Its own scrubbed reason and level | Kept pairs from another series or pass, or a claim that zero blocks were flagged |
| PR: warned timing series with flagged pairs | Count the pairs of that series only | Global counts |
| Linux block context | Short inter-run spawn/IPC gaps; per-run evidence always stands | A silent-key timeout or a slow reset dominating the span (`wall > 10 × measured + 200 ms`) |
| Automatic / explicit / CCD affinity | Allowed CPUs; disjoint runner mask if one exists, otherwise inherit | Zero as “use the machine”; truncating an invalid range |
| Child with a requested pin | Apply the requested mask before release, or fail the launch | A running unpinned child after a setter failure |
| Track confirmation | Distinguish unread accounting from detected contention, both re-runnable | Permanent rejection on an inconclusive warned confirmation |
| Native and terminal measurement hints | `cpu.foreign` needs a pin, explicit or automatic | Assuming every unpinned run requested `--pin none` |
| Filmed check: CLI summary and Markdown, video and phone | Exact recording metadata from the result's source, without marking it used | Prefix match, made-up dimensions or frame counts, paths supplied by metadata |
| Recording details and query links after merging main | Same exact recording and experiment IDs; validate the copy first, then try the store. A link also needs data for its requested view. | Creating a state directory to look up a recording, or accepting metadata that the link reader would reject |
| macOS / standalone run / Windows terminal | `auto` and `none` remain unpinned on macOS; reject explicit placement. A standalone run has no runner. Both the identified console host and the child must accept the full mask. | Reporting a pin the OS did not apply, a nonexistent runner, or a partial mask |

### 0.37 consumer audit

| Source | Live diagnostic | Persisted / rendered reason |
|---|---|---|
| Native and page speed checks (first slow try, recovered note, final refusal, check conversion) | Busiest processes with PIDs and CPU | Count and busiest share; steal is numeric |
| One-sided retry / kept block | Busiest elsewhere | Incidence and CPU only; do not write names to progress.log |
| Reserve | Names of executables moved or failed | Mean moved per block; failed executable count, not a count of unique PIDs |
| WindowHidden / locked input / slot holder | Blocker can be identified locally | Do not quote another window, its owner, PID, or another bench's command |
| Ordinary SessionLocked, preflight Background, steal-only refusal, quota, thermal | Lock state or numeric measurement | Already contain no process list |
| Linux unresolved window | No observed load is implied | Retain per-block evidence; never infer quiet across independent launches. Above capacity report minimum duration, not an impossible CPU percentage |
| Main paired loop | Set-aside attempts use real wall time | Kept launches, set-aside launches and warmups are distinct; neither retries nor warmups enlarge the sample size |

Owner-only structured exception (SPEC §1 rule 9): `plan.reserve.moved` (name → peak number moved per block), `failed` (name → reason), block `background` ([name, share]) and recordings' busiest lists remain unchanged. The PR pairs footer deduplicates names across blocks to compute a count and peak; the HTML payload omits the names. See [the reader audit](PRIVACY.md) for the diagnostic boundary.
