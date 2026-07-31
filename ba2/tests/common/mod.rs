//! Shared test helpers for ba2 integration tests.
//!
//! This module is not compiled as its own test binary (it lives under a
//! subdirectory, so Cargo ignores it as a test target).  Individual test files
//! pull it in with `mod common;`.

use ba2::format::{
    ArchiveKind, RECORD_FLAGS, Record, TexChunk, TexRecord, write_header, write_record,
    write_tex_chunk, write_tex_record,
};
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

    let header_bytes = write_header(1, ArchiveKind::Gnrl, file_count, name_table_offset);

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

/// One texture entry for [`make_test_texture_archive`]: an archive path and
/// its already-chunked, stored (uncompressed) mip data.
pub struct TestTexture<'a> {
    pub path: &'a str,
    pub dxgi_format: u8,
    pub width: u16,
    pub height: u16,
    pub mip_count: u8,
    pub cubemap: bool,
    /// One entry per chunk: `(mip_first, mip_last, bytes)`.
    pub chunks: &'a [(u16, u16, &'a [u8])],
}

/// Build a minimal stored (uncompressed) DX10 BA2 archive in a temp file.
///
/// Unlike GNRL, chunk layout is caller-supplied rather than derived from a
/// policy — tests decide their own chunk boundaries to isolate what they're
/// checking.
pub fn make_test_texture_archive(textures: &[TestTexture]) -> NamedTempFile {
    let file_count = textures.len() as u32;
    let records_size: usize = textures.iter().map(|t| 24 + 24 * t.chunks.len()).sum();
    let data_start = 24u64 + records_size as u64;

    let mut cursor = data_start;
    let mut chunk_offsets: Vec<Vec<u64>> = Vec::with_capacity(textures.len());
    for t in textures {
        let mut offsets = Vec::with_capacity(t.chunks.len());
        for (_, _, bytes) in t.chunks {
            offsets.push(cursor);
            cursor += bytes.len() as u64;
        }
        chunk_offsets.push(offsets);
    }
    let name_table_offset = cursor;

    let header_bytes = write_header(1, ArchiveKind::Dx10, file_count, name_table_offset);

    let mut records_bytes: Vec<u8> = Vec::new();
    for (t, offsets) in textures.iter().zip(&chunk_offsets) {
        let (name_hash, dir_hash, ext) = hash_path(t.path);
        let tex = TexRecord {
            name_hash,
            ext,
            dir_hash,
            chunk_count: t.chunks.len() as u8,
            height: t.height,
            width: t.width,
            mip_count: t.mip_count,
            dxgi_format: t.dxgi_format,
            cubemap: t.cubemap,
            tile_mode: 0x08,
        };
        records_bytes.extend_from_slice(&write_tex_record(&tex));
        for ((mip_first, mip_last, bytes), &offset) in t.chunks.iter().zip(offsets) {
            let c = TexChunk {
                data_offset: offset,
                packed_size: 0,
                unpacked_size: bytes.len() as u32,
                mip_first: *mip_first,
                mip_last: *mip_last,
            };
            records_bytes.extend_from_slice(&write_tex_chunk(&c));
        }
    }

    let mut name_table: Vec<u8> = Vec::new();
    for t in textures {
        let p = t.path.to_lowercase().replace('/', "\\");
        let len = p.len() as u16;
        name_table.extend_from_slice(&len.to_le_bytes());
        name_table.extend_from_slice(p.as_bytes());
    }

    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all(&header_bytes).unwrap();
    tmp.write_all(&records_bytes).unwrap();
    for t in textures {
        for (_, _, bytes) in t.chunks {
            tmp.write_all(bytes).unwrap();
        }
    }
    tmp.write_all(&name_table).unwrap();
    tmp.flush().unwrap();
    tmp
}
