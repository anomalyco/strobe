//! The bench runner link: spawns `bun` with strobe's embedded SDK and speaks the runner protocol (SPEC §9.6).

use crate::host::{self, paths};
use crate::model::{Error, Refusal, Result};
use crate::probe::http_probe;
use crate::store::actions::{ActionLog, Hook, Outcome};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

const INDEX: &str = include_str!("../sdk/bench/src/index.ts");
const RUNNER: &str = include_str!("../sdk/bench/src/runner.ts");
const PRELOAD: &str = include_str!("../sdk/bench/src/preload.ts");
const HOST: &str = include_str!("../sdk/bench/src/host.mjs");

/// Writes the embedded SDK under the state dir (once per strobe build) and returns its folder. Each
/// use touches its `.used` file; other builds' folders are caches `strobe prune` removes (one per
/// build, they would pile up with every update).
pub fn sdk_dir() -> Result<PathBuf> {
    let root = paths::sdk();
    let dir = root.join(env!("STROBE_BUILD_ID"));
    std::fs::create_dir_all(&dir)?;
    // Marked used before anything else, so another strobe's prune never takes a folder being filled.
    if let Ok(f) = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join(".used")) {
        let _ = f.set_modified(std::time::SystemTime::now());
    }
    for (name, body) in [("index.ts", INDEX), ("runner.ts", RUNNER), ("preload.ts", PRELOAD), ("host.mjs", HOST)] {
        let p = dir.join(name);
        if std::fs::read_to_string(&p).ok().as_deref() != Some(body) {
            paths::write_atomic(&p, body.as_bytes())?;
        }
    }
    Ok(dir)
}

pub fn find_bun() -> Option<PathBuf> {
    let home = host::home();
    let exe = if cfg!(windows) { "bun.exe" } else { "bun" };
    host::find_exe("bun", &[home.join(".bun").join("bin").join(exe)])
}

/// A bench hook that failed, as `RunIncomplete`: `what` names it once (the runner's own `<op>: `
/// prefix dropped), `extra` follows the message (a screenshot), and a fix the driver gave (a missing
/// permission and where to grant it) replaces the generic `fix`. A hook that failed because a
/// validator refused one of its driver calls (`Runner::call`) is that validator's refusal, named
/// for the hook: a covered window is `WindowHidden`, which a bench waits out, not a broken case.
pub fn incomplete(what: &str, op: &str, e: Error, extra: &str, fix: &str) -> Error {
    // A question the bench file asked wrong (`ctx.httpProbe`'s options): exit 2, as it is.
    if e.code == 2 {
        return e;
    }
    let prefix = format!("{op}: ");
    let msg = e.message.strip_prefix(&prefix).unwrap_or(&e.message);
    match e.refusal {
        Some(r) => {
            let why = msg.strip_prefix(&format!("{}: ", r.validator)).unwrap_or(msg);
            Refusal { summary: format!("{what}: {why}{extra}"), detail: None, ..*r }.into()
        }
        None => Error::refused("RunIncomplete", format!("{what}: {msg}{extra}"), e.hint.as_deref().unwrap_or(fix)),
    }
}

/// The runner process, in a group of its own with everything its hooks start: dropped (after `exit`
/// or without it), the group ends.
pub struct Runner {
    /// Taken when it ends.
    child: Option<host::spawn::Owned>,
    pid: u32,
    conn: TcpStream,
    lines: Receiver<String>,
    next: u64,
    pub log: PathBuf,
    /// The experiment's seed, once given (`seed`).
    seed: Option<u32>,
    /// Some hook read `ctx.seed` or called `ctx.random()` (SPEC §14.2: only then is `--seed` the workload).
    used_random: bool,
    /// What the current block's hooks did to the target (`store::actions`), since its `block` call,
    /// until the block's recording takes it (`take_actions`).
    actions: RefCell<ActionLog>,
    /// Whether blocks from now log their actions (`collect_actions`): only those that keep them.
    collect: bool,
    /// The block's `run` calls so far, the current hook's open `ctx.untimed` stretch and `ctx.phase`s.
    runs: u32,
    untimed: bool,
    phases: Vec<String>,
    /// A batch (`--check --batch`): each run's and reset's actions name their case (`ActionRecord::case`).
    batch: bool,
    /// A launch hook (prepare, a `url` function, setup) read what differs per case (`ctx.case`,
    /// `ctx.seed`, `ctx.random()`) since the last `took_case`: the first hook and field.
    read_case: Option<(String, String)>,
    /// How long a CPU quota throttled strobe's cgroup during each `run` call of the block, in order
    /// (`host::cgroup_throttled_ns`; None where nothing reads it): `Throttled` over the measured runs.
    throttled: Vec<Option<u64>>,
    /// The block's HTTP probe (SPEC §8.9).
    http: Http,
}

/// What a block's setup asked of the HTTP probe (`ctx.httpProbe`), and the probe while it runs: from
/// the end of setup (after its idle control) until the block's teardown begins.
#[derive(Default)]
struct Http {
    /// Whether this block runs the probe its setup asks for, and on which CPUs (`Runner::http_probe`).
    on: bool,
    cpus: Option<u64>,
    /// What setup asked for: its setup as recordings keep it, the probe until it starts, or why the
    /// options were wrong (the setup's error, exit 2).
    described: Option<http_probe::Described>,
    asked: Option<http_probe::Config>,
    /// What the probe sends that strobe never writes: no memory dump while it runs (`http_secret`), and
    /// the experiment's logs scrubbed after the block (`http_probe::scrub_logs`).
    secrets: Option<http_probe::Secrets>,
    usage: Option<Error>,
    driver: Option<http_probe::Driver>,
    stopped: Option<http_probe::Stopped>,
    idle: Option<(u64, u64)>,
    /// Each `run` call's span on the host clock with its `ctx.untimed` stretches, and the stretch open now.
    runs: Vec<http_probe::Span>,
    untimed: Vec<(u64, u64)>,
    open: Option<u64>,
    /// The run's `ctx.phase` spans so far, and those open now (innermost last).
    phases: Vec<(String, u64, u64)>,
    open_phases: Vec<(String, u64)>,
}

/// A block's HTTP probe after the block: what setup asked for, and what the probe read (None when the
/// block ran none: a clean timing block, or a setup that failed before it started).
pub struct HttpTaken {
    pub described: http_probe::Described,
    pub data: Option<http_probe::Data>,
    /// What the probe sent, which the target or the hooks may have printed to the experiment's logs.
    pub secrets: http_probe::Secrets,
}

/// What the runner reports about a bench file.
pub type Describe = Value;

impl Runner {
    /// Spawns the runner for `file`; `cpus` pins it (and only it) to a CPU mask.
    pub fn spawn(file: &Path, log: PathBuf, cpus: Option<u64>) -> Result<Runner> {
        let bun = find_bun().ok_or_else(|| Error::msg("bench files need Bun").hint("install it from https://bun.sh"))?;
        let sdk = sdk_dir()?;
        // The protocol runs over a loopback socket, never the runner's stdio: a bench file's hooks spawn
        // processes, and Bun's `$` hands stdin on to them, which stalled on a protocol pipe that never
        // closes. stdin is empty; stdout and stderr are the experiment's runner.log.
        let listener = TcpListener::bind("127.0.0.1:0")?;
        // Owner-only: the hooks print what they like there (SPEC rule 9).
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        let out = o.open(&log)?;
        let err = out.try_clone()?;
        let mut cmd = Command::new(bun);
        cmd.arg("--preload")
            .arg(sdk.join("preload.ts"))
            .arg(sdk.join("runner.ts"))
            .arg(file)
            .env("STROBE_SDK_INDEX", sdk.join("index.ts"))
            .env("STROBE_RUNNER_PORT", listener.local_addr()?.port().to_string())
            .current_dir(file.parent().unwrap_or(Path::new(".")))
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err);
        // An early return from here drops the child, which ends the runner and what it started.
        let mut child = host::spawn_owned(&mut cmd, cpus, 0, None).map_err(|e| Error::msg(format!("spawn bun: {e}")))?;
        listener.set_nonblocking(true)?;
        let deadline = std::time::Instant::now() + Duration::from_secs(120);
        let conn = loop {
            match listener.accept() {
                Ok((conn, _)) => break conn,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(Error::msg(format!("runner connection: {e}"))),
            }
            if std::time::Instant::now() > deadline || child.exited().is_some() {
                drop(child);
                let text = std::fs::read_to_string(&log).unwrap_or_default();
                let tail: Vec<&str> = text.lines().rev().take(12).collect::<Vec<_>>().into_iter().rev().collect();
                return Err(Error::msg(format!("the bench runner did not start\n{}", tail.join("\n"))));
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        conn.set_nonblocking(false)?;
        conn.set_nodelay(true)?;
        let reader = conn.try_clone()?;
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(reader).lines().map_while(std::result::Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let pid = child.pid();
        let mut runner = Runner {
            child: Some(child),
            pid,
            conn,
            lines: rx,
            next: 1,
            log,
            seed: None,
            used_random: false,
            actions: RefCell::default(),
            collect: false,
            runs: 0,
            untimed: false,
            phases: Vec::new(),
            batch: false,
            read_case: None,
            throttled: Vec::new(),
            http: Http::default(),
        };
        runner.wait_ready()?;
        Ok(runner)
    }

    fn wait_ready(&mut self) -> Result<()> {
        loop {
            let line = self.lines.recv_timeout(Duration::from_secs(60)).map_err(|_| self.died("did not start"))?;
            if serde_json::from_str::<Value>(&line).ok().and_then(|v| v.get("ready").cloned()).is_some() {
                return Ok(());
            }
        }
    }

    fn died(&mut self, what: &str) -> Error {
        // What the probe sent leaves the log before its tail goes into the error.
        if let Some(s) = &self.http.secrets {
            http_probe::scrub_logs(std::slice::from_ref(&self.log), s);
        }
        let log = std::fs::read_to_string(&self.log).unwrap_or_default();
        let tail: Vec<&str> = log.lines().rev().take(12).collect::<Vec<_>>().into_iter().rev().collect();
        Error::msg(format!("the bench runner {what}\n{}", tail.join("\n")))
    }

    /// Takes what an HTTP probe sent the target out of a target's own log (a script host's), in place,
    /// before its tail goes into an error (as `died` does for the runner's).
    pub fn scrub_log(&self, log: &Path) {
        if let Some(s) = &self.http.secrets {
            http_probe::scrub_logs(&[log.to_path_buf()], s);
        }
    }

    fn write(&mut self, v: &Value) -> Result<()> {
        writeln!(self.conn, "{v}").map_err(|e| Error::msg(format!("runner: {e}")))?;
        self.conn.flush().ok();
        Ok(())
    }

    /// Calls `op` and serves the runner's requests (driver calls, phases) until it answers. The hooks'
    /// actions on the target go into the block's log (`actions`) as they are served: a host clock read
    /// at each end, no call to the target.
    pub fn call(&mut self, op: &str, args: Value, timeout: Duration, serve: impl FnMut(&str, &Value) -> Result<Value>) -> Result<Value> {
        if op == "block" {
            self.throttled.clear();
            let (on, cpus) = (self.http.on, self.http.cpus);
            self.http = Http { on, cpus, ..Http::default() };
        }
        // The probe stops as the teardown begins: the target may go away in it.
        if op == "close" {
            self.stop_http();
        }
        let run = Hook::of_op(op) == Some(Hook::Run);
        let quota = if run { host::cgroup_throttled_ns() } else { None };
        let probing = run && self.http.driver.is_some();
        let t0 = probing.then(host::now_ns);
        let r = self.exchange(op, args, timeout, serve);
        if run {
            self.throttled.push(quota.zip(host::cgroup_throttled_ns()).map(|(a, b)| b.saturating_sub(a)));
        }
        if let Some(t0) = t0 {
            let t1 = host::now_ns();
            let mut untimed = std::mem::take(&mut self.http.untimed);
            if let Some(a) = self.http.open.take() {
                untimed.push((a, t1));
            }
            let mut phases = std::mem::take(&mut self.http.phases);
            phases.extend(self.http.open_phases.drain(..).map(|(n, a)| (n, a, t1)));
            let key = format!("run:{}", self.runs.saturating_sub(1));
            self.http.runs.push(http_probe::Span { key, t0_ns: t0, t1_ns: t1, untimed, phases, measured: false });
        }
        if op == "setup" {
            return self.after_setup(r);
        }
        r
    }

    /// Whether the next block runs the HTTP probe its setup asks for, its threads on `cpus` (SPEC §8.9).
    /// A block that runs none still says what setup asked for (`take_http`).
    pub fn http_probe(&mut self, on: bool, cpus: Option<u64>) {
        self.http = Http { on, cpus, ..Http::default() };
    }

    /// After setup: its `ctx.httpProbe` options' mistake (exit 2, even when the hook caught it), else the
    /// probe starts once the server settles, and the idle control runs before any run.
    fn after_setup(&mut self, r: Result<Value>) -> Result<Value> {
        if let Some(e) = self.http.usage.take() {
            return Err(e);
        }
        let r = r?;
        if self.http.on
            && let Some(cfg) = self.http.asked.take()
        {
            std::thread::sleep(http_probe::SETTLE);
            let d = http_probe::Driver::start(cfg, self.http.cpus)?;
            match d.idle() {
                Ok(idle) => {
                    self.http.driver = Some(d);
                    self.http.idle = idle;
                }
                // The block ends here: the probe stops at once (its probes kept for `take_http`).
                Err(e) => {
                    self.http.stopped = Some(d.stop());
                    return Err(e);
                }
            }
        }
        Ok(r)
    }

    /// Whether this block's HTTP probe runs (or ran) with something strobe never writes (an
    /// authorization, a query value, a header's value): the target has it then, so no dump of the
    /// target's memory is taken (`--probes memory` aside).
    pub fn http_secret(&self) -> bool {
        self.http.secrets.as_ref().is_some_and(|s| !s.is_empty()) && (self.http.driver.is_some() || self.http.stopped.is_some())
    }

    fn stop_http(&mut self) {
        if let Some(d) = self.http.driver.take() {
            self.http.stopped = Some(d.stop());
        }
    }

    /// The block's HTTP probe, stopped: what setup asked for and what it read; None when setup asked
    /// for none.
    pub fn take_http(&mut self) -> Option<HttpTaken> {
        self.stop_http();
        let h = std::mem::take(&mut self.http);
        let described = h.described?;
        let data = h.stopped.map(|s| http_probe::Data { config: described.clone(), start_ns: s.start_ns, end_ns: s.end_ns, cpu_ms: s.cpu_ms, idle: h.idle, runs: h.runs, samples: s.samples });
        Some(HttpTaken { described, data, secrets: h.secrets.unwrap_or_default() })
    }

    /// `ctx.httpProbe(options)` from setup: its options read (the authorization into a `Secret`), and
    /// the reply. Never an action of the block's log, never echoed.
    fn ask_http(&mut self, hook: Option<Hook>, args: &Value) -> std::result::Result<(), Error> {
        let r = if hook != Some(Hook::Setup) {
            Err(Error::usage("ctx.httpProbe: call it in the bench file's setup, once the server is up"))
        } else if self.http.described.is_some() {
            Err(Error::usage("ctx.httpProbe: one probe per block, and setup asked for two"))
        } else {
            http_probe::Config::parse(args)
        };
        match r {
            Ok(cfg) => {
                self.http.described = Some(cfg.describe());
                self.http.secrets = Some(cfg.secrets());
                self.http.asked = Some(cfg);
                Ok(())
            }
            Err(e) => {
                self.http.usage = Some(e.clone());
                Err(e)
            }
        }
    }

    /// How long a CPU quota throttled strobe's cgroup during each `run` call since the block began.
    pub fn throttled(&self) -> &[Option<u64>] {
        &self.throttled
    }

    fn exchange(&mut self, op: &str, args: Value, timeout: Duration, mut serve: impl FnMut(&str, &Value) -> Result<Value>) -> Result<Value> {
        let id = self.next;
        self.next += 1;
        if op == "block" {
            self.actions.borrow_mut().clear(self.seed);
            self.runs = 0;
            self.read_case = None;
        }
        let hook = Hook::of_op(op);
        if hook == Some(Hook::Run) {
            self.runs += 1;
        }
        // A stretch or phase a failed hook left open is none of the next one's.
        if hook.is_some() {
            self.untimed = false;
            self.phases.clear();
        }
        // A reset belongs to the run before it.
        let run = matches!(hook, Some(Hook::Run | Hook::Reset)).then(|| self.runs.checked_sub(1)).flatten();
        let case = args["case"].as_str().filter(|_| self.batch && matches!(hook, Some(Hook::Run | Hook::Reset))).map(str::to_string);
        self.write(&json!({ "id": id, "op": op, "args": args }))?;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Err(Error::timeout(format!("{op} took longer than {} s", timeout.as_secs())));
            }
            let mut line = match self.lines.recv_timeout(left) {
                Ok(l) => l,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(_) => return Err(self.died("exited")),
            };
            let Ok(mut msg) = serde_json::from_str::<Value>(&line) else { continue };
            // `ctx.httpProbe`: its options hold the authorization. Read here, never served, logged or
            // echoed, and every copy of the line zeroed once read.
            if msg["op"] == "http_probe"
                && let Some(rid) = msg.get("id").and_then(Value::as_str).map(str::to_string)
            {
                let mut args = msg.get_mut("args").map(Value::take).unwrap_or(Value::Null);
                let asked = self.ask_http(hook, &args);
                http_probe::wipe_value(&mut args);
                http_probe::wipe_value(&mut msg);
                http_probe::wipe_string(&mut line);
                let reply = match asked {
                    Ok(()) => json!({ "id": rid, "ok": true }),
                    Err(e) => json!({ "id": rid, "error": e.message }),
                };
                self.write(&reply)?;
                continue;
            }
            if msg.get("id").and_then(Value::as_u64) == Some(id) {
                self.used_random |= msg["used_random"] == true;
                if self.read_case.is_none()
                    && let (Some(hook), Some(field)) = (msg["read_case"]["hook"].as_str(), msg["read_case"]["field"].as_str())
                {
                    self.read_case = Some((hook.to_string(), field.to_string()));
                }
                if let Some(err) = msg.get("error").and_then(Value::as_str) {
                    let stack = msg.get("stack").and_then(Value::as_str).unwrap_or("");
                    let first = stack.lines().find(|l| l.contains(".bench.")).unwrap_or("").trim();
                    let message = format!("{op}: {err}{}", if first.is_empty() { String::new() } else { format!(" ({first})") });
                    let hint = msg.get("hint").and_then(Value::as_str).map(str::to_string);
                    // A driver call a validator refused (a click on a covered window), rethrown by the
                    // hook: the refusal rides on the error frame both ways, so it keeps its validator.
                    let refusal = msg.get("refusal").and_then(|r| serde_json::from_value::<crate::model::Refusal>(r.clone()).ok());
                    return Err(match refusal {
                        Some(r) => Error { message, ..Error::from(r) },
                        None => Error { hint, ..Error::msg(message) },
                    });
                }
                return Ok(msg.get("ok").cloned().unwrap_or(Value::Null));
            }
            if let Some(rid) = msg.get("id").and_then(Value::as_str) {
                let req_op = msg.get("op").and_then(Value::as_str).unwrap_or("").to_string();
                let args = msg.get("args").cloned().unwrap_or(Value::Null);
                let action = hook.filter(|_| self.collect).and_then(|h| ActionLog::of_request(h, run, self.untimed, self.phases.last().cloned(), &req_op, &args, host::now_ns()));
                crate::driver::take_sending();
                // The probe's view of a run's `ctx.untimed` stretch: from before strobe's readings at its
                // start to after those at its end, so a probe due during them is in neither the run nor
                // out of it by mistake.
                let probed = hook == Some(Hook::Run) && self.http.driver.is_some() && matches!(req_op.as_str(), "untimed" | "phase");
                let edge = probed.then(|| (args["edge"] == "start", host::now_ns()));
                let served = serve(&req_op, &args);
                if let (Some((start, before)), Ok(Value::Bool(true))) = (edge, &served) {
                    match (req_op.as_str(), start) {
                        ("untimed", true) => self.http.open = Some(before),
                        ("untimed", false) => {
                            if let Some(a) = self.http.open.take() {
                                self.http.untimed.push((a, host::now_ns()));
                            }
                        }
                        (_, true) => self.http.open_phases.push((args["name"].as_str().unwrap_or("").to_string(), before)),
                        (_, false) => {
                            if let Some((n, a)) = self.http.open_phases.pop() {
                                self.http.phases.push((n, a, host::now_ns()));
                            }
                        }
                    }
                }
                if let Some(mut a) = action {
                    a.case.clone_from(&case);
                    // What the driver said it sent: its send time, a click's press, what it aimed at.
                    let sending = crate::driver::take_sending();
                    a.t_ns = sending.t_ns.unwrap_or(a.t_ns);
                    (a.press_ns, a.hit, a.measured, a.touch) = (sending.press_ns, sending.hit, sending.measured, sending.touch);
                    a.end_ns = host::now_ns();
                    a.outcome = Outcome::of(&served);
                    self.actions.borrow_mut().push(a);
                }
                if served.is_ok() {
                    let start = args["edge"] == "start";
                    match req_op.as_str() {
                        "untimed" => self.untimed = start,
                        "phase" if start => self.phases.push(args["name"].as_str().unwrap_or("").to_string()),
                        "phase" => {
                            self.phases.pop();
                        }
                        _ => {}
                    }
                }
                let reply = match served {
                    Ok(v) => json!({ "id": rid, "ok": v }),
                    Err(e) => {
                        let mut reply = json!({ "id": rid, "error": e.message });
                        if let Some(h) = &e.hint {
                            reply["hint"] = json!(h);
                        }
                        if let Some(r) = &e.refusal {
                            reply["refusal"] = json!(r);
                        }
                        reply
                    }
                };
                self.write(&reply)?;
            }
        }
    }

    /// The runner's process, for its CPU in a block's overhead.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Gives the runner the experiment's seed (`--seed`, or drawn), before any block: hooks get each
    /// case's seed from it (`ctx.seed`, `ctx.random()`).
    pub fn seed(&mut self, seed: u32) -> Result<()> {
        self.seed = Some(seed);
        self.call("seed", json!({ "seed": seed }), Duration::from_secs(60), |_, _| Err(Error::msg("no driver while seeding")))?;
        Ok(())
    }

    /// Whether some hook so far read `ctx.seed` or called `ctx.random()`.
    pub fn used_random(&self) -> bool {
        self.used_random
    }

    /// Whether the blocks from now log their actions: only one whose recording (or `--check` result)
    /// keeps them does, so an ordinary timed block reads no clock and copies nothing for them.
    pub fn collect_actions(&mut self, on: bool) {
        self.collect = on;
    }

    /// The block's per-input latency outcomes (by send time), joined to its run actions once the runs
    /// are measured (`ActionLog::join`); `runs`: the measured runs' host-clock spans.
    pub fn join_samples(&self, each: &[(u64, crate::store::actions::Sample)], runs: &[(u64, u64)]) {
        self.actions.borrow_mut().join(each, runs);
    }

    /// Whether each run's and reset's actions name their case: a batch's launch runs several.
    pub fn batch(&mut self, on: bool) {
        self.batch = on;
    }

    /// The launch hook and the per-case field (`ctx.case`, `ctx.seed`, `ctx.random()`) it read since the
    /// last call, if one did (a batch's prepare, `url` function or setup runs once for several cases);
    /// cleared.
    pub fn took_case(&mut self) -> Option<(String, String)> {
        self.read_case.take()
    }

    /// Each series' `setupKey` (`{ case, target, params }` each) as canonical text, in order: cases share
    /// a batch's launch only when theirs are the same. A key that is not plain JSON, or a `setupKey` that
    /// throws, is the bench file's error (usage, exit 2).
    pub fn setup_keys(&mut self, series: Vec<Value>) -> Result<Vec<String>> {
        let n = series.len();
        let out = self.call("setup_keys", json!({ "series": series }), Duration::from_secs(60), |_, _| Err(Error::msg("no driver while keying setups"))).map_err(|e| {
            let why = e.message.strip_prefix("setup_keys: ").unwrap_or(&e.message).to_string();
            Error::usage(why).hint("return plain JSON from setupKey (strings, finite numbers, booleans, null, arrays and plain objects) built from ctx.case, ctx.target and ctx.params")
        })?;
        let keys: Vec<String> = out.as_array().into_iter().flatten().filter_map(|k| k.as_str().map(str::to_string)).collect();
        if keys.len() != n {
            return Err(Error::msg(format!("the bench runner gave {} setup keys for {n} series", keys.len())));
        }
        Ok(keys)
    }

    /// runner.log's length now: what the hooks printed from here on is the next case's.
    pub fn log_len(&self) -> u64 {
        std::fs::metadata(&self.log).map(|m| m.len()).unwrap_or(0)
    }

    /// The current (or last) block's actions since its `block` call, taken: written once.
    pub fn take_actions(&self) -> ActionLog {
        let mut log = self.actions.borrow_mut();
        let seed = log.seed;
        std::mem::replace(&mut *log, ActionLog { seed, ..ActionLog::default() })
    }

    /// What the bench file holds. Its shape failing the runner's checks (a key strobe does not read, a
    /// case without a run) is the file's error: usage, exit 2.
    pub fn describe(&mut self) -> Result<Describe> {
        self.call("describe", json!({}), Duration::from_secs(60), |_, _| Err(Error::msg("no driver during describe"))).map_err(|e| match e.message.strip_prefix("describe: ") {
            Some(why) if e.refusal.is_none() => Error::usage(why.to_string()),
            _ => e,
        })
    }

    /// Asks the runner to exit (its `close` hooks have run), up to 3 s, then ends its group.
    pub fn exit(&mut self) {
        let Some(mut child) = self.child.take() else { return };
        let _ = self.write(&json!({ "id": 0, "op": "exit" }));
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline && child.exited().is_none() {
            std::thread::sleep(Duration::from_millis(30));
        }
        child.end();
    }
}
