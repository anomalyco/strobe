# Stage B: headless support evidence

**Result: four Linux cells passed on v0.39.0 (`929d2700`). The matrix labels 24 cells verified, 20 core.** Four pre-existing command labels still need review under the new watches; the branch is not ready to land. No command labels or stamps changed. The matrix check before extending `WATCH` passed (114 rows on 6 hosts).

| Host | Before (total / core¹) | Short list excluding commands | After (total / core¹) |
|---|---:|---:|---:|
| Windows x64 desktop | 4 / 4 | 6 | 4 / 4 |
| Linux x64 VM, two unpinned vCPUs | 6 / 6 | 4 | 10 / 9 |
| macOS arm64 laptop | 10 / 7 | 4 | 10 / 7 |
| Total | 20 / 17 | 14 | 24 / 20 |

¹ `Capability::core()` counts wall and counts, page engine, process CPU, process memory and terminal grid. The original 16-cell plan (including both command wall-time cells) also counted page launch toward its **headless cohort**: its projected 33 in that cohort would have been only 30 `CORE` cells. The gate of 31 core cells is not met.

## Candidate cells

Each host in a row needs **its own** four readings. Commands are expanded below. Use a release binary whose reported SHA is on main. Record its identity, not the fixture checkout or the branch that records the evidence. Times below are fixture **targets**, not new results.

| Runtime · capability | Host | Batch | Known answer | A/A metric | Planted A/B | Boundary | Why repeat | Watch group |
|---|---|---|---|---|---|---|---|---|
| `chromium_page · launch` | Windows | L | ready difference 200 ms ±3 | `launch.ready` | 200 → 400 ms | `launch.painted`: `Headless` | attach rewrite `7c01e766` | L |
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
| L | `P --headless --filter launch --target launch-200,launch-400`; `P --headless --filter launch --target launch-400 --primary launch.painted` | `P --headless --filter launch --head . --target launch-400 --aa`; `P --headless --filter launch --base launch-200 --head launch-400` |
| W, E | `W --headless --filter phases,spin100,layout`; `W --headless --filter phases --primary phase.absent`; inspect `main.script` refusal on `spin100` | For each `M=run.duration,main.work`: `W --headless --filter spin100 --head . --aa --primary M`; `P --headless --filter page --base page-a --head page-b --primary M` |
| C | `W --headless --filter cpu2s`; `P --headless --filter page-work --target work-0 --runs 3`; `W --headless --filter spin100 --primary cpu.wakes` | `P --headless --filter page-work --head . --target work-1x --aa`; `P --headless --filter page-work --base work-1x --head work-2x` |
| S | `S --filter spin --runs 3`; `scripts/dev/refused-series.bench.ts --filter no-phase`; inspect the script-only refusal in `spin` | For each `M=run.duration,cpu.work`: `P --filter spin --head . --target script-a --aa --primary M`; `P --filter spin --base script-a --head script-b --primary M` |

- Use at least eight completed pairs. Require `same (±2%)` for the cell's metric, not just for another primary. A descriptive secondary or an inconclusive A/A is not evidence.
- For the A/B record the absolute change **and** the 95% interval. For plant `a → b` with common fixed cost `o`, compare with `100 × (b − a)/(a + o)`. Measure `o` from the zero/control or the check, not by fitting it to the A/B interval. For fixed iterations, use `100 × (work-1x − work-0)/work-1x`. Exact counters must match exactly. CPU tolerance is 2%; wall tolerance is ±1.5 ms per edge.
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

## Release results

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

Windows comparisons are reserved for the desktop's operator. No display state was changed. The macOS host was offline: no known answer, A/A, A/B or refusal was measured for its four page candidates or the optional terminal/probe batches. Do not transfer evidence between OSes.

The command watch additions expose omissions in four **existing review explanations**, not new post-review clock changes: Windows CPU and memory omit `jobs`; Linux CPU omits `Release`; Linux memory omits both `Release` and `Launch`. No review stamp was written.
