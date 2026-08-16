//! `daemon` and `cache` subcommand handlers, plus the daemon-adjacent
//! `cache status` logic that reads `esm_cache/` and the build lock straight
//! off disk (no daemon, no ESM open — see `cmd_cache_status`'s call site in
//! `main()` for why it must never construct a `Backend`).

use esm::CacheInventory;
use esm::backend::{
    RemoteBackend, daemon_fresh, read_daemon_info, start_daemon_process, stop_daemon,
};
use std::collections::BTreeMap;
use std::path::Path;

use crate::output::print_json;
use crate::progress_ui;

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

/// One-word summary of `esm cache status`'s overall state — the four
/// states called out in the design: no cache at all, a build in flight
/// (regardless of how much is already on disk), fully built, or partially
/// built (the common steady state once a build has run but `xref`, say,
/// has never been triggered).
fn cache_state_label(inventory: &CacheInventory, building: bool) -> &'static str {
    if building {
        "building"
    } else if inventory.is_empty() {
        "empty"
    } else if inventory.is_complete() {
        "complete"
    } else {
        "partial"
    }
}

pub(crate) fn cmd_cache_status(esm: &Path, as_json: bool) -> anyhow::Result<()> {
    let inventory = esm::cache_inventory(esm)?;
    let building = esm::progress::read(esm);
    let state = cache_state_label(&inventory, building.is_some());

    if as_json {
        let sections: BTreeMap<&str, bool> = esm::progress::BuildStage::ALL
            .iter()
            .map(|s| (s.label(), inventory.present.contains(s)))
            .collect();
        let build = building.as_ref().map(|p| {
            serde_json::json!({
                "pid": p.pid,
                "stage": p.stage.label(),
                "stage_index": p.stage_index,
                "stage_count": p.stage_count,
                "percent": p.percent(),
                "done": p.done,
                "total": p.total,
                "eta_secs": p.eta().map(|d| d.as_secs()),
            })
        });
        print_json(
            &serde_json::json!({
                "esm": esm,
                "state": state,
                "sections": sections,
                "build": build,
            }),
            true,
        );
        return Ok(());
    }

    println!("{}: {state}", esm.display());
    if let Some(p) = &building {
        println!("  {}", progress_ui::format_stage_summary(p));
    }
    print!("  sections:");
    for stage in esm::progress::BuildStage::ALL {
        let mark = if inventory.present.contains(&stage) {
            "+"
        } else {
            "-"
        };
        print!(" {mark}{}", stage.label());
    }
    println!();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_state_label_covers_all_four_states() {
        let empty = CacheInventory {
            present: vec![],
            missing: esm::progress::BuildStage::ALL.to_vec(),
        };
        assert_eq!(cache_state_label(&empty, false), "empty");
        assert_eq!(cache_state_label(&empty, true), "building");

        let partial = CacheInventory {
            present: vec![
                esm::progress::BuildStage::Forms,
                esm::progress::BuildStage::Tree,
            ],
            missing: vec![
                esm::progress::BuildStage::Edid,
                esm::progress::BuildStage::Search,
                esm::progress::BuildStage::Xref,
            ],
        };
        assert_eq!(cache_state_label(&partial, false), "partial");

        let complete = CacheInventory {
            present: esm::progress::BuildStage::ALL.to_vec(),
            missing: vec![],
        };
        assert_eq!(cache_state_label(&complete, false), "complete");
    }
}
