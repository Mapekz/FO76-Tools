# Commit messages: Conventional Commits

Every commit subject is `type(scope): summary` ([Conventional Commits](https://www.conventionalcommits.org/)),
imperative mood, no trailing period.

Most of the history predates this rule and carries a bare area prefix instead
(`esm: split diff.rs — array-diff engine and noise-suppression pipeline`). That area is the scope;
new commits keep it and gain a type in front of it (`refactor(esm): split diff.rs — ...`). Old
subjects stay as they are.

- **Types in use**: `feat`, `fix`, `docs`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`. Pick
  the narrowest one that fits; don't invent a type for a one-off commit. Dependency and toolchain
  bumps are `chore(deps)`, named after the `deps` CI job that gates them.
- **Scope** is the subproject (`esm`, `ba2`, `esm-viewer`, `patch-notes`, or `napi` for
  `esm/bindings/napi`) or a cross-cutting area (`repo`, `ci`, `deps`, `docs`, `esm-cli`). A
  path-shaped scope is fine when it is more precise, as `esm/tools` is for the schema tooling, and a
  change that lands in two subprojects names both (`docs(esm/ba2): ...`). Omit the scope only when
  the change is genuinely repo-wide.
- **Breaking changes**: append `!` after the type/scope (`feat(esm)!: ...`) and say what breaks in
  the body. What breaks for a caller is a CLI flag or JSON output shape, an op the N-API addon
  runs, a patch-notes verb or artifact, or a crate's public Rust API.
- **Issue refs** go in the subject tail, `(#29)`. Not `(issue #27)`.
- Body lines explain why, not what the diff already shows — the same rule the code comments in this
  repo follow.

Examples already in this repo's history: `docs(esm): document --resolve stub to avoid N follow-up
get calls`, `fix: RecordHeaderInfo.form_id serializes as hex string, not a bare number`,
`ci: drop continue-on-error from the napi job`.
