use log::{debug, info};
use tracing_subscriber::EnvFilter;

fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_target(false)
        .with_level(false)
        .with_line_number(false)
        .with_file(false)
        .without_time()
        .with_ansi(true)
        .with_env_filter(EnvFilter::new("test_ansi=debug"))
        .init();

    info!("\x1b[31mRED TEXT\x1b[0m normal");
    info!("\x1b[34mBLUE TEXT\x1b[0m normal");
    debug!("\x1b[90mGREY TEXT\x1b[0m");
    info!("😀 unicode emoji works");
}
