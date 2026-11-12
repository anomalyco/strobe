//! `strobe run -- <command>`: an ad-hoc harness under a bench block's conditions (SPEC §5.13): the
//! device's bench slot, the fastest power plan, pinned to the target's CPUs with this user's other
//! processes kept off them. It measures nothing itself: the harness prints its own numbers.
//!
//! `strobe run --base "<command>" --head "<command>"`: a paired A/B of two commands, as a bench of a
//! `cli()` target whose variants are the command lines (`cmd:<line>`): the same blocks, looks,
//! verdicts, result and report as any bench. `--check` runs each command once (one side, or both
//! with `--base`), its line showing run.duration and memory.peak beside cpu.work unless `--metrics`
//! names others.

use super::{Output, bench, progress};
use crate::cli::{Args, duration_ms};
use crate::host;
use crate::model::{Error, Result};
use serde_json::json;
use std::process::Command;
use std::time::{Duration, Instant};

pub fn run(args: &Args) -> Result<Output> {
    if args.get("base").is_some() || args.get("head").is_some() || args.has("aa") {
        return paired(args);
    }
    if args.has("seed") {
        return Err(Error::usage("--seed seeds a bench's ctx.random(); strobe run -- <command> runs your own harness: give it its own seed"));
    }
    // A harness under a bench block's conditions times something: on a shared CI runner nothing does.
    bench::refuse_shared("strobe run")?;
    let at = args.pos.iter().position(|a| a == "--").ok_or_else(|| {
        Error::usage(
            "run needs commands: strobe run --base \"<command>\" --head \"<command>\" times two against each other (add --check to run each once first), or strobe run [--pin auto] [--reserve-cpus on|off] [--power high|keep] -- <command> [args…] runs your own harness under a bench's conditions",
        )
    })?;
    let cmd = &args.pos[at + 1..];
    let (program, rest) = cmd.split_first().ok_or_else(|| Error::usage("run: nothing after --"))?;
    let (pin, _, label) = bench::pinning(args.get("pin").unwrap_or("auto"), &host::topology())?;
    let pin_label = label.replace(" · runner on the other CPUs", "");
    let id = crate::store::new_id("run");
    // Takes turns with benches: a harness and a timed block never share the CPUs. `--max-time` caps the
    // wait for its turn (exit 124, `ConcurrentBench`), not the harness.
    let deadline = args.get("max-time").map(duration_ms).transpose()?.map(|ms| Instant::now() + Duration::from_millis(ms));
    let reg = bench::slot::Registration::new(&id, &cmd.join(" "));
    let (slot, waited) = reg.acquire_until(&format!("strobe run {program}"), deadline)?;
    let power = bench::power_for(args, true, pin)?;
    let reserve = match (args.get("reserve-cpus").unwrap_or("on"), pin) {
        ("off", _) | (_, None) => None,
        (_, Some(cpus)) => match host::reserve::Reserve::start(cpus) {
            Ok(r) => Some(r),
            Err(why) => {
                progress(format!("  other processes stay on the target's CPUs: {why}"));
                None
            }
        },
    };
    let awake = host::Awake::start();
    awake.hold(true);
    progress(format!("strobe run · {pin_label} · power plan {}{}", power.used, if reserve.is_some() { " · others kept off those CPUs" } else { "" }));
    let started = Instant::now();
    let spawned = host::spawn_pinned(Command::new(program).args(rest), pin, 0);
    let status = spawned.map_err(|e| Error::msg(format!("run {program}: {e}"))).and_then(|mut c| c.wait().map_err(Error::from));
    let ms = started.elapsed().as_secs_f64() * 1e3;
    let report = reserve.map(host::reserve::Reserve::finish);
    awake.hold(false);
    drop(power);
    drop(slot);
    let status = status?;
    let code = status.code().unwrap_or(1);
    let moved = report.as_ref().map(|r| r.moved_total()).unwrap_or(0);
    let text = format!(
        "ran {program} in {} · exit {code} · {pin_label}{}\n",
        super::fmt::ms_duration(ms as u64),
        if report.is_some() { format!(" · {moved} other processes kept off its CPUs") } else { String::new() }
    );
    let mut out = Output::new(text, json!({ "id": id, "command": cmd, "exit": code, "ms": ms, "waited_ms": waited.as_secs_f64() * 1e3, "pin": pin_label, "moved": moved }));
    out.code = code;
    Ok(out)
}

/// What `strobe run --check` shows beside its primary (`cpu.work`) when `--metrics` names none.
const CHECK_METRICS: [&str; 2] = ["run.duration", "memory.peak"];

/// The bench file of a command A/B: one `cli()` target whose variants supply the command, one case
/// that runs it once per run. Its text depends only on the declared metrics, so the rerun ledger sees
/// the same harness for the same question.
fn bench_file(metrics: &[String]) -> String {
    format!(
        "// Written by `strobe run --base … --head …`: a paired A/B of two commands (SPEC §5.13).\nimport {{ bench, cli }} from \"@hona/strobe\"\n\nexport default bench({{\n  targets: {{ command: cli() }},\n  cases: {{\n    run: {{ primary: \"cpu.work\", metrics: {}, resetCheck: false, run: (ctx) => ctx.exec() }},\n  }},\n}})\n",
        serde_json::to_string(metrics).unwrap_or_else(|_| "[]".into())
    )
}

fn paired(args: &Args) -> Result<Output> {
    if args.pos.iter().any(|a| a == "--") {
        return Err(Error::usage("strobe run compares commands given as --base \"<command>\" --head \"<command>\" (or --aa --head \"<command>\"), not after --"));
    }
    let head = args.get("head").ok_or_else(|| Error::usage("strobe run --base \"<command>\" --head \"<command>\": --head names the command to measure"))?;
    match (args.get("base"), args.has("aa"), args.has("check")) {
        (Some(_), true, _) => return Err(Error::usage("--aa runs --head against itself; drop --base")),
        (None, false, false) => return Err(Error::usage("say what to compare: --base \"<command>\" (or --aa for the same command on both sides, or --check to run it once)")),
        _ => {}
    }
    let mut metrics: Vec<String> = args.get("metrics").unwrap_or("").split(',').map(str::trim).filter(|m| !m.is_empty()).map(str::to_string).collect();
    if let Some(bad) = metrics.iter().find(|m| crate::model::metric(m).is_none()) {
        return Err(Error::usage(format!("--metrics: {bad} is no metric (a command gives cpu.work, run.duration, cli.launch, cpu.wakes, memory.peak, io.read, io.write and metric.<name>)")));
    }
    // A check reads each side once: only an exact counter compares that way. Said before anything runs,
    // in this command's words.
    if args.has("check")
        && let Some(timed) = args.get("noworse").and_then(|n| {
            n.split(',')
                .map(|p| p.split_once('=').map_or(p, |(k, _)| k).trim())
                .map(|k| if k.ends_with('%') || k.parse::<f64>().is_ok() { "cpu.work" } else { k })
                .find(|k| !crate::model::is_ci_counter(k))
        })
    {
        return Err(Error::usage(format!(
            "--check --noworse {timed}: a check runs each command once, and compares only exact counters (count.*, net.*, tty.frames, dom.nodes), which a command gives none of; {timed} needs timed pairs: strobe run --base \"<command>\" --head \"<command>\" --noworse {timed}=2%"
        )));
    }
    // A check times nothing against anything: its line shows what a person running a command first
    // asks, how long it took and how much memory it used, beside its CPU. A comparison declares only
    // what --metrics asks (each secondary widens the gate's Holm family).
    if args.has("check") && args.get("metrics").is_none() {
        metrics = CHECK_METRICS.iter().map(|m| m.to_string()).collect();
    }
    let text = bench_file(&metrics);
    use sha2::Digest;
    let hash: String = sha2::Sha256::digest(text.as_bytes()).iter().take(6).map(|b| format!("{b:02x}")).collect();
    let dir = host::paths::run().join(format!("cmd-{hash}"));
    // Leased before it is written, until the bench ends: `strobe prune` leaves it (SPEC §5.14), and the
    // stamp's time is its last use (a `--resume` of its experiment reads it again).
    let _lease = host::cache::take(&dir)?;
    std::fs::create_dir_all(&dir)?;
    let file = dir.join("run.bench.ts");
    if std::fs::read_to_string(&file).ok().as_deref() != Some(text.as_str()) {
        crate::store::write_atomic(&file, text.as_bytes())?;
    }
    let mut b = args.clone();
    b.command = "bench".into();
    b.pos = vec![file.display().to_string()];
    b.set("head", &format!("cmd:{head}"));
    if let Some(base) = args.get("base") {
        b.set("base", &format!("cmd:{base}"));
    }
    bench::run(&b)
}
