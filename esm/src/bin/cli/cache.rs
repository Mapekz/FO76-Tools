//! `esm cache` subcommands: inspect, build and clear an ESM's `esm_cache/`,
//! plus the detached build delegate every other subcommand installs.

use esm::CacheInventory;
use esm::progress::BuildStage;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::output::print_json;
use crate::progress_ui;

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

/// `esm cache build`: build `sections` (every section when empty) for `esm`
/// in this process, under the usual build lease. Opening the database builds
/// the eager sections (`forms`, `tree`, `lstrings`, `curves`); the lazy index
/// sections are built on request. Progress shows on stderr (the detached
/// builder's stderr is null).
pub(crate) fn cmd_cache_build(esm: &Path, sections: &[BuildStage]) -> anyhow::Result<()> {
    crate::progress_ui::watched(&[esm], || {
        let db = esm::Database::open(esm)?;
        let wants = |stage| sections.is_empty() || sections.contains(&stage);
        if wants(BuildStage::Edid) {
            db.ensure_edid_index()?;
        }
        if wants(BuildStage::Search) {
            db.ensure_search_index()?;
        }
        if wants(BuildStage::Xref) {
            db.ensure_xref_index()?;
        }
        Ok(())
    })
}

/// `esm cache clear`: delete every cache section built for `esm`.
pub(crate) fn cmd_cache_clear(esm: &Path) -> anyhow::Result<()> {
    let removed = esm::progress::clear_cache(esm)?;
    eprintln!(
        "removed {} cache file(s) for {}",
        removed.len(),
        esm.display()
    );
    Ok(())
}

/// The build delegate the CLI installs (see [`esm::progress::delegate_builds`]):
/// build `stage` in a detached `esm cache build` and wait for it. The child
/// runs in its own session with no inherited pipes, so killing this process
/// (or its process group) leaves the build running to completion, and the
/// next call finds the section built.
pub(crate) fn build_in_detached_process(esm: &Path, stage: BuildStage) -> anyhow::Result<()> {
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.arg("--esm")
        .arg(esm)
        .args(["cache", "build", "--section", stage.label()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    detach(&mut cmd);
    let status = cmd.status()?;
    anyhow::ensure!(status.success(), "esm cache build exited with {status}");
    Ok(())
}

#[cfg(unix)]
#[allow(unsafe_code)] // see the SAFETY comment inside
fn detach(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: `setsid` is async-signal-safe and touches no memory of the
    // parent; it runs in the forked child just before `exec`.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
}

#[cfg(windows)]
fn detach(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
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
