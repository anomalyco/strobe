# Which pair attempts pay for themselves?

## Changes within a series

Cross quiet, a single burst, alternating bursts, light symmetric load, sustained
uneven load on both sides, and sustained one-sided load in both orders. Switch
halfway to the first look. Also exercise a late isolated burst, six unanimous
hits, an uneven patch that changes sides, and a busy replacement then a quiet
pair. Budgets are 6, 8, 12, auto. `--sequences` on the background harness prints
attempts/verdict/background level for each cell. `Refused` means no number.

Baseline (`0990c47d` retry condition and judgement); each token is extra attempts,
then S/B/R for same/better/refused and i/w/- for info/warn/silent. Read left
to right at 6, 8, 12, auto; auto decided at eight in these rows. The sequence
harness reads this independent ledger; changes to product judgement cannot
rewrite the baseline it compares against.

```text
quiet quiet 0S- 0S- 0S- 0S-
quiet burst 1Si 1Si 1Si 1Si
quiet alternating 2Sw 2Sw 2Si 2Sw
quiet light 0Sw 0Sw 0Sw 0Sw
quiet both 2Sw 2Sw 2Sw 2Sw
quiet one 1Sw 1Sw 1Rw 1Sw
burst quiet 1Si 1Si 1Si 1Si
burst burst 1Sw 1Sw 1Si 1Sw
burst alternating 2Sw 2Sw 2Sw 2Sw
burst light 1Sw 1Sw 1Sw 1Sw
burst both 2Sw 2Sw 2Sw 2Sw
burst one 2Sw 2Sw 2Sw 2Sw
alternating quiet 2Sw 2Sw 2Si 2Sw
alternating burst 2Sw 2Sw 2Sw 2Sw
alternating alternating 2Sw 2Sw 2Sw 2Sw
alternating light 2Sw 2Sw 2Sw 2Sw
alternating both 2Sw 2Sw 2Sw 2Sw
alternating one 2Sw 2Sw 2Sw 2Sw
light quiet 0Sw 0Sw 0Sw 0Sw
light burst 1Sw 1Sw 1Sw 1Sw
light alternating 2Sw 2Sw 2Sw 2Sw
light light 0Sw 0Sw 0Sw 0Sw
light both 2Sw 2Sw 2Sw 2Sw
light one 1Sw 1Sw 1Rw 1Sw
both quiet 2Sw 2Sw 2Sw 2Sw
both burst 2Sw 2Sw 2Sw 2Sw
both alternating 2Sw 2Sw 2Sw 2Sw
both light 2Sw 2Sw 2Sw 2Sw
both both 2Sw 2Sw 2Sw 2Sw
both one 2Sw 2Sw 2Sw 2Sw
one quiet 1Sw 1Sw 1Rw 1Sw
one burst 2Sw 2Sw 2Sw 2Sw
one alternating 2Sw 2Sw 2Sw 2Sw
one light 1Sw 1Sw 1Rw 1Sw
one both 2Sw 2Sw 2Sw 2Sw
one one 1Rw 1Rw 1Rw 1Rw
late - 1Si 1Si 1Bi 1Si
unanimous - 1Rw 1Rw 1Rw 1Rw
irregular - 1Sw 1Rw 1Rw 1Rw
recovery - 2Sw 2Sw 2Si 2Sw
```

Outcome on the revised branch: **158/160 agree on the verdict and level; no
warning becomes a note**. Two less-cautious cells: `irregular` at fixed 8 and
auto. Main ran a ninth attempt (seven +150%, two −30%, p=.0156), so refused;
the branch measured eight (six +150%, two −30%, p=.03125), so cannot claim the
specified p≤.02 refusal. Both carry a warning: fixed 8 says worse; auto says
same, warned. A hypothetical ninth reading is not evidence. At fixed 6 the
unanimous exception is reachable, and at 12 both policies refuse this sequence.

The late-burst auto row now matches main: nine attempts, same, note (formerly
eight, faster, note). The unanimous six-pair row refuses with seven attempts on
both versions: the opening two-sided pair stays; the first later isolated hit
is replaced. Six pairs with *both* sides busy and the same large difference in
every pair exercise the six-difference refusal directly.

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

Keep the pair when both of its blocks are busy. After a busy replacement, keep
subsequent bursts until a clean pair runs. Neither condition persists after the
evidence for it is gone. A repeated isolated hit on the same side stays, as on
main. Equal steady load has no large difference and never needed a retry.

| Pair plan (cross every row above) | Kept pairs / launches here | Main before first pair, unknown spread | Target before first pair |
|---|---:|---|---|
| Default | 8 / 16 (first look) | At least 8 pairs | 8 to 40 kept pairs; 16–80 launches before retries |
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

Each formerly set-aside attempt had ≥25% of a core in both blocks. The
scheduler therefore keeps all five formerly set-aside attempts when fed the
recorded load streams. One later, formerly kept, isolated hit in the first stream
does qualify for a retry. Its replacement was never measured. This does not
predict a different verdict for any of them.

## Live A/A at `c47cc48` (six kept pairs)

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
command. The separate historical background replay changes from 40 warnings,
59 notes and 35 silent results to 47/52/35. In seven old directional results a
kept pair crosses the burst ceiling: CPU headroom no longer clears those hits.
