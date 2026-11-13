//! Script targets (SPEC §9.6): a variant's Bun or Node module, loaded by strobe's host in a fresh
//! process per block and called from the bench file (`ctx.call(name, ...args)`). Runs are timed on the
//! process's own CPU (`cpu.work`), its main thread's (`cpu.main`) and wall clock; attribution blocks
//! add the runtime's CPU profile (`--cpu-prof`) and Node's allocation profile (`--heap-prof`), the CPU
//! profile placed on strobe's clock and cut to each measured run, and the host's thread timeline
//! (`Profiled`).

use super::*;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::mpsc::Receiver;

/// The host process and its line protocol, over a loopback connection.
struct Host {
    /// The host with its process group: dropped, the host and what it started end.
    child: host::spawn::Owned,
    conn: TcpStream,
    lines: Receiver<String>,
    next: u64,
    /// The runtime's own process, as it says in its ready message: the child strobe started, or the
    /// runtime a launcher it started (a version manager's shim) runs.
    runtime_pid: u32,
    /// Bun runs the module, as the host says in its ready message (a `node` on PATH may be Bun's shim),
    /// whatever the target declared.
    bun: bool,
}

/// A profiled host's clocks against strobe's (`probe::cpu::Sync`), and its runtime (`name`, `version`).
type Clocks = (probe::cpu::Sync, Value);

/// The run folder a profiled host writes its profiles to: removed when dropped, whatever happened, so
/// a failed block leaves no partial profile behind.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A measured run as `runs.json` keeps it: its key, case, span with untimed stretches and phase marks
/// (strobe's clock), its wall and CPU ms, and its main thread's CPU ms (`cpu.main`) or why none.
struct Measured {
    key: String,
    case: String,
    timed: metrics::Timed,
    marks: Vec<(String, u64)>,
    ms: f64,
    cpu_ms: f64,
    main_ms: std::result::Result<f64, host::MainLost>,
    /// What processes of the host's tree that ended in its timed parts used there (ms), whether that is not
    /// known, and by executable (ms, processes): the recording's `ended_runs`.
    ended: (f64, bool, Vec<(String, f64, u32)>),
}

/// The harness module for `root`, and whether strobe placed it there. The entry resolves from the bench
/// file, like everything else in it, so both variants run the same harness: one in the harness (the
/// bench file's folder, strobe.json's `harness` paths) is already the bench's copy (`overlay_harness`).
/// In a worktree strobe made for a git ref, an entry the ref lacks is placed at the same path, so its
/// relative imports resolve to that variant's code; one the ref holds is the variant's own and is never
/// replaced (an entry outside the harness, `script("../src/x.ts")`, is code under test: its own copy on
/// each side). A directory variant is the user's own checkout: strobe never writes there, and the file
/// must already exist.
fn place(bench_file: &Path, entry: &str, root: &Path) -> Result<(PathBuf, bool)> {
    let dir = bench_file.parent().unwrap_or(Path::new("."));
    let source = std::fs::canonicalize(dir.join(entry)).map_err(|_| Error::usage(format!("script entry {entry}: no such file next to {}", bench_file.display())))?;
    // Relative to the repository, not the folder with strobe.json: a variant's folder is a whole checkout (a git
    // ref's worktree, or a directory variant), so a harness under games/x/perf is at games/x/perf there too.
    let repo = host::git::toplevel(dir);
    let project = std::fs::canonicalize(repo.unwrap_or_else(|| config::project_root(dir)))?;
    let rel = source.strip_prefix(&project).map_err(|_| Error::usage(format!("script entry {} is outside the project {}", source.display(), project.display())))?;
    let placed = root.join(rel);
    let owned = std::fs::canonicalize(root).ok().zip(std::fs::canonicalize(host::paths::sub("builds")).ok()).is_some_and(|(r, b)| r.starts_with(b));
    let mut put = false;
    if owned && !placed.exists() {
        if let Some(p) = placed.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(&placed, std::fs::read(&source)?)?;
        put = true;
    }
    if !placed.is_file() {
        return Err(Error::usage(format!("the harness {} is missing in {}: commit it there, or compare against a git ref", rel.display(), root.display())));
    }
    Ok((placed, put))
}

impl Host {
    fn spawn(target: &Value, entry: &Path, root: &Path, cpus: Option<u64>, profiles: Option<&Path>, log: &Path) -> Result<Host> {
        let runtime = target["runtime"].as_str().unwrap_or("bun");
        let exe = match runtime {
            "bun" => crate::runner::find_bun().ok_or_else(|| Error::msg("script targets on Bun need Bun").hint("install it from https://bun.sh"))?,
            "node" => host::find_exe("node", &[]).ok_or_else(|| Error::msg("script targets on Node need Node on PATH"))?,
            other => return Err(Error::usage(format!("script runtime: bun or node, not {other}"))),
        };
        let mut cmd = Command::new(exe);
        // `js.heap.retained` collects garbage between runs: Bun has `Bun.gc`, Node needs the flag.
        if runtime == "node" {
            cmd.arg("--expose-gc");
        }
        if let Some(dir) = profiles {
            // The whole process's profiles, written on a normal exit; samples at the interval pages get.
            cmd.args(["--cpu-prof", &format!("--cpu-prof-interval={}", probe::cpu::interval_us())]).arg(format!("--cpu-prof-dir={}", dir.display()));
            // Sampled allocations from Node only: Bun's `--heap-prof` writes a whole heap snapshot at
            // its exit (every live string in it), which no allocation view can read.
            if runtime == "node" {
                cmd.arg("--heap-prof").arg(format!("--heap-prof-dir={}", dir.display()));
            }
        }
        cmd.arg(crate::runner::sdk_dir()?.join("host.mjs")).arg(entry).current_dir(root);
        for (k, v) in target["env"].as_object().into_iter().flatten() {
            cmd.env(k, v.as_str().unwrap_or(""));
        }
        // Requests go over a loopback socket, never the host's stdio: the module and the children it
        // spawns own that (Bun's `$` hands stdin on, and stalled on a protocol pipe that never closes).
        // stdin is empty; stdout and stderr go to the experiment's script.log.
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        cmd.env("STROBE_HOST_PORT", listener.local_addr()?.port().to_string());
        // Owner-only: the module prints what it likes there (SPEC rule 9).
        let mut o = std::fs::OpenOptions::new();
        o.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        let out = o.open(log)?;
        let err = out.try_clone()?;
        cmd.stdin(Stdio::null()).stdout(out).stderr(err);
        let mut child = host::spawn_owned(&mut cmd, cpus, 0, None).map_err(|e| Error::msg(format!("start the script host: {e}")))?;
        let failed = |log: &Path| Error::msg(format!("the script host did not load {}:\n{}", entry.display(), log_tail(log, 12).join("\n")));
        // The host connects once its runtime is up; a module that fails to import ends it first.
        listener.set_nonblocking(true)?;
        let deadline = Instant::now() + Duration::from_secs(60);
        let conn = loop {
            match listener.accept() {
                Ok((conn, _)) => break conn,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(Error::msg(format!("script host connection: {e}"))),
            }
            if Instant::now() > deadline || child.exited().is_some() {
                return Err(failed(log));
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        conn.set_nonblocking(false)?;
        conn.set_nodelay(true)?;
        let reader = conn.try_clone()?;
        let (tx, lines) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(reader).lines().map_while(std::result::Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let pid = child.pid();
        let mut host = Host { child, conn, lines, next: 1, runtime_pid: pid, bun: runtime == "bun" };
        loop {
            let line = host.lines.recv_timeout(Duration::from_secs(60)).map_err(|_| failed(log))?;
            if let Some(v) = serde_json::from_str::<Value>(&line).ok().filter(|v| v["ready"] == true) {
                host.runtime_pid = v["pid"].as_u64().and_then(|p| u32::try_from(p).ok()).filter(|p| *p > 0).unwrap_or(pid);
                if let Some(r) = v["runtime"].as_str() {
                    host.bun = r == "bun";
                }
                return Ok(host);
            }
        }
    }

    fn request(&mut self, message: Value, timeout: Duration) -> Result<Value> {
        let id = self.next;
        self.next += 1;
        let mut m = message;
        m["id"] = json!(id);
        writeln!(self.conn, "{m}").map_err(|e| Error::msg(format!("script host: {e}")))?;
        self.conn.flush().ok();
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = self.lines.recv_timeout(left).map_err(|e| match e {
                std::sync::mpsc::RecvTimeoutError::Timeout => Error::timeout(format!("the process host did not answer in {} s", timeout.as_secs())),
                std::sync::mpsc::RecvTimeoutError::Disconnected => match self.child.exited() {
                    Some(code) => Error::msg(format!("the process host exited (code {code})")),
                    None => Error::msg("the process host closed its connection"),
                },
            })?;
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            if v["id"] == json!(id) {
                if let Some(e) = v["error"].as_str() {
                    return Err(Error::msg(e.to_string()));
                }
                return Ok(v["ok"].clone());
            }
        }
    }

    fn pid(&self) -> u32 {
        self.child.pid()
    }

    /// The host's clocks against strobe's (`probe::cpu::Sync`): one request in which it spins at least
    /// 5 ms and until it catches a wall-clock millisecond tick within 50 µs (20 ms at most), then five
    /// quick round trips.
    fn clocks(&mut self) -> Result<Clocks> {
        let t = Duration::from_secs(10);
        let a = host::now_ns();
        let marked = self.request(json!({ "op": "clock", "mark": true }), t)?;
        let b = host::now_ns();
        let mut trips = Vec::new();
        for _ in 0..5 {
            let a = host::now_ns();
            let v = self.request(json!({ "op": "clock" }), t)?;
            trips.push((a, host::now_ns(), v["mono_us"].as_f64().unwrap_or(f64::NAN)));
        }
        Ok((probe::cpu::Sync::of((a, b, &marked), &trips)?, json!({ "name": marked["runtime"], "version": marked["version"] })))
    }

    /// A normal exit (profiles are written), waited for up to `wait` (`cut`: what shortened it), then
    /// its group, whatever is left of it (the drop). A profiled host's clocks are read first (`clocks`).
    fn exit(mut self, profiled: bool, wait: Duration, cut: Option<&'static str>) -> (Option<Result<Clocks>>, ExitState) {
        let clocks = profiled.then(|| self.clocks());
        let _ = self.request(json!({ "op": "exit" }), Duration::from_secs(5));
        // The connection closed too: a host whose event loop had not read the request yet exits on that
        // (`host.mjs`), a normal exit as well. Its write side only: the host's late reply is then no reset.
        let _ = self.conn.shutdown(std::net::Shutdown::Write);
        let began = Instant::now();
        let state = loop {
            match self.child.ended() {
                Some(host::spawn::Ended::Code(c)) => break ExitState::Exited(c),
                #[cfg(unix)]
                Some(host::spawn::Ended::Signal(s)) => break ExitState::Signaled(s),
                // Never: a script's host is strobe's own child.
                Some(host::spawn::Ended::Unreported) => break ExitState::Exited(-1),
                None => {}
            }
            if stop_requested() {
                break ExitState::TimedOut(began.elapsed(), Some("a stop"));
            }
            if began.elapsed() >= wait {
                break ExitState::TimedOut(began.elapsed(), cut);
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        (clocks, state)
    }
}

/// How the host ended after strobe asked it to exit (`Host::exit`).
#[derive(Clone, Debug)]
enum ExitState {
    /// It exited by itself, with this code.
    Exited(i32),
    /// Unix: a signal ended it (its number).
    #[cfg(unix)]
    Signaled(i32),
    /// It still ran this long after the request, and strobe ended it; with what cut the wait short.
    TimedOut(Duration, Option<&'static str>),
}

impl std::fmt::Display for ExitState {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            ExitState::Exited(c) => write!(f, "it exited (code {c}) without writing one"),
            #[cfg(unix)]
            ExitState::Signaled(s) => write!(f, "signal {s} ended it before it wrote one"),
            ExitState::TimedOut(d, cut) => write!(
                f,
                "it still ran {:.0} s after strobe asked it to exit, and strobe ended it (a runtime writes its profile as it exits, which takes long for a long block on a busy machine){}",
                d.as_secs_f64(),
                cut.map(|c| format!("; {c} cut the wait short")).unwrap_or_default()
            ),
        }
    }
}

/// The last `n` lines of the experiment's script.log (of its last 8 KB), each with its secrets redacted
/// (cookies, authorization, bearer tokens, URL secrets: `redact_text`) and at most 200 characters. What
/// an HTTP probe sent is scrubbed from the file first (`Runner::scrub_log`), where the caller has a runner.
fn log_tail(log: &Path, n: usize) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut buf = Vec::new();
    if let Ok(mut f) = std::fs::File::open(log) {
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        if f.seek(SeekFrom::Start(len.saturating_sub(8192))).is_ok() {
            let _ = f.read_to_end(&mut buf);
        }
    }
    let lines: Vec<String> =
        String::from_utf8_lossy(&buf).lines().map(str::trim).filter(|l| !l.is_empty()).map(|l| crate::host::ios::logs::redact_text(l, false).chars().take(200).collect()).collect();
    lines[lines.len().saturating_sub(n)..].to_vec()
}

/// CPU ms between two readings over `wall_ms`, from cycles (exact). Never from process times, which
/// move in 15.6 ms ticks on Windows (a 2 ms call read 0 ms or 15.6 ms): unknown (NaN) without cycles,
/// and where they are not time (`host::cycles_refused`) the block refuses CPU.
pub(super) fn cpu_ms(a: (u64, Option<u64>), b: (u64, Option<u64>), wall_ms: f64) -> f64 {
    match (a.1, b.1) {
        (Some(t0), Some(t1)) if t1 > t0 && b.0 > 0 => b.0.saturating_sub(a.0) as f64 / (t1 - t0) as f64 * wall_ms,
        _ => f64::NAN,
    }
}

/// The host process tree's cycles by process (`record::per_process`).
fn cycles(pid: u32) -> super::super::record::Cycles {
    super::super::record::per_process(&host::process_tree(pid))
}

/// One run: wall ms, CPU ms (both without its untimed stretches), its counters, the heap it retained
/// (MB), its phase marks, what the OS counted for the host's processes, its span, and the other work on
/// the target's CPUs meanwhile (% of one core; SPEC §10.2 `Background`).
type ScriptRun = (f64, f64, BTreeMap<String, f64>, f64, Vec<(String, u64)>, probe::wakes::Counted, metrics::Timed, Option<f64>);

/// One host process for the cases of one launch: one case, or a chunk of them with `--check --batch`
/// (SPEC §5.6), each case's runs after the case before it.
pub(super) fn chunk(ctx: &mut Ctx, cases: &[Series], v: &Variant, pair: usize, x: &Exec) -> Result<Launch> {
    let s = &cases[0];
    let batch = ctx.batch.is_some();
    let set = x.set;
    let target = ctx.describe["targets"][&s.target].clone();
    let root = v.root.clone().ok_or_else(|| Error::msg(format!("{}: a script target runs in a variant's folder (a directory or a git ref)", v.role)))?;
    let id = store::new_id("rec");
    // Attribution blocks only: a clean timing block starts no profiler.
    let profiles = (set == Set::Attribution).then(|| Scratch(host::paths::run().join(format!("{id}-prof"))));
    if let Some(p) = &profiles {
        std::fs::create_dir_all(&p.0)?;
    }
    let mut steps: Vec<(String, f64)> = Vec::new();
    let launched = Instant::now();
    let (entry, put) = place(Path::new(ctx.describe["file"].as_str().unwrap_or(".")), target["entry"].as_str().unwrap_or(""), &root)?;
    if put {
        let file = entry.strip_prefix(&root).unwrap_or(&entry).to_string_lossy().replace('\\', "/");
        let text = format!("{} ({}) had no {file}, which {} runs: strobe placed the bench's own copy there, so both sides run that one file", v.role, v.label, s.target);
        super::say_overlay(ctx, vec![validity::info("HarnessOverlay", text)]);
    }
    let log = store::experiment_dir(&ctx.exp)?.join("script.log");
    // What a profile is cut to, on strobe's clock: the host's life outside the measured runs
    // (`lifecycle_spans`), each passing case's measured runs, and its measured range of the launch's runs.
    let life: RefCell<Vec<(String, u64, u64)>> = RefCell::new(Vec::new());
    let kept: RefCell<Vec<Measured>> = RefCell::new(Vec::new());
    // Every run of those cases, warmup ones too: the recording's `run_list`.
    let listed: RefCell<Vec<RunRow>> = RefCell::new(Vec::new());
    let passed: RefCell<Vec<(usize, std::ops::Range<usize>)>> = RefCell::new(Vec::new());
    let started = host::now_ns();
    let mut spawned = Host::spawn(&target, &entry, &root, ctx.pin, profiles.as_ref().map(|p| p.0.as_path()), &log)?;
    life.borrow_mut().push(("startup".into(), started, host::now_ns()));
    let ready = if profiles.is_some() { Some(spawned.clocks()?) } else { None };
    let pid = spawned.pid();
    // The runtime's main thread: the child's own, bound at its launch, or where a launcher (a version
    // manager's shim) started the runtime, that process's, only when it is of the child's tree (a pid
    // the ready message names is the host's word, never a process strobe did not start).
    let (runtime_pid, main_thread) = match spawned.runtime_pid {
        p if p == pid => (pid, spawned.child.main_thread()),
        p if host::tree_pids(pid).contains(&p) => (p, host::MainThread::attach(p)),
        p => (pid, Err(host::MainLost::Gone(format!("the script host named process {p} as its runtime, which is not of the process tree strobe started ({pid})")))),
    };
    // Each thread of the host's tree over the block, where its recording is kept (attribution), and in an
    // untimed block, whose one cold run it checks for startup still running (`block::startup_spill`).
    let timeline = (profiles.is_some() || x.untimed).then(|| probe::wakes::Timeline::start(move || host::tree_pids(pid)));
    let host = RefCell::new(spawned);
    // A host that cannot collect garbage (no collector to call) says so: refused, never a heap of garbage.
    let heap_mb = || -> Result<f64> {
        let used = host.borrow_mut().request(json!({ "op": "heap" }), Duration::from_secs(120)).map_err(|e| {
            if e.message.starts_with("js.heap.retained:") {
                Refusal::new(ValidatorId::EngineUnsupported, e.message).fix("host the module on a runtime strobe can collect: Bun, or Node started with --expose-gc").into()
            } else {
                e
            }
        })?;
        Ok(used.as_f64().unwrap_or(f64::NAN) / (1024.0 * 1024.0))
    };
    steps.push(("launch".into(), launched.elapsed().as_secs_f64() * 1e3));
    let main_read = || -> std::result::Result<host::MainCpu, host::MainLost> { main_thread.as_ref().map_err(Clone::clone)?.read() };
    let main_edges: RefCell<Vec<std::result::Result<host::MainCpu, host::MainLost>>> = RefCell::new(Vec::new());
    let hooks = super::block::Hooks::new(ctx, move || host::tree_pids(pid));
    let cost = SelfCost::new(ctx.runner.pid());
    // The host's CPU at each edge of the run's untimed stretches, with what tells a process of its tree
    // that ended (`host::ended_marks`), read right after the cycles and of the same processes.
    let edges: RefCell<Vec<(super::super::record::Cycles, host::EndedMarks)>> = RefCell::new(Vec::new());
    let ended_of = |c: &super::super::record::Cycles| host::ended_marks(&c.keys().map(|k| k.0).collect::<Vec<_>>(), None);
    // The run's `ctx.call`s in its timed parts: what strobe relays (`Overhead::script_calls`).
    let calls = Cell::new(0usize);
    let serve = |op: &str, a: &Value| -> Result<Value> {
        match op {
            "call" => {
                if hooks.in_run() && !hooks.untimed_now() {
                    calls.set(calls.get() + 1);
                }
                host.borrow_mut().request(json!({ "name": a["name"], "args": a["args"] }), Duration::from_secs(600))
            }
            "count" => {
                hooks.count(a);
                Ok(Value::Bool(true))
            }
            "phase" => hooks.phase(a),
            "untimed" => hooks.untimed(a, |start, _| {
                if start {
                    cost.pause();
                    main_edges.borrow_mut().push(main_read());
                }
                let c = cycles(pid);
                let e = ended_of(&c);
                edges.borrow_mut().push((c, e));
                if !start {
                    main_edges.borrow_mut().push(main_read());
                    cost.resume();
                }
                Ok(())
            }),
            "driver" => Err(Error::msg("a script target has no page: call its exports with ctx.call(name, ...args)")),
            "line" => Err(Error::usage("waitForLine reads a native app's output or log files; a script target's calls return what they return")),
            other => Err(Error::msg(format!("unknown runner request {other}"))),
        }
    };
    let t = Duration::from_secs(600);
    let result = (|| -> Result<Launch> {
        let root_text = host::paths::plain(&root);
        ctx.runner.call("block", json!({ "case": s.case, "params": s.params, "target": s.target, "variant": v.role, "url": "", "root": root_text }), t, &serve)?;
        let lapped = Instant::now();
        let set0 = host::now_ns();
        let setup = ctx.runner.call("setup", json!({}), t, &serve);
        life.borrow_mut().push(("setup".into(), set0, host::now_ns()));
        super::batch::launch_hook_case(ctx)?;
        setup.map_err(|e| crate::runner::incomplete("setup", "setup", e, "", "fix the bench file's setup"))?;
        hooks.check_fixture();
        steps.push(("setup".into(), lapped.elapsed().as_secs_f64() * 1e3));
        let perf = x.counters.then(probe::wakes::Perf::new);
        let mut launch = Launch::default();
        // The launch's runs so far: a case's runs are numbered after the ones before it.
        let mut base = 0usize;
        for (k, s) in cases.iter().enumerate() {
            // A launch hook that read what differs per case set up the first case alone (`unbatched`).
            if k > 0 && (stop_requested() || ctx.unbatched.is_some()) {
                break;
            }
            launch.log_at.push(ctx.runner.log_len());
            let lapped = Instant::now();
            // `js.heap.retained`: the post-GC heap a run leaves behind, read outside its timed span.
            let heap = declared_metrics(s).iter().any(|m| *m == "js.heap.retained");
            let r = (|| -> Result<Block> {
                let mut runs: Vec<ScriptRun> = Vec::new();
                let mut ends: Vec<Option<(f64, Option<u64>)>> = Vec::new();
                // Per run: what processes of the host's tree that ended in a timed part used there (ms), and
                // whether that is not known (their CPU there gone with them).
                let mut lost: Vec<(f64, bool)> = Vec::new();
                let mut cpu_reasons = Vec::new();
                // And by executable (ms, processes), where the OS names them.
                let mut ended_exes: Vec<Vec<(String, f64, u32)>> = Vec::new();
                // Per run: the share of the timed parts the host's main thread was on a CPU (`cpu.main`), or why none.
                let mut mains: Vec<std::result::Result<f64, host::MainLost>> = Vec::new();
                // Per run: its timed `ctx.call`s.
                let mut called: Vec<usize> = Vec::new();
                // Who else used the CPUs, from two snapshots outside the runs, for a block with other work.
                let procs0 = ctx.pin.map(|_| (host::now_ns(), host::all_processes()));
                // Steady once two runs cost the same CPU (SPEC §9.2).
                let mut warmup = super::block::Warmup::new(s, x);
                // A host with no collector refuses js.heap.retained alone (`EngineUnsupported`), once: the
                // block's other metrics stand, unless the heap is the primary (`slotted` refuses the block then).
                let mut heap_refused: Option<Refusal> = None;
                let heap_now = |heap_refused: &mut Option<Refusal>| -> Result<f64> {
                    if !heap || heap_refused.is_some() {
                        return Ok(f64::NAN);
                    }
                    match heap_mb() {
                        Err(e) if e.validator() == Some(ValidatorId::EngineUnsupported) => {
                            *heap_refused = e.refusal.map(|r| *r);
                            Ok(f64::NAN)
                        }
                        r => r,
                    }
                };
                while warmup.more(runs.len()) {
                    let h0 = heap_now(&mut heap_refused)?;
                    hooks.begin(&format!("run:{}", base + runs.len()));
                    let listed = probe::wakes::wall_us();
                    let k0 = probe::wakes::Counters::take(listed, &host::tree_pids(pid), perf.as_ref());
                    edges.borrow_mut().clear();
                    main_edges.borrow_mut().clear();
                    // Right before the cycles (nothing read between), so it adds nothing to the run's CPU; a
                    // process that differs between the two is refused, never miscounted (`dead_since`).
                    let ended0 = host::ended_marks(&host::tree_pids(pid), None);
                    let mut c0 = cycles(pid);
                    let busy = ctx.pin.and_then(|_| validity::CpuBusy::start());
                    let main0 = main_read();
                    if let Some(tl) = &timeline {
                        tl.mark_edge();
                    }
                    // The listed processes read again right at the run's start, and at its end before the tree
                    // is listed for new ones, each by itself and with the clocks: a listing takes milliseconds,
                    // which would be on one side of the run (`record::read_known`, `record::Clock`).
                    let at0 = super::super::record::read_known(c0.keys());
                    super::super::record::read_at(&mut c0, &at0);
                    let clock0 = super::super::record::Clock::now();
                    calls.set(0);
                    cost.start();
                    let t0 = host::now_ns();
                    ctx.runner.call("run", json!({ "case": s.case }), t, &serve).map_err(|e| crate::runner::incomplete(&format!("{} run", s.case), "run", e, "", "the run threw or timed out"))?;
                    let t1 = host::now_ns();
                    cost.end();
                    called.push(calls.get());
                    if base + runs.len() == 0 {
                        launch.first_run = Some((t0, t1));
                    }
                    let at1 = super::super::record::read_known(c0.keys().chain(edges.borrow().iter().flat_map(|e| e.0.keys())));
                    let clock1 = super::super::record::Clock::now();
                    let main1 = main_read();
                    if let Some(tl) = &timeline {
                        tl.mark_edge();
                    }
                    let mut m = main_edges.take();
                    if m.len() % 2 == 1 {
                        m.push(main1.clone());
                    }
                    let main = std::iter::once(main0)
                        .chain(m)
                        .chain(std::iter::once(main1))
                        .collect::<std::result::Result<Vec<_>, _>>()
                        .and_then(|at| host::main_share(&at).ok_or_else(|| host::MainLost::Gone("the CPU clock gave no rate over the run".into())));
                    mains.push(main);
                    // Ended after the known processes' readings and before the tree's listing, which takes
                    // milliseconds: the tree's CPU below lies inside it.
                    let busy = busy.zip(ctx.pin).and_then(|(b, cpus)| b.stop(cpus));
                    let mut c1 = cycles(pid);
                    super::super::record::read_at(&mut c1, &at1);
                    let ended1 = ended_of(&c1);
                    // The whole tree's CPU over the run, untimed stretches too, in cycles, with what processes
                    // of it that ended used (`record::timed_cpu`): the rest of the CPUs' busy time is other
                    // work. Unknown where what ended is not known.
                    let foreign = busy.zip(super::super::record::timed_cpu(&[&c0, &c1], &[&ended0, &ended1]).ok()).and_then(|(b, c)| b.other_cycles(c.total));
                    let counted = probe::wakes::counted(&k0, &probe::wakes::Counters::take(probe::wakes::wall_us(), &host::tree_pids(pid), perf.as_ref()));
                    hooks.end();
                    let timed = metrics::Timed { t0, t1, untimed: hooks.untimed_stretches() };
                    // CPU over the timed parts, each process against its own readings at their edges (a stretch
                    // the run never ended ends with it), at the cycle clock's rate over the run.
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
                    cpu_reasons.push(timed_cpu.as_ref().err().copied());
                    let timed_cpu = timed_cpu.ok();
                    // Short CPU is none (warmup falls back to the run's time).
                    let ms = |cycles: u64| if c1.values().any(|c| *c > 0) { clock0.ms(&clock1, cycles) } else { f64::NAN };
                    let used = timed_cpu.as_ref().map_or(f64::NAN, |c| ms(c.total));
                    cost.target_ms(used);
                    lost.push((timed_cpu.as_ref().map_or(0.0, |c| ms(c.ended)), timed_cpu.is_none()));
                    ended_exes.push(timed_cpu.as_ref().map(|c| c.ended_by.iter().map(|(n, (cy, k))| (n.clone(), ms(*cy), *k)).collect()).unwrap_or_default());
                    let retained = heap_now(&mut heap_refused)? - h0;
                    // The host's memory as the run left it, before its reset (SPEC §9.6), where this run may be
                    // the last measured one.
                    ends.push(warmup.may_end(runs.len()).then(|| {
                        let tree = host::process_tree(pid);
                        (tree.iter().map(|p| p.private_bytes).sum::<u64>() as f64 / (1024.0 * 1024.0), host::pss_bytes(&tree.iter().map(|p| p.pid).collect::<Vec<_>>()))
                    }));
                    runs.push((timed.ns() as f64 / 1e6, used, hooks.counts(), retained, hooks.take_marks(), counted, timed, foreign));
                    if s.reset {
                        let r0 = host::now_ns();
                        ctx.runner.call("reset", json!({ "case": s.case }), t, &serve).map_err(|e| crate::runner::incomplete(&format!("{} reset", s.case), "reset", e, "", "fix the case's reset"))?;
                        life.borrow_mut().push((format!("reset:{}", base + runs.len() - 1), r0, host::now_ns()));
                    }
                    // Without CPU (cycles that are not time), steady by wall time.
                    warmup.after(&runs.iter().map(|r| if r.1.is_finite() { r.1 } else { r.0 }).collect::<Vec<_>>(), &[], &lost.iter().map(|l| l.1).collect::<Vec<_>>());
                }
                // In a batch the host must outlive the case: one it ended fails the case that did it.
                if batch && let Some(code) = host.borrow_mut().child.exited() {
                    return Err(super::super::record::exited(code, &format!("after {}'s runs", s.case), None));
                }
                let first = warmup.first().unwrap_or(0);
                // A check that measured its second run (`Warmup::after`): run 1 is not what it measured.
                if base == 0 && first > 0 {
                    launch.first_run = None;
                }
                let local = first..warmup.measured().end.clamp(first, runs.len());
                let (strobe_pct, runner_pct) = cost.share(base + local.start..base + local.end);
                let mut steps = steps.clone();
                steps.push(("runs".into(), lapped.elapsed().as_secs_f64() * 1e3));
                let measured = &runs[local.clone()];
                let key = |j: usize| format!("run:{}", base + first + j);
                let mut extra: BTreeMap<String, BTreeMap<String, f64>> = BTreeMap::new();
                extra.insert("run.duration".into(), measured.iter().enumerate().map(|(j, r)| (key(j), r.0)).collect());
                if measured.iter().all(|r| r.1.is_finite()) {
                    extra.insert("cpu.work".into(), measured.iter().enumerate().map(|(j, r)| (key(j), r.1)).collect());
                    extra.insert("cpu.app.total".into(), measured.iter().enumerate().filter(|(_, r)| r.0 > 0.0).map(|(j, r)| (key(j), 100.0 * r.1 / r.0)).collect());
                }
                if heap && heap_refused.is_none() {
                    extra.insert("js.heap.retained".into(), measured.iter().enumerate().map(|(j, r)| (key(j), r.3)).collect());
                }
                let main_lost = mains[local.clone()].iter().find_map(|m| m.as_ref().err()).cloned();
                if main_lost.is_none() {
                    let main = &mains[local.clone()];
                    // The share of the run's timed parts on a CPU (`host::main_share`), and it in ms of them.
                    extra.insert("cpu.main".into(), main.iter().zip(measured).enumerate().filter_map(|(j, (m, r))| Some((key(j), m.as_ref().ok()? * r.0))).collect());
                    extra.insert("cpu.main.busy".into(), main.iter().enumerate().filter_map(|(j, m)| Some((key(j), 100.0 * m.as_ref().ok()?))).collect());
                }
                let hooked: Vec<super::block::Hooked> = measured.iter().map(|r| (&r.2, r.4.as_slice(), &r.6)).collect();
                extra.extend(super::block::per_run_hooks(s, &key, &hooked));
                let counted: Vec<(String, &probe::wakes::Counted)> = measured.iter().enumerate().map(|(j, r)| (key(j), &r.5)).collect();
                let (counts, uncounted) = probe::wakes::counter_metrics(&counted, declared_metrics(s));
                extra.extend(counts);
                let mut values = metrics::block_values(&metrics::BlockData::default(), "run", &extra);
                warmup.values(&mut values);
                // Never a reading from after the reset in its place: no value.
                let (private_mb, pss) = local.end.checked_sub(1).and_then(|i| ends.get(i).copied().flatten()).unwrap_or((0.0, None));
                // macOS, a Bun host: its allocator gives freed pages back as reusable (out of the footprint, still
                // resident) and writes them again without saying so, so the footprint misses memory it holds
                // again (air: 100 MiB held read as 5.1 MiB, and a planted +100 MiB as +0.19 MB). Resident size
                // cannot tell those pages from free ones, so a Bun host's footprint is never a value. Node's
                // allocator tells macOS (the same 100 MiB read 114 MiB). Bun as the host says it runs (a
                // `node` on PATH may be Bun's shim), not as the target declares.
                let footprint_short = cfg!(target_os = "macos") && host.borrow().bun;
                if private_mb > 0.0 && !footprint_short {
                    values.insert("memory.app.private".into(), private_mb);
                }
                if let Some(b) = pss {
                    values.insert("memory.app.pss".into(), b as f64 / (1024.0 * 1024.0));
                }
                let mut refused = uncounted;
                if footprint_short && declared_metrics(s).iter().any(|m| *m == "memory.app.private") {
                    refused.insert("memory.app.private".into(), super::bun_footprint_refusal());
                }
                if let Some(r) = heap_refused.take() {
                    refused.insert("js.heap.retained".into(), r);
                }
                if let Some(lost) = &main_lost {
                    for m in declared_metrics(s).into_iter().filter(|m| *m == "cpu.main" || *m == "cpu.main.busy") {
                        refused.insert(m.clone(), lost.refusal(m));
                    }
                }
                // A process the host started that ended inside a timed part took its CPU with it, where what
                // it used is not known.
                let gone = lost[local.clone()].iter().filter(|l| l.1).count();
                let exes: Vec<(String, f64, u32)> = lost[local.clone()].iter().zip(&ended_exes[local.clone()]).filter(|(l, _)| !l.1).flat_map(|(_, e)| e.iter().cloned()).collect();
                super::note_ended(&mut ctx.warnings, s, &lost[local.clone()], &exes);
                if gone > 0 {
                    for m in ["cpu.work", "cpu.app.total", "cpu.process.total"] {
                        values.remove(m);
                        if declared_metrics(s).iter().any(|d| *d == m) {
                            refused.insert(m.to_string(), super::super::record::cpu_lost_with(m, gone, measured.len(), "the script host's tree", cpu_reasons[local.clone()].iter().copied().flatten()));
                        }
                    }
                }
                // Other work on the target's CPUs during the runs: kept and flagged (`Background`, SPEC §10.2).
                let foreign: Vec<f64> = measured.iter().filter_map(|r| r.7).collect();
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
                refuse_not_on_this_os(s, &mut values, &mut refused);
                super::block::untimed_state(s, &measured.iter().map(|r| &r.6).collect::<Vec<_>>(), &mut values, &mut refused);
                let unread: Vec<String> = declared_metrics(s).into_iter().filter(|m| script_unread(m) && !values.contains_key(*m) && !refused.contains_key(*m)).cloned().collect();
                for m in unread {
                    let r = script_unread_refusal(&m);
                    refused.insert(m, r);
                }
                if profiles.is_some() {
                    for (i, r) in runs.iter().enumerate().filter(|(i, _)| !local.contains(i)) {
                        let what = if i < local.start { "warm" } else { "unmeasured" };
                        life.borrow_mut().push((format!("{what}:{}", base + i), r.6.t0, r.6.t1));
                    }
                    let ended = lost[local.clone()].iter().zip(&ended_exes[local.clone()]);
                    kept.borrow_mut().extend(measured.iter().zip(&mains[local.clone()]).zip(ended).enumerate().map(|(j, ((r, m), (l, e)))| Measured {
                        key: key(j),
                        case: s.case.clone(),
                        timed: r.6.clone(),
                        marks: r.4.clone(),
                        ms: r.0,
                        cpu_ms: r.1,
                        main_ms: m.clone().map(|share| share * r.0),
                        ended: (l.0, l.1, e.clone()),
                    }));
                    listed.borrow_mut().extend(runs.iter().zip(&mains).zip(&lost).enumerate().map(|(i, ((r, m), l))| RunRow {
                        run: format!("run:{}", base + i),
                        measured: local.contains(&i),
                        ms: r.0,
                        cpu_work_ms: r.1,
                        task_ms: None,
                        main_cpu_ms: m.as_ref().ok().map(|share| share * r.0),
                        cpu_lost: l.1,
                    }));
                    passed.borrow_mut().push((k, base + local.start..base + local.end));
                }
                base += runs.len();
                Ok(Block {
                    variant: v.role.to_string(),
                    pair,
                    values,
                    run_ms: measured.iter().map(|r| r.0).collect(),
                    warmup: first,
                    speed_ratio: None,
                    nodes: Vec::new(),
                    recording: None,
                    phases: BTreeMap::new(),
                    frames: Vec::new(),
                    intervals: Vec::new(),
                    steps,
                    background,
                    leak: None,
                    refused,
                    locked: false,
                    display: None,
                    screen: None,
                    reserve: None,
                    shown: None,
                    overhead: Some(Overhead { strobe_pct, runner_pct, script_calls: Some(called[local.clone()].iter().sum::<usize>() as f64 / local.len().max(1) as f64), ..Overhead::default() }),
                    process_set: Some(programs(&host::process_tree(pid))),
                    fixture_connected: None,
                    client: None,
                    frame: false,
                    browser: None,
                    directions: BTreeMap::new(),
                    spread: super::run_spread(x, &extra),
                    runtime: Some(if host.borrow().bun { "bun" } else { "node" }.to_string()),
                    ..Block::default()
                })
            })();
            let end = r.is_err() || !s.reset_check;
            launch.case_ms.push(lapped.elapsed().as_secs_f64() * 1e3);
            launch.cases.push(r);
            // Runs that build on each other on purpose (`resetCheck: false`) leave a state no later case
            // may start from: the launch ends with it, as with a failed case.
            if end {
                break;
            }
        }
        if launch.cases.iter().any(Result::is_ok) {
            let c0 = host::now_ns();
            let _ = ctx.runner.call("close", json!({}), t, &serve);
            life.borrow_mut().push(("teardown".into(), c0, host::now_ns()));
        }
        Ok(launch)
    })();
    // Its last tick while the host still runs: the exit is no part of the block.
    let timeline = timeline.map(probe::wakes::Timeline::stop);
    // A profiled host writes its profiles as it exits: tens of MB for a long block, which took over 30 s
    // on a 2-vCPU VM at 100%. A minute and half the block's time, at most 3, within `--max-time` but
    // never under the 30 s an unprofiled host gets.
    let wait = if profiles.is_some() { (Duration::from_secs(60) + launched.elapsed() / 2).min(Duration::from_secs(180)) } else { Duration::from_secs(30) };
    let left = time_left(ctx).max(Duration::from_secs(30));
    let (wait, cut) = if left < wait { (left, Some("--max-time")) } else { (wait, None) };
    let (exited, ended) = host.into_inner().exit(profiles.is_some(), wait, cut);
    let mut launch = result?;
    // A check's one cold run that measured the host's startup still running says so.
    if let Some(why) = timeline.as_ref().zip(launch.first_run.filter(|_| x.untimed)).and_then(|(t, (t0, t1))| super::block::startup_spill(t, t0, t1, "the host started")) {
        ctx.warnings.push(validity::warn("NotSteady", format!("{} · {} block: {why}", s.label(), side_of(ctx, v))));
    }
    if let Some(dir) = &profiles
        && launch.cases.iter().any(Result::is_ok)
    {
        let clocks = ready.zip(exited).map(|(r, e)| e.map(|e| (r, e)));
        let made = Profiled {
            dir: &dir.0,
            id: &id,
            pid: runtime_pid,
            main_tid: main_thread.as_ref().ok().map(|m| m.tid),
            cases,
            v,
            target: &target,
            clocks,
            ended,
            log: &log,
            life: &life.take(),
            runs: &kept.take(),
            listed: &listed.take(),
            passed: &passed.take(),
            timeline: timeline.as_ref(),
        };
        match made.keep(ctx, &launch) {
            Ok(()) => {
                for b in launch.cases.iter_mut().flatten() {
                    b.recording = Some(id.clone());
                }
            }
            // The block's profile is the reason it ran: without a valid one there is no block.
            Err(e) => {
                for c in launch.cases.iter_mut().filter(|c| c.is_ok()) {
                    *c = Err(e.clone());
                }
            }
        }
    }
    Ok(launch)
}

/// What a profiled host left (`--cpu-prof`, `--heap-prof` in `dir`) and what to cut it to.
struct Profiled<'a> {
    dir: &'a Path,
    id: &'a str,
    /// The runtime's process (the host, or the runtime its launcher started) and its main thread's id.
    pid: u32,
    main_tid: Option<u32>,
    cases: &'a [Series],
    v: &'a Variant,
    target: &'a Value,
    /// The host's clocks at its start and before its exit; an error where the exit's could not be read.
    clocks: Option<Result<(Clocks, Clocks)>>,
    /// How the host ended, and the experiment's script.log (its output): why it wrote no profile.
    ended: ExitState,
    log: &'a Path,
    life: &'a [(String, u64, u64)],
    runs: &'a [Measured],
    /// Every run of the passing cases, warmup and unmeasured ones too (`run_list`).
    listed: &'a [RunRow],
    passed: &'a [(usize, std::ops::Range<usize>)],
    /// The host's threads over the block (`query threads --timeline`).
    timeline: Option<&'a probe::wakes::TimelineData>,
}

impl Profiled<'_> {
    /// The block's recording (SPEC §8.3): the host's CPU profile on strobe's clock, cut to each measured
    /// run's timed span (`cpu-<run>.cpuprofile`, what `query hotspots` reads), and whole
    /// (`whole-host.json.gz`, startup, setup, warmup, runs, resets and teardown: `--source host`); the
    /// runs' spans and phase marks (`runs.json`) and the rest of the host's life (`lifecycle_spans`).
    /// Only passing cases' runs. Refuses a profile strobe cannot place within 1 ms (`ClockUnsynced`) or
    /// that ends before the last measured run did (`RunIncomplete`); nothing is kept then.
    fn keep(&self, ctx: &mut Ctx, launch: &Launch) -> Result<()> {
        let incomplete =
            |what: String| -> Error { Refusal::new(ValidatorId::RunIncomplete, format!("the script host's CPU profile: {what}")).fix("run it again; see the experiment's script.log").into() };
        let ((ready, runtime), (exit, _)) = match &self.clocks {
            Some(Ok(c)) => c.clone(),
            // A reading strobe refused (no tight wall-clock tick: `ClockUnsynced`) says so as it is.
            Some(Err(e)) if e.refusal.is_some() => return Err(e.clone()),
            Some(Err(e)) => return Err(incomplete(format!("its clock could not be read before it exited: {}", e.message))),
            None => return Err(incomplete("its clock was not read".into())),
        };
        // The runtime names its own files: the newest of each kind is this process's.
        let newest = |ext: &str| {
            std::fs::read_dir(self.dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == ext))
                .max_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
        };
        let raw = newest("cpuprofile").ok_or_else(|| -> Error {
            // What the probe sent leaves the log before its tail goes into the refusal.
            ctx.runner.scrub_log(self.log);
            let tail = log_tail(self.log, 6);
            let r = Refusal::new(ValidatorId::RunIncomplete, format!("the script host's CPU profile: the host wrote none: {}", self.ended));
            let r = if tail.is_empty() { r.detail("script.log is empty") } else { r.detail(format!("script.log ends: {}", tail.join(" | "))) };
            match self.ended {
                ExitState::TimedOut(..) => r.fix("run it again on a quieter machine (nothing else busy on its CPUs), or with fewer or shorter runs; see the experiment's script.log"),
                _ => r.fix("see why the host ended in the experiment's script.log; run it again"),
            }
            .into()
        })?;
        let bytes = std::fs::read(&raw)?;
        crate::store::deep_enough(&bytes, &raw)?;
        let mut de = serde_json::Deserializer::from_slice(&bytes);
        de.disable_recursion_limit();
        let profile: Value = serde::Deserialize::deserialize(&mut de).map_err(|e: serde_json::Error| incomplete(format!("{}: {e}", raw.display())))?;
        let rt = runtime["name"].as_str().and_then(probe::cpu::Runtime::of).ok_or_else(|| incomplete(format!("an unknown runtime {}", runtime["name"])))?;
        let rebased = probe::cpu::rebase(&profile, rt, &ready, &exit)?;
        // `rebase` found samples of the spin before the exit, after every run: the profile covers them.
        let us = |ns: u64| ns as f64 / 1e3;
        let draft = store::claim_recording(self.id)?;
        let rec = draft.dir().to_path_buf();
        // The target's own data, owner-only (SPEC rule 9).
        for r in self.runs {
            let untimed: Vec<(f64, f64)> = r.timed.untimed.iter().map(|&(a, b)| (us(a), us(b))).collect();
            if let Some(p) = rebased.clip(&profile, (us(r.timed.t0), us(r.timed.t1)), &untimed) {
                owner_only(&rec.join(format!("cpu-{}.cpuprofile", r.key.trim_start_matches("run:"))), &serde_json::to_vec(&p)?)?;
            }
        }
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&serde_json::to_vec(&rebased.whole(&profile))?)?;
        owner_only(&rec.join(probe::cpu::WHOLE_HOST), &gz.finish()?)?;
        if let Some(p) = newest("heapprofile") {
            owner_only(&rec.join("alloc-0.heapprofile"), &std::fs::read(&p)?)?;
        }
        let runs: Vec<Value> = self
            .runs
            .iter()
            .map(|r| {
                json!({ "run": r.key, "case": r.case, "t0_ns": r.timed.t0, "t1_ns": r.timed.t1, "untimed": r.timed.untimed, "marks": r.marks, "ms": r.ms, "cpu_ms": r.cpu_ms, "main_cpu_ms": r.main_ms.as_ref().ok() })
            })
            .collect();
        store::write_json(&rec.join(probe::cpu::RUNS), &json!({ "runs": runs }))?;
        super::keep_actions(ctx, &rec)?;
        let segments: Vec<(f64, f64)> = self.runs.iter().flat_map(|r| r.timed.segments()).map(|(a, b)| (us(a), us(b))).collect();
        let sampling = probe::cpu::Sampling::of(probe::cpu::interval_us() as f64, rebased.gaps_in(&segments));
        // The main thread's CPU in the measured runs the profile holds no sample of; not known where
        // `cpu.main` was refused.
        let missed = match self.runs.iter().map(|r| r.main_ms.clone()).collect::<std::result::Result<Vec<f64>, _>>() {
            Ok(main) => probe::cpu::missed(main.iter().sum(), rebased.sampled_in(&profile, &segments) / 1e3),
            Err(lost) => json!({ "refused": lost.refusal("cpu.main").summary }),
        };
        for text in probe::cpu::missed_texts(&missed, runtime["name"].as_str()) {
            ctx.warnings.push(validity::warn("Attribution", format!("{}: {text}", self.cases[0].label())));
        }
        if let Some(t) = self.timeline {
            crate::symbol::write_gz(&rec.join("timeline.json.gz"), t)?;
        }
        let s = &self.cases[0];
        let batch = ctx.batch.is_some();
        let blocks: Vec<&Block> = launch.cases.iter().flatten().collect();
        let mut meta = json!({
            "id": self.id, "kind": "block", "experiment": ctx.exp, "seed": ctx.seed, "series": s.label(), "variant": self.v.role, "variant_label": self.v.label, "variant_git": variant_git(self.v),
            "target": self.target["entry"], "probes": Set::Attribution, "timing": "perturbed", "runs": self.runs.len(), "measured": self.runs.iter().map(|r| &r.key).collect::<Vec<_>>(),
            "values": if batch { json!({}) } else { json!(blocks.first().map(|b| &b.values)) },
            "created": super::super::record::now_iso(), "valid": true, "env": host::env(), "script": true,
            "runtime": runtime, "sampling": sampling, "missed": missed, "whole_host": probe::cpu::WHOLE_HOST, "pid": self.pid, "main_tid": self.main_tid,
            "lifecycle_spans": self.life.iter().map(|(name, a, b)| json!({ "name": name, "t0_ns": a, "t1_ns": b })).collect::<Vec<_>>(),
            "clock": { "rtt_ms": [ready.rtt_ns as f64 / 1e6, exit.rtt_ns as f64 / 1e6], "drift_ms": (exit.offset_ns - ready.offset_ns) / 1e6, "cap_ms": rebased.cap_us / 1e3, "mark_outliers": rebased.outliers, "mark_worst_ms": rebased.worst_ns / 1e6, "mark_fit_ms": rebased.fit_ms },
            "note": "cpu-<run>.cpuprofile: each measured run's timed span alone (query hotspots, --phase); whole-host.json.gz: the whole host process (startup, setup, warmup, runs, resets, teardown: query hotspots --source host); alloc-0.heapprofile (Node): sampled allocations of the whole host process; timeline.json.gz: the host's threads over the block (query threads --timeline)",
        });
        if batch {
            meta["batch"] = super::batch::meta(self.passed.iter().zip(&blocks).map(|((k, r), b)| (&self.cases[*k], r.clone(), *b)));
        }
        meta["ended_runs"] = super::ended_runs(self.runs.iter().map(|r| (r.key.clone(), true, r.ended.0, r.ended.1, r.ended.2.as_slice())));
        meta["run_list"] = super::run_list(self.listed.iter().cloned());
        store::write_json(&rec.join("meta.json"), &meta)?;
        draft.keep();
        Ok(())
    }
}

/// Writes `bytes` whole or not at all, readable by its owner alone (`store::create`).
fn owner_only(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut f = store::create(path)?;
    f.write_all(bytes)?;
    f.commit().map_err(|e| Error::msg(format!("write {}: {e}", path.display())))
}
