//! Compile-time preparation of the embedded schema files.
//!
//! - `schema_digest.rs`: the `xref` cache section is derived from a full
//!   schema decode of every record, so its on-disk identity must change
//!   whenever the embedded schema does. This digest is folded into that
//!   section's fingerprint (see `src/index.rs`), so a rebuilt binary with a
//!   different schema rebuilds `xref` instead of reading one built from the
//!   old schema.
//! - `schema_records.rs`: `fo76.json` split into one raw-JSON string per
//!   record type, so opening the embedded schema doesn't scan 2 MB of JSON;
//!   each definition is still parsed on first use (see `src/schema.rs`).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use serde_json::value::RawValue;

const SCHEMA_FILES: [&str; 2] = ["schema/fo76.json", "schema/fo76.ctda.json"];

fn main() {
    let mut digest: u64 = 0xcbf2_9ce4_8422_2325;
    for file in SCHEMA_FILES {
        println!("cargo:rerun-if-changed={file}");
        let bytes = std::fs::read(file).unwrap_or_else(|e| panic!("reading {file}: {e}"));
        for b in bytes {
            digest ^= u64::from(b);
            digest = digest.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let out = Path::new(&out_dir).join("schema_digest.rs");
    std::fs::write(
        out,
        format!("pub(crate) const SCHEMA_DIGEST: u64 = {digest:#018x};\n"),
    )
    .expect("writing schema_digest.rs");

    #[derive(serde::Deserialize)]
    struct RawSchema<'a> {
        #[serde(borrow)]
        records: BTreeMap<&'a str, &'a RawValue>,
    }
    let text = std::fs::read_to_string(SCHEMA_FILES[0]).expect("reading fo76.json");
    let schema: RawSchema = serde_json::from_str(&text).expect("parsing fo76.json");
    let mut table = String::from("static EMBEDDED_RECORDS: &[(&str, &str)] = &[\n");
    for (sig, raw) in schema.records {
        writeln!(table, "    ({sig:?}, {:?}),", raw.get()).unwrap();
    }
    table.push_str("];\n");
    std::fs::write(Path::new(&out_dir).join("schema_records.rs"), table)
        .expect("writing schema_records.rs");
}
