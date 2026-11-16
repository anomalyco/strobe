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

Take head minus base **within each kept pair**. Use the same look, pass, pairs and Walsh interval ranks as the percentage. Do not subtract medians, multiply the percent by the median, or use the clean-pair sensitivity analysis. Negative time means less time. Positive memory means more memory. The verdict uses the existing comparison, never a second test. Omit the shift if the intervals lie on different sides of zero (touching zero counts as spanning it), or if the two nonzero estimates point opposite ways and the ratio estimate reaches its bound. Never print estimates with opposite printed signs.

| Magnitude of shift | Default unit | Other time units | Precision |
|---|---|---|---|
| 80 µs | µs | ms or s | Same decimal place for all three numbers, set by interval width |
| 4.2 ms | grouped µs | ms or s | 1–2 significant digits in the width |
| 738 ms | grouped µs | ms or s | Round whole places too; do not retain every µs of a wide interval |
| 18.3 s | grouped µs | ms or s | Same rule, no automatic switch to s |
| Bytes | Largest magnitude among the shift and its ends sets B, KB, MB or GB | No time-unit override | Keep one scale across estimate and ends |

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
| PR block / summary | Metrics table | Second line in Change and Likely range, including moved descriptive rows under Also measured. No extra row. Unchanged descriptive rows stay in the footer. |
| Single-tile card | Headline, medians, pair tally | Shift and range where the full line fits; tile description retains them. No extra text on a crowded tile. |
| Badge | Headline alone | No shift; cannot fit a second quantity and range |
| HTML | Forest row with percentage and range | A subordinate line under the metric |
| Result file / JSON | `estimate`, `lo`, `hi` on decision scale | Optional `shift: {estimate, lo, hi, unit}` in canonical units; no display rounding. Older files without the field stay readable. |

| Edge (pairs in ms unless noted) | Expected reading |
|---|---|
| Seven +0.5, one +150 on base 100 | `+500 [+500 … +80,000] µs`; retain the first significant digit on a one-sided interval |
| Identical pairs; a symmetric interval with floating residue | Unsigned `0` at the interval's precision |
| Interval endpoint exactly zero | Counts as containing zero; never manufacture a one-sided claim |
| Same time plant, baselines 100 and 200 | `+150,000 [+150,000 … +150,000] µs` |
| Four [1,3], four [120,100]; seven [1,3], three [500,400] | Opposite substantive direction or different interval sides: no field, text or label |
| Four [100,100.2], four [110,109.79] | Clean same: a small center rounds to unsigned zero, not an opposing direction |
| Shift 10,000 times smaller/larger than its baseline | Fixed chosen time unit; bytes scaled by the shift, not the baseline |
| Descriptive time and memory (`run`) | Terminal, Markdown, HTML, JSON and moved PR rows carry the same shift as a tested row |
| A ratio and shift that print opposite signs in even one unit | Omit the shift at computation, including from JSON; changing units cannot restore it |
| One-sided interval from +0.5 to +1 byte | `shift +1 [+<1 … +1] B`; an endpoint below the display resolution is not zero |
