//! Argument parsing (hand-rolled, like mesh) and help text.

use crate::model::{Error, Result};
use std::collections::HashMap;

/// How a flag takes its value. `Number` is a value that MCP tool schemas type as a number. `Optional`
/// is a switch that may name one thing, only as `--name=<value>` (`--mobile`, `--mobile=<serial>`): the
/// next argument is never taken as its value. Bare, it reads as an empty value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Switch,
    Value,
    Number,
    Optional,
}

use Kind::{Number, Optional, Switch, Value};

/// Every flag strobe reads, how it takes its value, and the commands that read it (`*`: every
/// command), which `strobe <command> --help` lists. The parser refuses any other flag (a typo is an
/// error, not a silently ignored setting), a command refuses a flag not listed for it
/// (`Args::check_command`), and MCP tool schemas take their types from here (SPEC §17), so the two
/// cannot drift. A new flag goes here, with every command that reads it: one a command passes on to
/// another (`plan --pilot` and `run` to bench, `record --bench` to bench) is that command's too.
pub const FLAGS: &[(&str, Kind, &str)] = &[
    // Global.
    ("json", Switch, "*"),
    ("quiet", Switch, "*"),
    ("verbose", Switch, "*"),
    ("help", Switch, "*"),
    ("version", Switch, "*"),
    ("progress", Value, "*"),
    // The unit times print in: us (default), ms or s.
    ("unit", Value, "record bench plan query report look run status top track history"),
    // record, bench, plan, run.
    ("id", Value, "record bench run"),
    ("for", Value, "record"),
    ("probes", Value, "record bench plan"),
    ("headless", Switch, "record bench plan track"),
    ("wayland", Switch, "record bench plan track look"),
    ("engine", Value, "record bench plan track"),
    ("served", Value, "record"),
    ("emulate", Value, "record"),
    ("mobile", Optional, "record bench plan look up"),
    ("channel", Value, "up"),
    ("style", Switch, "record bench plan"),
    ("native", Switch, "record query"),
    ("windowless", Switch, "record look"),
    ("app", Value, "record look"),
    ("home", Value, "record look"),
    ("os-frames", Switch, "record"),
    ("video", Switch, "record bench"),
    ("bench", Value, "record"),
    ("gpu-cache", Value, "record"),
    ("until-line", Value, "record"),
    ("stop-line", Value, "record"),
    ("head", Value, "record bench plan track run"),
    ("base", Value, "bench plan track run"),
    ("aa", Switch, "bench plan run"),
    ("check", Switch, "bench update run up"),
    ("filter", Value, "bench plan track"),
    ("target", Value, "record bench plan top track init"),
    ("params", Value, "record bench plan track"),
    ("primary", Value, "bench plan track run"),
    ("equivalence", Number, "bench plan track run history"),
    ("noworse", Value, "bench plan run"),
    ("pairs", Value, "bench plan run track"),
    ("runs", Number, "record bench plan track run"),
    ("repeat", Number, "bench"),
    ("why", Value, "bench plan"),
    ("trace", Value, "bench plan track"),
    ("power", Value, "record bench plan run track"),
    ("pin", Value, "record bench plan run track"),
    ("reserve-cpus", Value, "bench plan run track"),
    ("pixels", Switch, "bench"),
    ("pixels-only", Switch, "bench"),
    ("bisect", Value, "bench"),
    ("keep", Switch, "bench"),
    ("max-time", Value, "record bench run"),
    ("unlock-wait", Value, "bench"),
    ("require-verdict", Switch, "bench run"),
    ("require-parity", Switch, "bench"),
    ("plan", Switch, "bench"),
    ("effect", Number, "bench plan"),
    ("pilot", Switch, "bench plan"),
    ("stop", Value, "bench"),
    ("resume", Value, "bench"),
    ("gpu-store", Value, ""),
    ("metrics", Value, "run"),
    ("seed", Number, "record bench plan run track"),
    // query, report, open, history.
    ("by", Value, "query"),
    ("source", Value, "query"),
    ("top", Number, "query history"),
    ("reveal", Switch, "query"),
    ("at", Value, "query"),
    ("thread", Value, "query"),
    ("pid", Value, "query"),
    ("case", Value, "query"),
    ("phase", Value, "query"),
    ("timeline", Switch, "query"),
    ("format", Value, "report"),
    ("out", Value, "report look"),
    ("open", Switch, "report"),
    ("update-pr", Number, "report"),
    ("repo", Value, "report"),
    ("force", Switch, "report update init prune"),
    ("no-card", Switch, "report"),
    ("labels", Value, "report"),
    ("theme", Value, "report"),
    ("badge-url", Switch, "report"),
    ("scoreboard", Switch, "report"),
    ("no-browser", Switch, "open"),
    ("no-server", Switch, "open"),
    // track.
    ("ref", Value, "track"),
    ("range", Value, "track"),
    ("every", Number, "track"),
    ("blocks", Number, "bench track"),
    ("batch", Number, "bench"),
    ("again", Switch, "track"),
    ("confirm", Switch, "track"),
    ("no-confirm", Switch, "track"),
    ("alert", Value, "track"),
    ("alert-command", Value, "track"),
    ("alert-webhook", Value, "track"),
    ("schedule", Value, "track"),
    ("schedules", Switch, "track"),
    ("unschedule", Value, "track"),
    ("from-schedule", Value, "track"),
    // look.
    ("until", Value, "look"),
    ("wait", Value, "look"),
    ("size", Value, "look"),
    ("scale", Number, "look"),
    ("chars", Number, "look"),
    ("keys", Number, "look"),
    ("full", Switch, "look"),
    ("element", Value, "look"),
    ("headed", Switch, "look"),
    ("build", Switch, "look"),
    ("timeout", Value, "look"),
    ("type", Value, "look"),
    ("press", Value, "look"),
    // status, top.
    ("calibrate", Switch, "status"),
    ("fast", Switch, "status"),
    ("disk", Switch, "status"),
    // prune.
    ("all", Switch, "prune"),
    ("older-than", Value, "prune"),
    ("dry-run", Switch, "prune"),
    ("once", Switch, "top"),
    ("interval", Value, "top"),
    ("limit", Number, "top"),
    ("sort", Value, "top query"),
    // up, update, leave, helper.
    ("skip", Value, "up"),
    ("yes", Switch, "leave"),
    ("from", Value, "update"),
    ("log", Value, "helper"),
    ("sid", Value, "helper"),
];

/// Flags whose value is a comma-separated list: given twice, the lists add up (`--filter a --filter b`
/// is `--filter a,b`). Any other flag given twice with another value is a usage error, never the last
/// one silently.
const LISTS: &[&str] = &["filter", "target", "metrics", "params", "noworse", "skip"];

/// How `--name` takes its value, when strobe has such a flag.
pub fn kind(name: &str) -> Option<Kind> {
    FLAGS.iter().find(|(n, _, _)| *n == name).map(|(_, k, _)| *k)
}

/// Every command strobe has (`app::dispatch`).
pub const COMMANDS: &[&str] =
    &["record", "bench", "plan", "query", "report", "open", "look", "run", "status", "top", "track", "history", "prune", "up", "update", "leave", "helper", "mcp", "daemon", "config", "init", "stats"];

/// Flags a command reads that `FLAGS` does not list for it: only to refuse them in its own typed way
/// (`look --engine firefox` is `EngineUnsupported`, exit 4, never a page in Chromium).
const REFUSED_BY_COMMAND: &[(&str, &str)] = &[("look", "engine")];

/// Whether `command` reads `--name`: `FLAGS` lists it for that command or for every one.
pub fn takes(command: &str, name: &str) -> bool {
    FLAGS.iter().any(|(n, _, c)| *n == name && (*c == "*" || c.split(' ').any(|c| c == command)))
}

/// `strobe <command> --help`: that command's lines of the help, then every flag it reads (from
/// `FLAGS`). A command the help does not name gets the whole help.
pub fn help(command: &str) -> String {
    let mut lines = Vec::new();
    let mut taking = false;
    for l in HELP.lines() {
        let indent = l.len() - l.trim_start().len();
        if indent == 2 {
            taking = l[2..].split([' ', '\t']).next() == Some(command);
        } else if indent < 2 {
            taking = false;
        }
        if taking {
            lines.push(l);
        }
    }
    if lines.is_empty() {
        return HELP.to_string();
    }
    let mut out = format!("strobe {command}\n\n{}\n\nFlags\n", lines.join("\n"));
    let mut row = String::from(" ");
    for (name, kind, _) in FLAGS.iter().filter(|(_, _, c)| c.split(' ').any(|c| c == command)) {
        let flag = format!(
            " --{name}{}",
            match kind {
                Switch => "",
                Optional => "[=<value>]",
                Value | Number => " <value>",
            }
        );
        if row.len() + flag.len() > 100 {
            out += &format!("{row}\n");
            row = String::from(" ");
        }
        row += &flag;
    }
    out += &format!("{row}\n");
    out += "Global    --json  -q  -h  -V  --progress stderr|stdout|off\n";
    out += "Schema    result.json and --json: schema/result.v1.json (node_modules/@hona/strobe/schema/)\n";
    out
}

/// The refusal for a flag strobe does not have, naming the closest one.
pub fn unknown_flag(name: &str) -> Error {
    let distance = |a: &str, b: &str| {
        let b: Vec<char> = b.chars().collect();
        let mut row: Vec<usize> = (0..=b.len()).collect();
        for (i, ca) in a.chars().enumerate() {
            let mut prev = row[0];
            row[0] = i + 1;
            for (j, cb) in b.iter().enumerate() {
                let cur = row[j + 1];
                row[j + 1] = (prev + usize::from(ca != *cb)).min(row[j] + 1).min(cur + 1);
                prev = cur;
            }
        }
        row[b.len()]
    };
    let near = FLAGS.iter().map(|(n, _, _)| (distance(name, n), *n)).filter(|(d, n)| *d <= 1 + name.len() / 4 || (name.len() >= 3 && n.starts_with(name))).min();
    let e = Error::usage(format!("unknown flag --{name}"));
    match near {
        Some((_, n)) => e.hint(format!("did you mean --{n}? strobe <command> --help lists a command's flags")),
        None => e.hint("strobe <command> --help lists a command's flags"),
    }
}

/// A flag's value from the command line: a list flag given again adds to its list (`LISTS`); another
/// flag given again must say the same.
fn given(flags: &mut HashMap<String, String>, name: &str, value: &str) -> Result<()> {
    match flags.get_mut(name) {
        None => {
            flags.insert(name.to_string(), value.to_string());
        }
        Some(was) if LISTS.contains(&name) => {
            if !value.trim().is_empty() {
                *was = if was.trim().is_empty() { value.to_string() } else { format!("{was},{value}") };
            }
        }
        Some(was) if was == value => {}
        Some(was) => {
            let shown = |v: &str| if v.is_empty() { format!("--{name}") } else { format!("--{name} {v}") };
            return Err(Error::usage(format!("--{name} is given twice ({} and {}): give it once", shown(was), shown(value))));
        }
    }
    Ok(())
}

#[derive(Clone)]
pub struct Args {
    pub command: String,
    pub pos: Vec<String>,
    flags: HashMap<String, String>,
}

impl Args {
    pub fn parse(raw: Vec<String>) -> Result<Args> {
        let mut pos = Vec::new();
        let mut flags = HashMap::new();
        let mut iter = raw.into_iter().peekable();
        while let Some(arg) = iter.next() {
            // `--` ends strobe's flags: the rest is a command line of its own (`strobe run -- …`).
            if arg == "--" {
                pos.push(arg);
                pos.extend(iter.by_ref());
                break;
            }
            let name = match arg.as_str() {
                "-h" => Some("help".to_string()),
                "-V" => Some("version".to_string()),
                "-q" => Some("quiet".to_string()),
                "-v" => Some("verbose".to_string()),
                // As docker's (`prune -f`).
                "-f" => Some("force".to_string()),
                // As gh names a repository (`report … --update-pr 12 -R owner/repo`).
                "-R" => Some("repo".to_string()),
                a if a.starts_with("--") && a.len() > 2 => Some(a[2..].to_string()),
                // A short flag strobe does not have (a negative number stays an argument).
                a if a.len() > 1 && a.starts_with('-') && !a[1..].starts_with(|c: char| c.is_ascii_digit() || c == '.') => {
                    return Err(Error::usage(format!("unknown flag {a}")).hint("short flags: -h -V -q -v -R -f; strobe --help lists the rest"));
                }
                _ => None,
            };
            let Some(name) = name else {
                pos.push(arg);
                continue;
            };
            if let Some((key, value)) = name.split_once('=') {
                match (kind(key).ok_or_else(|| unknown_flag(key))?, value) {
                    // `--check=false` is the switch off, not on.
                    (Switch, "true") => {
                        flags.insert(key.to_string(), "true".to_string());
                    }
                    (Switch | Optional, "false") => {
                        flags.remove(key);
                    }
                    (Switch, _) => return Err(Error::usage(format!("--{key} is a switch: --{key}, --{key}=true or --{key}=false, not --{key}={value}"))),
                    (Optional, "true") => given(&mut flags, key, "")?,
                    _ => given(&mut flags, key, value)?,
                };
                continue;
            }
            match kind(&name).ok_or_else(|| unknown_flag(&name))? {
                Switch => {
                    flags.insert(name, "true".to_string());
                    continue;
                }
                Optional => {
                    given(&mut flags, &name, "")?;
                    continue;
                }
                Value | Number => {}
            }
            let value = iter.next().ok_or_else(|| Error::usage(format!("--{name} needs a value")))?;
            given(&mut flags, &name, &value)?;
        }
        let command = if pos.is_empty() { String::new() } else { pos.remove(0) };
        Ok(Args { command, pos, flags })
    }

    /// Refuses (exit 2) a flag strobe has that this command does not read: a flag given to the wrong
    /// command is a mistake, never a setting silently ignored (`look --engine` once ran Chromium).
    pub fn check_command(&self) -> Result<()> {
        let mut given: Vec<&str> = self.flags.keys().map(String::as_str).collect();
        given.sort_unstable();
        let command = self.command.as_str();
        let Some(name) = given.into_iter().find(|n| !takes(command, n) && !REFUSED_BY_COMMAND.contains(&(command, *n))) else { return Ok(()) };
        let users: Vec<&str> = FLAGS.iter().filter(|(n, _, _)| *n == name).flat_map(|(_, _, c)| c.split(' ')).filter(|c| !c.is_empty()).collect();
        let who = match users.as_slice() {
            [] => "strobe sets it itself; no command takes it from the command line".to_string(),
            [one] => format!("only {one} takes it"),
            [rest @ .., last] => format!("{} and {last} take it", rest.join(", ")),
        };
        Err(Error::usage(format!("{command} has no flag --{name} ({who})")).hint(format!("strobe {command} --help lists {command}'s flags")))
    }

    /// Sets a flag (a derived command line, such as one bisect step).
    pub fn set(&mut self, name: &str, value: &str) {
        self.flags.insert(name.to_string(), value.to_string());
    }

    pub fn remove(&mut self, name: &str) {
        self.flags.remove(name);
    }

    pub fn has(&self, name: &str) -> bool {
        self.flags.contains_key(name)
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.flags.get(name).map(String::as_str)
    }

    pub fn num(&self, name: &str) -> Result<Option<f64>> {
        self.get(name).map(|v| v.trim_end_matches('%').parse::<f64>().map_err(|_| Error::usage(format!("--{name}: not a number: {v}")))).transpose()
    }

    pub fn json(&self) -> bool {
        self.has("json")
    }

    /// The command line as JSON, kept with an experiment so `--resume` runs it again.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({ "command": self.command, "pos": self.pos, "flags": self.flags })
    }

    pub fn from_json(v: &serde_json::Value) -> Option<Args> {
        Some(Args { command: v["command"].as_str()?.to_string(), pos: serde_json::from_value(v["pos"].clone()).ok()?, flags: serde_json::from_value(v["flags"].clone()).ok()? })
    }
}

/// Parses `500ms`, `10s`, `2m`, `2h`, `1d` (and a bare `0`); a negative or non-finite one, or a number
/// without its unit, is refused: `--for 2000` meant as milliseconds once recorded for 33 minutes.
pub fn duration_ms(text: &str) -> Result<u64> {
    let t = text.trim();
    let (num, mult) = if let Some(n) = t.strip_suffix("ms") {
        (n, 1.0)
    } else if let Some(n) = t.strip_suffix('s') {
        (n, 1000.0)
    } else if let Some(n) = t.strip_suffix('m') {
        (n, 60_000.0)
    } else if let Some(n) = t.strip_suffix('h') {
        (n, 3_600_000.0)
    } else if let Some(n) = t.strip_suffix('d') {
        (n, 86_400_000.0)
    } else {
        if t.parse::<f64>().is_ok_and(|v| v != 0.0) {
            return Err(Error::usage(format!("not a duration: {text}; give a unit: {text}ms or {text}s")));
        }
        (t, 1000.0)
    };
    match num.trim().parse::<f64>() {
        Ok(v) if v.is_finite() && v >= 0.0 => Ok((v * mult) as u64),
        _ => Err(Error::usage(format!("not a duration: {text} (500ms, 10s, 2m, 2h, 1d)"))),
    }
}

pub const HELP: &str = "strobe — measure and explain performance

Usage: strobe <command> [flags]

Use
  status [--fast] [--calibrate] [--disk]
                                  environment, validity preflight, recent recordings and experiments, the disk
                                  strobe's files take and what strobe prune frees (--disk: by category)
  record <target> [--for 10s] [--probes timing|attribution|memory] [--headless] [--emulate mobile|touch]
         [--served <dir>] [--style]
                                  record a target; prints a handle (rec_…); exe:<path> --native: a native app
                                  (--engine firefox|webkit: a page in that engine, from outside; --wayland: a
                                  native app on strobe's headless Wayland compositor, Linux)
                                  (a native app: --gpu-cache warm|cold, its GPU shader caches from earlier
                                  recordings of it; --windowless: it shows no window (a tray app, a service),
                                  as native(…, { window: false }))
                                  (--video: also film it, timing perturbed: the window's screen, or a headless
                                  page's renderer)
                                  (--mobile[=<serial>]: the page in Chrome on an Android phone over adb, its
                                  own screen and touch; timed: no emulator, no adb over Wi-Fi, no hot, saving
                                  or low phone)
                                  (--max-time 5m: a timed recording caps its wait for the bench slot, exit 124
                                  ConcurrentBench)
  record wir:[<selector>] [--for 10s] [--probes attribution|memory] [--served <dir>]
                                  an open Safari tab (or WKWebView) on a USB-cabled iPhone over Web Inspector:
                                  Timeline, ScriptProfiler, Heap, Network, Safari/WebKit device logs and
                                  crashes (read-only)
  record --bench <file.bench.ts>#<case> [--head <variant>] [--target <name>] [--video]
                                  record one bench case (it drives the page: clicks, keys), attribution probes
                                  (its block ends with a heap snapshot; --probes waits adds off-CPU waits to a
                                  native case; --probes memory exits 2: a case's leak run is bench --probes
                                  memory)
  record pid:<n>|proc:<name> [--probes timing|deep|waits|stacks-unprivileged] [--for 10s]
                                  any process from outside: OS present events (frames), CPU, GPU; deep: stacks
                                  (--os-frames on a Chromium record adds the OS present events to its trace)
  bench <file.bench.ts> --head <variant> [--base <variant> | --aa | --check | --blocks N] [--filter <cases>]
        [--pairs auto|N] [--why auto|always|never] [--trace lean|full] [--power high|keep]
                                  run an experiment: a verdict per series (case × target × params); --check
                                  runs each case once to see that it works (no verdict)
        [--params k=v,…|all]           (all: every parameter's `more` values too)
        [--probes memory --repeat N]   (a leak run per variant instead of pairs; not with --check, which takes
                                        no heap snapshot: exit 2)
        [--pixels | --pixels-only]     (compare base and head at the case's page.snapshot()s, after or instead
                                        of timing)
        [--video]                      (film untimed: the check run, or one warmed run per side after the
                                        timed pairs)
        [--check --batch N]            (up to N run cases per launch, each held to the launch's start after
                                        its reset)
        [--check --runs N]             (N runs in each case's one launch, no warmup: values their median,
                                        [lowest … highest run]; runs that stepped down warn NotSteady)
        [--check --repeat R]           (R fresh cold launches per case, from 2: the median over them and each
                                        launch's value; not with --batch or --video)
        [--check --base <variant>]     (each case once on base and on head, each line base's under head's;
                                        nothing compared but --noworse's exact counters; a case base cannot
                                        build or run fails, exit 1: head was checked against nothing)
        [--check --noworse count.x=0%,…]  (with --base: exact counters only (count.*, net.requests, net.bytes,
                                        tty.frames, dom.nodes; a bare name is 0%); each must hold one value
                                        per side (2 runs with a reset, else 2 launches) and head stay within
                                        its margin of base, or the case fails, exit 1 (worse, unstable,
                                        missing, not_compared); a count.* only --noworse names reads 0 in a
                                        block that counted none once any block of either side counted it, and
                                        nothing at all when none did; a check that compared none of the
                                        counters it names exits 4 (Uncounted says why): check the name against
                                        the runs' ctx.count calls, and declare a counter a run counts only on
                                        some paths in that case's metrics)
        [--max-time 30m]               (the bench's time cap, its waits included (the bench slot too): a
                                        comparison keeps the pairs it has, anything else stops, exit 124)
        [--mobile[=<serial>]]          (phone targets, emulate: \"mobile\", in Chrome on an Android phone over
                                        adb instead of emulated; timed blocks refuse an emulator: --check and
                                        leak runs run there)
        [--pin auto|none|ccdN|<cpus>]  (auto: the V-Cache CCD; equal CCDs: the first; a hybrid CPU: its
                                        fastest cores; otherwise all but the last 1-2 whole cores, which
                                        strobe and the runner get; fewer than 4 cores, more than 64 CPUs, or
                                        macOS: unpinned)
        [--reserve-cpus auto|on|off]   (keep this user's other processes off the target's CPUs: timed blocks,
                                        every block, or never)
        [--bisect good..bad]           (the commit that moved the primary; one series)
        [--style]                      (attribution blocks also trace what invalidated style: query <rec>
                                        style)
        [--noworse 2% | metric=2%,…]   (non-inferiority: is head at most 2% worse? verdicts no worse, worse,
                                        unclear; a bare exact counter is 0%, a bare other metric exits 2; with
                                        --check: exact counters only, as --check --noworse above)
        [--unlock-wait 2h]             (a series that needs a presenting window waits at most that long for an
                                        unlock, or on macOS for nobody to use the Mac)
        [--seed <n>]                   (ctx.seed and ctx.random(): replay a seeded workload; without it strobe
                                        draws one and prints it)
        a server: host it in a script() target and probe it in setup, ctx.httpProbe({ url, authorization,
                                       expect }); declare http.latency.p95 or http.timeouts, cpu.main for its
                                       main thread (the runnable example: node_modules/@hona/strobe/README.md)
        --filter: case names, comma-separated (or --filter given again); exact unless a name holds * (a glob)
        --base <target>[=<variant>] --head <target>[=<variant>]: two targets of the file compared (two
                                  implementations of one app; with --check: bare target names); target:<name>
                                  when a folder or ref has that name
        a shared CI runner (GitHub-hosted) refuses timing (SharedRunner): --check (with --base and --noworse
                                  on exact counters), leak runs and record --probes attribution run there
        exit: a comparison 0 whatever its verdicts (read them; --require-verdict: 5 when one is undecided, 4
                                  when one is refused), 4 when every series is refused; a check 0 when every
                                  case passed, 1 when one failed, 4 when it compared nothing it was asked to
                                  (--noworse) or every case was refused; 124 timeout, 130 stopped
  plan <file.bench.ts> [--base <variant>] [--head <variant>] [--filter …] [--effect <pct>] [--pilot]
                                  before a bench: pairs and minutes per series from earlier runs here, builds,
                                  the slot's queue (= bench --plan; --pilot runs 4 pairs first to learn the
                                  spread); a series no block of which ran here says its time is unknown, never
                                  a guess (a --check or --pilot measures one)
  bench --stop <exp> | --resume <exp>
                                  stop a running bench after its block (at once while it waits; it keeps what
                                  finished), or go on with one (a unique prefix of the id works)
  config [<dir>]                  check the project's strobe.json (and your own) without a bench: where it is,
                                  each app profile resolved for this OS (its executable, guards, window),
                                  build and serve
  query <rec_…|exp_…|rec_a..rec_b> <view> [--top N] [--by function|file|package]
        [--source console|device|crash|host|startup|setup] [--reveal] [--native] [--thread <name>]
                                  views: summary frames inputs threads gpu hotspots waits alloc layout style
                                  shifts gc heap leaks memory http log network insights actions sql runs
                                  (runs: every run of a block recording, warmup too: time and work per run,
                                  which are measured)
                                  (inputs [--case <c>] [--top N]: each measured input: its run and time, what
                                  proved its change, the frames around it, its latency or why it gave none
                                  (and how much later it had to come); the first 100 by default, --json lists
                                  every one)
                                  (sql: a recording without a Chrome trace: the Perfetto trace strobe open
                                  builds, built on first use)
                                  (hotspots: --sort self|total; native: --thread <name> --pid <pid|exe>;
                                  --phase <name>: native samples, or a script recording's profile; --source
                                  host: a script target's whole host process (its measured runs by default),
                                  by lifecycle part at the end; --source startup|setup: the host's startup
                                  (the runtime's boot, the target's static imports) or the bench file's setup
                                  alone (a server's dynamic imports); not with --native, --thread, --pid or
                                  --phase; --case <name>: one case of a --batch recording)
                                  (http: a bench block's HTTP probe (ctx.httpProbe): idle against the runs,
                                  timeouts, errors, missed sends, stalled cohorts (one stall, or stalls back
                                  to back that held probes across a drain); --phase <name>; exp_… its probe
                                  blocks, a..b too)
                                  (actions: what a bench block's hooks sent the target, in order; --phase;
                                  --reveal shows typed text, act values, ctx.call arguments and ctx.exec
                                  lines)
                                  (waits: where threads were off their CPUs and who woke them, from a native
                                  capture; --thread <name> --phase <name> --by function|file|package)
                                  (a..b: hotspots alloc layout style threads frames summary heap memory,
                                  per-run deltas; exp_…: summary progress insights http, and those views as
                                  each series' base..head; memory: --by process|layer|allocator --at <i>; a
                                  phone's page lists its processes from Chromium's dump, with no OS memory;
                                  threads --timeline: each thread's CPU per second from the launch, exited
                                  ones too)
                                  (a handle may be a path: a result.json, a meta.json or the folder of either,
                                  read as it is, as report reads it; a named folder holding a link out of it
                                  is refused, and nothing it says makes strobe look elsewhere on the disk or
                                  the network)
                                  (insights: rule hits with their evidence: dominant-thread (one thread off
                                  the UI thread doing most of the CPU), off-cpu-bound (runs that mostly
                                  waited), style-layout-dominant, wakes-while-idle (idle recordings and idle
                                  native blocks), hot-function, http-stalls, forced-layout, gc-pressure, …;
                                  exp_…: insights per series: verdict or status, ranked suspects, the warnings
                                  that bear on it, the commands to run next)
  report [experiment|recording] <handle> [--format html|md|pr] [--out <path>] [--open]
                                  write a self-contained HTML (or markdown) report; pr: an experiment's tables
                                  for a PR body
                                  (md and pr print the markdown; the file's path goes to stderr; the view
                                  follows from the handle; a path to a result.json, a meta.json or the folder
                                  of either is read as query reads it, nothing written beside it; a bench
                                  --blocks run: HTML and md, its blocks' median and interval, no verdict; a
                                  format the handle has none of exits 2 before anything is written)
  report experiment <exp> --format summary [--out <file.md>]
                                  the result as short GitHub Markdown for a CI job's summary page
                                  ($GITHUB_STEP_SUMMARY): a check, pixels run, leak run or --blocks as one
                                  table (a check shows only exact counters, base -> head with --noworse when
                                  it ran both; never a time), a comparison as its PR block without images
  report experiment <exp> --format pr --update-pr <n> [-R owner/repo] [--force] [--no-card]
         [--labels <base>,<head>]
                                  put the block in PR n's body, with the result card and badges attached
                                  through gh; the sides read Before and This PR only when head is the working
                                  tree or HEAD, and with --update-pr at the PR's head commit; else each side's
                                  commit (A/A: Base (A/A), Head (A/A)); --labels names them (pr and card); pr,
                                  summary, card and badge are public: every string of the result is scrubbed
                                  first (no other program's name or window title, no person's folder; a plain
                                  folder variant reads <base folder> or <head folder>)
  report experiment <exp> --format card|badge [--out <file.png|file.svg>] [--theme light|dark|auto]
         [--badge-url] [--scoreboard] [--labels <base>,<head>]
                                  the result as an image: card 1200×630 (decided wins large, else what
                                  happened), badge one line; SVG follows the reader's colour scheme (auto);
                                  --badge-url prints a shields.io URL; --scoreboard: an implementation
                                  comparison's journeys (vs <base> | better in 4/5 · 1 gap)
  open <rec_…|exp_…[/head|base|<block>|<case>]> [--no-browser | --no-server]
                                  the recording in the Perfetto UI (local): one trace with the Chrome trace,
                                  CPU profiles and native stacks as flame graphs, runs, phases, inputs,
                                  presents, threads, memory
                                  (--no-browser: print the URL instead of opening it; --no-server: build and
                                  check the trace and print what it holds, no UI and no server)
  track <file.bench.ts> [--ref <rev> | --range a..b [--every N]] [--filter …] [--blocks 6] [--no-confirm]
        [--alert-command '<cmd>'] [--alert-webhook <url>] [--alert regressions|all]
                                  a point per series per commit in this device's history; change points are
                                  confirmed by a paired A/B of the commits around them, then bisected; only
                                  confirmed ones alert
  track <file> --ref <branch> --schedule commits|nightly|6h
                                  the daemon tracks the branch (--schedules, --unschedule <id>)
  track <file> --base <target>[=<variant>] --head <target>=git:<ref> [--range a..b] [--pairs 3] [--schedule …]
                                  a port: per head commit, a few alternating pairs of both implementations,
                                  ln(head/base) per journey; a move is confirmed by an A/B of the head alone
                                  and bisected; a comparator that changed is an epoch, never an alert;
                                  journeys going gap -> compared are marked
  track --confirm [<ser_…|chg_…>] confirm suspected changes (or one again)
  history [<ser_…|file>]          tracked series: points with intervals, a sparkline, changes and culprits; a
                                  port's journeys x commits (query history|changes, query <ser_…>
                                  history|changes; report history <ser_…|file> [--format card | --format badge
                                  [--scoreboard]])
  look <url|file> [--until <css>] [--wait 1s] [--size 1280x800] [--out <png>] [--chars 4000]
       [--full | --element <css>] [--headed]
       [--mobile[=<serial>]]       (the page in Chrome on an Android phone or emulator, at the phone's own
                                   screen)
  look wir:[<selector>] [--until <selector>] [--wait 200ms] [--out <png>] [--chars 4000]
       [--full | --element <selector>]
                                   (read-only snapshot of an open Safari tab on a USB-cabled iPhone; never
                                   navigates)
  look app:<id> | exe:<path> [--app <id>] [--build] [--until <selector>] [--wait 2s] [--keys 40] [--out <png>]
       [-- <args…>]
                                   (after --: the app's own arguments, after its profile's; the window waited
                                   for 30 s, --timeout for longer)
       [--windowless]              (an app that shows no window, as a profile's window: false: whether it kept
                                   running over --wait, how it ended, and the last lines it printed)
  look tty:<command> [--size 120x40] [--until text=<text>] [--press <key>] [--type <text>] [--wait 1s]
       [--home <seed>]
                                  a page in a headless browser, nothing measured: title, address, headings,
                                  visible text, console messages and uncaught errors, and a screenshot (exit
                                  124 when it never loads); tty: the program on strobe's own terminal in an
                                  isolated home: its grid, the modes it set
  top [--target <pid>] [--once] [--interval 1s] [--limit N] [--sort cpu|gpu|vram|mem]
                                  live per-core load by CCD, processes with CPU, GPU, VRAM
  run [--pin auto|none|ccdN|<cpus>] [--reserve-cpus on|off] [--power high|keep] [--max-time 5m] -- <command>
      [args…]
                                  run your own harness as a bench block runs: the bench slot, the fastest
                                  power plan, pinned to the target's CPUs with other processes kept off them
                                  (--pin as for bench); it prints its own numbers (--max-time caps the wait
                                  for the bench slot, not the command)
  run --base \"<command>\" --head \"<command>\" [--aa] [--check] [--primary cpu.work] [--metrics metric.<name>,…]
      [--pairs auto|N] [--runs 5] [--noworse 2%] [--pin …] [--reserve-cpus auto|on|off] [--power high|keep]
      [--require-verdict]
                                  a paired A/B of two commands, as a bench (verdicts, report, PR block): each
                                  run starts the command in the folder strobe runs in (a bench's cli() target:
                                  the variant's root) and waits for its exit; cpu.work, run.duration,
                                  cli.launch, cpu.wakes, memory.peak, io.*, and metric.<name> from
                                  `strobe:metric <name>=<value>` lines it prints (--aa --head: an A/A)
                                  (--check: each command once, --head alone or --base too: cpu.work,
                                  run.duration and memory.peak, nothing compared; a base that fails fails the
                                  check; --noworse needs timed pairs)
                                  (Windows, a known issue: a line is started directly, its first word the
                                  program, so after a real program &&, ||, |, < and > go to it as arguments
                                  and only its first command runs and is measured: wrap the line in cmd /c \"…\"
                                  on both sides, or use a script)

Setup
  init [<dir>] [--target web|lib|server|native|electron|tty|cli] [--force]
                                  a project's first bench from its package.json, Cargo.toml or index.html:
                                  strobe.json (its schema linked) and a runnable perf/smoke.bench.ts; never
                                  replaces a file without --force; prints the commands that check them
  up [--skip deep,daemon,mcp,calibrate]  install, download the engine, deep mode (one UAC prompt), start the
                                  daemon, register MCP and the skill (--check: only what it finds, cpu to bun;
                                  nothing changes)
                                  (--skip self,skill: install nothing into this user's profile (no binary,
                                  PATH or skill; with daemon): a CI job's own copy, which only fetches the
                                  engine)
  up --mobile[=<serial>] --channel canary|dev|beta
                                  give strobe that Chrome channel on the phone, once: --mobile runs may
                                  force-stop it and clear the origins they load (strobe never drives a phone's
                                  own Chrome)
  update [<version>] [--from <path>] [--check] [--force]
                                  install the latest npm release (or that version, or a local build) over the
                                  installed copy; refreshes the skill, MCP, the helper where installed, and
                                  restarts the daemon; keeps a newer or local build unless --force
  leave [--yes]                   undo up (--yes also deletes recordings and experiments)
  prune [builds|seeds|gpu-cache|symbols|sdk|engine|run|recordings|experiments …] [--all] [--older-than 7d]
        [--dry-run] [-f]
                                  remove what strobe keeps and no process uses: with no category every cache
                                  (builds, seeds, GPU caches, symbols, other builds' SDKs and engines,
                                  finished processes' files); recordings and experiments only when named or
                                  with --all, older than 30d (--older-than) and never one an experiment that
                                  stays names; prints the plan first, asks on a terminal, removes nothing
                                  without one unless -f (--force: exit 3 otherwise); --dry-run: the plan only
                                  (caches unused for 14d also go on their own, once a day after a bench and in
                                  the daemon, never beside a bench: \"prune\": { \"unused\": \"30d\" | \"off\" } in
                                  <state>/strobe.json)

Targets   https://… · a local page.html (or file:page.html) · app:<id> (a strobe.json profile, Electron or
          native) · exe:<path> [--app <id> | --native]
          · cdp:<port> · cdp:ws://… · file:<trace> (import) · pid:<n> · proc:<name> · tty:<command> (look)
          · wir:[<selector>] (an open Safari tab on a USB-cabled iPhone: look, record)
Variants  a directory · a git ref (git:<ref>) · app:<id> · exe:<path> · url:<href> · cmd:<line> (a cli()
          target's command)
Global    --json  -q  -h  -V  --progress stderr|stdout|off (stdout for shells that count stderr lines as
          a failure)
          (-q: no progress lines and no text report; a command that makes a recording or experiment prints its
          handle; report --format md|pr|summary still prints its markdown)
          (--filter, --target, --params, --metrics, --noworse, --skip given twice add up; any other flag
          given twice with another value is a usage error; so is a flag the command does not take: strobe
          <command> --help lists its flags)
Handles   rec_…, exp_…, ser_… and --id are plain ids: letters, digits, _ and - (an id at most 96); a
          unique prefix of a handle works; query and report also take a path (above)
Clean-up  strobe ends what it launched; a process it cannot prove it launched it leaves alone, and the
          command ends with a line per such process (its pid and why; --json: left_alone; the state
          folder's log/left-alone.log; strobe status says when there were any in the last day)
Schema    result.json and --json: schema/result.v1.json (node_modules/@hona/strobe/schema/); strobe.json:
          schema/strobe.v1.json
Units     times print in microseconds, digits grouped (1,234 µs); --unit ms|s (or \"unit\": \"ms\" in strobe.json)
          prints them in milliseconds or seconds; stored and --json numbers keep their metric's unit (ms)
          KB, MB and GB are binary: 2^10, 2^20 and 2^30 bytes
Exit codes: 0 ok · 1 error · 2 usage · 3 needs a human · 4 refused (invalid) · 5 inconclusive · 124 timeout ·
            130 stopped · 255 unreachable
";
