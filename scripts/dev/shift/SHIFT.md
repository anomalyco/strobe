# Reading a paired shift

Read one line from each table. Their intersection defines the row; it does not change any verdict.

| Metric | Today | Extra quantity |
|---|---|---|
| Wall time, CPU time, latency percentile | Relative change and interval when all pairs are positive | Paired time shift |
| Memory, stored in bytes or MB | Relative change and interval | Paired memory shift |
| Count | Already a difference; stable counts can be exact | None; `+` means more, not faster |
| Rate, percent share, score | Ratio or difference (percentage points for a share) | None; not a duration or amount of memory |
| Frame interval / dropped fraction / number presented | Duration / share / count | Only the duration gets a shift, and only if measured |
| A time or memory compared by difference (e.g. baseline zero) | Already in the row's unit | Do not repeat the same three numbers |

Take head minus base **within each kept pair**. Use the same look, pass, pairs and Walsh interval ranks as the percentage. Do not subtract medians, multiply the percent by the median, or use the clean-pair sensitivity analysis. Negative time means less time. Positive memory means more memory. The verdict uses the existing comparison, never a second test. Raw differences and log ratios are different estimands: with varying baselines even their signs can differ. Label a disagreement rather than flipping a sign or promising equivalent tests.

| Magnitude of shift | Default unit | Other time units | Precision |
|---|---|---|---|
| 80 µs | µs | ms or s | Same decimal place for all three numbers, set by interval width |
| 4.2 ms | grouped µs | ms or s | 1–2 significant digits in the width |
| 738 ms | grouped µs | ms or s | Round whole places too; do not retain every µs of a wide interval |
| 18.3 s | grouped µs | ms or s | Same rule, no automatic switch to s |
| MB or GB | Row's memory scale | No time-unit override | Keep one scale across estimate and ends |

| Verdict | Today | Shift |
|---|---|---|
| Better / worse | Direction, possibly small | Estimate and interval, not another verdict |
| Same / no worse | Bound or margin | Estimate and interval, not an absolute equivalence bound |
| Underpowered / unclear / untested | No claim | Descriptive estimate and interval, at the coverage achieved by those pairs |
| Refused or no comparison | No defensible numbers | None, including JSON |
| Warned | Qualified verdict | Mark the shift `(warned)` too |

| Surface | Today | Placement |
|---|---|---|
| Terminal / Markdown | Side medians, change, range, verdict | Indented continuation immediately under the metric; no new column. New line ≤110 columns (`surfaces.ts` previously had no column assertion). |
| PR block / summary | Metrics table | A subordinate row with nonbreaking values; no wider main table |
| Single-tile card | Headline, medians, pair tally | Shift and range where the full line fits; tile description retains them. No extra text on a crowded tile. |
| Badge | Headline alone | No shift; cannot fit a second quantity and range |
| HTML | Forest row with percentage and range | A subordinate line under the metric |
| Result file / JSON | `estimate`, `lo`, `hi` on decision scale | Optional `shift: {estimate, lo, hi, unit}` in canonical units; no display rounding. Older files without the field stay readable. |
