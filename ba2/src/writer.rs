//! BA2 GNRL archive writer.
//!
//! `write_ba2` creates a version-1 GNRL BA2 archive from an ordered list of
//! `(archive_path, source_file)` pairs.  The layout is the same as the real
//! game archives: a 24-byte header, N×36-byte records, all data blobs
//! back-to-back, then the name table — no padding or alignment gaps anywhere.
//!
//! # Memory model
//!
//! Data blobs are written to a temporary file as they are compressed so peak
//! memory is roughly one source file + its compressed buffer at a time,
//! regardless of total archive size.

use crate::compress::{compress_entry, Codec};
use crate::format::{write_header, write_record, Record, HEADER_SIZE, RECORD_FLAGS, RECORD_SIZE};
use crate::hash::hash_path;
use anyhow::{bail, Context, Result};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

/// Options for creating a BA2 archive.
#[derive(Debug, Clone)]
pub struct WriteOptions {
    /// Compression codec for data blobs.
    ///
    /// `Lz4` (default) — raw LZ4 block, compatible with FO76.
    /// `Zlib`          — DEFLATE, compatible with FO4.
    /// `Store`         — uncompressed.
    /// `Auto`          — treated as `Store` on write.
    pub codec: Codec,
    /// Skip compression and store the file raw when the compressed size is
    /// not smaller than `raw_len * min_shrink_ratio` (default `1.0`, meaning
    /// only keep if strictly smaller).
    pub min_shrink_ratio: f32,
}

impl Default for WriteOptions {
    fn default() -> Self {
        WriteOptions {
            codec: Codec::Lz4,
            min_shrink_ratio: 1.0,
        }
    }
}

/// Per-entry metadata recorded during Pass 1.
struct EntryMeta {
    archive_path: String,
    name_hash: u32,
    dir_hash: u32,
    ext: [u8; 4],
    /// Offset into the temporary data blob file.
    blob_offset: u64,
    packed_size: u32,
    unpacked_size: u32,
}

/// Create a BA2 GNRL archive at `output` from `files`.
///
/// `files` is a slice of `(archive_path, source_path)` pairs.  `archive_path`
/// may use `/` or `\`; it will be lowercased and backslash-normalised.
/// The order of `files` determines the order of entries in the archive.
pub fn write_ba2(output: &Path, files: &[(String, PathBuf)], opts: &WriteOptions) -> Result<()> {
    let file_count = files.len();
    if file_count > u32::MAX as usize {
        bail!("too many files: {} (max {})", file_count, u32::MAX);
    }

    // ── Pass 1: compress blobs into a temp file ──────────────────────────────
    let tmp_dir = output.parent().unwrap_or_else(|| Path::new("."));
    // Keep the NamedTempFile alive until we've finished reading it back.
    // BufWriter gets a cloned file descriptor so the NamedTempFile (and its
    // auto-cleanup) is not consumed.
    let tmp_guard =
        tempfile::NamedTempFile::new_in(tmp_dir).context("failed to create temporary data file")?;
    let tmp_path = tmp_guard.path().to_path_buf();
    let mut tmp_writer = BufWriter::new(
        tmp_guard
            .as_file()
            .try_clone()
            .context("failed to clone temp file descriptor")?,
    );

    let mut metas: Vec<EntryMeta> = Vec::with_capacity(file_count);
    let mut blob_cursor: u64 = 0;

    for (archive_path, src_path) in files {
        let archive_path_norm = archive_path.to_lowercase().replace('/', "\\");

        // Read source file.
        let raw = std::fs::read(src_path)
            .with_context(|| format!("failed to read '{}'", src_path.display()))?;

        let unpacked_size = raw.len();
        if unpacked_size > u32::MAX as usize {
            bail!(
                "'{}' is too large ({} bytes; max {})",
                src_path.display(),
                unpacked_size,
                u32::MAX
            );
        }
        let unpacked_size = unpacked_size as u32;

        if archive_path_norm.len() > u16::MAX as usize {
            bail!(
                "archive path '{}' is too long ({} bytes; max {})",
                archive_path_norm,
                archive_path_norm.len(),
                u16::MAX
            );
        }

        // Compress (or store).
        let (blob, packed_size) = compress_entry(&raw, opts.codec, opts.min_shrink_ratio)
            .with_context(|| format!("compression failed for '{}'", archive_path_norm))?;

        let blob_len = blob.len() as u64;
        let blob_offset = blob_cursor;
        blob_cursor = blob_cursor
            .checked_add(blob_len)
            .ok_or_else(|| anyhow::anyhow!("archive data size overflow"))?;

        tmp_writer
            .write_all(&blob)
            .context("failed to write blob to temp file")?;

        let (name_hash, dir_hash, ext) = hash_path(&archive_path_norm);
        metas.push(EntryMeta {
            archive_path: archive_path_norm,
            name_hash,
            dir_hash,
            ext,
            blob_offset,
            packed_size,
            unpacked_size,
        });
    }
    drop(tmp_writer); // flush + close before reading back

    // ── Pass 2: write output archive ─────────────────────────────────────────
    // Arithmetic offset resolution — no seeking required.
    //
    //   header_size       = 24
    //   records_size      = 36 * N
    //   data_start        = 24 + 36*N
    //   each data_offset  = data_start + blob_offset (accumulated from temp)
    //   name_table_offset = data_start + total_blob_bytes

    let data_start = (HEADER_SIZE + RECORD_SIZE * file_count) as u64;
    let name_table_offset = data_start
        .checked_add(blob_cursor)
        .ok_or_else(|| anyhow::anyhow!("name_table_offset overflow"))?;

    let out_file =
        File::create(output).with_context(|| format!("failed to create '{}'", output.display()))?;
    let mut out = BufWriter::new(out_file);

    // Header.
    out.write_all(&write_header(1, file_count as u32, name_table_offset))
        .context("failed to write BA2 header")?;

    // Records (offsets resolved).
    for meta in &metas {
        let data_offset = data_start
            .checked_add(meta.blob_offset)
            .ok_or_else(|| anyhow::anyhow!("data_offset overflow for '{}'", meta.archive_path))?;
        let r = Record {
            name_hash: meta.name_hash,
            ext: meta.ext,
            dir_hash: meta.dir_hash,
            flags: RECORD_FLAGS,
            data_offset,
            packed_size: meta.packed_size,
            unpacked_size: meta.unpacked_size,
        };
        out.write_all(&write_record(&r))
            .context("failed to write BA2 record")?;
    }

    // Data blobs (streamed from temp file).
    {
        let mut tmp_read =
            File::open(&tmp_path).context("failed to re-open temporary data file")?;
        std::io::copy(&mut tmp_read, &mut out).context("failed to stream data blobs to output")?;
    }

    // Name table.
    for meta in &metas {
        let len = meta.archive_path.len() as u16;
        out.write_all(&len.to_le_bytes())
            .context("failed to write name table length")?;
        out.write_all(meta.archive_path.as_bytes())
            .context("failed to write name table entry")?;
    }

    out.flush().context("failed to flush output archive")?;
    Ok(())
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compress::Codec;
    use crate::reader::Ba2Archive;
    use tempfile::{NamedTempFile, TempDir};

    fn round_trip(codec: Codec) -> (Vec<u8>, Vec<u8>) {
        let content_a = b"alpha content 1234567890".repeat(4);
        let content_b = b"beta  content ABCDEFGHIJ".repeat(4);

        let src_dir = TempDir::new().unwrap();
        let file_a = src_dir.path().join("a.txt");
        let file_b = src_dir.path().join("b.bin");
        std::fs::write(&file_a, &content_a).unwrap();
        std::fs::write(&file_b, &content_b).unwrap();

        let out = NamedTempFile::new().unwrap();
        let files = vec![
            ("data/a.txt".to_string(), file_a),
            ("data/b.bin".to_string(), file_b),
        ];
        let opts = WriteOptions {
            codec,
            min_shrink_ratio: 1.0,
        };
        write_ba2(out.path(), &files, &opts).unwrap();

        let archive = Ba2Archive::open(out.path()).unwrap();
        assert_eq!(archive.list().len(), 2);

        // Verify names and hashes.
        let entry_a = &archive.list()[0];
        let entry_b = &archive.list()[1];
        assert_eq!(entry_a.name, "data\\a.txt");
        assert_eq!(entry_b.name, "data\\b.bin");

        let (nh, dh, ext) = crate::hash::hash_path("data/a.txt");
        assert_eq!(entry_a.name_hash, nh);
        assert_eq!(entry_a.dir_hash, dh);
        assert_eq!(entry_a.ext, ext);

        let out_a = archive.read("data/a.txt", Codec::Auto).unwrap();
        let out_b = archive.read("data/b.bin", Codec::Auto).unwrap();
        (out_a, out_b)
    }

    #[test]
    fn store_round_trip() {
        let content_a = b"alpha content 1234567890".repeat(4);
        let content_b = b"beta  content ABCDEFGHIJ".repeat(4);
        let (a, b) = round_trip(Codec::Store);
        assert_eq!(a, content_a.to_vec());
        assert_eq!(b, content_b.to_vec());
    }

    #[test]
    fn lz4_round_trip() {
        let content_a = b"alpha content 1234567890".repeat(4);
        let content_b = b"beta  content ABCDEFGHIJ".repeat(4);
        let (a, b) = round_trip(Codec::Lz4);
        assert_eq!(a, content_a.to_vec());
        assert_eq!(b, content_b.to_vec());
    }

    #[test]
    fn zlib_round_trip() {
        let content_a = b"alpha content 1234567890".repeat(4);
        let content_b = b"beta  content ABCDEFGHIJ".repeat(4);
        let (a, b) = round_trip(Codec::Zlib);
        assert_eq!(a, content_a.to_vec());
        assert_eq!(b, content_b.to_vec());
    }
}
