# Todo: Finish N-API Binding and Decide WASM Strategy

> **Status: partially implemented, rescope required.**
>
> The original plan assumed no FFI, no Electron app, and no workspace. That is no
> longer true: the repo now has a `bindings/napi` workspace member and the
> Electron app consumes `@fo76/esm-napi`. The remaining work is to harden,
> package, and verify the N-API binding. WASM remains unimplemented and should be
> treated as a separate product decision, not bundled into the N-API cleanup.

## Current implementation status

### Implemented

- Root `Cargo.toml` is already a workspace with members `.` and `bindings/napi`.
- `bindings/napi` exists as a `cdylib` crate using `napi`, `napi-derive`, and
  `napi-build`.
- `EsmDatabase` wraps a warm `Database` in `Mutex<Database>`.
- `openDatabase` is async via `tokio::task::spawn_blocking`.
- The binding exposes `fileInfo`, `listGroups`, `listTypeRecords`,
  `recordByFormid`, `recordByEdid`, `referencedBy`, and `parseFormId`.
- The Electron app depends on `@fo76/esm-napi` and loads it through
  `app/src/main/addon.ts`.
- Electron IPC calls into the N-API object in `app/src/main/ipc.ts`.
- Electron packaging unpacks `.node` files and `node_modules/@fo76/esm-napi`.

### Not Implemented

- `bindings/wasm` does not exist.
- Core is still mmap/filesystem-only; there is no `EsmFile::from_bytes` or
  `Database::from_bytes`.
- `memmap2` and cache I/O are not cfg-gated for `wasm32`.
- No `wasm-bindgen`, `serde-wasm-bindgen`, `wasm-pack`, or browser package exists.

### N-API Gaps

- `bindings/napi/index.d.ts` is empty even though `package.json` advertises it in
  `"types"`.
- `bindings/napi/smoke.mjs` hardcodes a local absolute ESM path.
- `bindings/napi/src/lib.rs` uses `Mutex::lock().unwrap()` and
  `serde_json::to_value(...).unwrap()`.
- `recordByEdid` and `referencedBy` are synchronous after DB open and can block
  Electron's main process during first-time index builds.
- `bindings/napi/package.json` only targets `x86_64-unknown-linux-gnu`, while
  Electron packaging declares Linux, macOS, and Windows outputs.
- IPC callers currently rely on loose `unknown` casts around native methods,
  which makes the empty `.d.ts` problem easy to miss.

## Rescoped Goal

Finish the native Electron binding as a production-quality local desktop bridge.
Keep WASM as an optional follow-up only if browser-only support is still desired.

The old recommendation to move the core crate into `crates/core/` should be
dropped unless a separate packaging need appears. The current lower-churn
workspace style is adequate.

## Remaining N-API Work

1. Generate or hand-author `bindings/napi/index.d.ts` so the advertised package
   types match the actual JS API.
2. Replace `Mutex::lock().unwrap()` with poison-aware error mapping to
   `napi::Error`.
3. Replace `serde_json::to_value(...).unwrap()` with fallible conversion and
   error mapping.
4. Move expensive binding calls that can build large indexes (`recordByEdid`,
   `referencedBy`) onto blocking tasks or expose async variants.
5. Make `bindings/napi/smoke.mjs` read `process.env.ESM` and fail with a useful
   message when it is not set.
6. Decide supported native targets and update `bindings/napi/package.json`,
   build scripts, release docs, and Electron packaging accordingly.
7. Replace loose native-object casts in `app/src/main/ipc.ts` with the generated
   package types once `index.d.ts` is valid.
8. Document how to build the native addon before running the Electron app.

## Optional WASM Follow-Up

Only pursue this if a browser-only, no-Electron build is still a product goal.
If not, close this portion as deferred because the native Electron path is the
right fit for an ~880 MB mmap-backed ESM.

If WASM remains desired, create a separate todo for:

- `bindings/wasm` crate using `wasm-bindgen` and `serde-wasm-bindgen`.
- Bytes-backed `EsmFile::from_bytes` and `Database::from_bytes`.
- `wasm32` cfg-gating for `memmap2` and `.esm.idx` cache I/O.
- In-memory-only indexing for WASM.
- Documentation of the practical memory ceiling for large ESM files in browser
  linear memory.

## Acceptance Criteria

- `@fo76/esm-napi` has useful checked TypeScript declarations.
- Native binding methods return `napi::Error` instead of panicking on poisoned
  locks or serialization failures.
- The Electron main process does not block on first-time EditorID or xref index
  builds.
- The smoke test runs on any machine with `ESM=/path/to/SeventySix.esm`.
- Supported platforms are explicit and match the Electron packaging targets, or
  packaging is narrowed to the actually supported target.
- WASM is either split into a new todo or explicitly closed as not planned.

## Verification

- `cargo test`
- `cargo clippy --all-targets -- -D warnings`
- `npm run build` in `bindings/napi` or equivalent `napi build --platform --release`
- `ESM=/path/to/SeventySix.esm node bindings/napi/smoke.mjs`
- `npm run build` in `app/`

---

## Superseded Original Notes

The remainder of this file is retained only as historical context until the
rescope above is implemented. Do not follow the old workspace move or combined
N-API/WASM plan verbatim.

## Context

The only consumer-facing interface today is the `fo76` CLI binary. A planned
Electron/TypeScript frontend (data browser, DPS calculator) needs to query the
parser interactively: fetch a record by FormID/EditorID, list records by type,
and read the TES4 header — all returning JSON the renderer can bind directly to
UI.

Two ways to consume the Rust core from TS:

1. **Spawn the `fo76` CLI** as a child process and parse stdout. Simple, but
   pays full `Database::open` + index build/load cost on *every* query (no
   warm in-memory state), serializes everything through a process boundary, and
   is awkward to make async/cancellable. Unworkable for an interactive UI over
   an ~880 MB ESM.
2. **Native bindings** — a `.node` addon (napi-rs) that keeps a `Database`
   alive in the Node process across calls, so the FormID index is built/loaded
   once and reused. This is the right model for a desktop app and is the
   primary target of this plan.

For a future browser-only build (no Electron / no Node filesystem), a **WASM**
package is a secondary target. It shares the core crate but cannot use mmap and
must accept file bytes from JS.

This task is item `- [ ] napi-rs / WASM binding for Electron/TS frontend` under
*Productization (post-POC)* in `todos.md`.

## Current state

- Single crate `fo76-esm-parser` (`Cargo.toml` at repo root): lib
  `fo76_esm_parser` (`src/lib.rs`) + bin `fo76` (`src/bin/cli.rs`). No
  workspace.
- Public API (all `anyhow::Result`, all on `&mut Database` except `file_info`):
  - `Database::open(path: impl AsRef<Path>) -> Result<Database>`
  - `db.file_info() -> Result<FileInfo>`
  - `db.record_by_formid(FormId) -> Result<RecordResult>`
  - `db.record_by_edid(&str) -> Result<RecordResult>` — calls
    `index.ensure_edid_index(&self.esm)` first, which builds (and disk-caches)
    the EditorID index on first use. This is the expensive, blocking call.
  - `db.list_by_type(&str, usize) -> Result<Vec<ListEntry>>`
  - `db.record_raw(FormId) -> Result<ParsedRecord>`
  - free fn `parse_form_id_input(&str) -> Result<FormId>`
- Result types are `Serialize`: `RecordResult { header: RecordHeaderInfo,
  editor_id: Option<String>, fields: serde_json::Value }`, `ListEntry {
  form_id: String, editor_id: Option<String>, full_lstring_id: Option<String> }`,
  `FileInfo`, `RecordHeaderInfo`. `FormId(pub u32)` is a `Serialize` newtype.
- I/O is **mmap-only**: `reader.rs` holds `pub struct EsmFile { pub mmap:
  memmap2::Mmap, pub path: PathBuf }`; `EsmFile::open` does
  `unsafe { Mmap::map(&file)? }` and exposes the bytes via `data() -> &[u8]`.
  All record parsing goes through `EsmFile::data()`. `memmap2` is an
  unconditional dependency (`Cargo.toml:20`).
- No FFI, no `#[napi]`, no `wasm-bindgen`, no package manifest. MSRV 1.70,
  edition 2021.

The mmap coupling is the one structural blocker for WASM: `wasm32-unknown-unknown`
has no `std::fs::File` / `memmap2`. napi-rs (native Node) is unaffected — it can
use the mmap path as-is.

## Approach

### Workspace conversion

Promote the repo to a Cargo workspace so the bindings are separate crates that
depend on the core lib, keeping `fo76-esm-parser` free of binding deps. Root
`Cargo.toml` becomes a virtual manifest listing three members:

```
fo76-esm-parser/                 (workspace root)
├── Cargo.toml                   # [workspace] only
├── crates/
│   └── core/                    # moved from root: the existing lib + CLI
│       ├── Cargo.toml           # was the root Cargo.toml
│       ├── src/...              # moved src/
│       └── schema/fo76.json     # moved (include_str! path stays relative to core)
├── bindings/
│   ├── napi/                    # fo76-esm-napi
│   └── wasm/                    # fo76-esm-wasm
```

To minimize churn we can instead keep core where it is and only add the two
binding crates as members (root `Cargo.toml` keeps `[package]` **and** gains
`[workspace] members = [...]`). Both are valid; **recommended: move core into
`crates/core/`** for a clean virtual workspace root — it makes the eventual
axum/MCP crate (todos.md *Productization*) drop in naturally. The plan below
assumes the move; Step 1 notes the relative-path adjustments.

### napi-rs binding (`bindings/napi`, crate `fo76-esm-napi`)

- Use `napi` + `napi-derive` (the v2 macro API) and `@napi-rs/cli` for the JS
  packaging + `.d.ts` generation. crate-type `cdylib`.
- Wrapper class holding the warm DB:

  ```rust
  #[napi]
  pub struct EsmDatabase {
      inner: std::sync::Mutex<fo76_esm_parser::Database>,
  }
  ```

  `Database` is not `Sync` (it holds `&mut`-mutating index state), and
  `record_*`/`list_*` take `&mut self`. napi requires shared (`&self`) methods,
  so wrap in a `Mutex` and lock per call. (An `RwLock` does not help because the
  read methods mutate the EditorID index.)

- **Error bridge** `anyhow::Error -> napi::Error`:

  ```rust
  fn to_napi_err(e: anyhow::Error) -> napi::Error {
      napi::Error::from_reason(format!("{e:#}"))   // {:#} includes the .context chain
  }
  // helper:  res.map_err(to_napi_err)
  ```

- **Value bridge** `serde_json::Value` / result structs -> JS. Two options:
  1. **Serialize to a JSON string** in Rust, `JSON.parse` on the JS side (a thin
     hand-written `index.ts` wrapper). Trivial, zero extra deps, robust for the
     arbitrary-shaped `fields: Value`. Downside: double-encode/decode.
  2. **`napi::bindgen_prelude::Env::to_js_value` via `serde`** — napi can
     convert any `Serialize` type to a `JsUnknown` directly. Cleaner, no JSON
     round-trip, but `serde_json::Value`'s dynamic shape becomes a plain JS
     object (fine).

  **Recommended: option 2** (return native JS objects) for `file_info` and
  `list_by_type` (fixed shapes -> good `.d.ts`), and for `record_*` return the
  whole `RecordResult` via `to_js_value` too. If `.d.ts` fidelity on the
  dynamic `fields` object proves annoying, fall back to returning the record as
  a JSON string method (`record_by_formid_json`) and document `fields: unknown`.

- **Async / Task pattern.** `open` and `record_by_edid` block (index
  build/load, mmap fault-ins, EditorID index). Expose these as async via napi's
  `AsyncTask`/`Task` so they run on libuv's thread pool and return a JS
  `Promise`, keeping the Electron main/renderer thread responsive:

  ```rust
  struct OpenTask { path: String }
  impl napi::Task for OpenTask {
      type Output = fo76_esm_parser::Database;
      type JsValue = EsmDatabase;
      fn compute(&mut self) -> napi::Result<Self::Output> {
          fo76_esm_parser::Database::open(&self.path).map_err(to_napi_err)
      }
      fn resolve(&mut self, _env: Env, db: Self::Output) -> napi::Result<Self::JsValue> {
          Ok(EsmDatabase { inner: Mutex::new(db) })
      }
  }

  #[napi]
  pub fn open_database(path: String) -> AsyncTask<OpenTask> {
      AsyncTask::new(OpenTask { path })
  }
  ```

  Same pattern for an async `record_by_edid` (its first call builds the EditorID
  index). The cheap, already-warm lookups (`record_by_formid`, `list_by_type`,
  `file_info`, `record_raw`) can be **synchronous `#[napi]` methods** — they are
  in-memory hash lookups plus a single record parse, fast enough not to need the
  thread pool. (If profiling shows mmap fault-in stalls on cold pages, promote
  them to Tasks later. Note the `Mutex<Database>` is `Send` so it is safe to
  move the handle into a Task.)

- `parse_form_id_input` is exposed as a free `#[napi]` fn `parse_form_id(s:
  String) -> Result<u32>` so the TS layer can validate/normalize FormID input
  the same way the CLI does.

### WASM binding (`bindings/wasm`, crate `fo76-esm-wasm`) — secondary

- Use `wasm-bindgen` + `wasm-pack` (target `bundler`), optionally `tsify` +
  `serde-wasm-bindgen` for `.d.ts`-generating struct return types. crate-type
  `cdylib`.
- **The mmap blocker.** Feature-gate `memmap2` out of the WASM build and add a
  bytes-backed reader. In `crates/core/Cargo.toml`:

  ```toml
  [target.'cfg(not(target_arch = "wasm32"))'.dependencies]
  memmap2 = "0.9"
  ```

  Then make `EsmFile` storage conditional in `reader.rs`:

  ```rust
  #[cfg(not(target_arch = "wasm32"))]
  pub struct EsmFile { pub mmap: memmap2::Mmap, pub path: PathBuf }
  #[cfg(target_arch = "wasm32")]
  pub struct EsmFile { pub bytes: Vec<u8>, pub path: PathBuf }
  ```

  Keep `data(&self) -> &[u8]` returning `&self.mmap` / `&self.bytes` so **all
  downstream parsing is unchanged** (it only ever touches `data()`). Add a
  `#[cfg(target_arch = "wasm32")] EsmFile::from_bytes(name: String, bytes:
  Vec<u8>) -> Result<Self>` and a `Database::from_bytes(...)` constructor that
  skips the disk `Index` cache (no fs on wasm; build the index in-memory and
  never write the `*.idx` sidecar). Verify `index.rs`'s cache load/store is also
  `#[cfg(not(target_arch = "wasm32"))]`-gated.
- The WASM crate exposes `WasmDatabase` constructed from a JS `Uint8Array`
  (`Vec<u8>`); JS reads the file (`FileReader` / fetch `arrayBuffer`) and hands
  bytes in. Same method set as napi, returning values via `serde-wasm-bindgen`.
- **Performance caveat (document, don't fix):** an ~880 MB ESM means ~880 MB of
  WASM linear memory plus the in-memory FormID index (~5.8M records). That is at
  or beyond practical browser limits and there is no mmap lazy paging. WASM is
  therefore positioned as "works for smaller plugins / future streaming work";
  Electron should use the napi addon. Note this in the WASM crate README.

## Files to create / modify

**Modify**
- `Cargo.toml` (root) — convert to virtual workspace manifest (`[workspace]`,
  `members`, shared `[workspace.package]`/`[workspace.dependencies]` for
  `anyhow`, `serde`, `serde_json` versions). Remove `[package]` (moves to core).
- `crates/core/Cargo.toml` — the former root manifest, with `memmap2` moved
  under `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]`; add an
  internal `wasm` cfg helper if needed. `include_str!("schema/fo76.json")` path
  stays valid since `schema/` moves with `src/`.
- `crates/core/src/reader.rs` — cfg-gated `EsmFile` storage + `from_bytes`
  constructor; `// SAFETY:` comment on the (still native-only) `Mmap::map`.
- `crates/core/src/index.rs` — cfg-gate the `*.idx` disk cache read/write so the
  wasm build builds the index in memory only.
- `crates/core/src/lib.rs` — add `#[cfg(target_arch = "wasm32")]
  Database::from_bytes(name, bytes)` alongside `open`.

**Create — napi**
- `bindings/napi/Cargo.toml` — `[package] name = "fo76-esm-napi"`, crate-type
  `["cdylib"]`, deps `fo76-esm-parser` (path), `napi = { version = "2", features
  = ["napi6", "serde-json"] }`, `napi-derive = "2"`; `[build-dependencies]
  napi-build = "2"`.
- `bindings/napi/build.rs` — `napi_build::setup();`
- `bindings/napi/src/lib.rs` — `EsmDatabase` class, `OpenTask`/edid Task, sync
  lookup methods, `parse_form_id`, error/value bridges.
- `bindings/napi/package.json` — `@napi-rs/cli` config (`napi` block:
  `name`, `triples`), scripts `build` (`napi build --platform --release`),
  `build:debug`. Output `.node` + generated `index.js`/`index.d.ts`.
- `bindings/napi/index.d.ts` — **auto-generated** by `napi build`; commit it.
- `bindings/napi/index.js` — auto-generated loader; commit it.
- `bindings/napi/.npmignore` / optional thin `index.ts` wrapper if using the
  JSON-string fallback for `fields`.

**Create — wasm**
- `bindings/wasm/Cargo.toml` — `[package] name = "fo76-esm-wasm"`, crate-type
  `["cdylib"]`, deps `fo76-esm-parser` (path), `wasm-bindgen = "0.2"`,
  `serde-wasm-bindgen = "0.6"`, optional `tsify = "0.4"`,
  `console_error_panic_hook` (dev).
- `bindings/wasm/src/lib.rs` — `WasmDatabase` from `Uint8Array`, method set,
  `serde-wasm-bindgen` returns.
- `bindings/wasm/README.md` — the 880 MB linear-memory caveat.

## Steps

1. **Workspace conversion.** Create `crates/core/`; `git mv` `src/`,
   `schema/`, `tools/` into it and the current root `Cargo.toml` to
   `crates/core/Cargo.toml`. Write a new root `Cargo.toml` as a virtual
   `[workspace]` with `members = ["crates/core", "bindings/napi",
   "bindings/wasm"]` and a `[workspace.dependencies]` table. Confirm
   `cargo build -p fo76-esm-parser` and the `fo76` bin still build and that
   `include_str!` resolves. Run `cargo fmt --check` and
   `cargo clippy --all-targets -- -D warnings`.

2. **Core: cfg-gate mmap.** In `crates/core/Cargo.toml` move `memmap2` under
   `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]`. In `reader.rs`
   split `EsmFile` storage by cfg, keep `data() -> &[u8]` uniform, add the
   `#[cfg(target_arch = "wasm32")] from_bytes`. Add the `// SAFETY:` comment to
   the mmap call. In `index.rs` cfg-gate disk cache I/O. In `lib.rs` add
   `Database::from_bytes`. Verify the native build is byte-for-byte behavior-
   unchanged (`cargo test`, CLI smoke test against the local ESM). Optionally
   add `wasm32-unknown-unknown` target and `cargo check -p fo76-esm-parser
   --target wasm32-unknown-unknown` to prove core compiles for wasm.

3. **napi crate scaffold.** `cargo new --lib bindings/napi`; install
   `@napi-rs/cli` (`npm i -D @napi-rs/cli` in `bindings/napi`) and run
   `napi new`-style init to seed `package.json`/`build.rs`, or hand-write them
   per the Files section. Set crate-type `cdylib`, add the `napi`/`napi-derive`
   deps and `napi-build` build-dep.

4. **napi: error + value bridges.** Implement `to_napi_err` and choose the
   value strategy (Env `to_js_value` for fixed-shape results; JSON-string method
   for `fields` if needed). Add a unit-level helper module.

5. **napi: warm DB + sync lookups.** Implement `#[napi] struct EsmDatabase {
   inner: Mutex<Database> }` and `#[napi]` methods:
   - `file_info(&self) -> Result<FileInfo>`
   - `record_by_formid(&self, form_id: u32) -> Result<RecordResult>`
   - `list_by_type(&self, sig: String, limit: u32) -> Result<Vec<ListEntry>>`
   - `record_raw(&self, form_id: u32) -> Result<ParsedRecord>` (or its JSON
     string)
   Each locks the mutex, calls the core method, maps the error.

6. **napi: async Tasks.** Implement `OpenTask` (-> `open_database(path) ->
   AsyncTask<OpenTask>`) and an `EdidTask`/async method
   `record_by_edid(&self, edid: String)` that builds the EditorID index on the
   thread pool and resolves a `Promise`. Add free fn `parse_form_id(s: String)
   -> Result<u32>` wrapping `parse_form_id_input(...).map(|f| f.0)`.

7. **napi: build + types.** `napi build --platform --release` in
   `bindings/napi`; confirm it emits the `.node`, `index.js`, and `index.d.ts`.
   Inspect the generated `.d.ts` for the `open_database` Promise type and
   `EsmDatabase` method signatures. Commit generated `index.js`/`index.d.ts`.

8. **napi: TS smoke harness.** Add a tiny `bindings/napi/examples/smoke.ts`
   (or `.mjs`) that `await open_database(process.env.ESM)`, prints
   `file_info()`, lists `WEAP`, fetches a known FormID (`0x463F`) and an
   EditorID (`AssaultRifle`). Run it under `ts-node`/`node` to validate end to
   end against the local ESM.

9. **wasm crate scaffold (secondary).** `cargo new --lib bindings/wasm`,
   crate-type `cdylib`, add `wasm-bindgen` + `serde-wasm-bindgen` (+ `tsify`).
   Implement `#[wasm_bindgen] WasmDatabase::new(bytes: Vec<u8>, name: String)`
   over `Database::from_bytes`, plus the same lookup methods returning values via
   `serde_wasm_bindgen::to_value`. Errors -> `JsError`/`JsValue` via
   `e.to_string()`.

10. **wasm build + caveat.** `wasm-pack build --target bundler bindings/wasm`;
    confirm the `pkg/` with `.wasm`, `.js`, and generated `.d.ts`. Document the
    linear-memory ceiling in `bindings/wasm/README.md`. (No need to load the
    full 880 MB ESM in a browser to validate — test with a small synthetic or
    trimmed plugin.)

11. **Docs + CI.** Add a "Bindings" section to the root `README.md` pointing at
    `bindings/napi` (primary, Electron) and `bindings/wasm` (secondary,
    browser). If CI exists, add `cargo fmt --check` + `clippy -D warnings`
    across the workspace and a `napi build` job on Linux/macOS/Windows triples.

## Edge cases & risks

- **`Database` mutability vs napi `&self`.** All read methods take `&mut self`
  (EditorID index can mutate). `Mutex<Database>` is required; document that
  concurrent JS calls serialize on the lock. A single Electron renderer making
  sequential awaits is unaffected.
- **First `record_by_edid` is expensive** (full EditorID index build over ~5.8M
  records + disk-cache write). Must be the async Task, or the Electron UI
  freezes. Subsequent calls are cheap.
- **mmap lifetime on wasm.** The wasm `Vec<u8>` holds the whole file in linear
  memory; dropping `WasmDatabase` must free it. Confirm no leaked references and
  that core parsing never assumes `'static` mmap bytes (it borrows via `data()`,
  so fine).
- **`*.idx` cache on wasm.** No filesystem — index must be in-memory only. Ensure
  every `std::fs` touch in `index.rs` is cfg-gated; a stray `File::create` will
  fail to compile or panic at runtime on wasm.
- **`.d.ts` fidelity for `fields: serde_json::Value`.** It will surface as
  `Record<string, unknown>` / `any`. Acceptable; the JSON-string fallback is the
  escape hatch if `to_js_value` produces awkward types.
- **napi version / Node ABI.** Pin a `napiN` feature (e.g. `napi6`) matching the
  Electron Node ABI to avoid runtime "wrong NODE_MODULE_VERSION" errors. Build
  per-triple for distribution.
- **MSRV 1.70.** Check `napi`/`napi-derive`, `wasm-bindgen`, `serde-wasm-bindgen`
  MSRVs; if a recent release requires newer Rust, pin a compatible version or
  raise the documented MSRV for the binding crates only (core can stay 1.70).
- **`u32` FormID across the boundary.** JS numbers are f64 but `u32` is exactly
  representable, so passing FormIDs as `number` is safe (no `bigint` needed).
  `parse_form_id` handles `"0x463F"`/decimal string input from the UI.
- **Workspace move breaks paths.** `include_str!("schema/fo76.json")`, the
  extractor's `../TES5Edit` sibling assumption, and any `*.idx`-next-to-ESM
  logic are relative; re-verify after the `git mv`.

## Dependencies

- **No hard prerequisites.** This can be built against the current public API.
- **Benefits from (not blocked by):** `03 schema-coverage` and
  `04 formid-load-order` — a more complete/stable `RecordResult.fields` shape
  and FormID resolution make the TS types more useful, but the binding works
  with today's API.
- **New external tooling:** Node + `@napi-rs/cli` (napi build), and
  `wasm-pack`/`wasm-bindgen-cli` + the `wasm32-unknown-unknown` target (wasm
  build). Both are dev/build-time only; the published `.node`/`.wasm` artifacts
  carry no extra runtime deps beyond the Node/browser runtime.

## Verification

- `cargo build` / `cargo test` / `cargo clippy --all-targets -- -D warnings` /
  `cargo fmt --check` pass across the whole workspace; the existing `fo76` CLI
  still builds and runs against the local ESM (no behavior change in core).
- `cargo check -p fo76-esm-parser --target wasm32-unknown-unknown` succeeds
  (proves the mmap/fs cfg-gating is correct).
- `napi build --platform --release` produces a loadable `.node` plus
  `index.js` + `index.d.ts`; the TS smoke harness (Step 8) opens the local ESM,
  reads `file_info`, lists `WEAP`, and fetches both a FormID and an EditorID
  with results matching the equivalent `fo76` CLI calls
  (`get --formid 0x463F`, `get --edid AssaultRifle`, `list --type WEAP`).
- `wasm-pack build --target bundler` produces a `pkg/` with `.wasm` + `.d.ts`;
  loading a small/trimmed test plugin via `WasmDatabase` returns the same
  records as the native path for that plugin.
- Generated `index.d.ts` shows `open_database(path: string):
  Promise<EsmDatabase>` and an async `recordByEdid` returning a `Promise`.
