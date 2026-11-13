# Background measurement contract (audit of 161041dc)

Read a target row **and** a mode row. A mode does not get a second clock. `own` is CPU time (not wall time or a median CPU percentage). On shared CPUs subtract strobe and runner CPU too. A failed counter is not a zero. An exempt target has no host-load verdict.

| Target / operation | Clock of reported work | Load edges required | Own CPU for those edges | Disposition |
|---|---|---|---|---|
| Page run (including a page in an app/webview) | run send to last measured action/frame; timed segments only | SelfCost resume/pause, not the later headless frame or reset | renderer + other target tree, with exited CPU and untimed gaps removed | measured runs only; missing accounting warns |
| Page launch | navigation send to observer's ready | immediately before navigation and on ready notification, not observer-clock interpolation | browser processes relative to navigation baseline | single measured launch |
| App/webview/command-attached browser launch | process release to ready | spawn edge to ready observation; no prepare, attach grace or teardown as a separate sample | target tree from spawn | same; discarded launch must not publish |
| Native windowed run | run send to return, timed segments | SelfCost timed segments | timed process set with exited CPU | same; window checks outside |
| Native windowless run | same, without a presenting window | same | same | same |
| Native launch, windowed or windowless | release to window, element or milestone | spawn edge and ready observation; no milestone grace | tree at ready, recoverable ended processes included | same |
| Script run | request to reply minus untimed stretches | SelfCost timed segments | script host tree, not runner | same; no script launch case |
| CLI / each counted exec | release (plus OS creation on Windows) to exit | begin around spawn, end on exit callback, not pipe drain or process inventory | that exact child's exit Usage including children; sum only counted commands | one run may contain several disjoint commands; no exec in setup/untimed counts |
| Terminal run | write/run to initial end, extended only for an actual answer | keep first fast edge on silence; include wait only if reported end advances; do not include process walks or settle wait | program + timed console-host CPU exactly once | same |
| Terminal launch | release to text/state/milestone, blank grid may already match | after console setup before release; directly at observation | tree at ready plus console-host delta; zero CPU is valid | zero-length grid span has no opportunity for interference |
| Warm-up launch / measured pair 0 / subsequent launch | independent release-to-ready span per launch | only measured launch contributes | never reuse discarded launch's counters | same as single launch; order independent |

| Mode (applies to every supported target above) | Report / load / own span | Later transformations | Missing or disturbed reading |
|---|---|---|---|
| Check, batch of checks, repeated check | diagnostic cold run(s); no paired performance claim | check values, case boundaries, repeat ranges; no timed Background judgement | no fabricated unread warning for a check; preserve declared supported measurements |
| Timed comparison, including implementations | target row for each role independently, measured warmup slice | paired test includes set-aside attempts in their own set; checkpoint before retry; judge only final post-Holm metrics | warn unread; lopsided refuses only what it moves |
| Separate probe / counter pass | each side measured afresh | retain context before stripping metrics; separate pairs never borrowed | judge each contributing set |
| Single-build blocks, track samples | target row, no pairs | if passes combined, union validity evidence before selecting metrics; dirty either way round must survive | flag block, do not claim a paired verdict |
| Bisect, including tracked confirmation used as step 0 | underlying paired comparison | carry warnings to step, progress, result and text; warned step cannot move either bound | end unclear with current bounds |
| Resume new checkpoint | saved measured blocks plus fresh ones | preserve peaks, unread, identity and discarded attempts | same decision as uninterrupted run |
| Resume old checkpoint | saved block may have no counter observation | never infer quiet from an absent key | warn when a host reading is expected |
| Leak / attribution / pixels-only | heap, profile or image observations, not timing comparisons | not part of the load decision for timed pairs | not applicable |
| Phone, any mode | workload on another device | no desktop preflight load, foreign readings or desktop CPU topology | not applicable |
| macOS, any mode | host run but no supported per-CPU foreign reader | retain the preflight caution; no missing-block penalty | not applicable to per-block foreign work |

Audit findings: the launch poll bracket and warm-up selection, terminal tentative end, short exec, merged blocks, step roll-up and checkpoint compatibility all violate the corresponding rows above. Also check a missing pass reading paired with a present zero (not just max vs max), a warned parent-confirmation step in bisect, and failure to read the cycle clock while pausing: none may erase evidence. A short Linux measurement can be below idle tick resolution: a reported conservative floor is not proof that no work occurred.
