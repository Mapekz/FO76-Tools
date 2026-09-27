//! Compile-time digest of the embedded schema files.
//!
//! The `xref` cache section is derived from a full schema decode of every
//! record, so its on-disk identity must change whenever the embedded schema
//! does. This digest is folded into that section's fingerprint (see
//! `src/index.rs`), so a rebuilt binary with a different schema rebuilds
//! `xref` instead of reading one built from the old schema.

use std::path::Path;

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
    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).join("schema_digest.rs");
    std::fs::write(
        out,
        format!("pub(crate) const SCHEMA_DIGEST: u64 = {digest:#018x};\n"),
    )
    .expect("writing schema_digest.rs");
}
