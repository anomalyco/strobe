//! Plant the same foreign name in all three local structured exceptions. Check whole artifacts,
//! rather than maintaining a list of warnings a particular renderer happens to print today.
use serde_json::{Value, json};
use std::fs;
use strobe::{
    app::{bench::ReserveTotal, query, report},
    cli::Args,
    host, validity,
};

pub fn run() {
    const NAME: &str = "foreign-canary-6c459.exe";
    const PID: &str = "96371482";
    let allowed = |s: &str| assert!(!s.contains(NAME) && !s.contains(PID), "private process in artifact");
    let mut reserve = ReserveTotal::default();
    let mut sample = host::reserve::Report::default();
    sample.moved.insert(NAME.into(), 12);
    sample.failed.insert(NAME.into(), "OS declined".into());
    reserve.add(&sample);
    let detail = validity::speed_context(&[(format!("{NAME} ({PID})"), 75.)], Some(2.), true, |s| assert!(s.contains(NAME) && s.contains(PID)));
    allowed(&detail);
    assert!(detail.contains("75%"));
    let home = std::env::temp_dir().join(format!("strobe-private-bench-{}", host::now_ns()));
    fs::create_dir(&home).unwrap();
    let file = home.join("result.json");
    let path = file.to_str().unwrap();
    for kind in ["check", "experiment"] {
        let mut r = json!({"id":"exp_private_rows","kind":kind,"plan":{"reserve":reserve,"runs_per_block":1},"variants":[{"role":"base","label":"base"},{"role":"head","label":"head"}],"warnings":[reserve.note(),validity::warn("Background",detail.clone())],"series":[{"label":"fixture","kind":"runs","passed":true,"primary":{"metric":"run.duration","verdict":"unclear","verdict_kind":"unclear"},"pairs":6,"launches":12,"discarded_bursts":[{},{}],"warnings":[validity::warn("Throttled",detail.clone())],"blocks":[{"pair":0,"variant":"base","values":{"cpu.foreign.max":75},"background":[[NAME,75]]},{"pair":0,"variant":"head","values":{"cpu.foreign.max":0}}]}]});
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
        fs::write(&file, r.to_string()).unwrap();
        let query = Args::parse(vec!["query".into(), path.into(), "summary".into()]).unwrap();
        let t = query::run(&query).unwrap().text;
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
    fs::remove_dir_all(home).unwrap();
    println!("foreign-name fixture: result prose, query, MD/HTML/PR/summary/card/badge; structured local names unchanged; 12 kept + 4 set aside");
}
