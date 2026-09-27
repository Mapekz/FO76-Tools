//! The `log` backend every binary and host installs at startup.
//!
//! The library reports recoverable problems (a string table or curve source
//! that failed to load, a corrupt cache section) through `log::warn!` and keeps
//! going. Without an installed logger those calls are no-ops, so each entry
//! point calls [`init`] once before doing any work.

use log::{Level, LevelFilter, Log, Metadata, Record};

struct StderrLogger;

impl Log for StderrLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= Level::Warn
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            let label = match record.level() {
                Level::Error => "error",
                _ => "warning",
            };
            eprintln!("{label}: {}", record.args());
        }
    }

    fn flush(&self) {}
}

static LOGGER: StderrLogger = StderrLogger;

/// Route `log` warnings and errors to stderr. Idempotent: a second call, or a
/// host that already installed its own logger, leaves the existing one alone.
pub fn init() {
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(LevelFilter::Warn);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn init_is_idempotent() {
        super::init();
        super::init();
        assert_eq!(log::max_level(), log::LevelFilter::Warn);
    }
}
