//! Compression and decompression helpers for BA2 data blobs.
//!
//! FO76 GNRL archives use **raw LZ4 blocks** (not the LZ4 frame format).
//! FO4 GNRL archives use **zlib/DEFLATE**.
//! The `Codec` enum covers both, plus the uncompressed `Store` variant.

use anyhow::{Context, Result};
use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};
use std::io::{Read, Write};

/// Hard upper bound on any single decompressed output buffer.
///
/// Both LZ4 and zlib decompress calls reject declared output sizes above this
/// limit before allocating, so a malformed or malicious archive with an
/// unreasonably large declared `unpacked_size` cannot cause an unbounded
/// allocation (a classic decompression-bomb vector).
pub const MAX_DECOMP_SIZE: usize = 64 * 1024 * 1024; // 64 MiB

/// The codec blobs are written with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Codec {
    /// Raw LZ4 block (FO76 GNRL).
    #[default]
    Lz4,
    /// Zlib/DEFLATE (FO76 DX10 textures, FO4).
    Zlib,
    /// Uncompressed.
    Store,
}

/// How a compressed blob is decompressed on read. (An uncompressed blob is
/// recognised by its zero packed size and needs no codec.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReadCodec {
    /// Sniff zlib vs LZ4 from each blob's first two bytes.
    #[default]
    Auto,
    Lz4,
    Zlib,
}

// ── Decompression ────────────────────────────────────────────────────────────

/// Decompress raw-LZ4-block data that must expand to exactly `expected_size`
/// bytes.
///
/// FO76 BA2 blobs use `lz4_flex::decompress` (raw block, no size prefix).
pub fn decompress_lz4(compressed: &[u8], expected_size: usize) -> Result<Vec<u8>> {
    check_declared_size("LZ4", expected_size)?;
    let out = lz4_flex::decompress(compressed, expected_size)
        .map_err(|e| anyhow::anyhow!("LZ4 decompress: {}", e))?;
    check_exact_size("LZ4", out, expected_size)
}

/// Decompress a zlib stream that must inflate to exactly `expected_size`
/// bytes. Reading stops one byte past `expected_size`, so a stream that
/// inflates further is rejected without being inflated in full.
pub fn decompress_zlib(compressed: &[u8], expected_size: usize) -> Result<Vec<u8>> {
    check_declared_size("zlib", expected_size)?;
    let mut out = Vec::with_capacity(expected_size);
    ZlibDecoder::new(compressed)
        .take(expected_size as u64 + 1)
        .read_to_end(&mut out)
        .context("zlib decompression failed")?;
    check_exact_size("zlib", out, expected_size)
}

fn check_declared_size(codec: &str, expected_size: usize) -> Result<()> {
    if expected_size > MAX_DECOMP_SIZE {
        anyhow::bail!(
            "{codec} declared output size {expected_size} exceeds limit of {MAX_DECOMP_SIZE} bytes"
        );
    }
    Ok(())
}

fn check_exact_size(codec: &str, out: Vec<u8>, expected_size: usize) -> Result<Vec<u8>> {
    if out.len() != expected_size {
        anyhow::bail!(
            "{codec} output is {} bytes, expected {expected_size}",
            out.len()
        );
    }
    Ok(out)
}

/// Sniff whether a compressed blob is zlib (vs LZ4) by checking the two-byte
/// zlib header (RFC 1950): compression method 8 (deflate), a window of at
/// most 32 KiB (CINFO ≤ 7), and `(b0 << 8 | b1) % 31 == 0`. The window is not
/// always 32 KiB: FO76's Materials archive uses `0x48` (4 KiB).
pub fn is_zlib(data: &[u8]) -> bool {
    let [b0, b1, ..] = *data else {
        return false;
    };
    b0 & 0x0F == 8 && b0 >> 4 <= 7 && ((b0 as u16) << 8 | b1 as u16).is_multiple_of(31)
}

/// Decompress a compressed blob according to `codec`.
pub fn decompress(data: &[u8], unpacked_size: u32, codec: ReadCodec) -> Result<Vec<u8>> {
    let expected = unpacked_size as usize;
    match codec {
        ReadCodec::Lz4 => decompress_lz4(data, expected),
        ReadCodec::Zlib => decompress_zlib(data, expected),
        // A raw LZ4 block can start with bytes that pass the zlib sniff, so a
        // sniffed zlib blob that fails to inflate is retried as LZ4.
        ReadCodec::Auto => {
            if is_zlib(data) {
                decompress_zlib(data, expected)
                    .or_else(|zlib_err| decompress_lz4(data, expected).map_err(|_| zlib_err))
            } else {
                decompress_lz4(data, expected)
            }
        }
    }
}

// ── Compression ──────────────────────────────────────────────────────────────

/// Compress `data` with raw LZ4 block encoding.
pub fn compress_lz4(data: &[u8]) -> Vec<u8> {
    lz4_flex::compress(data)
}

/// Compress `data` with zlib at default compression level.
pub fn compress_zlib(data: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data).context("zlib compression failed")?;
    encoder.finish().context("zlib compression finish failed")
}

/// Compress `data` according to `codec`.
///
/// Returns `(blob, packed_size)`:
/// - `packed_size == 0` when the data is stored uncompressed (either because
///   `Store` was requested, or because compression did not shrink the data
///   below `min_shrink_ratio * raw_len`).
/// - Otherwise `packed_size` is the compressed length and `blob` is the
///   compressed bytes.
pub fn compress_entry(data: &[u8], codec: Codec, min_shrink_ratio: f32) -> Result<(Vec<u8>, u32)> {
    let compressed = match codec {
        Codec::Store => return Ok((data.to_vec(), 0)),
        Codec::Lz4 => compress_lz4(data),
        Codec::Zlib => compress_zlib(data)?,
    };

    let threshold = (data.len() as f32 * min_shrink_ratio) as usize;
    if compressed.len() < threshold {
        let packed_size = compressed.len() as u32;
        Ok((compressed, packed_size))
    } else {
        // Compression didn't help enough — store uncompressed.
        Ok((data.to_vec(), 0))
    }
}
