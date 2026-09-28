# Repo-root task runner: one module per project, each backed by that project's
# own justfile (its recipes run in its directory). `just --list --list-submodules`
# shows every recipe; run one as `just <module>::<recipe>`.
# Install `just` once: cargo install just
#
# Usage:
#   just        -> every project's local check (CI runs these same recipes)
#   just audit  -> esm's schema parity audit and drift guards (needs ./TES5Edit)

mod esm
mod ba2
mod viewer "esm-viewer"
mod patch-notes

default: check

# Every project's local check.
check: esm::check esm::tools-check ba2::check viewer::check patch-notes::check

# esm's schema parity audit against ./TES5Edit, plus the schema and
# hardcoded-forms drift guards.
[doc("esm's schema parity audit and drift guards (needs ./TES5Edit)")]
audit: esm::audit
