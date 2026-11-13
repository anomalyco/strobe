# Background measurement contract

Read a target row and a mode row. An edge is an observation on the host, not a reconstruction from a page, log or present timestamp. Own CPU is CPU time over the same edges. A failed counter is not zero.

| Target / operation | Reported span | Load edges and own CPU | Is there any wait, grace or confirmation between the reported end stamp and the load window's close? |
|---|---|---|---|
| Page run (including embedded pages) | Send through last measured action/frame, minus untimed stretches | SelfCost resume/pause; renderer and target tree including recovered exits | No. A requested frame is timed; the extra headless frame, counts, reset and teardown are not. |
| Page launch | Navigation send to ready | Navigation baseline to ready notification; browser tree delta | No deliberate wait. Ready delivery is observed, not interpolated; later milestones and teardown are outside. |
| App/webview/command-attached browser launch | Release to observer or window ready | Spawn to ready observation, target tree since spawn | No. Visible/painted use the matching sample. Settled keeps load and CPU at the last change; the second of unchanged pixels only confirms it. |
| Native windowed run | Send to return, minus untimed stretches | Timed SelfCost segments; process set plus recovered exits | No. The window checks, process inventory and reset do not extend the end. |
| Native windowless run | As above without a window | Same process accounting | No. No present wait. |
| Native launch | Release to window, selector, pixels or milestone | Spawn to ready observation, tree at that observation | No deliberate wait. Save the last change before stillness; sample at the selector/pixel observation before looking up the present. Its display timestamp can differ from the observation. Milestone grace and teardown follow the snapshot. |
| Script run | Request to reply, minus untimed stretches | Timed SelfCost segments; host tree including exits | No. Heap, inventories, reset and teardown follow the edge. |
| CLI / counted exec | Creation/release to exit; commands only | Pre-spawn to exit callback, exit Usage including descendants | No. Pipe drains, log reads and reset follow the exit. Failed spawn must disarm the edge. |
| Terminal run | Run to initial end or actual answer | Timed SelfCost segments; program plus console-host CPU once | No. Silent-key timeout rolls back to first end; a real answer advances both. Post-run quiet wait and inventory are outside. |
| Terminal launch | Release to matching grid, state or milestone | Spawn to ready observation, target tree plus console-host delta | No. No milestone grace or teardown. An already-matching blank grid has zero interval, not unread counters. |
| Launch warmup / pair 0 / later launch | Independent release-to-ready spans | Discarded launches contribute no reading; one measured launch per block | No wait is carried across launches. |

| Mode | Report, filtering and own CPU | Missing or disturbed readings | End-to-close wait? |
|---|---|---|---|
| Check / batch / repeat | Diagnostic observations only | Preserve supported requested metrics; no timed Background decision | N/A; no verdict span. |
| Comparison / implementations | Measure each role and its warmup slice independently; judge final post-Holm metrics | Keep discarded attempts for their own pass. Unread warns; one-sided work refuses what it moves | No; target row applies. |
| Separate probe / counter pass | New measurement; retain context when filtering | Judge each contributing set | No; never bridge passes. |
| Single-build blocks / track samples | When combining passes, preserve worst evidence before metric selection | Either pass can flag the block; no paired claim | No; never bridge launches or blocks. |
| Bisect / track confirmation | Underlying paired comparison; keep warnings with attempts | Accept readable decisive movement in the search direction; otherwise retry a warned bisect step within budget or stop unclear. An inconclusive warned track confirmation is unclear, not rejected | No; a quiet wait precedes a fresh experiment. |
| Resume new checkpoint | Saved blocks and new blocks; preserve peaks, resolution, unread and discarded attempts | Same decision as uninterrupted work | No; never bridge a checkpoint. |
| Resume old checkpoint | No reconstruction of absent evidence | Warn when host accounting was expected | N/A for an absent reading. |
| Leak / attribution / pixels / filmed | Diagnostic observations, not timed verdicts | No timed Background judgement | N/A. |
| Phone | Work on another device | No desktop topology or preflight | N/A. |
| macOS | No supported per-CPU foreign reader | Preflight only | N/A. |

**Linux block context:** In addition to the per-run readings, compare the first measured opening edge with the last measured closing edge of the same block. Charge idle tick error once, and subtract target CPU across resets and inter-run gaps, not just CPU inside `run`. Exclude blocks with untimed stretches. Observer/runner work on the counted CPUs is not foreign. Never combine independent cold launches. A lower bound below 25% with tick uncertainty above 50% cannot prove a quiet pair; an idle preflight yields a note, not a warning. Windows has no tick floor.
