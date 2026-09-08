use log::{Level, LevelFilter, set_max_level};
use std::io::Write;

pub(crate) fn setup_logging(verbose: bool, quiet: bool) {
    let level = if verbose {
        LevelFilter::Debug
    } else if quiet {
        LevelFilter::Warn
    } else {
        LevelFilter::Info
    };
    set_max_level(level);
    log::set_logger(&ConsoleLogger).expect("logger already set");
}

struct ConsoleLogger;

impl log::Log for ConsoleLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        if metadata.target().starts_with("ai::") || metadata.target() == "ai" {
            metadata.level() <= log::max_level()
        } else {
            metadata.level() <= Level::Warn
        }
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            crate::output::stderr_line(&format!("{}", record.args()));
        }
    }

    fn flush(&self) {
        let _ = std::io::stderr().flush();
    }
}

pub(crate) fn is_quiet() -> bool {
    log::max_level() <= LevelFilter::Warn
}
