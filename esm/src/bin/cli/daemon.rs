//! `daemon` subcommand handlers.

use esm::backend::{
    RemoteBackend, daemon_fresh, read_daemon_info, start_daemon_process, stop_daemon,
};

pub(crate) fn cmd_daemon_start() -> anyhow::Result<()> {
    let info = start_daemon_process()?;
    println!(
        "daemon running on 127.0.0.1:{} (pid {})",
        info.port, info.pid
    );
    Ok(())
}

pub(crate) fn cmd_daemon_stop() -> anyhow::Result<()> {
    stop_daemon()?;
    println!("daemon stopped");
    Ok(())
}

pub(crate) fn cmd_daemon_status(addr: Option<&str>, port: Option<u16>) -> anyhow::Result<()> {
    let remote = RemoteBackend::connect_existing_with_override(addr, port)?;
    let mut status = remote.status()?;
    // Best-effort: annotate whether the resident daemon is still
    // running the binary it started with (see `daemon_fresh` in
    // `backend.rs`). A `false` here means a rebuild happened since
    // it started and the next call will respawn it.
    if let Ok(info) = read_daemon_info()
        && let Some(obj) = status.as_object_mut()
    {
        obj.insert("binary_current".to_string(), daemon_fresh(&info).into());
    }
    println!("{}", serde_json::to_string_pretty(&status)?);
    Ok(())
}
