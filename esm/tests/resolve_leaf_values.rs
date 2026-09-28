//! Integration tests for value-bearing leaf inlining at `--resolve stub`/`full`
//! (see `src/decode/leaf_values.rs` and `esm/docs/adr/0011-value-bearing-leaf-inlining.md`).
//!
//! All records here are synthetic in-memory byte buffers (no real ESM
//! required), following the conventions in `tests/curves.rs` and
//! `tests/decode_records/`. A CHAL record's `HNAM` ("Required Count
//! Global") is decoded directly via `decode_record` rather than through a
//! full second ESM file, so each test only needs to build the *target*
//! record (GLOB/CURV) into a real `Database` for the resolver to look up.

mod common;

use common::bare_ctx_fv;
use esm::ctda::decode_ctda;
use esm::decode::{ResolveDepth, decode_record};
use esm::schema::Schema;
use esm::{Database, DatabaseResolver, FormId};

/// Build a synthetic ESM containing one GLOB record: `EDID` always present,
/// `FLTV` (the `Value` field) only when `value` is `Some`.
fn glob_esm(form_id: u32, edid: &str, value: Option<f32>) -> Vec<u8> {
    let mut subs = Vec::new();
    common::append_subrecord(&mut subs, b"EDID", &common::cstr(edid));
    if let Some(v) = value {
        common::append_subrecord(&mut subs, b"FLTV", &v.to_le_bytes());
    }
    let mut records = Vec::new();
    common::append_record(&mut records, b"GLOB", form_id, &subs);

    let mut esm_buf = common::tes4_header();
    esm_buf.extend(common::wrap_grup(b"GLOB", &records));
    esm_buf
}

/// A minimal CHAL `HNAM` ("Required Count Global") subrecord set: `EDID` plus
/// the FormID-typed reference. Decoded directly via `decode_record` — no
/// second synthetic ESM needed, since only the *target* GLOB has to live in
/// a real `Database` for the resolver to look up.
fn chal_subs_referencing(target: FormId) -> Vec<esm::reader::OwnedSubrecord> {
    let mut buf = Vec::new();
    common::append_subrecord(&mut buf, b"EDID", &common::cstr("TestChallenge"));
    common::append_subrecord(&mut buf, b"HNAM", &target.0.to_le_bytes());
    // `append_subrecord`'s output is raw on-wire bytes; re-parse it into
    // `OwnedSubrecord`s the same way `EsmFile::parse_record_at` would, so
    // `decode_record` sees exactly what a real record's subrecords look like.
    esm::reader::parse_subrecords_owned(&buf).expect("well-formed synthetic subrecord stream")
}

/// A raw 32-byte CTDA block. `use_global`/`is_or` set via `type_byte`;
/// `comp_value` is either a float (LE f32) or a FormID (LE u32), matching
/// `decode_ctda`'s layout (see `src/ctda.rs`). `ref_id` (bytes 24-27) is a
/// second, independent FormID reference — `decode_ctda` resolves it
/// unconditionally whenever nonzero, always with an EMPTY `valid_refs` slice
/// (`resolve_formid(ctx, &[], ref_id)`), which is exactly the call shape a
/// `valid_refs`-keyed inline rule can never reach.
fn ctda_bytes(use_global: bool, comp_value_bits: u32, ref_id: u32) -> Vec<u8> {
    let mut b = [0u8; 32];
    b[0] = if use_global { 0x04 } else { 0x00 };
    b[4..8].copy_from_slice(&comp_value_bits.to_le_bytes());
    b[24..28].copy_from_slice(&ref_id.to_le_bytes());
    b.to_vec()
}

#[test]
fn glob_ref_via_schema_field_inlines_value_at_stub() {
    let target = FormId::new(0x100);
    let (esm_path, db) = common::write_and_open(
        &glob_esm(target.0, "SDOW_SomeGlob", Some(76.0)),
        "glob_leaf_schema_field",
    );

    let schema = Schema::load_embedded().expect("embedded schema must load");
    let resolver = DatabaseResolver::new(&db, 2);
    let mut ctx = bare_ctx_fv(&schema, 208);
    ctx.resolve_depth = ResolveDepth::Stub;
    ctx.resolver = Some(&resolver);

    let chal = decode_record(&ctx, "CHAL", &chal_subs_referencing(target));
    std::fs::remove_file(&esm_path).ok();

    let global = &chal["Required Count Global"];
    assert_eq!(global["record_type"], serde_json::json!("GLOB"));
    assert_eq!(global["editor_id"], serde_json::json!("SDOW_SomeGlob"));
    assert_eq!(global["Value"], serde_json::json!(76.0));
}

/// The load-bearing case: `decode_ctda` always calls `resolve_formid` with an
/// EMPTY `valid_refs` for its `Reference`/use-global-comparison fields
/// (`src/ctda.rs:280,312,358`), so a `valid_refs`-keyed inline rule could
/// never reach it — this is why the leaf table is keyed on the *target's*
/// record signature instead, and it's the reason `walk`'s `resolve_glob_ref`
/// round-trip workaround becomes unnecessary.
#[test]
fn glob_ref_via_ctda_operand_inlines_value_at_stub() {
    let target = FormId::new(0x101);
    let (esm_path, db) = common::write_and_open(
        &glob_esm(target.0, "SDOW_ComparisonGlob", Some(40.0)),
        "glob_leaf_ctda",
    );

    let schema = Schema::load_embedded().expect("embedded schema must load");
    let resolver = DatabaseResolver::new(&db, 2);
    let mut ctx = bare_ctx_fv(&schema, 208);
    ctx.resolve_depth = ResolveDepth::Stub;
    ctx.resolver = Some(&resolver);

    // use_global=true: bytes 4-7 carry `target` as a FormID, not a float.
    let cond = decode_ctda(&ctda_bytes(true, target.0, 0), &ctx);
    std::fs::remove_file(&esm_path).ok();

    let comp = &cond["Comparison Value"];
    assert_eq!(comp["record_type"], serde_json::json!("GLOB"));
    assert_eq!(comp["Value"], serde_json::json!(40.0));
}

#[test]
fn resolve_none_emits_bare_hex_for_glob_ref() {
    let target = FormId::new(0x102);
    let (esm_path, _db) = common::write_and_open(
        &glob_esm(target.0, "SDOW_NoneDepthGlob", Some(5.0)),
        "glob_leaf_none",
    );

    let schema = Schema::load_embedded().expect("embedded schema must load");
    // Mirrors `Database::record_at_meta_with_depth` at `ResolveDepth::None`:
    // no resolver constructed at all — the depth `diff::run` decodes both
    // snapshot sides at.
    let ctx = bare_ctx_fv(&schema, 208);
    assert_eq!(ctx.resolve_depth, ResolveDepth::None);
    assert!(ctx.resolver.is_none());

    let chal = decode_record(&ctx, "CHAL", &chal_subs_referencing(target));
    std::fs::remove_file(&esm_path).ok();

    assert_eq!(
        chal["Required Count Global"],
        serde_json::json!(target.display()),
        "at --resolve none the reference must stay a bare hex string"
    );
}

/// A genuine `Full` expansion carries the value at `fields.Value` (from
/// decoding the whole target record), and must NOT also duplicate it as a
/// top-level flat `Value` — that flat key exists only to enrich a *stub*,
/// and a duplicate would erase the shape signal that distinguishes a real
/// expansion from `decode_full`'s stub-shaped fallbacks (depth limit, index
/// miss).
#[test]
fn resolve_full_keeps_value_under_fields_only() {
    let target = FormId::new(0x103);
    let (esm_path, db) = common::write_and_open(
        &glob_esm(target.0, "SDOW_FullDepthGlob", Some(12.5)),
        "glob_leaf_full",
    );

    let schema = Schema::load_embedded().expect("embedded schema must load");
    let resolver = DatabaseResolver::new(&db, 2);
    let mut ctx = bare_ctx_fv(&schema, 208);
    ctx.resolve_depth = ResolveDepth::Full;
    ctx.resolver = Some(&resolver);

    let chal = decode_record(&ctx, "CHAL", &chal_subs_referencing(target));
    std::fs::remove_file(&esm_path).ok();

    let global = &chal["Required Count Global"];
    assert_eq!(global["fields"]["Value"], serde_json::json!(12.5));
    assert!(
        global.get("Value").is_none(),
        "a genuine Full expansion must not also carry a top-level flat Value: {global}"
    );
}

/// When the target GLOB has no `FLTV` subrecord at all, `leaf_inline` must
/// decline (no field to lift) and the reference degrades byte-identically to
/// the plain three-key stub — never an error, never a `Value: null`.
#[test]
fn glob_ref_with_no_fltv_degrades_to_plain_stub() {
    let target = FormId::new(0x104);
    let (esm_path, db) = common::write_and_open(
        &glob_esm(target.0, "SDOW_ValuelessGlob", None),
        "glob_leaf_no_fltv",
    );

    let schema = Schema::load_embedded().expect("embedded schema must load");
    let resolver = DatabaseResolver::new(&db, 2);
    let mut ctx = bare_ctx_fv(&schema, 208);
    ctx.resolve_depth = ResolveDepth::Stub;
    ctx.resolver = Some(&resolver);

    let chal = decode_record(&ctx, "CHAL", &chal_subs_referencing(target));
    std::fs::remove_file(&esm_path).ok();

    let global = &chal["Required Count Global"];
    assert_eq!(
        global,
        &serde_json::json!({
            "formid": target.display(),
            "editor_id": "SDOW_ValuelessGlob",
            "record_type": "GLOB",
        })
    );
}

/// Target-signature keying means a CURV reached through an empty-`valid_refs`
/// call site (here, `decode_ctda`'s `Reference` field — same shape as the
/// real `PROJ."Speed Curve Table"` / `"Seek Strength Curve Table"` fields,
/// whose schema `valid_refs` is also empty) inlines its points too, not just
/// bare hex. No `record_type` key on the result — CURV's inline shape is
/// deliberately the same one `resolve_formid`'s `valid_refs`-keyed branch
/// produces.
#[test]
fn curv_ref_from_empty_valid_refs_inlines_via_target_signature() {
    let dir = std::env::temp_dir().join(format!(
        "fo76_esm_test_dir_curv_leaf_target_sig_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create isolated test dir");

    let target = FormId::new(0x105);
    let mut subs = Vec::new();
    common::append_subrecord(&mut subs, b"EDID", &common::cstr("CT_Leaf_Target_Sig"));
    common::append_subrecord(
        &mut subs,
        b"JASF",
        &common::cstr(r"LegendaryMods\Weapon_DamagePerKill.json"),
    );
    let mut records = Vec::new();
    common::append_record(&mut records, b"CURV", target.0, &subs);
    let mut esm_buf = common::tes4_header();
    esm_buf.extend(common::wrap_grup(b"CURV", &records));
    let esm_path = dir.join("Test.esm");
    std::fs::write(&esm_path, &esm_buf).expect("write test esm");

    let curve_json_dir = dir.join("misc/curvetables/json/legendarymods");
    std::fs::create_dir_all(&curve_json_dir).expect("create curve json dir");
    std::fs::write(
        curve_json_dir.join("weapon_damageperkill.json"),
        br#"{"curve":[{"x":0,"y":0},{"x":1,"y":10},{"x":10,"y":100}]}"#,
    )
    .expect("write curve json fixture");

    let db = Database::open(&dir).expect("open db (dir form, auto-discovers misc/curvetables)");
    let schema = Schema::load_embedded().expect("embedded schema must load");
    let resolver = DatabaseResolver::new(&db, 2);
    let mut ctx = bare_ctx_fv(&schema, 208);
    ctx.resolve_depth = ResolveDepth::Stub;
    ctx.resolver = Some(&resolver);
    // Deliberately NOT setting ctx.curves — proves the inline came from the
    // resolver path (`DatabaseResolver::leaf_inline`), not from
    // `resolve_formid`'s depth-independent `valid_refs` branch, which reads
    // `ctx.curves` directly and would be a no-op here.
    assert!(ctx.curves.is_none());

    // use_global=false, comp value irrelevant; ref_id (bytes 24-27) is the
    // CURV target, resolved with an always-empty valid_refs slice.
    let cond = decode_ctda(&ctda_bytes(false, 0, target.0), &ctx);

    std::fs::remove_dir_all(&dir).ok();

    let reference = &cond["Reference"];
    assert!(
        reference.get("record_type").is_none(),
        "CURV inline shape must not gain record_type: {reference}"
    );
    assert_eq!(
        reference["curve"],
        serde_json::json!([
            {"x": 0.0, "y": 0.0},
            {"x": 1.0, "y": 10.0},
            {"x": 10.0, "y": 100.0},
        ])
    );
}
