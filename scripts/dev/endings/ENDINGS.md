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

**Loaded comparison in 0.39.0:** a consumer reading only
`series[].primary.verdict_kind` sees `same`. It does not see the separate
`Background` warning that load could hide a difference.

| Comparison | Baseline | Required ending |
|---|---|---|
| Warning impugns `same` or `no_worse` | Verdict alone looks clean; strict verdict exits 0 | Mark the metric `warned: true`, qualify the words; strict verdict exits 5, default 0 |
| Warning on a directional result | Direction survives contention | Preserve direction, carry warning with the metric |
| `--pairs 12`, one flagged block | 11 clean pairs ignored because 12 requested | Judge on clean pairs when at least eight remain |

## Evidence

### Downstream ending matrix (review baseline `5aeb2aa`)

These columns extend both tables above. Rows with identical downstream decisions
are grouped; a check never produces a timed verdict for bisect or confirmation.
The action column is `outputs.verdict / step exit` for one experiment; `fail-on:
never` reports rather than penalizes completed failures/refusals.

| Ending | HTML | Card / badge | PR text / step summary | Action output / exit | Bisect step | Track confirmation |
|---|---|---|---|---|---|---|
| Head, base, secondary refused, exact counters no worse | check passed, values | no timed claim | check table, counts | pass / 0 | — | — |
| Case defect, base failure, worse/unstable/missing counter, journey mismatch | failure + reason | no timed claim | fail, reason once | fail / 1 | — | — |
| Mixed passes with capability refusal, including dark display | pass and refusal cards | no timed claim | separate counts | pass / 0; strict unmet: fail / 1 | — | — |
| All refused, required coverage, no counter compared | **previously green; must show nonpass banner** | no timed claim | coverage reason first | fail / 1 | — | — |
| Some gaps, all gaps, all-gap counter check | **previously timed card; must show gap** | no timed claim | gap separate from not run | mixed pass / 0; inventory fail / 0; counter fail / 1 | — | — |
| Stopped/timeout with result | stop reason + completed cases | no completed claim | stop and number not run | fail / 1 (check) | no bound | not confirmed |
| Build, usage, connectivity or host error before result | no report | none | command error only | error / 1 | no bound | not confirmed |
| Clean same/no worse | equivalent, A/A passed only for same | same | equivalent/no worse | same or no_worse / 0 | may advance | not a regression |
| Warned same/no worse, load or drift | **previously confirmed; needs confirmation** | needs confirmation | needs confirmation, warned | **previously same / 0; unclear / 1 with fail-on unclear** | never advance; quiet retry only for load | inconclusive, actual reason |
| Directional shift, including small or beyond a margin, warned or clean | direction, warning if applicable | direction | direction and margin; **suffix previously damaged markup** | better / 0 or worse / 1 under regression policy | keep decisive, consistent direction; unread load blocks | confirm matching direction only |
| Undecided/underpowered | inconclusive | unclear | inconclusive | kind / 1 if strict | no decided move | not confirmed |
| One-sided load refusal | refusal | not measured | skipped | refused / 1 if strict | quiet wait and retry | inconclusive |
| Other refused primary / parity gap | refused or gap | not measured | not compared | refusal per policy | no direction | not confirmed |
| Max-time during comparison | partial counts and verdicts | finished result | finished result | from kept verdicts | use only valid step | use only valid comparison |

Other readers: `query summary`, `--format md` and MCP forward the CLI result;
`exp_insights` reads gap/refusal before check status; plan indexes usable blocks,
not check success. `family` and the implementation scoreboard consume the guarded
primary. Journey matching and counter comparisons produce the per-case status;
neither makes an independent experiment-exit decision. `stats` supplies the
machine kind and human words to extraction, card, bisect and track. Dev readers
(background, surfaces, next-commands, viewer-units, HTTP probe, endings) inspect
or generate evidence, not shipped verdicts. The action's regression boolean is
distinct from its verdict: an unclear comparison alone is not a regression.

`bun scripts/dev/endings/rows.ts` drives a headless script with `STROBE_BIN`.
`dark.ts` takes a separate binary built after applying `dark-block.patch` in a
disposable checkout. Its only substitution is the final display reading of a
block. No command changes the actual display or lock state.

| Linux known answer (CPU ms) | 0.39.0 | Candidate |
|---|---:|---:|
| Command, 300 ms | 302.322 | 303.093 |
| Command tree, 1,200 ms | 1,205.342 | 1,205.398 |
| Script spin, 300 ms wall | 302.894 (cold) | 299.906 (median of three) |
| Script child, 300 ms CPU | 303.740 | 304.214 |
| Windowless 4 ms tick at 50 Hz | 201.143 | 201.280 |
| Fixed-work page, two blocks, five runs each | RunIncomplete in 1/5 | 131.693; 131.505 |

Except for script spin, one run per control; these are not performance verdicts.
Warm script spin ranged from 298.769 to 305.889 ms; the VM was 97–100% idle and had no steal in the two preceding samples. VM load average at start of the original controls:
0.01 (baseline), 1.08 (candidate, after compilation). The command's own counters
still read 300 and 1,200. The native and script controls retain their planted work.

The background dev crate's `--replay <logs>` split stayed **40 warnings / 59 notes /
35 silent**: its historical replay already used eight pairs. `--fixed <results>`
instead replays the unrounded single-pass, fixed-design comparisons with the old
and new minimum. Among 16 comparisons: **9 warnings / 7 notes → 8 / 8**. Only
script CPU A/A changed level: its 9/12 clean pairs agreed on all five judged
metrics, with at most 0.01 percentage points of shift. Three notes gained direct
clean-pair evidence: terminal launch A/A (11/12; two metrics; 0.04 pp), page
fixed work A/B (9/12; five metrics; 0.12 pp), and script wall A/B (9/12; three
metrics; 0.13 pp). Other levels did not change. No pair, estimate or interval was
replaced by the clean subset.

`stop.ts` asks the real CLI to stop its own experiment and asserts exit 130 and
the saved and rendered ending. `resume.ts` starts a real headless timed pair,
then exercises comparison, warning assignment and finalization from unfinished block checkpoints: warned `same` /
`no_worse` (5 strict, 0 otherwise), warned directional (0), clean same (0),
unclear (0 or 5) and all refused (4). This checks policy and rendering, not the
ability to induce symmetric load on demand.

### Verified cells requiring review (no stamps)

After merging 0.40.0, `support-matrix --check` identifies six changed spans:
`src/stats.rs#compare` initializes `warned` to false; `src/app/bench.rs#run_chunk`
refreshes the Linux launch membership; `src/host/mod.rs#process_cycles_at`,
`#ended_marks`, `#impl EndedMarks`, and `src/host/cgroup.rs#read` check Linux process identities at both edges. The
statistics change no arithmetic, clock, interval, or counter value. The clean-pair
threshold changes the warning, not the pairs used for the estimate.

| Tripped cells | Hosts | Can a fixture number change because of this patch? |
|---|---|---|
| Page: frames, input latency | macOS | No: Linux edge code is not compiled here. |
| Windowless native: process CPU | Windows, macOS, Linux | Only on Linux if process membership changes at the edge; stable members use the same counters. |
| Windowless native: process memory | Windows, macOS, Linux | No memory reading changes. An accompanying Linux CPU reading has the edge qualification above. |
| Windowless native: hardware counters | macOS | No. |
| Script: wall and counts, process CPU | macOS, Linux | No change to wall or counts. Linux CPU can change for a zombie, reused PID or a helper joining or exiting at the edge. |
| Command: process CPU, process memory | Windows, Linux | No: the comparison constructor's new boolean changes no arithmetic, and the command's own clock and memory readings are unchanged. |
