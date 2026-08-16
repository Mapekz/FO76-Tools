//! `chase` / `walk` subcommand handlers.

use esm::ipc::{Op, RecordSel};
use std::path::Path;

use crate::Backend;

/// `chase` is JSON-only — a pipeline evidence contract, not something meant
/// to be read directly (see `esm::chase`'s module docs and `docs/adr/0001`).
/// The classifier itself runs server-side (`Op::Chase`, dispatched inside the
/// daemon or `--local`'s in-process `Database` — see `esm::ipc::dispatch_op`);
/// this is now just one wire call and a pretty-print.
pub(crate) fn cmd_chase(
    backend: &mut Backend,
    file: &Path,
    selector: &str,
    depth: usize,
    ref_limit: usize,
) -> anyhow::Result<()> {
    let sel = RecordSel::from_input(selector)?;
    let v = backend.run(
        file,
        Op::Chase {
            sel,
            depth,
            ref_limit,
        },
    )?;
    println!("{}", serde_json::to_string_pretty(&v)?);
    Ok(())
}

/// Interactive digest driver. The BFS, per-node digest computation, the
/// not-found search fallback, and the `--refs` reverse-reference summary all
/// run server-side in one `Op::Walk` call (`esm::ipc::dispatch_op`) — this
/// only resolves the CLI's own flags into the request and renders the
/// result, matching `--json` vs plain text either way (see `esm::walk`'s
/// module docs: only the *computation* moved server-side, `render.rs` is
/// still the sole place a `Digest`/`WalkResult` becomes text, so `--local`
/// and daemon output stay byte-identical).
#[allow(clippy::too_many_arguments)]
pub(crate) fn cmd_walk(
    backend: &mut Backend,
    file: &Path,
    selector: &str,
    depth: usize,
    ref_limit: usize,
    level: f32,
    want_refs: bool,
    json: bool,
) -> anyhow::Result<()> {
    let sel = RecordSel::from_input(selector)?;
    let v = backend.run(
        file,
        Op::Walk {
            sel,
            depth,
            ref_limit,
            level,
            want_refs,
        },
    )?;
    let result: esm::walk::WalkResult = serde_json::from_value(v)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!("{}", esm::walk::render_text(&result));
    }
    Ok(())
}
