//! What every block does the same way, whatever runs it (a page, a native app, a script): warm up
//! until two runs do the same work, and serve the bench file's `ctx.count` and `ctx.phase` during a
//! run, then turn them into `count.<name>` and `phase.<name>` per run.

use super::*;

/// Two runs did the same work: within 5%, or within 1 ms (an idle run does about 3 ms, where 5% is
/// noise). Work that was not read (NaN) is never steady.
fn steady(a: f64, b: f64) -> bool {
    a.is_finite() && b.is_finite() && ((a - b).abs() <= 1.0 || (a.max(b) > 0.0 && (a - b).abs() / a.max(b) <= 0.05))
}

/// Warmup until steady (SPEC §9.1): runs go on until two consecutive ones do the same work, then
/// `Exec::runs` more are measured (a leak run's: twice that). Both steady runs count as measured in a
/// timing block; an attribution or memory block starts its probes at the steady point and measures
/// the runs after it, as does a block that never got steady in `MAX_WARMUP` runs. An untimed block
/// measures its first run; a `--check` of a case with a reset then runs it once more after the reset
/// (`again`), unmeasured, so a run that fails from the state the reset leaves fails the check.
pub(super) struct Warmup {
    /// The first measured run, once decided.
    first: Option<usize>,
    unsteady: bool,
    untimed: bool,
    /// Whether the two steady runs are measured (timing blocks).
    keeps_pair: bool,
    measured: usize,
    /// Unmeasured runs after the measured ones: a check's run after its reset.
    again: usize,
    /// Unsteady, and the work grew across the runs (`grew`).
    growth: bool,
    /// A timing block's measured runs stepped down (`stepped`): the first one's work and the median of
    /// the last half's.
    stepped: Option<(f64, f64)>,
    /// An untimed block that may measure its second run when its first lost a process: not a pixels
    /// block (its snapshots are the first run's) or a filmed one, and a case whose reset brings each run
    /// back to the same start (`resetCheck` on). Without a reset the second run starts where the first
    /// ended, another stretch of the target than a cold run 1, so run 1 stays measured and its `cpu.*`
    /// refuse.
    retry: bool,
}

impl Warmup {
    pub(super) fn new(s: &Series, x: &Exec) -> Warmup {
        let measured = if x.set == Set::Memory { 2 * x.runs } else { x.runs };
        // Not a pixels block: its snapshots are the first run's.
        let again = usize::from(x.untimed && s.reset && x.pixels.is_none());
        let retry = x.untimed && x.pixels.is_none() && !x.video && s.reset && s.reset_check;
        // A video block films the one run after the steady point: run 0 is cold, never the filmed one.
        Warmup { first: None, unsteady: false, untimed: x.untimed, keeps_pair: !matches!(x.set, Set::Attribution | Set::Memory) && !x.video, measured, again, growth: false, stepped: None, retry }
    }

    /// Whether another run is due after `ran` runs.
    pub(super) fn more(&self, ran: usize) -> bool {
        self.first.is_none_or(|f| ran < f + self.measured + self.again)
    }

    /// The measured runs, once warmup is over (before it: none).
    pub(super) fn measured(&self) -> std::ops::Range<usize> {
        self.first.map_or(0..0, |f| f..f + self.measured)
    }

    /// Whether run `i` is a measured one.
    pub(super) fn measures(&self, i: usize) -> bool {
        self.measured().contains(&i)
    }

    /// Whether run `i` may turn out to be the last measured one. Once warmup is over, the last of the
    /// measured runs. Before (`after` decides only then): an untimed block measures its first run, so
    /// that run with one measured run; a timing block keeps the steady pair (`first` is the run before
    /// the steady one), so the run now or the one before the next ends the measured runs only with one
    /// or two of them; every other block measures from the run after the steady point.
    pub(super) fn may_end(&self, i: usize) -> bool {
        match self.first {
            Some(f) => i + 1 == f + self.measured,
            None if self.untimed => self.measured == 1,
            None => self.keeps_pair && self.measured <= 2,
        }
    }

    /// The first measured run, once warmup is over.
    pub(super) fn first(&self) -> Option<usize> {
        self.first
    }

    /// Never steady in `MAX_WARMUP` runs (`warmup.unsteady`, `NotSteady`).
    pub(super) fn unsteady(&self) -> bool {
        self.unsteady
    }

    /// After a run, with each run's work so far and whether a process of the target ended in it (`lost`;
    /// empty where nothing says): true when warmup ended with this run. Two runs are steady only when a
    /// process ended in both or in neither: a chore the target ran once (Chromium's processor-metrics
    /// utility process, a moment after its first page loaded) stays in warmup, where it had refused the
    /// block's `cpu.*`. An untimed block whose one run lost a process, and that may run again
    /// (`retry`: a case with a reset), measures the run after the reset instead, once; one without a
    /// reset keeps its run, whose lost `cpu.*` refuse. `cpu`, where the work is one process's main
    /// thread (a page's renderer): the target's whole process tree's CPU per run (NaN where unread or
    /// short), which must hold steady too: a browser's after-startup chores (Chromium's network and
    /// storage services' deferred work, about 230 ms of CPU just after its first page loads) leave the
    /// renderer's task time alone, and made a cold block's first measured run's `cpu.work` 4x.
    ///
    /// A timing block's steady pair may be a plateau of work left over from the launch (a model load
    /// that keeps a thread busy through the first runs, so each does the same: a core's worth). A
    /// measured run after the pair, up to `MAX_WARMUP`, that drops below half the plateau's work (and
    /// by 10 ms or more) shows the pair was never the steady point: warmup goes on from there.
    /// Measured runs that still step down (the first over twice the last half's median, by 20 ms or
    /// more) are said (`stepped`, `NotSteady`).
    pub(super) fn after(&mut self, work: &[f64], cpu: &[f64], lost: &[bool]) -> bool {
        if work.is_empty() {
            return false;
        }
        let i = work.len() - 1;
        if let Some(f) = self.first {
            if !self.keeps_pair || self.untimed || self.unsteady || i >= f + self.measured {
                return false;
            }
            if i > f + 1 && i <= MAX_WARMUP && work[i] < 0.5 * work[f] && work[f] - work[i] >= 10.0 {
                self.first = None;
                return false;
            }
            if i + 1 == f + self.measured && self.measured >= 2 {
                let m = &work[f..=i];
                let tail = metrics::median(&m[m.len() / 2..]);
                if m[0] > 2.0 * tail && m[0] - tail >= 20.0 {
                    self.stepped = Some((m[0], tail));
                }
            }
            return false;
        }
        if self.untimed {
            self.first = Some(usize::from(i == 0 && self.retry && lost.first() == Some(&true)));
            return true;
        }
        let tree = |j: usize| cpu.get(j).copied().filter(|c| c.is_finite());
        let tree_steady = i >= 1 && tree(i - 1).zip(tree(i)).is_none_or(|(a, b)| steady(a, b));
        let steady = i >= 1 && steady(work[i - 1], work[i]) && tree_steady && lost.get(i - 1) == lost.get(i);
        if !steady && i < MAX_WARMUP {
            return false;
        }
        self.unsteady = !steady;
        // Unsteady: work that grew run after run (rank correlation over 0.5 and the last run 5% over
        // the first), or swung back and forth.
        self.growth = !steady && validity::spearman(work) > 0.5 && work[i] > work[0] * 1.05;
        self.first = Some(if steady && self.keeps_pair { i - 1 } else { i + 1 });
        true
    }

    /// Never steady because the work grew across the warmup runs, not because it swung (`NotSteady`).
    pub(super) fn grew(&self) -> bool {
        self.growth
    }

    /// What a block's values keep of its warmup: `warmup.unsteady` (2 grew, 1 swung), and where its
    /// measured runs stepped down, the first one's work and the last half's median (ms of CPU, or of
    /// wall time where the block read no CPU): `warmup.stepped.first`, `warmup.stepped.last`.
    pub(super) fn values(&self, values: &mut BTreeMap<String, f64>) {
        if self.unsteady {
            values.insert("warmup.unsteady".into(), if self.growth { 2.0 } else { 1.0 });
        }
        if let Some((first, tail)) = self.stepped {
            values.insert("warmup.stepped.first".into(), first);
            values.insert("warmup.stepped.last".into(), tail);
        }
    }
}

/// A thread or the whole set busy at this share of a core (%) is busy, for `startup_spill`.
const SPILL_BUSY_PCT: f64 = 25.0;

/// Startup still running in an untimed block's measured run 1 (`NotSteady`, never a refusal): run 1
/// (`t0`..`t1`, 2 s or longer) found a thread of the target, or its whole set, busy at a quarter of a
/// core or more in the 2 s before it began and in its first half (50 ms or more), and three times
/// quieter in its second half: work the launch began, not the case, which a `--check`'s one cold run
/// measures. A steady busy target (a game, an animation) is as busy in both halves and says nothing.
/// From the block's timeline (`wakes::Timeline`: CPU per thread a second, and at the run's edges); each
/// window takes a tick's CPU in proportion to their overlap. What it says: the thread (its name, module
/// and origin), when it was busy (from the launch), and the wait that fixes it.
pub(super) fn startup_spill(t: &probe::wakes::TimelineData, t0: u64, t1: u64) -> Option<String> {
    if t1.saturating_sub(t0) < 2_000_000_000 || t.cpu_refused.is_some() || t0 <= t.t0_ns {
        return None;
    }
    let mid = t0 + (t1 - t0) / 2;
    let pre = (t0.saturating_sub(2_000_000_000).max(t.t0_ns), t0);
    let span = |i: usize| (if i == 0 { t.t0_ns } else { t.ticks[i - 1] }, t.ticks[i]);
    // A window's share of tick `i`'s span.
    let share = |i: usize, (a, b): (u64, u64)| {
        let (from, to) = span(i);
        if to <= from {
            return 0.0;
        }
        to.min(b).saturating_sub(from.max(a)) as f64 / (to - from) as f64
    };
    let n = t.threads.len();
    // Each thread's CPU ms in each window: before run 1, its first half, its second half.
    let mut ms = vec![[0.0f64; 3]; n + 1];
    for &(tick, th, cpu, _) in &t.cells {
        let (tick, th) = (tick as usize, th as usize);
        if tick >= t.ticks.len() || th >= n || t.cpu_unknown.contains(&(th as u32)) {
            continue;
        }
        for (k, w) in [pre, (t0, mid), (mid, t1)].into_iter().enumerate() {
            let x = cpu as f64 * share(tick, w);
            ms[th][k] += x;
            ms[n][k] += x;
        }
    }
    let pct = |x: f64, (a, b): (u64, u64)| 100.0 * x / (b.saturating_sub(a) as f64 / 1e6).max(1e-9);
    let spilled = |m: &[f64; 3]| pct(m[0], pre) >= SPILL_BUSY_PCT && pct(m[1], (t0, mid)) >= SPILL_BUSY_PCT && m[1] >= 50.0 && m[1] >= 3.0 * m[2];
    // The busiest such thread, else the set as a whole (startup spread over a pool).
    let thread = (0..n).filter(|&i| spilled(&ms[i])).max_by(|&a, &b| ms[a][1].total_cmp(&ms[b][1]));
    let i = match thread {
        Some(i) => i,
        None if spilled(&ms[n]) => n,
        None => return None,
    };
    // When it was busy: the ticks up to the run's end it used a quarter of a core in, from the launch.
    let busy: Vec<usize> = (0..t.ticks.len())
        .filter(|&k| span(k).1 <= t1 && span(k).1 > span(k).0)
        .filter(|&k| {
            let used: f64 = t.cells.iter().filter(|c| c.0 as usize == k && (i == n || c.1 as usize == i)).map(|c| c.2 as f64).sum();
            pct(used, span(k)) >= SPILL_BUSY_PCT
        })
        .collect();
    let at = |ns: u64| ns.saturating_sub(t.t0_ns) as f64 / 1e9;
    let (from, to) = (busy.first().map_or(t.t0_ns, |&k| span(k).0), busy.last().map_or(mid, |&k| span(k).1));
    let who = match t.threads.get(i) {
        Some(th) => {
            let name = th.name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| format!("thread {}", th.tid));
            let module = th.module.as_deref().map(|m| format!(" ({}, {})", m.rsplit(['/', '\\']).next().unwrap_or(m), probe::wakes::origin(m))).unwrap_or_default();
            format!("{name}{module} of process {}", th.pid)
        }
        None => "the target's processes".to_string(),
    };
    let m = &ms[i];
    Some(format!(
        "startup was still running in run 1: {who} used {:.0}% of a core in the 2 s before it, {:.0}% in its first half and {:.0}% in its second; busy from {:.1} s to {:.1} s after the launch, while run 1 ran from {:.1} s to {:.1} s, so run 1 measured the launch's work, not the case's; wait in setup until the app is ready (ctx.waitForLine(<its ready line>, {{ since: \"launch\" }}), or a page.waitFor)",
        pct(m[0], pre),
        pct(m[1], (t0, mid)),
        pct(m[2], (mid, t1)),
        at(from),
        at(to),
        at(t0),
        at(t1)
    ))
}

/// `startup_spill` for a page, whose browser strobe keeps no thread timeline of: the OS sampler's busy
/// time on the CPUs the target is pinned to (`cpus`), the set as a whole, each window as the sampler's
/// ticks cover it (at least half of the 2 s before run 1, which begins at the sampler's start or later).
pub(super) fn startup_spill_cpus(samples: &probe::os::Samples, cpus: u64, t0: u64, t1: u64) -> Option<String> {
    let first_tick = samples.ticks.first().map(|t| t.t_ns.saturating_sub(t.dt_ns))?;
    if t1.saturating_sub(t0) < 2_000_000_000 || t0 <= first_tick {
        return None;
    }
    let mid = t0 + (t1 - t0) / 2;
    let pre = (t0.saturating_sub(2_000_000_000).max(first_tick), t0);
    if pre.1 - pre.0 < 1_000_000_000 {
        return None;
    }
    let on = |c: usize| c < 64 && cpus >> c & 1 == 1;
    // % of a core over a window: the pinned CPUs' busy shares summed.
    let busy = |(a, b): (u64, u64)| -> f64 { samples.window(a, b).cores.iter().filter(|c| on(c.cpu as usize)).filter_map(|c| c.busy_pct).sum() };
    let (p, f, s) = (busy(pre), busy((t0, mid)), busy((mid, t1)));
    let first_ms = f / 100.0 * (mid - t0) as f64 / 1e6;
    if p < SPILL_BUSY_PCT || f < SPILL_BUSY_PCT || first_ms < 50.0 || f < 3.0 * s {
        return None;
    }
    let tick_busy = |t: &probe::os::Tick| -> f64 { t.busy.iter().enumerate().filter(|(c, x)| on(*c) && x.is_finite()).map(|(_, x)| *x as f64).sum() };
    let busy_ticks: Vec<&probe::os::Tick> = samples.ticks.iter().filter(|t| t.t_ns <= t1 && tick_busy(t) >= SPILL_BUSY_PCT).collect();
    let at = |ns: u64| ns.saturating_sub(first_tick) as f64 / 1e9;
    let (from, to) = (busy_ticks.first().map_or(first_tick, |t| t.t_ns.saturating_sub(t.dt_ns)), busy_ticks.last().map_or(mid, |t| t.t_ns));
    Some(format!(
        "startup was still running in run 1: the target's CPUs were busy {p:.0}% of a core in the 2 s before it, {f:.0}% in its first half and {s:.0}% in its second; busy from {:.1} s to {:.1} s after the block's sampler began, while run 1 ran from {:.1} s to {:.1} s, so run 1 measured the browser's or page's startup work, not the case's; wait in setup until the page is ready (page.waitFor, or ctx.waitForLine(<a ready line>, {{ since: \"launch\" }}) for an app's output)",
        at(from),
        at(to),
        at(t0),
        at(t1)
    ))
}

/// A run's count that is the fixture's CPU (ms), not the bench file's: `cpu.fixture`.
pub(super) const FIXTURE_CPU: &str = "\u{1}cpu.fixture";

/// The bench file's `ctx.count` and `ctx.phase` during a block: they count only inside a run, whose
/// key (`run:<i>`) names its phase marks. With a fixture, its CPU over each run, and whether the
/// target's processes hold a connection to it at each run's start and end (outside the run).
#[derive(Default)]
pub(super) struct Hooks<'a> {
    run: RefCell<String>,
    counts: RefCell<BTreeMap<String, f64>>,
    /// Phase edges on the host clock, for targets strobe times itself (native, script).
    marks: RefCell<Vec<(String, u64)>>,
    fixture: Option<FixtureHook<'a>>,
    /// The fixture's CPU at the run's start, and how many times a process of it had ended by then
    /// (`fixture::Running::lost`).
    fixture_at: Cell<Option<CpuAt>>,
    fixture_lost: Cell<u64>,
    /// The run's `ctx.untimed` stretches on the host clock, the one open now (its start, and the
    /// fixture's CPU then), and the fixture's CPU in the closed ones (ms).
    untimed: RefCell<Vec<(u64, u64)>>,
    open: Cell<Option<(u64, Option<CpuAt>)>>,
    fixture_untimed: Cell<f64>,
    /// No input call since the last stretch ended (`ended_untimed`).
    ended: Cell<bool>,
}

/// The block's fixture, and how to list the target's processes now.
type FixtureHook<'a> = (std::rc::Rc<super::fixture::Running>, Box<dyn Fn() -> Vec<u32> + 'a>);

/// A tree's CPU reading (`script::cpu`) and when it was taken.
type CpuAt = ((u64, Option<u64>), Instant);

impl<'a> Hooks<'a> {
    /// A block's hooks; `pids` lists the target's processes for the fixture's connection check.
    pub(super) fn new(ctx: &Ctx, pids: impl Fn() -> Vec<u32> + 'a) -> Hooks<'a> {
        Hooks { fixture: ctx.fixture_running.clone().map(|f| (f, Box::new(pids) as Box<dyn Fn() -> Vec<u32> + 'a>)), ..Hooks::default() }
    }

    /// Reads whether the target holds a connection to the block's fixture (after setup).
    pub(super) fn check_fixture(&self) {
        if let Some((f, pids)) = &self.fixture {
            f.check(&pids());
        }
    }

    /// A run starts: what it counts and marks starts empty.
    pub(super) fn begin(&self, key: &str) {
        *self.run.borrow_mut() = key.to_string();
        self.counts.borrow_mut().clear();
        self.marks.borrow_mut().clear();
        self.untimed.borrow_mut().clear();
        self.open.set(None);
        self.fixture_untimed.set(0.0);
        self.ended.set(false);
        if let Some((f, pids)) = &self.fixture {
            f.check(&pids());
            self.fixture_at.set(Some((f.cpu(), Instant::now())));
            self.fixture_lost.set(f.lost());
        }
    }

    /// The run ended: from here nothing counts. A stretch still open (its end never came: the run
    /// threw) ends here.
    pub(super) fn end(&self) {
        if let Some((from, _)) = self.open.take() {
            self.untimed.borrow_mut().push((from, host::now_ns()));
        }
        self.run.borrow_mut().clear();
        if let (Some((f, pids)), Some((a, at))) = (&self.fixture, self.fixture_at.take()) {
            let ms = super::script::cpu_ms(a, f.cpu(), at.elapsed().as_secs_f64() * 1e3) - self.fixture_untimed.get();
            // A process of the fixture ended in the run: its CPU there went with it, so none (NaN).
            let ms = if f.lost() > self.fixture_lost.get() { f64::NAN } else { ms };
            self.counts.borrow_mut().insert(FIXTURE_CPU.into(), ms);
            f.check(&pids());
        }
    }

    /// A `ctx.untimed` edge (SPEC §9.6): the stretch between its start and its end is left out of the
    /// run's time, CPU, frames and inputs. Inside a run `read` takes the target's readings for the
    /// edge (true at the start, and the stretch's index in the run) inside the stretch: after its
    /// start is stamped and before its end is, so what they take (a system snapshot is 15-20 ms on
    /// Windows) is in neither the run's time nor its phases. Outside a run (setup, reset) nothing is
    /// timed, so there is nothing to do. One stretch inside another is a usage error.
    pub(super) fn untimed(&self, a: &Value, read: impl FnOnce(bool, usize) -> Result<()>) -> Result<Value> {
        if !self.in_run() {
            return Ok(Value::Bool(false));
        }
        let n = self.untimed.borrow().len();
        let fixture = || self.fixture.as_ref().map(|(f, _)| (f.cpu(), Instant::now()));
        if a["edge"] == "start" {
            if self.open.get().is_some() {
                return Err(Error::usage("ctx.untimed inside ctx.untimed: one stretch at a time"));
            }
            self.open.set(Some((host::now_ns(), fixture())));
            return read(true, n).map(|_| Value::Bool(true));
        }
        let Some((from, cpu)) = self.open.get() else { return Ok(Value::Bool(false)) };
        let r = read(false, n);
        if let (Some((a, at)), Some((b, _))) = (cpu, fixture()) {
            let ms = super::script::cpu_ms(a, b, at.elapsed().as_secs_f64() * 1e3);
            if ms.is_finite() {
                self.fixture_untimed.set(self.fixture_untimed.get() + ms);
            }
        }
        self.open.set(None);
        self.untimed.borrow_mut().push((from, host::now_ns()));
        self.ended.set(true);
        r.map(|_| Value::Bool(true))
    }

    /// Whether an untimed stretch is open now.
    pub(super) fn untimed_now(&self) -> bool {
        self.open.get().is_some()
    }

    /// Whether the run's last step was an untimed stretch: no input call after its end.
    pub(super) fn ended_untimed(&self) -> bool {
        self.ended.get()
    }

    /// The run's untimed stretches so far, on the host clock.
    pub(super) fn untimed_stretches(&self) -> Vec<(u64, u64)> {
        self.untimed.borrow().clone()
    }

    /// The trace mark of an untimed edge: `untimed:<run>.<n>:start|end` (no `:run:` in it, so no reader
    /// of run windows mistakes it for a run; `metrics::BlockData` takes it out of its run).
    pub(super) fn untimed_mark(&self, n: usize, start: bool) -> String {
        let run = self.run.borrow();
        format!("untimed:{}.{n}:{}", run.trim_start_matches("run:"), if start { "start" } else { "end" })
    }

    pub(super) fn in_run(&self) -> bool {
        !self.run.borrow().is_empty()
    }

    /// The run's key (`run:<i>`), empty outside a run.
    pub(super) fn run_key(&self) -> String {
        self.run.borrow().clone()
    }

    /// `ctx.count(name, n)`; outside a run, or in an untimed stretch, it counts nothing.
    pub(super) fn count(&self, a: &Value) {
        if self.in_run() && !self.untimed_now() && a["name"] != FIXTURE_CPU {
            self.add(&BTreeMap::from([(a["name"].as_str().unwrap_or("").to_string(), a["n"].as_f64().unwrap_or(1.0))]));
        }
    }

    /// An input call of the bench file's run (`journey::op_count`), counted per run for `InputMismatch`;
    /// one in an untimed stretch is not the journey's.
    pub(super) fn op(&self, call: &str, args: &[Value]) {
        if !self.untimed_now() {
            self.ended.set(false);
        }
        if let Some(n) = super::journey::op_count(call, args).filter(|_| self.in_run() && !self.untimed_now()) {
            self.add(&BTreeMap::from([(format!("{}{call}", super::journey::OP), n)]));
        }
    }

    /// Counts from another source of the run (a page's counter global).
    pub(super) fn add(&self, from: &BTreeMap<String, f64>) {
        let mut c = self.counts.borrow_mut();
        for (k, n) in from {
            *c.entry(k.clone()).or_default() += n;
        }
    }

    pub(super) fn counts(&self) -> BTreeMap<String, f64> {
        self.counts.borrow().clone()
    }

    /// `ctx.phase` edge as a mark name, `run:<i>:phase:<name>:start|end` (the name with anything but
    /// letters, digits, `-`, `_` and `.` made `_`); none outside a run. A phase inside an untimed
    /// stretch is a usage error: it would time what the stretch leaves out (a stretch inside a phase
    /// leaves the phase).
    pub(super) fn phase_key(&self, a: &Value) -> Result<Option<String>> {
        let run = self.run.borrow();
        if run.is_empty() {
            return Ok(None);
        }
        let name: String = a["name"].as_str().unwrap_or("phase").chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' }).collect();
        if self.untimed_now() {
            return Err(Error::usage(format!(
                "ctx.phase({name:?}) inside ctx.untimed: a stretch leaves the phases it is in, so a phase in it would time nothing; put the untimed stretch inside the phase"
            )));
        }
        Ok(Some(format!("{run}:phase:{name}:{}", a["edge"].as_str().unwrap_or("start"))))
    }

    /// `ctx.phase` edge marked on the host clock.
    pub(super) fn phase(&self, a: &Value) -> Result<Value> {
        if let Some(key) = self.phase_key(a)? {
            self.marks.borrow_mut().push((key, host::now_ns()));
        }
        Ok(Value::Bool(true))
    }

    pub(super) fn take_marks(&self) -> Vec<(String, u64)> {
        std::mem::take(&mut self.marks.borrow_mut())
    }
}

/// One measured run's counts, host-clock phase marks and timed span.
pub(super) type Hooked<'a> = (&'a BTreeMap<String, f64>, &'a [(String, u64)], &'a metrics::Timed);

/// `count.<name>` and `phase.<name>` per measured run for the series' declared metrics, from each
/// run's counts and host-clock phase marks (`phase_ms`).
pub(super) fn per_run_hooks(s: &Series, key: &dyn Fn(usize) -> String, runs: &[Hooked]) -> BTreeMap<String, BTreeMap<String, f64>> {
    let mut extra = BTreeMap::new();
    // The fixture's CPU per run, whether declared or not: result.json's fixture says it per side. None
    // when a run has none (a process of the fixture ended in it), never a median of the others.
    if runs.iter().any(|(c, _, _)| c.contains_key(FIXTURE_CPU)) && runs.iter().all(|(c, _, _)| c.get(FIXTURE_CPU).is_none_or(|x| x.is_finite())) {
        extra.insert("cpu.fixture".to_string(), runs.iter().enumerate().filter_map(|(j, (c, _, _))| Some((key(j), *c.get(FIXTURE_CPU)?))).collect());
    }
    for m in declared_metrics(s) {
        if let Some(name) = m.strip_prefix("count.") {
            extra.insert(m.clone(), runs.iter().enumerate().map(|(j, (c, _, _))| (key(j), c.get(name).copied().unwrap_or(0.0))).collect());
        } else if let Some(name) = m.strip_prefix("phase.") {
            extra.insert(m.clone(), runs.iter().enumerate().map(|(j, (_, marks, t))| (key(j), super::phase_ms(marks, name, t))).filter(|(_, x)| x.is_finite()).collect());
        }
    }
    extra
}

/// Run totals strobe cannot take a `ctx.untimed` stretch out of (SPEC §9.6): sampled over a window
/// (GPU use), counted by the OS per process without a reading at each edge (energy, instructions,
/// idle wakes), kept by the page per run (requests, canvas frames), or read after the run (the heap
/// it retained). A run that left a stretch out has none of them.
const NOT_UNTIMED: &[&str] = &["gpu.process.", "energy.", "cpu.instructions", "cpu.idle_wakes", "net.", "canvas.", "js.heap.retained"];

/// When a measured run used `ctx.untimed`: the block's `untimed.n` and `untimed.ms`, and the run
/// totals in `NOT_UNTIMED` refused (`Untimed`): a declared one is listed refused, an undeclared one
/// leaves the values.
pub(super) fn untimed_state(s: &Series, runs: &[&metrics::Timed], values: &mut BTreeMap<String, f64>, refused: &mut BTreeMap<String, Refusal>) {
    if runs.iter().all(|r| r.untimed.is_empty()) {
        return;
    }
    super::untimed_values(runs, values);
    let hit = |m: &str| NOT_UNTIMED.iter().any(|p| m.starts_with(p));
    values.retain(|k, _| !hit(k));
    for m in declared_metrics(s).into_iter().filter(|m| hit(m)) {
        refused.entry(m.clone()).or_insert_with(|| {
            Refusal::new(ValidatorId::Untimed, format!("{m}: the runs left stretches out with ctx.untimed, and strobe cannot take a stretch out of this total"))
                .fix("time the case without ctx.untimed for this metric, or wait with until: on the step before")
        });
    }
}
