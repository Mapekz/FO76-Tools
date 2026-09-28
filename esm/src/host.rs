//! The databases one process has open, and the one entry point every surface
//! runs [`Op`]s through.
//!
//! A [`Host`] opens each ESM on first use, keyed by its canonical path, and
//! reopens it if the file changes on disk. The CLI holds one for a single
//! command, `esm batch` for the life of its stdin, and the napi addon for the
//! life of an open database.

use crate::Database;
use crate::ops::Op;
use serde_json::Value;
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

// ─── Opener seam ──────────────────────────────────────────────────────
//
// Separates the two OS-facing primitives `open_with_key` needs (read a file's
// identity, open a database at a path) from the caching *policy*
// (stale-eviction, the double-open race guard) so the policy can run against a
// fake opener in unit tests — no real ESM on disk, no mtime granularity luck.

/// OS-facing primitives used by [`Host`]'s caching policy.
trait Opener: Send + Sync {
    fn read_sig(&self, path: &Path) -> anyhow::Result<FileSig>;
    fn open(&self, path: &Path) -> anyhow::Result<Database>;
}

/// Production host: real `fs::metadata` + real `Database::open`.
struct RealOpener;

impl Opener for RealOpener {
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
    db: Arc<Database>,
}

/// Lazily opened ESM databases keyed by canonical path.
pub struct Host {
    inner: Mutex<HashMap<PathBuf, Resident>>,
    opener: Box<dyn Opener>,
}

impl Host {
    pub fn new() -> Self {
        Self::with_opener(RealOpener)
    }

    fn with_opener(opener: impl Opener + 'static) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            opener: Box::new(opener),
        }
    }

    /// Run `op` against the ESM at `esm`, opening it first if needed.
    /// `Op::Diff` opens its second ESM the same way.
    pub fn run(&self, esm: &Path, op: &Op) -> anyhow::Result<Value> {
        match op {
            Op::Diff(args) => {
                let db_a = self.open(esm)?;
                let db_b = self.open(&args.b)?;
                Ok(serde_json::to_value(crate::ops::diff(&db_a, &db_b, args)?)?)
            }
            _ => crate::ops::run(&*self.open(esm)?, op),
        }
    }

    /// Canonicalize `path` and return its open database, opening it if this
    /// host hasn't yet or if the file's size or mtime changed since.
    pub fn open(&self, path: &Path) -> anyhow::Result<Arc<Database>> {
        Ok(self.open_with_key(path)?.1)
    }

    /// Like [`Self::open`], but also returns the canonical path it is keyed by.
    pub fn open_with_key(&self, path: &Path) -> anyhow::Result<(PathBuf, Arc<Database>)> {
        let canonical = crate::discover::resolve_esm_path(path)?;

        let resident = {
            let map = self.lock();
            map.get(&canonical).map(|r| (r.sig.clone(), r.db.clone()))
        };
        if let Some((cached_sig, db)) = resident {
            if cached_sig.matches(&self.opener.read_sig(&canonical)?) {
                return Ok((canonical, db));
            }
            log::warn!("{} changed on disk; reopening", canonical.display());
            self.lock().remove(&canonical);
        }

        let sig = self.opener.read_sig(&canonical)?;
        let opened = Arc::new(self.opener.open(&canonical)?);
        let db = self
            .lock()
            .entry(canonical.clone())
            .or_insert(Resident { sig, db: opened })
            .db
            .clone();
        Ok((canonical, db))
    }

    /// Forget the database for `path`; a later [`Self::open`] reopens it.
    /// Callers still holding its `Arc` keep using it until they drop it.
    pub fn close(&self, path: &Path) -> anyhow::Result<()> {
        let canonical = crate::discover::resolve_esm_path(path)?;
        self.lock().remove(&canonical);
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<PathBuf, Resident>> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Default for Host {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

    /// A minimal valid ESM: just the 24-byte TES4 header, no records. Cheap
    /// to open repeatedly and sufficient for `Database::open` to succeed —
    /// `Opener::open`'s only job in these tests is to be counted, not
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

    /// A real, stable temp `.esm` file `Opener::open`'s test double can
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

    /// Test double for [`Opener`]: hands back a scripted queue of
    /// signatures (so eviction is driven by the test, not by real mtime
    /// granularity) and opens a real `Database` from a tiny synthetic file
    /// while counting calls (via a shared counter the test holds onto
    /// directly, since `Host` boxes it as `dyn Opener` with no downcast). An
    /// optional barrier lets a test force two threads' `open()` calls to
    /// overlap, exercising the double-open race guard in `open_with_key`
    /// deterministically instead of by luck.
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

    impl Opener for FakeHost {
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

    // ─── Stale eviction ─────────────────────────────────────────────────
    //
    // A long-lived host (`esm batch`, napi) reopens an ESM that changed on
    // disk. The `FakeHost` seam
    // below tests it deterministically, without a real ESM on disk, a real
    // write, or mtime-granularity luck.

    #[test]
    fn stale_signature_evicts_and_reopens_with_a_new_arc() {
        // Call 1 (cold): one read_sig to seed the cache with sig(1).
        // Call 2 (hit): recheck read_sig returns sig(2) != cached sig(1) ->
        // evict -> a second, fresh read_sig (sig(2) again) for the reopen.
        let (host, _open_count) = FakeHost::new(vec![sig(1), sig(2), sig(2)]);
        let esm_path = host.esm_path.clone();
        let host = Host::with_opener(host);

        let first = host.open(&esm_path).expect("first open");
        let second = host.open(&esm_path).expect("second open");

        assert!(
            !Arc::ptr_eq(&first, &second),
            "a changed signature must evict and produce a fresh handle"
        );
    }

    #[test]
    fn matching_signature_reuses_the_cached_arc_without_reopening() {
        let (host, _open_count) = FakeHost::new(vec![sig(1), sig(1), sig(1)]);
        let esm_path = host.esm_path.clone();
        let host = Host::with_opener(host);

        let first = host.open(&esm_path).expect("first open");
        let second = host.open(&esm_path).expect("second open");
        let third = host.open(&esm_path).expect("third open");

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
        let host = Host::with_opener(host);

        host.open(&esm_path).unwrap();
        host.open(&esm_path).unwrap();
        let after_evict = host.open(&esm_path).unwrap();
        let after_evict_again = host.open(&esm_path).unwrap();

        assert!(Arc::ptr_eq(&after_evict, &after_evict_again));
        // Two opens total: the initial one and the one triggered by sig(2).
        assert_eq!(open_count.load(Ordering::SeqCst), 2);
    }

    // ─── Double-open race guard ─────────────────────────────────────────
    //
    // Two threads racing `open` on a cold cache must end up sharing one
    // `Arc` — the `entry().or_insert` in `open_with_key`. A barrier forces both threads' `open()` calls
    // to overlap so the race window is hit deterministically rather than by
    // scheduling luck.

    #[test]
    fn concurrent_cold_opens_share_one_arc() {
        let barrier = Arc::new(Barrier::new(2));
        let (host, open_count) = FakeHost::with_barrier(vec![sig(1), sig(1)], Arc::clone(&barrier));
        let esm_path = host.esm_path.clone();
        let host = Arc::new(Host::with_opener(host));

        let r1 = Arc::clone(&host);
        let p1 = esm_path.clone();
        let t1 = std::thread::spawn(move || r1.open(&p1).expect("thread 1 open"));

        let r2 = Arc::clone(&host);
        let p2 = esm_path.clone();
        let t2 = std::thread::spawn(move || r2.open(&p2).expect("thread 2 open"));

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
