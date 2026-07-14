//! Canonical string→enum argument translation shared by the CLI, HTTP/MCP
//! server, and N-API bindings.
//!
//! Each serving surface previously carried its own copy of these mappings
//! (`parse_resolve` in `src/bin/cli.rs`, `parse_resolve_depth`/
//! `parse_search_field`/`parse_body_detail`/`parse_filter_op` in
//! `bindings/napi/src/lib.rs`, an inline match in `src/bin/server.rs`'s
//! `esm_get_record` tool, and a duplicated ref-depth clamp in both `server.rs`
//! and `bindings/napi`). This module is the one place the parsing logic lives;
//! callers still choose their own **default** value for the "argument
//! omitted" case — this deliberately does not unify defaults across surfaces
//! (e.g. the CLI's `esm get` defaults to `ResolveDepth::None` while the MCP
//! `esm_get_record` tool defaults to `ResolveDepth::Stub`).

use crate::diff::{BodyDetail, DiffOptions};
use crate::{FilterOp, ResolveDepth, SearchField};
use anyhow::bail;

/// Parse a `resolve` string (`"none"|"stub"|"full"`) into a [`ResolveDepth`].
///
/// `s == None` (the argument was omitted) yields `default` — each calling
/// surface supplies whatever default it already used before this was
/// extracted. An explicit but unrecognized string is always an error.
pub fn resolve_depth(s: Option<&str>, default: ResolveDepth) -> anyhow::Result<ResolveDepth> {
    match s {
        None => Ok(default),
        Some("none") => Ok(ResolveDepth::None),
        Some("stub") => Ok(ResolveDepth::Stub),
        Some("full") => Ok(ResolveDepth::Full),
        Some(other) => bail!("unknown resolve depth '{other}'; expected none|stub|full"),
    }
}

/// Parse a search-field string (`"edid"|"name"|"both"`) into a [`SearchField`].
///
/// `s == None` yields `default`; an explicit but unrecognized string is an error.
pub fn search_field(s: Option<&str>, default: SearchField) -> anyhow::Result<SearchField> {
    match s {
        None => Ok(default),
        Some("edid") => Ok(SearchField::Edid),
        Some("name") => Ok(SearchField::Name),
        Some("both") => Ok(SearchField::Both),
        Some(other) => bail!("unknown search field '{other}'; expected edid|name|both"),
    }
}

/// Parse a body-detail string (`"none"|"stub"|"full"`) into a [`BodyDetail`].
///
/// `s == None` yields `default`; an explicit but unrecognized string is an error.
pub fn body_detail(s: Option<&str>, default: BodyDetail) -> anyhow::Result<BodyDetail> {
    match s {
        None => Ok(default),
        Some("none") => Ok(BodyDetail::None),
        Some("stub") => Ok(BodyDetail::Stub),
        Some("full") => Ok(BodyDetail::Full),
        Some(other) => bail!("unknown body detail '{other}'; expected none|stub|full"),
    }
}

/// Parse a filter-operator string into a [`FilterOp`]. No default parameter —
/// every existing call site always supplies this explicitly.
pub fn filter_op(s: &str) -> anyhow::Result<FilterOp> {
    match s {
        "exists" => Ok(FilterOp::Exists),
        "eq" => Ok(FilterOp::Eq),
        "contains" => Ok(FilterOp::Contains),
        "gt" => Ok(FilterOp::Gt),
        "lt" => Ok(FilterOp::Lt),
        "gte" => Ok(FilterOp::Gte),
        "lte" => Ok(FilterOp::Lte),
        other => bail!("unknown filter op '{other}'; expected exists|eq|contains|gt|lt|gte|lte"),
    }
}

/// Clamp an optional reverse-reference walk depth to
/// `[1, ipc::DEFAULT_MAX_DEPTH]`, defaulting to `1` when `None`.
///
/// This is the same clamp [`crate::ipc::referenced_by_enriched`] performs
/// internally (that one stays in place — it's the authoritative safety net
/// applied right before the walk itself runs, regardless of what any caller
/// passes it). This helper exists for callers that want to compute/display
/// the clamped value *before* dispatch (e.g. echoing the effective depth back
/// to a caller, or building an `Op` whose depth field should already reflect
/// the clamp).
pub fn clamp_ref_depth(d: Option<usize>) -> usize {
    d.map(|d| d.clamp(1, crate::ipc::DEFAULT_MAX_DEPTH))
        .unwrap_or(1)
}

/// Build a [`DiffOptions`] from the primitive fields the CLI's `diff`/`Diff`
/// subcommand and napi's `EsmDatabase::diff` method each accept, sharing the
/// exclude-types uppercasing so it isn't duplicated on both surfaces.
///
/// Uppercasing here is a convenience for callers that want a canonical
/// `DiffOptions` value up front (e.g. to compare/log it) — `diff_databases_with`
/// itself already uppercases `exclude_types` again before use, so this has no
/// effect on diff correctness either way.
pub fn diff_options<I, S>(bodies: BodyDetail, suppress_noise: bool, exclude_types: I) -> DiffOptions
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    DiffOptions {
        bodies,
        suppress_noise,
        exclude_types: exclude_types
            .into_iter()
            .map(|s| s.as_ref().to_uppercase())
            .collect(),
    }
}
