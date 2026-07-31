//! `ba2` — extract and create Bethesda BA2 GNRL and DX10 (texture) archives.
//!
//! Supports Fallout 76 (LZ4-compressed or stored GNRL, or DX10 texture
//! archives) and Fallout 4 (zlib-compressed GNRL). DX10 entries are not DDS
//! files on disk — `Ba2Archive::read` synthesizes a complete `.dds` file from
//! each entry's texture header and mip chunks; see [`dds`].

pub mod compress;
pub mod dds;
pub mod extract;
pub mod format;
pub mod hash;
pub mod reader;
pub mod writer;

// Convenience re-exports for library consumers.
pub use compress::Codec;
pub use extract::{ExtractOptions, extract_all, extract_one};
pub use format::ArchiveKind;
pub use reader::{Ba2Archive, Ba2Entry, EntryData, TextureInfo};
pub use writer::{WriteOptions, write_ba2};
