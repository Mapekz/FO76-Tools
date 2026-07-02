//! Exhaustive full-decode integration test for all record types that are known
//! to decode completely (zero `_unmapped`, `raw_fallback`, and
//! `_unknown_record` markers) against the real game ESM.
//!
//! # Running this test
//!
//! ```sh
//! RUST_TEST_ESM=/path/to/Game.esm cargo test
//! ```
//!
//! The test skips silently when `RUST_TEST_ESM` is unset, so `cargo test` in
//! CI (where the game ESM is unavailable) passes without extra flags.
//!
//! # What is checked
//!
//! For every record of every **clean** type listed in `CLEAN_TYPES`:
//! - `_record_type` resolves to a non-empty string (schema lookup succeeded).
//! - No `_unknown_record` marker (the signature is in the schema).
//! - No `raw_fallback` markers (`_raw: true` + `reason` key anywhere in the
//!   decoded tree).
//! - No `_unmapped` subrecords (every subrecord signature was consumed by the
//!   schema).
//!
//! `_unresolved` (unresolved LString IDs) is intentionally NOT checked — that
//! marker indicates a missing localization BA2, not a decode bug.
//!
//! # Skipped (dirty) types
//!
//! The following types are **excluded** from `CLEAN_TYPES` because they still
//! emit `raw_fallback` or undocumented `_unmapped` markers on at least some
//! records in a reference ESM.
//! Types marked partial† in the README decode only with documented drift
//! (`LVLD` / `NAM5`) and have drift-locked tests in `decode_records.rs`.
//!
//! Recently cleaned (now in `CLEAN_TYPES` or basic-tested): TERM, FLOR, FURN,
//! INFO, MISC, QMDL, NOTE, ENCH, BOOK, WEAP, PERK, RACE, CONT,
//! LVLI, LVLN, LVPC, LVLP, RESO, GMRW, QUST, NPC_.
//! Batch-promoted 2026-06-27: 97 additional types confirmed zero-marker on
//! a reference ESM — AACT, AAMD, ADDN, AECH, ANIO, AORU, ARMA, ARTO,
//! ASPC, ASTM, ASTP, ATXO, AUVF, AVTR, BNDS, CAMS, CLAS, CLFM, CLMT, CNCY,
//! CNDF, CPRD, CPTH, CSEN, CSTY, DCGF, DEBR, DIAL, DIST, DOBJ, ECAT, EFSH,
//! EMOT, EQUP, FSTP, FSTS, GCVR, GRAS, HDPT, IDLE, IDLM, IMAD, IMGS, INGR,
//! IPCT, IPDS, KSSM, LAYR, LCRT, LCTN, LENS, LOUT, LSCR, LTEX, MATO, MATT,
//! MESG, MOVT, MUSC, MUST, NAVI, NOCM, OVIS, PACH, PACK, PKIN, PMFT, PPAK,
//! REGN, RELA, REVB, RFCT, RFGP, SCEN, SCOL, SCSN, SECH, SMBN, SMEN, SMQN,
//! SNCT, SNDR, SOPM, SOUN, SPGD, STAG, STHD, STMP, STND, TRNS, TXST, UTIL,
//! VOLI, VTYP, WATR, WSPR, ZOOM.
//! Also promoted: MGEF (3109 records, zero markers — prior exclusion was stale).
//! Batch-promoted 2026-07-02 (schema parity A+B): AAPD, ACHR, ACTI, COLL,
//! DLBR, DOOR, KEYM, LGTM, LIGH, MSTT, PGRE, PHZD, PLYR, PMIS, REFR, SCCO,
//! STAT, TACT.
//! DLVW, GDRY, TREE absent from this ESM version (no records); deferred.

mod common;

use common::collect_decode_problems;
use esm::{Database, FormId};

/// All record types verified (via `esm coverage`) to decode with zero markers
/// on every record in a reference ESM.
const CLEAN_TYPES: &[&str] = &[
    // Original + incrementally promoted batch
    "ARMO", "SPEL", "GLOB", "KYWD", "OMOD", "AMMO", "PROJ", "EXPL", "ALCH", "COBJ", "ENTM", "DMGT",
    "FISH", "FACT", "FLST", "WTHR", "WAVE", "OTFT", "MSWP", "CURV", "DFOB", "CHAL", "CMPO", "CMPT",
    "COEN", "MDSP", "TEPF", "TRAP", "LGDI", "AVIF", "BPTD", "PEPF", "PCRD", "PLYT", "HAZD", "INNR",
    "GMST", "AMDL", "ENCH", "BOOK", "WEAP", "PERK", "TERM", "FLOR", "FURN", "INFO", "MISC", "QMDL",
    "NOTE", "RACE", "CONT", "LVLI", "LVLN", "LVPC", "LVLP", "RESO", "GMRW", "QUST", "NPC_",
    // Batch-promoted (97 types confirmed zero-marker on a reference ESM)
    "AACT", "AAMD", "ADDN", "AECH", "ANIO", "AORU", "ARMA", "ARTO", "ASPC", "ASTM", "ASTP", "ATXO",
    "AUVF", "AVTR", "BNDS", "CAMS", "CLAS", "CLFM", "CLMT", "CNCY", "CNDF", "CPRD", "CPTH", "CSEN",
    "CSTY", "DCGF", "DEBR", "DIAL", "DIST", "DOBJ", "ECAT", "EFSH", "EMOT", "EQUP", "FSTP", "FSTS",
    "GCVR", "GRAS", "HDPT", "IDLE", "IDLM", "IMAD", "IMGS", "INGR", "IPCT", "IPDS", "KSSM", "LAYR",
    "LCRT", "LCTN", "LENS", "LOUT", "LSCR", "LTEX", "MATO", "MATT", "MESG", "MOVT", "MUSC", "MUST",
    "NAVI", "NOCM", "OVIS", "PACH", "PACK", "PKIN", "PMFT", "PPAK", "REGN", "RELA", "REVB", "RFCT",
    "RFGP", "SCEN", "SCOL", "SCSN", "SECH", "SMBN", "SMEN", "SMQN", "SNCT", "SNDR", "SOPM", "SOUN",
    "SPGD", "STAG", "STHD", "STMP", "STND", "TRNS", "TXST", "UTIL", "VOLI", "VTYP", "WATR", "WSPR",
    "ZOOM", "MGEF",
    // Batch-promoted 2026-07-02 (schema parity A+B — zero markers on reference ESM)
    "AAPD", "ACHR", "ACTI", "COLL", "DLBR", "DOOR", "KEYM", "LGTM", "LIGH", "MSTT", "PGRE", "PHZD",
    "PLYR", "PMIS", "REFR", "SCCO", "STAT", "TACT",
];

/// Walk `v` and count every `_unmapped`, `raw_fallback`, and `_unknown_record`
/// marker found anywhere in its JSON tree.  Returns the count plus a sample of
/// problem descriptions (capped at 5 to keep failure messages readable).
fn count_problems(v: &serde_json::Value) -> (usize, Vec<String>) {
    let problems = collect_decode_problems(v);
    let count = problems.len();
    let sample: Vec<String> = problems.into_iter().take(5).collect();
    (count, sample)
}

#[test]
fn decode_all_clean_types_fully() {
    let Ok(esm_path) = std::env::var("RUST_TEST_ESM") else {
        eprintln!("RUST_TEST_ESM not set — skipping");
        return;
    };

    let mut db = Database::open(&esm_path)
        .unwrap_or_else(|e| panic!("failed to open ESM at {esm_path:?}: {e}"));

    let mut total_records: u64 = 0;
    let mut failed_types: Vec<(String, u64, Vec<String>)> = Vec::new();

    for &sig in CLEAN_TYPES {
        // list_by_type uses .take(limit) internally; usize::MAX means no cap.
        let entries = db
            .list_by_type(sig, usize::MAX)
            .unwrap_or_else(|e| panic!("list_by_type({sig}) failed: {e}"));

        let mut type_problems: u64 = 0;
        let mut first_samples: Vec<String> = Vec::new();

        for entry in &entries {
            total_records += 1;

            let fid: FormId = esm::parse_form_id_input(&entry.form_id)
                .unwrap_or_else(|e| panic!("bad FormID {}: {e}", entry.form_id));

            let result = db
                .record_by_formid(fid)
                .unwrap_or_else(|e| panic!("record_by_formid({}) failed: {e}", entry.form_id));

            let (n, samples) = count_problems(&result.fields);
            if n > 0 {
                type_problems += n as u64;
                if first_samples.len() < 5 {
                    let edid = entry.editor_id.as_deref().unwrap_or("<no edid>");
                    for s in &samples {
                        first_samples.push(format!("  [{}] {}: {}", entry.form_id, edid, s));
                    }
                }
            }
        }

        if type_problems > 0 {
            failed_types.push((sig.to_string(), type_problems, first_samples));
        }

        eprintln!(
            "{sig:5}  {} records  {} problem(s)",
            entries.len(),
            if type_problems == 0 {
                "0".to_string()
            } else {
                format!("\x1b[31m{type_problems}\x1b[0m")
            }
        );
    }

    assert!(
        failed_types.is_empty(),
        "decode problems found in {}/{} types ({total_records} records checked):\n{}",
        failed_types.len(),
        CLEAN_TYPES.len(),
        failed_types
            .iter()
            .map(|(sig, n, samples)| format!("  {sig}: {n} problem(s)\n{}", samples.join("\n")))
            .collect::<Vec<_>>()
            .join("\n")
    );

    eprintln!(
        "\nAll {} clean types passed ({total_records} records checked)",
        CLEAN_TYPES.len()
    );
}
