//! Binary STL: an 80-byte header, a triangle count, then 50 bytes per triangle.

use crate::Welded;

/// The binary STL of `mesh`. The header names the part and the program; it never starts
/// with `solid` in any case, which some readers take for ASCII STL.
pub(crate) fn write(mesh: &Welded, name: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(84 + 50 * mesh.triangles.len());
    let mut header = format!("Binary STL from SolidDraft, millimetres: {name}").into_bytes();
    header.resize(80, b' ');
    out.extend_from_slice(&header[..80]);
    out.extend_from_slice(&(mesh.triangles.len() as u32).to_le_bytes());
    for &t in &mesh.triangles {
        for v in std::iter::once(mesh.normal(t)).chain(t.map(|i| mesh.positions[i as usize])) {
            for c in v {
                out.extend_from_slice(&c.to_le_bytes());
            }
        }
        out.extend_from_slice(&0u16.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_triangle_is_134_bytes() {
        let mesh = Welded { positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], triangles: vec![[0, 1, 2]] };
        let bytes = write(&mesh, "a name longer than the header has room for, which is cut at eighty bytes");
        assert_eq!(bytes.len(), 84 + 50);
        assert!(bytes.starts_with(b"Binary STL"));
        assert_eq!(u32::from_le_bytes(bytes[80..84].try_into().unwrap()), 1);
        let float = |at: usize| f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        assert_eq!([float(84), float(88), float(92)], [0.0, 0.0, 1.0], "normal");
        assert_eq!([float(108), float(112), float(116)], [1.0, 0.0, 0.0], "second corner");
    }
}
