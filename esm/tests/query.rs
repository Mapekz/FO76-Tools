//! Unit tests for `src/query.rs` — the canonical string→enum argument
//! translation shared by the CLI, HTTP/MCP server, and N-API bindings.
//!
//! All functions here are fully `pub`, so per the colocated-vs-`tests/`
//! convention in `esm/CLAUDE.md` these live in their own integration test
//! file (mirroring `tests/wildcard.rs` for `wildcard_match`) rather than a
//! `#[cfg(test)]` block in `src/query.rs` itself.

use esm::diff::BodyDetail;
use esm::query::{
    body_detail, clamp_ref_depth, diff_options, filter_op, resolve_depth, search_field,
};
use esm::{FilterOp, ResolveDepth, SearchField};

#[test]
fn resolve_depth_parses_known_strings() {
    assert_eq!(
        resolve_depth(Some("none"), ResolveDepth::Stub).unwrap(),
        ResolveDepth::None
    );
    assert_eq!(
        resolve_depth(Some("stub"), ResolveDepth::None).unwrap(),
        ResolveDepth::Stub
    );
    assert_eq!(
        resolve_depth(Some("full"), ResolveDepth::None).unwrap(),
        ResolveDepth::Full
    );
}

#[test]
fn resolve_depth_none_input_yields_default() {
    assert_eq!(
        resolve_depth(None, ResolveDepth::Stub).unwrap(),
        ResolveDepth::Stub
    );
    assert_eq!(
        resolve_depth(None, ResolveDepth::Full).unwrap(),
        ResolveDepth::Full
    );
}

#[test]
fn resolve_depth_rejects_unknown_string() {
    assert!(resolve_depth(Some("bogus"), ResolveDepth::None).is_err());
    // Case matters — "Stub" is not "stub".
    assert!(resolve_depth(Some("Stub"), ResolveDepth::None).is_err());
}

#[test]
fn search_field_parses_known_strings() {
    assert_eq!(
        search_field(Some("edid"), SearchField::Both).unwrap(),
        SearchField::Edid
    );
    assert_eq!(
        search_field(Some("name"), SearchField::Both).unwrap(),
        SearchField::Name
    );
    assert_eq!(
        search_field(Some("both"), SearchField::Edid).unwrap(),
        SearchField::Both
    );
}

#[test]
fn search_field_none_input_yields_default() {
    assert_eq!(
        search_field(None, SearchField::Name).unwrap(),
        SearchField::Name
    );
}

#[test]
fn search_field_rejects_unknown_string() {
    assert!(search_field(Some("nope"), SearchField::Both).is_err());
}

#[test]
fn body_detail_parses_known_strings() {
    assert_eq!(
        body_detail(Some("none"), BodyDetail::Full).unwrap(),
        BodyDetail::None
    );
    assert_eq!(
        body_detail(Some("stub"), BodyDetail::Full).unwrap(),
        BodyDetail::Stub
    );
    assert_eq!(
        body_detail(Some("full"), BodyDetail::None).unwrap(),
        BodyDetail::Full
    );
}

#[test]
fn body_detail_none_input_yields_default() {
    assert_eq!(
        body_detail(None, BodyDetail::Stub).unwrap(),
        BodyDetail::Stub
    );
}

#[test]
fn body_detail_rejects_unknown_string() {
    assert!(body_detail(Some("nope"), BodyDetail::Full).is_err());
}

#[test]
fn filter_op_parses_all_known_strings() {
    assert_eq!(filter_op("exists").unwrap(), FilterOp::Exists);
    assert_eq!(filter_op("eq").unwrap(), FilterOp::Eq);
    assert_eq!(filter_op("contains").unwrap(), FilterOp::Contains);
    assert_eq!(filter_op("gt").unwrap(), FilterOp::Gt);
    assert_eq!(filter_op("lt").unwrap(), FilterOp::Lt);
    assert_eq!(filter_op("gte").unwrap(), FilterOp::Gte);
    assert_eq!(filter_op("lte").unwrap(), FilterOp::Lte);
}

#[test]
fn filter_op_rejects_unknown_string() {
    assert!(filter_op("nope").is_err());
}

#[test]
fn clamp_ref_depth_none_defaults_to_one() {
    assert_eq!(clamp_ref_depth(None), 1);
}

#[test]
fn clamp_ref_depth_clamps_to_range() {
    assert_eq!(clamp_ref_depth(Some(0)), 1);
    assert_eq!(clamp_ref_depth(Some(1)), 1);
    assert_eq!(clamp_ref_depth(Some(3)), 3);
    assert_eq!(clamp_ref_depth(Some(1000)), esm::ipc::DEFAULT_MAX_DEPTH);
}

#[test]
fn diff_options_uppercases_exclude_types() {
    let opts = diff_options(BodyDetail::Stub, false, vec!["land", "Navm"]);
    assert_eq!(opts.bodies, BodyDetail::Stub);
    assert!(!opts.suppress_noise);
    assert_eq!(
        opts.exclude_types,
        vec!["LAND".to_string(), "NAVM".to_string()]
    );
}

#[test]
fn diff_options_empty_exclude_types() {
    let opts = diff_options(BodyDetail::Full, true, Vec::<String>::new());
    assert!(opts.exclude_types.is_empty());
}
