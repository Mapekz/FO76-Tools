//! Shared test helpers for ba2 integration tests.
//!
//! This module is not compiled as its own test binary (it lives under a
//! subdirectory, so Cargo ignores it as a test target).  Individual test files
//! pull it in with `mod common;`.

use ba2::format::{write_header, write_record, Record, RECORD_FLAGS};
use ba2::hash::hash_path;
use std::io::Write;
use tempfile::NamedTempFile;

/// Build a minimal stored (uncompressed) GNRL BA2 archive in a temp file.
///
/// `entries` is a slice of `(archive_path, data)` pairs.  Paths may use `/`
/// or `\`; they are lowercased and backslash-normalised in the name table.
pub fn make_test_archive(entries: &[(&str, &[u8])]) -> NamedTempFile {
    let file_count = entries.len() as u32;
    let data_start = 24u64 + 36 * file_count as u64;

    // Compute per-entry data offsets.
    let mut offsets = Vec::new();
    let mut cursor = data_start;
    for (_, data) in entries {
        offsets.push(cursor);
        cursor += data.len() as u64;
    }
    let name_table_offset = cursor;

    let header_bytes = write_header(1, file_count, name_table_offset);

    let mut records_bytes: Vec<u8> = Vec::new();
    for (i, (path, data)) in entries.iter().enumerate() {
        let (name_hash, dir_hash, ext) = hash_path(path);
        let r = Record {
            name_hash,
            ext,
            dir_hash,
            flags: RECORD_FLAGS,
            data_offset: offsets[i],
            packed_size: 0,
            unpacked_size: data.len() as u32,
        };
        records_bytes.extend_from_slice(&write_record(&r));
    }

    let mut name_table: Vec<u8> = Vec::new();
    for (path, _) in entries {
        let p = path.to_lowercase().replace('/', "\\");
        let len = p.len() as u16;
        name_table.extend_from_slice(&len.to_le_bytes());
        name_table.extend_from_slice(p.as_bytes());
    }

    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all(&header_bytes).unwrap();
    tmp.write_all(&records_bytes).unwrap();
    for (_, data) in entries {
        tmp.write_all(data).unwrap();
    }
    tmp.write_all(&name_table).unwrap();
    tmp.flush().unwrap();
    tmp
}
