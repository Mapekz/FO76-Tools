//! BA2 archive reader for Fallout 76 General archives.
//!
//! Parses BTDX/GNRL BA2 files for extracting named file blobs.
//! Texture (DX10) archives are not supported.

use anyhow::{bail, Context, Result};
use memmap2::Mmap;
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;

/// A single file entry in a BA2 GNRL archive.
pub struct Ba2Entry {
    /// Lowercase, forward-slash-normalized path (backslashes in the raw name
    /// table, as used by real Bethesda-shipped archives, are converted to `/`).
    pub name: String,
    pub data_offset: u64,
    /// Compressed size; 0 means the data is stored uncompressed.
    pub packed_size: u32,
    pub unpacked_size: u32,
}

/// An open BA2 GNRL archive, memory-mapped for zero-copy reads.
pub struct Ba2Archive {
    mmap: Mmap,
    pub entries: Vec<Ba2Entry>,
    by_name: HashMap<String, usize>,
}

impl Ba2Archive {
    /// Open and parse a BTDX/GNRL BA2 archive at `path`.
    ///
    /// Returns an error for non-GNRL archives (e.g. DX10 texture archives)
    /// and for truncated or otherwise malformed files.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let file =
            File::open(path).with_context(|| format!("failed to open BA2: {}", path.display()))?;
        // SAFETY: We hold the file open for the lifetime of `Mmap`; no other process
        // is expected to truncate the file while it is mapped.
        let mmap = unsafe { Mmap::map(&file)? };
        let data = &*mmap;

        // Parse 24-byte header:
        //   [0..4]   magic       "BTDX"
        //   [4..8]   version     u32 LE (expect 1)
        //   [8..12]  archive_type "GNRL" or "DX10"
        //   [12..16] file_count  u32 LE
        //   [16..24] name_table_offset u64 LE
        if data.len() < 24 {
            bail!("BA2 too small to have a header");
        }
        if &data[0..4] != b"BTDX" {
            bail!("not a BA2 archive (bad magic)");
        }
        let version = u32::from_le_bytes(data[4..8].try_into().unwrap());
        if version != 1 {
            // Accept but warn — game may have version 2+ files
            log::warn!("unexpected BA2 version {}", version);
        }
        if &data[8..12] != b"GNRL" {
            bail!(
                "only GNRL (general) BA2 archives are supported; got {:?}",
                &data[8..12]
            );
        }
        let file_count = u32::from_le_bytes(data[12..16].try_into().unwrap());
        let name_table_offset = u64::from_le_bytes(data[16..24].try_into().unwrap());

        // Parse GNRL file records (36 bytes each, immediately after the 24-byte header):
        //   [0..4]   name_hash   u32
        //   [4..8]   ext         [u8; 4]
        //   [8..12]  dir_hash    u32
        //   [12..16] flags       u32
        //   [16..24] data_offset u64
        //   [24..28] packed_size u32  (0 = stored uncompressed)
        //   [28..32] unpacked_size u32
        //   [32..36] padding     u32  (0xBAADF00D)
        const RECORD_SIZE: usize = 36;
        let records_start = 24usize;
        let records_end = (file_count as usize)
            .checked_mul(RECORD_SIZE)
            .and_then(|n| records_start.checked_add(n))
            .ok_or_else(|| anyhow::anyhow!("BA2 file_count overflows record table size"))?;
        if records_end > data.len() {
            bail!("BA2 file records extend past end of file");
        }

        // Collect raw (offset, packed_size, unpacked_size) before reading name table.
        let mut raw_entries: Vec<(u64, u32, u32)> = Vec::with_capacity(file_count as usize);
        for i in 0..file_count as usize {
            let base = records_start + i * RECORD_SIZE;
            // Bounds already verified above.
            let data_offset = u64::from_le_bytes(data[base + 16..base + 24].try_into().unwrap());
            let packed_size = u32::from_le_bytes(data[base + 24..base + 28].try_into().unwrap());
            let unpacked_size = u32::from_le_bytes(data[base + 28..base + 32].try_into().unwrap());
            raw_entries.push((data_offset, packed_size, unpacked_size));
        }

        // Parse name table (at name_table_offset):
        //   For each file in record order: u16 length, then that many UTF-8 bytes.
        let nt_start = name_table_offset as usize;
        if nt_start >= data.len() {
            bail!("BA2 name table offset out of range");
        }
        let mut pos = nt_start;
        let mut entries: Vec<Ba2Entry> = Vec::with_capacity(raw_entries.len());
        let mut by_name: HashMap<String, usize> = HashMap::new();

        for (i, (data_offset, packed_size, unpacked_size)) in raw_entries.into_iter().enumerate() {
            if pos + 2 > data.len() {
                bail!("BA2 name table entry {} truncated", i);
            }
            let name_len = u16::from_le_bytes(data[pos..pos + 2].try_into().unwrap()) as usize;
            pos += 2;
            if pos + name_len > data.len() {
                bail!("BA2 name {} bytes out of range", i);
            }
            let name = String::from_utf8_lossy(&data[pos..pos + name_len])
                .replace('\\', "/")
                .to_lowercase();
            pos += name_len;
            by_name.insert(name.clone(), i);
            entries.push(Ba2Entry {
                name,
                data_offset,
                packed_size,
                unpacked_size,
            });
        }

        Ok(Ba2Archive {
            mmap,
            entries,
            by_name,
        })
    }

    /// Return all file entries in the archive.
    pub fn list(&self) -> &[Ba2Entry] {
        &self.entries
    }

    /// Extract and, if compressed, decompress a named file.
    ///
    /// `name` is matched case-insensitively.
    pub fn read(&self, name: &str) -> Result<Vec<u8>> {
        let name_lower = name.to_lowercase();
        let &idx = self
            .by_name
            .get(&name_lower)
            .ok_or_else(|| anyhow::anyhow!("file not found in BA2: {}", name))?;
        let entry = &self.entries[idx];
        let data = &*self.mmap;

        let start = entry.data_offset as usize;
        let compressed_len = if entry.packed_size == 0 {
            entry.unpacked_size as usize
        } else {
            entry.packed_size as usize
        };

        if start + compressed_len > data.len() {
            bail!("BA2 entry {} data out of range", entry.name);
        }
        let raw = &data[start..start + compressed_len];

        if entry.packed_size == 0 {
            // packed_size == 0 means the file is stored uncompressed.
            Ok(raw.to_vec())
        } else {
            // LZ4-compressed block.
            crate::compress::decompress_lz4(raw, entry.unpacked_size as usize)
                .with_context(|| format!("LZ4 decompress failed for {}", entry.name))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Build a minimal 24-byte BTDX/GNRL header with the given `file_count`
    /// and `name_table_offset`.  No record entries or name table follow, so
    /// any `file_count > 0` will fail the bounds check.
    fn make_ba2_header(file_count: u32, name_table_offset: u64) -> Vec<u8> {
        let mut buf = Vec::with_capacity(24);
        buf.extend_from_slice(b"BTDX");
        buf.extend_from_slice(&1u32.to_le_bytes()); // version
        buf.extend_from_slice(b"GNRL");
        buf.extend_from_slice(&file_count.to_le_bytes());
        buf.extend_from_slice(&name_table_offset.to_le_bytes());
        buf
    }

    /// A BA2 with `file_count = u32::MAX` must be rejected.
    ///
    /// On 64-bit targets `checked_mul` does not overflow (u32::MAX * 36 fits in
    /// u64), but the subsequent `records_end > data.len()` bounds check catches
    /// the out-of-range table.  On 32-bit targets the `checked_mul` itself
    /// overflows and returns the error via `ok_or_else`.  Either way the open
    /// call must return an error before attempting any large allocation.
    #[test]
    fn ba2_large_file_count_rejected() -> anyhow::Result<()> {
        let buf = make_ba2_header(u32::MAX, 24);

        let tmp_path =
            std::env::temp_dir().join(format!("fo76_ba2_overflow_test_{}.ba2", std::process::id()));
        {
            let mut f = std::fs::File::create(&tmp_path)?;
            f.write_all(&buf)?;
        }

        let result = Ba2Archive::open(&tmp_path);
        let _ = std::fs::remove_file(&tmp_path);

        assert!(
            result.is_err(),
            "expected error for overflowing file_count, got Ok"
        );
        Ok(())
    }
}
