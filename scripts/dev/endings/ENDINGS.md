# Endings in 0.39.0

A check asks whether the workload ran, not whether it got faster. Only a
counter check asks for a numerical comparison. A parity inventory can finish
with gaps; a counter check cannot succeed on gaps alone.

Baseline at `929d2700`. `P`, `F`, `R`, `G` mean `state/passed/decided` =
`pass/true/true`, `fail/false/false`, `refused/false/false`, `gap/absent/false`.
The JSON command returns the saved fields plus the path to the result. The
step summary counts passes out of *all listed* cases, with failed/refused/did
not run separately. The console denominator excludes gaps and refusals.
`—` means there is no result or step table; a command error gives the reason.

| Check ending | Case line | Console summary | Case | Top passed / exit | Step summary | Required change |
|---|---|---|---|---|---|---|
| All ran on head | pass + values | N of N passed; nothing compared | P | true / 0 | pass | none |
| Both builds ran, no counter gate | pass + base values | N of N passed; both checked, nothing compared | P | true / 0 | pass | none |
| Run threw, incomplete run, missing phase, ambiguous input | FAIL + reason | M of N passed | F | false / 1 | fail | keep run defects failures |
| Base failed a case | FAIL, base failed | M of N passed, K failed on base | F | false / 1 | fail | none |
| Build failed before first case | build error | — | — | — / 1 | — | none |
| Counter no worse | pass + counter row | only counters compared | P | true / 0 | pass, exact counter | none |
| Counter worse, unstable, missing or declared but not compared | FAIL + counter row | M of N passed | F | false / 1 | fail, counter reason | none |
| No named counter fired, or all were refused on both sides | pass, no counter row | N of N passed; nothing compared (exit 4) | P | **true / 4** | **N of N passed** | lead with nothing compared; top false |
| Secondary alone refused | pass + refused metric | N of N passed | P | true / 0 | pass | none |
| Some headless, windowless or unsupported primaries refused up front | refused + validator | M of M passed, K refused | R | true / 0 | refused | strict verdict must exit 4 |
| Script frame primary denied only in block | FAIL NotForTarget | M of N passed | F | false / 1 | fail | refuse before launch |
| All primaries denied by preflight | first refusal | — | — | — / 4 | — | retain case rows in a check |
| Dark headless page, wall/phase/span/CPU-share primary | **pass with no primary value** | N of N passed | **P** | **true / 0** | **pass** | wake if unlocked; if still off, R |
| Block discovers unsupported engine or headless mode | FAIL | M of N passed | F | false / 1 | fail | capability refusal, not a run defect |
| Session locked; input cannot be aimed | FAIL SessionLocked | M of N passed | F | false / 1 | fail | don't accept a primary stripped after the gate |
| Slow host in check (no primary removed) | pass + warning | N of N passed | P | true / 0 | pass + warning | not a timing verdict |
| All series refused after preflight | refused + reason | 0 of 0 passed, K refused | R | **true / 4** | 0 passed | top false |
| Some parity gaps; the rest pass | gap or pass | M of M passed, K gaps | G/P | true / 0 | gap/pass | strict parity or verdict: false / 4 |
| Only gaps, no counter gate | gap | 0 of 0 passed, K gaps | G | true / 0 | gap | parity inventory only; say nothing checked |
| Only gaps, counter gate | gap | 0 of 0 passed, K gaps | G | **true / 0** | gap | nothing compared, false / 4 |
| Required parity gap | gap | M of M passed, K gaps | G | **true / 4** | gap | top false; don't lead with passed |
| Journey mismatch | refused this journey | K where journeys differ | fail | false / 1 | fail | state and flags must agree |
| Nothing selected | flag error | — | — | — / 2 | — | no experiment |
| Invalid counter or incompatible flags | usage error, sometimes experiment header | — | — | — / 2 | — | no header or experiment directory |
| Stopped after some work / before first case | stopped, kept series / nothing ran | counts for completed rows | finished rows | false / 130 | counts, warning | lead with stopped |
| Deadline / unlock wait | timeout | stopped if result saved | finished rows | false / 124 or — / 124 | counts or — | stop takes precedence |

Case failure outranks unmet coverage. Stop outranks both. A whole-machine
refusal or device loss can end the command before it has a result (4 or 255).

| Timed comparison ending | Case and summary | Per-case JSON | Top passed | Exit | Step summary |
|---|---|---|---|---|---|
| Decided (including worse) | primary verdict, interval | verdict_kind, decided true | absent | 0 | comparison table |
| Undecided | unclear / underpowered | decided false | absent | 0; strict verdict 5 | comparison table |
| Primary refused before/during run | refused, reason | primary refused, decided false | absent | 0 beside a measured primary; all refused or strict verdict 4 | refusal |
| Gap / all gaps | not compared | kind gap, decided false | absent | 0; strict parity or verdict 4 | gap |
| Max-time in series loop | finished pairs, MaxTime | finished results | absent | normal comparison rules | finished results |
| Stopped / unlock wait | stopped, finished work | stopped.code | absent | 130 / 124 | finished results |
| Flag error | reason | — | absent | 2 | — |

**Loaded comparison (unchanged):** a consumer reading only
`series[].primary.verdict_kind` sees `same`. It does not see the separate
`Background` warning that load could hide a difference.
