//! Manual known answers for the production validator. Not linked into shipped binaries.
use super::*;

fn cmp(m: &str) -> Comparison {
    stats::compare(m, model::metric(m).unwrap(), &[(100., 100.); 8], 0.02, 0.95)
}
fn impact<'a>(p: &'a Comparison, cores: usize) -> LoadImpact<'a> {
    LoadImpact {
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
        .map(|i| Block {
            pair: i / 2,
            variant: if i % 2 == 0 { "base" } else { "head" }.into(),
            values: BTreeMap::from([("cpu.foreign.max".into(), load(i)), ("cpu.app.total".into(), 100.)]),
            ..Default::default()
        })
        .collect()
}
fn expect(row: &str, note: Option<Warning>, want: &str) {
    let level = note.as_ref().map_or("nothing", |w| w.level.as_str());
    assert_eq!(level, want, "{row}: {:?}", note.as_ref().map(|w| &w.text));
    println!("{row}: {want}");
}
fn read_run(timed: &[RunLoad], warmup: usize) -> Block {
    run_load(Block { warmup, run_ms: vec![100.], ..Default::default() }, timed, None)
}
pub fn run(logs: Option<&Path>) {
    let p = cmp("run.duration");
    let a = impact(&p, 8);
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
    sequential::gate_and_holm(&mut family, true);
    assert!(family.iter().all(|c| c.kind == VerdictKind::Untested && !judged_secondary(c)));
    let mut a = impact(&p, 8);
    a.judged.extend(family.iter().filter(|c| judged_secondary(c)).map(|c| (c, None)));
    expect("Holm discarded frame secondary", background_note(&blocks(16, |i| if i < 2 { 35. } else { 0. }), &a), "info");
    let measured = RunLoad { ms: 100., busy_ms: Some(177.), observer_ms: 2., own_ms: Some(100.), stolen_ms: Some(3.) };
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
            at.judged[0].1 = Some((quiet.len(), sequential::Design::new(None, 0.02, None, None).compare(metric, model::metric(metric).unwrap(), &quiet)));
        }
        let note = background_note(&bs, &at);
        let level = note.as_ref().map_or("nothing", |w| w.level.as_str());
        *counts.entry(level.into()).or_default() += 1;
        println!("{} {level}: {}", path.file_name().unwrap().to_string_lossy(), note.as_ref().map_or("", |w| w.text.as_str()));
    }
    println!("replay: {counts:?}");
}
