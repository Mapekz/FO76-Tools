//! Query backends: in-process (`LocalBackend`) and HTTP daemon client (`RemoteBackend`).

use crate::ipc::{self, Op, RecordSel, Request, Response};
use crate::registry::Registry;
use anyhow::{Context, bail};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

const DAEMON_FILENAME: &str = "esm-daemon.json";
/// Fast 2 s deadline for `/health` and `/status` probes — a live daemon responds instantly.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(100);
const HEALTH_POLL_MAX: Duration = Duration::from_secs(30);

/// Overall budget across retries for a full logical `/op` round-trip.
///
/// Generous because the *first* `refs`/`list`/`search` against a cold daemon triggers a
/// one-time whole-ESM index build (xref, edid, search) followed by writing that index's
/// own `xref`/`edid`/`search` rkyv section — easily tens of seconds, and on the largest FO76
/// ESM snapshots comfortably past this budget's old single-attempt meaning (the xref build
/// in particular decodes every record in the file).
///
/// No longer a single HTTP call's deadline — see [`post_op`]. Each individual attempt is
/// bounded by [`op_attempt_timeout`] instead; an attempt that times out is retried, not
/// surfaced as an error, for as long as `crate::progress::read` shows a build still making
/// (non-stalled) progress on the requested ESM. This constant is the ceiling on how long
/// that retrying continues before giving up with a clear "still building" error rather than
/// ureq's opaque timeout. Override with `ESM_OP_TIMEOUT_SECS` (`0` = no ceiling — keep
/// retrying indefinitely as long as the build keeps advancing).
fn op_timeout() -> Option<Duration> {
    match std::env::var("ESM_OP_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
    {
        Some(0) => None,
        Some(n) => Some(Duration::from_secs(n)),
        None => Some(Duration::from_secs(300)),
    }
}

/// Deadline for a SINGLE `/op` HTTP attempt (see [`post_op`]'s retry loop). Short relative
/// to [`op_timeout`]'s overall budget, so a still-building daemon is detected and retried
/// quickly rather than tying up one connection for the whole budget. Override with
/// `ESM_OP_ATTEMPT_TIMEOUT_SECS` (`0` disables the per-attempt cap — not recommended, since
/// it collapses back to one long wait with no chance to check `crate::progress::read` in
/// between, reintroducing the opaque-timeout failure this two-tier scheme exists to avoid).
fn op_attempt_timeout() -> Option<Duration> {
    match std::env::var("ESM_OP_ATTEMPT_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
    {
        Some(0) => None,
        Some(n) => Some(Duration::from_secs(n)),
        None => Some(Duration::from_secs(20)),
    }
}

/// Which live build (if any) is the likely cause of `req` timing out: the request's own
/// ESM, plus `Op::Diff`'s second path when present. Mirrors `cli.rs`'s `watched_paths` for
/// the progress-watcher hook — kept as an independent copy since `backend.rs` has no
/// dependency on the CLI binary crate.
fn building_progress(req: &Request) -> Option<crate::progress::BuildProgress> {
    if let Some(p) = crate::progress::read(&req.esm) {
        return Some(p);
    }
    if let Op::Diff { b, .. } = &req.op {
        return crate::progress::read(b);
    }
    None
}

/// Default selector-count threshold for splitting `Op::RecordBulk` into multiple
/// `/op` round-trips (see [`RemoteBackend::run`]). Overridable via `ESM_BULK_CHUNK`
/// (`0` disables chunking), same env-knob shape as [`op_timeout`].
///
/// This bounds daemon *peak memory per round-trip*, not response bytes — per-record
/// payload size spans three orders of magnitude (a bare EditorID lookup vs. a
/// `--resolve full` FLST, which alone can be several MB), so no selector count is a
/// byte guarantee. The actual fix for issue #26 is lifting the response-size cap in
/// [`read_json_unlimited`]; chunking here only keeps one very large bulk `get` from
/// building its entire resolved `Vec<BulkRecordEntry>` (and its serialized JSON) in a
/// single allocation on the daemon.
fn bulk_chunk_default() -> usize {
    std::env::var("ESM_BULK_CHUNK")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(512)
}

/// Discovery file written by the daemon on start.
///
/// `exe_*` fields fingerprint the daemon binary (size + mtime of the running
/// `esm-server` executable) so clients can detect a rebuild and respawn a stale
/// daemon instead of silently querying it with an outdated schema/decoder.
/// `#[serde(default)]` lets a discovery file written by a pre-fingerprint daemon
/// still deserialize; it is then treated as not-fresh, forcing one clean respawn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonInfo {
    pub port: u16,
    pub token: String,
    pub pid: u32,
    #[serde(default)]
    pub exe_path: String,
    #[serde(default)]
    pub exe_size: u64,
    #[serde(default)]
    pub exe_mtime_secs: u64,
    #[serde(default)]
    pub exe_mtime_nanos: u32,
}

impl DaemonInfo {
    /// Build a fresh `DaemonInfo` for the currently-running process, stamping the
    /// daemon binary's own file signature (best-effort: an unreadable exe path
    /// yields an empty signature, which `daemon_fresh` always treats as stale).
    pub fn current(port: u16, token: String) -> Self {
        let (exe_path, exe_size, exe_mtime_secs, exe_mtime_nanos) = exe_sig();
        Self {
            port,
            token,
            pid: std::process::id(),
            exe_path,
            exe_size,
            exe_mtime_secs,
            exe_mtime_nanos,
        }
    }
}

/// Signature `(path, size, mtime_secs, mtime_nanos)` of the currently-running
/// executable, mirroring the mtime convention used for the ESM identity stamp
/// every `Index` rkyv section carries (`CacheSig`, see `rkyvcache.rs`/`index.rs`).
/// Returns an empty/zeroed tuple on any error so callers can
/// still start (or compare against) a daemon even when the exe can't be stat'd.
///
/// `pub` (not `pub(crate)`): the daemon binary (`src/bin/server.rs`) is a
/// separate crate that links against this library, and its idle-TTL watchdog
/// calls this directly to detect its own binary changing on disk (self-eviction).
pub fn exe_sig() -> (String, u64, u64, u32) {
    let path = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return (String::new(), 0, 0, 0),
    };
    let meta = match std::fs::metadata(&path) {
        Ok(m) => m,
        Err(_) => return (String::new(), 0, 0, 0),
    };
    let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let dur = mtime
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    (
        path.to_string_lossy().into_owned(),
        meta.len(),
        dur.as_secs(),
        dur.subsec_nanos(),
    )
}

/// Whether the daemon described by `info` is still running the exact binary it
/// was started with. `false` for any pre-fingerprint discovery file (empty
/// `exe_path`) or if the binary can no longer be stat'd at that path.
pub fn daemon_fresh(info: &DaemonInfo) -> bool {
    if info.exe_path.is_empty() {
        return false;
    }
    let meta = match std::fs::metadata(&info.exe_path) {
        Ok(m) => m,
        Err(_) => return false,
    };
    let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let dur = mtime
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    meta.len() == info.exe_size
        && dur.as_secs() == info.exe_mtime_secs
        && dur.subsec_nanos() == info.exe_mtime_nanos
}

/// Trait implemented by local and remote query backends. `run` is the whole
/// interface — callers build an [`Op`] and read the typed result back out of
/// the returned [`Value`] with `serde_json::from_value`, the same idiom
/// `cli.rs` already uses at every one of its call sites. (Previously this
/// trait also carried five convenience methods mirroring individual `Op`
/// variants; they were a `server.rs`-private helper set on a public trait —
/// one, `diff`, had zero callers, and `referenced_by` silently hardcoded
/// `sort: RefSort::Formid`, so MCP's `esm_refs` tool could never request
/// depth-ordering. Deleted rather than fixed: a narrower mirror of `Op` will
/// always eventually drop the next field `Op` gains the way it dropped
/// `sort`.)
pub trait QueryBackend {
    fn run(&mut self, esm: &Path, op: Op) -> anyhow::Result<Value>;
}

/// In-process backend backed by a [`Registry`].
pub struct LocalBackend {
    registry: Registry,
}

impl LocalBackend {
    pub fn new() -> Self {
        Self {
            registry: Registry::new(),
        }
    }
}

impl Default for LocalBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl QueryBackend for LocalBackend {
    fn run(&mut self, esm: &Path, op: Op) -> anyhow::Result<Value> {
        let req = Request {
            esm: esm.to_path_buf(),
            op,
        };
        match ipc::dispatch(&self.registry, &req) {
            Response::Ok { data } => Ok(data),
            Response::Err { error } => bail!("{}", error),
        }
    }
}

/// HTTP client for the resident daemon.
pub struct RemoteBackend {
    base_url: String,
    token: String,
    bulk_chunk: usize,
    attempt_timeout: Option<Duration>,
    overall_timeout: Option<Duration>,
}

impl RemoteBackend {
    pub fn new(addr: &str, port: u16, token: String) -> Self {
        Self {
            base_url: format!("http://{addr}:{port}"),
            token,
            bulk_chunk: bulk_chunk_default(),
            attempt_timeout: op_attempt_timeout(),
            overall_timeout: op_timeout(),
        }
    }

    /// Override [`post_op`]'s per-attempt and overall-budget deadlines. Exists so tests
    /// (and any explicit caller) can pin both without mutating process-global env — same
    /// reasoning as [`Self::with_bulk_chunk`]'s doc comment.
    pub fn with_op_timeouts(
        mut self,
        attempt: Option<Duration>,
        overall: Option<Duration>,
    ) -> Self {
        self.attempt_timeout = attempt;
        self.overall_timeout = overall;
        self
    }

    /// Override the selector-count threshold at which `Op::RecordBulk` requests are
    /// split across multiple `/op` round-trips (see [`Self::run`]). `0` disables
    /// chunking entirely. Exists so tests (and any explicit caller) can pin the
    /// threshold without mutating process-global env — `std::env::set_var` is
    /// `unsafe` under edition 2024 and races under a multithreaded `cargo test`.
    pub fn with_bulk_chunk(mut self, n: usize) -> Self {
        self.bulk_chunk = n;
        self
    }

    pub fn from_daemon_info(info: &DaemonInfo) -> Self {
        Self::new("127.0.0.1", info.port, info.token.clone())
    }

    /// Connect to a running daemon, auto-spawning one if absent. If a resident
    /// daemon is alive but stale (its binary was rebuilt since it started),
    /// `spawn_daemon_and_wait` stops it and spawns a fresh one instead of
    /// silently querying it with an outdated schema/decoder.
    pub fn connect_or_spawn() -> anyhow::Result<Self> {
        connect_or_spawn_with(&RealHost)
    }

    /// Connect with optional address/port override (skips discovery file for addr).
    /// `--port` alone defaults to `127.0.0.1`.
    pub fn connect_with_override(addr: Option<&str>, port: Option<u16>) -> anyhow::Result<Self> {
        if let Some(p) = port {
            let a = addr.unwrap_or("127.0.0.1");
            let info = read_daemon_info().ok();
            let token = info
                .as_ref()
                .filter(|i| i.port == p)
                .map(|i| i.token.clone())
                .unwrap_or_default();
            if health_check(a, p, &token).is_ok() {
                return Ok(Self::new(a, p, token));
            }
            bail!("no daemon listening on {}:{}", a, p);
        }
        Self::connect_or_spawn()
    }

    /// Connect to an already-running daemon without auto-spawning one.
    /// `--port` alone defaults to `127.0.0.1`.
    pub fn connect_existing_with_override(
        addr: Option<&str>,
        port: Option<u16>,
    ) -> anyhow::Result<Self> {
        if let Some(p) = port {
            let a = addr.unwrap_or("127.0.0.1");
            let info = read_daemon_info().ok();
            let token = info
                .as_ref()
                .filter(|i| i.port == p)
                .map(|i| i.token.clone())
                .unwrap_or_default();
            if health_check(a, p, &token).is_ok() {
                return Ok(Self::new(a, p, token));
            }
            bail!("no daemon listening on {}:{}", a, p);
        }

        let info = read_daemon_info().context("daemon is not running")?;
        if daemon_alive(&info) {
            Ok(Self::from_daemon_info(&info))
        } else {
            bail!("daemon is not running");
        }
    }

    pub fn health(&self) -> anyhow::Result<()> {
        health_check_url(&self.base_url, &self.token)
    }

    pub fn status(&self) -> anyhow::Result<Value> {
        let url = format!("{}/status", self.base_url);
        let mut resp = ureq::get(&url)
            .header("Authorization", &format!("Bearer {}", self.token))
            .config()
            .timeout_global(Some(CONNECT_TIMEOUT))
            .build()
            .call()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        read_json_unlimited(resp.body_mut())
    }

    pub fn shutdown(&self) -> anyhow::Result<()> {
        let req = Request {
            esm: PathBuf::new(),
            op: Op::Shutdown,
        };
        let _ = self.post_op(&req)?;
        Ok(())
    }

    /// Post one `/op` request, retrying a per-attempt timeout ([`op_attempt_timeout`]) for
    /// as long as [`building_progress`] shows a live, non-stalled build against the
    /// requested ESM — up to [`op_timeout`]'s overall budget. Safe to retry unconditionally:
    /// every op is read-only, and a retried request simply re-queues behind the daemon's
    /// per-ESM `Mutex<Database>` (see `registry.rs`) rather than restarting or duplicating
    /// the build itself, which keeps running server-side regardless of whether an earlier
    /// attempt's connection was abandoned.
    fn post_op(&self, req: &Request) -> anyhow::Result<Response> {
        let url = format!("{}/op", self.base_url);
        let overall_deadline = self.overall_timeout.map(|d| Instant::now() + d);
        loop {
            let request = ureq::post(&url)
                .header("Authorization", &format!("Bearer {}", self.token))
                .header("Content-Type", "application/json")
                .config()
                .timeout_global(self.attempt_timeout)
                .build();
            match request.send_json(req) {
                Ok(mut resp) => return read_json_unlimited(resp.body_mut()),
                Err(ureq::Error::Timeout(_)) => {
                    let Some(progress) = building_progress(req) else {
                        bail!(
                            "timed out waiting for daemon response ({})",
                            req.esm.display()
                        );
                    };
                    if progress.is_stalled() {
                        bail!(
                            "cache build for {} appears stalled (pid {}, last seen {:.0}% \
                             through the {} stage, no heartbeat update in over a minute) — \
                             not retrying further",
                            req.esm.display(),
                            progress.pid,
                            progress.percent(),
                            progress.stage.label(),
                        );
                    }
                    if let Some(deadline) = overall_deadline
                        && Instant::now() >= deadline
                    {
                        bail!(
                            "timed out waiting for the cache build for {} to finish (pid {}, \
                             {:.0}% through the {} stage at last check) — raise \
                             ESM_OP_TIMEOUT_SECS or wait for the build to finish",
                            req.esm.display(),
                            progress.pid,
                            progress.percent(),
                            progress.stage.label(),
                        );
                    }
                    // Still building and still advancing — retry.
                }
                Err(e) => return Err(anyhow::anyhow!("{e}")),
            }
        }
    }

    /// Split a large `Op::RecordBulk` across multiple `/op` round-trips of at most
    /// `self.bulk_chunk` selectors each, concatenating the resulting entry arrays in
    /// order. Safe because `dispatch_op` resolves `Op::RecordBulk` as
    /// `sels.iter().map(...).collect()` (see `ipc.rs`) — order-preserving, each entry
    /// self-identified by its own `sel`, each independently fallible — so N chunked
    /// requests produce byte-for-byte the same array as one request would, just spread
    /// over more round-trips. A `Response::Err` from any chunk fails the whole call,
    /// same as an unchunked request would.
    fn run_bulk_chunked(
        &self,
        esm: &Path,
        sels: &[RecordSel],
        depth: crate::ResolveDepth,
    ) -> anyhow::Result<Value> {
        let mut out = Vec::with_capacity(sels.len());
        for chunk in sels.chunks(self.bulk_chunk) {
            let req = Request {
                esm: esm.to_path_buf(),
                op: Op::RecordBulk {
                    sels: chunk.to_vec(),
                    depth,
                },
            };
            match self.post_op(&req)? {
                Response::Ok { data } => match data {
                    Value::Array(entries) => out.extend(entries),
                    other => bail!("expected RecordBulk to return a JSON array, got {other}"),
                },
                Response::Err { error } => bail!("{}", error),
            }
        }
        Ok(Value::Array(out))
    }
}

/// Deserialize a `/op` (or `/status`) response body with no size ceiling.
///
/// `Body::read_json` applies ureq's 10 MiB `MAX_BODY_SIZE`; `BodyWithConfig`'s own
/// default limit is `u64::MAX`, so routing through `.with_config()` alone lifts it —
/// see issue #26. Unbounded is correct here, not a hazard: the peer is a localhost,
/// bearer-token-gated daemon this client spawned itself, and response size is purely a
/// function of the request the caller just made. `read_json` parses via
/// `serde_json::from_reader`, so lifting the ceiling adds no buffering step — it only
/// stops refusing large-but-legitimate results.
///
/// The response body deliberately stays one JSON document rather than moving to a
/// streamed/NDJSON shape: `dispatch_op` (`ipc.rs`) materializes a `serde_json::Value`
/// for every op, and axum's `Json` responder serializes it in one `to_vec` — the daemon
/// already holds two full in-memory copies before a byte goes out, so streaming the
/// wire format wouldn't bound daemon memory without rewriting every dispatch arm into a
/// lazy/iterator model. It would also break `tools/esm_gateway.py`'s single
/// `json.loads` of the body and the tested one-JSON-document stdout contract
/// (`cli.rs`'s `one_shot_json_stdout_is_exactly_one_document`, see ADR 0002).
fn read_json_unlimited<T: serde::de::DeserializeOwned>(body: &mut ureq::Body) -> anyhow::Result<T> {
    Ok(body.with_config().read_json()?)
}

impl QueryBackend for RemoteBackend {
    fn run(&mut self, esm: &Path, op: Op) -> anyhow::Result<Value> {
        if let Op::RecordBulk { sels, depth } = &op
            && self.bulk_chunk > 0
            && sels.len() > self.bulk_chunk
        {
            return self.run_bulk_chunked(esm, sels, *depth);
        }
        let req = Request {
            esm: esm.to_path_buf(),
            op,
        };
        match self.post_op(&req)? {
            Response::Ok { data } => Ok(data),
            Response::Err { error } => bail!("{}", error),
        }
    }
}

// ─── Daemon discovery & lifecycle ───────────────────────────────────────────

pub fn runtime_dir() -> PathBuf {
    dirs::runtime_dir()
        .or_else(dirs::cache_dir)
        .unwrap_or_else(std::env::temp_dir)
}

pub fn daemon_info_path() -> PathBuf {
    runtime_dir().join(DAEMON_FILENAME)
}

pub fn read_daemon_info() -> anyhow::Result<DaemonInfo> {
    let path = daemon_info_path();
    let data =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    Ok(serde_json::from_str(&data)?)
}

pub fn write_daemon_info(info: &DaemonInfo) -> anyhow::Result<()> {
    let path = daemon_info_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string(info)?;
    std::fs::write(&path, &data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

pub fn remove_daemon_info() -> anyhow::Result<()> {
    let path = daemon_info_path();
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("getrandom");
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn daemon_alive(info: &DaemonInfo) -> bool {
    health_check("127.0.0.1", info.port, &info.token).is_ok()
}

fn health_check(addr: &str, port: u16, token: &str) -> anyhow::Result<()> {
    health_check_url(&format!("http://{addr}:{port}"), token)
}

fn health_check_url(base_url: &str, token: &str) -> anyhow::Result<()> {
    let url = format!("{base_url}/health");
    let resp = ureq::get(&url)
        .header("Authorization", &format!("Bearer {}", token))
        .config()
        .timeout_global(Some(CONNECT_TIMEOUT))
        .build()
        .call()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    if resp.status() == 200 {
        Ok(())
    } else {
        bail!("health check returned {}", resp.status());
    }
}

/// Resolve the `esm-server` binary adjacent to the current executable.
pub fn esm_server_exe() -> anyhow::Result<PathBuf> {
    let current = std::env::current_exe().context("resolve current executable")?;
    let dir = current
        .parent()
        .context("executable has no parent directory")?;
    let name = if cfg!(windows) {
        "esm-server.exe"
    } else {
        "esm-server"
    };
    let sibling = dir.join(name);
    if sibling.exists() {
        Ok(sibling)
    } else {
        bail!(
            "esm-server not found at {}; build with --features server",
            sibling.display()
        )
    }
}

// ─── DaemonHost seam ────────────────────────────────────────────────────────
//
// Separates OS-facing primitives (file lock, process spawn, HTTP health, clock)
// from the lifecycle *policy* (re-check-after-lock, stale stop-and-respawn,
// health-poll timeout) so the policy can run against a fake host in unit tests
// without real processes, sockets, or multi-second sleeps.

/// OS-facing primitives used by daemon spawn / connect / stop policy.
trait DaemonHost {
    /// Guard that releases the advisory spawn-coalescing lock on drop.
    type LockGuard;
    /// Opaque handle for a spawned daemon process.
    type Child;

    /// Acquire the advisory spawn-coalescing lock.
    fn lock_exclusive(&self) -> anyhow::Result<Self::LockGuard>;
    fn read_info(&self) -> anyhow::Result<DaemonInfo>;
    fn remove_info(&self) -> anyhow::Result<()>;
    fn health_check(&self, port: u16, token: &str) -> anyhow::Result<()>;
    /// Request a graceful `/op` shutdown from a running daemon.
    fn request_shutdown(&self, port: u16, token: &str) -> anyhow::Result<()>;
    /// Spawn the daemon process; returns a handle for [`Self::kill`] / pid checks.
    fn spawn_server(&self) -> anyhow::Result<Self::Child>;
    fn kill(&self, child: &mut Self::Child);
    /// Force-kill a process by PID (used after a graceful shutdown attempt).
    fn kill_pid(&self, pid: u32);
    fn is_pid_alive(&self, pid: u32) -> bool;
    fn now(&self) -> std::time::Instant;
    fn sleep(&self, d: Duration);
}

/// Production host: real file lock, discovery JSON, ureq health checks, processes, clock.
struct RealHost;

impl DaemonHost for RealHost {
    type LockGuard = std::fs::File;
    type Child = std::process::Child;

    fn lock_exclusive(&self) -> anyhow::Result<Self::LockGuard> {
        let lock_path = runtime_dir().join("esm-daemon.lock");
        let lock_file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("open spawn lock {}", lock_path.display()))?;
        lock_file
            .lock_exclusive()
            .context("acquire daemon spawn lock")?;
        Ok(lock_file)
    }

    fn read_info(&self) -> anyhow::Result<DaemonInfo> {
        read_daemon_info()
    }

    fn remove_info(&self) -> anyhow::Result<()> {
        remove_daemon_info()
    }

    fn health_check(&self, port: u16, token: &str) -> anyhow::Result<()> {
        health_check("127.0.0.1", port, token)
    }

    fn request_shutdown(&self, port: u16, token: &str) -> anyhow::Result<()> {
        RemoteBackend::new("127.0.0.1", port, token.to_string()).shutdown()
    }

    fn spawn_server(&self) -> anyhow::Result<Self::Child> {
        let server = esm_server_exe()?;
        let child = Command::new(&server)
            .arg("--daemon")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("spawn esm-server --daemon")?;
        Ok(child)
    }

    fn kill(&self, child: &mut Self::Child) {
        let _ = child.kill();
    }

    fn kill_pid(&self, pid: u32) {
        #[cfg(unix)]
        {
            let _ = Command::new("kill").arg(pid.to_string()).status();
        }
        #[cfg(not(unix))]
        {
            let _ = Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/F"])
                .status();
        }
    }

    fn is_pid_alive(&self, pid: u32) -> bool {
        is_pid_alive(pid)
    }

    fn now(&self) -> std::time::Instant {
        std::time::Instant::now()
    }

    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
}

/// Return a healthy, binary-fresh daemon if one is already running.
fn existing_fresh_daemon<H: DaemonHost>(host: &H) -> Option<DaemonInfo> {
    let info = host.read_info().ok()?;
    if host.health_check(info.port, &info.token).is_ok() && daemon_fresh(&info) {
        Some(info)
    } else {
        None
    }
}

fn connect_or_spawn_with<H: DaemonHost>(host: &H) -> anyhow::Result<RemoteBackend> {
    if let Some(info) = existing_fresh_daemon(host) {
        return Ok(RemoteBackend::from_daemon_info(&info));
    }
    spawn_daemon_and_wait_with(host)?;
    let info = host
        .read_info()
        .context("daemon started but discovery file missing")?;
    Ok(RemoteBackend::from_daemon_info(&info))
}

/// Spawn `esm-server --daemon` detached and poll until `/health` succeeds.
///
/// An advisory file lock (`esm-daemon.lock`) is held for the duration of the
/// spawn so that concurrent callers (parallel agents) coalesce: the first one
/// to acquire the lock performs the spawn; subsequent ones re-check the
/// discovery file after acquiring the lock and, if a healthy *and fresh*
/// daemon is already running, return immediately without spawning a second
/// instance. A daemon that's alive but stale (binary rebuilt since it
/// started) is stopped here, under the lock, before the respawn below —
/// this is what lets a resident daemon self-heal after `cargo build`.
pub fn spawn_daemon_and_wait() -> anyhow::Result<()> {
    spawn_daemon_and_wait_with(&RealHost)
}

fn spawn_daemon_and_wait_with<H: DaemonHost>(host: &H) -> anyhow::Result<()> {
    // Acquire an advisory exclusive lock for the duration of the spawn.
    // The lock is released when `_guard` is dropped (fd close / fake unlock).
    let _guard = host.lock_exclusive()?;

    // Re-check: another process may have won the race while we waited for the
    // lock.
    if let Ok(info) = host.read_info()
        && host.health_check(info.port, &info.token).is_ok()
    {
        if daemon_fresh(&info) {
            return Ok(());
        }
        // Alive but stale: stop it before spawning a replacement so the
        // fresh daemon isn't blocked from binding/registering.
        stop_running_daemon_with(host, &info);
        let _ = host.remove_info();
    }

    let mut child = host.spawn_server()?;

    // Detach: don't wait on the child (real Child::id is a no-op side-effect
    // reminder; fakes have nothing equivalent).
    let deadline = host.now() + HEALTH_POLL_MAX;
    while host.now() < deadline {
        if let Ok(info) = host.read_info()
            && host.health_check(info.port, &info.token).is_ok()
        {
            return Ok(());
        }
        host.sleep(HEALTH_POLL_INTERVAL);
    }
    host.kill(&mut child);
    bail!("daemon did not become ready within {:?}", HEALTH_POLL_MAX);
}

/// Start the daemon process (for `esm daemon start`). Respawns if the
/// resident daemon is alive but stale (binary rebuilt since it started),
/// same freshness gate as `connect_or_spawn`.
pub fn start_daemon_process() -> anyhow::Result<DaemonInfo> {
    start_daemon_process_with(&RealHost)
}

fn start_daemon_process_with<H: DaemonHost>(host: &H) -> anyhow::Result<DaemonInfo> {
    if let Some(info) = existing_fresh_daemon(host) {
        return Ok(info);
    }
    spawn_daemon_and_wait_with(host)?;
    host.read_info()
}

/// Gracefully shut down a running daemon: request `/op` shutdown, wait
/// briefly, then force-kill by PID if it's still alive. Does not touch the
/// discovery file — callers remove it themselves once they're done reading
/// `info` (e.g. `info.pid`).
fn stop_running_daemon_with<H: DaemonHost>(host: &H, info: &DaemonInfo) {
    let _ = host.request_shutdown(info.port, &info.token);
    // Give it a moment, then signal if still alive
    host.sleep(Duration::from_millis(200));
    if host.is_pid_alive(info.pid) {
        host.kill_pid(info.pid);
    }
}

/// Stop a running daemon.
pub fn stop_daemon() -> anyhow::Result<()> {
    let host = RealHost;
    if let Ok(info) = host.read_info() {
        if host.health_check(info.port, &info.token).is_ok() {
            stop_running_daemon_with(&host, &info);
        }
        let _ = host.remove_info();
    }
    Ok(())
}

fn is_pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {}", pid)])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
            .unwrap_or(false)
    }
}

/// Shared registry for the daemon (re-exported for server.rs).
pub type SharedRegistry = Arc<Registry>;

pub fn shared_registry(warm_xref: bool) -> SharedRegistry {
    Arc::new(Registry::with_warm_xref(warm_xref))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;
    use std::time::Instant;

    /// Unique path in the OS temp dir for a test fixture file, disambiguated
    /// by pid + name so parallel test runs don't collide.
    fn fixture_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("esm_backend_test_{}_{}", std::process::id(), name))
    }

    /// Build a `DaemonInfo` whose `exe_*` fields match `path`'s current
    /// on-disk signature, as `DaemonInfo::current` would if `path` were the
    /// running executable.
    fn info_for(path: &Path) -> DaemonInfo {
        let meta = std::fs::metadata(path).unwrap();
        let dur = meta
            .modified()
            .unwrap()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default();
        DaemonInfo {
            port: 0,
            token: "t".to_string(),
            pid: 0,
            exe_path: path.to_string_lossy().into_owned(),
            exe_size: meta.len(),
            exe_mtime_secs: dur.as_secs(),
            exe_mtime_nanos: dur.subsec_nanos(),
        }
    }

    #[test]
    fn daemon_fresh_true_when_sig_matches() {
        let path = fixture_path("fresh_match.bin");
        std::fs::write(&path, b"hello").unwrap();
        let info = info_for(&path);
        assert!(daemon_fresh(&info));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn daemon_fresh_false_after_file_changes() {
        let path = fixture_path("fresh_change.bin");
        std::fs::write(&path, b"hello").unwrap();
        let mut info = info_for(&path);

        // Simulate a rebuild: different size and a bumped mtime.
        std::fs::write(&path, b" world, this is longer now").unwrap();
        let future = SystemTime::now() + Duration::from_secs(120);
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_modified(future)
            .expect("set_modified should be supported on this platform");

        // The stale `info` (captured before the rewrite) must no longer match.
        assert!(!daemon_fresh(&info));

        // A freshly-captured signature must match again.
        info = info_for(&path);
        assert!(daemon_fresh(&info));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn daemon_fresh_false_for_empty_exe_path() {
        let info = DaemonInfo {
            port: 0,
            token: "t".to_string(),
            pid: 0,
            exe_path: String::new(),
            exe_size: 0,
            exe_mtime_secs: 0,
            exe_mtime_nanos: 0,
        };
        assert!(!daemon_fresh(&info));
    }

    #[test]
    fn daemon_fresh_false_for_missing_exe() {
        let info = DaemonInfo {
            port: 0,
            token: "t".to_string(),
            pid: 0,
            exe_path: "/nonexistent/path/esm-server-does-not-exist".to_string(),
            exe_size: 0,
            exe_mtime_secs: 0,
            exe_mtime_nanos: 0,
        };
        assert!(!daemon_fresh(&info));
    }

    #[test]
    fn daemon_info_serde_round_trip_with_exe_fields() {
        let info = DaemonInfo {
            port: 4321,
            token: "abc123".to_string(),
            pid: 999,
            exe_path: "/usr/local/bin/esm-server".to_string(),
            exe_size: 42,
            exe_mtime_secs: 100,
            exe_mtime_nanos: 200,
        };
        let json = serde_json::to_string(&info).unwrap();
        let back: DaemonInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(back.port, info.port);
        assert_eq!(back.token, info.token);
        assert_eq!(back.pid, info.pid);
        assert_eq!(back.exe_path, info.exe_path);
        assert_eq!(back.exe_size, info.exe_size);
        assert_eq!(back.exe_mtime_secs, info.exe_mtime_secs);
        assert_eq!(back.exe_mtime_nanos, info.exe_mtime_nanos);
    }

    #[test]
    fn daemon_info_legacy_json_deserializes_and_is_treated_as_stale() {
        // A discovery file written by a pre-fingerprint daemon has no `exe_*`
        // fields at all; `#[serde(default)]` must still let it parse, and the
        // resulting empty `exe_path` must make `daemon_fresh` reject it so a
        // legacy daemon gets one clean respawn instead of a deserialize error.
        let legacy = r#"{"port":1,"token":"x","pid":2}"#;
        let info: DaemonInfo = serde_json::from_str(legacy).unwrap();
        assert_eq!(info.port, 1);
        assert_eq!(info.token, "x");
        assert_eq!(info.pid, 2);
        assert_eq!(info.exe_path, "");
        assert!(!daemon_fresh(&info));
    }

    // ─── FakeHost + lifecycle policy tests ──────────────────────────────────

    /// Shared mutable state for one or more [`FakeHost`] handles (e.g. two
    /// simulated spawners racing for the same lock / discovery file).
    struct FakeState {
        locked: bool,
        info: Option<DaemonInfo>,
        /// Scripted `/health` outcomes, consumed FIFO. An empty queue means
        /// every check fails (drives the poll-timeout path).
        health_results: VecDeque<Result<(), String>>,
        /// When `spawn_server` runs, install this discovery info (simulating
        /// the child writing `esm-daemon.json`).
        info_on_spawn: Option<DaemonInfo>,
        /// After spawn, enqueue this many `Ok(())` health results so the poll
        /// loop can succeed without the test hand-queuing them.
        health_ok_after_spawn: usize,
        spawn_count: usize,
        kill_child_count: usize,
        kill_pid_count: usize,
        shutdown_count: usize,
        remove_info_count: usize,
        pid_alive: bool,
        clock: Instant,
    }

    struct FakeLockGuard {
        state: Rc<RefCell<FakeState>>,
    }

    impl Drop for FakeLockGuard {
        fn drop(&mut self) {
            self.state.borrow_mut().locked = false;
        }
    }

    struct FakeChild;

    #[derive(Clone)]
    struct FakeHost {
        state: Rc<RefCell<FakeState>>,
    }

    impl FakeHost {
        fn new() -> Self {
            Self {
                state: Rc::new(RefCell::new(FakeState {
                    locked: false,
                    info: None,
                    health_results: VecDeque::new(),
                    info_on_spawn: None,
                    health_ok_after_spawn: 0,
                    spawn_count: 0,
                    kill_child_count: 0,
                    kill_pid_count: 0,
                    shutdown_count: 0,
                    remove_info_count: 0,
                    pid_alive: true,
                    clock: Instant::now(),
                })),
            }
        }

        /// Second handle sharing the same in-memory OS state (two spawners).
        fn clone_handle(&self) -> Self {
            Self {
                state: Rc::clone(&self.state),
            }
        }

        fn set_info(&self, info: DaemonInfo) {
            self.state.borrow_mut().info = Some(info);
        }

        fn queue_health_ok(&self, n: usize) {
            let mut s = self.state.borrow_mut();
            for _ in 0..n {
                s.health_results.push_back(Ok(()));
            }
        }

        fn set_info_on_spawn(&self, info: DaemonInfo) {
            self.state.borrow_mut().info_on_spawn = Some(info);
        }

        fn set_health_ok_after_spawn(&self, n: usize) {
            self.state.borrow_mut().health_ok_after_spawn = n;
        }

        fn spawn_count(&self) -> usize {
            self.state.borrow().spawn_count
        }

        fn kill_child_count(&self) -> usize {
            self.state.borrow().kill_child_count
        }

        fn shutdown_count(&self) -> usize {
            self.state.borrow().shutdown_count
        }

        fn remove_info_count(&self) -> usize {
            self.state.borrow().remove_info_count
        }

        fn kill_pid_count(&self) -> usize {
            self.state.borrow().kill_pid_count
        }
    }

    impl DaemonHost for FakeHost {
        type LockGuard = FakeLockGuard;
        type Child = FakeChild;

        fn lock_exclusive(&self) -> anyhow::Result<Self::LockGuard> {
            let mut s = self.state.borrow_mut();
            // Single-threaded tests never hold the lock across a second
            // acquire; a true concurrent wait would need a condvar. Bail so a
            // mistaken re-entrant acquire fails loudly instead of deadlocking.
            if s.locked {
                bail!("fake spawn lock already held");
            }
            s.locked = true;
            drop(s);
            Ok(FakeLockGuard {
                state: Rc::clone(&self.state),
            })
        }

        fn read_info(&self) -> anyhow::Result<DaemonInfo> {
            self.state
                .borrow()
                .info
                .clone()
                .ok_or_else(|| anyhow::anyhow!("no daemon info"))
        }

        fn remove_info(&self) -> anyhow::Result<()> {
            let mut s = self.state.borrow_mut();
            s.info = None;
            s.remove_info_count += 1;
            Ok(())
        }

        fn health_check(&self, _port: u16, _token: &str) -> anyhow::Result<()> {
            let mut s = self.state.borrow_mut();
            match s.health_results.pop_front() {
                Some(Ok(())) => Ok(()),
                Some(Err(e)) => bail!("{e}"),
                None => bail!("health check failed"),
            }
        }

        fn request_shutdown(&self, _port: u16, _token: &str) -> anyhow::Result<()> {
            self.state.borrow_mut().shutdown_count += 1;
            Ok(())
        }

        fn spawn_server(&self) -> anyhow::Result<Self::Child> {
            let mut s = self.state.borrow_mut();
            s.spawn_count += 1;
            if let Some(info) = s.info_on_spawn.clone() {
                s.info = Some(info);
            }
            let n = s.health_ok_after_spawn;
            for _ in 0..n {
                s.health_results.push_back(Ok(()));
            }
            Ok(FakeChild)
        }

        fn kill(&self, _child: &mut Self::Child) {
            self.state.borrow_mut().kill_child_count += 1;
        }

        fn kill_pid(&self, _pid: u32) {
            let mut s = self.state.borrow_mut();
            s.kill_pid_count += 1;
            s.pid_alive = false;
        }

        fn is_pid_alive(&self, _pid: u32) -> bool {
            self.state.borrow().pid_alive
        }

        fn now(&self) -> Instant {
            self.state.borrow().clock
        }

        fn sleep(&self, d: Duration) {
            self.state.borrow_mut().clock += d;
        }
    }

    /// Fresh daemon info stamped against a real on-disk fixture so the
    /// untouched `daemon_fresh` returns true.
    fn fresh_info(port: u16, pid: u32) -> (DaemonInfo, PathBuf) {
        let path = fixture_path(&format!("lifecycle_fresh_{port}.bin"));
        std::fs::write(&path, b"fresh-bin").unwrap();
        let mut info = info_for(&path);
        info.port = port;
        info.token = "tok".into();
        info.pid = pid;
        (info, path)
    }

    /// Stale daemon info: empty `exe_path` makes real `daemon_fresh` return false.
    fn stale_info(port: u16, pid: u32) -> DaemonInfo {
        DaemonInfo {
            port,
            token: "tok".into(),
            pid,
            exe_path: String::new(),
            exe_size: 0,
            exe_mtime_secs: 0,
            exe_mtime_nanos: 0,
        }
    }

    #[test]
    fn recheck_after_lock_returns_without_spawning() {
        let host = FakeHost::new();
        let (info, path) = fresh_info(9001, 42);
        host.set_info(info);
        // One health Ok for the re-check-after-lock probe.
        host.queue_health_ok(1);

        spawn_daemon_and_wait_with(&host).unwrap();
        assert_eq!(host.spawn_count(), 0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn two_spawners_coalesce_to_single_spawn() {
        // First caller finds nothing and spawns; second caller (sharing state)
        // re-checks after lock and sees the healthy fresh daemon — no 2nd spawn.
        let first = FakeHost::new();
        let second = first.clone_handle();

        let (info, path) = fresh_info(9002, 43);
        first.set_info_on_spawn(info);
        first.set_health_ok_after_spawn(1);

        spawn_daemon_and_wait_with(&first).unwrap();
        assert_eq!(first.spawn_count(), 1);

        // Second spawner: discovery + health already good (post-first-spawn).
        second.queue_health_ok(1);
        spawn_daemon_and_wait_with(&second).unwrap();
        assert_eq!(
            second.spawn_count(),
            1,
            "second caller must not spawn again"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn stale_daemon_is_stopped_then_respawned() {
        let host = FakeHost::new();
        let stale = stale_info(9003, 44);
        host.set_info(stale);
        // Re-check-after-lock: alive (health ok) but stale (empty exe_path).
        host.queue_health_ok(1);

        let (fresh, path) = fresh_info(9003, 45);
        host.set_info_on_spawn(fresh);
        host.set_health_ok_after_spawn(1);
        // After graceful shutdown sleep, pid still looks alive → force kill.
        host.state.borrow_mut().pid_alive = true;

        spawn_daemon_and_wait_with(&host).unwrap();

        assert_eq!(host.shutdown_count(), 1, "stale daemon should be shut down");
        assert_eq!(host.remove_info_count(), 1);
        assert_eq!(host.kill_pid_count(), 1);
        assert_eq!(host.spawn_count(), 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn health_poll_timeout_kills_child_and_errors_fast() {
        let host = FakeHost::new();
        let clock_start = host.now();
        // Spawn succeeds but health never becomes ready (empty queue → always Err).
        let wall_start = Instant::now();
        let err = spawn_daemon_and_wait_with(&host).unwrap_err();
        let wall_elapsed = wall_start.elapsed();

        assert!(
            err.to_string().contains("did not become ready"),
            "unexpected error: {err}"
        );
        assert_eq!(host.spawn_count(), 1);
        assert_eq!(host.kill_child_count(), 1);
        // Fake clock must have advanced through the full poll window.
        assert!(
            host.now() >= clock_start + HEALTH_POLL_MAX,
            "fake clock did not reach HEALTH_POLL_MAX"
        );
        // Wall time must stay tiny — if this fails, sleep() isn't faked.
        assert!(
            wall_elapsed < Duration::from_secs(2),
            "timeout path slept for real time ({wall_elapsed:?}); clock not faked"
        );
    }
}
