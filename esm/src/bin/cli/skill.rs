//! `esm skill` — print or install the embedded `esm-cli` usage-knowledge doc.

use anyhow::Context as _;
use std::path::{Path, PathBuf};

/// The `esm-cli` usage-knowledge skill doc, embedded at compile time (same
/// `include_str!` pattern as `schema/fo76.json` in `src/schema.rs`). `esm
/// skill` prints it verbatim; `esm skill --install` writes it into a
/// consumer repo's `.claude/skills/esm-cli/` for Claude Code to auto-discover.
const SKILL_MD: &str = include_str!("../../../skills/esm-cli/SKILL.md");

/// Where `esm skill --install [--dir <DIR>]` writes the doc, relative to
/// `dir` (or the current directory when `dir` is `None` upstream).
fn skill_dest_path(dir: &Path) -> PathBuf {
    dir.join(".claude/skills/esm-cli/SKILL.md")
}

/// Pure overwrite-guard decision for `esm skill --install`: refuses to
/// clobber an existing install unless `--force` was passed. Split out from
/// `cmd_skill` so the decision is unit-testable without touching the
/// filesystem.
fn skill_install_allowed(dest_exists: bool, force: bool) -> Result<(), &'static str> {
    if dest_exists && !force {
        Err("destination already exists; pass --force to overwrite")
    } else {
        Ok(())
    }
}

pub(crate) fn cmd_skill(install: bool, dir: Option<PathBuf>, force: bool) -> anyhow::Result<()> {
    if !install {
        print!("{SKILL_MD}");
        return Ok(());
    }
    let base = dir.unwrap_or_else(|| PathBuf::from("."));
    let dest = skill_dest_path(&base);
    if let Err(msg) = skill_install_allowed(dest.exists(), force) {
        anyhow::bail!("{}: {msg}", dest.display());
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(&dest, SKILL_MD).with_context(|| format!("writing {}", dest.display()))?;
    println!("wrote {}", dest.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `esm skill --install` writes to `<dir>/.claude/skills/esm-cli/SKILL.md`.
    #[test]
    fn skill_dest_path_is_under_dot_claude_skills() {
        assert_eq!(
            skill_dest_path(Path::new("/repo")),
            PathBuf::from("/repo/.claude/skills/esm-cli/SKILL.md")
        );
        assert_eq!(
            skill_dest_path(Path::new(".")),
            PathBuf::from("./.claude/skills/esm-cli/SKILL.md")
        );
    }

    /// The overwrite guard only blocks an existing destination without `--force`.
    #[test]
    fn skill_install_allowed_guards_existing_without_force() {
        assert!(skill_install_allowed(false, false).is_ok());
        assert!(skill_install_allowed(false, true).is_ok());
        assert!(skill_install_allowed(true, true).is_ok());
        assert!(skill_install_allowed(true, false).is_err());
    }

    /// The embedded doc is non-empty and starts with the expected frontmatter,
    /// so `esm skill`/`esm skill --install` never ship a stale/empty file.
    #[test]
    fn skill_md_has_frontmatter() {
        assert!(SKILL_MD.starts_with("---\nname: esm-cli"));
    }
}
