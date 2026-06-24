//! BA2 archive reader for Fallout 76/FO4 General (GNRL) archives.
//!
//! Ported and extended from `esm-parser/src/ba2.rs`.  Changes vs. the original:
//! - `Ba2Entry` exposes `name_hash`, `dir_hash`, `ext`, and `flags` for display.
//! - `read()` is codec-aware: it sniffs the first two bytes to detect zlib vs
//!   LZ4, and accepts an explicit `Codec` override.
//! - DX10 texture archives are detected and rejected with a clear error.
//! - Version != 1 causes an error rather than a warning.

use crate::compress::{decompress, Codec};
use crate::format::{
    read_header, read_record, Header, HEADER_SIZE, RECORD_SIZE, TAG_DX10, TAG_GNRL, VERSION,
};
use anyhow::{bail, Context, Result};
use memmap2::Mmap;
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;

/// A single file entry in a BA2 GNRL archive.
#[derive(Debug, Clone)]
pub struct Ba2Entry {
    /// Lowercase path as stored in the name table (backslash-separated).
    pub name: String,
    pub name_hash: u32,
    pub dir_hash: u32,
    pub ext: [u8; 4],
    pub flags: u32,
    pub data_offset: u64,
    /// Compressed size; 0 means the data is stored uncompressed.
    pub packed_size: u32,
    pub unpacked_size: u32,
}

impl Ba2Entry {
    /// True when this entry's blob is compressed.
    pub fn is_compressed(&self) -> bool {
        self.packed_size != 0
    }
}

/// An open BA2 GNRL archive, memory-mapped for zero-copy reads.
pub struct Ba2Archive {
    mmap: Mmap,
    pub entries: Vec<Ba2Entry>,
    by_name: HashMap<String, usize>,
    pub header: Header,
}

impl Ba2Archive {
    /// Open and parse a BTDX/GNRL BA2 archive at `path`.
    ///
    /// Returns an error for DX10 texture archives, unsupported versions, and
    /// for truncated or otherwise malformed files.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let file =
            File::open(path).with_context(|| format!("failed to open BA2: {}", path.display()))?;
        // SAFETY: We hold `file` open for the entire lifetime of `mmap`.
        // No other process is expected to truncate the file while it is mapped.
        let mmap = unsafe { Mmap::map(&file)? };
        let data = &*mmap;

        let header = read_header(data)?;

        if header.version != VERSION {
            bail!(
                "unsupported BA2 version {} (only version {} is supported)",
                header.version,
                VERSION
            );
        }
        if &header.archive_type == TAG_DX10 {
            bail!("DX10 (texture) BA2 archives are not supported; only GNRL archives");
        }
        if &header.archive_type != TAG_GNRL {
            bail!(
                "unsupported BA2 archive type {:?}; expected GNRL",
                &header.archive_type
            );
        }

        let file_count = header.file_count as usize;
        let records_start = HEADER_SIZE;
        let records_end = records_start
            .checked_add(file_count * RECORD_SIZE)
            .ok_or_else(|| anyhow::anyhow!("BA2 file-count overflow"))?;
        if records_end > data.len() {
            bail!("BA2 file records extend past end of file");
        }

        // Read all records first; names come from the name table.
        // Re-use the public Record type to avoid a complex local tuple.
        let mut raw: Vec<crate::format::Record> = Vec::with_capacity(file_count);
        for i in 0..file_count {
            let base = records_start + i * RECORD_SIZE;
            raw.push(read_record(data, base));
        }

        // Parse name table.
        let nt_start = header.name_table_offset as usize;
        if nt_start >= data.len() {
            bail!("BA2 name table offset out of range");
        }
        let mut pos = nt_start;
        let mut entries: Vec<Ba2Entry> = Vec::with_capacity(file_count);
        let mut by_name: HashMap<String, usize> = HashMap::with_capacity(file_count);

        for (i, r) in raw.into_iter().enumerate() {
            let (name_hash, ext, dir_hash, flags, data_offset, packed_size, unpacked_size) = (
                r.name_hash,
                r.ext,
                r.dir_hash,
                r.flags,
                r.data_offset,
                r.packed_size,
                r.unpacked_size,
            );
            if pos + 2 > data.len() {
                bail!("BA2 name table entry {} truncated (no length prefix)", i);
            }
            let name_len = u16::from_le_bytes(data[pos..pos + 2].try_into().unwrap()) as usize;
            pos += 2;
            if pos + name_len > data.len() {
                bail!("BA2 name table entry {} string bytes out of range", i);
            }
            // Normalise to backslash and lowercase so `read()` lookups are
            // consistent regardless of whether the archive uses `/` or `\`.
            let name = String::from_utf8_lossy(&data[pos..pos + name_len])
                .to_lowercase()
                .replace('/', "\\");
            pos += name_len;
            by_name.insert(name.clone(), i);
            entries.push(Ba2Entry {
                name,
                name_hash,
                dir_hash,
                ext,
                flags,
                data_offset,
                packed_size,
                unpacked_size,
            });
        }

        Ok(Ba2Archive {
            mmap,
            entries,
            by_name,
            header,
        })
    }

    /// Return all file entries in the archive.
    pub fn list(&self) -> &[Ba2Entry] {
        &self.entries
    }

    /// Extract and decompress a named file.
    ///
    /// `name` is matched case-insensitively.  `codec` controls decompression:
    /// `Auto` (default) sniffs the blob for zlib vs LZ4.
    pub fn read(&self, name: &str, codec: Codec) -> Result<Vec<u8>> {
        // Names in the archive are lowercased and backslash-separated.
        // Normalise the caller's input to match.
        let name_lower = name.to_lowercase().replace('/', "\\");
        let &idx = self
            .by_name
            .get(&name_lower)
            .ok_or_else(|| anyhow::anyhow!("file not found in BA2: {}", name))?;
        let entry = &self.entries[idx];
        let data = &*self.mmap;

        let start = entry.data_offset as usize;
        let stored_len = if entry.packed_size == 0 {
            entry.unpacked_size as usize
        } else {
            entry.packed_size as usize
        };

        if start.saturating_add(stored_len) > data.len() {
            bail!("BA2 entry '{}' data out of range", entry.name);
        }
        let raw = &data[start..start + stored_len];

        if entry.packed_size == 0 {
            // Stored uncompressed.
            Ok(raw.to_vec())
        } else {
            decompress(raw, entry.unpacked_size, codec)
                .with_context(|| format!("decompression failed for '{}'", entry.name))
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{write_header, write_record, Record, RECORD_FLAGS};
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn make_test_archive(entries: &[(&str, &[u8])]) -> NamedTempFile {
        // Build a minimal stored (uncompressed) GNRL BA2.
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

        // Build records.
        let mut records_bytes: Vec<u8> = Vec::new();
        for (i, (path, data)) in entries.iter().enumerate() {
            let (name_hash, dir_hash, ext) = crate::hash::hash_path(path);
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

        // Name table.
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

    #[test]
    fn open_and_read_stored_entries() {
        let entries = &[
            ("interface/test.txt", b"hello world" as &[u8]),
            ("data/config.bin", b"\x00\x01\x02\x03"),
        ];
        let tmp = make_test_archive(entries);
        let archive = Ba2Archive::open(tmp.path()).unwrap();
        assert_eq!(archive.list().len(), 2);

        let txt = archive.read("interface/test.txt", Codec::Auto).unwrap();
        assert_eq!(txt, b"hello world");

        let bin = archive.read("DATA/CONFIG.BIN", Codec::Auto).unwrap();
        assert_eq!(bin, b"\x00\x01\x02\x03");
    }

    #[test]
    fn missing_entry_returns_error() {
        let entries = &[("foo/bar.txt", b"data" as &[u8])];
        let tmp = make_test_archive(entries);
        let archive = Ba2Archive::open(tmp.path()).unwrap();
        assert!(archive.read("foo/missing.txt", Codec::Auto).is_err());
    }
}
