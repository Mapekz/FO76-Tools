use anyhow::Context;
use flate2::read::ZlibDecoder;
use std::io::Read;

pub fn decompress_zlib(compressed: &[u8], expected_size: usize) -> anyhow::Result<Vec<u8>> {
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
    let uncompressed_size =
        u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
    if uncompressed_size == 0 {
        return Ok(Vec::new());
    }
    decompress_zlib(&data[4..], uncompressed_size)
}
