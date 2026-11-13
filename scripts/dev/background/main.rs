// Manual calibration and optional log replay (no strobe process, CPU load or external services):
// cargo run --release --manifest-path scripts/dev/background/Cargo.toml -- [recorded-log-directory]
mod cases;
mod followups;
fn main() {
    if std::env::args().nth(1).as_deref() == Some("--child-mask") {
        println!("{}", strobe::host::topology().all());
        return;
    }
    followups::run();
    cases::run(std::env::args_os().nth(1).as_deref().map(std::path::Path::new));
}
