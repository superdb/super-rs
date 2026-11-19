//! Reading and writing BSUP files.
//!
//! A BSUP file is a sequence of super frames, each a sequence of frames
//! followed by a super footer.  A frame holds values either in rows (row
//! frames) or as vectors (column frames).  The [`Reader`] reads both kinds
//! and the [`Writer`] writes row frames.  See the [BSUP specification].
//!
//! [BSUP specification]: https://superdb.org/docs/formats/bsup

mod column;
mod intcomp;
mod lz4;
mod reader;
mod writer;

pub use reader::{Item, Reader};
pub use writer::{Writer, WriterOptions};

pub(crate) const ROW_MAGIC: &[u8; 4] = b"SUPR";
pub(crate) const CONTROL_MAGIC: &[u8; 4] = b"SUPX";
pub(crate) const COLUMN_MAGIC: &[u8; 4] = b"SUPC";
pub(crate) const SUPER_MAGIC: &[u8; 4] = b"SUPS";

pub(crate) const ROW_HEADER_SIZE: usize = 26;
pub(crate) const COLUMN_HEADER_SIZE: usize = 42;
pub(crate) const SUPER_FOOTER_SIZE: usize = 38;
pub(crate) const SUPER_FOOTER_TRAILER: usize = 8;

/// The major version of BSUP implemented by this crate.
pub const VERSION_MAJOR: u8 = 0;
/// The minor version of BSUP implemented by this crate.
pub const VERSION_MINOR: u16 = 31;

pub(crate) const MAX_FRAME_SIZE: u64 = 2 * 1024 * 1024 * 1024;
pub(crate) const MAX_META_SIZE: u64 = 100 * 1024 * 1024;

pub(crate) fn version_bytes() -> [u8; 2] {
    ((u16::from(VERSION_MAJOR) << 10) | VERSION_MINOR).to_le_bytes()
}

pub(crate) fn check_version(b: [u8; 2]) -> crate::Result<()> {
    let v = u16::from_le_bytes(b);
    let (major, minor) = ((v >> 10) as u8, v & 0x3ff);
    if major != VERSION_MAJOR || (VERSION_MAJOR == 0 && minor != VERSION_MINOR) {
        return Err(crate::Error::Unsupported(format!(
            "BSUP version {major}.{minor} (this crate reads version {VERSION_MAJOR}.{VERSION_MINOR})"
        )));
    }
    Ok(())
}

pub(crate) fn le64(b: &[u8]) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[..8]);
    u64::from_le_bytes(a)
}

pub(crate) fn le32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}
