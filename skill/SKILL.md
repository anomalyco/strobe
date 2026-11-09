---
name: strobe
description: Measure and explain performance of web pages, Electron apps, native apps, terminal programs, Bun or Node processes and any process. Use when the user asks whether a change is faster, why something is slow, janky or heavy, where memory goes, what the CPU or GPU is doing, or to write a benchmark.
---

# Strobe

strobe records a target on one timeline and answers with verdicts and handles (`rec_…`, `exp_…`) to
drill into. It refuses measurements it cannot defend: a refused run is an answer, never "no value".
The design and every rule behind this file: SPEC.md.

## Playbooks

### Did my change make it faster?

1. Write or find a bench file (below). Check it first: `strobe bench <file> --head . --check`
   (each case once, pass or fail with a screenshot; nothing is decided).
2. `strobe plan <file> --base origin/dev --head .` says pairs and minutes per series.
3. `strobe bench <file> --base origin/dev --head .`: a verdict per series (case × target × params).
   - Read the ★ primary's verdict: `faster`/`slower` (`less`/`more`, `fewer`), `same (±2%)`,
     `unclear`, `underpowered` (more pairs or a quieter machine, not "no change"), `refused`.
   - Secondaries get words only after the primary decided (Holm-corrected); `·` is descriptive.
   - `ledger  3rd experiment on these variants; earlier: …`: reruns count. Paste it as it is.
4. Put the evidence in the PR (below).

- Refactor or dependency bump? `--noworse 2%` (or `noworse: "2%"` on the case;
  `--noworse 2%,cpu.wakes=5%` per metric): `no worse (≤2%)`, `worse (>2%)` or `unclear`.
- `strobe run --base "<old command>" --head "<new command>"`: the same A/B for two command lines.
- `--aa --head .` checks the method on this machine (same build both sides).
- Long benches keep running in the daemon: `strobe query exp_… progress`. Ctrl-C, SIGTERM or
  `strobe bench --stop <exp>` stop after the block (exit 130, what finished is kept);
  `strobe bench --resume <exp>` goes on.

### Why is it slow?

- `bench … --why always` (an attribution block per series), `bench … --check --why always` (one build),
  or `strobe record <target> --probes attribution` / `strobe record --bench <file>#<case>`. Then:
  - `query <rec> hotspots` (CPU by function, full stacks, source-mapped file:line:column;
    `--by file|package`; `--sort total` for callers; native: `--thread <name>`, `--pid <pid|exe>`,
    `--phase <name>`).
  - `query <rec> layout` (forced layouts by their call site), `style` (what invalidated style:
    attribution blocks trace it for a series declaring `main.style` or `style.count`, or with
    `--style`), `alloc` (who allocates), `gc`, `summary` (per run: style, layout, paint ms), `insights`.
  - A page served from your own folder (`record https://localhost:… --served <dir>`): its source-mapped
    frames resolve to files on disk.
  - What changed: `query <exp> hotspots` (each series' base..head) or `query <recA>..<recB> hotspots`;
    `~` marks a change inside its sampling interval (one launch a side: not a verdict).
- `strobe open <rec>` (or `<exp>/head`): the whole recording in a local Perfetto UI. For seeing;
  numbers come from strobe's views. The "strobe itself" track is strobe's own cost: check it first.
- A thread waiting (a lock, IO, a timer) and who wakes it: `record pid:<n> --probes waits --for 10s`,
  then `query <rec> waits --thread <name>` (Linux needs CAP_PERFMON, Windows the deep helper;
  macOS refuses `NoWaits`).
- A slow page frame in the renderer or the GPU process: `frames.renderer.p95` against `frames.viz.p95`,
  and `query <rec> frames` (each worst frame's stages).
- Inputs with no latency sample (`busy`), or one that looks wrong: `query <rec> inputs [--case <c>]`
  (the first 100; `--top N`, `--json` every one):
  each input's run and time, what proved its change, the frames around it, its latency or why it
  was busy (`mid_drawing`, `behind`, `ambiguous_press`, `multi_frame_pixel`) and how much later it
  had to come. Give the case that wait before such inputs.
- A profiled launch is slower than a timed one: compare totals only from the timed pairs.

### Where is the memory? Does it leak?

- `query <rec> memory` (per process; `--by layer|allocator` for Chromium's accounting). A native
  app: what holds it (host heap, GPU memory mapped in, unmapped commit, shared, files) beside
  `gpu.vram`. On Windows `memory.app.private` counts VRAM a driver maps in: quote `memory.app.host`.
- `query <rec> heap`: classes by retained size, detached DOM, retainer paths; `query <a>..<b> heap`.
- A case's leak: `bench <file> --filter <case> --head . --probes memory --repeat 10`: growth per run
  with an interval, and `query <rec> leaks` for what from the first half is still alive.
- An app over time: `record <target> --probes memory --for 60s` while it does the suspect thing.
- Sizes are binary: KB, MB and GB are 2^10, 2^20 and 2^30 bytes.

### A native app, or any process from outside

- `record app:<id> --for 10s` (a strobe.json profile) or `record exe:<path> --native`: CPU, thread
  wakes, IO, GPU, memory, the UI thread and what reached the screen. `--windowless` for a tray app.
  `record pid:<n>` or `proc:<name>` for a process strobe did not start.
- `query <rec> threads`: wakes/s and CPU per thread, the module each started in, whose (app, driver,
  os, `unknown (?)`); `--timeline` per second. Idle wake-ups: `query <rec> insights`.
- Native hotspots of a tree pool every process: read `by process`, then `--pid <pid|exe>`.
  A native attribution recording (`record --bench`, `--why`) keeps frames too: `query <rec> inputs`.
  `! Coverage`: samples cover under 80% of the CPU (read shares, not totals); it names what the
  sampler counted of the rest (`late_ms` before a thread was opened, `forgone_ms`, unopened threads,
  lost events). A launched app's children are sampled from their start. Two resolved shares
  (leaf, all frames): a low all-frame share means the app's own binary has no symbols.
  macOS samples are untimed snapshots: counts, never ms.
- `record pid:… --probes deep|waits` prints `cost`; `! CaptureCost`: the capture slowed a
  switch-heavy process, so its stacks describe that slower process.
- Benching it: `native("my-app")` in a bench file (below).

### Two implementations of one app (a port, a rewrite)

- One bench file, one target per implementation, the same cases (journeys):
  `bench <file> --base original --head rewrite` (`rewrite=git:main`, `original=exe:/path`;
  `target:<name>` when a folder has that name). Each target's `name` is what reports call it.
- Pick a primary both sides measure the same way: `cpu.work`, `run.duration`, `launch.ready`,
  `memory.app.private`. `main.work` or `ui.cpu` across engines exits 2 (`NotComparable`).
- Give each target a `ui` adapter for what differs (`ui: { row: ".row", open: ({ page }) => … }`);
  journeys say `@row`, `await ctx.ui.open()`. A journey one side cannot run is a parity gap
  (`unsupported: { rewrite: "why" }`, or an `@name` its `ui` lacks): listed, never run.
- `expect: { same: ["@topRow"] }`: both sides must read it alike after setup, each run and reset, or
  `NotEquivalent`. Both talk to one `fixture("<server> --port {port}")` (`{fixture.url}` in args/env).
- A page against a native app (or another engine) gets OS input: run it headed, window in front;
  `page.act` on one side and `click` on the other is `InputMismatch`.
- `--check --base a --head b` prints the parity matrix; `--why always` profiles each side once.
- Over time: `track <file> --base original=exe:/path --head rewrite=git:main --schedule commits`.

### Which commit moved it? Did anything regress?

- `bench <file> --filter <case> --bisect <good>..<bad>`: one series; each step is a full experiment;
  the culprit is benched against its own parent.
- `track <file> --range <old>..<branch> --every 3` (or `--schedule` on a branch), then
  `history <file>` or `query changes`. Only `confirmed` changes are findings.

### No display, a locked session, a server

- `--headless` runs page cases (check or timed) with no display: frames, latency, scroll, canvas and
  GPU use refuse `Headless`; CPU, main-thread work, counters, phases and memory stay. Headless,
  what frames cost is `main.paint` beside `main.style` and `main.layout`.
- A locked session runs what needs no presenting window: scripts, `cli()`, `tty()`, leak runs,
  headless pages, `window: false` native cases measuring `cpu.*`, `memory.*`, `gpu.vram`, `io.*`,
  `count.*`, `phase.*`, and any native case under `--check`. Compare them only with locked runs.
- The rest waits for an unlock (a line a minute). `--unlock-wait 2h` caps it: then those series are
  not run (exit 124, resumable) and the windowless ones still run.
- Locked, a native app's tree, `page.act` and keys work; clicks, moves and wheels refuse
  `SessionLocked`. On Windows its pixels still read (image anchors, `--pixels`).
- Linux without `DISPLAY`: native apps run on strobe's headless Wayland compositor (install sway,
  dbus-daemon and at-spi2-core); frames and latency refuse `Headless`.

### A busy server: which thread held it, and could a client get an answer?

- Host the server in a `script()` target. In `setup`, `await ctx.httpProbe({ url, authorization })`:
  strobe probes it every `cadenceMs` (100) from its own threads on fixed deadlines, `timeoutMs` 1000.
  The whole runnable example (`server.ts` too): `node_modules/@hona/strobe/README.md`.

```ts
export default bench({
  targets: { server: script("./server.ts") }, // exports start(tokenHash) and stall(ms)
  async setup(ctx) {
    const token = crypto.randomUUID() // the server gets its hash only
    const hash = new Bun.CryptoHasher("sha256").update(`Bearer ${token}`).digest("hex")
    const url = await ctx.call<string>("start", hash)
    const expect = { jsonPath: { path: "$.status", equals: "ok" } }
    await ctx.httpProbe({ url, authorization: `Bearer ${token}`, cadenceMs: 40, expect })
  },
  cases: {
    "slow handler": {
      primary: "http.latency.p95", // one 900 ms run: ~22 probes, enough for a p95 in a --check
      metrics: ["cpu.main", "cpu.main.busy", "http.timeouts"],
      run: (ctx) => ctx.call("stall", 900),
    },
  },
})
```

- `record --bench <file>#<case>` (or `--check`, `--why always`): stacks, the threads' timeline and the
  probe on one recording. `query <rec> http`: idle against the runs, timeouts, errors and the stalled
  cohorts; `strobe open <rec>` draws the probes beside the runs and threads.
- To compare it, declare `http.latency.p95` (or `http.timeouts`, `http.errors`): the probe then runs
  in probe blocks of its own. A clean timing block never probes.
- A timeout ranks above every answer: a quantile on one refuses `RunIncomplete` (raise `timeoutMs`).
  A 500 or a failed `expect` refuses the latencies. `http.missed_sends` is strobe's lag, not the server's.
- `authorization` stays in strobe's memory: never in a file, report, trace or answer. Loopback only.
- `cpu.main` is the main thread alone beside `cpu.work` (the tree): `cpu.main.busy` near 100% is a
  saturated event loop; a worker or a child process shows in `cpu.work` only. A script's (or a
  windowless native app's) check line shows both even when the case does not declare them.
- A script's `query <rec> hotspots` is its measured runs; `--source host` the whole host process
  (start, setup, warmup, resets), its `by lifecycle` lines each part's wall time against the JavaScript
  sampled in it; `--source startup` the host's start and the target's static imports, `--source setup`
  the bench file's setup (a server's dynamic imports); `--phase <name>` one phase.
  - Slow startup with few rows: `N modules evaluated` (each module is a stack of its own: `--by file`
    sums them). Lifecycle shares are of wall time: unsampled wall is mostly waiting, and on Bun
    native work (transpiling, module loading) too, which `--cpu-prof` never samples.
  - `! Missed`: the main thread used CPU (`cpu.main`) the profile has no sample of, so rows
    undercount (Node labels short timer callbacks `(idle)`; Bun never samples its event loop's own
    work). `! Waited`: rows include time the thread waited for a CPU (samples weigh wall time).
  - `query <rec> threads --timeline`: which thread was busy when, runs marked.

### Phones

- Emulated: `web({ emulate: "mobile" })` is a phone (390×844 @3x, touch, headless): `click` is a tap,
  `drag` a finger, `scroll` swipes (Chromium only); `hover`, `move`, `wheel` and mouse buttons refuse
  `DeviceMismatch`. Headless, so `frames.*` and `interaction.*` refuse `Headless`: measure
  `main.work`, `cpu.*` or `run.duration` there, and frames and latency on a real phone (`--mobile`).
  `"touch"` is a desktop with a touch screen: the mouse stays its primary pointer.
- A real Android phone: `--mobile` (or `--mobile=<serial>`) runs the file's `emulate: "mobile"`
  targets in Chrome on the phone over adb; `look --mobile`, `record --mobile` too.
  - Give strobe a channel once: install Chrome Canary (or Dev, Beta), open it once, then
    `strobe up --mobile=<serial> --channel canary`. strobe never drives the phone's own Chrome.
  - An emulator runs `--check`, `look` and attribution only (`Emulator`); timed runs refuse Wi-Fi
    adb, Battery Saver, under 20% battery, and wait up to 3 min for a hot phone.
  - `cpu.*`, `memory.*`, `gpu.*` refuse `NotForTarget` (this computer's processes): measure
    `main.work`, `frames.*`, `interaction.*`, `run.duration`. A lost phone ends the bench (exit 255).
- An iPhone on a cable: `look wir:` and `record wir:` read an open Safari tab or WKWebView over Web
  Inspector, read-only (`hotspots`, `heap`, `memory`, `log --source console|device|crash`,
  `network`); `strobe status` lists the phones and their pages.

### Writing the PR: every performance claim comes from strobe

1. **Discovery**: the strobe command and recording id that pointed at the problem, and what it showed.
2. **The change**: what the code does differently.
3. **Results**: `strobe report experiment <exp> --format pr --update-pr <n>` puts strobe's block in
   PR n's body, led by its result card (or paste the `--format pr` output).
   - The block is delimited (`<!-- strobe:report <exp> v1 begin sha256:… -->` … "end of generated
     report"). Never edit inside it: no retyped, rounded or dropped numbers, no rewritten verdicts;
     strobe refuses to update a hand-edited block. Write your reading above or below it, and say
     there when a metric was refused or `unclear`. One block per experiment.
   - Never put a card or badge in a PR without the block. `--format card|badge` is for elsewhere.
4. **The bench**: the case's code, or a link to it. A new case ships in the PR of the first change
   it measures.

Never put hand-timed numbers, single recordings or `--check` values in a results table: label them
noisy discovery evidence, or leave them out.

### When strobe refuses

- Exit 4 names the validator and the fix: fix the setup, never work around it. Common ones:
  - `WindowHidden`, `SessionLocked`: the window must show; wait or unlock.
  - `Background`: other work on the target's CPUs (named); a timed bench waits it out before refusing.
  - `Throttled`: the speed check stayed slow on quiet CPUs, a Linux CPU quota throttled the runs,
    or a headless page ran with the display off.
  - A `--check` never waits on the speed check: a slow one is a `! Background` or `! Throttled` line and
    the case runs at once. Its values are then a loaded machine's.
  - `RunIncomplete` on `cpu.renderer`/`cpu.gpu`/`cpu.browser` names why: a process of that role ended
    in the run, or ended processes (their CPU pooled) used over 1% of it. `cpu.work` keeps
    their CPU: compare that.
  - `RunIncomplete`: a run threw, timed out, its `until` never came, or the app exited. "The host
    wrote none": a script host's profile never came; it says how the host ended and ends `script.log`.
  - `ResetIncomplete`: a run started from another state than run 1 (`the reset did not restore …`).
  - `TailTooShort`: a percentile lacks its minimum n (`interaction.p95` needs 20 samples a block).
  - `AmbiguousInteraction`: over half the inputs came while the target was drawing.
  - `NotOnThisOs`, `NotForTarget`, `EngineUnsupported`, `Headless`, `NoCounter`: this OS, target,
    engine or mode cannot give that metric; pick another primary (the message names one).
  - `OverheadBudget`: strobe's own work disturbed timed runs (a `--check` only warns).
  - `VariantMismatch`, `DisplayTooSmall`, `DeviceMismatch`, `InputMismatch`: the sides differ.
- A refused primary refuses its series; the rest of the file runs. A series refused while it runs
  (`OverheadBudget`, `NoPhase`) ends alone too: `refused` in the listing, `--resume` keeps it. Only
  the machine's refusals (`SessionLocked`, `Background`, `Thermal`, …) stop the bench. Every series
  refused: exit 4.
- `!` lines warn next to a kept number; `i` lines inform. Read them before trusting a number:
  `NotSteady`, `HighVariance`, `ScreenChanged`, `EndedCpu`, `HeadlessFrame`, `ObserverCost`,
  `Rerun`, `StaleCalibration`, `DisplayOff`, `TimerCoalescing`. SPEC §10 lists them all.

## Commands

```text
strobe status [--fast] [--calibrate]
strobe record <target> [--probes timing|attribution|memory] [--for 10s] [--headless]
              [--engine chromium|firefox|webkit] [--emulate mobile|touch] [--mobile[=<serial>]] [--video]
strobe record app:<id>|exe:<path> [--native] [--windowless] [--until-line "<re>"] [--stop-line "<re>"]
strobe record pid:<n>|proc:<name> [--probes timing|deep|waits|stacks-unprivileged] [--for 10s]
strobe record wir:[<selector>] [--probes attribution|memory] [--for 10s]
strobe record --bench <file.bench.ts>#<case> [--head <variant>] [--target <name>] [--max-time 10m]
strobe bench <file.bench.ts> --head <variant> [--base <variant> | --aa | --check [--batch N] [--runs N]
             [--repeat R] | --blocks N]
             [--filter <cases>] [--target <names>] [--params k=v|all] [--primary <metric>]
             [--pairs auto|N] [--runs 5] [--noworse 2%] [--why auto|always|never] [--trace lean|full]
             [--probes memory --repeat 10] [--bisect good..bad] [--pixels | --pixels-only]
             [--engine chromium|firefox|webkit] [--headless] [--wayland] [--mobile] [--video]
             [--pin auto|none|ccdN|<cpus>] [--reserve-cpus auto|on|off] [--power high|keep]
             [--unlock-wait 2h] [--max-time 30m] [--seed N] [--require-verdict]
strobe bench --stop <exp> | --resume <exp>
strobe plan <file.bench.ts> [--base …] [--head …] [--filter …] [--effect 3%] [--pilot]
strobe run [--pin auto] [--reserve-cpus on|off] -- <command>
strobe run --base "<command>" --head "<command>" [--aa | --check] [--metrics metric.<name>,…]
strobe query <rec_…|exp_…|recA..recB> <view> [--top N] [--by …] [--reveal] [--case <name>]
strobe report <exp_…|rec_…|ser_…|result.json> [--format html|md|pr|card|badge] [--open]
strobe report experiment <exp> --format pr --update-pr <n> [-R owner/repo] [--force] [--no-card]
strobe open <rec_…|exp_…[/head|base|<block>|<case>]> [--no-browser | --no-server]
strobe look <url|file|wir:…|app:<id>|exe:<path>|tty:<command>> [--until <selector>] [--out shot.png]
strobe track <file> [--ref <rev> | --range a..b [--every N]] [--schedule commits|nightly|6h]
strobe history [<ser_…|file>]
strobe config [<dir>]
strobe update [<version>] [--from <path>] [--check] [--force]
```

- `strobe <command> --help` lists that command's flags. An unknown flag exits 2 naming the closest;
  so does another command's flag (`status --runs 5`: which commands take it), never ignored.
- `--max-time` is the run's deadline, `--check` and `record --bench` too: no series starts and no wait
  (the bench slot, busy CPUs, a locked session, a launch retried) runs past it. A comparison keeps its
  verdicts; a check stops with exit 124 and `--resume` runs the rest. On a timed `record` and
  `run -- <command>` it caps the wait for the bench slot (exit 124, `ConcurrentBench`).
- `strobe status` lists the benches holding or queued for the slot, and a `slot` line when a bench of
  another user or state folder holds it.
- `record --bench` prints the bench's warnings (`Attribution`, `ProbeOverhead`, `ObserverCost`,
  `TimerCoalescing`); `--json` marks each recording `kind: "attribution"`.
- Every duration needs its unit (`500ms`, `10s`, `2m`, `2h`, `1d`): `--for 2000` exits 2.
- `--filter`, `--target`, `--params`, `--metrics` and `--noworse` given twice add up; any other flag
  given twice with another value exits 2. `--params n=1,n=3` runs both values.
- `-q`: stdout holds only the handles (`rec_…`, `exp_…`); a failure says why on stderr.
- Progress goes to stderr. Windows PowerShell 5.1 with `2>&1` turns each stderr line into an error
  and exits 1: read `$LASTEXITCODE`, or pass `--progress stdout`.
- Query views: `summary` `frames` `inputs` `threads` `gpu` `hotspots` `waits` `alloc` `layout` `style`
  `shifts` `gc` `heap` `leaks` `memory` `http` `log` `network` `insights` `actions` `sql`; `exp_…` also
  `progress` and `http`. `sql` works on any recording (a script's or a native one's: on the trace
  `strobe open` builds). A script's hotspots leave out strobe's host frames (`--source host` keeps them).
  `a..b` and `exp_…` diff `hotspots` `alloc` `layout` `style` `threads` `frames` `summary` `heap`
  `memory`. `query <rec> summary` of a large recording is slow once, then cached.
- `query <rec|exp> actions [--reveal]`: what a block's hooks sent the target, in order, with the
  element hit and the latency sample.
- `look`: a page in a headless browser, a native app, an iPhone tab or a terminal program, nothing
  measured: titles, headings (what `expect` checks), visible text or keyed elements, console errors
  and a screenshot. Use it to check a UI loads; it works on a locked session. A page look is Chromium
  only (`--engine firefox` is refused); `look app:<id> --out shot.png` writes the native window's
  pixels; `look exe:<path> -- <args…>` passes the app its own arguments. A native window holding a
  Chromium web view may show no headings through UI Automation: check its content from a
  `webview()` or `app()` target, or expect a title or a visible control.
- On a laptop with two GPUs, read `gpu.*` with the adapter the target drew on: the `screen` line
  (`drawn on …`), `query <rec> gpu` and `screen.gpu` in the JSON; `CrossAdapterScanout` says when
  another adapter drives the window's display.
- MCP tools `status`, `record`, `bench`, `query`, `report`: the CLI's names, arguments and JSON.
  Through the daemon, `bench` needs an absolute file and `head`.
- Exit codes: `0` · `1` error · `2` usage · `3` needs a human · `4` refused · `5` inconclusive
  (with `--require-verdict`) · `124` timeout or an unlock wait that ran out · `130` stopped ·
  `255` unreachable.
- result.json (`--json`): read each comparison's `verdict_kind` (`better`, `worse`, `same`,
  `no_worse`, `unclear`, `underpowered`, `refused`, `untested`, …) and a refused metric's
  `refusal.validator`, never the `verdict` words. Schema:
  `node_modules/@hona/strobe/schema/result.v1.json`.

## Targets

- `https://…` or `page.html`: Chromium (`STROBE_BROWSER`, else Chrome/Edge/Chromium, else
  Playwright's), `--engine firefox` (stock Firefox 148 or later; an older one is refused
  `EngineUnsupported` before it launches), `--engine webkit` (Safari headed on macOS;
  WebKitGTK on Linux, headed, or `--headless` under `xvfb-run`; WPE headless sees a touch device and
  refuses `DeviceMismatch`; never Windows).
- `app:<id>`: a strobe.json profile (Electron, `kind: "native"` or `kind: "webview"`), launched in an
  isolated home. `exe:<path>` (`--native` for a native app, `--app <id>` borrows a profile).
- `cdp:<port>`, `cdp:ws://…`: attach. `pid:<n>`, `proc:<name>`: from outside. `file:<path>`: import
  a trace, `.cpuprofile` or `.heapsnapshot`. `wir:[<selector>]`: an iPhone's Safari tab.
- Probe sets: `timing` (default), `attribution` (stacks, allocations, layouts, GC, a heap snapshot;
  timing perturbed), `memory` (heap snapshots for leaks), `deep` (native stacks), `waits` (`deep`
  plus off-CPU waits, at every context switch).
- Windows present events (frames) and the deep helper need `strobe up`'s one UAC prompt. Without it
  native stacks come from strobe's own sampler (x64, user frames only, 2 ms of a thread's CPU a
  sample); `record https://… --os-frames` adds OS presents to a Chromium recording.

## A bench file

```ts
import { bench, web, app } from "@hona/strobe"
import { mockFeed } from "../e2e/mocks"

export default bench({
  targets: {
    web: web({
      viewport: { width: 1366, height: 768 },
      prepare: ({ mock, url }) => mockFeed(mock, { server: new URL(url).origin }),
      url: ({ prepared }) => prepared.path,
    }),
    desktop: app("desktop", { home: "perf/fixtures/home" }),
  },
  params: { items: [24, 200] },
  async setup({ page }) {
    await page.waitFor("[data-feed]")
    await page.idle()
  },
  cases: {
    scroll: {
      primary: "main.busy",
      run: async ({ page, phase }) => {
        await phase("up", () => page.scroll("[data-feed]", { by: -4000, speed: 3000 }))
      },
    },
    type: {
      primary: "interaction.p95",
      run: ({ page }) => page.type("[contenteditable]", "hello", { delay: 80 }),
      reset: async ({ page }) => {
        await page.press("ControlOrMeta+a")
        await page.press("Backspace")
      },
    },
  },
})
```

- strobe refuses a key it does not read (in the file, a target, a case, a launch) with the nearest
  key: a case's `setup` is the file's. Cases run in the file's order.
- Targets: `web()` (each variant's build, served by strobe.json `build` and `serve`; a `url` function
  may return an absolute address its `prepare` started), `url(href)` (a running page: the same for
  every build, so `--base <ref> --head .` on it refuses `VariantMismatch`), `app(id)`, `exe(path)`,
  `native(id | path, { window?, output?, display? })`, `webview(id | path)`,
  `command(run, { cwd, env, unset })` (attaches at `DevTools listening on ws://…`),
  `script(entry)` (a Bun or Node module; `ctx.call(name, …)`), `cli(run?)`, `tty(run, { cols, rows })`.
  `fixture(command)` at the top level starts a server per block for both sides.
- `cli("./target/release/tool input.txt")`: one command per `ctx.exec()` (`run: (ctx) => ctx.exec()`),
  run to its exit in the variant's root (the checkout's top folder, a git ref's worktree), measured
  from outside: `cpu.work`, `run.duration`, `cli.launch`, `cpu.wakes` (not Windows), `memory.peak`,
  `io.*`, and `metric.<name>` from `strobe:metric <name>=<value> [higher]` lines it prints (declare
  one in `metrics` to test it). A nonzero exit, a timeout or a process left running fails the run.
- `params: { files: { run: [100, 10_000], more: [1, 1_000] } }`: `more` only with `--params all`.
- `bun add -d @hona/strobe` pins strobe for the project and gives editors the types; strobe runs a
  bench file with its own SDK copy either way.
- Source maps without changing what ships: let the bundler read `STROBE_SOURCEMAP=hidden`
  (Vite: `build: { sourcemap: process.env.STROBE_SOURCEMAP }`).

### `page`

- `goto`, `click(sel, { button, modifiers, count })`, `hover`, `move(x, y)`, `type(sel, text, { delay })`,
  `press("ControlOrMeta+Shift+P")`, `down(key)`/`up(key)` (also `"Mouse"`, `{ at }`), `drag(sel,
  path, { duration })`, `scroll`, `wheel(sel, { dy })`, `window("maximize" | "restore" |
  { resize })`, `act(sel, "invoke" | "toggle" | "setValue", { value })` (native), `query(sel?)`,
  `waitFor(sel, { text, until })`, `idle`, `wait`, `snapshot(name)` (for `--pixels`).
- Input calls take `{ until }`: `"idle"`, `"frame"`, `"animations"` (pages: no finite animation
  running), a selector, or a list run in order under one timeout: `{ until: ["@row", "frame"] }`.
- Clicks aim at the visible part of the element: nothing visible is `OutOfView`, something over it
  `Covered`. `type` is paced (one key a frame, each an `interaction.*` sample);
  `{ burst: true }` refuses `interaction.*` (measure `cpu.work` or a phase).
- `page.eval` takes a function; never call it in `run` (`main.script` refuses after it).
- Headless pages: a run ends at its last step; `! HeadlessFrame` says the frame after it did the
  step's style and layout: give that step `{ until: [selector, "frame"] }`.
- Selectors are one grammar on pages, native apps and terminals: a base (CSS, an accessibility key or
  key path, grid text) then `role=`, `text=` (`text==` the whole text), `index=`, `:focus`; or
  `{ selector, role, text, exact, index }`. Points: `page.click([x, y])`.
- No DOM or tree (a canvas, a game): image anchors, `page.click({ image: "play.png" })`,
  `until: { image }`, `{ pixels: { changed: true } }`; `AnchorNotFound`, `AnchorAmbiguous`. Pixel
  reads cost (`ObserverCost` past 8 a run); find images in setup and click points in runs. Compare
  their cost across builds by `driver.pixel_cpu_ms` (ms a run), not the share of a run's time.
  `resetPixels: { region, ignore, tolerance }` on a target or case: runs that start from other pixels
  are `ResetIncomplete`.

### Case lifecycle

- `setup` once per block, `run` measured, `reset` between runs (never measured), `teardown` after.
  What a run leaves is read before its reset (`dom.nodes`, `js.heap.used`, `memory.app.*`).
- Every run must start from run 1's state: editable controls and toggles are compared
  (`ResetIncomplete`). `resetCheck: false` on a case whose runs build on each other on purpose.
  Select-all is `ControlOrMeta+a`.
- Waits that are not the journey: `await ctx.untimed(() => page.idle())`. Its time, CPU, frames and
  inputs leave the run; `gpu.process.3d`, `energy.*`, `net.*`, `canvas.*` refuse (`Untimed`).
- `ctx.addInitScript(fn, arg)` and `ctx.mock(pattern, handler)` in `prepare` work in every engine;
  Playwright (`ctx.playwright`) only on Chromium.
- Random workloads: draw from `ctx.random()` (each hook call replays the same stream), never
  `Math.random()`; `--seed N` replays; `ctx.seed` seeds the app only if you pass it in.
- `expect: { title, heading, url, visible }` on a case or target: a block on another screen is
  `WrongScreen`, not timed. Check what shows with `strobe look`.
- Launch cases: `launch: { until, milestones? }` instead of `run`: one cold launch per block, timed
  from spawn to `until` (`launch.ready`), `launch.painted`/`launch.settled` from the window's pixels,
  `until: { milestone: "up" }` for a tray app.
- `--check --batch 10`: up to 10 run cases share a launch (same target, params and `setupKey`); each
  must leave the launch's start state. A setup that differs per case: `setupKey: (ctx) => …` and read
  `ctx.setupKey` in `prepare`, `url` and `setup`. Only `--check`: a verdict needs a fresh launch.
- Several untimed known-answer runs without a verdict: `--check --runs 5` (5 runs in the one launch,
  the median and `[lowest … highest run]`) or `--check --repeat 3` (3 fresh cold launches per case,
  each launch's value listed). Still a check: nothing is decided.

### Native apps, webviews, terminals

- `native("my-app")` (a profile with `kind: "native"`) is driven through the OS accessibility tree:
  UI Automation and posted messages (Windows, never takes focus), AX and Quartz events (macOS: needs
  Accessibility for the app strobe runs under; `status` names it), AT-SPI and XTest (Linux X11), or
  strobe's headless Wayland compositor (`--wayland`, or no `DISPLAY`).
- Measures `cpu.work`, `ui.cpu` (the UI thread: the primary for "does it cost less"), `cpu.wakes`,
  `io.*`, `gpu.*`, `memory.app.*`, and from OS presents `frames.*`, `interaction.p95`. A click that
  acts on the press is timed from it.
- `ctx.waitForLine(/saved/, { in: "output" })` (with `native(…, { output: true })`) or a log glob.
  `ctx.home`, `ctx.env`, `ctx.exe` drive the copy through its own CLI where no driver reaches.
- `webview("my-app")`: an app on WebView2 (Windows), its page over CDP; Linux and macOS webviews
  refuse `EngineUnsupported`: use `native()`.
- `tty(["my-tui"])`: a program on strobe's own terminal; look first with
  `strobe look tty:"my-tui" --until text=Ready`. Metrics `tty.latency.*`, `tty.settle.*`,
  `tty.frames`, `tty.bytes`, `cpu.work` (a check line counts the latencies' inputs: `(10 inputs, 2 busy)`;
  busy and silent ones are left out); mouse calls need the program's mouse mode
  (`MouseNotEnabled`). Events: the program appends JSON lines to `STROBE_EVENTS`
  (`{"strobe":"state","name":"done"}` for `until: { state: "done" }`). Wait in setup for what arrives
  late (a status line), and leave a clock out of the reset check with `ignore: [[col, row, cols, rows]]`.
- A directory or git variant runs strobe.json `build` and launches its `output`; `gpuCache: "warm"`
  (the default) keeps each variant's GPU shader caches across launches. A debug build is refused.

### Picking metrics

- Primary: `main.work` (page main-thread work: "does it cost less"), `ui.cpu` or `cpu.work` (native),
  `cpu.wakes` (idle native), `main.busy` (scrolling, streaming), `interaction.p95` (typing, clicks),
  `frames.dropped.pct` (when frames drop), `launch.ready` (launches), `cpu.work` (scripts, commands).
  Fewer than 6 pairs never gets a verdict.
- Where CPU goes: `cpu.renderer`, `cpu.gpu`, `cpu.browser`. Where a frame goes: `frames.renderer.p95`,
  `frames.viz.p95`. Main thread: `main.script`, `main.layout`, `main.style`, `main.paint`, `main.gc`.
- Your own spans: `phase("name", fn)` gives `phase.name`; the app's `performance.measure("x")`
  gives `span.x`. Counters: `ctx.count(name)` or `globalThis.__strobeCount?.(name)` → `count.<name>`,
  `net.requests{/api/*}`, `net.bytes`, `layout.shift`. A stable count compares exactly.
- macOS also gives `energy.cpu` (mJ a run), `cpu.instructions` (tight run to run: small CPU changes
  decide in fewer pairs) and `cpu.idle_wakes`; Linux `cpu.instructions` in counter blocks of their
  own; Windows refuses all three (`NoCounter`).
- A process that ends inside a run: Linux and Windows add the CPU it used (`i EndedCpu`); macOS and
  targets strobe did not launch refuse `cpu.*` (`RunIncomplete`). `· run 2` on a check line: run 1
  lost a process, so strobe measured the run after the reset. `query <rec> summary` lists per run
  what ended (`ended` lines: executable, count, CPU).

## The machine

- Benches take the device's slot per block, switch to the fastest power plan (put back after, also
  after a kill; `--power keep`), pin the target (`--pin auto`: the V-Cache CCD, a hybrid CPU's
  fast cores, else all but the last 1–2 cores; unpinned under 4 cores, over 64 CPUs, or on macOS)
  and keep this user's other processes off its CPUs (`--reserve-cpus`, Windows and Linux).
- Unpinned, strobe and the runner share the target's CPUs: over 5% of a core warns
  `OverheadBudget`, over 20% refuses a timed bench's times (`--check` warns and passes). Fixes:
  4 or more cores, prebuilt `ctx.mock` responses, a heavy mock server in `fixture()`, or `cpu.work`.
- Never bench under background priority (`taskpolicy -b`, nice 19, Windows' idle class): frame and
  latency metrics refuse `BackgroundPriority`.
- macOS: a strobe started by a launchd job without `ProcessType` (a remote shell's daemon) runs with
  coalesced timers (`TimerCoalescing`, a `timers` line in `status`); strobe's own daemon does not.
- macOS frames come from ScreenCaptureKit (Screen Recording), Linux X11 frames from X Present: screen
  times only, so `frames.interval.*` and `frames.dropped*` refuse there while counts and
  `interaction.*` stand.
- A strobe that died may leave its app running: the next `bench`, `record` or `look` ends it.
  Everything strobe launches carries `STROBE_HOME` and `STROBE_OWNER`; `LiveChannel` names a copy
  that is yours or another strobe's.

## strobe.json

- Everything about the app under test lives in its repo: `build` and `serve` for web targets, `apps`
  profiles (`kind`, `exe` per OS, `env`, `unset`, `files`, `guard`, `protocols`, `extends`).
  `build.windows|macos|linux` overlay `build` (`env` merged). Strobe itself names no product.
- A relative `exe` (`./app.exe`, `build/app`) is the project's build, from strobe.json's folder;
  `exe:<path>` resolves from the current folder. `strobe config` shows where each resolves.
- The nearest strobe.json upward from the bench file wins; `build.cwd` and `serve.cwd` resolve
  from the checkout's root. `//` comments and trailing commas are fine; an unknown key exits 2.
- `"$schema": "./node_modules/@hona/strobe/schema/strobe.v1.json"`. `strobe config` checks it.
- If an isolated launch disturbs your own state, strobe stops: add what leaked to the profile.
