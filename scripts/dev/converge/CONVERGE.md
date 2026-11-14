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

Each recording had ≥25% of a core in both blocks of its first attempt. The
scheduler therefore keeps all five formerly set-aside attempts when fed the
recorded load streams. This does not predict a different verdict for any of them.

## Live A/A (six kept pairs)

| Machine and load | Attempts, main → candidate | Wall time | Primary, main → candidate |
|---|---|---|---|
| Two-vCPU guest, one CPU pinned, steady 75% of that CPU | 6 → 6 | 9.58 → 9.45 s | Same, warned → faster (small), warned |
| Same guest, 75% on head's first measured block only | 7 → 7 | 10.47 → 10.38 s | Same → same |
| Working desk, headless script, target CPUs 42% busy at start | 7 → 6 | 45.77 → 38.34 s | Same, warned → same |

The guest ran a 220 ms sleep; both versions flagged all 12 kept blocks in the
steady run and none after the burst replacement. The warned A/A difference is
not a performance claim. The desk ran a script that waits 800 ms. No load was
planted there: 7/12 vs 2/12 kept blocks were busy. This one run cannot separate
the effect of the scheduler from a change in the desk's activity.

Run `cargo run --release --manifest-path scripts/dev/background/Cargo.toml -- --converge`.
Optional result paths after `--converge` replay the original attempt streams.
`bun scripts/dev/converge/plans.ts` reads `STROBE_BIN`; its scratch history
exercises fixed, automatic and variance-limited plans without executing either
command. The separate historical background replay is unchanged: 40 warnings,
59 notes and 35 silent results.
