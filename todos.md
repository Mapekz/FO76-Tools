# FO76-Tools — Backlog

Tracked, actionable work lives in [GitHub Issues](https://github.com/Mapekz/FO76-Tools/issues)
(`gh`, see `docs/agents/issue-tracker.md`) — that's the live, priority-triaged queue as of
2026-07-24, ranked with `P1`/`P2`/`P3` labels (see `docs/agents/triage-labels.md`). This file
is the complement: informal dated notes not yet promoted to an issue and
scope decisions deliberately *not* tracked as work items, grouped under the project they belong
to. Do not reintroduce per-project `todos.md` files or a `todos/` directory — this stays the
one file.

Scope checks are dated so a stale claim is obvious on sight.

---

## `esm/`

Two follow-ups from the 2026-07-31 bincode → rkyv disk-cache migration (RUSTSEC-2025-0141;
`.esm.idx` replaced by five independent zero-copy sections — `.esm.tree`/`.esm.forms` eager,
`.esm.edid`/`.esm.search`/`.esm.xref` lazy — see `src/rkyvcache.rs` and the commit history from
`340f5ac` through `fcb5996`), both done 2026-07-31:

- ~~**Consolidate `mindex.rs`/`.esm.midx` into `.esm.forms`.**~~ `mindex.rs`, `Database.mmap_index`/
  `open_lite`, `check_not_lite`, and the `--mmap-index`/`ESM_MMAP_INDEX` CLI flag are deleted
  outright rather than migrated — `.esm.forms` already served the same zero-copy, binary-searched
  FormID lookup, so there was nothing to consolidate into it, only a redundant path to remove.
  `Database::open` is now the only open path.
- ~~**Finish the doc pass in `esm/CLAUDE.md` and `src/bin/cli.rs`.**~~ Also touched `tree.rs`,
  `rkyvcache.rs`, `README.md`, `.gitignore`, `deny.toml`, and `skills/esm-cli/SKILL.md` — the
  drift was wider than this entry's original scope, since commit `7b88574` had missed those too
  despite its own claim to have covered "every other doc/comment location."

A follow-up 2026-07-31 relocation, done same day: the five flat `.esm.tree`/`.esm.forms`/
`.esm.edid`/`.esm.search`/`.esm.xref` sidecars above moved into one shared `esm_cache/` directory
(sibling of the ESM), files named `<esm file name>.<section>` so multiple plugins in one directory
never collide. Closes three gaps the flat scheme had: `.tmp.<hex>` crash debris wasn't gitignored,
five near-duplicate path helpers existed with no single source of truth, and a naive
`set_extension`-based rename would have fabricated a bogus `.esm` for a non-`.esm` input.
`CACHE_VERSION` was deliberately not bumped — see `index.rs`'s changelog comment on
`CACHE_VERSION` for why. `cache_dir_for`/`section_path_for` (`rkyvcache.rs`) are now the one place
a section's file name is decided.

Outstanding work beyond the above lives in [GitHub Issues](https://github.com/Mapekz/FO76-Tools/issues).

One 2026-07-22 architecture-review finding was deliberately **not** filed: the seven IPC methods
carved out of `esm-viewer`'s `CONTRACT` table and hand-written in three places. The carve-out is
documented and intentional (those methods have non-uniform shapes — `diff` needs two registry
lookups), so it reads as a considered partial refactor rather than an oversight.

---

## `ba2/`

No tracked follow-ups beyond the issue queue.

Scope change (2026-07-29): the GNRL-only stance is retired — the CLI should handle all BA2
archives. DX10 (texture) read/list/extract support is planned in
[#21](https://github.com/Mapekz/FO76-Tools/issues/21) (separate code path per the reader
invariant; DX10 *write* remains out of scope pending its own design). Until #21 lands, the
reader still detects and rejects DX10 with a clear error.

---

## `esm-viewer/`

No tracked follow-ups.

Scope note (2026-07-24): the vitest include glob deliberately excludes `.tsx`, so component
tests are structurally absent — a considered choice, not a gap. Enabling them is tracked at
low priority in [#16](https://github.com/Mapekz/FO76-Tools/issues/16) (`P3`,
`ready-for-human`), kept open in case component testing becomes desirable later.

The 2026-07-28 follow-up (ReferencedByPanel hides the depth-1 `VIA` carrier that
`esm refs --ep` now provides) is promoted to
[#20](https://github.com/Mapekz/FO76-Tools/issues/20).

---

## Cross-cutting

Two follow-ups from the 2026-07-30 Rust 1.97.1 / edition 2024 / dependency migration:

- **Drop `continue-on-error` from the CI `napi` job.** It was added "while the napi build
  environment is being stabilised in CI", which means the napi 2 → 3 bump landed without CI able
  to catch a regression — the addon build and smoke test were verified only locally. Once the
  job has run green on a few pushes, remove the flag so the job actually gates.
- ~~**Revisit `bincode` (RUSTSEC-2025-0141).**~~ Done 2026-07-31: `esm/`'s disk cache moved to
  zero-copy `rkyv` sections (`340f5ac`..`fcb5996`); `bincode` is no longer a dependency and the
  `deny.toml` ignore is removed. See the `esm/` section above — both follow-ups this left behind
  are also done.

Separately, and not a migration artifact: **the Rust crates declare no license and the repository
has no LICENSE file**, so `deny.toml` sets `licenses.private.ignore = true` and the crates are
marked `publish = false`. That is a placeholder, not a decision — worth choosing a license (or
deliberately confirming "all rights reserved") for a public repo.

The one cross-project seam is `esm-viewer/` → `esm/bindings/napi` (the `@fo76/esm-napi` addon,
a local `file:` dependency). Anything that changes the `EsmDatabase` N-API surface has to land
on both sides — run `just gen-types` in `esm/` to regenerate the shared TypeScript DTOs. As of
2026-07-22 the drift guard runs in CI too, so forgetting the regen now fails the build rather
than passing silently.

The typed-envelope gap at that boundary — N-API returns `serde_json::Value`, so
`bun run typecheck` can't verify the seam — is written up in full in
[#9](https://github.com/Mapekz/FO76-Tools/issues/9).
