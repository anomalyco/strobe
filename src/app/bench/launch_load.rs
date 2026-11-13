//! Event-edge load accounting. No sampling thread: read once at the target's spawn (or the
//! navigation send), once when ready is observed next to the target CPU reading.
use super::*;

#[derive(Clone)]
pub(super) struct LaunchLoad {
    at: LoadStart,
    others: Vec<(u32, Option<u64>)>,
    clock: Option<u64>,
}

impl LaunchLoad {
    pub(super) fn start(console: &[u32]) -> Option<Self> {
        RUN_LOADS.with_borrow(|l| {
            let l = l.as_ref()?;
            let shared = [std::process::id(), l.runner];
            let others = console.iter().chain(shared.iter().filter(|_| l.shared)).map(|p| (*p, host::process_cycles(*p))).collect();
            Some(Self { others, at: LoadStart::new(l), clock: host::tsc() })
        })
    }
    pub(super) fn stop(&self) -> RunLoad {
        let mut r = self.at.stop();
        let cycles: Option<u64> = self.others.iter().map(|(p, a)| host::process_cycles(*p)?.checked_sub((*a)?)).sum();
        match cycles.zip(self.clock.zip(host::tsc()).and_then(|(a, b)| b.checked_sub(a)).filter(|d| *d > 0)) {
            Some((used, span)) => r.observer_ms = r.ms * used as f64 / span as f64,
            None => r.busy_ms = None,
        }
        r
    }
    /// Arm only the real launch. The target's spawn helper invokes this one-shot hook immediately
    /// before spawning, after preparing its home and command. Failures cannot leak it to another launch.
    pub(super) fn observe<T>(measured: bool, spawn: impl FnOnce() -> T) -> (T, Option<Self>) {
        if measured_launch(measured, || RUN_LOADS.with_borrow(|l| l.as_ref().map(|_| ()))).is_none() {
            return (spawn(), None);
        }
        let start = std::rc::Rc::new(RefCell::new(None));
        let result = start.clone();
        let target = host::spawn::observe_release(move || *start.borrow_mut() = Self::start(&[]), spawn);
        (target, result.take())
    }
}
