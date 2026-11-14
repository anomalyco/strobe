//! Ordered regimes, including replacements. Compare with the recorded main results, not just a
//! retry count. A main table is obtained before changing the product judgement, using its original
//! retry condition. The primary, clean comparison, refusal and warning always come from strobe.
use strobe::app::bench::{
    Block, Burst, Design, LoadImpact, background_note, load_sets,
    retry::{Decision, Schedule},
};
use strobe::model::{VerdictKind, metric};

const REGIMES: [&str; 6] = ["quiet", "burst", "alternating", "light", "both", "one"];

fn reading(regime: &str, i: usize, replacement: bool) -> [f64; 2] {
    match regime {
        "quiet" => [0., 0.],
        "light" => [30., 30.],
        "one" => [0., 150.],
        "both" => {
            if i % 2 == 0 {
                [50., 200.]
            } else {
                [200., 50.]
            }
        }
        "burst" if i != 0 || replacement => [0., 0.],
        "burst" => [150., 0.],
        "alternating" if replacement => [0., 0.],
        "alternating" if i % 2 == 0 => [0., 150.],
        "alternating" => [150., 0.],
        _ => unreachable!(),
    }
}

fn sequence(first: &str, second: &str, budget: Option<usize>) -> String {
    let design = Design::new(budget, 0.02, None, None);
    let n = design.min_pairs();
    let mut schedule = Schedule::resume(Some(8), &[], &[], &[]);
    let (mut bs, mut pairs, mut discarded) = (vec![], vec![], Vec::<Burst>::new());
    let mut pp = vec![];
    let mut quiet = vec![];
    loop {
        let i = pairs.len();
        let replacement = discarded.last().is_some_and(|b| b.pair == i);
        let load = match first {
            "late" if i == 0 => [30., 30.],
            "late" if i == n - 1 && !replacement => [150., 0.],
            "late" => [0., 0.],
            "unanimous" if i == 0 && !replacement => [30., 180.],
            "unanimous" => [0., 150.],
            "irregular" if i < n - 2 => [50., 200.],
            "irregular" => [80., 50.],
            "recovery" => match i {
                0 if !replacement => [0., 150.],
                0 => [30., 30.],
                1 | 3 => [150., 0.],
                _ => [0., 0.],
            },
            _ if i < n / 2 => reading(first, i, replacement),
            _ => reading(second, i - n / 2, replacement),
        };
        let d: f64 = match first {
            "late" if i == n - 1 && !replacement => -0.035,
            "late" if i == n - 1 => 0.002,
            "late" if i == n - 2 => 0.0005,
            "late" => -0.001,
            "unanimous" => 0.12,
            _ => (load[1] - load[0]) * 0.00004,
        };
        let [a, b] = std::array::from_fn(|v| {
            let mut b = Block::default();
            b.variant = ["base", "head"][v].into();
            b.pair = i;
            b.values.insert("cpu.foreign.max".into(), load[v]);
            b.values.insert("cpu.app.total".into(), 100.);
            b
        });
        let apart = load[1] - load[0];
        let retry = matches!(schedule.decide(&a, &b, discarded.len(), true, true), Decision::Replace(_));
        if retry {
            discarded.push(Burst { pair: i, base: load[0], head: load[1], apart, base_steal: None, head_steal: None });
            continue;
        }
        pairs.push((bs.len(), bs.len() + 1));
        bs.extend([a, b]);
        let values = (100., 100. * (1. + d));
        pp.push(values);
        if load.iter().all(|&f| f < 25.) {
            quiet.push(values);
        }
        if !design.looks.contains(&pp.len()) {
            continue;
        }
        let compare = |p: &[(f64, f64)]| design.compare("run.duration", metric("run.duration").unwrap(), p);
        let c = compare(&pp);
        if !c.decided && pp.len() < design.max_pairs() {
            continue;
        }
        let refusal = load_sets(&bs, &pairs, &discarded, None)[0].refusal(["base", "head"]);
        let impact = LoadImpact {
            pairs: &pairs,
            counted_cpus: 8,
            busy_before: 0.,
            expects_load: true,
            moves: true,
            primary: Some(&c),
            judged: vec![(&c, (quiet.len() >= 8).then(|| (quiet.len(), compare(&quiet))))],
            min_clean: 8,
            cores: Some(8),
            split_core: false,
            discarded: &discarded,
            lopsided: refusal.as_ref(),
            names: ["base", "head"],
        };
        let note = background_note(&bs, &impact);
        let kind = if refusal.is_some() { VerdictKind::Refused } else { c.kind };
        return format!(
            "{}{}{}",
            pp.len() + discarded.len() - n,
            match kind {
                VerdictKind::Refused => "R",
                VerdictKind::Same => "S",
                VerdictKind::Better => "B",
                VerdictKind::Worse => "W",
                _ => "?",
            },
            note.as_ref().map_or("-", |w| if w.level == "warn" { "w" } else { "i" })
        );
    }
}

pub fn table() {
    // No seventh difference: ambient load suppresses all six retries. Five pairs must not use the
    // six-pair exception; six qualify only when every difference clears the floor in one direction.
    for (n, dissent, want) in [(5, false, false), (6, true, false), (6, false, true)] {
        let mut blocks = Vec::new();
        let mut pairs = Vec::new();
        for i in 0..n {
            pairs.push((2 * i, 2 * i + 1));
            for pct in [30., if dissent && i == 0 { 40. } else { 180. }] {
                let mut b = Block::default();
                b.values.insert("cpu.foreign.max".into(), pct);
                blocks.push(b);
            }
        }
        assert_eq!(load_sets(&blocks, &pairs, &[], None)[0].refusal(["base", "head"]).is_some(), want);
    }
    let mut rows: Vec<_> = REGIMES.iter().flat_map(|a| REGIMES.iter().map(move |b| (*a, *b))).collect();
    rows.extend([("late", ""), ("unanimous", ""), ("irregular", ""), ("recovery", "")]);
    for (a, b) in rows {
        let prefix = format!("{a} {} ", if b.is_empty() { "-" } else { b });
        let line = include_str!("CONVERGE.md").lines().find_map(|l| l.strip_prefix(&prefix)).unwrap();
        let cells: Vec<_> = [Some(6), Some(8), Some(12), None]
            .into_iter()
            .zip(line.split_whitespace())
            .map(|(budget, main)| {
                let branch = sequence(a, b, budget);
                let lost_refusal = main.contains('R') && !branch.contains('R');
                let lost_warning = main.ends_with('w') && !branch.ends_with('w');
                let new_direction = main.contains('S') && (branch.contains('W') || branch.contains('B'));
                // The omitted ninth observation cannot be counted as if it had actually run.
                let exception = a == "irregular" && main == "1Rw" && matches!((budget, branch.as_str()), (Some(8), "0Ww") | (None, "0Sw"));
                assert_eq!(lost_refusal || lost_warning || new_direction, exception, "{a}/{b}/{budget:?} {main}:{branch}");
                assert!(!lost_warning, "warning lost: {a}/{b}/{budget:?}");
                if main == branch { branch } else { format!("{main}:{branch}") }
            })
            .collect();
        assert_eq!(cells.len(), 4);
        println!("| {a} {b} | {} |", cells.join(" | "));
    }
}
