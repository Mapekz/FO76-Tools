//! Query backends: in-process (`LocalBackend`) and HTTP daemon client (`RemoteBackend`).

use crate::ipc::{self, Op, Request, Response};
use crate::registry::Registry;
use crate::SearchField;
use anyhow::{bail, Context};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

const DAEMON_FILENAME: &str = "esm-daemon.json";
/// Fast 2 s deadline for `/health` and `/status` probes — a live daemon responds instantly.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(100);
const HEALTH_POLL_MAX: Duration = Duration::from_secs(30);

/// Deadline for a full `/op` round-trip.
///
/// Generous because the *first* `refs`/`list`/`search` against a cold daemon triggers a
/// one-time whole-ESM index build (xref, edid, search) followed by a full re-serialisation
/// of the ~280 MiB `.esm.idx` cache — easily tens of seconds on a full FO76 ESM.
///
/// Override with `ESM_OP_TIMEOUT_SECS` (set to `0` for no deadline).
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
/// executable, mirroring the mtime convention used for the `.esm.idx` cache
/// (see `index.rs`). Returns an empty/zeroed tuple on any error so callers can
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

/// Trait implemented by local and remote query backends.
pub trait QueryBackend {
    fn run(&mut self, esm: &Path, op: Op) -> anyhow::Result<Value>;

    fn file_info(&mut self, esm: &Path) -> anyhow::Result<crate::reader::FileInfo> {
        let v = self.run(esm, Op::FileInfo)?;
        Ok(serde_json::from_value(v)?)
    }

    fn search(
        &mut self,
        esm: &Path,
        pattern: &str,
        types: Vec<String>,
        field: SearchField,
        limit: usize,
    ) -> anyhow::Result<Vec<crate::RecordRow>> {
        let v = self.run(
            esm,
            Op::Search {
                pattern: pattern.to_string(),
                types,
                field,
                limit,
            },
        )?;
        Ok(serde_json::from_value(v)?)
    }

    #[allow(clippy::too_many_arguments)]
    fn referenced_by(
        &mut self,
        esm: &Path,
        sel: ipc::RecordSel,
        limit: usize,
        depth: usize,
        type_filter: Option<String>,
        paths: bool,
    ) -> anyhow::Result<ipc::RefList> {
        let v = self.run(
            esm,
            Op::ReferencedBy {
                sel,
                limit,
                depth,
                type_filter,
                paths,
            },
        )?;
        Ok(serde_json::from_value(v)?)
    }

    fn list_by_type(
        &mut self,
        esm: &Path,
        sig: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<crate::ListEntry>> {
        let v = self.run(
            esm,
            Op::ListByType {
                sig: sig.to_string(),
                limit,
            },
        )?;
        Ok(serde_json::from_value(v)?)
    }

    fn diff(
        &mut self,
        esm_a: &Path,
        esm_b: &Path,
        record_type: Option<String>,
        options: crate::diff::DiffOptions,
    ) -> anyhow::Result<crate::DiffResult> {
        let v = self.run(
            esm_a,
            Op::Diff {
                b: esm_b.to_path_buf(),
                record_type,
                options,
            },
        )?;
        Ok(serde_json::from_value(v)?)
    }
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
}

impl RemoteBackend {
    pub fn new(addr: &str, port: u16, token: String) -> Self {
        Self {
            base_url: format!("http://{addr}:{port}"),
            token,
        }
    }

    pub fn from_daemon_info(info: &DaemonInfo) -> Self {
        Self::new("127.0.0.1", info.port, info.token.clone())
    }

    /// Connect to a running daemon, auto-spawning one if absent. If a resident
    /// daemon is alive but stale (its binary was rebuilt since it started),
    /// `spawn_daemon_and_wait` stops it and spawns a fresh one instead of
    /// silently querying it with an outdated schema/decoder.
    pub fn connect_or_spawn() -> anyhow::Result<Self> {
        if let Ok(info) = read_daemon_info() {
            if daemon_alive(&info) && daemon_fresh(&info) {
                return Ok(Self::from_daemon_info(&info));
            }
        }
        spawn_daemon_and_wait()?;
        let info = read_daemon_info().context("daemon started but discovery file missing")?;
        Ok(Self::from_daemon_info(&info))
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
        let resp = ureq::get(&url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .timeout(CONNECT_TIMEOUT)
            .call()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(resp.into_json()?)
    }

    pub fn shutdown(&self) -> anyhow::Result<()> {
        let req = Request {
            esm: PathBuf::new(),
            op: Op::Shutdown,
        };
        let _ = self.post_op(&req)?;
        Ok(())
    }

    fn post_op(&self, req: &Request) -> anyhow::Result<Response> {
        let url = format!("{}/op", self.base_url);
        let mut request = ureq::post(&url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Content-Type", "application/json");
        if let Some(t) = op_timeout() {
            request = request.timeout(t);
        }
        let resp = request.send_json(req).map_err(|e| anyhow::anyhow!("{e}"))?;
        let response: Response = resp.into_json()?;
        Ok(response)
    }
}

impl QueryBackend for RemoteBackend {
    fn run(&mut self, esm: &Path, op: Op) -> anyhow::Result<Value> {
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
    getrandom::getrandom(&mut bytes).expect("getrandom");
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
        .set("Authorization", &format!("Bearer {}", token))
        .timeout(CONNECT_TIMEOUT)
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
    // Acquire an advisory exclusive lock for the duration of the spawn.
    // The lock file is created if absent and automatically released when
    // `lock_file` is dropped (fd close).
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

    // Re-check: another process may have won the race while we waited for the
    // lock.
    if let Ok(info) = read_daemon_info() {
        if health_check("127.0.0.1", info.port, &info.token).is_ok() {
            if daemon_fresh(&info) {
                return Ok(());
            }
            // Alive but stale: stop it before spawning a replacement so the
            // fresh daemon isn't blocked from binding/registering.
            stop_running_daemon(&info);
            let _ = remove_daemon_info();
        }
    }

    let server = esm_server_exe()?;
    let mut child = Command::new(&server)
        .arg("--daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("spawn esm-server --daemon")?;

    // Detach: don't wait on the child.
    let _ = child.id();

    let deadline = std::time::Instant::now() + HEALTH_POLL_MAX;
    while std::time::Instant::now() < deadline {
        if let Ok(info) = read_daemon_info() {
            if health_check("127.0.0.1", info.port, &info.token).is_ok() {
                return Ok(());
            }
        }
        std::thread::sleep(HEALTH_POLL_INTERVAL);
    }
    let _ = child.kill();
    bail!("daemon did not become ready within {:?}", HEALTH_POLL_MAX);
}

/// Start the daemon process (for `esm daemon start`). Respawns if the
/// resident daemon is alive but stale (binary rebuilt since it started),
/// same freshness gate as `connect_or_spawn`.
pub fn start_daemon_process() -> anyhow::Result<DaemonInfo> {
    if let Ok(info) = read_daemon_info() {
        if daemon_alive(&info) && daemon_fresh(&info) {
            return Ok(info);
        }
    }
    spawn_daemon_and_wait()?;
    read_daemon_info()
}

/// Gracefully shut down a running daemon: request `/op shutdown`, wait
/// briefly, then force-kill by PID if it's still alive. Does not touch the
/// discovery file — callers remove it themselves once they're done reading
/// `info` (e.g. `info.pid`).
fn stop_running_daemon(info: &DaemonInfo) {
    let backend = RemoteBackend::from_daemon_info(info);
    let _ = backend.shutdown();
    // Give it a moment, then signal if still alive
    std::thread::sleep(Duration::from_millis(200));
    if is_pid_alive(info.pid) {
        #[cfg(unix)]
        {
            let _ = Command::new("kill").arg(info.pid.to_string()).status();
        }
        #[cfg(not(unix))]
        {
            let _ = Command::new("taskkill")
                .args(["/PID", &info.pid.to_string(), "/F"])
                .status();
        }
    }
}

/// Stop a running daemon.
pub fn stop_daemon() -> anyhow::Result<()> {
    if let Ok(info) = read_daemon_info() {
        if daemon_alive(&info) {
            stop_running_daemon(&info);
        }
        let _ = remove_daemon_info();
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
}
