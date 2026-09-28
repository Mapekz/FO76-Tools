# Repo-root task runner: one module per project, each backed by that project's
# own justfile (its recipes run in its directory). `just --list --list-submodules`
# shows every recipe; run one as `just <module>::<recipe>`.
# Install `just` once: cargo install just
#
# Usage:
#   just        -> every project's local check (CI runs these same recipes)
#   just policy -> repo-wide documentation policy (links, cited paths, consumer names)
#   just audit  -> esm's schema parity audit and drift guards (needs ./TES5Edit)

mod esm
mod ba2
mod viewer "esm-viewer"
mod patch-notes

default: check

# Every project's local check.
check: esm::check esm::tools-check ba2::check viewer::check patch-notes::check policy

# Markdown links and cited repo paths resolve; no file names a downstream consumer.
[doc("Repo-wide documentation policy: links, cited paths, consumer names")]
policy:
    cargo fmt -p repo-policy --check
    cargo clippy -p repo-policy --all-targets -- -D warnings
    cargo test -p repo-policy

# esm's schema parity audit against ./TES5Edit, plus the schema and
# hardcoded-forms drift guards.
[doc("esm's schema parity audit and drift guards (needs ./TES5Edit)")]
audit: esm::audit
