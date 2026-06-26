//! Compression and decompression helpers for BA2 data blobs.
//!
//! FO76 GNRL archives use **raw LZ4 blocks** (not the LZ4 frame format).
//! FO4 GNRL archives use **zlib/DEFLATE**.
//! The `Codec` enum covers both, plus the uncompressed `Store` variant.

use anyhow::{Context, Result};
use flate2::{read::ZlibDecoder, write::ZlibEncoder, Compression};
use std::io::{Read, Write};

/// Compression codec for BA2 data blobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Codec {
    /// Raw LZ4 block (FO76 default).
    #[default]
    Lz4,
    /// Zlib/DEFLATE (FO4).
    Zlib,
    /// Uncompressed.
    Store,
    /// Auto-detect on read by sniffing the first two bytes.
    Auto,
}

// ── Decompression ────────────────────────────────────────────────────────────

/// Decompress raw-LZ4-block data to `expected_size` bytes.
///
/// FO76 BA2 blobs use `lz4_flex::decompress` (raw block, no size prefix).
pub fn decompress_lz4(compressed: &[u8], expected_size: usize) -> Result<Vec<u8>> {
    lz4_flex::decompress(compressed, expected_size)
        .map_err(|e| anyhow::anyhow!("LZ4 decompress: {}", e))
}

/// Decompress a zlib-wrapped buffer to approximately `expected_size` bytes.
pub fn decompress_zlib(compressed: &[u8], expected_size: usize) -> Result<Vec<u8>> {
    let mut decoder = ZlibDecoder::new(compressed);
    let mut out = Vec::with_capacity(expected_size);
    decoder
        .read_to_end(&mut out)
        .context("zlib decompression failed")?;
    Ok(out)
}

/// Sniff whether a compressed blob is zlib (vs LZ4) by checking the two-byte
/// zlib header: first byte `0x78` and `(b0 as u16) << 8 | b1 as u16) % 31 == 0`.
pub fn is_zlib(data: &[u8]) -> bool {
    if data.len() < 2 {
        return false;
    }
    let b0 = data[0] as u16;
    let b1 = data[1] as u16;
    b0 == 0x78 && (b0 << 8 | b1).is_multiple_of(31)
}

/// Decompress a blob according to `codec`.  `Auto` sniffs the first two bytes.
pub fn decompress(data: &[u8], unpacked_size: u32, codec: Codec) -> Result<Vec<u8>> {
    let expected = unpacked_size as usize;
    match codec {
        Codec::Store => Ok(data.to_vec()),
        Codec::Lz4 => decompress_lz4(data, expected),
        Codec::Zlib => decompress_zlib(data, expected),
        Codec::Auto => {
            if is_zlib(data) {
                decompress_zlib(data, expected)
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
    if codec == Codec::Store || codec == Codec::Auto {
        return Ok((data.to_vec(), 0));
    }

    let compressed = match codec {
        Codec::Lz4 => compress_lz4(data),
        Codec::Zlib => compress_zlib(data)?,
        Codec::Store | Codec::Auto => unreachable!(),
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
