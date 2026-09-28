//! `chase` / `walk` subcommand handlers.

use esm::FormIdBase;
use esm::ipc::{Op, RecordSel};
use std::path::Path;

use crate::Backend;
use crate::output::render_form_id;

/// `chase` is JSON-only — a pipeline evidence contract, not something meant
/// to be read directly (see `esm::chase`'s module docs and `docs/adr/0001`).
/// The classifier itself runs in the library (`Op::Chase`, see
/// `esm::ipc::dispatch_op`); this is one `Op` and a pretty-print.
///
/// `--decimal` still affects *input* selector parsing here (`base`, for
/// consistency with every other subcommand), but deliberately never touches
/// this command's output: `chase`'s JSON is a machine pipeline contract
/// (`docs/adr/0001`) consumed by `tools/patchnotes_lib.py`'s `is_formid_str`,
/// which requires literal `0x` + 8 hex digits.
pub(crate) fn cmd_chase(
    backend: &mut Backend,
    file: &Path,
    selector: &str,
    depth: usize,
    ref_limit: usize,
    base: FormIdBase,
) -> anyhow::Result<()> {
    let sel = RecordSel::from_input_with(selector, base)?;
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
/// run in the library in one `Op::Walk` call (`esm::ipc::dispatch_op`) — this
/// only resolves the CLI's own flags into the request and renders the
/// result, matching `--json` vs plain text either way (`esm::walk::render`
/// is the sole place a `Digest`/`WalkResult` becomes text).
/// Rewrite a [`esm::walk::WalkResult`]'s identity FormIDs into `base`, in
/// place: each node's `formid`, the not-found fallback's `target` and its
/// search-match rows' `form_id`. `digest` (each node's record-type-specific
/// decoded payload) is untouched — any FormIDs inside it are decoded
/// reference fields, not this node's own identity, and stay hex regardless
/// of `--decimal`. Applied to the deserialized result before either the
/// `--json` or the text (`render::render_text`) path, so both stay
/// consistent without threading `base` into the library's renderer.
fn convert_walk_result_form_ids(result: &mut esm::walk::WalkResult, base: FormIdBase) {
    if let Some(nf) = result.not_found.as_mut() {
        nf.target = render_form_id(&nf.target, base);
        for m in nf.matches.iter_mut() {
            m.form_id = render_form_id(&m.form_id, base);
        }
    }
    for node in result.nodes.iter_mut() {
        node.formid = render_form_id(&node.formid, base);
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn cmd_walk(
    backend: &mut Backend,
    file: &Path,
    selector: &str,
    depth: Option<usize>,
    ref_limit: usize,
    level: f32,
    want_refs: bool,
    json: bool,
    base: FormIdBase,
) -> anyhow::Result<()> {
    let sel = RecordSel::from_input_with(selector, base)?;
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
    let mut result: esm::walk::WalkResult = serde_json::from_value(v)?;
    convert_walk_result_form_ids(&mut result, base);

    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!("{}", esm::walk::render_text(&result));
    }
    Ok(())
}
