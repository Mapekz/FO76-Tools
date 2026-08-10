//! Multi-ESM registry: lazily opens and caches [`Database`] instances.

use crate::Database;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// Cached file identity used to detect stale in-memory databases.
#[derive(Clone, Debug, PartialEq, Eq)]
struct FileSig {
    size: u64,
    mtime: SystemTime,
}

impl FileSig {
    fn read(path: &Path) -> anyhow::Result<Self> {
        let meta = std::fs::metadata(path)?;
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        Ok(FileSig {
            size: meta.len(),
            mtime,
        })
    }

    fn matches(&self, other: &FileSig) -> bool {
        self.size == other.size && self.mtime == other.mtime
    }
}

// ─── RegistryHost seam ──────────────────────────────────────────────────────
//
// Separates the two OS-facing primitives `get_or_open_with_key` needs (read a
// file's identity, open a database at a path) from the caching *policy*
// (stale-eviction, the double-open race guard, warm-on-open) so the policy
// can run against a fake host in unit tests — no real ESM on disk, no mtime
// granularity luck. Mirrors `backend.rs`'s `DaemonHost` seam; this is the
// second adapter pair in the crate, not a novel pattern.

/// OS-facing primitives used by [`Registry`]'s caching policy.
trait RegistryHost: Send + Sync {
    fn read_sig(&self, path: &Path) -> anyhow::Result<FileSig>;
    fn open(&self, path: &Path) -> anyhow::Result<Database>;
}

/// Production host: real `fs::metadata` + real `Database::open`.
struct RealHost;

impl RegistryHost for RealHost {
    fn read_sig(&self, path: &Path) -> anyhow::Result<FileSig> {
        FileSig::read(path)
    }

    fn open(&self, path: &Path) -> anyhow::Result<Database> {
        Database::open(path)
    }
}

/// One resident ESM: its disk signature + the live `Database` handle.
struct Resident {
    sig: FileSig,
    db: Arc<Mutex<Database>>,
}

/// Lazily opened ESM databases keyed by canonical path.
pub struct Registry {
    inner: Mutex<HashMap<PathBuf, Resident>>,
    /// When true, eagerly build the edid + search indexes on open (daemon behaviour).
    auto_warm: bool,
    /// When true, also eagerly build the xref index (slow, opt-in).
    pub warm_xref: bool,
    host: Box<dyn RegistryHost>,
}

impl Registry {
    /// New registry without auto-warm — used by `LocalBackend` for short-lived processes.
    /// Lazy indexes are still built on demand when an op needs them.
    pub fn new() -> Self {
        Self::with_host(RealHost, false, false)
    }

    /// New registry for the daemon: auto-warms edid + search on open, and optionally xref.
    pub fn with_warm_xref(warm_xref: bool) -> Self {
        Self::with_host(RealHost, true, warm_xref)
    }

    fn with_host(host: impl RegistryHost + 'static, auto_warm: bool, warm_xref: bool) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            auto_warm,
            warm_xref,
            host: Box::new(host),
        }
    }

    /// Canonicalize `path`, open the ESM if not already cached (or if the
    /// on-disk file changed), warm indexes, and return a shared handle.
    ///
    /// On a cache **hit**, this does one `fs::metadata` call to check whether
    /// the file's size or mtime changed.  If they differ, the stale entry is
    /// dropped and the ESM is re-opened transparently.
    ///
    /// The outer map lock is held only long enough to fetch or insert the
    /// `Arc`; the inner `Database` lock is acquired afterward so different ESMs
    /// never serialize on each other.
    pub fn get_or_open(&self, path: &Path) -> anyhow::Result<Arc<Mutex<Database>>> {
        Ok(self.get_or_open_with_key(path)?.1)
    }

    /// Like [`Self::get_or_open`], but also returns the canonical cache key.
    pub fn get_or_open_with_key(
        &self,
        path: &Path,
    ) -> anyhow::Result<(PathBuf, Arc<Mutex<Database>>)> {
        // Resolve folder → ESM before canonicalizing so the cache key and the
        // FileSig track the ESM file, not a directory.  Resolving a file input
        // is idempotent and costs one `is_dir` stat. `resolve_esm_path` is the
        // one place this two-step resolution lives — `BuildLease`/`progress::read`
        // callers must key off this exact same canonical path (see its doc comment).
        let canonical = crate::discover::resolve_esm_path(path)?;

        // Check for a live, non-stale entry.
        if let Some(resident) = {
            let map = self.inner.lock().unwrap();
            map.get(&canonical).map(|r| (r.sig.clone(), r.db.clone()))
        } {
            let (cached_sig, arc) = resident;
            // One stat per call — negligible; mostly defensive.
            let current_sig = self.host.read_sig(&canonical)?;
            if cached_sig.matches(&current_sig) {
                // Still fresh: warm indexes if needed and return.
                self.warm_indexes(&arc)?;
                return Ok((canonical, arc));
            }
            // File changed (size or mtime differs): evict and fall through to re-open.
            log::warn!(
                "esm-daemon: ESM at {} changed on disk; re-opening.",
                canonical.display()
            );
            self.inner.lock().unwrap().remove(&canonical);
        }

        let sig = self.host.read_sig(&canonical)?;
        let db = self.host.open(&canonical)?;
        let opened = Arc::new(Mutex::new(db));

        let arc = {
            let mut map = self.inner.lock().unwrap();
            // Guard against a race: another thread may have opened while we did.
            if let Some(r) = map.get(&canonical) {
                r.db.clone()
            } else {
                map.insert(
                    canonical.clone(),
                    Resident {
                        sig,
                        db: opened.clone(),
                    },
                );
                opened
            }
        };

        self.warm_indexes(&arc)?;

        Ok((canonical, arc))
    }

    /// Before Stage C, `ensure_xref_index` needed several of `Database`'s
    /// OTHER fields (`esm`/`schema`/`is_localized`/`localization`/`curves`)
    /// handed back in as parameters, which meant this function had to
    /// destructure `Database` field-by-field to borrow `index` mutably
    /// alongside the rest immutably. Now that the three `ensure_*_index`
    /// methods live on `Database` itself (`lib.rs`) and take no extra
    /// parameters, each call below is just `db.method()?` — no destructure
    /// needed, since there's no longer a second field to borrow alongside
    /// the one being built.
    fn warm_indexes(&self, db_arc: &Arc<Mutex<Database>>) -> anyhow::Result<()> {
        if !self.auto_warm && !self.warm_xref {
            return Ok(());
        }
        let mut db = db_arc.lock().unwrap();
        db.ensure_edid_index()?;
        db.ensure_search_index()?;
        if self.warm_xref {
            db.ensure_xref_index()?;
        }
        Ok(())
    }

    /// List resident ESM paths and their record counts (for daemon status).
    pub fn list_resident(&self) -> Vec<ResidentInfo> {
        let map = self.inner.lock().unwrap();
        map.iter()
            .map(|(path, r)| {
                let db = r.db.lock().unwrap();
                ResidentInfo {
                    path: path.clone(),
                    record_count: db.index.len(),
                }
            })
            .collect()
    }

    /// Drop all cached databases (used on daemon shutdown).
    pub fn clear(&self) {
        self.inner.lock().unwrap().clear();
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

/// Summary of one ESM held in the registry.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ResidentInfo {
    pub path: PathBuf,
    pub record_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

    /// A minimal valid ESM: just the 24-byte TES4 header, no records. Cheap
    /// to open repeatedly and sufficient for `Database::open` to succeed —
    /// `RegistryHost::open`'s only job in these tests is to be counted, not
    /// to exercise decoding.
    fn minimal_esm_bytes() -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"TES4");
        buf.extend_from_slice(&0u32.to_le_bytes()); // data_size
        buf.extend_from_slice(&0u32.to_le_bytes()); // flags
        buf.extend_from_slice(&0u32.to_le_bytes()); // form_id
        buf.extend_from_slice(&0u32.to_le_bytes()); // vcs1
        buf.extend_from_slice(&0u16.to_le_bytes()); // form_version
        buf.extend_from_slice(&0u16.to_le_bytes()); // vcs2
        buf
    }

    /// A real, stable temp `.esm` file `RegistryHost::open`'s test double can
    /// point `Database::open` at. `resolve_esm_path` canonicalizes its input,
    /// which requires the file to actually exist — the *contents* being
    /// stale or the mtime being wrong is what `read_sig` fakes independent
    /// of this file's real, unchanging identity.
    fn temp_esm_path() -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "fo76_esm_registry_test_{}_{n}.esm",
            std::process::id()
        ));
        std::fs::write(&path, minimal_esm_bytes()).expect("write temp esm");
        path
    }

    fn sig(n: u64) -> FileSig {
        // Distinct `size` is enough to make `FileSig::matches` see two
        // scripted signatures as different — the mtime need not vary.
        FileSig {
            size: n,
            mtime: SystemTime::UNIX_EPOCH,
        }
    }

    /// Test double for [`RegistryHost`]: hands back a scripted queue of
    /// signatures (so eviction is driven by the test, not by real mtime
    /// granularity) and opens a real `Database` from a tiny synthetic file
    /// while counting calls (via a shared counter the test holds onto
    /// directly, since `Registry` boxes its host as `dyn RegistryHost` with
    /// no downcast). An optional barrier lets a test force two threads'
    /// `open()` calls to overlap, exercising the double-open race guard in
    /// `get_or_open_with_key` deterministically instead of by luck.
    struct FakeHost {
        sigs: Mutex<VecDeque<FileSig>>,
        esm_path: PathBuf,
        open_count: Arc<AtomicUsize>,
        open_barrier: Option<Arc<Barrier>>,
    }

    impl FakeHost {
        /// Returns the host plus a shared handle on its open counter.
        fn new(sigs: Vec<FileSig>) -> (Self, Arc<AtomicUsize>) {
            let open_count = Arc::new(AtomicUsize::new(0));
            let host = Self {
                sigs: Mutex::new(sigs.into()),
                esm_path: temp_esm_path(),
                open_count: Arc::clone(&open_count),
                open_barrier: None,
            };
            (host, open_count)
        }

        fn with_barrier(sigs: Vec<FileSig>, barrier: Arc<Barrier>) -> (Self, Arc<AtomicUsize>) {
            let (mut host, open_count) = Self::new(sigs);
            host.open_barrier = Some(barrier);
            (host, open_count)
        }
    }

    impl Drop for FakeHost {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.esm_path);
        }
    }

    impl RegistryHost for FakeHost {
        fn read_sig(&self, _path: &Path) -> anyhow::Result<FileSig> {
            self.sigs
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| anyhow::anyhow!("FakeHost: no more scripted signatures"))
        }

        fn open(&self, _path: &Path) -> anyhow::Result<Database> {
            self.open_count.fetch_add(1, Ordering::SeqCst);
            if let Some(b) = &self.open_barrier {
                b.wait();
            }
            Database::open(&self.esm_path)
        }
    }

    // ─── Warm policy ────────────────────────────────────────────────────
    //
    // `Registry::new()` and `Registry::with_warm_xref(_)` imply two
    // different implicit warming policies (see `warm_indexes`) with nothing
    // previously asserting either — see the architecture review this seam
    // was added for.

    #[test]
    fn new_registry_does_not_auto_warm() {
        let r = Registry::new();
        assert!(!r.auto_warm);
        assert!(!r.warm_xref);
    }

    #[test]
    fn with_warm_xref_false_still_auto_warms_edid_and_search() {
        // `auto_warm` is unconditionally true for the daemon constructor —
        // `warm_xref` only gates the *third*, slower index.
        let r = Registry::with_warm_xref(false);
        assert!(r.auto_warm);
        assert!(!r.warm_xref);
    }

    #[test]
    fn with_warm_xref_true_warms_everything() {
        let r = Registry::with_warm_xref(true);
        assert!(r.auto_warm);
        assert!(r.warm_xref);
    }

    // ─── Stale eviction ─────────────────────────────────────────────────
    //
    // The daemon's advertised "stale-evicts if the ESM changes on disk — no
    // manual restart needed" behaviour (CLAUDE.md), previously untestable
    // without a real ESM on disk, a real write, and mtime-granularity luck.

    #[test]
    fn stale_signature_evicts_and_reopens_with_a_new_arc() {
        // Call 1 (cold): one read_sig to seed the cache with sig(1).
        // Call 2 (hit): recheck read_sig returns sig(2) != cached sig(1) ->
        // evict -> a second, fresh read_sig (sig(2) again) for the reopen.
        let (host, _open_count) = FakeHost::new(vec![sig(1), sig(2), sig(2)]);
        let esm_path = host.esm_path.clone();
        let registry = Registry::with_host(host, false, false);

        let first = registry.get_or_open(&esm_path).expect("first open");
        let second = registry.get_or_open(&esm_path).expect("second open");

        assert!(
            !Arc::ptr_eq(&first, &second),
            "a changed signature must evict and produce a fresh handle"
        );
    }

    #[test]
    fn matching_signature_reuses_the_cached_arc_without_reopening() {
        let (host, _open_count) = FakeHost::new(vec![sig(1), sig(1), sig(1)]);
        let esm_path = host.esm_path.clone();
        let registry = Registry::with_host(host, false, false);

        let first = registry.get_or_open(&esm_path).expect("first open");
        let second = registry.get_or_open(&esm_path).expect("second open");
        let third = registry.get_or_open(&esm_path).expect("third open");

        assert!(Arc::ptr_eq(&first, &second));
        assert!(Arc::ptr_eq(&second, &third));
    }

    #[test]
    fn stale_eviction_only_opens_once_per_generation() {
        // Call 1 (cold): read_sig -> sig(1), open #1.
        // Call 2 (hit): recheck read_sig -> sig(1), matches, no reopen.
        // Call 3 (hit): recheck read_sig -> sig(2), mismatch -> evict ->
        //   fresh read_sig -> sig(2), open #2.
        // Call 4 (hit): recheck read_sig -> sig(2), matches, no reopen.
        let (host, open_count) = FakeHost::new(vec![sig(1), sig(1), sig(2), sig(2), sig(2)]);
        let esm_path = host.esm_path.clone();
        let registry = Registry::with_host(host, false, false);

        registry.get_or_open(&esm_path).unwrap();
        registry.get_or_open(&esm_path).unwrap();
        let after_evict = registry.get_or_open(&esm_path).unwrap();
        let after_evict_again = registry.get_or_open(&esm_path).unwrap();

        assert!(Arc::ptr_eq(&after_evict, &after_evict_again));
        // Two opens total: the initial one and the one triggered by sig(2).
        assert_eq!(open_count.load(Ordering::SeqCst), 2);
    }

    // ─── Double-open race guard ─────────────────────────────────────────
    //
    // Two threads racing `get_or_open` on a cold cache must end up sharing
    // one `Arc` — the "Guard against a race" branch in
    // `get_or_open_with_key`. A barrier forces both threads' `open()` calls
    // to overlap so the race window is hit deterministically rather than by
    // scheduling luck.

    #[test]
    fn concurrent_cold_opens_share_one_arc() {
        let barrier = Arc::new(Barrier::new(2));
        let (host, open_count) = FakeHost::with_barrier(vec![sig(1), sig(1)], Arc::clone(&barrier));
        let esm_path = host.esm_path.clone();
        let registry = Arc::new(Registry::with_host(host, false, false));

        let r1 = Arc::clone(&registry);
        let p1 = esm_path.clone();
        let t1 = std::thread::spawn(move || r1.get_or_open(&p1).expect("thread 1 open"));

        let r2 = Arc::clone(&registry);
        let p2 = esm_path.clone();
        let t2 = std::thread::spawn(move || r2.get_or_open(&p2).expect("thread 2 open"));

        let a = t1.join().unwrap();
        let b = t2.join().unwrap();

        assert!(
            Arc::ptr_eq(&a, &b),
            "both racing callers must end up sharing one Database handle"
        );
        // Both threads genuinely opened (that's what the barrier proved they
        // overlapped on) — the race guard's job is to make exactly one of
        // those two `Database`s the one every caller ends up holding, which
        // `Arc::ptr_eq` above already confirms.
        assert_eq!(open_count.load(Ordering::SeqCst), 2);
    }
}
