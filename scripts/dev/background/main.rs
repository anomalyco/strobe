// Manual calibration and optional log replay (no strobe process, CPU load or external services):
// cargo run --release --features dev-background --example background -- [recorded-log-directory]
fn main() {
    strobe::app::bench::background_dev::run(std::env::args_os().nth(1).as_deref().map(std::path::Path::new));
}
