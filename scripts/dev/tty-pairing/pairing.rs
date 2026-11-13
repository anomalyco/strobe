// Dev check of the terminal latency pairing (dev only, not shipped): feeds `driver::tty::pair` and
// `driver::tty::ambiguous` made-up sequences of inputs and grid updates (host-clock times) and says
// what strobe prints for each: a latency, a refusal and its reason, or no latency. Live sleeps cannot
// hold an edge to the microsecond; these sequences do.
//   cargo run --release --manifest-path scripts/dev/tty-pairing/Cargo.toml [-- rows|table]
//
//   rows   (the default) the boundary rows: the 50 ms and 1 s edges exactly, and each sequence that
//          printed a wrong number or refused an honest bench before; each row states its expected
//          outcome, and any mismatch exits 1
//   table  a simulated terminal program driven as strobe's driver paces each input kind and bench
//          pattern, for answer delays around 50 ms, 300 ms and 1 s and four answer shapes; each cell is
//          what strobe prints against the program's own answer times (`WRONG` where it would print a
//          latency that is not the truth; any WRONG cell exits 1)

use strobe::driver::tty::{self as drv, Input, Latencies};
use strobe::metrics::{Timed, percentile};
use strobe::target::tty::Update;

const MS: f64 = 1e6;
/// Every time is this far after the host clock's zero (ns), so a made-up time before 0 ms is no wrap.
const BASE: u64 = 10_000_000_000;

fn at(ms: f64) -> u64 {
    (BASE as f64 + ms * MS).round() as u64
}

fn ms_of(ns: u64) -> f64 {
    (ns as f64 - BASE as f64) / MS
}

fn input(what: &'static str, sent: f64, end: f64, sample: bool, rested: bool, action: bool) -> Input {
    Input { sent_ns: at(sent), end_ns: at(end), what, sample, rested, action, burst: false, echoed: false }
}

fn key(t: f64) -> Input {
    input("key", t, t, true, true, true)
}

/// A plain update: one read that changed cells.
fn change(t: f64) -> Update {
    Update { start_ns: at(t), commit_ns: at(t), cells: 40, synced: false }
}

/// A synchronized frame from its begin to its end.
fn frame(start: f64, end: f64) -> Update {
    Update { start_ns: at(start), commit_ns: at(end), cells: 40, synced: true }
}

/// What strobe prints: `refused <reason>`, `no latency (…)`, or the p50.
fn describe(lat: &Latencies) -> String {
    if drv::ambiguous(lat).is_some() {
        let why = if lat.burst > 0 {
            "burst"
        } else if lat.echoed > 0 {
            "echo"
        } else if lat.busy * 2 > lat.ms.len() + lat.busy {
            "busy"
        } else {
            "overrun"
        };
        return format!("refused {why}");
    }
    if lat.ms.is_empty() {
        return format!("no latency ({} silent, {} busy)", lat.silent, lat.busy);
    }
    let mut ms = lat.ms.clone();
    ms.sort_by(f64::total_cmp);
    format!("{:.2} ms", percentile(&ms, 0.5))
}

fn print_of(inputs: &[Input], updates: &[Update], t0: f64, t1: f64) -> (String, Latencies) {
    let s = Timed { t0: at(t0), t1: at(t1), untimed: vec![] };
    let lat = drv::pair(inputs, updates, &[&s]);
    (describe(&lat), lat)
}

struct Row {
    name: &'static str,
    inputs: Vec<Input>,
    updates: Vec<Update>,
    /// The measured stretch.
    stretch: (f64, f64),
    expect: &'static str,
}

fn rows() -> Vec<Row> {
    let mut r = Vec::new();
    let mut row = |name, inputs, updates, stretch, expect| r.push(Row { name, inputs, updates, stretch, expect });
    // The 50 ms rest and the late answer: a key at exactly 50.00 ms after the one before, whose answer
    // comes 0.17 ms after it (Linux's 0.17 ms read).
    row("next key at 50.00 ms, late frame at 50.17 ms", vec![key(0.0), key(50.0)], vec![change(50.17), change(100.17)], (-10.0, 400.0), "refused overrun");
    // The late frame just before the next key: the first key's latency is its own; the second went out
    // 0.01 ms after a change (busy, a minority).
    row("late frame at 49.99 ms, next key at 50.00 ms", vec![key(0.0), key(50.0)], vec![change(49.99), change(99.99)], (-10.0, 400.0), "49.99 ms");
    // Stillness before a key: a change 49.99 ms before it is drawing (busy); 50.00 ms before is rest.
    row("last change 49.99 ms before the key", vec![key(0.0)], vec![change(-49.99), change(20.0)], (-1.0, 400.0), "refused busy");
    row("last change 50.00 ms before the key", vec![key(0.0)], vec![change(-50.0), change(20.0)], (-1.0, 400.0), "20.00 ms");
    // The 1 s window of an unanswered key.
    row("silent key, next key 999.99 ms later", vec![key(0.0), key(999.99)], vec![change(1019.99)], (-1.0, 2000.0), "refused overrun");
    row("silent key, next key 1000.00 ms later", vec![key(0.0), key(1000.0)], vec![change(1020.0)], (-1.0, 2000.0), "20.00 ms");
    // A1: a click is press and release back to back: a press-acting program slower than the old 50 ms
    // hold reads its own time (it read 5 ms), and one faster still reads its own.
    row("A1 click, press-acting, 55 ms", vec![input("click", 0.0, 0.01, true, true, true)], vec![change(55.0)], (-1.0, 400.0), "55.00 ms");
    row("A1 click, press-acting, 45 ms", vec![input("click", 0.0, 0.01, true, true, true)], vec![change(45.0)], (-1.0, 400.0), "45.00 ms");
    // A2: typing whose program stopped resting after 3 keys: the 7 keys sent unpaced are samples
    // (busy), not left out: 6 of 9 tried, so the block refuses.
    {
        let mut ins = vec![key(0.0), key(70.0), key(140.0)];
        ins.push(input("key", 230.0, 230.0, true, false, true));
        for k in 0..6 {
            ins.push(input("key", 231.0 + k as f64, 231.0 + k as f64, true, false, true));
        }
        let mut ups = vec![change(20.0), change(90.0), change(160.0)];
        ups.extend((0..15).map(|k| change(170.0 + 30.0 * k as f64)));
        row("A2 typing that fell back to unpaced", ins, ups, (-1.0, 1000.0), "refused busy");
    }
    // A3: the first key's answer is a synchronized frame still open when the second goes out: it holds
    // the second key's work.
    row("A3 open synchronized frame across the next key", vec![key(0.0), key(50.0)], vec![frame(40.0, 260.0)], (-1.0, 600.0), "refused busy");
    // A4: a drag (down at 0, release at 1000) whose drop is drawn 80 ms after the release, a key sent
    // 50 ms after the release: the drop lands after the key.
    // A drag as the driver records it: a write every 50 ms from 0 to 950 ms, each answered 16 ms later,
    // the release with the last move (back to back) at 950 ms.
    let drag = |then: Vec<Input>| -> Vec<Input> {
        let mut v: Vec<Input> = (0..20).map(|k| 50.0 * k as f64).map(|t| input("drag", t, if t == 950.0 { 950.01 } else { t }, false, false, true)).collect();
        v.extend(then);
        v
    };
    let moves = |extra: &[f64]| -> Vec<Update> {
        let mut u: Vec<Update> = (0..19).map(|k| change(16.0 + 50.0 * k as f64)).collect();
        u.extend(extra.iter().map(|t| change(*t)));
        u
    };
    // Its release sets off a drop drawn 80 ms after it; a key 50 ms after the release (its rest) would
    // take the drop for its own answer (it read 30 ms for 200).
    row("A4 drag with a late drop, key 50 ms after", drag(vec![key(1000.0)]), moves(&[1030.0, 1200.0]), (-1.0, 2000.0), "refused overrun");
    row("A4 drag with a late drop, wait(1000), key", drag(vec![key(1951.0)]), moves(&[1030.0, 2151.0]), (-1.0, 3000.0), "200.00 ms");
    // A scrollbar drag: its moves drew, its release draws nothing. After wait(1000) the key reads its own
    // time; after until: "idle" (300 ms) the release may still owe a drop, so the block refuses.
    row("A4 scrollbar drag, wait(1000), key", drag(vec![key(1951.0)]), moves(&[1971.0]), (-1.0, 3000.0), "20.00 ms");
    row("A4 scrollbar drag, until idle, key", drag(vec![key(1250.0)]), moves(&[1270.0]), (-1.0, 3000.0), "refused overrun");
    // Moves sent faster than the program answers them (16 ms apart, 299 ms each): its answers come
    // after the drag ends, the first of them before the next key, the rest after it (it read 189 ms for
    // 488 ms): a mix-up inside the drag refuses the key after it within a second.
    {
        let mut ins: Vec<Input> = (0..=12).map(|k| 16.0 * k as f64).map(|t| input("drag", t, if t == 192.0 { t + 0.01 } else { t }, false, false, true)).collect();
        ins.push(key(400.0));
        row("A4 drag outrunning a 299 ms program, wait(200), key", ins, vec![change(299.0), change(598.0), change(897.0)], (-1.0, 2000.0), "refused overrun");
    }
    // A4: a button pressed (down, its own answer) and let go (up, a late drop), a key after it.
    row(
        "A4 down then up with a late drop, key",
        vec![input("mouse", 0.0, 0.0, true, true, true), input("mouse", 100.0, 100.0, false, false, true), key(150.0)],
        vec![change(10.0), change(180.0), change(350.0)],
        (-1.0, 1000.0),
        "refused overrun",
    );
    // B6: a focusing click that changes nothing (the field is focused already, or focus is a cursor
    // move) is no action: the key after it reads its own time.
    row("B6 focusing click that changes nothing, key", vec![input("click", 0.0, 0.01, false, true, false), key(300.0)], vec![change(320.0)], (-1.0, 1000.0), "20.00 ms");
    // B7: inputs before the run (setup, a reset) that owe nothing let the run's keys pair at once.
    row("B7 setup answered, run key", vec![key(0.0), key(200.0)], vec![change(20.0), change(220.0)], (150.0, 1000.0), "20.00 ms");
    // B7: a burst in setup (answered together at 20 ms) mixes answers up; the run's key reads its own
    // time from 1 s after the burst's last key on (which `Driver::quiesce` waits for), and is refused
    // before.
    {
        let burst = |t: f64| Input { burst: true, sample: false, ..key(t) };
        let setup = vec![burst(0.0), burst(1.0), burst(2.0)];
        let mut early = setup.clone();
        early.push(key(1001.99));
        row("B7 burst in setup, run key at 1001.99 ms", early, vec![change(20.0), change(1021.99)], (1000.0, 2000.0), "refused overrun");
        let mut late = setup.clone();
        late.push(key(1002.0));
        row("B7 burst in setup, run key at 1002.00 ms", late, vec![change(20.0), change(1022.0)], (1000.0, 2000.0), "20.00 ms");
    }
    // B7: a key in a reset that changes nothing (Escape at rest): its owed answer ends 1 s after it.
    row("B7 no-op key in reset, run key 1 s after", vec![key(0.0), key(1000.0)], vec![change(1020.0)], (999.0, 2000.0), "20.00 ms");
    r
}

/// `Driver::quiesce`'s wait: until when the next action must wait, as ms (None: at once).
fn owed(inputs: &[Input], updates: &[Update]) -> Option<f64> {
    drv::owed_until(inputs, updates).map(ms_of)
}

fn check_rows() -> bool {
    let mut ok = true;
    for r in rows() {
        let (got, lat) = print_of(&r.inputs, &r.updates, r.stretch.0, r.stretch.1);
        let pass = got.starts_with(r.expect);
        ok &= pass;
        println!(
            "{} {:<52} {:<34} expected {:<16} (samples {}, busy {}, silent {}, overran {}+{})",
            if pass { "ok  " } else { "FAIL" },
            r.name,
            got,
            r.expect,
            lat.ms.len(),
            lat.busy,
            lat.silent,
            lat.overran,
            lat.after_overrun
        );
    }
    // What `quiesce` waits for after setup: nothing when setup owes nothing.
    let quiesce = [
        ("quiesce after an answered setup", vec![key(0.0)], vec![change(20.0)], None),
        (
            "quiesce after a burst in setup",
            vec![Input { burst: true, sample: false, ..key(0.0) }, Input { burst: true, sample: false, ..key(1.0) }, Input { burst: true, sample: false, ..key(2.0) }],
            vec![change(20.0)],
            Some(1002.0),
        ),
        ("quiesce after a no-op key", vec![key(0.0)], vec![], Some(1000.0)),
        ("quiesce after a drag whose release drew nothing", vec![input("drag", 0.0, 0.0, false, false, true), input("drag", 50.0, 50.01, false, false, true)], vec![change(16.0)], Some(1050.01)),
    ];
    for (name, ins, ups, expect) in quiesce {
        let got = owed(&ins, &ups);
        let pass = match (got, expect) {
            (None, None) => true,
            (Some(g), Some(e)) => (g - e).abs() < 0.001,
            _ => false,
        };
        ok &= pass;
        println!("{} {:<52} waits until {:<26} expected {:?}", if pass { "ok  " } else { "FAIL" }, name, format!("{got:?}"), expect);
    }
    ok
}

// ---- The table: a simulated program and strobe's pacing of each input kind ----

#[derive(Clone, Copy, PartialEq)]
enum Shape {
    /// One write, `d` after the program read the input.
    One,
    /// Two writes, at `d` and this many ms later (an answer in pieces).
    Pieces(f64),
    /// One synchronized frame from 1 ms after the read to `d`.
    Sync,
    /// No cell change at all (a cursor move only, or no answer).
    Nothing,
}

/// The program: single-threaded, it reads every write queued when it is free, works `d` ms on them
/// together and answers once. Returns its updates and each write's answer: the first and last update
/// of the job that read it.
fn program(writes: &[f64], d: f64, shape: Shape) -> (Vec<Update>, Vec<Option<(f64, f64)>>) {
    let (mut ups, mut answer) = (Vec::new(), vec![None; writes.len()]);
    let (mut free, mut i) = (f64::MIN, 0);
    while i < writes.len() {
        let start = writes[i].max(free);
        let mut j = i;
        while j < writes.len() && writes[j] <= start {
            j += 1;
        }
        let done = start + d;
        let first = match shape {
            Shape::One => {
                ups.push(change(done));
                Some((done, done))
            }
            Shape::Pieces(gap) => {
                ups.push(change(done));
                ups.push(change(done + gap));
                Some((done, done + gap))
            }
            Shape::Sync => {
                ups.push(frame((start + 1.0).min(done), done));
                Some((done, done))
            }
            Shape::Nothing => None,
        };
        for a in answer.iter_mut().take(j).skip(i) {
            *a = first;
        }
        free = done;
        i = j;
    }
    ups.sort_by_key(|u| u.commit_ns);
    (ups, answer)
}

struct Sim {
    d: f64,
    shape: Shape,
    now: f64,
    writes: Vec<f64>,
    inputs: Vec<Input>,
    /// The program's updates for the first `.0` writes.
    cache: std::cell::RefCell<(usize, Vec<Update>)>,
}

impl Sim {
    fn ups(&self) -> Vec<Update> {
        let mut c = self.cache.borrow_mut();
        if c.0 != self.writes.len() {
            *c = (self.writes.len(), program(&self.writes, self.d, self.shape).0);
        }
        c.1.clone()
    }

    fn last_end(&self) -> f64 {
        self.inputs.iter().map(|i| ms_of(i.end_ns)).fold(f64::MIN, f64::max)
    }

    /// `Tty::quiet`: the first time from now the grid has held still `q` since its last change and
    /// `since`, with no synchronized frame open, at most `cap` later.
    fn quiet(&self, q: f64, since: f64, cap: f64) -> Option<f64> {
        let ups = self.ups();
        let mut x = self.now.max(since + q);
        loop {
            if x > self.now + cap {
                return None;
            }
            if let Some(f) = ups.iter().filter(|u| u.synced && ms_of(u.start_ns) <= x && x < ms_of(u.commit_ns)).map(|u| ms_of(u.commit_ns)).reduce(f64::max) {
                x = f;
                continue;
            }
            let last = ups.iter().map(|u| ms_of(u.commit_ns)).filter(|c| *c <= x).reduce(f64::max);
            match last {
                // In whole nanoseconds, as the host clock: `c + q` read back in ms may fall short of q.
                Some(c) if at(x) - at(c) < (q * MS) as u64 => x = ms_of(at(c) + (q * MS) as u64),
                _ => return Some(x),
            }
        }
    }

    /// `Driver::settle`: rest before a measured input (50 ms, at most 300 ms): the send time and rested.
    fn settle(&mut self) -> bool {
        let since = self.last_end();
        match self.quiet(50.0, since, 300.0) {
            Some(x) => {
                self.now = x;
                true
            }
            None => {
                self.now += 300.0;
                false
            }
        }
    }

    /// `Tty::first_change`: the commit of the first update begun at or after `after`, by `cap` from now.
    fn first_change(&self, after: f64, cap: f64) -> Option<f64> {
        self.ups().iter().filter(|u| ms_of(u.start_ns) >= after).map(|u| ms_of(u.commit_ns)).reduce(f64::min).filter(|c| *c <= self.now + cap)
    }

    fn write(&mut self, t: f64) {
        self.writes.push(t);
    }

    fn press(&mut self) {
        let rested = self.settle();
        self.write(self.now);
        self.inputs.push(input("key", self.now, self.now, true, rested, true));
    }

    /// A pattern after a call; false when the bench itself fails (`until: "frame"` with no change).
    fn after(&mut self, p: &str) -> bool {
        let last = self.last_end();
        match p {
            "back-to-back" => true,
            "until-frame" => match self.first_change(last, 30_000.0) {
                Some(c) => {
                    self.now = self.now.max(c);
                    true
                }
                None => false,
            },
            "until-idle" => match self.quiet(300.0, last, 30_000.0) {
                Some(x) => {
                    self.now = x;
                    true
                }
                None => false,
            },
            "wait-200" => {
                self.now += 200.0;
                true
            }
            _ => {
                self.now += 1000.0;
                true
            }
        }
    }

    fn step(&mut self, kind: &str, p: &str) -> bool {
        match kind {
            "press" | "paste" => {
                self.press();
                self.after(p)
            }
            "click" => {
                let rested = self.settle();
                self.write(self.now);
                self.write(self.now + 0.01);
                self.inputs.push(input("click", self.now, self.now + 0.01, true, rested, true));
                self.now += 0.01;
                self.after(p)
            }
            "wheel" => {
                let rested = self.settle();
                for k in 0..3 {
                    self.write(self.now + 0.01 * k as f64);
                }
                self.inputs.push(input("wheel", self.now, self.now + 0.02, true, rested, true));
                self.now += 0.02;
                self.after(p)
            }
            "scroll" => {
                let rested = self.settle();
                let x = self.now;
                for k in 0..3 {
                    let w = x + 50.0 * k as f64;
                    self.write(w);
                    self.inputs.push(input("wheel", w, w, k == 0, rested && k == 0, true));
                }
                self.now = x + 100.0;
                self.after(p)
            }
            "drag-press" => {
                // Down, a move every 16 ms, the release right after the last move (one input with it).
                let x = self.now;
                for k in 0..=12 {
                    let w = x + 16.0 * k as f64;
                    self.write(w);
                    self.inputs.push(input("drag", w, if k == 12 { w + 0.01 } else { w }, false, false, true));
                }
                self.write(x + 192.01);
                self.now = x + 192.01;
                if !self.after(p) {
                    return false;
                }
                self.press();
                self.after(p)
            }
            "down-up" => {
                let rested = self.settle();
                self.write(self.now);
                self.inputs.push(input("mouse", self.now, self.now, true, rested, true));
                if !self.after(p) {
                    return false;
                }
                self.now += 0.01;
                self.write(self.now);
                self.inputs.push(input("mouse", self.now, self.now, false, false, true));
                self.after(p)
            }
            _ => unreachable!(),
        }
    }

    /// `page.type` of 10 keys: each paced, and waited on for its change up to 1 s; unpaced (still a
    /// sample) once the grid never rested.
    fn type_keys(&mut self) {
        let mut paced = true;
        for _ in 0..10 {
            let rested = paced && self.settle();
            self.write(self.now);
            self.inputs.push(input("key", self.now, self.now, true, rested, true));
            if paced {
                let sent = self.now;
                self.now = self.first_change(sent, 1000.0).map_or(sent + 1000.0, |c| c.max(sent));
            }
            paced &= rested;
        }
    }
}

/// One cell: 10 calls of `kind` with `p` after each in one run, the run's end awaiting the last
/// answer, against the program's truth.
fn cell(kind: &str, p: &str, d: f64, shape: Shape) -> String {
    let mut s = Sim { d, shape, now: 0.0, writes: vec![], inputs: vec![], cache: Default::default() };
    if kind == "type" {
        s.type_keys();
    } else {
        for _ in 0..10 {
            if !s.step(kind, p) {
                return "bench error (until frame timed out)".into();
            }
        }
    }
    // The run's end: its last input's first change and then rest, each within 1 s.
    let last = s.last_end();
    if s.first_change(last, 1000.0).is_some()
        && let Some(x) = s.quiet(50.0, last, 1000.0)
    {
        s.now = s.now.max(x);
    }
    let (ups, answer) = program(&s.writes, d, shape);
    let lat = drv::pair(&s.inputs, &ups, &[&Timed { t0: at(-1.0), t1: at(s.now), untimed: vec![] }]);
    let got = describe(&lat);
    // The truth: each sample's own answer, from its first write.
    let mut truth: Vec<f64> = s.inputs.iter().filter(|i| i.sample).filter_map(|i| s.writes.iter().position(|w| at(*w) == i.sent_ns).and_then(|k| answer[k]).map(|a| a.0 - ms_of(i.sent_ns))).collect();
    truth.sort_by(f64::total_cmp);
    if got.starts_with("refused") {
        return got;
    }
    if lat.ms.is_empty() {
        return if truth.is_empty() { "silent (no answer)".into() } else { format!("no latency, refused NoInput ({} silent, {} busy)", lat.silent, lat.busy) };
    }
    let mut ms = lat.ms.clone();
    ms.sort_by(f64::total_cmp);
    let (p50, t50) = (percentile(&ms, 0.5), if truth.is_empty() { f64::NAN } else { percentile(&truth, 0.5) });
    if (p50 - t50).abs() <= 1.0 {
        return format!("right ({p50:.1} ms)");
    }
    // Outside the model strobe pairs by (SPEC §13.9): some write waited over 1 s for its answer (an
    // answer slower than `ANSWER_MAX`, or one queued behind others), or one input's answer came in
    // parts more than 50 ms apart (two writes of one answer, or the events of one input, a click's
    // press and release, answered one after the other).
    let over = s.writes.iter().zip(&answer).any(|(w, a)| a.is_some_and(|a| a.0 - w > 1000.0));
    let parts = s.inputs.iter().any(|i| {
        let mut ts: Vec<f64> = s.writes.iter().zip(&answer).filter(|(w, _)| at(**w) >= i.sent_ns && at(**w) <= i.end_ns).filter_map(|(_, a)| *a).flat_map(|a| [a.0, a.1]).collect();
        ts.sort_by(f64::total_cmp);
        ts.windows(2).any(|p| p[1] - p[0] > 50.0)
    });
    match (over, parts) {
        (true, _) => format!("outside: an answer over 1 s ({p50:.1} ms for {t50:.1} ms)"),
        (_, true) => format!("outside: an answer in parts over 50 ms apart ({p50:.1} ms for {t50:.1} ms)"),
        _ => format!("WRONG ({p50:.1} ms for {t50:.1} ms)"),
    }
}

fn table() -> bool {
    let delays = [20.0, 49.0, 51.0, 120.0, 299.0, 301.0, 999.0, 1001.0];
    let shapes = [
        (Shape::One, "one write"),
        (Shape::Pieces(30.0), "two writes 30 ms apart"),
        (Shape::Pieces(70.0), "two writes 70 ms apart"),
        (Shape::Sync, "synchronized frame"),
        (Shape::Nothing, "no cell change"),
    ];
    let kinds = ["press", "paste", "click", "wheel", "scroll", "drag-press", "down-up", "type"];
    let patterns = ["back-to-back", "until-frame", "until-idle", "wait-200", "wait-1000"];
    let mut wrong = 0;
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    for kind in kinds {
        for p in if kind == "type" { &patterns[..1] } else { &patterns[..] } {
            for (shape, sname) in shapes {
                let cells: Vec<String> = delays.iter().map(|d| cell(kind, p, *d, shape)).collect();
                for c in &cells {
                    let k = c.split(" (").next().unwrap_or(c).to_string();
                    *counts.entry(k).or_default() += 1;
                    wrong += c.starts_with("WRONG") as usize;
                }
                let line: Vec<String> = delays.iter().zip(&cells).map(|(d, c)| format!("{d:>4} ms: {c}")).collect();
                println!("{kind:<11} | {:<12} | {sname:<22} | {}", if kind == "type" { "paced type" } else { p }, line.join(" | "));
            }
        }
    }
    println!();
    for (k, n) in &counts {
        println!("{n:>4}  {k}");
    }
    println!("{wrong} wrong");
    wrong == 0
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let ok = match args.get(1).map(String::as_str) {
        Some("table") => table(),
        // One cell: `cell <kind> <pattern> <delay ms> <one|pieces30|pieces70|sync|nothing>`.
        Some("cell") => {
            let shape = match args.get(5).map(String::as_str) {
                Some("pieces30") => Shape::Pieces(30.0),
                Some("pieces70") => Shape::Pieces(70.0),
                Some("sync") => Shape::Sync,
                Some("nothing") => Shape::Nothing,
                _ => Shape::One,
            };
            println!("{}", cell(&args[2], &args[3], args[4].parse().unwrap_or(20.0), shape));
            true
        }
        _ => check_rows(),
    };
    std::process::exit(if ok { 0 } else { 1 });
}
