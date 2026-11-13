//! Plant the same foreign name in all three local structured exceptions. Check whole artifacts,
//! rather than maintaining a list of warnings a particular renderer happens to print today.
use serde_json::{Value, json};
use std::fs;
use strobe::{
    app::{self, bench::ReserveTotal, obstruction, query, report},
    cli::Args,
    host,
    text::{PrivKind, Private},
    validity,
};

const NAME: &str = "foreign-canary-6c459.exe";
const PID: u32 = 96371482;
const TITLE: &str = "covering-title-4dc";
const SERIAL: &str = "phone-serial-0a8";
const URL: &str = "https://private-tab-91d.invalid";
const SID: &str = "S-1-5-21-836190";
const PATH: &str = "/private/grantee-42a.app";
const LEGIT: &str = "case target.exe: could not read /fixture/target.exe\nRunIncomplete: child.exe (pid 7182)\nbusy thread of process 7183";

fn allowed(s: &str) {
    for token in [NAME, "96371482", TITLE, SERIAL, "private-tab-91d", SID, "grantee-42a"] {
        assert!(!s.contains(token), "planted identity {token} in artifact");
    }
}

fn producers() -> Vec<String> {
    let private = |s: &str, k| Private::new(s.to_owned(), k);
    obstruction::window(&private(TITLE, PrivKind::WindowTitle), PID);
    vec![
        obstruction::ancestor(&private(PATH, PrivKind::HostAncestor)),
        obstruction::unattributed(PID),
        obstruction::timer(16., &private(PATH, PrivKind::HostAncestor)),
        obstruction::running_browser("browser", &[PID]),
        obstruction::unverified_browser("browser", PID, "wrong image"),
        obstruction::foreign_launch(PID, "already open"),
        obstruction::stay_awake(&format!("{PID}:712"), SERIAL),
        obstruction::socket_holder(&private(NAME, PrivKind::ForeignProc), SERIAL),
        obstruction::tab_count(&[private(URL, PrivKind::UrlSecret)], SERIAL),
        obstruction::helper_pipe(&private(PATH, PrivKind::ForeignProc), PID, &private(SID, PrivKind::UserAccount)).into(),
    ]
}

pub fn diagnostic_stream() {
    for line in producers() {
        allowed(&line);
    }
    println!("fixture-finished");
}

fn streams() {
    use strobe::target::tty::{Run, Spec, Tty};
    let exe = std::env::current_exe().unwrap();
    let pipe = std::process::Command::new(&exe).arg("--diagnostic-stream").output().unwrap();
    assert!(pipe.status.success() && pipe.stderr.is_empty());
    assert_eq!(String::from_utf8(pipe.stdout).unwrap().trim(), "fixture-finished");
    let tty = Tty::spawn(
        &Spec { run: Run::Argv(vec![exe.display().to_string(), "--diagnostic-stream".into()]), cwd: std::env::temp_dir(), env: vec![], unset: vec![], cols: 180, rows: 20, home: None },
        None,
        None,
    )
    .unwrap();
    assert!(tty.wait(std::time::Instant::now() + std::time::Duration::from_secs(10), |g, _, _| g.text().join("\n").contains("fixture-finished")));
    let screen = tty.grid(|g| g.text().join("\n"));
    for token in [NAME, "96371482", TITLE, SERIAL, URL, SID, PATH] {
        assert!(screen.contains(token), "missing {token} in private terminal");
    }
    tty.close();
}

pub fn run() {
    let mut reserve = ReserveTotal::default();
    let mut sample = host::reserve::Report::default();
    sample.moved.insert(NAME.into(), 12);
    sample.failed.insert(NAME.into(), "OS declined".into());
    reserve.add(&sample);
    let detail = validity::speed_context(&[(format!("{NAME} ({PID})"), 75.)], Some(2.), true, |s| assert!(s.starts_with("  ") && s.contains(NAME) && s.contains(&PID.to_string())));
    allowed(&detail);
    assert!(detail.contains("75%"));
    let home = std::env::temp_dir().join(format!("strobe-private-bench-{}", host::now_ns()));
    fs::create_dir(&home).unwrap();
    let file = home.join("result.json");
    let path = file.to_str().unwrap();
    streams();
    let scope = app::experiment_scope(&home);
    let current = producers();
    for text in &current {
        app::progress(text);
    }
    drop(scope);
    let log = fs::read_to_string(home.join("progress.log")).unwrap();
    allowed(&log);
    assert!(log.contains("1 tab a person opened"));
    let legacy = [
        format!("WindowHidden: another window covers the point (1, 2): {TITLE:?} of process {PID}"),
        format!("the window is not on top at (1, 2): {TITLE:?} (pid {PID}) is"),
        format!("window covered; in front of it: {NAME}"),
        format!("an element is under pane {TITLE:?} (key {NAME:?}) of process {PID}"),
        format!("{TITLE:?} of process {PID} covers the region"),
        format!("another app's window covers the point (1,2): a window of {NAME} of process {PID}"),
        format!("timers wake late: it runs in a launchd job (its oldest ancestor: {PATH}) whose ProcessType lets macOS coalesce timers"),
        format!("the app started responsible to process {PID} (what strobe runs under)"),
        format!("permission, which {PATH} (the app strobe runs under) does not have"),
        format!("the browser is running (pid {PID}): a WebDriver session would run in that same app"),
        format!("could not prove process {PID} is the browser it asked for"),
        format!("the debugging port belongs to a browser (pid {PID}) outside this launch"),
        format!("Launch Services answered the launch with process {PID}, which is not this launch's"),
        format!("another strobe ({PID}:712) keeps {SERIAL} awake"),
        format!("{NAME} holds Chrome's DevTools socket on {SERIAL}"),
        format!("{NAME} took Chrome's DevTools socket on {SERIAL} while strobe restarted the channel"),
        format!("every Chrome channel strobe may drive on {SERIAL} holds tabs a person opened: channel holds 2 tabs a person opened ({URL})"),
        format!("refused the helper pipe: served by {PATH} (pid {PID}, owner {SID}), not the installed helper"),
        format!("your live app runs from this app ({PATH})"),
        format!("waiting for the bench of an older strobe (pid {PID}) to finish"),
        format!("it ends when pid {PID} releases the slot or exits"),
        format!("Throttled: busiest processes (% of a core): {NAME} ({PID}) 75%"),
    ];
    for kind in ["check", "experiment"] {
        let mut r = json!({"id":"exp_private_rows","kind":kind,"plan":{"reserve":reserve,"runs_per_block":1},"variants":[{"role":"base","label":"base"},{"role":"head","label":"head"}],"warnings":[reserve.note(),validity::warn("Background",detail.clone())],"series":[{"label":"fixture","kind":"runs","passed":true,"primary":{"metric":"run.duration","verdict":"unclear","verdict_kind":"unclear"},"pairs":6,"launches":12,"discarded_bursts":[{},{}],"warnings":[validity::warn("Throttled",detail.clone())],"blocks":[{"pair":0,"variant":"base","values":{"cpu.foreign.max":75},"background":[[NAME,75]]},{"pair":0,"variant":"head","values":{"cpu.foreign.max":0}}]}]});
        for line in &current {
            r["warnings"].as_array_mut().unwrap().push(json!({"level":"warn","id":"LiveChannel","text":line}));
        }
        let mut prose = r.clone();
        for k in ["moved", "failed"] {
            prose["plan"]["reserve"].as_object_mut().unwrap().remove(k);
        }
        prose["series"][0]["blocks"][0].as_object_mut().unwrap().remove("background");
        allowed(&prose.to_string());
        // Older persisted prose must also render safely. The structured fields stay on disk.
        r["warnings"]
            .as_array_mut()
            .unwrap()
            .push(json!({"level":"warn","id":"Background","text":format!("slow; busiest processes on the machine (% of a core): {NAME} ({PID}) 75%; the check ran at once")}));
        for (i, line) in legacy.iter().enumerate() {
            r["warnings"].as_array_mut().unwrap().push(json!({"level":"warn","id":"WindowHidden","text":line}));
            let key = format!("fixture.metric{i}");
            r["series"][0]["refused"][&key] = json!(line);
            r["series"][0]["base"]["refused"][&key] = json!(line);
            r["series"][0]["blocks"][0]["refused"][&key] = json!(line);
        }
        r["series"][0]["refused"]["fixture.legitimate"] = json!(LEGIT);
        r["series"][0]["label"] = json!("target.exe");
        fs::write(&file, r.to_string()).unwrap();
        let query = Args::parse(vec!["query".into(), path.into(), "summary".into()]).unwrap();
        let answer = query::run(&query).unwrap();
        let t = answer.text;
        assert_eq!(answer.json["series"][0]["refused"]["fixture.legitimate"], LEGIT);
        assert_eq!(answer.json["series"][0]["label"], "target.exe");
        for ptr in ["/series/0/refused", "/series/0/base/refused", "/series/0/blocks/0/refused"] {
            allowed(&answer.json.pointer(ptr).unwrap().to_string());
        }
        let insights = Args::parse(vec!["query".into(), path.into(), "insights".into()]).unwrap();
        allowed(&query::run(&insights).unwrap().text);
        allowed(&t);
        if kind == "experiment" {
            assert!(t.contains("12 kept launches + 4 set aside"));
        }
        for format in ["md", "html", "summary", "pr", "card", "badge"] {
            if kind == "check" && matches!(format, "pr" | "card" | "badge") {
                continue;
            }
            let out = home.join(match format {
                "card" | "badge" => "image.svg",
                _ => "report.txt",
            });
            let args = Args::parse(["report", path, "--format", format, "--out", out.to_str().unwrap()].map(str::to_string).to_vec()).unwrap();
            let result = report::run(&args).unwrap();
            allowed(&result.text);
            let body = fs::read_to_string(out).unwrap();
            allowed(&body);
            if kind == "experiment" && matches!(format, "md" | "pr" | "summary") {
                assert!(body.contains("12 kept launches + 4 set aside"), "retry count missing in {format}");
            }
        }
        let stored: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert_eq!(stored, r);
    }
    let meta = home.join("meta.json");
    let data = json!({"id":"rec_private_rows","kind":"native","background":[[NAME,75]],"insights":{"hits":[{"evidence":{"busiest":[[NAME,75]]}}]},"note":legacy[6],"unattributed":legacy[7]});
    fs::write(&meta, data.to_string()).unwrap();
    for format in ["md", "html"] {
        let out = home.join("recording.txt");
        let args = Args::parse(["report", "recording", meta.to_str().unwrap(), "--format", format, "--out", out.to_str().unwrap()].map(str::to_string).to_vec()).unwrap();
        allowed(&report::run(&args).unwrap().text);
        allowed(&fs::read_to_string(out).unwrap());
    }
    assert_eq!(serde_json::from_slice::<Value>(&fs::read(meta).unwrap()).unwrap(), data);
    fs::remove_dir_all(home).unwrap();
    println!("privacy fixture: terminal vs pipe, progress.log, result prose, legacy refusals, query summary/insights, six experiment formats, recording MD/HTML; owner-only lists unchanged");
}
