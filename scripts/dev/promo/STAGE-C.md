# Stage C: Windows headless evidence

Use release v0.41.0 (`e4c1ddd8`) throughout. Keep power settings and process placement unchanged. Run one headless invocation per slot. Refusal checks need no quiet gate. For known answers and comparisons, sample over two seconds: logical CPUs 0–15 mean below 2%, maximum below 15%; CPUs 16–31 mean below 70%. Record all three readings and retain every attempt.

Accept a comparison only with an unwarned primary and either zero flagged kept blocks or the instrument's own same-verdict confirmation on at least eight clean pairs. Spare capacity is not confirmation. A/A must read `same`. Judge the A/B by the paired signed-rank interval on the difference in milliseconds: +300 for terminal launch, +60 for grid, +150 for script CPU, +100 for page engine, +200 for later page launch, +40 for HTTP. Report strict containment; distinguish any timer or edge tolerance. Fixed-loop page CPU instead needs the independent zero/1× controls and a truth-covering ratio with half-width at most two percentage points. No display-awake wall measurement on this desk.

**Final result: Windows terminal grid and script HTTP pass on v0.41.0.** The matrix labels 26 cells verified, 19 core. Both new records await review of the later `lopsided_background` change. No new stamp was added.

## First batch

The first batch did not promote a cell. Two legs qualified: terminal-grid A/B and page-engine A/A. The follow-up below kept both results.

Times are ms, A/A intervals are percent change. **F/C** means flagged kept blocks / fully clean pairs. The two counts have different denominators: for 12 pairs there are 24 blocks. Each load triple is the start sample in percent: target mean / target maximum / other-CPU mean. The raw archive holds the command, load, exit code, console output and each available result JSON, including unsuccessful invocations.

| Cell | Known answer | A/A (%; F/C) | A/B paired shift (ms; F/C) | Refusal | Decision |
|---|---|---|---|---|---|
| Terminal launch | 210.397 → 510.105, difference 299.708 for +300 | −0.019 [−0.136, +0.123]; 6/6 of 10 pairs, warned | +300.130 [299.750, 300.544]; 10/4 of 10 | `launch.painted`: `NotForTarget` | Preview: neither comparison has load confirmation |
| Script CPU | 307.631 for 300 (+2.54%; foreign peak 28.0% of a core) | −0.123 [−1.244, +0.178]; 5/8 of 12, spare capacity only | +150.129 [149.696, 151.393]; 10/7 of 12 | `cpu.wakes`: `NotForTarget` | Preview: known exceeds 2%; neither comparison qualifies |
| Terminal grid | 20.693 for 20; 10 frames, 400 cells | −0.044 [−0.113, +0.057]; 10/5 of 12, warned | +60.047 [59.982, 60.116]; 4/8 of 12, same verdict on 8 clean pairs | `overrun-120`: `AmbiguousInteraction` | Preview: planted leg qualifies, A/A does not |
| Page wall/counts | Display-off refusal attempt returned phase.b 41.2, phase.a 22.9, counts 7/5 | not run | not run | `Throttled` **not observed** | Preview: no display-off boundary; no timed legs requested |
| Page engine | main.work 100.443 for 100; 25 layouts and 25 styles | +0.008 [−0.024, +0.047]; 5/8 of 12, clean pairs confirm | +100.070 [99.976, 100.147]; 4/8 of 12, no clean-pair confirmation | `main.script`: `NotForTarget` | Preview: known, A/A and refusal pass; needs qualifying A/B |
| Page CPU | 2003.941 vs 2000 OS ms, ended count 0; peak foreign 25.82% | +0.039 [−0.177, +0.218]; 6/6 of 12 | +93.364 [91.715, 94.991]; relative +100.45 [98.89, 101.88]%, 7/6 of 12 | `cpu.wakes`: `NotForTarget` | Preview: both comparisons lack clean-pair confirmation |
| Page launch | `launch.ready` 430.699 / 617.532; `plant_start` 27.599 / 14.332; spans 400.200 / 600.100, ready 2.900 / 3.100 after end | −0.021 [−0.186, +0.067]; 50/3 of 32, warned | +200.383 [199.484, 201.599]; 52/4 of 32, warned | `launch.painted`: `Headless` | Preview: later pair covers +200, but both comparisons lack load confirmation |
| Script HTTP | 21.638 / 61.667 for 20 / 60; 15 probes each, no errors or timeouts | −0.004 [−0.106, +0.064]; 6/6 of 12 | +40.070 [40.043, 40.113]; 28/1 of 16, warned | `status-500`: `RunIncomplete` for latency after 3 invalid replies in 9 | Preview: neither comparison qualifies; interval does not contain +40 |

| Cell | Known load | A/A load | A/B load | Boundary load |
|---|---|---|---|---|
| Terminal launch | 0.9/5.0/47.3 | 1.7/8.3/18.3 | 0.9/6.4/58.1 | 0.9/4.0/87.9 |
| Script CPU | 0.8/4.8/62.8 | 0.9/7.1/21.9 | 0.4/5.1/19.7 | 0.9/4.4/19.8 |
| Terminal grid | 1.3/9.0/66.2 | 1.0/5.6/27.4 | 1.4/7.3/23.4 | 0.9/5.7/61.3 |
| Page wall/counts | — | — | — | 1.3/6.1/89.3 |
| Page engine | 0.6/5.0/7.2 | 0.7/3.8/6.8 | 1.7/7.5/22.8 | 1.6/12.8/21.6 |
| Page CPU | 1.3/4.4/19.1 | 1.0/5.1/8.6 | 1.1/7.4/27.5 | 0.6/2.5/20.0 |
| Page launch | 0.4/5.3/13.7 | 0.3/2.7/13.9 | 0.6/5.4/27.0 | 0.5/4.6/21.3 |
| Script HTTP | 0.7/7.4/27.1 | 0.5/5.2/34.0 | 0.9/5.6/26.8 | 0.9/5.7/26.3 |

Page CPU controls: work-0 0.116 [0.098, 0.132] ms (load 1.1/5.9/65.4); work-1x 90.652 [90.593, 90.712] ms (1.0/5.6/8.4). These imply an expected fixed-iteration change of about +99.85 to +99.89%, inside the A/B interval. The block flags still prevent promotion.

## Follow-up on the remaining legs

All four parts for both promotions used the same v0.41.0 release. No comparison was pooled with an earlier attempt. F/C is still flagged blocks / clean pairs; the load column gives the start triple for each leg that ran.

| Cell | A/A (%; F/C) | A/B shift (ms; F/C) | A/A load; A/B load | Decision |
|---|---|---|---|---|
| Terminal grid | +0.026 [−0.010, +0.065]; 2/10 of 12, clean subset confirmed | Original +60.047 [59.982, 60.116]; 4/8 of 12, clean subset confirmed | 0.35/4.43/7.63; original above | **Verified** with the 20.693 ms answer, exact frame/cell counts and `AmbiguousInteraction` |
| Script HTTP | −0.024 [−0.064, +0.029]; 3/9 of 12, clean subset confirmed | +40.035 [39.997, 40.064]; 4/12 of 16, clean subset confirmed | 0.64/6.32/32.28; 0.87/5.83/28.69 | **Verified** with the delay checks and `RunIncomplete` for HTTP 500. The first A/B missed +40 and was warned. |
| Terminal launch | +0.021 [−0.084, +0.099]; 3/7 of 10 | +300.525 [299.714, 300.935]; 0/10 of 10 | 0.80/3.14/31.55; 0.74/5.76/20.80 | Preview: A/B now passes; A/A still lacks confirmation |
| Script CPU | −0.057 [−0.279, +0.090]; 7/6 of 12 | +149.932 [149.740, 151.481]; 9/6 of 12 | 1.32/6.03/17.11; 1.24/5.65/49.80 | Preview: repeat known 303.753 ms vs 300, within 2% (0.65/5.16/15.38); neither paired leg qualifies |
| Page engine | Original same, confirmed on 8 of 12 clean pairs | +100.017 [99.930, 100.076]; 17/1 of 12 | original above; 0.90/4.83/16.58 | Preview: both planted intervals covered +100 but neither comparison qualified under load |
| Page CPU | +0.110 [−0.351, +0.648]; 11/6 of 12, warned | +92.308 [90.938, 93.868]; 9/7 of 12. Relative [98.89, 100.98]% covers control | 1.96/14.93/62.68; 1.00/7.05/24.28 | Preview: repeat known 2005.35 vs 2016 OS ms, ended count 0 (1.28/7.62/37.99); neither paired leg qualifies |
| Page launch | +0.055 [−0.116, +0.319]; 37/8 of 32, clean subset confirmed | Not started in either ten-minute gate window | 1.79/6.39/31.26; no start | Preview: A/A now passes; still needs qualifying A/B |
| Page wall/counts | Not run | Not run | Refusal check 0.77/5.70/22.70 | `phases` gave values, not `Throttled`; no timed wall run requested |

The repeat HTTP delay check read 21.612 / 61.663 ms with 15 probes per side and no timeouts (start 1.91/8.39/27.99). These checks recovered the known delays under load: the original fast/slow foreign peaks were 17.81/70.70% of a core and the repeat peaks 34.37/232.60%. The timing verdicts above rely on the independent paired runs with clean-subset confirmation, not on either check's load.

Adding `phase_state` to the command and script wall/counts watches tripped **no verified cell**. Its function body is identical at the Mac and Linux script evidence revisions, `ebc9eeaf`, `43772400` and the reviewed head. Those two commits changed the code or comments that follow the function. The merge through v0.43.0 retains main's 24 reviewed records and reason strings. Only Windows `tty · tty_grid` (12 A/A, 12 A/B pairs) and `script · http_probe` (12 A/A, 16 A/B pairs) trip `src/app/bench.rs#lopsided_background`, changed since the v0.41.0 evidence.

The Mac remains offline. Its [headless run list](STAGE-B.md#when-the-mac-returns) still applies: take all four parts on the same build, use 400 → 600 ms for page launch, and do the command planner's Mac verification on the planner build. Do not transfer any Windows reading to a Mac cell.
