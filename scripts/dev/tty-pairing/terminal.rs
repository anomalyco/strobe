//! A program on a virtual terminal. Only writes and time enter here; this side knows nothing
//! about SDK calls, pacing, input ledgers or the ownership decision it will be checked against.
use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};
use strobe::driver::tty::Terminal;
use strobe::model::Result;
use strobe::target::tty::{Grid, Modes, Mouse, Update};

pub const MS: u64 = 1_000_000;
pub const START: u64 = 10_000 * MS;

#[derive(Clone, Copy, Debug)]
pub enum Response {
    One,
    Pieces(u64),
    Frame,
    Silent,
    PlainThenFrame,
    BothEdges,
}

pub struct Wire {
    pub at: u64,
    pub bytes: Vec<u8>,
    pub first: Option<u64>,
}

pub struct Program {
    clock: Cell<u64>,
    epoch: Instant,
    pub script: Vec<(u64, Response)>,
    pub wire: RefCell<Vec<Wire>>,
    pub events: RefCell<Vec<Update>>,
    pub frame_hits: RefCell<Vec<(u64, u64)>>,
    // Work already scheduled. Bytes queued during the same job are read together.
    batch: Cell<Option<(u64, u64, Option<u64>)>>,
    pub conpty: bool,
    pub kitty: u16,
    pub echo: bool,
    pub cursor: (u16, u16),
    cells: Vec<vt100::Cell>,
}

pub fn update(start_ns: u64, commit_ns: u64) -> Update {
    Update { start_ns, commit_ns, synced: start_ns != commit_ns, cells: 1 }
}

impl Program {
    pub fn new(delay: u64, shape: Response) -> Self {
        let mut p = vt100::Parser::default();
        p.process(b"Field");
        let cells = (0..24).flat_map(|r| (0..80).map(move |c| (r, c))).map(|(r, c)| p.screen().cell(r, c).unwrap().clone()).collect();
        Self {
            clock: Cell::new(START),
            epoch: Instant::now(),
            script: vec![(delay, shape)],
            wire: Default::default(),
            events: Default::default(),
            frame_hits: Default::default(),
            batch: Cell::new(None),
            conpty: false,
            kitty: 0,
            echo: false,
            cursor: (1, 0),
            cells,
        }
    }

    fn deadline(&self, d: Instant) -> u64 {
        START + (d - self.epoch).as_nanos() as u64
    }
    fn accept(&self, bytes: &[u8]) -> u64 {
        let at = self.now_ns();
        let (delay, shape) = self.script[self.wire.borrow().len().min(self.script.len() - 1)];
        let (start, free, answer) = self.batch.get().unwrap_or((0, 0, None));
        let first = if at <= start {
            answer
        } else {
            let start = at.max(free);
            let t = start + delay;
            let first = (!matches!(shape, Response::Silent)).then_some(t);
            let mut log = self.events.borrow_mut();
            if self.conpty && bytes.is_empty() {
                log.push(update(at + 5 * MS, at + 5 * MS));
            }
            match shape {
                Response::One => log.push(update(t, t)),
                Response::Frame => log.push(update((start + MS).min(t), t)),
                Response::Pieces(gap) => log.extend([update(t, t), update(t + gap, t + gap)]),
                Response::PlainThenFrame => log.extend([update(start + 10 * MS, start + 10 * MS), update(start + 20 * MS, t)]),
                Response::BothEdges => log.extend([update(t, t), update(t + delay, t + delay)]),
                Response::Silent => (),
            }
            let first = if matches!(shape, Response::PlainThenFrame) { Some(start + 10 * MS) } else { first };
            self.batch.set(Some((start, if matches!(shape, Response::BothEdges) { t + delay } else { t }, first)));
            first
        };
        self.wire.borrow_mut().push(Wire { at, bytes: bytes.to_vec(), first });
        self.clock.set(at + 1); // ordered writes, including back-to-back syscalls
        at
    }
}

impl Terminal for Program {
    fn instant(&self) -> Instant {
        self.epoch + Duration::from_nanos(self.now_ns() - START)
    }
    fn now_ns(&self) -> u64 {
        self.clock.get()
    }
    fn sleep(&self, d: Duration) {
        self.clock.set(self.now_ns() + d.as_nanos() as u64);
    }
    fn host_redraw(&self) -> bool {
        self.conpty
    }
    fn size(&self) -> (u16, u16) {
        (80, 24)
    }
    fn resize(&self, _: u16, _: u16) -> Result<()> {
        self.accept(&[]);
        Ok(())
    }
    fn line_echo(&self) -> bool {
        self.echo
    }
    fn modes(&self) -> Modes {
        Modes { mouse: Mouse::Motion, sgr: true, app_cursor: false, kitty: self.kitty, bracketed_paste: true }
    }
    fn write(&self, b: &[u8]) -> Result<u64> {
        Ok(self.accept(b))
    }
    fn updates(&self, from: u64, to: u64) -> Vec<Update> {
        let mut seen: Vec<_> = self.events.borrow().iter().copied().filter(|u| (from..=to.min(self.now_ns())).contains(&u.commit_ns)).collect();
        seen.sort_by_key(|u| u.commit_ns);
        seen
    }
    fn quiet(&self, d: Duration, since: u64, deadline: Instant) -> bool {
        let deadline = self.deadline(deadline);
        let events = self.events.borrow();
        loop {
            let now = self.now_ns();
            let last = events.iter().filter(|u| u.commit_ns <= now).map(|u| u.commit_ns).max().unwrap_or(0).max(since);
            let open = events.iter().any(|u| u.start_ns <= now && now < u.commit_ns);
            let due = last + d.as_nanos() as u64;
            if !open && now >= due {
                return true;
            }
            if now >= deadline {
                return false;
            }
            let event = events.iter().flat_map(|u| [u.start_ns, u.commit_ns]).filter(|t| *t > now).min().unwrap_or(u64::MAX);
            self.clock.set(deadline.min(event).min(if open { u64::MAX } else { due }));
        }
    }
    fn first_change(&self, since: u64, deadline: Instant) -> Option<u64> {
        let deadline = self.deadline(deadline);
        let hit = self.events.borrow().iter().filter(|u| u.start_ns >= since && u.commit_ns <= deadline).map(|u| u.commit_ns).min();
        // The reader stamps the commit before waking the writer. A later write cannot share that stamp.
        self.clock.set(self.now_ns().max(hit.map_or(deadline, |t| t + 1)));
        if let Some(t) = hit {
            self.frame_hits.borrow_mut().push((since, t));
        }
        hit
    }
    fn grid<R>(&self, f: impl FnOnce(&Grid) -> R) -> R {
        f(&Grid { cells: &self.cells, rows: 24, cols: 80, cursor: self.cursor, cursor_on: true, title: "" })
    }
    fn wait(&self, deadline: Instant, mut f: impl FnMut(&Grid, u64, usize) -> bool) -> bool {
        if self.grid(|g| f(g, START, 0)) {
            true
        } else {
            self.clock.set(self.deadline(deadline));
            false
        }
    }
}
