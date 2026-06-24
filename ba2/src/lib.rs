//! `ba2` — extract and create Bethesda BA2 GNRL archives.
//!
//! Supports Fallout 76 (LZ4-compressed or stored GNRL) and Fallout 4
//! (zlib-compressed GNRL).  DX10 texture archives are detected and rejected.

pub mod compress;
pub mod extract;
pub mod format;
pub mod hash;
pub mod reader;
pub mod writer;

// Convenience re-exports for library consumers.
pub use compress::Codec;
pub use extract::{extract_all, extract_one, ExtractOptions};
pub use reader::{Ba2Archive, Ba2Entry};
pub use writer::{write_ba2, WriteOptions};
