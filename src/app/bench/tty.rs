//! Terminal targets (SPEC §9.6 `tty()`, §13.9): a program on a pseudo-terminal strobe owns, in a fresh
//! isolated home per block, driven by the bytes a terminal sends (`driver::tty`) and measured from the
//! cell grid strobe's terminal keeps (`target::tty`) and from outside its processes: CPU in cycles,
//! memory, wakes and IO. Nothing needs a display: it runs on a locked session and a headless server.

use super::*;
use crate::driver::tty as drv;
use crate::target::events::{Event, Events, Kind as EventKind};
use crate::target::tty::{self as term, Tty};

/// A terminal's default size: what a person's terminal window often holds.
const COLS: u16 = 120;
const ROWS: u16 = 40;

/// The target's terminal size: `cols` × `rows`.
fn size_of(target: &Value) -> Result<(u16, u16)> {
    let n = |k: &str, d: u16| match &target[k] {
        Value::Null => Ok(d),
        v => v.as_u64().filter(|n| (1..=1000).contains(n)).map(|n| n as u16).ok_or_else(|| Error::usage(format!("tty({k}): a whole number of cells from 1 to 1000, not {v}"))),
    };
    Ok((n("cols", COLS)?, n("rows", ROWS)?))
}

/// The program as the target gives it (`run`: a command line, or the program and its arguments), with
/// `{home}` and `{port}` filled in.
fn run_of(target: &Value, home: &isolate::Home) -> Result<term::Run> {
    match &target["run"] {
        Value::String(s) if !s.trim().is_empty() => Ok(term::Run::Line(home.fill(s))),
        Value::Array(a) if !a.is_empty() && a.iter().all(Value::is_string) => Ok(term::Run::Argv(a.iter().map(|x| home.fill(x.as_str().unwrap_or(""))).collect())),
        _ => Err(Error::usage("a tty target's run is a command line or [program, ...args]: tty(\"htop\"), tty([\"nvim\", \"--clean\"])")),
    }
}

/// The program's name, for the runner's `ctx.exe` and the process set.
fn program(run: &term::Run) -> String {
    match run {
        term::Run::Argv(a) => a.first().cloned().unwrap_or_default(),
        term::Run::Line(l) => l.split_whitespace().next().unwrap_or("").trim_matches('"').to_string(),
    }
}

/// The executable the target's command starts, when it names one strobe can find: a path (from the
/// folder it runs in), else a name on PATH (`.exe` added on Windows). None for a shell line's builtin.
fn exe_of(target: &Value, cwd: &Path) -> Option<PathBuf> {
    let first = match &target["run"] {
        Value::Array(a) => a.first()?.as_str()?.to_string(),
        Value::String(l) => l.split_whitespace().next()?.trim_matches('"').to_string(),
        _ => return None,
    };
    if first.contains('{') {
        return None;
    }
    let p = Path::new(&first);
    if p.components().count() > 1 || p.is_absolute() {
        let p = cwd.join(p);
        let with_exe = p.with_extension("exe");
        return [p, with_exe].into_iter().find(|x| x.is_file());
    }
    host::find_exe(first.trim_end_matches(".exe"), &[])
}

/// The program's whole environment, for the hooks (`ctx.env`): strobe's, the home's, then the launch's own.
fn environment(home: &isolate::Home, spec: &term::Spec) -> BTreeMap<String, String> {
    let mut env = home.environment(&isolate::App::bare(Path::new("tty")));
    env.retain(|k, _| !isolate::env_unset(&spec.unset, k));
    for (k, val) in &spec.env {
        env.retain(|e, _| !isolate::env_name_is(e, k));
        env.insert(k.clone(), val.clone());
    }
    env
}

/// A block's terminal, ready to start.
struct Prepared {
    home: isolate::Home,
    spec: term::Spec,
    events: Events,
    /// The user's own state as it was before the launch (Documents, Desktop, shared seeds): the
    /// program must not change it (rule 10).
    guard: isolate::Guard,
}

/// A block's terminal: its home (fresh, seeded from the target's `home`, with its `files`) and how to
/// start the program in it, in the variant's folder (the target's `cwd` under it). A timed block of a
/// debug build is refused (`DevBuild`).
fn prepare(ctx: &Ctx, s: &Series, v: &Variant, target: &Value, home: Option<PathBuf>, timed: bool) -> Result<Prepared> {
    let root = v.root.clone().ok_or_else(|| Error::msg(format!("{}: a tty target runs in a variant's folder (a directory or a git ref)", v.role)))?;
    let cwd = target["cwd"].as_str().map_or(root.clone(), |c| root.join(c));
    let exe = exe_of(target, &cwd);
    if let (Some(e), true) = (&exe, timed) {
        dev_build(e)?;
    }
    let mut app = isolate::App::bare(exe.as_deref().unwrap_or(Path::new("tty")));
    if let Some(files) = target["files"].as_object() {
        app = app.with_files(files);
    }
    if exe.is_some() {
        isolate::library_check(&app)?;
    }
    let home = isolate::prepare_home(&app, seed_dir(ctx, s, target).as_deref(), home)?;
    let mut guard = isolate::Guard::take(&app);
    guard.shared = home.shared.clone();
    let (cols, rows) = size_of(target)?;
    let events = Events::open(&home.dir, super::thread_pin())?;
    let mut env = home.env.clone();
    env.extend(term::private_tmp(&home.dir)?);
    env.push(events.env());
    for (k, val) in target["env"].as_object().into_iter().flatten() {
        env.push((k.clone(), home.fill(val.as_str().unwrap_or(""))));
    }
    let unset: Vec<String> = target["unset"].as_array().into_iter().flatten().filter_map(|u| u.as_str().map(str::to_string)).collect();
    let run = run_of(target, &home)?;
    let dir = Some(home.dir.clone());
    Ok(Prepared { spec: term::Spec { run, cwd, env, unset, cols, rows, home: dir }, home, events, guard })
}

/// A run's app events in its timed parts: what they add to its `count.<name>`s, and its `app.<name>`s.
fn event_values(events: &[Event], timed: &metrics::Timed) -> (BTreeMap<String, f64>, BTreeMap<String, f64>) {
    let (mut counts, mut app) = (BTreeMap::new(), BTreeMap::new());
    for e in events.iter().filter(|e| e.at_ns >= timed.t0 && e.at_ns <= timed.t1 && !timed.untimed.iter().any(|g| e.at_ns >= g.0 && e.at_ns < g.1)) {
        match e.kind {
            EventKind::Count(n) => *counts.entry(e.name.clone()).or_insert(0.0) += n,
            EventKind::Metric(x) => *app.entry(e.name.clone()).or_insert(0.0) += x,
            EventKind::State | EventKind::Mark => {}
        }
    }
    (counts, app)
}

/// `ctx.waitForLine(match, { in: "events" })`: the first line of the event stream matching it that
/// arrived after the watch began, on strobe's host clock.
#[derive(Default)]
struct EventWatches(Vec<(regex_lite::Regex, u64, Instant, Duration)>);

impl EventWatches {
    fn serve(&mut self, events: &Events, a: &Value) -> Result<Value> {
        if a["step"] == "watch" {
            let timeout = Duration::from_millis(a["timeout"].as_u64().unwrap_or(30_000));
            self.0.push((launch::pattern(a, "waitForLine")?, host::now_ns(), Instant::now() + timeout, timeout));
            return Ok(json!(self.0.len() - 1));
        }
        let (re, after, deadline, timeout) = self.0.get(a["id"].as_u64().unwrap_or(u64::MAX) as usize).ok_or_else(|| Error::msg("waitForLine: no such watch"))?;
        if let Some(e) = events.between(*after, u64::MAX).into_iter().find(|e| re.is_match(&e.line)) {
            return Ok(json!({ "line": e.line, "at": e.at_ns }));
        }
        if Instant::now() >= *deadline {
            return Err(Error::timeout(format!("waitForLine: no event line matching {} in {:.1} s", re.as_str(), timeout.as_secs_f64())));
        }
        Ok(json!({ "pending": true }))
    }
}

/// The grid as a fingerprint every run must start from (the first run's): each row cell by cell (a
/// cell in an `ignore` region, `[col, row, columns, rows]`, is `·` whatever it holds, so a wide or
/// combining character there shifts nothing), the cells shown inverse (a selection, a highlighted
/// row), and the cursor.
fn fingerprint(tty: &Tty, ignore: &[[u16; 4]]) -> Vec<String> {
    tty.grid(|g| {
        let ignored = |r: u16, c: u16| ignore.iter().any(|i| c >= i[0] && c < i[0] + i[2] && r >= i[1] && r < i[1] + i[3]);
        let mut out: Vec<String> = (0..g.rows)
            .map(|r| {
                let mut text = String::new();
                let mut inverse = Vec::new();
                for c in 0..g.cols {
                    if ignored(r, c) {
                        text.push('·');
                        continue;
                    }
                    let Some(cell) = g.cell(r, c) else { continue };
                    if cell.inverse() {
                        inverse.push(c);
                    }
                    if !cell.is_wide_continuation() {
                        text.push_str(if cell.has_contents() { cell.contents() } else { " " });
                    }
                }
                let mut row = text.trim_end().to_string();
                if !inverse.is_empty() {
                    row += &format!(" [inverse at {}]", spans(&inverse));
                }
                row
            })
            .collect();
        if !ignored(g.cursor.0, g.cursor.1) {
            out.push(format!("cursor at row {}, col {}{}", g.cursor.0, g.cursor.1, if g.cursor_on { "" } else { " (hidden)" }));
        }
        out
    })
}

/// `3-7,12`: sorted columns as runs.
fn spans(cols: &[u16]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < cols.len() {
        let start = cols[i];
        while i + 1 < cols.len() && cols[i + 1] == cols[i] + 1 {
            i += 1;
        }
        out.push(if cols[i] == start { start.to_string() } else { format!("{start}-{}", cols[i]) });
        i += 1;
    }
    out.join(",")
}

fn ignore_of(target: &Value) -> Result<Vec<[u16; 4]>> {
    target["ignore"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| {
            let a: Option<Vec<u16>> = r.as_array().filter(|a| a.len() == 4).map(|a| a.iter().filter_map(|n| n.as_u64().map(|n| n as u16)).collect());
            a.filter(|a| a.len() == 4).map(|a| [a[0], a[1], a[2], a[3]]).ok_or_else(|| Error::usage(format!("tty({{ ignore }}): each region is [col, row, columns, rows] in cells, not {r}")))
        })
        .collect()
}

/// The rows where two grids differ, for a `ResetIncomplete`.
fn grid_diff(a: &[String], b: &[String]) -> String {
    let rows: Vec<String> = (0..a.len().max(b.len()))
        .filter(|&i| a.get(i) != b.get(i))
        .take(4)
        .map(|i| {
            let (then, now) = (a.get(i).map_or("", |s| s.trim()), b.get(i).map_or("", |s| s.trim()));
            if then.starts_with("cursor") || now.starts_with("cursor") { format!("{then:?} then, {now:?} now") } else { format!("row {i}: {then:?} then, {now:?} now") }
        })
        .collect();
    rows.join("; ")
}

/// The program's processes' CPU by process.
fn cycles(pid: u32, home: &Path) -> super::super::record::Cycles {
    super::super::record::per_process(&host::process_set(pid, Some(home)))
}

/// What tells a process of the program's set that ended (`host::ended_marks`), of the processes a
/// `cycles` reading just listed (None: listed now).
fn ended(pid: u32, cycles: Option<&super::super::record::Cycles>, home: &Path) -> host::EndedMarks {
    let pids = cycles.map_or_else(|| host::process_set_pids(pid, Some(home)), |c| c.keys().map(|k| k.0).collect());
    host::ended_marks(&pids, Some(home))
}

/// The pseudo-console host's cycles (Windows' conhost: the terminal's work, never the program's).
fn host_cycles(tty: &Tty) -> u64 {
    tty.host_pids.iter().filter_map(|p| host::process_cycles(*p)).sum()
}

/// What a terminal block measures beyond CPU and memory, said when a declared metric has no value.
const MEASURES: &str = "run.duration, cpu.work, cpu.app.total, cpu.pty_host, cpu.wakes, memory.app.private, io.*, tty.latency.*, tty.settle.*, tty.updates, tty.frames, tty.frame.*, tty.bytes, tty.cells, count.*, phase.*, app.*";

/// One measured run.
struct Run {
    ms: f64,
    cpu: f64,
    pty_ms: f64,
    /// `app.<name>`: what the program's metric events added up to in the run.
    app: BTreeMap<String, f64>,
    counts: BTreeMap<String, f64>,
    marks: Vec<(String, u64)>,
    timed: metrics::Timed,
    counted: probe::wakes::Counted,
    /// Context switches and IO of the program's processes over the run's timed parts; None when its
    /// threads could not be read at an end.
    span: Option<probe::wakes::Span>,
    /// To when the last input's settle is judged: the run's end, or after an untimed wait for an answer
    /// already in to hold still.
    settled: u64,
    updates: usize,
    frames: usize,
    bytes: u64,
    cells: u64,
    /// The program's private memory as the run left it (MB), where it may be the last measured run.
    end_mb: Option<f64>,
    /// A process of the program's set ended in a timed part and what it used there is not known
    /// (`record::timed_cpu`): its CPU there went with it.
    cpu_lost: bool,
    /// What processes that ended in the run's timed parts used there (ms), in `cpu`.
    ended_ms: f64,
    /// strobe's terminal reader's CPU over the run's timed parts and the cycle clock over them (both in
    /// `host::tsc` units).
    reader: (u64, u64),
    /// Other work on the program's CPUs over the run (% of one core; SPEC §10.2 `Background`).
    foreign: Option<f64>,
}

/// One launch of a terminal program for its cases (one, or a chunk with `--check --batch`).
pub(super) fn chunk(ctx: &mut Ctx, cases: &[Series], v: &Variant, pair: usize, x: &Exec) -> Result<Launch> {
    let s = &cases[0];
    if x.set == Set::Memory {
        return Err(Error::usage("a tty target has no memory blocks yet: declare memory.app.private on a run case"));
    }
    let batch = ctx.batch.is_some();
    let target = ctx.describe["targets"][v.target_of(s)].clone();
    let ignore = ignore_of(&target)?;
    let mut steps: Vec<(String, f64)> = Vec::new();
    let launched = Instant::now();
    // `Throttled`: no page to run the speed check in, so strobe's own workload on the target's CPUs.
    let ratio = validity::native_speed_checked(ctx.pin)?;
    let Prepared { home, spec, events, guard } = prepare(ctx, s, v, &target, None, x.measures_time())?;
    let stop = isolate::end_on_stop(Some(&home));
    let env = environment(&home, &spec);
    let t = s.hook_timeout();
    let block_args = json!({ "case": s.case, "params": s.params, "target": v.target_of(s), "variant": v.role, "url": "", "root": v.root.as_deref().map(host::paths::plain), "home": host::paths::plain(&home.dir), "env": env, "exe": program(&spec.run) });
    if let Err(e) = ctx.runner.call("block", block_args, t, launch::no_page) {
        drop(stop);
        isolate::end_launch(None, Some(&home.dir), true);
        return Err(e);
    }
    let tty = match Tty::spawn(&spec, ctx.pin, super::thread_pin()) {
        Ok(t) => t,
        Err(e) => {
            drop(stop);
            isolate::end_launch(None, Some(&home.dir), true);
            return Err(e);
        }
    };
    steps.push(("launch".into(), launched.elapsed().as_secs_f64() * 1e3));
    // `BackgroundPriority`: a program at background priority keeps no latency values (`priority_state`).
    ctx.target_qos = host::background_qos(Some(tty.pid));
    let pid = tty.pid;
    // The program's process set: its tree, and what left it stamped with its home (a daemon).
    let set_home = home.dir.clone();
    let set_pids = |home: &Path| host::process_set_pids(pid, Some(home));
    let driver = drv::Driver::new(&tty, Some(&events));
    let hooks_home = home.dir.clone();
    let hooks = super::block::Hooks::new(ctx, move || host::process_set_pids(pid, Some(&hooks_home)));
    let cost = SelfCost::new(ctx.runner.pid());
    let lines = RefCell::new(launch::Lines::default());
    let watches = RefCell::new(EventWatches::default());
    // The program's CPU at each edge of the run's untimed stretches, the console host's, and strobe's
    // terminal reader's with the cycle clock.
    let edges: RefCell<Vec<(super::super::record::Cycles, host::EndedMarks)>> = RefCell::new(Vec::new());
    let host_edges: RefCell<Vec<u64>> = RefCell::new(Vec::new());
    let reader_edges: RefCell<Vec<(u64, Option<u64>)>> = RefCell::new(Vec::new());
    // The set's threads and IO at the same edges: switches and IO over the timed parts.
    let snap_edges: RefCell<Vec<probe::wakes::Snapshot>> = RefCell::new(Vec::new());
    let pixels = x.pixels.clone();
    let home_dir = home.dir.clone();
    let serve = |op: &str, a: &Value| -> Result<Value> {
        match op {
            "driver" => {
                let call = a["call"].as_str().unwrap_or("");
                let args = a["args"].as_array().cloned().unwrap_or_default();
                hooks.op(call, &args);
                if let Some(code) = tty.exited().filter(|_| tty.ended()) {
                    return Err(super::super::record::exited(code, &format!("before page.{call}()"), Some(&last_rows(&tty))));
                }
                driver.dispatch(call, &args, pixels.as_deref())
            }
            "count" => {
                hooks.count(a);
                Ok(Value::Bool(true))
            }
            "phase" => hooks.phase(a),
            "untimed" => hooks.untimed(a, |start, _| {
                if start {
                    cost.pause();
                }
                let c = cycles(pid, &set_home);
                let e = ended(pid, Some(&c), &set_home);
                edges.borrow_mut().push((c, e));
                host_edges.borrow_mut().push(host_cycles(&tty));
                reader_edges.borrow_mut().push((tty.reader_cycles(), host::tsc()));
                snap_edges.borrow_mut().push(probe::wakes::Snapshot::take_since(probe::wakes::wall_us(), &set_pids(&set_home), None));
                if !start {
                    cost.resume();
                }
                Ok(())
            }),
            "line" if a["in"] == "events" => watches.borrow_mut().serve(&events, a),
            "line" if a["in"] == "output" && a["step"] == "watch" => {
                Err(Error::usage("waitForLine on a tty target: its output is its screen (page.waitFor(\"text=…\")); in: \"events\" reads its STROBE_EVENTS lines, a glob its log files"))
            }
            "line" => lines.borrow_mut().serve(None, &home_dir, a),
            "call" => Err(Error::usage("ctx.call is a script() target's; a tty target is driven with page (type, press, click, …)")),
            "exec" => Err(Error::usage("ctx.exec is a cli() target's; a tty target is driven with page (type, press, click, …)")),
            other => Err(Error::msg(format!("unknown runner request {other}"))),
        }
    };
    let result = (|| -> Result<Launch> {
        let lapped = Instant::now();
        let setup = ctx.runner.call("setup", json!({}), t, serve);
        super::batch::launch_hook_case(ctx)?;
        setup.map_err(|e| crate::runner::incomplete("setup", "setup", e, &screen_note(&tty), "fix the bench file's setup"))?;
        hooks.check_fixture();
        steps.push(("setup".into(), lapped.elapsed().as_secs_f64() * 1e3));
        let perf = x.counters.then(probe::wakes::Perf::new);
        let mut launch = Launch::default();
        let mut base = 0usize;
        // What the launch's first run started from: every later run, a batch's later cases' too.
        let mut first_screen: Option<Vec<String>> = None;
        for (k, s) in cases.iter().enumerate() {
            // A launch hook that read what differs per case set up the first case alone (`unbatched`).
            if k > 0 && (stop_requested() || ctx.unbatched.is_some()) {
                break;
            }
            launch.log_at.push(ctx.runner.log_len());
            let lapped = Instant::now();
            let r = (|| -> Result<Block> {
                let mut runs: Vec<Run> = Vec::new();
                let mut warmup = super::block::Warmup::new(s, x);
                // Who else used the CPUs, from two snapshots outside the runs, for a block with other work.
                let procs0 = ctx.pin.map(|_| (host::now_ns(), host::all_processes()));
                let rt = s.run_timeout() + Duration::from_secs(10);
                while warmup.more(runs.len()) {
                    // Every run starts where the first did (`resetCheck`): the grid, less `ignore`.
                    let screen = fingerprint(&tty, &ignore);
                    match &first_screen {
                        Some(f) if s.reset_check && *f != screen => {
                            return Err(Refusal::new(ValidatorId::ResetIncomplete, format!("{} run {}: the terminal does not show what run 0 started from", s.case, runs.len()))
                                .detail(grid_diff(f, &screen))
                                .fix("make the case's reset put the program back (press Escape, clear the prompt), leave changing cells out with tty({ ignore: [[col, row, cols, rows]] }), or resetCheck: false for runs that build on each other")
                                .into());
                        }
                        None => first_screen = Some(screen),
                        _ => {}
                    }
                    hooks.begin(&format!("run:{}", base + runs.len()));
                    let k0 = probe::wakes::Snapshot::take_since(probe::wakes::wall_us(), &set_pids(&set_home), perf.as_ref());
                    edges.borrow_mut().clear();
                    host_edges.borrow_mut().clear();
                    reader_edges.borrow_mut().clear();
                    snap_edges.borrow_mut().clear();
                    // Right before the cycles (nothing read between), so it adds nothing to the run's CPU; a
                    // process that differs between the two is refused, never miscounted (`dead_since`).
                    let ended0 = ended(pid, None, &set_home);
                    let mut c0 = cycles(pid, &set_home);
                    let h0 = host_cycles(&tty);
                    let r0 = tty.reader_cycles();
                    let busy = ctx.pin.and_then(|_| validity::CpuBusy::start());
                    // The listed processes read again right at the run's start, and at its end before the set is
                    // listed for new ones, each by itself and with the clocks: a listing takes milliseconds, which
                    // would be on one side of the run (`record::read_known`, `record::Clock`).
                    let at0 = super::super::record::read_known(c0.keys());
                    super::super::record::read_at(&mut c0, &at0);
                    let clock0 = super::super::record::Clock::now();
                    cost.start();
                    let t0 = host::now_ns();
                    let ran = ctx.runner.call("run", json!({ "case": s.case }), rt, serve);
                    // The run's end and its readings: (t1, the CPUs' busy time, reader, console host, cycles, the
                    // clocks, ended children, threads and IO). The busy time is stopped before the process
                    // listing, which takes milliseconds: the program's CPU below lies inside it.
                    let read_end = || {
                        let t1 = host::now_ns();
                        let busy = busy.as_ref().zip(ctx.pin).and_then(|(b, cpus)| b.stop(cpus));
                        let (r1, h1) = (tty.reader_cycles(), host_cycles(&tty));
                        let at1 = super::super::record::read_known(c0.keys().chain(edges.borrow().iter().flat_map(|e| e.0.keys())));
                        let clock1 = super::super::record::Clock::now();
                        let mut c1 = cycles(pid, &set_home);
                        super::super::record::read_at(&mut c1, &at1);
                        // Right after the cycles, of the processes they listed (`dead_since`).
                        let ended1 = ended(pid, Some(&c1), &set_home);
                        (t1, busy, r1, h1, c1, clock1, ended1, probe::wakes::Snapshot::take_since(probe::wakes::wall_us(), &set_pids(&set_home), perf.as_ref()))
                    };
                    // A run may return right after its last input (no until), before the program draws its
                    // answer: the run then ends once the answer came and held still, so its time, CPU,
                    // output, IO and counts all cover it. One that ended in an untimed stretch keeps its end.
                    // Asked before the end is read: an answer arriving while it is read is still awaited.
                    let last = (ran.is_ok() && !hooks.untimed_now() && !hooks.ended_untimed()).then(|| driver.last_input(t0, &hooks.untimed_stretches())).flatten();
                    let mut end = read_end();
                    if let Some((sent, false)) = last
                        && driver.await_answer(sent)
                    {
                        end = read_end();
                    }
                    cost.end();
                    // An answer already in (until: "frame"): an untimed wait for it to hold still, so its
                    // settle can be judged (latencies read to here, inputs are still the run's).
                    let settled = match last {
                        Some((sent, true)) if driver.await_quiet(sent) => host::now_ns(),
                        _ => end.0,
                    };
                    let (t1, busy, r1, h1, c1, clock1, ended1, k1) = end;
                    // Other work on the program's CPUs (`cpu.foreign`): their busy time less the program's (with
                    // what processes of it that ended used, `record::timed_cpu`) and the console host's cycles;
                    // unknown where what ended is not known.
                    let foreign = busy.zip(super::super::record::timed_cpu(&[&c0, &c1], &[&ended0, &ended1])).and_then(|(b, c)| b.other_cycles(c.total + h1.saturating_sub(h0)));
                    hooks.end();
                    ran.map_err(|e| crate::runner::incomplete(&format!("{} run", s.case), "run", e, &screen_note(&tty), "the run threw or timed out"))?;
                    let timed = metrics::Timed { t0, t1, untimed: hooks.untimed_stretches() };
                    // Switches and IO over the timed parts: the snapshots at the run's ends and its untimed
                    // edges in pairs (a stretch the run never ended ends with it), each part its own span.
                    let mut se = snap_edges.take();
                    if se.len() % 2 == 1 {
                        se.push(k1.clone());
                    }
                    let mut snaps: Vec<&probe::wakes::Snapshot> = vec![&k0];
                    snaps.extend(se.iter());
                    snaps.push(&k1);
                    // None when the set's threads could not be read at an end (the program gone).
                    let os_span = (!k0.is_empty() && !k1.is_empty()).then(|| probe::wakes::Span::joined(snaps.as_chunks::<2>().0.iter().map(|[a, b]| probe::wakes::span(a, b)).collect()));
                    let counted = os_span.as_ref().map(|s| s.counted.clone()).unwrap_or_else(|| probe::wakes::span(&k0, &k1).counted);
                    let mut e = edges.take();
                    if e.len() % 2 == 1 {
                        e.push((c1.clone(), ended1.clone()));
                    }
                    let mut at: Vec<&super::super::record::Cycles> = vec![&c0];
                    at.extend(e.iter().map(|x| &x.0));
                    at.push(&c1);
                    let mut ended_at: Vec<&host::EndedMarks> = vec![&ended0];
                    ended_at.extend(e.iter().map(|x| &x.1));
                    ended_at.push(&ended1);
                    let timed_cpu = super::super::record::timed_cpu(&at, &ended_at);
                    let cpu_lost = timed_cpu.is_none();
                    let rate = clock0.per_ms(&clock1);
                    // Short CPU is none (warmup falls back to the run's time).
                    let ms = |cycles: u64| match rate {
                        Some(r) if c1.values().any(|c| *c > 0) => cycles as f64 / r,
                        _ => f64::NAN,
                    };
                    let cpu = timed_cpu.as_ref().map_or(f64::NAN, |c| ms(c.total));
                    let ended_ms = timed_cpu.as_ref().map_or(0.0, |c| ms(c.ended));
                    // The console host's cycles over the timed parts (its readings at the same edges).
                    let mut he = host_edges.take();
                    if he.len() % 2 == 1 {
                        he.push(h1);
                    }
                    let mut hs = vec![h0];
                    hs.extend(he);
                    hs.push(h1);
                    let host_timed: u64 = hs.chunks(2).map(|p| p.get(1).copied().unwrap_or(p[0]).saturating_sub(p[0])).sum();
                    let pty_ms = rate.map_or(f64::NAN, |r| host_timed as f64 / r);
                    let in_timed = |ns: u64| ns >= t0 && ns <= t1 && !timed.untimed.iter().any(|g| ns >= g.0 && ns < g.1);
                    let ups: Vec<term::Update> = tty.updates(t0, t1).into_iter().filter(|u| in_timed(u.commit_ns)).collect();
                    let bytes: u64 = tty.bytes(t0, t1).saturating_sub(timed.untimed.iter().map(|g| tty.bytes(g.0, g.1.saturating_sub(1))).sum());
                    let end_mb = warmup.may_end(runs.len()).then(|| host::process_set(pid, Some(&set_home)).iter().map(|p| p.private_bytes).sum::<u64>() as f64 / (1024.0 * 1024.0));
                    // strobe's terminal reader over the run's timed parts (its readings at the same edges),
                    // with the cycle clock's span over them.
                    let mut re = reader_edges.take();
                    if re.len() % 2 == 1 {
                        re.push((r1, clock1.tsc));
                    }
                    let mut rs = vec![(r0, clock0.tsc)];
                    rs.extend(re);
                    rs.push((r1, clock1.tsc));
                    let reader = rs.chunks(2).filter_map(|p| Some((p.get(1)?.0.saturating_sub(p[0].0), p.get(1)?.1?.checked_sub(p[0].1?)?))).fold((0u64, 0u64), |a, p| (a.0 + p.0, a.1 + p.1));
                    // The program's own counts and metrics from its event stream, by when they arrived.
                    let (ev_counts, app) = event_values(&events.between(t0, t1), &timed);
                    let mut counts = hooks.counts();
                    for (k, n) in ev_counts {
                        *counts.entry(k).or_insert(0.0) += n;
                    }
                    runs.push(Run {
                        ms: timed.ns() as f64 / 1e6,
                        cpu,
                        pty_ms,
                        app,
                        counts,
                        marks: hooks.take_marks(),
                        counted,
                        span: os_span,
                        settled,
                        updates: ups.len(),
                        frames: ups.iter().filter(|u| u.synced).count(),
                        cells: ups.iter().map(|u| u.cells as u64).sum(),
                        bytes,
                        timed,
                        end_mb,
                        cpu_lost,
                        ended_ms,
                        reader,
                        foreign,
                    });
                    driver.release_all()?;
                    if s.reset {
                        ctx.runner
                            .call("reset", json!({ "case": s.case }), rt, serve)
                            .map_err(|e| crate::runner::incomplete(&format!("{} reset", s.case), "reset", e, &screen_note(&tty), "fix the case's reset"))?;
                    }
                    warmup.after(&runs.iter().map(|r| if r.cpu.is_finite() { r.cpu } else { r.ms }).collect::<Vec<_>>(), &[], &runs.iter().map(|r| r.cpu_lost).collect::<Vec<_>>());
                }
                if batch && tty.ended() {
                    return Err(super::super::record::exited(tty.exited().unwrap_or(-1), &format!("after {}'s runs", s.case), Some(&last_rows(&tty))));
                }
                let first = warmup.first().unwrap_or(0);
                let local = first..warmup.measured().end.clamp(first, runs.len());
                let (strobe_pct, runner_pct) = cost.share(base + local.start..base + local.end);
                // Over the measured runs' timed parts only: what the reader did in warmup or an untimed
                // stretch stamped nothing the block keeps.
                let (reader, span) = runs[local.clone()].iter().fold((0u64, 0u64), |a, r| (a.0 + r.reader.0, a.1 + r.reader.1));
                let terminal_pct = (span > 0).then(|| 100.0 * reader as f64 / span as f64);
                let mut steps = steps.clone();
                steps.push(("runs".into(), lapped.elapsed().as_secs_f64() * 1e3));
                let measured = &runs[local.clone()];
                let key = |j: usize| format!("run:{}", base + first + j);
                let per = |f: &dyn Fn(&Run) -> f64| -> BTreeMap<String, f64> { measured.iter().enumerate().map(|(j, r)| (key(j), f(r))).filter(|(_, v)| v.is_finite()).collect() };
                let mut extra: BTreeMap<String, BTreeMap<String, f64>> = BTreeMap::new();
                extra.insert("run.duration".into(), per(&|r| r.ms));
                // A process of the program's set that ended in a timed part took its CPU with it, where what it
                // used there is not known: none.
                let gone = measured.iter().filter(|r| r.cpu_lost).count();
                super::note_ended(&mut ctx.warnings, s, &measured.iter().map(|r| (r.ended_ms, r.cpu_lost)).collect::<Vec<_>>(), &[]);
                if gone == 0 && measured.iter().all(|r| r.cpu.is_finite()) {
                    extra.insert("cpu.work".into(), per(&|r| r.cpu));
                    extra.insert("cpu.app.total".into(), per(&|r| 100.0 * r.cpu / r.ms.max(1e-9)));
                }
                if !tty.host_pids.is_empty() && measured.iter().all(|r| r.pty_ms.is_finite()) {
                    extra.insert("cpu.pty_host".into(), per(&|r| r.pty_ms));
                }
                extra.insert("tty.updates".into(), per(&|r| r.updates as f64));
                extra.insert("tty.frames".into(), per(&|r| r.frames as f64));
                extra.insert("tty.bytes".into(), per(&|r| r.bytes as f64));
                extra.insert("tty.cells".into(), per(&|r| r.cells as f64));
                let read = measured.iter().all(|r| r.span.is_some());
                if read {
                    let span = |r: &Run, f: fn(&probe::wakes::Span) -> f64| r.span.as_ref().map_or(f64::NAN, f);
                    if measured.iter().all(|r| r.span.as_ref().is_some_and(|s| s.uncounted.is_none())) {
                        extra.insert("cpu.wakes".into(), per(&|r| span(r, probe::wakes::Span::wakes_per_s)));
                    }
                    extra.insert("io.read".into(), per(&|r| span(r, |s| s.io_read as f64)));
                    extra.insert("io.write".into(), per(&|r| span(r, |s| s.io_write as f64)));
                }
                let hooked: Vec<super::block::Hooked> = measured.iter().map(|r| (&r.counts, r.marks.as_slice(), &r.timed)).collect();
                extra.extend(super::block::per_run_hooks(s, &key, &hooked));
                // `app.<name>`: every measured run must have had the event (a run without one is not 0).
                for m in declared_metrics(s).into_iter().filter(|m| m.starts_with("app.")) {
                    let name = &m["app.".len()..];
                    if measured.iter().all(|r| r.app.contains_key(name)) {
                        extra.insert(m.clone(), measured.iter().enumerate().map(|(j, r)| (key(j), r.app[name])).collect());
                    }
                }
                // Events whose app clock disagrees with their arrival describe a late stream.
                Events::sync(&measured.iter().flat_map(|r| events.between(r.timed.t0, r.timed.t1)).collect::<Vec<_>>())?;
                if events.lost() > 0 {
                    return Err(Error::refused(
                        "RunIncomplete",
                        format!("{}: the program wrote more event lines than strobe keeps ({} dropped), so its counts and metrics are short", s.case, events.lost()),
                        "write fewer event lines (counts with n, not one line each)",
                    ));
                }
                let counted: Vec<(String, &probe::wakes::Counted)> = measured.iter().enumerate().map(|(j, r)| (key(j), &r.counted)).collect();
                let (counts, uncounted) = probe::wakes::counter_metrics(&counted, declared_metrics(s));
                extra.extend(counts);
                let mut values = metrics::block_values(&metrics::BlockData::default(), "run", &extra);
                let mut refused = uncounted;
                // Wakes where the OS stopped counting a thread's switches: unknown, never short.
                if let Some(why) = measured.iter().find_map(|r| r.span.as_ref().and_then(|s| s.uncounted.clone())) {
                    for m in declared_metrics(s).into_iter().filter(|m| *m == "cpu.wakes") {
                        refused.entry(m.clone()).or_insert_with(|| Refusal::new(ValidatorId::NoCounter, format!("cpu.wakes: {why}")));
                    }
                }
                if gone > 0 {
                    for m in declared_metrics(s).into_iter().filter(|m| matches!(m.as_str(), "cpu.work" | "cpu.app.total" | "cpu.process.total")) {
                        refused.insert(m.clone(), super::super::record::cpu_lost(m, gone, measured.len(), "the program"));
                    }
                }
                // Input latencies, pooled over the measured runs' timed parts.
                // Each run to when its last input's settle can be judged (`settled`): inputs are still the run's own.
                let settled: Vec<metrics::Timed> = measured.iter().map(|r| metrics::Timed { t1: r.settled, ..r.timed.clone() }).collect();
                let lat = drv::latencies(&tty, &driver.inputs(), &settled.iter().collect::<Vec<_>>());
                let stretches: Vec<&metrics::Timed> = measured.iter().map(|r| &r.timed).collect();
                let mut ms = lat.ms.clone();
                ms.sort_by(f64::total_cmp);
                if !ms.is_empty() {
                    values.insert("tty.latency.n".into(), ms.len() as f64);
                    values.insert("tty.latency.p50".into(), metrics::percentile(&ms, 0.5));
                    values.insert("tty.latency.p95".into(), metrics::percentile(&ms, 0.95));
                }
                let mut settle = lat.settle_ms.clone();
                settle.sort_by(f64::total_cmp);
                // A resize after which only the host drew would leave its slowest settles out: none then.
                if !settle.is_empty() && lat.host_only == 0 {
                    values.insert("tty.settle.n".into(), settle.len() as f64);
                    values.insert("tty.settle.p50".into(), metrics::percentile(&settle, 0.5));
                    values.insert("tty.settle.p95".into(), metrics::percentile(&settle, 0.95));
                }
                if lat.busy + lat.silent > 0 {
                    values.insert("tty.latency.busy".into(), lat.busy as f64);
                    values.insert("tty.latency.silent".into(), lat.silent as f64);
                }
                // Synchronized frames: how long each took from its begin to its end, pooled.
                let mut frame_ms: Vec<f64> = stretches
                    .iter()
                    .flat_map(|r| {
                        tty.updates(r.t0, r.t1).into_iter().filter(|u| u.synced && !r.untimed.iter().any(|g| u.commit_ns >= g.0 && u.commit_ns < g.1)).map(|u| (u.commit_ns - u.start_ns) as f64 / 1e6)
                    })
                    .collect();
                frame_ms.sort_by(f64::total_cmp);
                if !frame_ms.is_empty() {
                    values.insert("tty.frame.n".into(), frame_ms.len() as f64);
                    values.insert("tty.frame.p50".into(), metrics::percentile(&frame_ms, 0.5));
                    values.insert("tty.frame.p95".into(), metrics::percentile(&frame_ms, 0.95));
                }
                if let Some(mb) = local.end.checked_sub(1).and_then(|i| runs.get(i)).and_then(|r| r.end_mb).filter(|m| *m > 0.0) {
                    values.insert("memory.app.private".into(), mb);
                }
                if warmup.unsteady() {
                    values.insert("warmup.unsteady".into(), if warmup.grew() { 2.0 } else { 1.0 });
                }
                // Other work on the program's CPUs during the runs: kept and flagged (`Background`, SPEC §10.2).
                let foreign: Vec<f64> = measured.iter().filter_map(|r| r.foreign).collect();
                let mut background = Vec::new();
                if !foreign.is_empty() {
                    let most = foreign.iter().copied().fold(0.0, f64::max);
                    values.insert("cpu.foreign".into(), metrics::median(&foreign));
                    values.insert("cpu.foreign.max".into(), most);
                    if most >= BACKGROUND_PCT
                        && let Some(p0) = &procs0
                    {
                        background = busiest_others(p0, &(host::now_ns(), host::all_processes()));
                    }
                }
                let syncs = tty.sync_timeouts();
                for m in declared_metrics(s) {
                    if values.contains_key(m) || refused.contains_key(m) || m.starts_with("count.") || m.starts_with("phase.") {
                        continue;
                    }
                    let r = match m.as_str() {
                        _ if m.starts_with("tty.latency.") && lat.host_resizes > 0 && lat.busy + lat.silent == 0 => Refusal::new(
                            ValidatorId::NotOnThisOs,
                            format!("{m}: the runs' only inputs were {} resizes, and ConPTY's console host redraws its own buffer at a resize before the program hears of it, so the first change is the host's", lat.host_resizes),
                        )
                        .fix(drv::RESIZE_FIX),
                        _ if m.starts_with("tty.latency.") => Refusal::new(
                            ValidatorId::NoInput,
                            format!("{m}: no input of the measured runs changed the grid while it was at rest ({} written while the program still drew, {} after which nothing changed)", lat.busy, lat.silent),
                        )
                        .fix("send keys or clicks in the run (page.type, page.press) on a program that has settled; a program that never rests has no input latency"),
                        _ if m.starts_with("tty.settle.") && lat.host_only > 0 => Refusal::new(
                            ValidatorId::NotOnThisOs,
                            format!("{m}: after {} of the runs' resizes only ConPTY's console host drew (one change), so the program's own redraw, if any, never showed apart from the host's", lat.host_only),
                        )
                        .fix(drv::RESIZE_FIX),
                        _ if m.starts_with("tty.settle.") => Refusal::new(
                            ValidatorId::NoInput,
                            format!("{m}: no input of the measured runs was followed by grid changes that then held still for {} ms before the next input or the run's end", drv::QUIET.as_millis()),
                        )
                        .fix("wait for the program to settle after the input (until: \"idle\"); a program that never rests never settles"),
                        _ if m.starts_with("tty.frame.") => Refusal::new(ValidatorId::NotForTarget, format!("{m}: the program drew no synchronized frames (?2026){}", if syncs > 0 { format!(", and {syncs} it began never ended within a second") } else { String::new() })),
                        "cpu.pty_host" => Refusal::new(ValidatorId::NotOnThisOs, "cpu.pty_host: on this OS the kernel is the terminal: no host process does its work"),
                        "cpu.wakes" | "io.read" | "io.write" if !read => Refusal::new(ValidatorId::RunIncomplete, format!("{m}: the program's threads could not be read at a measured run's start or end (its processes gone)")),
                        _ if m.starts_with("app.") => {
                            let bad = events.bad();
                            let r = Refusal::new(ValidatorId::NoMetric, format!("{m}: in {} of {} measured runs the program wrote no {{\"strobe\":\"metric\",\"name\":\"{}\"}} line to $STROBE_EVENTS", measured.iter().filter(|r| !r.app.contains_key(&m["app.".len()..])).count(), measured.len(), &m["app.".len()..]))
                                .fix("append the line, with a numeric value, from every run");
                            if bad.is_empty() { r } else { r.detail(format!("lines that were no event: {}", bad.iter().take(3).cloned().collect::<Vec<_>>().join("; "))) }
                        }
                        _ => Refusal::new(ValidatorId::NotForTarget, format!("{m}: a tty target measures {MEASURES}")),
                    };
                    refused.insert(m.clone(), r);
                }
                // The terminal's reader past its budget makes what it stamps late (rule 2): a timed block's
                // stamps are refused; a `--check` keeps them and says what a timed bench refuses.
                if let Some(p) = terminal_pct.filter(|p| *p > 2.0) {
                    let stamped = |k: &str| ["tty.latency.", "tty.settle.", "tty.frame."].iter().any(|p| k.starts_with(p));
                    let used = format!("strobe's terminal reader used {p:.2}% of a core (budget 0.5%)");
                    if x.measures_time() {
                        for m in values.keys().filter(|k| stamped(k)).cloned().collect::<Vec<_>>() {
                            values.remove(&m);
                            refused.insert(m.clone(), Refusal::new(ValidatorId::OverheadBudget, format!("{m}: {used}, so it stamped the program's output late")));
                        }
                    } else if x.judges_time() {
                        super::check_overhead(&mut ctx.warnings, s, &used, &stamped, None);
                    }
                }
                refuse_not_on_this_os(s, &mut values, &mut refused);
                super::block::untimed_state(s, &measured.iter().map(|r| &r.timed).collect::<Vec<_>>(), &mut values, &mut refused);
                base += runs.len();
                Ok(Block {
                    variant: v.role.to_string(),
                    pair,
                    values,
                    run_ms: measured.iter().map(|r| r.ms).collect(),
                    warmup: first,
                    steps,
                    refused,
                    overhead: Some(Overhead { strobe_pct, runner_pct, terminal_pct, ..Overhead::default() }),
                    process_set: Some(programs(&host::process_set(pid, Some(&set_home)))),
                    speed_ratio: ratio,
                    background,
                    ..Block::default()
                })
            })();
            let end = r.is_err() || !s.reset_check;
            launch.case_ms.push(lapped.elapsed().as_secs_f64() * 1e3);
            launch.cases.push(r);
            // The user's own state, after each case (rule 10): disturbed, the case fails and the bench stops.
            if let Err(e) = guard.check(&format!("after {}'s runs", s.case)) {
                launch.disturbed(e);
                break;
            }
            if end {
                break;
            }
        }
        if launch.cases.iter().any(Result::is_ok) {
            let _ = ctx.runner.call("close", json!({}), t, serve);
        }
        Ok(launch)
    })();
    drop(stop);
    drop(driver);
    tty.close();
    drop(events);
    isolate::end_launch(None, Some(&home.dir), true);
    let mut launch = result?;
    if let Err(e) = guard.check("after the terminal program closed") {
        launch.disturbed(e);
    }
    Ok(launch)
}

/// The grid's last non-empty rows, for a refusal when the program exits.
fn last_rows(tty: &Tty) -> Vec<String> {
    let rows: Vec<String> = tty.grid(|g| g.text()).into_iter().filter(|r| !r.trim().is_empty()).collect();
    rows[rows.len().saturating_sub(5)..].to_vec()
}

/// What the terminal showed when a hook failed, for its error.
fn screen_note(tty: &Tty) -> String {
    let rows = last_rows(tty);
    if rows.is_empty() { "\n  the terminal showed nothing".into() } else { format!("\n  the terminal's last rows:\n    {}", rows.join("\n    ")) }
}

/// A launch case: from the spawn until the grid first shows `until` (`launch.ready`: the update that
/// drew it), and each of its `milestones` that shows text (`in: "screen"`) or writes a log line.
/// One discarded warm-up launch per variant and series comes first, as for apps.
pub(super) fn launch(ctx: &mut Ctx, s: &Series, v: &Variant, pair: usize, x: &Exec) -> Result<Block> {
    let key = format!("{}\n{}", s.label(), v.role);
    if !x.untimed && ctx.launched.insert(key) {
        progress(format!("  {} warm-up launch (discarded)", v.role));
        launch_once(ctx, s, v, pair, false)?;
    }
    launch_once(ctx, s, v, pair, !x.untimed)
}

fn launch_once(ctx: &mut Ctx, s: &Series, v: &Variant, pair: usize, timed: bool) -> Result<Block> {
    let spec = s.launch.clone();
    let target = ctx.describe["targets"][v.target_of(s)].clone();
    let until = match &spec["until"] {
        u if u.get("window").is_some() => return Err(Error::usage(format!("{}: a terminal has no window of its own: time the launch to text on its grid (until: \"text=…\")", s.case))),
        u if u.get("milestone").is_some() || u.get("state").is_some() => None,
        u => Some(drv::parse_sel(u).map_err(|e| Error::usage(format!("{}: launch.until: {}", s.case, e.message)))?),
    };
    // `until: { state }`: the program's own state on its event stream ends the launch.
    let state = spec["until"]["state"].as_str().map(str::to_string);
    // A terminal program's output is its screen: `output` (the default) and `screen` read the grid.
    let mut marks = launch::milestones(&spec)?;
    for m in marks.iter_mut().filter(|m| m.source == "output") {
        m.source = "screen".into();
    }
    if let Some(m) = marks.iter().find(|m| m.source == "page") {
        return Err(Error::usage(format!("{}: launch milestone {}: a terminal program has no page: in: \"screen\" (its grid's text) or a glob of log files under its home", s.case, m.name)));
    }
    if state.is_some() && marks.iter().any(|m| m.source == "screen") {
        return Err(Error::usage(format!(
            "{}: a launch until a state on the event stream reads no screen milestones (their first showing is timed while strobe waits on the grid): time it until text, or give log milestones",
            s.case
        )));
    }
    let wanted = spec["until"]["milestone"].as_str().map(str::to_string);
    if let Some(w) = &wanted
        && !marks.iter().any(|m| &m.name == w)
    {
        return Err(Error::usage(format!("{}: launch.until.milestone {w} is not one of its milestones", s.case)));
    }
    let warm = match spec["profile"].as_str().unwrap_or("fresh") {
        "fresh" => false,
        "warm" => true,
        other => return Err(Error::usage(format!("{}: launch.profile is fresh or warm, not {other}", s.case))),
    };
    let label = format!("{}\n{}", s.label(), v.role);
    let home_dir = if warm { Some(ctx.homes.entry(label).or_insert_with(isolate::new_home).clone()) } else { None };
    let ratio = validity::native_speed_checked(ctx.pin)?;
    let Prepared { home, spec: tspec, events, guard } = prepare(ctx, s, v, &target, home_dir, timed)?;
    let stop = isolate::end_on_stop(Some(&home));
    let env = environment(&home, &tspec);
    let t = Duration::from_secs(120);
    let mut steps: Vec<(String, f64)> = Vec::new();
    let result = (|| -> Result<Block> {
        ctx.runner.call(
            "block",
            json!({ "case": s.case, "params": s.params, "target": v.target_of(s), "variant": v.role, "url": "", "root": v.root.as_deref().map(host::paths::plain), "home": host::paths::plain(&home.dir), "env": env, "exe": program(&tspec.run) }),
            t,
            launch::no_page,
        )?;
        if spec["before"] == true {
            ctx.runner
                .call("before", json!({ "case": s.case }), Duration::from_secs(900), launch::no_page)
                .map_err(|e| crate::runner::incomplete(&format!("{} before", s.case), "before", e, "", "fix the launch's before hook"))?;
        }
        let lapped = Instant::now();
        let clock = host::tsc();
        let tty = Tty::spawn(&tspec, ctx.pin, super::thread_pin())?;
        ctx.target_qos = host::background_qos(Some(tty.pid));
        let timeout = Duration::from_millis(spec["timeout"].as_u64().unwrap_or(120_000));
        let deadline = Instant::now() + timeout;
        // Each screen milestone's first showing (the update that drew it), and `until`'s.
        let screen: Vec<(String, regex_lite::Regex)> = marks.iter().filter(|m| m.source == "screen").map(|m| (m.name.clone(), m.pattern.clone())).collect();
        let mut seen: BTreeMap<String, u64> = BTreeMap::new();
        let mut ready: Option<u64> = None;
        let done = if let Some(name) = &state {
            ready = events.wait_state(name, tty.started_ns, deadline);
            if let Some(at) = ready {
                Events::sync(&events.between(tty.started_ns, at))?;
            }
            ready.is_some()
        } else {
            tty.wait(deadline, |g, last, _| {
                let rows = g.text();
                for (name, p) in &screen {
                    if !seen.contains_key(name) && rows.iter().any(|r| p.is_match(r)) {
                        seen.insert(name.clone(), last);
                    }
                }
                if ready.is_none() {
                    ready = match (&until, &wanted) {
                        (Some(sel), _) => (!drv::find(g, sel).is_empty()).then_some(last),
                        (None, Some(w)) => seen.get(w).copied(),
                        _ => None,
                    };
                }
                ready.is_some()
            })
        };
        let procs_at_ready = host::process_set(tty.pid, Some(&home.dir));
        // A process of the launch that ended before it was ready took its CPU with it (every count starts
        // at 0 at the spawn), unless its launch's cgroup kept it (`host::launch_ended_cpu`, read with the
        // set's cycles).
        let ended_at_ready = host::launch_ended_cpu(&procs_at_ready.iter().map(|p| p.pid).collect::<Vec<_>>(), Some(&home.dir));
        let cpu_at_ready = super::super::record::per_process(&procs_at_ready).values().sum::<u64>() + ended_at_ready.unwrap_or(0);
        let launch_lost = ended_at_ready.is_none() && (tty.ended() || ended(tty.pid, None, &home.dir).ended_since(&host::EndedMarks::default()));
        let tsc_at_ready = host::tsc();
        let wall_at_ready = lapped.elapsed();
        if !done {
            let why = if tty.ended() { format!("the program exited (code {}) first", tty.exited().unwrap_or(-1)) } else { format!("it never showed in {} s", timeout.as_secs()) };
            let what = match (&until, &state) {
                (Some(u), _) => format!("{:?}", u.source),
                (None, Some(st)) => format!("state {st:?} on $STROBE_EVENTS"),
                _ => format!("milestone {}", wanted.clone().unwrap_or_default()),
            };
            let e = Error::refused("RunIncomplete", format!("{} launch: {what}: {why}", s.case), "check the until text against what the program shows (strobe look tty:<command>)");
            let note = screen_note(&tty);
            tty.close();
            return Err(Error { message: format!("{}{note}", e.message), ..e });
        }
        let ready_ns = ready.unwrap_or(tty.started_ns);
        steps.push(("ready".into(), lapped.elapsed().as_secs_f64() * 1e3));
        let mut values = BTreeMap::new();
        values.insert("launch.ready".to_string(), (ready_ns - tty.started_ns) as f64 / 1e6);
        for (name, at) in &seen {
            values.insert(format!("launch.{name}"), (*at - tty.started_ns) as f64 / 1e6);
        }
        // The program's CPU from the spawn until ready, from its cycles over the cycle clock's rate.
        if let (Some(a), Some(b)) = (clock, tsc_at_ready)
            && b > a
            && cpu_at_ready > 0
            && !launch_lost
        {
            let per_ms = (b - a) as f64 / (wall_at_ready.as_secs_f64() * 1e3);
            values.insert("cpu.work".into(), cpu_at_ready as f64 / per_ms);
            if let (Some(c), true) = (ended_at_ready, timed) {
                super::note_ended_launch(&mut ctx.warnings, s, c as f64 / per_ms);
            }
        }
        let ups = tty.updates(tty.started_ns, ready_ns);
        values.insert("tty.updates".into(), ups.len() as f64);
        values.insert("tty.frames".into(), ups.iter().filter(|u| u.synced).count() as f64);
        values.insert("tty.bytes".into(), tty.bytes(tty.started_ns, ready_ns) as f64);
        if super::launch::declares(s, "memory.app.") {
            super::launch::memory(&host::process_set(tty.pid, Some(&home.dir)), &mut values);
        }
        // Log milestones: the first matching line in files under the home by now, at when it was read.
        for m in marks.iter().filter(|m| m.source != "screen") {
            let now = host::now_ns();
            let hit =
                super::launch::matching(&home.dir, &m.source).iter().filter_map(|p| std::fs::read_to_string(p).ok()).any(|text| text.lines().any(|l| m.pattern.is_match(&isolate::strip_ansi(l))));
            if hit {
                values.insert(format!("launch.{}", m.name), (now - tty.started_ns) as f64 / 1e6);
            }
        }
        for m in &marks {
            let k = format!("launch.{}", m.name);
            if !values.contains_key(&k) && declared_metrics(s).contains(&&k) {
                tty.close();
                return Err(Error::refused("RunIncomplete", format!("{} launch: milestone {} never matched in {}", s.case, m.name, m.source), "check the milestone's match and where it looks"));
            }
        }
        let mut refused = super::launch::refusals(s, &values, "a terminal has no window of its own", false);
        if launch_lost && refused.contains_key("cpu.work") {
            refused.insert("cpu.work".into(), super::super::record::cpu_lost("cpu.work", 1, 1, "the program (a launcher that handed over, a helper)"));
        }
        if let Some(r) = refused.get(&s.primary) {
            tty.close();
            return Err(r.clone().fix("pick a primary a launch case measures").into());
        }
        let procs = host::process_set(tty.pid, Some(&home.dir));
        if let Some(f) = &ctx.fixture_running {
            f.check(&procs.iter().map(|p| p.pid).collect::<Vec<_>>());
        }
        tty.close();
        Ok(Block { variant: v.role.to_string(), pair, run_ms: vec![values["launch.ready"]], values, steps, refused, process_set: Some(programs(&procs)), speed_ratio: ratio, ..Block::default() })
    })();
    drop(stop);
    drop(events);
    isolate::end_launch(None, Some(&home.dir), !warm);
    // The user's own state after the launch (rule 10).
    let result = result.and_then(|b| guard.check("after the terminal program's launch").map(|_| b));
    let closed = ctx.runner.call("close", json!({}), t, launch::no_page);
    if let Err(e) = closed
        && !ctx.warnings.iter().any(|w| w.id == "Teardown")
    {
        ctx.warnings.push(validity::warn("Teardown", format!("{} launch: {}", side_of(ctx, v), e.message)));
    }
    result
}
