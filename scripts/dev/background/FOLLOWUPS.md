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
