//! Manual known-answer check. `cargo run --release --manifest-path scripts/dev/tty-pairing/Cargo.toml`
//! exercises the regression rows; `-- table` prints the full delay grid. Only the program, terminal
//! and clock are simulated. Input bytes, sleeps, action stamps, waits and attribution are strobe's.
mod terminal;
use Response::*;
use serde_json::{Value, json};
use strobe::driver::tty::{self as tty, Driver, Terminal};
use strobe::metrics::Timed;
use strobe::model::Result;
use terminal::{MS, Program, Response, START, update};

type Page<'a> = Driver<'a, Program>;
fn call(page: &Page, name: &str, args: Value) -> Result<()> {
    page.dispatch(name, args.as_array().unwrap(), None).map(drop)
}
fn press(page: &Page) -> Result<()> {
    call(page, "press", json!(["x"]))
}
fn step(page: &Page, kind: &str, pattern: &str) -> Result<()> {
    let opts = match pattern {
        "until-frame" => json!({"until":"frame"}),
        "until-idle" => json!({"until":"idle"}),
        _ => json!({}),
    };
    let act = |name: &str, args: Value| -> Result<()> {
        call(page, name, args)?;
        match pattern {
            "wait-200" => call(page, "wait", json!([200])),
            "wait-1000" => call(page, "wait", json!([1000])),
            _ => Ok(()),
        }
    };
    match kind {
        "press" => act("press", json!(["x", opts])),
        "paste" => {
            let mut o = opts;
            o["paste"] = true.into();
            act("type", json!(["", "A\nB", o]))
        }
        "click" => act("click", json!([[0, 0], opts])),
        "wheel" => act("wheel", json!([[0,0],{"dy":3},opts])),
        "scroll" => act("scroll", json!([[0,0],{"by":3},opts])),
        "drag-press" => {
            let mut o = opts.clone();
            o["duration"] = 200.into();
            act("drag", json!([[0, 0], [[0, 12]], o]))?;
            act("press", json!(["x", opts]))
        }
        "down-up" => {
            let mut o = opts.clone();
            o["at"] = json!([0, 0]);
            act("down", json!(["Mouse", o]))?;
            act("up", json!(["Mouse", opts]))
        }
        "type" => act("type", json!(["", "xxxxxxxxxx", opts])),
        _ => unreachable!(),
    }
}

fn end(page: &Page, p: &Program) -> u64 {
    let end = p.now_ns();
    match page.last_input(START, &[]) {
        Some((s, false)) if page.await_answer(s) => p.now_ns(),
        Some((s, true)) if page.await_quiet(s) => p.now_ns(),
        _ => end,
    }
}
fn result(page: &Page, p: &Program, t: Timed) -> (tty::Latencies, Vec<(u64, u64)>) {
    let mut samples = vec![];
    let lat = tty::pair_observing(&page.inputs(), &p.updates(0, u64::MAX), &[&t], |i, u| samples.push((i.sent_ns, u.commit_ns)));
    (lat, samples)
}
fn window(t1: u64) -> Timed {
    Timed { t0: START, t1, untimed: vec![] }
}

fn cell(kind: &str, pattern: &str, d: u64, shape: Response) -> &'static str {
    let p = Program::new(d, shape);
    let page = Driver::new(&p, None);
    for _ in 0..if kind == "type" { 1 } else { 10 } {
        if let Err(e) = step(&page, kind, pattern) {
            assert!(e.message.contains("grid did not change"), "{e}");
            return "err";
        }
    }
    let (lat, samples) = result(&page, &p, window(end(&page, &p)));
    if let Some((why, _)) = tty::ambiguous(&lat) {
        return if why.contains("drawing") { "R·busy" } else { "R·over" };
    }
    if samples.is_empty() {
        return if matches!(shape, Silent) { "silent" } else { "NoInput" };
    }
    let log = p.wire.borrow();
    let mut verdict = "ok";
    let mut shifted = std::collections::BTreeMap::new();
    // A release can wait for a frame without contributing a latency sample. Its witnessed wait
    // can explain how its own delayed answer came to be credited to the next press.
    let mut observations: Vec<_> = samples.into_iter().map(|(s,c)| (s,c,true)).collect();
    observations.extend(p.frame_hits.borrow().iter().map(|&(s,c)| (s,c,false)));
    observations.sort_unstable();
    for (sent, commit, measured) in observations {
        let answer = log.iter().find(|w| w.at == sent).unwrap().first.unwrap();
        if answer == commit {
            continue;
        }
        // Check *this* wrong pairing, not whether some unrelated write anywhere in the table cell
        // had a long delay. A late or second piece from an older write can cross the boundary.
        let late = log.iter().take_while(|w| w.at < sent).any(|w| w.first == Some(commit) && commit - w.at >= tty::ANSWER_MAX.as_nanos() as u64);
        let piece = matches!(shape,Pieces(g) if g>tty::QUIET.as_nanos() as u64 && log.iter().take_while(|w|w.at<sent).any(|w|w.first.map(|t|t+g)==Some(commit)));
        // If the earlier input already took a late piece, its *first* change may in turn be
        // credited to this one. Follow only a chain witnessed in the program's answer log.
        let inherited = log.iter().filter(|w| w.at < sent && w.first == Some(commit)).find_map(|w| shifted.get(&w.at)).copied();
        let reason = if late {
            "out·1s"
        } else if piece {
            "out·parts"
        } else if let Some(reason) = inherited {
            reason
        } else if measured {
            return "WRONG";
        } else {
            continue;
        };
        shifted.insert(sent, reason);
        if measured { verdict = reason; }
    }
    verdict
}
fn table() {
    let mut counts = std::collections::BTreeMap::new();
    for kind in ["press", "paste", "click", "wheel", "scroll", "drag-press", "down-up", "type"] {
        for pattern in ["back-to-back", "until-frame", "until-idle", "wait-200", "wait-1000"] {
            if kind == "type" && pattern != "back-to-back" {
                continue;
            }
            for (shape, label) in
                [(One, "one write"), (Pieces(30 * MS), "two writes 30 ms apart"), (Pieces(70 * MS), "two writes 70 ms apart"), (Frame, "synchronized frame"), (Silent, "no cell change")]
            {
                let cells = [20, 49, 51, 120, 299, 301, 999, 1001].map(|d| cell(kind, pattern, d * MS, shape));
                println!("| {kind} | {} | {label} | {} |", if kind == "type" { "paced type" } else { pattern }, cells.join(" | "));
                for c in cells {
                    *counts.entry(c).or_insert(0) += 1;
                }
            }
        }
    }
    println!("{counts:?}");
    assert!(!counts.contains_key("WRONG"));
}

fn rows() -> Result<()> {
    let mut p = Program::new(120 * MS, One);
    p.kitty = 8 | 2; // every key, including releases
    let page = Driver::new(&p, None);
    for (edge, key) in [("down","Shift"), ("press","a"), ("up","Shift")] {
        call(&page, edge, json!([key,{"until":"frame"}]))?;
        assert!(p.now_ns() > p.wire.borrow().last().unwrap().first.unwrap());
    }
    let (lat, _) = result(&page, &p, window(end(&page, &p)));
    assert!(tty::ambiguous(&lat).is_none());
    assert_eq!(lat.ms, [120.]);
    assert_eq!(p.wire.borrow()[1].bytes, b"\x1b[97;2u");
    for (edge,key) in [("down","Shift"),("up","Shift"),("up","x")] {
        let p = Program::new(0, Silent);
        let page = Driver::new(&p, None);
        assert_eq!(call(&page, edge, json!([key,{"until":"frame","timeout":5}])).unwrap_err().code,124);
        assert!(p.wire.borrow().is_empty() && page.inputs().is_empty());
    }
    println!("modifier hold: both reported edges wait; only the shifted key is sampled; no-byte edges still wait");
    assert!(!strobe::driver::press_needs_focus(&json!({"burst":true})));
    assert!(strobe::driver::press_needs_focus(&json!({"selector":"#field","burst":true})));
    assert!(!strobe::probe::present::frames::burst_refusal(1).contains("type with burst"));
    println!("7: page burst press skips focus/settle unless it names a selector; refusal names burst, not type");
    assert_eq!([tty::QUIET, tty::SETTLE_MAX, tty::IDLE, tty::ANSWER_MAX].map(|n| n.as_millis()), [50, 300, 300, 1000], "review the timing contract before changing these boundaries");
    for d in [299, 301] {
        assert_eq!(cell("press", "until-idle", d * MS, One), if d == 299 { "ok" } else { "R·over" });
    }
    println!("9: 299 ms idle: right; 301 ms: refused overrun");
    let p = Program::new(320 * MS, One);
    let page = Driver::new(&p, None);
    call(&page, "type", json!(["text=Field", "x"]))?;
    let (l, _) = result(&page, &p, window(end(&page, &p)));
    assert_eq!(l.ms, [320.0]);
    assert!(tty::ambiguous(&l).is_none());
    println!("1: 320 ms focus followed by 320 ms key: 320 ms (not 20)");
    let p = Program::new(80 * MS, One);
    let page = Driver::new(&p, None);
    press(&page)?;
    call(&page, "press", json!(["x",{"selector":"text=Field"}]))?;
    let (l, _) = result(&page, &p, window(end(&page, &p)));
    assert!(tty::ambiguous(&l).is_some());
    println!("1: unanswered key before focus: refused");
    let p = Program::new(200 * MS, One);
    let page = Driver::new(&p, None);
    call(&page, "drag", json!([[0,0],[[0,0],[0,0],[0,0],[0,0],[0,1]],{"duration":992,"until":"frame"}]))?;
    press(&page)?;
    let ins = page.inputs();
    assert_eq!(ins.len(), 4);
    assert!(ins[2].sent_ns - ins[1].sent_ns > 50 * MS);
    let (l, _) = result(&page, &p, window(end(&page, &p)));
    assert!(tty::ambiguous(&l).is_some());
    println!("2: sparse drag with sleeps after final move: release is a third write; next key refused");
    let p = Program::new(0, Silent);
    let page = Driver::new(&p, None);
    press(&page)?;
    let stop = end(&page, &p);
    let reset = p.now_ns() + 40 * MS;
    p.events.borrow_mut().push(update(reset, reset));
    p.sleep(std::time::Duration::from_millis(100));
    assert_eq!(result(&page, &p, window(stop)).0.silent, 1);
    let t = Timed { untimed: vec![(START + MS, reset + MS)], ..window(p.now_ns()) };
    assert_eq!(result(&page, &p, t).0.silent, 1);
    println!("3: reset after run, or redraw in untimed gap: silent");
    let p = Program::new(520 * MS, PlainThenFrame);
    let page = Driver::new(&p, None);
    press(&page)?;
    press(&page)?;
    let (l, _) = result(&page, &p, window(end(&page, &p)));
    assert!(l.overran > 0);
    println!("4: plain change then open frame spans second key: refused");
    let mut p = Program::new(120 * MS, One);
    p.conpty = true;
    let page = Driver::new(&p, None);
    call(&page, "window", json!([{"resize":{"width":100,"height":30}}]))?;
    press(&page)?;
    let (l, _) = result(&page, &p, window(end(&page, &p)));
    assert!(l.overran > 0);
    println!("5: resize's host redraw does not answer for the program: refused");
    for kind in ["click", "wheel", "paste"] {
        let p = Program::new(150_000, One);
        let page = Driver::new(&p, None);
        for _ in 0..2 {
            step(&page, kind, "until-idle")?;
        }
        let (l, _) = result(&page, &p, window(p.now_ns()));
        assert!(tty::ambiguous(&l).is_none());
        assert_eq!(l.ms, [0.15, 0.15]);
        let log = p.wire.borrow();
        assert_eq!(log.len(), 2);
        let expected: &[u8] = match kind {
            "click" => b"\x1b[<0;1;1M\x1b[<0;1;1m",
            "wheel" => b"\x1b[<65;1;1M\x1b[<65;1;1M\x1b[<65;1;1M",
            _ => b"\x1b[200~A\rB\x1b[201~",
        };
        assert_eq!(log[0].bytes, expected);
        println!("6: {kind}, one write; two 0.15 ms answers");
    }
    let p = Program::new(80 * MS, BothEdges);
    let page = Driver::new(&p, None);
    step(&page, "click", "until-idle")?;
    step(&page, "click", "until-idle")?;
    assert_eq!(result(&page, &p, window(p.now_ns())).0.ms, [80., 80.]);
    println!("8: click drawing on both edges: idle separates clicks");
    for (kind, fix) in [("scroll", "lower scroll speed"), ("click", "click or wheel"), ("press", "paced type")] {
        let p = Program::new(120 * MS, One);
        let page = Driver::new(&p, None);
        for _ in 0..5 {
            step(&page, kind, "back-to-back")?;
        }
        press(&page)?;
        let (l, _) = result(&page, &p, window(end(&page, &p)));
        assert!(tty::ambiguous(&l).is_some_and(|(_, s)| s.contains(fix)), "wrong advice for {kind}: {:?}", tty::ambiguous(&l));
    }
    for (delay, want) in [(50_170_000, "R·over"), (49_990_000, "ok")] {
        assert_eq!(cell("press", "back-to-back", delay, One), want);
    }
    for delay in [45, 55] {
        assert_eq!(cell("click", "until-frame", delay * MS, One), "ok");
    }
    println!("50.17 ms late answer: overrun; 49.99 ms answer: right. Click across old 50 ms cliff: right on both sides");
    for wait in [999_990_000, 1000 * MS] {
        let mut p = Program::new(0, Silent);
        p.script.push((20 * MS, One));
        let page = Driver::new(&p, None);
        press(&page)?;
        p.sleep(std::time::Duration::from_nanos(wait - 1));
        press(&page)?;
        let (l, _) = result(&page, &p, window(end(&page, &p)));
        assert_eq!(tty::ambiguous(&l).is_none(), wait == 1000 * MS);
    }
    println!("unanswered key: below one second refused, at one second right");
    for pre in [49_990_000, 50 * MS] {
        let p = Program::new(20 * MS, One);
        p.events.borrow_mut().push(update(START - pre, START - pre));
        let page = Driver::new(&p, None);
        press(&page)?;
        assert_eq!(p.wire.borrow()[0].at, START + 50 * MS - pre);
    }
    let p = Program::new(20 * MS, One);
    let page = Driver::new(&p, None);
    call(&page, "type", json!(["","xyz",{"burst":true}]))?;
    page.quiesce();
    let t0 = p.now_ns();
    press(&page)?;
    let (l, _) = result(&page, &p, Timed { t0, ..window(end(&page, &p)) });
    assert_eq!(l.ms, [20.]);
    assert!(tty::ambiguous(&l).is_none());
    let mut p = Program::new(0, Silent);
    p.cursor = (0, 1);
    p.script = vec![(20 * MS, One)];
    let page = Driver::new(&p, None);
    call(&page, "type", json!(["text=Field", "x"]))?;
    assert_eq!(p.wire.borrow().len(), 1);
    let p = Program::new(10 * MS, One);
    for k in 0..60 {
        let t = START + k * 20 * MS;
        p.events.borrow_mut().push(update(t, t));
    }
    let page = Driver::new(&p, None);
    call(&page, "type", json!(["", "abcdef"]))?;
    let t = end(&page, &p);
    let (l, _) = result(&page, &p, window(t));
    assert_eq!(page.inputs().len(), 6);
    assert!(page.inputs().iter().all(|i| i.sample));
    assert!(tty::ambiguous(&l).is_some());
    println!("rest edge: 49.99 ms waits, 50.00 ms sends; setup burst fenced; already-focused field sends only the key; six mid-draw keys refuse the block");
    Ok(())
}
fn main() {
    rows().unwrap();
    if std::env::args().nth(1).as_deref() == Some("table") {
        table();
    }
}
