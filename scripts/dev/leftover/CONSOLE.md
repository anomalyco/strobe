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
