//! The `esm` CLI docs match the built binary: `README.md` and
//! `skills/esm-cli/SKILL.md` name only real subcommands and mention every
//! one, and SKILL.md's invocations use only real flags. Repo-wide path and
//! link checks live in `repo-policy`.

use std::path::Path;

use repo_policy::{Cli, assert_no_failures, read_doc};

const README: &str = "README.md";
const SKILL: &str = "skills/esm-cli/SKILL.md";

#[test]
fn docs_match_the_cli() {
    let cli = Cli::new("esm", env!("CARGO_BIN_EXE_esm"));
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let (readme, skill) = (read_doc(root, README), read_doc(root, SKILL));

    let mut failures = Vec::new();
    for (name, doc) in [(README, &readme), (SKILL, &skill)] {
        failures.extend(cli.check_invocations(name, doc));
        failures.extend(cli.check_coverage(name, doc));
    }
    failures.extend(cli.check_flags(SKILL, &skill));
    assert_no_failures(&failures);
}
