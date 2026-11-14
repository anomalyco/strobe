# Stage B: headless support evidence

**Result: 26 verified cells, 20 core, on this branch.** Round two added Linux page launch and script HTTP to the four Linux cells verified in round one. Neither new cell is core. No Windows cell passed the load rule. No command label or review stamp changed. Keep user-facing labels off until every cell in the headless starter set is verified on all three primary systems, not until a numerical quota is met.

| Host | Before (total / core¹) | Short list excluding commands | After (total / core¹) |
|---|---:|---:|---:|
| Windows x64 desktop | 4 / 4 | 6 | 4 / 4 |
| Linux x64 VM, two unpinned vCPUs | 6 / 6 | 4 | 12 / 9 |
| macOS arm64 laptop | 10 / 7 | 4 | 10 / 7 |
| Total | 20 / 17 | 14 | 26 / 20 |

¹ `Capability::core()` counts wall and counts, page engine, process CPU, process memory and terminal grid. The short list excludes the optional terminal and probe batches. The totals include them.

## Candidate cells

Each host in a row needs **its own** four readings. Commands are expanded below. Use a release binary whose reported SHA is on main. Record its identity, not the fixture checkout or the branch that records the evidence. Times below are fixture **targets**, not new results.

| Runtime · capability | Host | Batch | Known answer | A/A metric | Planted A/B | Boundary | Why repeat | Watch group |
|---|---|---|---|---|---|---|---|---|
| `chromium_page · launch` | Windows | L | marked spans within 0.5 ms of each plant; ready 0–20 ms after end | `launch.ready` | paired shift covers +200 ms | `launch.painted`: `Headless` | attach rewrite `7c01e766` | L |
| `chromium_page · launch` | Linux | L | same | same | same | same | same rewrite | L |
| `chromium_page · launch` | macOS | L | same | same | same | same | same rewrite | L |
| `chromium_page · wall_and_counts` | Windows | W | phases 2 × 10 and 40 ms; counts 7 and 5 exactly | `run.duration` | 100 → 200 ms | missing phase: `NoPhase` | prior A/A and A/B flagged; lean trace removed | W |
| `chromium_page · wall_and_counts` | Linux | W | same | same | same | same | lean trace and run marks removed (`ebc9eeaf`) | W |
| `chromium_page · wall_and_counts` | macOS | W | same | same | same | same | same trace change | W |
| `chromium_page · page_engine` | Windows | E | `main.work` 100 ms; 25 layouts and 25 styles | `main.work` | 100 → 200 ms | `main.script` on eval: `NotForTarget` | flagged comparisons; lean trace removed | E |
| `chromium_page · page_engine` | Linux | E | same | same | same | same | lean trace removed | E |
| `chromium_page · page_engine` | macOS | E | same | same | same | same | lean trace removed | E |
| `chromium_page · process_cpu` | Windows | C | `cpu2s` vs independent tree CPU (no lost processes), ≤2% | `cpu.work` | fixed 125M → 250M iterations | page `cpu.wakes` unavailable; check validator | flagged comparisons; old wall spin not fixed work | C |
| `chromium_page · process_cpu` | Linux | C | `cpu2s` ≈2000 ms, ≤2% at near-zero steal | same | same | same | old plant measured wall time on an unpinned VM | C |
| `chromium_page · process_cpu` | macOS | C | `cpu2s` ≈2000 ms, ≤2% | same | same | same | lean trace CPU removed | C |
| `script · wall_and_counts` | Windows | S | 300 ms + call overhead, ±1.5 ms per edge | `run.duration` | 150 → 300 ms | missing phase: `NoPhase` | flagged A/As; load gate changed | S + hooks |
| `script · process_cpu` | Windows | S | 300 ms CPU + host cost, ≤2% | `cpu.work` | 150 → 300 ms | script `cpu.wakes` or `io.read`: `NotForTarget` | same flagged A/As | S + tree |

## Commands and decision rule

Run from the release fixture checkout. Substitute `scripts/dev/known/planted.bench.ts` for `P`, `scripts/dev/known-web.bench.ts` for `W` and `scripts/dev/known/script.bench.ts` for `S`. Keep all page commands `--headless`, including deliberate refusals. Use `--power keep --reserve-cpus off` where host settings must not change. Take the host slot separately for each invocation, then release it; do not hold it during analysis or waits. No timed comparison on a desktop without its owner's permission.

| Batch | Single-build checks (`strobe bench … --head . --check`) | Paired commands (`strobe bench …`) |
|---|---|---|
| L | `P --headless --filter launch --target launch-400,launch-600`; `P --headless --filter launch --target launch-600 --primary launch.painted` | `P --headless --filter launch --head . --target launch-600 --aa`; `P --headless --filter launch --base launch-400 --head launch-600` |
| W, E | `W --headless --filter phases,spin100,layout`; `W --headless --filter phases --primary phase.absent`; inspect `main.script` refusal on `spin100` | For each `M=run.duration,main.work`: `W --headless --filter spin100 --head . --aa --primary M`; `P --headless --filter page --base page-a --head page-b --primary M` |
| C | `W --headless --filter cpu2s`; `P --headless --filter page-work --target work-0 --runs 3`; `W --headless --filter spin100 --primary cpu.wakes` | `P --headless --filter page-work --head . --target work-1x --aa`; `P --headless --filter page-work --base work-1x --head work-2x` |
| S | `S --filter spin --runs 3`; `scripts/dev/refused-series.bench.ts --filter no-phase`; inspect the script-only refusal in `spin` | For each `M=run.duration,cpu.work`: `P --filter spin --head . --target script-a --aa --primary M`; `P --filter spin --base script-a --head script-b --primary M` |

- Use at least eight completed pairs. Require `same (±2%)` for the cell's metric, not just for another primary. A descriptive secondary or an inconclusive A/A is not evidence.
- For an additive A/B, judge the paired 95% signed-rank interval on `head − base` in the metric's unit against Δ, or compare the relative interval with the range from an independent control interval `[L, H]`: `[100 × Δ/H, 100 × Δ/L]` for positive Δ and L. Never treat one sampled control value as exact. State the method and any fixture tolerance before running, and report all comparisons. For fixed iterations the expected ratio depends on the zero-loop and 1× costs; loop count is not an exact CPU duration. Exact counters must match. Known-answer CPU tolerance is 2%; wall tolerance is ±1.5 ms per edge.
- Round three uses paired milliseconds: +200 ms for later page launch (400 → 600; 40 pairs, no tolerance added), +300 ms for terminal launch, +150 ms for script, +60 ms for grid. For script and grid report exact containment and the existing fixture tolerance separately. On Windows use eight pairs per new leg; accept only unflagged runs. Keep the already clean terminal-launch A/B. For each short invocation wait up to ten minutes for a three-second CPU reading below 3%; do not move other processes.
- Record the actual refusal ID, metric and lack of a value. A failed invocation, a warning or refusal of a different target is not a boundary. Archive unsuccessful attempts too; never relax the tolerance after seeing a result.
- For load-sensitive metrics, accept zero flagged blocks, or the instrument's own comparison with **at least eight unflagged pairs**, same verdict, estimate shift ≤0.5 percentage points and truth-covering A/B half-width ≤2 percentage points. A spare-capacity note alone does not prove this. Record actual steal and load on the two-vCPU VM.
- Commands, including both preview wall-time cells and the seven existing verified cells, belong to the launcher follow-up. The Linux preview's 15–18 ms offset was fixed in 0.35.0. Replace that reason in the command round; it is not a current known-answer failure.

## Measuring paths

All cells depend on the common `WATCH` groups for paired statistics, warmup, block reduction, SDK run handling, load validation and the host clock. In addition:

- **L:** `bench/launch.rs` `once`, `observer`, `arm`, `Attacher`, `Watch`, `Observer`, `refusals`; `target/web.rs` `start_navigation`, `target/isolate.rs` `epoch_ms`; headless selection and refusal.
- **W:** `bench.rs` `run_chunk`, `phase_ms`, `phase_state`, headless/trace selection; `block.rs` `Hooks`, `per_run_hooks`; `metrics.rs` `phase_stretches`, trace marks. Both traced phases and untraced lean wall runs matter.
- **E:** `run_chunk` reads performance counters and rejects script time on evaluations; `WORK_PARTS`, `Driver::dispatch`, trace/lean selection and metric reduction.
- **C:** `run_chunk`, `record.rs` per-process clock and `timed_cpu`, OS process tree and ended-process accounting, role loss and target-specific refusal. Do not count the trace process or borrow a script's CPU proof.
- **S:** existing `script · *` watches (`chunk`, `Host::{spawn,request}`, SDK host call, `SelfCost`, `script_unread_refusal`). Add the existing hooks for wall/counts, or Windows tree, cycle and ended-process spans for CPU. Include the handler for the boundary actually observed.
- **D:** existing `cli · *` watches (`command`, `exec`, `block`, `Owned::run_to_exit`, launch/exit accounting and leftover detection), wall/count hooks and OS thread cycles. Also watch `Owned::{released_ns,creation_ns}` on all OSes, `impl Release` and cgroup `impl Launch` on Linux, and `host/jobs.rs` on Windows. A change to how a compound command is parsed can trip even a simple fixture: never stamp it by inference.

## Outside the short list

- No Windows headed page frames, canvas, pixels, input or windowed native cases on an occupied or locked desktop. No macOS windowed native/app cases, canvas, pixels or first interactive browser launch on a shared laptop. Prior headed macOS evidence stays separate.
- No display/GPU promotion on the Linux VM: virtual output is not a physical display. No mobile promotion without a physical phone. Do not substitute a desktop emulator.
- Windowless native CPU and memory were already verified on all three hosts. Linux native instruction counters still lack a proved counter-path refusal on the privileged VM; a guard on an unrelated metric is not a substitute.
- The extra headless batches use `known/tty.bench.ts` (`--filter keys --aa --target keys`, `--filter spaced --base keys --head keys-80`, `--filter ready --aa --target ready-500`, `--filter ready --base ready-200 --head ready-500`); check `overrun-120` and `ready --primary launch.painted` separately. Watch the terminal launch, tracker, latency pairing and block reducer. For `http-probe.bench.ts` use the fixture's token/hash setup, `--filter planted --aa --target probe-slow` and `--filter planted --base probe-fast --head probe-slow`; `status-500` must refuse latency. Watch the probe and `http_state`. This proves nothing about an HTTP probe in a different runtime.

## Round one: release results

All values here are from the v0.39.0 executables, not the earlier rehearsal. Times are ms, intervals are percent change. A blank comparison means **not measured**, not `same`.

| Linux cell | Known answer observed | A/A | Planted A/B vs independent truth | Refusal | Label |
|---|---|---|---|---|---|
| Page wall/counts | phases 41.69/40 and 23.02/20 (two and four edges); 7/5 counts exact | +0.09 [−0.06, +0.25], same, 12 pairs | +100.05 ms; +98.60 [98.43, 98.76] covers 98.466%, 32 pairs | `NoPhase` | **verified** |
| Page engine | main.work 101.29/100; layouts and styles 25/25 | −0.04 [−0.18, +0.21], same, 12 | +100.06 ms; +99.02 [98.81, 99.18] covers 98.944%, 12 | `NotForTarget` on eval script time | **verified** |
| Terminal grid | 20.2214/20; 10 frames, 400 cells, 620 bytes | −0.03 [−0.07, +0.11], same, 8 | +60.025 ms; +296.58 [296.00, 296.95] covers 296.715%, 8 | `AmbiguousInteraction` | **verified** |
| Terminal launch | 201.28 → 501.22; Δ299.94/300 | −0.002 [−0.021, +0.008], same, 12 | +300.05 ms; +149.12 [148.29, 149.23] covers 149.048%, 12 | `NotForTarget` on painted launch | **verified** |
| Page launch | 15 launches/side: 253.96 → 449.58, Δ195.61/200 (misses ±3) | −0.24 [−1.32, +0.66], same, 40 | +199.19 ms; +79.80 [76.95, 82.68] covers 78.75%, 40 | `Headless` | preview: independent known answer missed |
| Page CPU | 2006.40/2000; zero-loop control 0.486 | refused | refused; no 1×/2× estimate | `RunIncomplete` for ended processes; `NotForTarget` on wakes | preview |
| Script HTTP | 20.469 → 60.527 for 20 → 60; no timeout | +0.005 [−0.047, +0.106], same, 8 | +40.04 ms, but [195.47, 196.15] and [195.74, 196.25] both miss 195.414% | `RunIncomplete` for 3 of 9 invalid responses | preview |

For page wall and engine, the independent warm control was `P --head . --blocks 8 --runs 3 --filter page --target page-a --primary run.duration --headless`. Medians: 101.55785 ms wall and 101.06750 ms main work. The first 12-pair wall interval [98.469, 98.837] narrowly missed the control. The 32-pair confirmation covered it without rounding. The first 12-pair launch comparison read Δ203.85 ms; increasing it to 40 pairs did not fix the independent known answer. Every A/A and A/B cited for a promotion had zero flagged **kept** blocks. Terminal launch retried one pair; VM steal was at most 0.02% around the promoted comparisons.

| Windows cell | Release check | Boundary | Why preview |
|---|---|---|---|
| Page launch | 225.52 → 421.93, Δ196.41/200 | `Headless` | cold check outside ±3 ms; no A/A or A/B |
| Page wall/counts | counts 7/5 exact; both phases refused | `NoPhase`; `Throttled` with display off | no wall truth or pairs |
| Page engine | 100.43/100; 25 layouts/styles | `NotForTarget` on eval script time | no pairs |
| Page CPU | 2381.21 vs 2000 independent OS ms | `NotForTarget` on wakes | outside 2%; no pairs |
| Script wall/counts | 301.29 for 300 + host call | `NoPhase` | no pairs |
| Script CPU | 301.06 for 300 | `NotForTarget` on wakes/IO | no pairs |
| Terminal grid | 20.66/20; 10 frames, 400 cells | `AmbiguousInteraction` | no pairs |
| Terminal launch | 208.68 → 508.30, Δ299.63/300 | `NotForTarget` on painted launch | no pairs |
| Script HTTP | 21.77 → 61.77, Δ40.00/40 | `RunIncomplete` on HTTP 500 | no pairs |

Round one did not run Windows comparisons. No display state was changed. The macOS host was offline: no known answer, A/A, A/B or refusal was measured for its four page candidates or the optional terminal/probe batches. Do not transfer evidence between OSes.

The command watch additions expose omissions in four **existing review explanations**, not new post-review clock changes: Windows CPU and memory omit `jobs`; Linux CPU omits `Release`; Linux memory omits both `Release` and `Launch`. No review stamp was written.

## Round two: release results

All measurements used v0.39.0 (`929d2700`), including the checks with the revised fixtures. Preserve the round-one attempts above. For launch, subtract the two page milestones **within each launch**, then compare `launch.ready` across **matched pairs**. The unpaired 195.61 ms reading had a standard error of about 4.4 ms; it was not a clock calibration. The absolute paired intervals below use the signed-rank interval on the pairwise head-minus-base differences, not a conversion of the percentage interval.

| Linux cell | Known answer (ms) | A/A, % [95% interval] | Planted A/B | Boundary | Decision |
|---|---|---|---|---|---|
| Page launch | `launch.ready` 257.17 / 443.34; `plant_start` 47.47 / 38.94; marked spans 200.30 / 400.20; ready 9.40 / 4.20 after end | 40 pairs, −0.10 [−0.87, +0.92], same | 252.29 → 451.17 ms; paired +198.95 [194.02, 204.26] ms covers +200; +79.33 [76.69, 82.11]% | `launch.painted`: `Headless`, exit 4, no value | **Verified** |
| Script HTTP | 20.469 / 60.527 for 20 / 60; 15 probes each, no timeout | 8 pairs, +0.005 [−0.047, +0.106], same | All three intervals overlap the independent control range (below) | `status-500`: `RunIncomplete`, no p50 after 3 invalid replies in 9 | **Verified** |
| Page CPU | 2006.40 / 2000; zero-loop 0.486 | `RunIncomplete` | `RunIncomplete`, no estimate | `cpu.wakes`: `NotForTarget` | Preview until ended-process edge accounting is fixed and both pairs run |

The warm HTTP control (`--blocks 8 --runs 2 --filter planted --target probe-fast`) had median 20.44635 ms and a signed-rank interval of [20.397, 20.526] ms. For +40 ms this implies **[194.88, 196.11]%**. The 8-pair [195.47, 196.15]% and **both** 16-pair [195.74, 196.25]% and [195.55, 196.20]% intervals overlap it. Their paired shifts were +40.013 [39.995, 40.054], +40.042 [40.009, 40.071] and +40.045 [40.019, 40.074] ms: only the first strictly contains 40 ms. The percentage verdict uses control uncertainty, not exact containment by the last run. Both promotions had zero flagged kept blocks in A/A and A/B.

### Windows: readings under load, not promotions

All eight A/A primaries read `same (±2%)`. The instrument did not establish clean-pair agreement for any of them. The load column gives **machine CPU % at A/A and A/B start**, then **flagged kept blocks in A/A and A/B**. A low starting load does not establish a clean run. No repeats were selected to discard a busy result.

| Cell | Known answer (ms unless noted) | A/A, % [95% interval] | A/B: paired shift in ms [95% interval] | Boundary | Start load; flagged blocks |
|---|---|---|---|---|---|
| Page launch | marks 200.20 / 400.00; ready 9.70 / 3.70 later | +0.03 [−0.09, +0.35] | 220.89 → 417.99; paired +197.32 [195.57, 198.93] **misses +200 ms**; +89.18 [87.61, 90.30]% | `Headless` on painted | 23.2 / 21.9%; 31/48, 24/48 |
| Script wall | 301.29 for 300 + call | −0.024 [−0.102, +0.052] | +149.986 [149.910, 150.080] covers +150 | `NoPhase` | 19.5 / 14.2%; 5/24, 5/24 |
| Script CPU | 301.06 for 300 | +0.090 [−0.143, +0.161] | +149.887 [149.484, 152.050] covers +150 | `NotForTarget` on wakes/IO | 9.5 / 31.7%; 3/24, 14/24 |
| Page engine | 100.43 for 100; 25 layouts and 25 styles | +0.028 [−0.026, +0.133] | +100.006 [99.918, 100.117] covers +100 | `NotForTarget` on eval script time | 20.5 / 39.4%; 13/24, 17/24 |
| Page CPU | 2003.98 vs 2016 OS (−0.60%); median ended count 0 | −0.150 [−0.771, +0.708] | +93.40 [91.08, 95.89]; fixed iterations, not fixed ms. Relative half-width 2.49 pp | `NotForTarget` on wakes | 21.0 / 42.4%; 17/24, 3/24 |
| Terminal grid | 20.660 for 20; 10 frames and 400 cells | +0.015 [−0.096, +0.124] | +59.969 [59.891, 60.108] covers +60 | `AmbiguousInteraction` | 44.0 / 55.7%; 10/16, 3/16 |
| Terminal launch | 208.675 → 508.301; +299.626 for +300 | −0.061 [−0.191, +0.045] | +300.08 [299.55, 300.56] **covers +300**; the earlier percentage miss used one cold launch as an exact control | `NotForTarget` on painted | 0.5 / 0.0%; 1/24, 0/24 |
| Script HTTP | 21.766 / 61.768 for 20 / 60; no timeout | +0.002 [−0.059, +0.119] | +40.045 [40.008, 40.086]; relative [184.06, 185.14]% overlaps the warm control range [184.36, 185.24]% | `RunIncomplete` on HTTP 500 | 9.7 / 16.7%; 9/24, 11/24 |
| Page wall/counts | counts 7 / 5; no phase or wall value | not run | not run | `NoPhase`; `Throttled` with display off | skipped: needs awake display and truthful refused-primary check |

The first three-run `cpu2s` check, before changing the fixture, read 2016.69 vs 2000 OS ms (start load 9.9%). With both snapshots untimed, the three-run median read 2003.98 vs 2016 OS ms (start 51.3%). Both recover the known answer within 2%. Neither substitutes for qualified fixed-work pairs.

Independent controls (start load): script-a 151.32143 wall / 153.53680 CPU ms (16.8%); page-a 100.59750 main-work ms (24.2%); work-1x 92.64765 CPU ms (48.6%) and work-0 0.09204 (47.1%); probe-fast 21.673875 ms (15.9%). The marked launch check started at 21.6%, its refusal at 49.0%. For terminal known answers and non-launch boundaries see the release checks above. The script wall A/A illustrates why warnings matter: its JSON primary says `same`, but its load warning says the measured CPUs were too busy to call wall time the same.

**Watch result:** the added phase, count, trace-window, windowless, terminal pairing, event and selector helpers did not trip a verified cell. The new Linux launch and probe records also have their own watches. Only the four earlier command review explanations above fail the matrix check.

**Re-reading round one:** Linux page wall's initial 12-pair shift +100.079 [99.929, 100.242] ms already covered +100; the 32-pair shift +100.036 [99.939, 100.130] did too. The Linux page-engine (+100.054 [99.927, 100.178]), terminal grid (+60.021 [59.993, 60.050]) and terminal launch (+299.995 [299.322, 300.094]) plants still pass. Both early Linux ready A/Bs cover +200 ms ([192.72, 214.26] and [194.39, 203.96]); marks supplied the missing known answer, not a different A/B verdict. No Linux label changes. On Windows the clean terminal-launch A/B changes from miss to pass. Page launch still misses: after the 200 ms spin the frame wait had median 5.95 ms vs 3.30 ms after 400 ms. The mark clock was not short. No load warning is cleared by changing the arithmetic.

### When the Mac returns

Use the fixture checkout with the marks and a binary with a recorded release or commit. Take and release the host slot around **each** invocation. `P` and `W` below are the paths defined above; `T=scripts/dev/known/tty.bench.ts`, `H=scripts/dev/http-probe.bench.ts`. Prefix each command with `strobe bench` and expand the file abbreviation. Use `--power keep --reserve-cpus off --why never`. Compile `scripts/dev/known/known.rs` with `rustc -O` to a scratch `KNOWN_BIN` before the terminal batch. No headed or GPU work is requested.

| Mac cell | Commands (semicolon separates invocations) |
|---|---|
| Page wall and engine | `W --headless --head . --check --runs 3 --filter phases,spin100,layout`; `W --headless --head . --check --filter phases --primary phase.absent`; `P --headless --head . --blocks 8 --runs 3 --filter page --target page-a`. For **each** `M=run.duration` and `M=main.work`: `W --headless --head . --aa --filter spin100 --primary M --pairs 12 --runs 3`; `P --headless --filter page --base page-a --head page-b --primary M --pairs 12 --runs 3`. Check counts 7/5 and 25/25; check `main.script` refused on eval. |
| Page CPU | `W --headless --head . --check --filter cpu2s --runs 3`; `W --headless --head . --check --filter spin100 --primary cpu.wakes`; `P --headless --head . --check --filter page-work --target work-0 --runs 3`; `P --headless --head . --blocks 8 --runs 3 --filter page-work --target work-1x`; `P --headless --head . --aa --filter page-work --target work-1x --pairs 12 --runs 3`; `P --headless --filter page-work --base work-1x --head work-2x --pairs 12 --runs 3`. |
| Page launch | Run both L checks above. Run both L comparisons with `--pairs 40`. Read both mark widths, the ready-minus-end times and the signed-rank interval on the paired absolute changes. |
| Terminal grid and launch | `T --head . --check --filter keys,ready,overrun-120 --runs 3`; `T --head . --check --filter ready --target ready-500 --primary launch.painted`; `T --head . --aa --filter keys --pairs 8 --runs 2`; `T --filter spaced --base keys --head keys-80 --pairs 8 --runs 2`; `T --head . --aa --filter ready --target ready-500 --pairs 12`; `T --filter ready --base ready-200 --head ready-500 --pairs 12`. |
| Script HTTP | With the fixture's private `HTTP_PROBE_TOKEN_FILE` and `HTTP_PROBE_TOKEN_SHA256` (SHA-256 of `Bearer ` plus the trimmed token), run `H --head . --check --filter planted,status-500`; `H --head . --blocks 8 --runs 2 --filter planted --target probe-fast`; `H --head . --aa --filter planted --target probe-slow --pairs 8 --runs 2`; `H --filter planted --base probe-fast --head probe-slow --pairs 16 --runs 2`. Propagate the warm control interval for +40 ms. |

The **starter gate** requires the command (wall, CPU, memory), script (wall, CPU), page (wall, engine) and windowless native (CPU, memory) paths on each primary OS. Do not use unrelated launch, HTTP or headed cells to make up the numbers. Mac script and windowless-native evidence already exists. After the command planner lands, also run **Restore macOS verification** in its `scripts/dev/cli-plan/PLAN.md` on the planner binary: planner and leftover checks; `cli-spin`, `cli-alloc`, `cli-io` check/A/A/A/B; separate `cli-spin` wall-time A/A/A/B; `tree,orphan,breakaway-left` checks. Command measurements and labels remain with that package.
