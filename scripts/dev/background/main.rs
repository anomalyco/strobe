// Manual calibration and optional log replay (no strobe process, CPU load or external services):
// cargo run --release --manifest-path scripts/dev/background/Cargo.toml -- [recorded-log-directory]
mod cases;
mod followups;
mod surfaces;
fn main() {
    if std::env::args().nth(1).as_deref() == Some("--history-privacy") {
        surfaces::history();
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("--diagnostic-stream") {
        surfaces::diagnostic_stream();
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("--child-mask") {
        println!("{}", strobe::host::topology().all());
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("--hold-terminal") {
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        return;
    }
    if let Some(file) = std::env::args().nth(2).filter(|_| std::env::args().nth(1).as_deref() == Some("--summary")) {
        let a = strobe::cli::Args::parse(vec!["query".into(), file, "summary".into()]).unwrap();
        print!("{}", strobe::app::query::run(&a).unwrap().text);
        return;
    }
    followups::run();
    surfaces::run();
    cases::run(std::env::args_os().nth(1).as_deref().map(std::path::Path::new));
}
