# A command's console is not a leftover

Windows x64, 0.42.0 (`0990c47d`). Both executables ran the same six `run --check` lines with redirected streams. A hidden window station was not available. The launcher used a separate desktop on the existing station, never switched to it, and created its driver with `CREATE_NO_WINDOW`. The console executable inherited an invisible console; the GUI executable had none. No fixture needed the input desktop.

The exit codes are from unmodified binaries. The process IDs below are from a second build of the same revision with temporary logging at the exit sweep. `→` means parent to child; parentheses mean already exited. All listed processes were members of that row's launch job; the root's parent and stamp owner is the strobe in the owner column. Roots received both environment stamps at launch. Exited environments cannot be read retrospectively. `H` is the OS console host; `W` is the fixture worker. The last column lists what the old refusal claimed it ended, not proof that a termination beat natural exit.

| Entry | Command | Owner PID:start | Process tree at root exit | Live stamp read | Old exit / named leftover |
|---|---|---|---|---|---|
| console | `bun -e 0` | 15020:134391531469363196 | (bun 34296) | none left | 0 / none |
| console | GUI `--version` | 13996:134391531483251116 | (GUI 16780) | none left | 0 / none |
| console | `cmd /d /c exit 0` | 32636:134391531497287557 | (cmd 36724) | none left | 0 / none |
| console | waited batch shim | 10804:134391531513182394 | (cmd 2792 → bun 36780) | none left | 0 / none |
| console | waited grandchild | 33460:134391531528649790 | (fixture 33736 → W 17500) | none left | 0 / none |
| console | orphan | 33840:134391531543616273 | (fixture 26072) → W 23864 | owner matches | 1 / W |
| windowless | `bun -e 0` | 13836:134391531591279413 | (bun 28816) → H 36100 | owner matches | 1 / H |
| windowless | GUI `--version` | 32040:134391531611072087 | (GUI 34684) | none left | 0 / none |
| windowless | `cmd /d /c exit 0` | 33136:134391531630606644 | (cmd 19800) → H 12120 | owner matches | 1 / H |
| windowless | waited batch shim | 31572:134391531650409391 | (cmd 34432 → bun 612); H 36648 in job | gone by stamp sweep; parent unreadable | 1 / H |
| windowless | waited grandchild | 36496:134391531669963014 | (fixture 9476 → W 12012); H 35392 in job | gone by stamp sweep; parent unreadable | 1 / H |
| windowless | orphan | 612:134391531690158232 | (fixture 28380) → H 23128, W 15604 | both match | 1 / H and W |

The stamped host wins the merge, so its nonzero start defeats `s == 0`; even a job-only host fails the old predicate if its image is gone by the subsequent PID lookup. Both appeared in this trace. For PIDs 36100 and 12120 the full image matched the system directory. Waiting on the host handles *without* ending the job or either PID saw natural exit in 11 and 14 ms. That is evidence for these launches, not a general proof of a host's last client. There is no grace period for actual workers.

## Which launch reproduces it?

The immediate launcher of the measured command matters, not whether a grandparent has a terminal. On the old path `cli::exec` passes 0 creation flags; the job launcher adds `CREATE_SUSPENDED` before assigning the job and resuming. Standard input is `NUL` and both outputs are pipes in **every** cell. These handles do not attach a console. A console-subsystem child normally inherits an attached parent's console. If none is available and creation did not request detachment, Windows allocates a console for it. A GUI-subsystem child does not need one. Whether the exit sweep actually sees its host is also timing-dependent.

| Parent of strobe | `strobe.exe`, before → after | `strobew.exe`, before → after |
|---|---|---|
| Terminal (invisible attached console) | 0 → 0 | 1 → 0 |
| No console, as for an editor or agent (console entry explicitly detached) | 1 → 0 | 1 → 0 |
| Script run by a console program, explicit binary path | 0 → 0 | 1 → 0 |
| Private HTTP daemon, `bench` tool running the same command | not the daemon entry | failed → passed |

`parents.ts` makes the parent with `CREATE_NO_WINDOW` (attached but invisible) or `DETACHED_PROCESS`, and checks `GetConsoleCP` in that parent. Both inputs use the same environment, job launcher and redirected streams. An attached **grandparent** did not make the GUI executable inherit its console. The ordinary leftover harness used one binary, `STROBE_BIN` or bare `strobe`; `next-commands.ts` uses `--strobe` or bare `strobe`. The latter resolved to the console executable on this PATH. A pass without the resolved binary and flags cannot establish which path another session took.

`daemon.ts` sends an authenticated request to a daemon it starts on a private loopback port with its own state. The desk runs used scratch builds with stale-owner maintenance disabled on both sides so the daemon could not clean up another session. The old daemon (`f228deca`) refused for host 56428; the candidate passed. No installed daemon was stopped or reconfigured.

## What the candidate ends

| Work at root exit | Both executables | Individually named in a stored or published refusal |
|---|---|---|
| Console command, GUI command, shell, waited batch or waited console grandchild | Pass; no PID signal. Normal teardown closes the launch job. | Nothing |
| Background worker sharing the console | Refuse; the job ends the worker. | Worker basename only |
| Background worker with a second console | Refuse; the job ends the worker. Neither host is signalled by PID. | Worker basename only |
| Worker started below the real console host | Refuse; inspecting each member still finds the worker. | Worker basename only |
| Worker named `conhost.exe` outside the system directory | Refuse; the job ends the worker. | `conhost.exe` (the fixture, not the OS host) |

The fixture records the worker PID at creation and checks it is gone after the refusal. No ended PID belongs in saved prose. On old results, only the list inside the command-exit sentence loses PIDs; a name alone is not proof of a system image.

## Known answers

Windows x64 release build, `known/cli.bench.ts --check --runs 3`, median, 8% machine CPU at start. Discovery readings, not a paired performance verdict. Times in ms, peak in MiB.

| Plant | Console CPU / wall / peak | Windowless CPU / wall / peak |
|---|---:|---:|
| Spin 300 ms CPU | 305.38 / 315.37 / 0.852 | 307.00 / 327.30 / 0.855 |
| Sleep 500 ms | 5.75 / 508.70 / 0.805 | 6.48 / 518.08 / 0.805 |
| Three children, 1,200 ms CPU | 1,227.86 / 433.45 / 0.859 | 1,222.65 / 439.46 / 0.855 |
| Allocate 200 MiB | 27.14 / 329.71 / 201.207 | 30.95 / 343.05 / 201.223 |

Just dropping the host from the refusal would still count its cycles (55,566,320 in the empty command's baseline job reading), peak and IO. With a complete job reading, the candidate excludes those counters; without an inherited console it requests `CREATE_NO_WINDOW`. This is **not** identical wall time: creating an invisible console still takes real time. The 500 ms sleep differed by 1.84%; the spin's wall times differed by 3.78% even though CPU differed by 0.53%. Do not mix entry points for short wall-time comparisons. Neither verified Windows CLI record in `src/support.rs` names an executable: it records the fixture, revision, machine, numbers and boundaries. The documented fixture invocation uses bare `strobe`, not `strobew`. That supports the console interpretation but is not an executable-path receipt for the historical run.
