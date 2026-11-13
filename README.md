# strobe

The referee for a performance claim in a pull request. "This makes it faster" becomes a verdict: strobe
runs the old and the new side in alternating pairs on CPUs of their own, and says which side won and by
how much, with a confidence interval, or that it cannot tell yet. A number it cannot defend, it refuses.

Compare two commands, from any folder, with nothing installed but Bun:

```sh
bunx @hona/strobe run --base "<command on main>" --head "<command on your branch>"
```

Each run starts the command and waits for it to exit; strobe compares their CPU time (`cpu.work`) and
also shows wall time, peak memory and I/O. From an empty folder on a 1-core Linux VM, the first run took
about 8 s: 0.6 s to fetch strobe, 7 s for 8 pairs. `--check --head "<command>"` runs it once to see
that it works (add `--base "<command>"` to run both once).

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/card-dark.png">
  <img alt="44% less CPU time, 78.0% less memory, better in 8 of 8 pairs; before 7,052 µs, this PR 3,904 µs" src="docs/card-light.png">
</picture>

Read the verdict to see which command won, or read `series[].primary.verdict_kind` in `--json`.
`strobe run` exits 0 even when head is slower or `worse`. An `unclear` or `underpowered` result also
exits 0 unless you pass `--require-verdict` (then 5). A refused comparison exits 4. To check that
both commands run, add `--check`: a command that fails makes the check exit 1; timings from a check
are not a verdict.

Known issue: on Windows a command line starts its first program directly, so `a && b`, `a | b` or
`a > out` measures `a` alone. Wrap such a line in `cmd /c "a && b"` on both sides, or use a script. A
fix is designed.

## In your project

From the project's folder:

```sh
bun add -d @hona/strobe                                            # pins strobe for the project
bunx strobe init                                                   # strobe.json and a runnable perf/smoke.bench.ts
bunx strobe bench perf/smoke.bench.ts --head . --check             # each case once: does it work?
bunx strobe bench perf/smoke.bench.ts --base origin/main --head .  # did the branch help? (timed)
bunx strobe report experiment <exp_…> --format pr --update-pr 42   # the card above, in the pull request
```

`strobe init` reads your package.json, Cargo.toml or index.html and writes a bench for a web app, a
library, a server, a native app, an Electron app, a terminal program or a command-line tool (`--target`
picks one). It never replaces a file without `--force`, and prints what to run next. On the same VM, init
took 45 ms and the first check of a page 12 s.

Then make the cases what your users do:

```ts
// perf/scroll.bench.ts
import { bench, web } from "@hona/strobe"

export default bench({
  targets: { app: web() }, // each variant's build, served as strobe.json's build and serve say
  cases: {
    scroll: {
      primary: "main.work",
      run: ({ page }) => page.scroll("main", { by: 4000 }),
      reset: ({ page }) => page.scroll("main", { by: -4000 }),
    },
  },
})
```

## When a number moves

```sh
bunx strobe query <exp_…> insights                      # what changed, the suspects, the next command
bunx strobe record --bench perf/scroll.bench.ts#scroll  # one case under the profilers
bunx strobe query rec_3f9a..rec_7c1d hotspots           # what got slower between two recordings
bunx strobe open rec_3f9a                               # the whole recording in Perfetto
bunx strobe query ./result.json insights                # a result someone sent you, read by its path
```

Times print in microseconds (`18,309,038 µs`); `--unit ms` or `"unit": "ms"` in strobe.json to change it.

`bunx strobe status` says what this machine can measure, which permission is missing, and how much
disk strobe's files take; `bunx strobe prune` frees what nothing uses. strobe ends only processes it
can prove it launched; any it leaves alone, a command lists at its end. Web, Electron and native apps,
terminal programs, servers, libraries and commands, on Windows, macOS and Linux.

**Agents:** read [skill/SKILL.md](skill/SKILL.md). `bunx strobe up` installs `strobe` on your PATH, the
skill as the `strobe` skill, and registers strobe's MCP server.

`strobe update` refreshes the installed copy. A daemon running an MCP bench finishes it before switching
builds. To install without starting or stopping the daemon during setup, run `bunx strobe up --skip
daemon,deep,mcp,calibrate` **from a project pinned to the version you want**. An installed daemon
checks once a minute and defers automatic setup while it sees any registered bench. A **shell bench**
does not prevent a manual `up` or `update`: use the project's pinned binary directly (`bunx strobe`)
and defer installation until the bench ends.

**CI:** With the default `fail-on: worse`, a failed check fails the PR job. Set `noworse` to hold
exact counters to base: a worse counter fails the job, as does a check that ran but compared none.
A leak found with `leak` or a regression in dedicated-runner `mode: compare` also fails the job.
In your own script, check the exit code of `bench --check --base <ref> --noworse count.items` (1 for
failure, 4 if cases ran but compared no counter). Known issue in 0.34.0: an all-gap counter check
exits 0 without comparing anything. Add `--require-parity` when the file has target gaps.
For timed comparisons, read the verdict, not the exit code.
Inputs: [action.yml](action.yml); design: [SPEC.md §26](SPEC.md#26-ci-the-github-action-actionyml).

**Changes:** each release's notes are on [GitHub Releases](https://github.com/anomalyco/strobe/releases).

Packages for Windows (x64, Arm64), macOS (Apple silicon, Intel) and Linux (x64, arm64). Design:
[SPEC.md](SPEC.md).
