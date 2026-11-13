//! Release and ready can both be reported after the event (Linux's pre-exec pipe, the terminal's
//! first write, a frame's presentation time). Read cumulative counters across the launch; use only
//! the two reported edges, not the time spent setting up the child or finding out it was ready.
use super::*;
use std::sync::mpsc;

pub(super) struct LaunchLoad {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<Vec<Point>>>,
    cpus: u64,
}

struct Point {
    ns: u64,
    clock: Option<u64>,
    idle: Vec<u64>,
    steal: Vec<u64>,
    // Observer on shared CPUs, and the separate console host when present. Neither is app cpu.work.
    other_own: Option<u64>,
}

impl Point {
    fn read(ours: &[u32]) -> Self {
        let before = host::now_ns();
        let (mut idle, mut steal) = (Vec::new(), Vec::new());
        if !host::idle_cycles(&mut idle) {
            idle.clear()
        }
        if !host::steal_ns(&mut steal) {
            steal.clear()
        }
        let clock = host::tsc();
        let other_own = ours.iter().map(|p| host::process_cycles(*p)).sum();
        Self { ns: before.midpoint(host::now_ns()), clock, idle, steal, other_own }
    }
}

impl LaunchLoad {
    pub(super) fn start(console: &[u32]) -> Option<Self> {
        let (cpus, unpinned, runner) = RUN_LOADS.with_borrow(|l| l.as_ref().map(|l| (l.cpus, l.pin.is_none(), l.runner)))?;
        let mut ours = console.to_vec();
        if unpinned {
            ours.extend([std::process::id(), runner]);
        }
        let first = Point::read(&ours);
        let (stop, rx) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let mut readings = vec![first];
            loop {
                let done = !matches!(rx.recv_timeout(Duration::from_millis(5)), Err(mpsc::RecvTimeoutError::Timeout));
                readings.push(Point::read(&ours));
                if done {
                    return readings;
                }
            }
        });
        Some(Self { stop: Some(stop), thread: Some(thread), cpus })
    }

    pub(super) fn window(mut self, from: u64, until: u64) -> RunLoad {
        self.stop.take();
        let points = self.thread.take().unwrap().join().unwrap_or_default();
        window(&points, self.cpus, from, until).unwrap_or_default()
    }
}

impl Drop for LaunchLoad {
    fn drop(&mut self) {
        self.stop.take();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn window(p: &[Point], mask: u64, from: u64, until: u64) -> Option<RunLoad> {
    let ns = until.checked_sub(from).filter(|v| *v > 0)?;
    let bracket = |edge| {
        let i = p.partition_point(|v| v.ns < edge);
        let (a, b) = (p.get(i.checked_sub(1)?)?, p.get(i)?);
        // A short launch or a sampler that was not scheduled: don't treat an extrapolated
        // reading from before release or after ready as evidence about the launch.
        let span = b.ns.checked_sub(a.ns).filter(|v| *v > 0 && *v <= ns)?;
        Some((a, b, (edge - a.ns) as f64 / span as f64))
    };
    let ((a, b, x), (c, d, y)) = (bracket(from)?, bracket(until)?);
    let delta = |f: &dyn Fn(&Point) -> Option<u64>| {
        let (a, b, c, d) = (f(a)?, f(b)?, f(c)?, f(d)?);
        Some(c.checked_sub(a)? as f64 + y * d.checked_sub(c)? as f64 - x * b.checked_sub(a)? as f64)
    };
    let clock = delta(&|p| p.clock).filter(|v| *v > 0.0)?;
    let ms = ns as f64 / 1e6;
    let cpus = (0..64).filter(|i| mask >> i & 1 == 1);
    let busy: Option<f64> = cpus.clone().map(|i| Some((1.0 - delta(&|p| p.idle.get(i).copied())? / clock).clamp(0.0, 1.0) * ms)).sum();
    let n = mask.count_ones() as f64;
    let ticks = if cfg!(target_os = "linux") { n * host::IDLE_TICK_NS as f64 / 1e6 } else { 0.0 };
    let steal: Option<f64> = cpus.map(|i| delta(&|p| p.steal.get(i).copied())).sum();
    Some(RunLoad { ms, busy_ms: busy.map(|v| (v - ticks).max(0.0)), stolen_ms: steal.filter(|_| n > 0.0).map(|v| v / 1e6 / n), observer_ms: ms * delta(&|p| p.other_own)? / clock, own_ms: None })
}

#[cfg(feature = "dev-background")]
pub(super) fn known_window() {
    // Only the middle 100 ms is release..ready. The CPU was idle during setup and observation
    // afterwards; of its 100 ms busy inside the window, the target used 20 and the host 5.
    let sample = |ms: u64, idle: u64, console: u64| Point { ns: ms * 1_000_000, clock: Some(ms * 1_000_000), idle: vec![idle * 1_000_000], steal: vec![0], other_own: Some(console * 1_000_000) };
    let pts = [sample(0, 0, 0), sample(100, 100, 0), sample(150, 100, 2), sample(200, 100, 5), sample(240, 140, 5)];
    let mut load = window(&pts, 1, 100_000_000, 200_000_000).unwrap();
    load.own_ms = Some(20.0);
    let expected = 75.0 - if cfg!(target_os = "linux") { 10.0 } else { 0.0 };
    assert_eq!(load.foreign(), Some(expected));
    assert!(window(&pts, 1, 100_000_000, 100_000_000).is_none());
    println!("launch release..ready: 100 ms, console host billed once; {expected}% foreign after tick allowance");
}
