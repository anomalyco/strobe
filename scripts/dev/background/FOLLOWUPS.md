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
