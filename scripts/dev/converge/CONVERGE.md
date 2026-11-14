# Which pair attempts pay for themselves?

One timed pair is two blocks. In the scripted rows each block takes one second;
the clock counts blocks only, not startup, discarded warmups or a quiet wait.
Inputs are block readings, not a load planted on the machine. Each row runs for
CPU work, wall time and launch readiness. Compare identical values on both sides
so a load refusal cannot hide behind a change in the workload.

| Foreign work (% of a core, base/head) | Set aside, main → target | Extra block time | Primary on both versions |
|---|---:|---:|---|
| Quiet (0/0) | 0 → 0 | 0 | Same |
| One burst (0/150), replacement quiet | 1 → 1 | +2 s | Same; note |
| Separate bursts on opposite sides, quiet pair between | 2 → 2 | +4 s | Same; note |
| Both sides busy, 50/200 then 200/50, alternating | 2 → 0 | +4 → 0 s | Same, warned |
| Only head stays busy (0/150) | 1 → 1 | +2 s | One-sided load refuses, counting set-aside attempts |

Two consecutive timed blocks at or above 25% of a core are evidence that load
lasted across a block boundary. Stop retrying for load for the rest of the series.
A repeated large difference toward the same side already stays on main; keep
that protection too. Equal steady load has no large difference and never needed
a retry. Neither policy changes a block reading or decides the verdict.

| Pair plan (cross every row above) | Kept pairs / launches here | Main before first pair, unknown spread | Target before first pair |
|---|---:|---|---|
| Default | 8 / 16 (first look) | At least 8 pairs | Estimated kept pairs and launches; spread unknown |
| Under eight: 6 | 6 / 12 | At least 6 pairs | Fixed count; cannot check agreement on 8 clean pairs; `--pairs 8` |
| Eight to eleven: 9 | 9 / 18 | At least 9 pairs | Fixed count and launches; no clean-floor warning |
| Twelve or more: 12 | 12 / 24 | At least 12 pairs | Fixed count and launches; no clean-floor warning |

For each cell total attempts = kept pairs + set aside, total seconds = twice
that. For these constant values no more pairs are needed. For noisy data, main
can project an underpowered plan from history but gives no flag. Target: both
the projection and the underpowered ending name the total `--pairs N` for an
80% chance, not N extra attempts. Without spread neither guesses a variance.

Recorded six-pair desk A/As (not a simulated re-run): 8, 8 and 7 attempts;
56.3, 633.8 and 482.6 s. Their kept blocks crossed the floor in 10/12, 11/12
and 12/12. The first and last ended underpowered (~48 and ~25 pairs); the middle
was unclear. A new schedule cannot recreate an unmeasured replacement block.
