//! Each scripted attempt supplies two block readings. The product chooses the replacement, the
//! interval, and the load refusal/note. No command is timed and no load is generated.
use strobe::app::bench::{
    Block, Burst, Design, LoadImpact, background_note, load_sets,
    retry::{Decision, Schedule},
};
use strobe::model::{VerdictKind, metric};

pub fn run() {
    for path in std::env::args_os().skip(2) {
        recorded(path.as_ref());
    }
    for name in ["cpu.work", "run.duration", "launch.ready"] {
        for fixed in [None, Some(6), Some(9), Some(12)] {
            let design = Design::new(fixed, 0.02, None, None);
            let n = design.min_pairs();
            for (regime, want) in [("quiet", 0), ("one burst", 1), ("separate bursts", 2), ("both busy", 0), ("head busy", 1)] {
                let mut schedule = Schedule::resume(Some(8), &[], &[], &[]);
                let (mut blocks, mut pairs, mut discarded) = (Vec::new(), Vec::new(), Vec::new());
                let mut attempts = 0;
                while pairs.len() < n {
                    let load = match (regime, attempts) {
                        ("both busy", i) if i % 2 == 0 => [50., 200.],
                        ("both busy", _) => [200., 50.],
                        ("head busy", _) | ("one burst" | "separate bursts", 0) => [0., 150.],
                        ("separate bursts", 3) => [150., 0.],
                        _ => [0., 0.],
                    };
                    attempts += 1;
                    let pair = pairs.len();
                    let [a, b] = std::array::from_fn(|i| {
                        let mut b = Block::default();
                        b.pair = pair;
                        b.variant = ["base", "head"][i].into();
                        b.values.insert("cpu.foreign.max".into(), load[i]);
                        b.values.insert(name.into(), 100.);
                        b
                    });
                    let resumed = Schedule::resume(Some(8), &blocks, &pairs, &discarded).decide(&a, &b, discarded.len(), true, true);
                    let decision = schedule.decide(&a, &b, discarded.len(), true, true);
                    assert_eq!(decision, resumed);
                    if regime == "both busy" {
                        assert!(matches!(decision, Decision::Keep(Some("other work continued across consecutive blocks"))));
                    }
                    match decision {
                        Decision::Replace(apart) => discarded.push(Burst { pair, apart, base: load[0], head: load[1], base_steal: None, head_steal: None }),
                        Decision::Keep(_) => {
                            let i = blocks.len();
                            if pair % 2 == 0 {
                                blocks.extend([a, b]);
                                pairs.push((i, i + 1));
                            } else {
                                blocks.extend([b, a]);
                                pairs.push((i + 1, i));
                            }
                        }
                    }
                }
                assert_eq!(discarded.len(), want, "{regime}");
                assert_eq!(attempts, n + want);
                let pp: Vec<_> = pairs.iter().map(|&(a, b)| (blocks[a].values[name], blocks[b].values[name])).collect();
                let p = design.compare(name, metric(name).unwrap(), &pp);
                assert_eq!(p.kind, VerdictKind::Same);
                let names = ["base", "head"];
                let refusal = load_sets(&blocks, &pairs, &discarded, None)[0].refusal(names);
                assert_eq!(refusal.is_some(), regime == "head busy");
                let impact = LoadImpact {
                    counted_cpus: 8,
                    busy_before: 0.,
                    expects_load: true,
                    moves: true,
                    primary: Some(&p),
                    judged: vec![(&p, None)],
                    min_clean: 8,
                    cores: Some(8),
                    split_core: false,
                    discarded: &discarded,
                    lopsided: refusal.as_ref(),
                    names,
                };
                let note = background_note(&blocks, &impact);
                let level = note.as_ref().map_or("nothing", |w| w.level.as_str());
                assert_eq!(
                    level,
                    match regime {
                        "quiet" => "nothing",
                        "both busy" | "head busy" => "warn",
                        _ => "info",
                    }
                );
                println!("{name} {fixed:?} {regime}: {attempts} attempts / {} s; {level}", 2 * attempts);
            }
        }
    }
}

// Ask about the actual attempts, in order, without relabelling them as new measurements or
// inventing the primary values of set-aside attempts. Print counts, never local identifiers.
fn recorded(path: &std::path::Path) {
    let result: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    for s in result["series"].as_array().unwrap() {
        let bs: Vec<Block> = serde_json::from_value(s["blocks"].clone()).unwrap();
        let bursts: Vec<Burst> = serde_json::from_value(s["discarded_bursts"].clone()).unwrap();
        let cores = s["target_cores"].as_u64().map(|n| n as usize);
        let mut retries = Schedule::resume(cores, &[], &[], &[]);
        let mut replaced = 0;
        let mut original_aside_kept = 0;
        for p in 0..s["pairs"].as_u64().unwrap() as usize {
            for burst in bursts.iter().filter(|b| b.pair == p) {
                let [a, b] = [burst.base, burst.head].map(|load| {
                    let mut b = Block::default();
                    b.pair = p;
                    b.values.insert("cpu.foreign.max".into(), load);
                    b
                });
                match retries.decide(&a, &b, replaced, true, true) {
                    Decision::Replace(_) => replaced += 1,
                    Decision::Keep(_) => original_aside_kept += 1,
                }
            }
            let side = |role| bs.iter().find(|b| b.pair == p && b.variant == role).unwrap();
            if matches!(retries.decide(side("base"), side("head"), replaced, true, true), Decision::Replace(_)) {
                replaced += 1;
            }
        }
        println!("recorded: {} attempts discarded originally; {original_aside_kept} of those would be kept; {replaced} retry decisions on this stream", bursts.len());
    }
}
