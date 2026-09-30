//! `esm skill` — print or install the embedded `esm-cli` usage-knowledge doc.

use anyhow::Context as _;
use std::path::{Path, PathBuf};

/// The `esm-cli` usage-knowledge skill doc, embedded at compile time (same
/// `include_str!` pattern as `schema/fo76.json` in `src/schema/mod.rs`). `esm
/// skill` prints it verbatim; `esm skill --install` writes it into a
/// consumer repo for its agents to auto-discover.
const SKILL_MD: &str = include_str!("../../../skills/esm-cli/SKILL.md");

/// A skill directory `esm skill --install` can write to: `agents` is the
/// cross-agent `.agents/skills/` (Codex, Gemini CLI, Copilot, Cursor and
/// others read it), `claude` is Claude Code's `.claude/skills/`, which it
/// alone reads. Each gets its own copy: a downstream tree isn't ours to
/// symlink into, and Codex skips a symlinked `SKILL.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum SkillTarget {
    Agents,
    Claude,
}

impl SkillTarget {
    fn dest(self, dir: &Path) -> PathBuf {
        let skills = match self {
            SkillTarget::Agents => ".agents/skills",
            SkillTarget::Claude => ".claude/skills",
        };
        dir.join(skills).join("esm-cli/SKILL.md")
    }
}

/// Every destination `targets` names under `dir` (`agents` when none
/// are given), deduplicated in the order given.
fn destinations(targets: &[SkillTarget], dir: &Path) -> Vec<PathBuf> {
    let targets = if targets.is_empty() {
        &[SkillTarget::Agents][..]
    } else {
        targets
    };
    let mut dests: Vec<PathBuf> = Vec::new();
    for target in targets {
        let dest = target.dest(dir);
        if !dests.contains(&dest) {
            dests.push(dest);
        }
    }
    dests
}

/// Without `--force`, refuse before writing anything if any destination
/// already exists, so an install is never partial.
fn preflight(dests: &[PathBuf], force: bool, exists: impl Fn(&Path) -> bool) -> anyhow::Result<()> {
    if force {
        return Ok(());
    }
    let existing: Vec<String> = dests
        .iter()
        .filter(|d| exists(d))
        .map(|d| d.display().to_string())
        .collect();
    if !existing.is_empty() {
        anyhow::bail!(
            "{} already exist(s); pass --force to overwrite",
            existing.join(", ")
        );
    }
    Ok(())
}

pub(crate) fn cmd_skill(
    install: bool,
    targets: &[SkillTarget],
    dir: Option<PathBuf>,
    force: bool,
) -> anyhow::Result<()> {
    if !install {
        print!("{SKILL_MD}");
        return Ok(());
    }
    let dests = destinations(targets, &dir.unwrap_or_else(|| PathBuf::from(".")));
    preflight(&dests, force, Path::exists)?;
    for dest in &dests {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(dest, SKILL_MD).with_context(|| format!("writing {}", dest.display()))?;
        println!("wrote {}", dest.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;

    fn parse_targets(args: &[&str]) -> Result<Vec<SkillTarget>, clap::Error> {
        let mut argv = vec!["esm", "skill"];
        argv.extend_from_slice(args);
        match crate::Cli::try_parse_from(argv)?.command {
            crate::Commands::Skill { target, .. } => Ok(target),
            _ => unreachable!(),
        }
    }

    #[test]
    fn each_target_has_its_own_destination() {
        let dir = Path::new("/repo");
        assert_eq!(
            destinations(&[SkillTarget::Agents], dir),
            [PathBuf::from("/repo/.agents/skills/esm-cli/SKILL.md")]
        );
        assert_eq!(
            destinations(&[SkillTarget::Claude], dir),
            [PathBuf::from("/repo/.claude/skills/esm-cli/SKILL.md")]
        );
    }

    #[test]
    fn default_target_is_agents() {
        assert_eq!(
            destinations(&[], Path::new(".")),
            [PathBuf::from("./.agents/skills/esm-cli/SKILL.md")]
        );
    }

    #[test]
    fn target_takes_a_comma_separated_list_and_rejects_unknown_names() {
        assert_eq!(
            parse_targets(&["--install", "--target", "claude,agents"]).unwrap(),
            [SkillTarget::Claude, SkillTarget::Agents]
        );
        assert!(parse_targets(&["--install", "--target", "cursor"]).is_err());
    }

    #[test]
    fn target_requires_install() {
        assert!(parse_targets(&["--target", "claude"]).is_err());
    }

    #[test]
    fn preflight_checks_every_destination_before_writing() {
        let dests = destinations(&[SkillTarget::Agents, SkillTarget::Claude], Path::new("/r"));
        let claude_exists = |p: &Path| p.to_string_lossy().contains(".claude");
        let err = preflight(&dests, false, claude_exists)
            .unwrap_err()
            .to_string();
        assert!(err.contains(".claude/skills/esm-cli/SKILL.md"), "{err}");
        assert!(
            preflight(&dests, true, claude_exists).is_ok(),
            "--force overwrites"
        );
        assert!(preflight(&dests, false, |_| false).is_ok());
    }

    /// The embedded doc is non-empty and starts with the expected frontmatter,
    /// so `esm skill`/`esm skill --install` never ship a stale/empty file.
    #[test]
    fn skill_md_has_frontmatter() {
        assert!(SKILL_MD.starts_with("---\nname: esm-cli"));
    }
}
