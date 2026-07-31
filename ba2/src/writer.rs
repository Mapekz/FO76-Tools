//! BA2 archive writer — GNRL and DX10 (texture) archives.
//!
//! `write_ba2` creates a version-1 BA2 archive from an ordered list of
//! `(archive_path, source_file)` pairs. The layout is the same as the real
//! game archives: a 24-byte header, N per-file records (36-byte GNRL, or a
//! variable-stride DX10 texture header + chunk records), all data blobs
//! back-to-back, then the name table — no padding or alignment gaps anywhere.
//!
//! For DX10, each source file must be a `.dds`; its header is parsed
//! ([`crate::dds::parse_header`]) to recover `(dxgi_format, width, height,
//! mip_count, cubemap)`, and its mip data is split into chunks per
//! [`dx10_chunk_count`] — Archive2's own mip-chunking policy, ground-truthed
//! against every DX10 archive shipped with FO76.
//!
//! # Memory model
//!
//! Data blobs are written to a temporary file as they are compressed so peak
//! memory is roughly one source file + its compressed buffer at a time,
//! regardless of total archive size.

use crate::compress::{Codec, compress_entry};
use crate::dds;
use crate::format::{
    ArchiveKind, HEADER_SIZE, RECORD_FLAGS, RECORD_SIZE, Record, TEX_CHUNK_SIZE, TEX_RECORD_SIZE,
    TexChunk, TexRecord, write_header, write_record, write_tex_chunk, write_tex_record,
};
use crate::hash::hash_path;
use anyhow::{Context, Result, bail};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

/// Options for creating a BA2 archive.
#[derive(Debug, Clone)]
pub struct WriteOptions {
    /// Archive kind to write: `Gnrl` (default) or `Dx10`.
    pub kind: ArchiveKind,
    /// Compression codec for data blobs.
    ///
    /// `Lz4` (default) — raw LZ4 block, compatible with FO76.
    /// `Zlib`          — DEFLATE, compatible with FO4 and DX10 texture chunks.
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
            kind: ArchiveKind::Gnrl,
            codec: Codec::Lz4,
            min_shrink_ratio: 1.0,
        }
    }
}

/// Create a BA2 archive at `output` from `files`.
///
/// `files` is a slice of `(archive_path, source_path)` pairs. `archive_path`
/// may use `/` or `\`; it will be lowercased and backslash-normalised. The
/// order of `files` determines the order of entries in the archive.
pub fn write_ba2(output: &Path, files: &[(String, PathBuf)], opts: &WriteOptions) -> Result<()> {
    let file_count = files.len();
    if file_count > u32::MAX as usize {
        bail!("too many files: {} (max {})", file_count, u32::MAX);
    }

    match opts.kind {
        ArchiveKind::Gnrl => write_gnrl(output, files, opts),
        ArchiveKind::Dx10 => write_dx10(output, files, opts),
    }
}

// ── GNRL ─────────────────────────────────────────────────────────────────────

/// Per-entry metadata recorded during GNRL Pass 1.
struct GnrlEntryMeta {
    archive_path: String,
    name_hash: u32,
    dir_hash: u32,
    ext: [u8; 4],
    /// Offset into the temporary data blob file.
    blob_offset: u64,
    packed_size: u32,
    unpacked_size: u32,
}

fn write_gnrl(output: &Path, files: &[(String, PathBuf)], opts: &WriteOptions) -> Result<()> {
    let file_count = files.len();

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

    let mut metas: Vec<GnrlEntryMeta> = Vec::with_capacity(file_count);
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
        metas.push(GnrlEntryMeta {
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
    out.write_all(&write_header(
        1,
        ArchiveKind::Gnrl,
        file_count as u32,
        name_table_offset,
    ))
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
        write_name(&mut out, &meta.archive_path)?;
    }

    out.flush().context("failed to flush output archive")?;
    Ok(())
}

// ── DX10 ─────────────────────────────────────────────────────────────────────

/// Archive2's mip-chunking policy: start at one chunk, add a chunk per mip
/// level while there is more than one mip left, the chunk cap (4) has not
/// been hit, and the *current* mip's pixel area is still >= 512x512.
/// Cubemaps are never chunked.
///
/// Ground-truthed against every DX10 archive shipped with FO76 (250,722
/// texture entries, 0 mismatches) — note this is an area rule, not xEdit's
/// `width >= 512 && height >= 512` (which diverges from Archive2 on 320 of
/// those entries, all non-square).
fn dx10_chunk_count(mut width: u32, mut height: u32, mip_count: u8, cubemap: bool) -> u8 {
    if cubemap {
        return 1;
    }
    let mip_count = if mip_count == 0 {
        1u32
    } else {
        mip_count as u32
    };
    let mut count = 1u32;
    while count < mip_count && count < 4 && width * height >= 512 * 512 {
        count += 1;
        width /= 2;
        height /= 2;
    }
    count as u8
}

/// Per-entry metadata recorded during DX10 Pass 1.
struct Dx10EntryMeta {
    archive_path: String,
    tex: TexRecord,
    /// One per chunk: `(blob_offset, packed_size, unpacked_size, mip_first, mip_last)`.
    chunks: Vec<(u64, u32, u32, u16, u16)>,
}

fn write_dx10(output: &Path, files: &[(String, PathBuf)], opts: &WriteOptions) -> Result<()> {
    let file_count = files.len();

    let tmp_dir = output.parent().unwrap_or_else(|| Path::new("."));
    let tmp_guard =
        tempfile::NamedTempFile::new_in(tmp_dir).context("failed to create temporary data file")?;
    let tmp_path = tmp_guard.path().to_path_buf();
    let mut tmp_writer = BufWriter::new(
        tmp_guard
            .as_file()
            .try_clone()
            .context("failed to clone temp file descriptor")?,
    );

    let mut metas: Vec<Dx10EntryMeta> = Vec::with_capacity(file_count);
    let mut blob_cursor: u64 = 0;
    let mut total_chunks: usize = 0;

    for (archive_path, src_path) in files {
        let archive_path_norm = archive_path.to_lowercase().replace('/', "\\");
        if archive_path_norm.len() > u16::MAX as usize {
            bail!(
                "archive path '{}' is too long ({} bytes; max {})",
                archive_path_norm,
                archive_path_norm.len(),
                u16::MAX
            );
        }

        let raw = std::fs::read(src_path)
            .with_context(|| format!("failed to read '{}'", src_path.display()))?;
        let meta = dds::parse_header(&raw)
            .with_context(|| format!("'{}' is not a texture ba2 can pack", src_path.display()))?;
        let mip_data = &raw[meta.header_len..];

        let chunk_count = dx10_chunk_count(
            meta.width as u32,
            meta.height as u32,
            meta.mip_count,
            meta.cubemap,
        );
        let mip0_size = dds::mip0_size(meta.dxgi_format, meta.width as u32, meta.height as u32)
            .with_context(|| format!("'{}'", src_path.display()))?;

        let mut chunks = Vec::with_capacity(chunk_count as usize);
        let mut mip_offset = 0usize;
        for i in 0..chunk_count {
            let is_last = i == chunk_count - 1;
            let chunk_len = if is_last {
                mip_data.len().checked_sub(mip_offset).ok_or_else(|| {
                    anyhow::anyhow!(
                        "'{}': computed mip chunk layout overruns the file",
                        src_path.display()
                    )
                })?
            } else {
                let len = (mip0_size >> (2 * i as u32)) as usize;
                if mip_offset + len > mip_data.len() {
                    bail!(
                        "'{}': computed mip chunk {} overruns the file",
                        src_path.display(),
                        i
                    );
                }
                len
            };

            let chunk_bytes = &mip_data[mip_offset..mip_offset + chunk_len];
            let (blob, packed_size) =
                compress_entry(chunk_bytes, opts.codec, opts.min_shrink_ratio).with_context(
                    || format!("compression failed for '{}' chunk {}", archive_path_norm, i),
                )?;

            let blob_len = blob.len() as u64;
            let blob_offset = blob_cursor;
            blob_cursor = blob_cursor
                .checked_add(blob_len)
                .ok_or_else(|| anyhow::anyhow!("archive data size overflow"))?;
            tmp_writer
                .write_all(&blob)
                .context("failed to write blob to temp file")?;

            let mip_first = i as u16;
            let mip_last = if is_last {
                meta.mip_count.saturating_sub(1) as u16
            } else {
                i as u16
            };
            chunks.push((
                blob_offset,
                packed_size,
                chunk_len as u32,
                mip_first,
                mip_last,
            ));
            mip_offset += chunk_len;
        }

        let (name_hash, dir_hash, ext) = hash_path(&archive_path_norm);
        total_chunks += chunks.len();
        metas.push(Dx10EntryMeta {
            archive_path: archive_path_norm,
            tex: TexRecord {
                name_hash,
                ext,
                dir_hash,
                chunk_count,
                height: meta.height,
                width: meta.width,
                mip_count: meta.mip_count,
                dxgi_format: meta.dxgi_format,
                cubemap: meta.cubemap,
                tile_mode: crate::format::TEX_TILE_MODE_LINEAR,
            },
            chunks,
        });
    }
    drop(tmp_writer);

    let data_start =
        (HEADER_SIZE + TEX_RECORD_SIZE * file_count + TEX_CHUNK_SIZE * total_chunks) as u64;
    let name_table_offset = data_start
        .checked_add(blob_cursor)
        .ok_or_else(|| anyhow::anyhow!("name_table_offset overflow"))?;

    let out_file =
        File::create(output).with_context(|| format!("failed to create '{}'", output.display()))?;
    let mut out = BufWriter::new(out_file);

    out.write_all(&write_header(
        1,
        ArchiveKind::Dx10,
        file_count as u32,
        name_table_offset,
    ))
    .context("failed to write BA2 header")?;

    for meta in &metas {
        out.write_all(&write_tex_record(&meta.tex))
            .context("failed to write DX10 texture record")?;
        for &(blob_offset, packed_size, unpacked_size, mip_first, mip_last) in &meta.chunks {
            let data_offset = data_start.checked_add(blob_offset).ok_or_else(|| {
                anyhow::anyhow!("data_offset overflow for '{}'", meta.archive_path)
            })?;
            let c = TexChunk {
                data_offset,
                packed_size,
                unpacked_size,
                mip_first,
                mip_last,
            };
            out.write_all(&write_tex_chunk(&c))
                .context("failed to write DX10 chunk record")?;
        }
    }

    {
        let mut tmp_read =
            File::open(&tmp_path).context("failed to re-open temporary data file")?;
        std::io::copy(&mut tmp_read, &mut out).context("failed to stream data blobs to output")?;
    }

    for meta in &metas {
        write_name(&mut out, &meta.archive_path)?;
    }

    out.flush().context("failed to flush output archive")?;
    Ok(())
}

fn write_name(out: &mut impl Write, archive_path: &str) -> Result<()> {
    let len = archive_path.len() as u16;
    out.write_all(&len.to_le_bytes())
        .context("failed to write name table length")?;
    out.write_all(archive_path.as_bytes())
        .context("failed to write name table entry")?;
    Ok(())
}

// ── Tests ────────────────────────────────────────────────────────────────────
//
// `dx10_chunk_count` is module-private, so its tests live here per house
// style; public-API round-trip tests live in `tests/writer.rs`.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_count_cubemap_always_one() {
        assert_eq!(dx10_chunk_count(2048, 2048, 12, true), 1);
    }

    #[test]
    fn chunk_count_small_texture_never_chunks() {
        // Below the 512x512 area threshold from mip 0: always 1 chunk.
        assert_eq!(dx10_chunk_count(256, 256, 9, false), 1);
    }

    #[test]
    fn chunk_count_caps_at_four() {
        // 4096x1024, 13 mips: area rule keeps adding chunks until area < 512*512
        // or the 4-chunk cap is hit — verified against a real archive entry.
        assert_eq!(dx10_chunk_count(4096, 1024, 13, false), 4);
    }

    #[test]
    fn chunk_count_square_1024_11_mips() {
        assert_eq!(dx10_chunk_count(1024, 1024, 11, false), 3);
    }

    #[test]
    fn chunk_count_square_512_10_mips() {
        assert_eq!(dx10_chunk_count(512, 512, 10, false), 2);
    }

    #[test]
    fn chunk_count_nonsquare_diverges_from_xedit_wh_rule() {
        // xEdit's `w >= 512 && h >= 512` rule would stop at 1 chunk here
        // (height 256 < 512), but the real archive entry for this exact
        // shape has 2 chunks — the area rule (width*height >= 512*512)
        // matches it.
        assert_eq!(dx10_chunk_count(2048, 256, 12, false), 2);
    }

    #[test]
    fn chunk_count_mip_count_limits_below_area_cap() {
        // Only 3 mips available, so chunking stops at 3 even though the area
        // rule alone would keep going.
        assert_eq!(dx10_chunk_count(4096, 4096, 3, false), 3);
    }
}
