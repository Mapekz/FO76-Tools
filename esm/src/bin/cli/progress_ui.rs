//! CLI-side rendering of `esm::progress`'s cache-build heartbeat: a
//! background watcher thread that polls `esm::progress::read` while a
//! `Backend::run` call is in flight, plus the pure formatting functions it
//! (and `--no-wait`/`esm cache status`) share.
//!
//! Not part of the `esm` library: `esm::progress` is the domain module every
//! process (daemon, `--local` CLI, N-API host) writes to and reads from via
//! the filesystem; a stderr TTY renderer is CLI-presentation logic that
//! those other consumers must never inherit.
//!
//! # Where this hooks in
//!
//! [`Watcher::spawn`]/[`Watcher::stop`] wrap `impl QueryBackend for
//! Backend`'s `run` method in `cli.rs` — not `dispatch_command` — because
//! every `cmd_*` function prints its result immediately after its
//! `backend.run(...)` call returns (see e.g. `cmd_info`). Wrapping `run`
//! itself means `stop()` (which blocks until the in-progress render, if
//! any, is erased) completes synchronously before control returns to
//! whichever `cmd_*` function is about to write to stdout — the only place
//! that ordering can be guaranteed without threading a stop signal through
//! every individual print call site.

use esm::progress::{self, BuildProgress, ProgressUnit};
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Delay before the watcher's first render — a warm call (~0.08s to remap
/// `tree`+`forms`) completes well inside this window, so `stop()` fires and
/// nothing is ever written to stderr. Overridable via
/// `ESM_PROGRESS_GRACE_MS` so tests can force a deterministic render
/// without a real multi-second build.
fn grace_period() -> Duration {
    env_duration_ms("ESM_PROGRESS_GRACE_MS").unwrap_or(Duration::from_millis(500))
}

/// How often the `\r`-updated TTY line refreshes once past the grace
/// period. Overridable via `ESM_PROGRESS_POLL_MS`.
fn tty_poll_interval() -> Duration {
    env_duration_ms("ESM_PROGRESS_POLL_MS").unwrap_or(Duration::from_millis(150))
}

/// How often a non-TTY (piped/redirected) run emits a plain progress line —
/// much coarser than the TTY refresh rate so captured logs (CI,
/// `tools/esm_gateway.py`) stay readable rather than one line per poll.
const PLAIN_LINE_INTERVAL: Duration = Duration::from_secs(10);

/// Bar width in character cells, fixed rather than terminal-width-adaptive
/// — no terminal-size dependency is available (no `libc`/`terminal_size`
/// crate) and `$COLUMNS` isn't reliably exported to non-interactive child
/// processes, so a real-width query isn't worth adding a dependency for.
const BAR_WIDTH: usize = 15;

fn env_duration_ms(var: &str) -> Option<Duration> {
    std::env::var(var)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_millis)
}

/// Background thread that renders `esm::progress::read`'s output to stderr
/// for as long as it's live, torn down by [`Self::stop`].
pub struct Watcher {
    signal: Arc<(Mutex<bool>, Condvar)>,
    handle: Option<JoinHandle<()>>,
}

impl Watcher {
    /// Spawn a watcher over `paths` — every ESM this call could plausibly
    /// be building against (`esm`, plus `Op::Diff`'s second path when
    /// present). Cheap to call even when nothing is building: the grace
    /// period plus `progress::read`'s "instant, never blocks" contract mean
    /// a warm call never renders anything before [`Self::stop`] tears it
    /// down.
    pub fn spawn(paths: Vec<PathBuf>) -> Self {
        let signal = Arc::new((Mutex::new(false), Condvar::new()));
        let signal_thread = Arc::clone(&signal);
        let handle = std::thread::spawn(move || run(paths, signal_thread));
        Watcher {
            signal,
            handle: Some(handle),
        }
    }

    /// Signal the watcher to stop and block until it has erased its
    /// current line (if any) and exited. Call this immediately after the
    /// wrapped `backend.run()` returns, before anything else — see the
    /// module doc for why this specific ordering is what keeps a rendered
    /// line from ever appearing above a result the caller is about to
    /// print.
    pub fn stop(mut self) {
        self.signal_stop();
        self.join();
    }

    fn signal_stop(&self) {
        let (lock, cvar) = &*self.signal;
        *lock.lock().unwrap() = true;
        cvar.notify_all();
    }

    fn join(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Safety net only: every real call site uses [`Watcher::stop`] explicitly.
/// A future call site that returns early via `?` between `spawn` and an
/// explicit `stop` still gets the thread torn down (just not necessarily
/// before the next print) rather than leaking or racing stdout at process
/// exit.
impl Drop for Watcher {
    fn drop(&mut self) {
        if self.handle.is_some() {
            self.signal_stop();
            self.join();
        }
    }
}

fn run(paths: Vec<PathBuf>, signal: Arc<(Mutex<bool>, Condvar)>) {
    let (lock, cvar) = &*signal;
    let start = Instant::now();
    let grace = grace_period();
    let is_tty = std::io::stderr().is_terminal();

    let mut rendered_width = 0usize;
    let mut announced = false;
    let mut last_plain_emit: Option<Instant> = None;

    let mut guard = lock.lock().unwrap();
    loop {
        if *guard {
            break;
        }
        let elapsed = start.elapsed();
        let wait = if elapsed < grace {
            grace - elapsed
        } else if is_tty {
            tty_poll_interval()
        } else {
            // Non-TTY still polls responsively for the stop signal; the
            // plain line itself is throttled separately to
            // PLAIN_LINE_INTERVAL below, not by this wait.
            Duration::from_millis(500)
        };
        let (g, _timeout) = cvar.wait_timeout(guard, wait).unwrap();
        guard = g;
        if *guard {
            break;
        }
        if start.elapsed() < grace {
            continue;
        }

        let Some(progress) = paths.iter().find_map(|p| progress::read(p)) else {
            if rendered_width > 0 {
                erase(rendered_width);
                rendered_width = 0;
            }
            announced = false;
            continue;
        };

        if !announced {
            eprintln!(
                "note: building index cache (started by pid {})",
                progress.pid
            );
            announced = true;
        }

        if is_tty {
            let line = render_bar_line(&progress, BAR_WIDTH);
            eprint!("\r{line}");
            let _ = std::io::stderr().flush();
            rendered_width = rendered_width.max(line.chars().count());
        } else {
            let now = Instant::now();
            let due = last_plain_emit
                .map(|t| now.duration_since(t) >= PLAIN_LINE_INTERVAL)
                .unwrap_or(true);
            if due {
                eprintln!("  {}", format_stage_summary(&progress));
                last_plain_emit = Some(now);
            }
        }
    }
    drop(guard);

    if rendered_width > 0 {
        erase(rendered_width);
    }
}

fn erase(width: usize) {
    eprint!("\r{}\r", " ".repeat(width));
    let _ = std::io::stderr().flush();
}

// ─── Pure formatting — no I/O, unit-tested directly ────────────────────────

/// `▕████████░░░░░░░▏` at `width` cells, `fraction` clamped to `[0, 1]`.
fn bar(fraction: f64, width: usize) -> String {
    let filled = ((fraction.clamp(0.0, 1.0) * width as f64).round() as usize).min(width);
    format!("▕{}{}▏", "█".repeat(filled), "░".repeat(width - filled))
}

/// `3_100_000 -> "3.1M"`, `950 -> "950"`. One decimal place past 1000.
fn humanize_count(n: u64) -> String {
    const K: f64 = 1_000.0;
    const M: f64 = K * 1_000.0;
    const G: f64 = M * 1_000.0;
    let f = n as f64;
    if f >= G {
        format!("{:.1}G", f / G)
    } else if f >= M {
        format!("{:.1}M", f / M)
    } else if f >= K {
        format!("{:.1}K", f / K)
    } else {
        n.to_string()
    }
}

/// `925_000_000 -> "925.0 MB"`, `512 -> "512 B"`. Decimal (1000-based, `MB`
/// not `MiB`) — matches how ESM file sizes are already reported elsewhere
/// in this crate's docs (`README.md`'s "~200 MiB"/"925 MB" mix
/// notwithstanding, decimal is the more common convention for a byte count
/// this large and avoids a second unit system alongside `humanize_count`).
fn humanize_bytes(n: u64) -> String {
    const KB: f64 = 1_000.0;
    const MB: f64 = KB * 1_000.0;
    const GB: f64 = MB * 1_000.0;
    let f = n as f64;
    if f >= GB {
        format!("{:.1} GB", f / GB)
    } else if f >= MB {
        format!("{:.1} MB", f / MB)
    } else if f >= KB {
        format!("{:.1} KB", f / KB)
    } else {
        format!("{n} B")
    }
}

/// `done`/`total` humanized per [`ProgressUnit`] — `"3.1M/5.6M recs"` for
/// [`ProgressUnit::Records`], `"612.0 MB/925.0 MB"` for
/// [`ProgressUnit::Bytes`].
fn humanize_done_total(done: u64, total: u64, unit: ProgressUnit) -> String {
    match unit {
        ProgressUnit::Records => {
            format!("{}/{} recs", humanize_count(done), humanize_count(total))
        }
        ProgressUnit::Bytes => {
            format!("{}/{}", humanize_bytes(done), humanize_bytes(total))
        }
    }
}

/// `Some(Duration::from_secs(80)) -> Some("~1m 20s")`. `None` in, `None`
/// out — callers decide what (if anything) to render for a missing ETA.
fn format_eta(eta: Option<Duration>) -> Option<String> {
    let secs = eta?.as_secs();
    Some(if secs < 60 {
        format!("~{secs}s")
    } else if secs < 3_600 {
        format!("~{}m {}s", secs / 60, secs % 60)
    } else {
        format!("~{}h {}m", secs / 3_600, (secs % 3_600) / 60)
    })
}

/// One-line summary shared by the non-TTY plain line, `--no-wait`'s
/// one-shot print, and `esm cache status`'s human output — e.g. `"stage
/// 4/5 (xref), 54%, eta ~1m 20s"` or `"stage 1/2 (forms), 100%,
/// writing…"`.
pub fn format_stage_summary(p: &BuildProgress) -> String {
    let counts = humanize_done_total(p.done, p.total, p.unit);
    let mut s = format!(
        "stage {}/{} ({}), {:.0}%, {}",
        p.stage_index,
        p.stage_count,
        p.stage.label(),
        p.percent(),
        counts
    );
    if p.writing {
        s.push_str(", writing…");
    } else if let Some(eta) = format_eta(p.eta()) {
        s.push_str(&format!(", eta {eta}"));
    } else if p.is_stalled() {
        s.push_str(", stalled?");
    }
    s
}

/// The TTY `\r`-updated bar line, e.g. `"  xref  ▕████████░░░░░░░▏ 54%
/// 3.1M/5.6M recs  eta ~1m 20s"`.
fn render_bar_line(p: &BuildProgress, width: usize) -> String {
    let counts = humanize_done_total(p.done, p.total, p.unit);
    let bar_str = bar(p.percent() / 100.0, width);
    let mut line = format!(
        "  {:<6} {bar_str} {:>3.0}%  {counts}",
        p.stage.label(),
        p.percent()
    );
    if p.writing {
        line.push_str("  writing…");
    } else if let Some(eta) = format_eta(p.eta()) {
        line.push_str(&format!("  eta {eta}"));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use esm::progress::BuildStage;

    fn progress(
        stage: BuildStage,
        stage_index: u8,
        stage_count: u8,
        done: u64,
        total: u64,
        unit: ProgressUnit,
    ) -> BuildProgress {
        // Round-trips through JSON so the test only depends on the public
        // `BuildProgress` shape, not any private constructor — `progress.rs`
        // keeps its timestamp fields private, so this is the only way to
        // build one from outside that module.
        serde_json::from_value(serde_json::json!({
            "pid": 4242,
            "stage": stage,
            "stage_index": stage_index,
            "stage_count": stage_count,
            "done": done,
            "total": total,
            "unit": unit,
            "writing": false,
            "started_at_unix_ms": 1_700_000_000_000u64,
            "updated_at_unix_ms": 1_700_000_010_000u64, // +10s
        }))
        .unwrap()
    }

    #[test]
    fn bar_renders_expected_fill() {
        assert_eq!(bar(0.0, 10), "▕░░░░░░░░░░▏");
        assert_eq!(bar(1.0, 10), "▕██████████▏");
        assert_eq!(bar(0.5, 10), "▕█████░░░░░▏");
        // Clamped, not panicking, on out-of-range input.
        assert_eq!(bar(-1.0, 4), "▕░░░░▏");
        assert_eq!(bar(2.0, 4), "▕████▏");
    }

    #[test]
    fn humanize_count_thresholds() {
        assert_eq!(humanize_count(0), "0");
        assert_eq!(humanize_count(950), "950");
        assert_eq!(humanize_count(3_100_000), "3.1M");
        assert_eq!(humanize_count(5_600_000), "5.6M");
        assert_eq!(humanize_count(2_500_000_000), "2.5G");
    }

    #[test]
    fn humanize_bytes_thresholds() {
        assert_eq!(humanize_bytes(512), "512 B");
        assert_eq!(humanize_bytes(925_000_000), "925.0 MB");
        assert_eq!(humanize_bytes(1_500_000_000), "1.5 GB");
    }

    #[test]
    fn format_eta_buckets() {
        assert_eq!(format_eta(None), None);
        assert_eq!(
            format_eta(Some(Duration::from_secs(45))),
            Some("~45s".into())
        );
        assert_eq!(
            format_eta(Some(Duration::from_secs(80))),
            Some("~1m 20s".into())
        );
        assert_eq!(
            format_eta(Some(Duration::from_secs(3_725))),
            Some("~1h 2m".into())
        );
    }

    #[test]
    fn format_stage_summary_includes_stage_percent_and_eta() {
        // done=3_100_000/total=5_600_000, 10s elapsed -> rate implies an ETA.
        let p = progress(
            BuildStage::Xref,
            4,
            5,
            3_100_000,
            5_600_000,
            ProgressUnit::Records,
        );
        let s = format_stage_summary(&p);
        assert!(s.starts_with("stage 4/5 (xref), 55%,"), "{s}");
        assert!(s.contains("3.1M/5.6M recs"), "{s}");
        assert!(s.contains("eta"), "{s}");
    }

    #[test]
    fn format_stage_summary_writing_overrides_eta() {
        let mut p = progress(BuildStage::Forms, 1, 2, 100, 100, ProgressUnit::Bytes);
        p.writing = true;
        let s = format_stage_summary(&p);
        assert!(s.ends_with("writing…"), "{s}");
    }

    #[test]
    fn render_bar_line_is_stderr_shaped_not_json() {
        let p = progress(BuildStage::Tree, 2, 2, 50, 100, ProgressUnit::Bytes);
        let line = render_bar_line(&p, 10);
        assert!(line.contains("tree"));
        assert!(line.contains('▕') && line.contains('▏'));
        assert!(line.contains("50%"));
        assert!(!line.starts_with('{'), "must never look like JSON output");
    }
}
