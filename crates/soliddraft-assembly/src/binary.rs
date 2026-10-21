//! The little-endian readers and writers of the binary entries (`placements.bin`, display
//! copies), shared with part files' display data in `soliddraft-tessellation`.

pub use soliddraft_tessellation::binary::BinaryError;
pub(crate) use soliddraft_tessellation::binary::{Reader, Writer};
