//! Query backends: in-process (`LocalBackend`) and HTTP daemon client (`RemoteBackend`).

use crate::ipc::{self, Op, Request, Response};
use crate::registry::Registry;
use crate::SearchField;
use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

const DAEMON_FILENAME: &str = "esm-daemon.json";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(100);
const HEALTH_POLL_MAX: Duration = Duration::from_secs(30);

/// Discovery file written by the daemon on start.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonInfo {
    pub port: u16,
    pub token: String,
    pub pid: u32,
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

    fn referenced_by(
        &mut self,
        esm: &Path,
        sel: ipc::RecordSel,
        limit: usize,
    ) -> anyhow::Result<ipc::RefList> {
        let v = self.run(esm, Op::ReferencedBy { sel, limit })?;
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
    ) -> anyhow::Result<crate::DiffResult> {
        let v = self.run(
            esm_a,
            Op::Diff {
                b: esm_b.to_path_buf(),
                record_type,
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

    /// Connect to a running daemon, auto-spawning one if absent.
    pub fn connect_or_spawn() -> anyhow::Result<Self> {
        if let Ok(info) = read_daemon_info() {
            if daemon_alive(&info) {
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
        let resp = ureq::post(&url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Content-Type", "application/json")
            .timeout(CONNECT_TIMEOUT)
            .send_json(req)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
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
pub fn spawn_daemon_and_wait() -> anyhow::Result<()> {
    let server = esm_server_exe()?;
    let mut child = Command::new(&server)
        .arg("--daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("spawn esm-server --daemon")?;

    // Detach: don't wait on the child
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

/// Start the daemon process (for `esm daemon start`).
pub fn start_daemon_process() -> anyhow::Result<DaemonInfo> {
    if let Ok(info) = read_daemon_info() {
        if daemon_alive(&info) {
            return Ok(info);
        }
    }
    spawn_daemon_and_wait()?;
    read_daemon_info()
}

/// Stop a running daemon.
pub fn stop_daemon() -> anyhow::Result<()> {
    if let Ok(info) = read_daemon_info() {
        if daemon_alive(&info) {
            let backend = RemoteBackend::from_daemon_info(&info);
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
