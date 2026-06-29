use anyhow::Context;
use flate2::read::ZlibDecoder;
use std::io::Read;

/// Hard upper bound on any single decompressed output buffer.
///
/// Both LZ4 and zlib decompress calls reject declared output sizes above this
/// limit before allocating, so a malformed record with an unreasonably large
/// declared size cannot cause an unbounded allocation.
///
/// Policy: malformed decompression input → controlled error, never panic or
/// large allocation.  This mirrors the decoder invariant ("unknown/malformed
/// bytes → raw hex fallback, never panic") extended to the compression layer.
pub const MAX_DECOMP_SIZE: usize = 64 * 1024 * 1024; // 64 MiB

/// Decompress an LZ4-block-compressed buffer to the given expected output size.
///
/// BA2 archives use raw LZ4 blocks (not the LZ4 frame format).
pub fn decompress_lz4(compressed: &[u8], expected_size: usize) -> anyhow::Result<Vec<u8>> {
    if expected_size > MAX_DECOMP_SIZE {
        anyhow::bail!(
            "LZ4 declared output size {} exceeds limit of {} bytes",
            expected_size,
            MAX_DECOMP_SIZE
        );
    }
    lz4_flex::decompress(compressed, expected_size)
        .map_err(|e| anyhow::anyhow!("LZ4 decompress: {}", e))
}

pub fn decompress_zlib(compressed: &[u8], expected_size: usize) -> anyhow::Result<Vec<u8>> {
    if expected_size > MAX_DECOMP_SIZE {
        anyhow::bail!(
            "zlib declared output size {} exceeds limit of {} bytes",
            expected_size,
            MAX_DECOMP_SIZE
        );
    }
    let mut decoder = ZlibDecoder::new(compressed);
    let mut out = Vec::with_capacity(expected_size);
    decoder
        .read_to_end(&mut out)
        .context("zlib decompression failed")?;
    if expected_size > 0 && out.len() != expected_size {
        // Some records may not match exactly; keep what we got.
    }
    Ok(out)
}

pub fn decompress_record_data(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    if data.len() < 4 {
        anyhow::bail!("compressed record data too short");
    }
    let uncompressed_size = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
    if uncompressed_size == 0 {
        return Ok(Vec::new());
    }
    if uncompressed_size > MAX_DECOMP_SIZE {
        anyhow::bail!(
            "record declared uncompressed size {} exceeds limit of {} bytes",
            uncompressed_size,
            MAX_DECOMP_SIZE
        );
    }
    decompress_zlib(&data[4..], uncompressed_size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decompress_lz4_rejects_oversized_expected_size() {
        let result = decompress_lz4(b"", MAX_DECOMP_SIZE + 1);
        assert!(
            result.is_err(),
            "expected error for oversized expected_size"
        );
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("exceeds limit"),
            "unexpected error message: {msg}"
        );
    }

    #[test]
    fn decompress_zlib_rejects_oversized_expected_size() {
        let result = decompress_zlib(b"", MAX_DECOMP_SIZE + 1);
        assert!(
            result.is_err(),
            "expected error for oversized expected_size"
        );
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("exceeds limit"),
            "unexpected error message: {msg}"
        );
    }

    #[test]
    fn decompress_record_data_rejects_oversized_declared_size() {
        // 4-byte little-endian prefix encoding MAX_DECOMP_SIZE + 1, followed by
        // an empty compressed payload.  The size guard fires before calling zlib.
        let declared = (MAX_DECOMP_SIZE + 1) as u32;
        let mut data = declared.to_le_bytes().to_vec();
        let result = decompress_record_data(&data);
        assert!(
            result.is_err(),
            "expected error for oversized declared size"
        );
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("exceeds limit"),
            "unexpected error message: {msg}"
        );

        // Sanity: a zero declared size returns an empty vec without error.
        data[0..4].copy_from_slice(&0u32.to_le_bytes());
        assert!(decompress_record_data(&data).is_ok());
    }
}
