//! Little-endian readers and writers for binary cache entries (display meshes in part
//! files and assembly copies, an assembly's `placements.bin`). Every binary entry states
//! its types, sizes and byte order where it is defined, never Rust memory layout.

/// Appends little-endian values to a byte vector.
#[derive(Default)]
pub struct Writer {
    /// What has been written.
    pub bytes: Vec<u8>,
}

impl Writer {
    /// An empty writer with room for `n` bytes.
    pub fn with_capacity(n: usize) -> Writer {
        Writer { bytes: Vec::with_capacity(n) }
    }

    /// Appends bytes as they are.
    pub fn raw(&mut self, b: &[u8]) {
        self.bytes.extend_from_slice(b);
    }

    /// Appends a `u32`.
    pub fn u32(&mut self, v: u32) {
        self.raw(&v.to_le_bytes());
    }

    /// Appends an `f32`.
    pub fn f32(&mut self, v: f32) {
        self.raw(&v.to_le_bytes());
    }

    /// Appends an `f64`.
    pub fn f64(&mut self, v: f64) {
        self.raw(&v.to_le_bytes());
    }

    /// Appends a string as its `u32` byte length and its UTF-8 bytes.
    pub fn str(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.raw(s.as_bytes());
    }
}

/// Reads little-endian values, failing cleanly at the end of the data.
pub struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

/// A binary entry that is cut short or malformed.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("{entry} is not valid: {what}")]
pub struct BinaryError {
    /// The entry.
    pub entry: &'static str,
    /// What is wrong.
    pub what: String,
}

impl<'a> Reader<'a> {
    /// A reader at the start of `bytes`.
    pub fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader { bytes, at: 0 }
    }

    /// The next `n` bytes, or `None` past the end.
    pub fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(n)?;
        let s = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(s)
    }

    /// The next `u32`.
    pub fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|b| u32::from_le_bytes(b.try_into().expect("four bytes")))
    }

    /// The next `f64`.
    pub fn f64(&mut self) -> Option<f64> {
        self.take(8).map(|b| f64::from_le_bytes(b.try_into().expect("eight bytes")))
    }

    /// The next length-prefixed UTF-8 string.
    pub fn str(&mut self) -> Option<&'a str> {
        let n = self.u32()? as usize;
        std::str::from_utf8(self.take(n)?).ok()
    }

    /// The bytes not read yet.
    pub fn remaining(&self) -> usize {
        self.bytes.len() - self.at
    }
}
