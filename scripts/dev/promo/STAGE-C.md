# Stage C: Windows headless evidence

Use release v0.41.0 (`e4c1ddd8`) throughout. Keep power settings and process placement unchanged. Run one headless invocation per slot. Refusal checks need no quiet gate. For known answers and comparisons, sample over two seconds: logical CPUs 0–15 mean below 2%, maximum below 15%; CPUs 16–31 mean below 70%. Record all three readings and retain every attempt.

Accept a comparison only with an unwarned primary and either zero flagged kept blocks or the instrument's own same-verdict confirmation on at least eight clean pairs. Spare capacity is not confirmation. A/A must read `same`. Judge the A/B by the paired signed-rank interval on the difference in milliseconds: +300 for terminal launch, +60 for grid, +150 for script CPU, +100 for page engine, +200 for later page launch, +40 for HTTP. Report strict containment; distinguish any timer or edge tolerance. Fixed-loop page CPU instead needs the independent zero/1× controls and a truth-covering ratio with half-width at most two percentage points. No display-awake wall measurement on this desk.

## Results

Times are ms, A/A intervals are percent change. **F/C** means flagged kept blocks / fully clean pairs. The two counts have different denominators: for 12 pairs there are 24 blocks. Each load triple is the start sample in percent: target mean / target maximum / other-CPU mean. The raw archive holds the command, load, exit code, console output and each available result JSON, including unsuccessful invocations.

| Cell | Known answer | A/A (%; F/C) | A/B paired shift (ms; F/C) | Refusal | Decision |
|---|---|---|---|---|---|
| Terminal launch | 210.397 → 510.105, difference 299.708 for +300 | −0.019 [−0.136, +0.123]; 6/6 of 10 pairs, warned | +300.130 [299.750, 300.544]; 10/4 of 10 | `launch.painted`: `NotForTarget` | Preview: neither comparison has load confirmation |
| Script CPU | 307.631 for 300 (+2.54%; foreign peak 28.0% of a core) | −0.123 [−1.244, +0.178]; 5/8 of 12, spare capacity only | +150.129 [149.696, 151.393]; 10/7 of 12 | `cpu.wakes`: `NotForTarget` | Preview: known exceeds 2%; neither comparison qualifies |
| Terminal grid | 20.693 for 20; 10 frames, 400 cells | −0.044 [−0.113, +0.057]; 10/5 of 12, warned | +60.047 [59.982, 60.116]; 4/8 of 12, same verdict on 8 clean pairs | `overrun-120`: `AmbiguousInteraction` | Preview: planted leg qualifies, A/A does not |
| Page wall/counts | Display-off refusal attempt returned phase.b 41.2, phase.a 22.9, counts 7/5 | not run | not run | `Throttled` **not observed** | Preview: no display-off boundary; no timed legs requested |
| Page engine | main.work 100.443 for 100; 25 layouts and 25 styles | +0.008 [−0.024, +0.047]; 5/8 of 12, clean pairs confirm | +100.070 [99.976, 100.147]; 4/8 of 12, no clean-pair confirmation | `main.script`: `NotForTarget` | Preview: known, A/A and refusal pass; needs qualifying A/B |
| Page CPU | 2003.941 vs 2000 OS ms, ended count 0; peak foreign 25.82% | +0.039 [−0.177, +0.218]; 6/6 of 12 | +93.364 [91.715, 94.991]; relative +100.45 [98.89, 101.88]%, 7/6 of 12 | `cpu.wakes`: `NotForTarget` | Preview: both comparisons lack clean-pair confirmation |

| Cell | Known load | A/A load | A/B load | Boundary load |
|---|---|---|---|---|
| Terminal launch | 0.9/5.0/47.3 | 1.7/8.3/18.3 | 0.9/6.4/58.1 | 0.9/4.0/87.9 |
| Script CPU | 0.8/4.8/62.8 | 0.9/7.1/21.9 | 0.4/5.1/19.7 | 0.9/4.4/19.8 |
| Terminal grid | 1.3/9.0/66.2 | 1.0/5.6/27.4 | 1.4/7.3/23.4 | 0.9/5.7/61.3 |
| Page wall/counts | — | — | — | 1.3/6.1/89.3 |
| Page engine | 0.6/5.0/7.2 | 0.7/3.8/6.8 | 1.7/7.5/22.8 | 1.6/12.8/21.6 |
| Page CPU | 1.3/4.4/19.1 | 1.0/5.1/8.6 | 1.1/7.4/27.5 | 0.6/2.5/20.0 |

Page CPU controls: work-0 0.116 [0.098, 0.132] ms (load 1.1/5.9/65.4); work-1x 90.652 [90.593, 90.712] ms (1.0/5.6/8.4). These imply an expected fixed-iteration change of about +99.85 to +99.89%, inside the A/B interval. The block flags still prevent promotion.

The Mac remains offline. Its [headless run list](STAGE-B.md#when-the-mac-returns) still applies: take all four parts on the same build, use 400 → 600 ms for page launch, and do the command planner's Mac verification on the planner build. Do not transfer any Windows reading to a Mac cell.
