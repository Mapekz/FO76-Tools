//! The `ba2` CLI docs match the built binary: `README.md` and `AGENTS.md`
//! name only real subcommands with real flags, and `README.md` mentions every
//! subcommand. Repo-wide path and link checks live in `repo-policy`.

use std::path::Path;

use repo_policy::{Cli, assert_no_failures, read_doc};

#[test]
fn docs_match_the_cli() {
    let cli = Cli::new("ba2", env!("CARGO_BIN_EXE_ba2"));
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let (readme, agents) = (read_doc(root, "README.md"), read_doc(root, "AGENTS.md"));

    let mut failures = Vec::new();
    for (name, doc) in [("README.md", &readme), ("AGENTS.md", &agents)] {
        failures.extend(cli.check_invocations(name, doc));
        failures.extend(cli.check_flags(name, doc));
    }
    failures.extend(cli.check_coverage("README.md", &readme));
    assert_no_failures(&failures);
}
