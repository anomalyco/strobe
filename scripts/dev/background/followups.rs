//! Manual boundary rows for FOLLOWUPS.md. No timing verdict relies on the host's speed here.
use serde_json::json;
use strobe::{
    app::{
        bench::{Block, RunLoad, TickEdge, pinning, run_load},
        pr,
    },
    host,
};

pub fn run() {
    let topo = host::Topology { logical: 2, allowed: 0b1010, limit: None, groups: vec![host::CacheGroup { mask: 10, cpus: 2, l3_mb: 8 }], cores: vec![] };
    for spec in ["", ",", "1,", "8-7", "64", "1-65", "1,2", "0-3"] {
        assert_eq!(pinning(spec, &topo).unwrap_err().code, 2, "{spec}");
    }
    for (spec, target, runner) in [("1", Some(2), Some(8)), ("1,3", Some(10), None), ("ccd0", Some(10), None), ("none", None, None)] {
        if cfg!(target_os = "macos") && spec != "none" {
            let e = pinning(spec, &topo).unwrap_err();
            assert_eq!(e.code, 2);
            assert!(e.message.contains("macOS has no CPU affinity"));
            continue;
        }
        let (a, b, _) = pinning(spec, &topo).unwrap();
        assert_eq!((a, b), (target, runner));
    }
    if cfg!(target_os = "macos") {
        assert_eq!(pinning("auto", &topo).unwrap().0, None);
    }
    for suffix in [" · runner on the other CPUs", " · runner shares these CPUs", ""] {
        assert_eq!(strobe::app::run::pin_label(&format!("CCD0 (8 MB L3, 2 CPUs){suffix}")), "CCD0 (8 MB L3, 2 CPUs)");
    }
    println!("pin: holes, empty selections, reversed and overwide ranges refused; explicit/CCD full mask inherits");
    #[cfg(not(target_os = "macos"))]
    child_masks();
    #[cfg(windows)]
    console_masks();

    for ms in [2u64, 20] {
        // Idle throughout each measured run. Both CPUs busy only during the intervening second.
        let runs = (0..5)
            .map(|i| {
                let t = i * (ms + 1000);
                let a = TickEdge { ns: t * 1_000_000, idle: i * ms * 2_000_000, steal: 0 };
                let b = TickEdge { ns: a.ns + ms * 1_000_000, idle: a.idle + ms * 2_000_000, steal: 0 };
                RunLoad { observers: Some((0, 0)), own_ms: Some(0.), gap_ms: Some(0.), ..a.until(b, 2).unwrap() }
            })
            .collect::<Vec<_>>();
        let mut b = Block::default();
        b.run_ms = vec![ms as f64; 5];
        let b = run_load(b, &runs, None, false);
        assert_eq!(b.values["cpu.foreign.max"], 0.);
        assert_eq!(b.values["background.subtick"], 2000. / ms as f64);
    }
    println!("five short runs: intervening one-second waits neither flag load nor prove quiet");

    for (level, verdict, peak, raw, summary) in [
        ("warn", "unclear", 80., "other work of a quarter core or more ran on the measured CPUs", true),
        ("warn", "refused", 80., "one-sided load refused the primary", false),
        ("info", "same", 80., "the clean pairs agree", false),
        ("warn", "unclear", 0., "CPU accounting was unread", false),
        ("info", "same", 0., "the measured block span was too short", false),
    ] {
        let series = json!({"label":"first","primary":{"metric":"run.duration","verdict_kind":verdict},"warnings":[{"id":"Background","level":level,"text":raw}],"blocks":[{"variant":"base","pair":0,"values":{"cpu.foreign.max":0}},{"variant":"head","pair":0,"values":{"cpu.foreign.max":peak}}]});
        let md = pr::block(&json!({"id":"exp_fixture","series":[series]}));
        assert_eq!(md.contains("kept; see Pairs"), summary, "{md}");
        if level == "info" {
            assert!(md.contains("**Background load:"), "{md}");
        }
        if !summary {
            assert!(md.contains(raw), "{md}");
        }
    }
    println!("PR details: counted warning, refused, note, unread and zero flagged");
    for (v, reason) in [(json!({"background.unread":1}), "could not separate"), (json!({"cpu.foreign.max":600}), "heavy background")] {
        assert!(strobe::app::track::background_reason(&json!({"blocks":[{"values":v}]})).contains(reason));
    }
    // Wording contract, not separate native/terminal measurement fixtures.
    assert!(include_str!("../../../SPEC.md").contains("is `unclear`, never `rejected`"));
    assert!(include_str!("../../../src/app/bench/tty.rs").contains("and cpu.foreign when pinned"));
    assert!(!include_str!("../../../src/app/bench/native.rs").contains("this bench pins it to none (--pin none)"));
    copied();
}

fn copied() {
    use std::fs;
    use strobe::{
        app::{query, report},
        cli::Args,
    };
    let home = std::env::temp_dir().join(format!("strobe-background-copy-{}", strobe::host::now_ns()));
    fs::create_dir(&home).unwrap();
    let dir = home.join("rec_bg_copy");
    fs::create_dir(&dir).unwrap();
    let result = home.join("result.json");
    fs::write(&result, json!({"kind":"check","id":"exp_bg_copy","series":[{"label":"copied","passed":true,"recording":"rec_bg_copy","video":{"head":"rec_bg_copy"}}]}).to_string()).unwrap();
    let args = Args::parse(vec!["query".into(), result.to_str().unwrap().into(), "summary".into()]).unwrap();
    let md = Args::parse(vec!["report".into(), result.to_str().unwrap().into(), "--format".into(), "md".into()]).unwrap();
    fs::write(dir.join("video.json"), r#"{"file":"video.mp4"}"#).unwrap();
    fs::write(dir.join("video.mp4"), b"fixture").unwrap();
    for (owner, id, token, want) in
        [("exp_bg_copy", "rec_bg_copy", "ok", true), ("exp_other", "rec_bg_copy", "ok", false), ("exp_bg_copy", "rec_other", "ok", false), ("exp_bg_copy", "rec_bg_copy", "not a plain token", false)]
    {
        fs::write(
            dir.join("meta.json"),
            json!({"id":id,"experiment":owner,"token":token,"video":{"kind":"screen","width":640,"height":360,"duration_s":1.5,"frames":45},"phone":{"platform":"ios","model":"fixture phone"}})
                .to_string(),
        )
        .unwrap();
        for text in [query::run(&args).unwrap().text, report::run(&md).unwrap().text] {
            assert_eq!(text.contains("640×360 · 1.5 s · 45 frames"), want, "{text}");
            assert_eq!(text.contains("fixture phone"), want);
            assert!(!text.contains("null×null") && !dir.join(".used").exists());
            assert_eq!(text.lines().find(|l| l.trim_start().starts_with("video    ")).unwrap().contains("strobe report recording"), want, "{text}");
        }
        let empty = home.join("unused-state");
        let o = std::process::Command::new(std::env::current_exe().unwrap()).env("LOCALAPPDATA", &empty).env("XDG_DATA_HOME", &empty).arg("--summary").arg(&result).output().unwrap();
        assert!(o.status.success());
        assert!(!empty.exists(), "a copied summary created its store");
    }
    fs::remove_dir_all(home).unwrap();
    println!("copied check: exact video and phone, no null dimensions, wrong experiment ignored, no .used");
}

/// Read only the two processes of this fixture's new ConPTY, while the child waits on input.
#[cfg(windows)]
fn console_masks() {
    use strobe::target::tty::{Run, Spec, Tty};
    let mask = host::topology().all().isolate_lowest_one();
    let spec = Spec {
        run: Run::Argv(vec![std::env::current_exe().unwrap().display().to_string(), "--hold-terminal".into()]),
        cwd: std::env::temp_dir(),
        env: vec![],
        unset: vec![],
        cols: 40,
        rows: 6,
        home: None,
    };
    let tty = Tty::spawn(&spec, Some(mask), None).unwrap();
    let [console] = tty.host_pids[..] else { panic!("expected one console host") };
    let cmd = format!("(Get-Process -Id {}).ProcessorAffinity.ToInt64(); (Get-Process -Id {console}).ProcessorAffinity.ToInt64()", tty.pid);
    let o = std::process::Command::new("pwsh").args(["-NoProfile", "-Command", &cmd]).output().unwrap();
    assert!(o.status.success());
    let masks = String::from_utf8(o.stdout).unwrap();
    assert_eq!(masks.lines().map(|l| l.trim().parse::<i64>().unwrap() as u64).collect::<Vec<_>>(), [mask, mask]);
    println!("ConPTY: child and console host both report {mask:#x}");
}

#[cfg(not(target_os = "macos"))]
fn child_masks() {
    use std::process::{Command, Stdio};
    let allowed = host::topology().all();
    let bit = allowed.isolate_lowest_one();
    let child = |mask| {
        let mut c = Command::new(std::env::current_exe().unwrap());
        c.arg("--child-mask").stdout(Stdio::piped());
        host::spawn::spawn_pinned(&mut c, mask, 0)
    };
    for (mask, want) in [(None, allowed), (Some(bit), bit)] {
        let o = child(mask).unwrap().wait_with_output().unwrap();
        assert!(o.status.success());
        assert_eq!(String::from_utf8(o.stdout).unwrap().trim().parse::<u64>().unwrap(), want);
    }
    // A CPU with no hardware on this host. Only probe the impossible mask on small machines.
    if std::env::var_os("STROBE_DEV_NO_CPU63").is_some() {
        assert!(child(Some(1 << 63)).is_err());
        println!("spawn: OS rejected CPU 63 before child release");
    }
    println!("spawn: child reported inherited and requested affinity, mask {allowed:#x}");
}
