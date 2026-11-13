//! Manual known answers for the production validator. Not linked into shipped binaries.
use std::{collections::BTreeMap, path::Path};
use strobe::{
    app::bench::{
        BACKGROUND_PCT, Block, Burst, Design, LoadImpact, RunLoad, background_note, fastest_cores, gate_and_holm, judged_secondary, load_metric, load_sets, retain_probe_context, run_load,
        take_metrics,
    },
    host,
    model::{self, VerdictKind},
    stats::{self, Comparison},
    validity::Warning,
};

fn cmp(m: &str) -> Comparison {
    stats::compare(m, model::metric(m).unwrap(), &[(100., 100.); 8], 0.02, 0.95)
}
fn impact<'a>(p: &'a Comparison, cores: usize) -> LoadImpact<'a> {
    LoadImpact {
        busy_before: 0.,
        expects_load: true,
        moves: load_metric(&p.metric),
        primary: Some(p),
        judged: vec![(p, None)],
        min_clean: 8,
        cores: Some(cores),
        split_core: false,
        discarded: &[],
        lopsided: None,
        names: ["base", "head"],
    }
}
fn blocks(n: usize, load: impl Fn(usize) -> f64) -> Vec<Block> {
    (0..n)
        .map(|i| {
            let mut b = Block::default();
            b.pair = i / 2;
            b.variant = if i % 2 == 0 { "base" } else { "head" }.into();
            b.values = BTreeMap::from([("cpu.foreign.max".into(), load(i)), ("cpu.app.total".into(), 100.)]);
            b
        })
        .collect()
}
fn expect(row: &str, note: Option<Warning>, want: &str) {
    let level = note.as_ref().map_or("nothing", |w| w.level.as_str());
    assert_eq!(level, want, "{row}: {:?}", note.as_ref().map(|w| &w.text));
    println!("{row}: {want}");
}
fn read_run(timed: &[RunLoad], warmup: usize) -> Block {
    let mut b = Block::default();
    b.warmup = warmup;
    b.run_ms = vec![100.];
    run_load(b, timed, None, false)
}
pub fn run(logs: Option<&Path>) {
    let p = cmp("run.duration");
    let a = impact(&p, 8);
    // A costly read uses the observer's CPU (60 us), not the target's. Main brackets the
    // 40 us of actual relay work. Both new load reads must land outside those same two edges.
    for us in [1000, 2000, 3000] {
        let ticks = std::cell::Cell::new(0);
        let load = || ticks.set(ticks.get() + 60);
        let a = strobe::app::bench::observer_edge(true, load, || ticks.get());
        ticks.set(ticks.get() + 40);
        let b = strobe::app::bench::observer_edge(false, load, || ticks.get());
        assert_eq!(b - a, 40);
        println!("{us} us script, 40 us relay: {:.3}% as on main; neither 60 us load read charged", 100. * f64::from(b - a) / f64::from(us));
    }
    let raw = |ms: f64, own: f64, foreign: f64| RunLoad { ms, busy_ms: Some(own + foreign * ms / 100.0), own_ms: Some(own), stolen_ms: Some(0.0), ..Default::default() };
    for (busy, want) in [(0., "info"), (0.1, "info"), (0.11, "warn")] {
        expect("unresolved Linux block", background_note(&[read_run(&[RunLoad { ticks_pct: 100., ..raw(2., 1., 0.) }], 0)], &LoadImpact { busy_before: busy, ..impact(&p, 8) }), want);
    }
    // Five 4 ms runs, with 20 ms between them: W=100 ms. Of 200 ms available, reserve
    // 10 ms for the target (including reset), 30 ms for the observer, then plant 75 ms.
    use strobe::app::bench::{TickEdge, settled};
    for (loaded, expected) in [(false, 0.), (true, 55.)] {
        let edge = |ms: u64| TickEdge { ns: ms * 1_000_000, idle: ms * if loaded { 850_000 } else { 1_600_000 }, steal: 0 };
        let runs = (0..5)
            .map(|i| {
                let t = i * 24;
                RunLoad { own_ms: Some(1.2), gap_ms: Some(1.), observers: Some((t * 300_000, (t + 4) * 300_000)), ..edge(t).until(edge(t + 4), 2).unwrap() }
            })
            .collect::<Vec<_>>();
        let block = |untimed: bool, pinned: bool| {
            let mut b = Block::default();
            b.run_ms = vec![4.; 5];
            if untimed {
                b.values.insert("untimed.n".into(), 1.);
            }
            run_load(b, &runs, None, pinned)
        };
        for pin in [false, true] {
            let b = block(false, pin);
            assert_eq!(b.values["cpu.foreign.max"], expected);
            assert_eq!(b.values.contains_key("cpu.foreign"), pin);
            assert!(!b.values.contains_key("background.subtick"));
            expect("continuous span: relay and reset excluded", background_note(&[b], &LoadImpact { primary: None, ..impact(&p, 2) }), if loaded { "warn" } else { "nothing" });
        }
        assert_eq!(block(true, false).values["background.subtick"], 500.);
    }
    for foreign in [0., 200.] {
        let mut sample = None;
        assert!(settled(&mut sample, 0., true, || raw(1., 0., 0.)).is_none());
        assert!(settled(&mut sample, 100., true, || raw(100., 50., foreign)).is_none());
        let end = settled(&mut sample, 1100., false, || panic!("sampled the stillness wait")).unwrap();
        assert_eq!(read_run(&[end], 0).values["cpu.foreign.max"], foreign);
    }
    println!("settled: last change keeps 0% or 200%, independent of confirmation");
    for row in ["page", "attached app", "native windowed", "native windowless", "script", "terminal", "command/exec"] {
        let during = raw(2.0, 1.0, 75.0);
        let only_own = raw(2.0, 1.0, 0.0);
        assert_eq!(read_run(&[only_own, during], 1).values["cpu.foreign.max"], 75.0);
        assert_eq!(read_run(&[during, only_own], 1).values["cpu.foreign.max"], 0.0);
        println!("{row}: 2 ms measured window: load in 75%, outside 0%, own alone 0%");
    }
    for ms in [1.0, 2.0, 20.0] {
        assert_eq!(read_run(&[raw(ms, ms / 2.0, 100.0)], 0).values["cpu.foreign.max"], 100.0);
    }
    assert_eq!(read_run(&[RunLoad::empty_launch()], 0).values["cpu.foreign.max"], 0.0);
    let console = RunLoad { ms: 100., busy_ms: Some(100.), own_ms: Some(20.), observer_ms: 5., ..RunLoad::default() };
    assert_eq!(read_run(&[console], 0).values["cpu.foreign.max"], 75.);
    println!("direct launch edges (1 ms, 2 ms, 20 ms); blank grid: read zero");
    let first = raw(20., 10., 100.);
    let mut late = first;
    late.add(raw(1000., 0., 0.));
    let silent = strobe::app::bench::terminal_end(first, late, false);
    assert_eq!(read_run(&[silent], 0).values["cpu.foreign.max"], 100.);
    let own = raw(20., 20., 0.);
    let mut outside = own;
    outside.add(raw(1000., 0., 100.));
    assert_eq!(read_run(&[strobe::app::bench::terminal_end(own, outside, false)], 0).values["cpu.foreign.max"], 0.);
    assert_eq!(strobe::app::bench::terminal_end(first, late, true).ms, 1020.);
    println!("terminal: 1 s silent-key timeout keeps first 20 ms; actual answer extends the span");
    for (first, second) in [(0., 200.), (200., 0.)] {
        let measured = raw(2., 1., second);
        // Launch warmup is not enqueued. The measured launch is at index 0, not at the warmup offset for run blocks.
        let launches = [(false, raw(2., 1., first)), (true, measured)].into_iter().filter_map(|(measure, r)| strobe::app::bench::measured_launch(measure, || Some(r))).collect::<Vec<_>>();
        assert_eq!(read_run(&launches, 0).values["cpu.foreign.max"], second);
        for primary_order in [true, false] {
            let mut b = read_run(&[raw(2., 1., first)], 0);
            let pass = read_run(&[measured], 0);
            take_metrics(&mut b, &pass, |m| m.starts_with("http.") != primary_order);
            assert_eq!(b.values["cpu.foreign.max"], 200.);
        }
    }
    println!("launch pair 0: measured only; blocks/track merge: 200% in either pass, both primary orders");
    for (row, timed, cases, phone, mac, want) in [
        ("comparison", true, 1, false, false, true),
        ("implementations (per role)", true, 1, false, false, true),
        ("blocks/track", true, 1, false, false, true),
        ("check", false, 1, false, false, false),
        ("repeat check", false, 1, false, false, false),
        ("batch check", false, 3, false, false, false),
        ("leak/attribution/pixels", false, 1, false, false, false),
        ("phone", true, 1, true, false, false),
        ("macOS", true, 1, false, true, false),
    ] {
        assert_eq!(strobe::app::bench::measures_host_load(timed, cases, phone, mac), want);
        println!("{row}: measure host load = {want}");
    }
    let mut old = Block::default();
    take_metrics(&mut old, &read_run(&[raw(2., 1., 0.)], 0), |_| true);
    expect("missing pass plus measured zero", background_note(&[old], &a), "warn");
    expect("resume pre-load checkpoint", background_note(&[Block::default()], &a), "warn");
    expect("resume measured quiet checkpoint", background_note(&[read_run(&[raw(2., 1., 0.)], 0)], &a), "nothing");
    let phone = LoadImpact { expects_load: false, ..impact(&p, 8) };
    expect("phone/macOS host reading inapplicable", background_note(&blocks(16, |_| 500.), &phone), "nothing");
    let warning = background_note(&blocks(16, |_| 600.), &a).unwrap();
    assert!(strobe::app::bisect::contaminated(&serde_json::json!({"decided":true,"verdict":"same","warnings":[warning]})));
    println!("bisect: warned same is not evidence for moving a bound");
    use strobe::app::bisect::{blocked, inconclusive_confirmation, retry_allowed};
    for (kind, d, unread, want) in [
        ("worse", 1, false, false),
        ("better", -1, false, true),
        ("same", 1, false, true),
        ("no_worse", 1, false, true),
        ("unclear", 1, false, true),
        ("underpowered", 1, false, true),
        ("refused", 1, false, true),
        ("worse", 1, true, true),
    ] {
        let mut step = serde_json::json!({"series_background":true,"unread":unread,"decided":!matches!(kind,"unclear"|"underpowered"|"refused"),"verdict_kind":kind,"lo":d,"hi":d});
        assert_eq!(blocked(&step, Some(1)), want);
        assert_eq!(blocked(&step, None), want && kind != "better");
        let vals = if unread { serde_json::json!({"background.unread":1}) } else { serde_json::json!({"cpu.foreign.max":600}) };
        assert_eq!(inconclusive_confirmation(&serde_json::json!({"primary":step,"warnings":[warning],"blocks":[{"values":vals}]}), 1), want);
        step["series_background"] = false.into();
        assert!(!blocked(&step, Some(1)));
    }
    assert!(retry_allowed(true, false, 1));
    assert!(!retry_allowed(true, true, 1));
    assert!(!retry_allowed(true, false, 2));
    assert!(!retry_allowed(false, false, 0));
    println!("bisect / track: experiment-only warning, both signs, Same, NoWorse, undecided, refusal, unread and retry caps");
    expect("light CPU noise", background_note(&blocks(16, |i| if i < 3 { 35. } else { 0. }), &a), "info");
    let single = LoadImpact { primary: None, ..impact(&p, 8) };
    let note = background_note(&blocks(1, |_| 35.), &single).unwrap();
    assert!(note.text.contains("in block 1") && !note.text.contains("in pair"));
    expect("single-build block location", Some(note), "warn");
    expect("half flagged", background_note(&blocks(16, |i| if i % 2 == 0 { 30. } else { 0. }), &a), "warn");
    let cores = (0..10).map(|i| host::Core { mask: 1 << i, class: u8::from(i < 2) }).collect::<Vec<_>>();
    let fast = fastest_cores(&cores, 1023).unwrap();
    assert_eq!(fast, 2);
    expect("2P + 8E", background_note(&blocks(16, |_| 150.), &impact(&p, fast)), "warn");
    let mut low = blocks(16, |i| if i < 3 { 60. } else { 0. });
    for b in &mut low {
        b.values.insert("cpu.app.total".into(), 5.);
    }
    expect("small host peak", background_note(&low, &impact(&p, 2)), "warn");
    let clean = blocks(16, |_| 0.);
    let ps = (0..8).map(|i| (2 * i, 2 * i + 1)).collect::<Vec<_>>();
    let pass = (blocks(16, |_| 500.), ps.clone());
    let probe = cmp("http.latency.p50");
    let sets = load_sets(&clean, &ps, &[], Some(&pass));
    expect("clean timing pass", background_note(sets[0].blocks, &a), "nothing");
    expect("dirty separate probe pass", background_note(sets[1].blocks, &impact(&probe, 8)), "warn");
    let one_side = (blocks(16, |i| if i % 2 == 1 { 150. } else { 0. }), ps.clone());
    let burst = [Burst { pair: 0, base: 0., head: 110., apart: 110., base_steal: None, head_steal: None }];
    let sets = load_sets(&clean, &ps, &burst, Some(&one_side));
    assert!(sets[0].refusal(a.names).is_none());
    assert!(sets[1].refusal(a.names).unwrap().summary.contains("8 of 8"));
    println!("lopsided probe pass: refused, 8/8; timing retry not counted");
    let mut family = vec![cmp("frames.interval.p95"), cmp("main.work")];
    for c in &mut family {
        c.kind = VerdictKind::Worse;
        c.lo = 0.001;
        c.hi = 0.06;
        c.p = 0.04;
    }
    gate_and_holm(&mut family, true);
    assert!(family.iter().all(|c| c.kind == VerdictKind::Untested && !judged_secondary(c)));
    let mut a = impact(&p, 8);
    a.judged.extend(family.iter().filter(|c| judged_secondary(c)).map(|c| (c, None)));
    expect("Holm discarded frame secondary", background_note(&blocks(16, |i| if i < 2 { 35. } else { 0. }), &a), "info");
    let measured = RunLoad { ms: 100., busy_ms: Some(177.), observer_ms: 2., own_ms: Some(100.), stolen_ms: Some(3.), ..Default::default() };
    let mut b = read_run(&[RunLoad::default(), measured], 1);
    assert_eq!(b.values["cpu.foreign.max"], 75.);
    retain_probe_context(&mut b);
    assert_eq!((b.values["cpu.foreign.max"], b.values["steal.pct"]), (75., 3.));
    println!("unpinned probe after filtering: 75% foreign, 3% steal");
    // Only resume..pause samples enter the accumulator. A frame wait after end and an untimed
    // stretch between the two pieces do not extend the denominator.
    let mut run = RunLoad::default();
    run.add(measured);
    assert_eq!(read_run(&[run], 0).values["cpu.foreign.max"], 75.);
    println!("headless 100 ms run, post-run frame wait excluded: 75%");
    let mut run = RunLoad::default();
    for ms in [25., 75.] {
        run.add(RunLoad { ms, busy_ms: Some(1.95 * ms), ..Default::default() });
    }
    run.own_ms = Some(100.);
    assert_eq!(read_run(&[run], 0).values["cpu.foreign.max"], 95.);
    println!("two timed stretches around an untimed wait: 95%");
    let mut unread = measured;
    unread.own_ms = None;
    expect("target CPU lost", background_note(&[read_run(&[unread], 0)], &a), "warn");
    let launch = cmp("launch.ready");
    for symmetric in [false, true] {
        let mut bs = blocks(16, |i| if symmetric || i % 2 == 1 { 75. } else { 0. });
        for b in &mut bs {
            b.values.remove("cpu.app.total");
            b.values.insert("launch.ready".into(), 100.);
            b.values.insert("cpu.work".into(), 50.);
            b.run_ms = vec![100.];
        }
        let why = load_sets(&bs, &ps, &[], None)[0].refusal(["base", "head"]);
        assert_eq!(why.is_some(), !symmetric);
        let mut a = impact(&launch, 8);
        a.lopsided = why.as_ref();
        expect(if symmetric { "symmetric launch" } else { "one-sided launch refused" }, background_note(&bs, &a), "warn");
    }
    if let Some(dir) = logs {
        replay(dir);
    }
}

// Historical logs have rounded primary pair values, block maxima above the flag floor, and the
// published verdicts. They lack run-level CPU and per-pair secondaries. Replay the primary under the
// same one-core target assumption as the original desk analysis. Never invent replacement pairs.
fn replay(dir: &Path) {
    use regex_lite::Regex;
    let pair = Regex::new(r"^\s*pair\s+(\d+)\s+\S+\s+([\d,.]+)\s*([^\s]+)\s+\S+\s+([\d,.]+)\s*([^\s]+)\s+[+−-]?\d").unwrap();
    let flag = Regex::new(r"pair (\d+) ([^:]+): up to (\d+)% of a core").unwrap();
    let interval = Regex::new(r"([+−-]?[\d.]+)%\s+([+−-]?[\d.]+)%\s+…\s+([+−-]?[\d.]+)%\s+(.+)$").unwrap();
    let number = |s: &str| s.replace(',', "").replace('−', "-").parse::<f64>().unwrap();
    let scaled = |s: &str, u: &str| {
        number(s)
            * match u {
                "µs" => 0.001,
                "s" => 1000.,
                "KB" => 0.001,
                "GB" => 1000.,
                _ => 1.,
            }
    };
    let mut counts = BTreeMap::<String, usize>::new();
    let mut files = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect::<Vec<_>>();
    files.sort();
    for path in files.iter().filter(|p| p.extension().is_some_and(|x| x == "log")) {
        let log = std::fs::read_to_string(path).unwrap();
        let Some(n) = log.lines().find_map(|l| l.strip_prefix("blocks ")?.split_whitespace().next()?.parse::<usize>().ok().filter(|_| l.contains(" pairs"))) else { continue };
        let row = log.lines().find(|l| l.starts_with("★ ")).unwrap();
        let metric = row.split_whitespace().nth(1).unwrap();
        let err = std::fs::read_to_string(path.with_extension("log.err")).unwrap();
        let mut pp = vec![None; n];
        for m in err.lines().filter_map(|l| pair.captures(l)) {
            pp[m[1].parse::<usize>().unwrap() - 1] = Some((scaled(&m[2], &m[3]), scaled(&m[4], &m[5])));
        }
        let mut bs = blocks(2 * n, |_| 0.);
        let mut labels = Vec::new();
        for m in flag.captures_iter(log.lines().find(|l| l.starts_with("! Background") && l.contains("block(s)")).unwrap_or("")) {
            let label = &m[2];
            if !labels.contains(&label.to_owned()) {
                labels.push(label.to_owned())
            }
            let side = usize::from(label == "head" || (label != "base" && labels.iter().position(|x| x == label) == Some(1)));
            let i = 2 * (m[1].parse::<usize>().unwrap() - 1) + side;
            bs[i].values.insert("cpu.foreign.max".into(), number(&m[3]));
        }
        let mut p = cmp(metric);
        let kind = if let Some(m) = interval.captures(row) {
            p.estimate = number(&m[1]) / 100.;
            p.lo = number(&m[2]) / 100.;
            p.hi = number(&m[3]) / 100.;
            p.verdict = m[4].to_string();
            match m[4].split_whitespace().next().unwrap() {
                "same" => VerdictKind::Same,
                "slower" | "more" | "worse" => VerdictKind::Worse,
                "faster" | "less" | "fewer" => VerdictKind::Better,
                "underpowered:" => VerdictKind::Underpowered,
                _ => VerdictKind::Unclear,
            }
        } else if row.contains("refused") {
            VerdictKind::Refused
        } else {
            panic!("unread primary: {row}")
        };
        p.kind = kind;
        p.decided = kind.decided();
        let quiet = pp.iter().zip(bs.as_chunks::<2>().0).filter(|(_, b)| b.iter().all(|x| x.values["cpu.foreign.max"] < BACKGROUND_PCT)).filter_map(|(v, _)| *v).collect::<Vec<_>>();
        let mut at = impact(&p, 8);
        if quiet.len() >= 8 {
            at.judged[0].1 = Some((quiet.len(), Design::new(None, 0.02, None, None).compare(metric, model::metric(metric).unwrap(), &quiet)));
        }
        let note = background_note(&bs, &at);
        let level = note.as_ref().map_or("nothing", |w| w.level.as_str());
        *counts.entry(level.into()).or_default() += 1;
        println!("{} {level}: {}", path.file_name().unwrap().to_string_lossy(), note.as_ref().map_or("", |w| w.text.as_str()));
    }
    println!("replay: {counts:?}");
}
