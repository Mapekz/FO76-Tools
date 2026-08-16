//! Cross-process cache-build progress: a per-ESM advisory build lock plus an
//! atomically-published JSON heartbeat, both living alongside the five rkyv
//! sections inside `esm_cache/` (see `rkyvcache.rs`'s module doc for that
//! directory's layout).
//!
//! # Why the filesystem, not the daemon
//!
//! A cold `Index::build`/`ensure_*_index` call can block for tens of seconds
//! to minutes on a full FO76 ESM, with nothing to show for it. Any process
//! that hits this — the daemon, a `--local` CLI invocation, or the N-API
//! host — needs a way to (a) publish what it's doing and (b) let a second
//! process notice and wait on the SAME build instead of starting a
//! redundant one. A daemon HTTP endpoint would only cover the daemon path;
//! `--local`/N-API callers and `tools/esm_gateway.py` would stay blind, and
//! a second `--local` process couldn't dedup against a building daemon at
//! all. Publishing to a well-known sidecar file next to the cache itself
//! covers every caller uniformly with no IPC, and — critically — stays
//! answerable even while the daemon's own per-ESM `Mutex<Database>` is held
//! for the whole build (see `registry.rs`), which is exactly the case a
//! caller most wants visibility into.
//!
//! # On-disk files
//!
//! Two files per ESM, named like the five rkyv sections
//! (`rkyvcache::section_path_for`) but suffixed `build.lock`/`build.json`
//! instead of a [`crate::rkyvcache::SectionKind`]:
//!
//! - `<esm file name>.build.lock` — an advisory `fs2` exclusive lock, held
//!   for the duration of a build. Whoever holds it is the one true builder;
//!   the OS releases it automatically on process exit, including a crash,
//!   so "is a build in flight" reduces to "can I take this lock right now"
//!   with no separate liveness/heartbeat-staleness protocol needed for that
//!   question specifically (see [`read`]).
//! - `<esm file name>.build.json` — the heartbeat: stage, done/total, pid,
//!   timestamps. Published via write-to-unique-temp-file + `fs::rename`
//!   (the same publish pattern `rkyvcache::write_section` uses), so a
//!   concurrent reader never observes a torn document.
//!
//! # Callers
//!
//! [`BuildLease::acquire`] is called internally by each of `index.rs`'s four
//! build entry points (`Index::build`'s `build_tree_and_forms` path, plus
//! `ensure_edid_index`/`ensure_search_index`/`ensure_xref_index`) — no
//! public signature there changes. [`read`] is the client-facing query used
//! by the CLI's progress watcher, `--no-wait`, and `esm cache status`.

use crate::rkyvcache::{cache_dir_for, unique_tmp_path};
use anyhow::Context;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Minimum interval between heartbeat file rewrites. [`BuildLease::tick`] is
/// called once per record during a full ESM walk (up to ~5.6M times on the
/// current FO76 ESM), so its common-case cost must be a single
/// `Instant::elapsed()` comparison, not a syscall — only every
/// `HEARTBEAT_INTERVAL` does a tick actually touch the filesystem.
const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(200);

/// A heartbeat with no update in this long is reported as stalled by
/// [`BuildProgress::is_stalled`] — the builder is still alive (it holds the
/// lock, or [`read`] would already report `None`) but has stopped making
/// progress, e.g. wedged inside a single very large record.
const STALL_TIMEOUT: Duration = Duration::from_secs(60);

/// Disables heartbeat *publishing* only — not the lock itself, since dedup
/// (a second process seeing the lock held and skipping a redundant rebuild)
/// is the valuable half of this module and stays on regardless. Meant for
/// hosts where a stray file write is undesirable, e.g. the N-API/Electron
/// embedding. Same naming convention as `ESM_CACHE_VERIFY`/`ESM_BULK_CHUNK`/
/// `ESM_OP_TIMEOUT_SECS` (`backend.rs`).
const NO_PROGRESS_ENV: &str = "ESM_NO_PROGRESS";

fn progress_disabled() -> bool {
    std::env::var(NO_PROGRESS_ENV).as_deref() == Ok("1")
}

/// One phase of an `Index` cache rebuild — the SAME five variants
/// [`crate::rkyvcache::SectionKind`] uses (that name is a crate-private
/// alias for this type, `pub(crate) use BuildStage as SectionKind` in
/// `rkyvcache.rs`), so a section's on-disk kind discriminant, its
/// `cache_inventory` bucket, and its build-progress heartbeat stage are all
/// one enum rather than three hand-paired copies. `#[repr(u32)]` with
/// explicit discriminants: this doubles as the on-disk `section_kind` field
/// `rkyvcache`'s 64-byte header stores (see that module's layout table), so
/// the numeric values are load-bearing on-disk identity, not just an
/// internal implementation detail — kept at their pre-unification values
/// (`Tree = 1` etc.) so this refactor doesn't force a cache rebuild.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u32)]
pub enum BuildStage {
    Tree = 1,
    Forms = 2,
    Edid = 3,
    Search = 4,
    Xref = 5,
}

impl BuildStage {
    /// Lowercase label used in rendered progress lines and `cache status` output.
    pub fn label(self) -> &'static str {
        match self {
            BuildStage::Forms => "forms",
            BuildStage::Tree => "tree",
            BuildStage::Edid => "edid",
            BuildStage::Search => "search",
            BuildStage::Xref => "xref",
        }
    }

    /// Which unit this stage's `done`/`total` are counted in — see
    /// [`ProgressUnit`]'s doc comment for why this is fixed per stage rather
    /// than a builder choice.
    fn unit(self) -> ProgressUnit {
        match self {
            BuildStage::Forms | BuildStage::Tree | BuildStage::Xref => ProgressUnit::Bytes,
            BuildStage::Edid | BuildStage::Search => ProgressUnit::Records,
        }
    }

    /// All five stages, in the fixed order [`crate::index::cache_inventory`]
    /// reports them.
    pub const ALL: [BuildStage; 5] = [
        BuildStage::Forms,
        BuildStage::Tree,
        BuildStage::Edid,
        BuildStage::Search,
        BuildStage::Xref,
    ];
}

/// What a [`BuildProgress`]'s `done`/`total` count.
///
/// Fixed per [`BuildStage`], not chosen freely, so the reported bar never
/// runs backwards:
///
/// - **Forms / Tree / Xref** — byte offset into the ESM. `walk_records`/
///   `walk_structure` both hand their closure a monotonically increasing
///   `offset`, and the ESM's total byte length is free (`esm.data().len()`,
///   no pre-count needed).
/// - **Edid / Search** — record count. These iterate `Index::iter_all`,
///   which walks the *FormID-sorted* forms table, not file offset order —
///   byte progress would oscillate. `Index::len()` is already known before
///   the loop starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressUnit {
    Bytes,
    Records,
}

/// A snapshot of a live build's heartbeat, as published to
/// `<esm>.build.json` and read back by [`read`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildProgress {
    pub pid: u32,
    pub stage: BuildStage,
    /// 1-based, for a "stage N/M" label.
    pub stage_index: u8,
    /// How many stages *this* build will run — varies by command (`get`
    /// only needs forms+tree; `refs` adds xref), so this is not always 5.
    pub stage_count: u8,
    pub done: u64,
    pub total: u64,
    pub unit: ProgressUnit,
    /// True once the counting pass has finished and the section is now
    /// being serialized and fsynced to disk. `done` pins at `total` during
    /// this — the renderer uses this flag to say "writing…" instead of
    /// sitting at 100% with no further ticks.
    pub writing: bool,
    started_at_unix_ms: u64,
    updated_at_unix_ms: u64,
}

impl BuildProgress {
    pub fn percent(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            (self.done as f64 / self.total as f64 * 100.0).min(100.0)
        }
    }

    /// Estimated remaining time in the *current stage*, extrapolated from
    /// this stage's own elapsed-time-so-far vs. `done`/`total`. Per-stage,
    /// not a whole-build ETA — see the module doc on [`BuildStage`] for why
    /// a single global ETA isn't meaningful here (stage cost varies by more
    /// than an order of magnitude, and which stages run varies by command).
    ///
    /// `None` before there's enough signal to extrapolate from (`done ==
    /// 0`), once the stage has already finished counting (`writing`), or if
    /// the clock is somehow non-monotonic across the read.
    pub fn eta(&self) -> Option<Duration> {
        if self.writing || self.done == 0 || self.total == 0 || self.done >= self.total {
            return None;
        }
        let elapsed = self.updated_at().duration_since(self.started_at()).ok()?;
        let elapsed_secs = elapsed.as_secs_f64();
        if elapsed_secs <= 0.0 {
            return None;
        }
        let rate = self.done as f64 / elapsed_secs;
        if rate <= 0.0 {
            return None;
        }
        let remaining_secs = (self.total - self.done) as f64 / rate;
        Some(Duration::from_secs_f64(remaining_secs.max(0.0)))
    }

    pub fn started_at(&self) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(self.started_at_unix_ms)
    }

    pub fn updated_at(&self) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(self.updated_at_unix_ms)
    }

    /// No heartbeat update in over [`STALL_TIMEOUT`]. Distinct from "no
    /// builder at all" ([`read`] returning `None`) — the lock is still held
    /// (the process is alive), it has just stopped making progress, e.g.
    /// stuck decoding one abnormally large record.
    pub fn is_stalled(&self) -> bool {
        SystemTime::now()
            .duration_since(self.updated_at())
            .map(|age| age >= STALL_TIMEOUT)
            .unwrap_or(false)
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Path to `<esm>.build.lock` / `<esm>.build.json`, mirroring
/// `rkyvcache::section_path_for`'s naming but for these two sidecar files
/// rather than a [`crate::rkyvcache::SectionKind`].
fn build_sidecar_path(esm_path: &Path, suffix: &str) -> anyhow::Result<PathBuf> {
    let file_name = esm_path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("esm path has no file name: {}", esm_path.display()))?;
    let mut name = file_name.to_os_string();
    name.push(".");
    name.push(suffix);
    Ok(cache_dir_for(esm_path)?.join(name))
}

fn lock_path(esm_path: &Path) -> anyhow::Result<PathBuf> {
    build_sidecar_path(esm_path, "build.lock")
}

fn heartbeat_path(esm_path: &Path) -> anyhow::Result<PathBuf> {
    build_sidecar_path(esm_path, "build.json")
}

/// Held by the process actively rebuilding one or more cache sections for an
/// ESM. Publishes a heartbeat while alive (unless [`NO_PROGRESS_ENV`] is
/// set); [`Drop`] removes the heartbeat file. The advisory lock itself
/// releases on process exit regardless of how the process ends, including a
/// crash — nothing but process death is required for [`read`] to see the
/// build as over.
pub struct BuildLease {
    esm_path: PathBuf,
    // Held only so the lock releases (via `fs2`'s `Drop`-on-close semantics
    // and OS-level advisory-lock-releases-on-exit) when this struct drops;
    // never read directly again after `acquire`.
    _lock_file: fs::File,
    stage: BuildStage,
    stage_index: u8,
    stage_count: u8,
    total: u64,
    done: u64,
    writing: bool,
    started_at_unix_ms: u64,
    last_write: Instant,
    publish: bool,
}

/// Outcome of [`BuildLease::acquire_or_recheck`] — the enforced form of the
/// acquire-then-recheck protocol [`BuildLease::acquire`]'s doc comment used
/// to just ask callers to remember by hand (four call sites in `index.rs`
/// did, identically, before this existed). Structured so the only way to
/// obtain a live [`BuildLease`] at all is through the [`Self::NeedsBuild`]
/// arm, which is only reachable once the caller-supplied recheck closure has
/// already run and reported the section still missing — skipping the
/// recheck is not merely discouraged, there is no code path that does.
pub enum Acquired<T> {
    /// Another process finished building and published the section while
    /// this call was blocked waiting for the lock — here it is, already
    /// mapped. The lease was acquired only long enough to run the recheck,
    /// then dropped without this caller building anything.
    AlreadyBuilt(T),
    /// The section is still missing after the recheck; this caller holds
    /// the lease and must build it.
    NeedsBuild(BuildLease),
}

impl BuildLease {
    /// Block until the per-ESM build lock is ours, then start publishing a
    /// heartbeat for `stage` (the first of `stage_count` stages this build
    /// will run).
    ///
    /// The lock is per-ESM, not per-section: a builder mid-`xref` blocks a
    /// second process that only wants `edid`. This lock's job is dedup, not
    /// fine-grained parallelism — per-section locking would let the two
    /// fight over the same mmap'd ESM and CPU for no real concurrency
    /// benefit.
    ///
    /// Low-level primitive — **every caller MUST re-check whether the
    /// section it wanted now exists immediately after this returns**,
    /// since another process may have built and published it while this
    /// call was blocked waiting for the lock. Prefer
    /// [`Self::acquire_or_recheck`], which makes that recheck structurally
    /// impossible to skip instead of relying on this doc comment; this
    /// method stays `pub` only because [`Self::acquire_or_recheck`] and this
    /// module's own tests are built directly on it.
    pub fn acquire(
        esm_path: &Path,
        stage: BuildStage,
        stage_index: u8,
        stage_count: u8,
        total: u64,
    ) -> anyhow::Result<Self> {
        Self::acquire_with_publish(
            esm_path,
            stage,
            stage_index,
            stage_count,
            total,
            !progress_disabled(),
        )
    }

    /// [`Self::acquire`], plus the mandatory recheck fused into one call so
    /// it cannot be forgotten: acquires the lock, then immediately runs
    /// `recheck` (typically a `Section::map` against the just-acquired
    /// lock's guarantee that no writer is mid-publish) and returns
    /// [`Acquired::AlreadyBuilt`] if it now reports the section present —
    /// dropping the lease without ever handing it to the caller — or
    /// [`Acquired::NeedsBuild`] with the live lease if the caller must build
    /// it after all.
    pub fn acquire_or_recheck<T>(
        esm_path: &Path,
        stage: BuildStage,
        stage_index: u8,
        stage_count: u8,
        total: u64,
        recheck: impl FnOnce() -> anyhow::Result<Option<T>>,
    ) -> anyhow::Result<Acquired<T>> {
        let lease = Self::acquire(esm_path, stage, stage_index, stage_count, total)?;
        if let Some(already) = recheck()? {
            return Ok(Acquired::AlreadyBuilt(already));
        }
        Ok(Acquired::NeedsBuild(lease))
    }

    /// Like [`Self::acquire`], but pins whether the heartbeat is published
    /// rather than reading it from [`NO_PROGRESS_ENV`] — lets tests pin the
    /// behavior without mutating process-global env, which races under a
    /// multithreaded `cargo test` (same reasoning as
    /// `RemoteBackend::with_bulk_chunk`'s doc comment in `backend.rs`).
    fn acquire_with_publish(
        esm_path: &Path,
        stage: BuildStage,
        stage_index: u8,
        stage_count: u8,
        total: u64,
        publish: bool,
    ) -> anyhow::Result<Self> {
        let lock_path = lock_path(esm_path)?;
        if let Some(parent) = lock_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create cache directory {}", parent.display()))?;
        }
        let lock_file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("open build lock {}", lock_path.display()))?;
        lock_file
            .lock_exclusive()
            .context("acquire cache build lock")?;

        let mut lease = BuildLease {
            esm_path: esm_path.to_path_buf(),
            _lock_file: lock_file,
            stage,
            stage_index,
            stage_count,
            total,
            done: 0,
            writing: false,
            started_at_unix_ms: now_unix_ms(),
            // Force the very first `tick`/`publish_now` to actually publish
            // rather than waiting a full `HEARTBEAT_INTERVAL`.
            last_write: Instant::now() - HEARTBEAT_INTERVAL,
            publish,
        };
        lease.publish_now();
        Ok(lease)
    }

    /// Move to a new stage on an already-held lock — used by the
    /// `forms`-then-`tree` build, which runs both under one lease rather
    /// than releasing and reacquiring the lock between them. Resets
    /// `done`/`writing`/`started_at` for the new stage, so [`Self::eta`]-
    /// adjacent math (actually [`BuildProgress::eta`]) always extrapolates
    /// from the current stage alone.
    pub fn begin_stage(&mut self, stage: BuildStage, stage_index: u8, total: u64) {
        self.stage = stage;
        self.stage_index = stage_index;
        self.total = total;
        self.done = 0;
        self.writing = false;
        self.started_at_unix_ms = now_unix_ms();
        self.last_write = Instant::now() - HEARTBEAT_INTERVAL;
        self.publish_now();
    }

    /// Record progress. Hot path — called once per record across a full ESM
    /// walk (up to ~5.6M times) — so the common case is one
    /// `Instant::elapsed()` comparison and nothing else; only every
    /// [`HEARTBEAT_INTERVAL`] does this touch the filesystem.
    #[inline]
    pub fn tick(&mut self, done: u64) {
        self.done = done;
        if self.last_write.elapsed() >= HEARTBEAT_INTERVAL {
            self.publish_now();
        }
    }

    /// Mark the counting pass finished and the section now being
    /// serialized/fsynced to disk. Pins `done` at `total`.
    pub fn writing(&mut self) {
        self.done = self.total;
        self.writing = true;
        self.publish_now();
    }

    fn publish_now(&mut self) {
        self.last_write = Instant::now();
        if !self.publish {
            return;
        }
        // A failed heartbeat write must never fail the build itself —
        // best-effort, errors silently dropped.
        let _ = self.write_heartbeat();
    }

    fn write_heartbeat(&self) -> anyhow::Result<()> {
        let progress = BuildProgress {
            pid: std::process::id(),
            stage: self.stage,
            stage_index: self.stage_index,
            stage_count: self.stage_count,
            done: self.done,
            total: self.total,
            unit: self.stage.unit(),
            writing: self.writing,
            started_at_unix_ms: self.started_at_unix_ms,
            updated_at_unix_ms: now_unix_ms(),
        };
        let path = heartbeat_path(&self.esm_path)?;
        let tmp = unique_tmp_path(&path)?;
        let json = serde_json::to_vec(&progress).context("serialize build heartbeat")?;
        {
            let mut f =
                fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
            f.write_all(&json)
                .with_context(|| format!("write {}", tmp.display()))?;
            f.sync_data()
                .with_context(|| format!("sync {}", tmp.display()))?;
        }
        fs::rename(&tmp, &path)
            .with_context(|| format!("rename {} to {}", tmp.display(), path.display()))?;
        Ok(())
    }
}

impl Drop for BuildLease {
    fn drop(&mut self) {
        if let Ok(path) = heartbeat_path(&self.esm_path) {
            let _ = fs::remove_file(&path);
        }
        // The lock itself releases when `_lock_file` drops immediately
        // after this, or — on a crash that skips `Drop` entirely — when the
        // OS reclaims the process's file descriptors on exit. Either way, a
        // stale heartbeat left behind by that crash path (this `drop` never
        // ran) is cleaned up by the next [`read`] or [`BuildLease::acquire`]
        // call to observe the lock as free.
    }
}

/// Read the live build heartbeat for `esm_path`, if any. **Never blocks.**
///
/// `Some` iff a live process currently holds the build lock — determined by
/// a non-blocking `try_lock_exclusive` on the lock file, never by the
/// heartbeat file's mere presence (a crash between `BuildLease::acquire`
/// and a clean `Drop` can leave one behind with no live writer). If the
/// lock is free, any such stale heartbeat is removed here before returning
/// `None`, so a caller never has to distinguish "no build" from "an old
/// build's leftovers" itself.
pub fn read(esm_path: &Path) -> Option<BuildProgress> {
    let lock_path = lock_path(esm_path).ok()?;
    let lock_file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .ok()?;
    match lock_file.try_lock_exclusive() {
        Ok(()) => {
            // No builder alive: release immediately (we only took the lock
            // to test it), clean up a stale heartbeat if one exists, and
            // report "no build in flight".
            let _ = lock_file.unlock();
            if let Ok(hb_path) = heartbeat_path(esm_path) {
                let _ = fs::remove_file(&hb_path);
            }
            None
        }
        Err(_) => {
            // Held by a live process — read the heartbeat it published. A
            // missing or unparseable heartbeat (e.g. read mid-first-publish
            // before any write has landed, vanishingly unlikely given the
            // rename-based publish, or a builder running with
            // ESM_NO_PROGRESS=1) degrades to `None` rather than a stale
            // guess — callers already treat `None` as "nothing to show".
            let hb_path = heartbeat_path(esm_path).ok()?;
            let data = fs::read(&hb_path).ok()?;
            serde_json::from_slice(&data).ok()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_esm_path(name: &str) -> PathBuf {
        let pid = std::process::id();
        let nonce = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("esm_progress_test_{name}_{pid}_{nonce}.esm"))
    }

    fn cleanup(esm_path: &Path) {
        let _ = fs::remove_file(lock_path(esm_path).unwrap());
        let _ = fs::remove_file(heartbeat_path(esm_path).unwrap());
    }

    #[test]
    fn read_is_none_with_no_lock_file_at_all() {
        let esm = test_esm_path("no_lock");
        assert!(read(&esm).is_none());
        cleanup(&esm);
    }

    #[test]
    fn acquire_publishes_a_readable_heartbeat() {
        let esm = test_esm_path("publishes");
        let lease = BuildLease::acquire(&esm, BuildStage::Forms, 1, 2, 1000).unwrap();

        let progress = read(&esm).expect("a live lease must be visible to read()");
        assert_eq!(progress.pid, std::process::id());
        assert_eq!(progress.stage, BuildStage::Forms);
        assert_eq!(progress.stage_index, 1);
        assert_eq!(progress.stage_count, 2);
        assert_eq!(progress.total, 1000);
        assert_eq!(progress.done, 0);
        assert!(!progress.writing);
        assert_eq!(progress.unit, ProgressUnit::Bytes);

        drop(lease);
        cleanup(&esm);
    }

    #[test]
    fn read_is_none_after_lease_drops() {
        let esm = test_esm_path("drops");
        let lease = BuildLease::acquire(&esm, BuildStage::Edid, 1, 1, 10).unwrap();
        assert!(read(&esm).is_some());
        drop(lease);
        assert!(
            read(&esm).is_none(),
            "dropping the lease must release the lock and clear the heartbeat"
        );
        cleanup(&esm);
    }

    #[test]
    fn tick_within_throttle_window_does_not_republish() {
        let esm = test_esm_path("throttle");
        let mut lease = BuildLease::acquire(&esm, BuildStage::Xref, 1, 1, 100).unwrap();
        // `acquire` already published once (done=0) and reset `last_write`
        // to "now" — this tick lands well inside HEARTBEAT_INTERVAL, so it
        // must update the in-memory value but NOT rewrite the file.
        lease.tick(42);
        let progress = read(&esm).unwrap();
        assert_eq!(
            progress.done, 0,
            "a tick inside the throttle window must not rewrite the heartbeat"
        );
        drop(lease);
        cleanup(&esm);
    }

    #[test]
    fn tick_after_throttle_window_elapses_republishes() {
        let esm = test_esm_path("tick_after_interval");
        let mut lease = BuildLease::acquire(&esm, BuildStage::Xref, 1, 1, 100).unwrap();
        std::thread::sleep(HEARTBEAT_INTERVAL + Duration::from_millis(50));
        lease.tick(42);
        let progress = read(&esm).unwrap();
        assert_eq!(progress.done, 42);
        drop(lease);
        cleanup(&esm);
    }

    #[test]
    fn begin_stage_resets_done_and_writing() {
        let esm = test_esm_path("begin_stage");
        let mut lease = BuildLease::acquire(&esm, BuildStage::Forms, 1, 2, 500).unwrap();
        lease.tick(500);
        lease.writing();
        assert!(read(&esm).unwrap().writing);

        lease.begin_stage(BuildStage::Tree, 2, 300);
        let progress = read(&esm).unwrap();
        assert_eq!(progress.stage, BuildStage::Tree);
        assert_eq!(progress.stage_index, 2);
        assert_eq!(progress.total, 300);
        assert_eq!(progress.done, 0);
        assert!(!progress.writing);

        drop(lease);
        cleanup(&esm);
    }

    #[test]
    fn writing_pins_done_to_total() {
        let esm = test_esm_path("writing");
        let mut lease = BuildLease::acquire(&esm, BuildStage::Search, 1, 1, 777).unwrap();
        lease.writing();
        let progress = read(&esm).unwrap();
        assert_eq!(progress.done, 777);
        assert!(progress.writing);
        assert!(
            progress.eta().is_none(),
            "no ETA once the counting pass is done and writing has started"
        );
        drop(lease);
        cleanup(&esm);
    }

    #[test]
    fn no_progress_disables_heartbeat_but_not_the_lock() {
        // Uses `acquire_with_publish(publish: false)` directly rather than
        // setting `ESM_NO_PROGRESS` for real — mutating process-global env
        // races under a multithreaded `cargo test` (see
        // `acquire_with_publish`'s doc comment).
        let esm = test_esm_path("no_progress");
        let lease =
            BuildLease::acquire_with_publish(&esm, BuildStage::Forms, 1, 1, 10, false).unwrap();
        // The lock is still held (dedup still works)...
        assert!(
            read(&esm).is_none(),
            "no heartbeat file means read() sees nothing to show, but the lock is still held — verified below"
        );
        // ...verified directly: a second attempt to lock would block.
        let lock_file = fs::OpenOptions::new()
            .write(true)
            .open(lock_path(&esm).unwrap())
            .unwrap();
        assert!(
            lock_file.try_lock_exclusive().is_err(),
            "lock must still be held even though no heartbeat was published"
        );
        drop(lease);
        cleanup(&esm);
    }

    #[test]
    fn percent_and_eta_edge_cases() {
        let mut p = BuildProgress {
            pid: 1,
            stage: BuildStage::Forms,
            stage_index: 1,
            stage_count: 1,
            done: 0,
            total: 0,
            unit: ProgressUnit::Bytes,
            writing: false,
            started_at_unix_ms: 0,
            updated_at_unix_ms: 0,
        };
        assert_eq!(p.percent(), 0.0, "total == 0 must not divide by zero");
        assert!(p.eta().is_none());

        p.total = 100;
        p.done = 0;
        assert_eq!(p.percent(), 0.0);
        assert!(p.eta().is_none(), "no ETA before any progress is made");

        p.done = 50;
        p.started_at_unix_ms = 0;
        p.updated_at_unix_ms = 10_000; // 10s elapsed, 50/100 done -> ~10s remaining
        assert_eq!(p.percent(), 50.0);
        let eta = p
            .eta()
            .expect("eta available once done > 0 and elapsed > 0");
        assert!(
            (eta.as_secs_f64() - 10.0).abs() < 0.5,
            "expected ~10s remaining, got {eta:?}"
        );

        p.done = 100;
        assert_eq!(p.percent(), 100.0);
        assert!(p.eta().is_none(), "no ETA once done >= total");
    }

    #[test]
    fn is_stalled_reflects_updated_at_age() {
        let fresh = BuildProgress {
            pid: 1,
            stage: BuildStage::Forms,
            stage_index: 1,
            stage_count: 1,
            done: 1,
            total: 10,
            unit: ProgressUnit::Bytes,
            writing: false,
            started_at_unix_ms: now_unix_ms(),
            updated_at_unix_ms: now_unix_ms(),
        };
        assert!(!fresh.is_stalled());

        let stale = BuildProgress {
            updated_at_unix_ms: now_unix_ms().saturating_sub(STALL_TIMEOUT.as_millis() as u64 + 1),
            ..fresh
        };
        assert!(stale.is_stalled());
    }

    #[test]
    fn build_stage_all_matches_label_set() {
        let labels: Vec<&str> = BuildStage::ALL.iter().map(|s| s.label()).collect();
        assert_eq!(labels, vec!["forms", "tree", "edid", "search", "xref"]);
    }

    #[test]
    fn heartbeat_json_round_trips() {
        let p = BuildProgress {
            pid: 4242,
            stage: BuildStage::Xref,
            stage_index: 5,
            stage_count: 5,
            done: 3_100_000,
            total: 5_600_000,
            unit: ProgressUnit::Records,
            writing: false,
            started_at_unix_ms: 1_700_000_000_000,
            updated_at_unix_ms: 1_700_000_080_000,
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: BuildProgress = serde_json::from_str(&json).unwrap();
        assert_eq!(back.pid, p.pid);
        assert_eq!(back.stage, p.stage);
        assert_eq!(back.done, p.done);
        assert_eq!(back.total, p.total);
        assert_eq!(back.unit, p.unit);
    }
}
