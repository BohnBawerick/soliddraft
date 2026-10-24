//! Wavefront OBJ from the display mesh, retaining face groups and sharp-face normals.

use std::io::Write;

use soliddraft_kernel_api::Mesh;

pub(super) fn write(mesh: &Mesh, name: &str, chord_height: f64) -> Vec<u8> {
    let mut out = Vec::new();
    writeln!(out, "# SolidDraft display mesh; millimetres").unwrap();
    writeln!(out, "# chord_height_mm {chord_height}").unwrap();
    write!(out, "o ").unwrap();
    // An object name is one line, never another OBJ record.
    for c in name.chars() {
        write!(out, "{}", if c.is_control() { ' ' } else { c }).unwrap();
    }
    writeln!(out).unwrap();
    for p in &mesh.positions {
        writeln!(out, "v {} {} {}", p[0], p[1], p[2]).unwrap();
    }
    for n in &mesh.normals {
        writeln!(out, "vn {} {} {}", n[0], n[1], n[2]).unwrap();
    }
    let mut previous = None;
    for (t, face) in mesh.indices.as_chunks::<3>().0.iter().zip(&mesh.triangle_faces) {
        let [a, b, c] = t.map(|i| mesh.positions[i as usize]);
        // Match the STL/3MF welding path's removal of collapsed triangles.
        if a == b || b == c || c == a {
            continue;
        }
        if previous != Some(*face) {
            writeln!(out, "g face_{}", face.raw()).unwrap();
            previous = Some(*face);
        }
        let [a, b, c] = t.map(|i| u64::from(i) + 1);
        writeln!(out, "f {a}//{a} {b}//{b} {c}//{c}").unwrap();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use soliddraft_kernel_api::FaceId;

    #[test]
    fn open_mesh_keeps_faces_normals_and_a_single_object_name() {
        let mesh = Mesh {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            indices: vec![0, 1, 2, 0, 0, 2],
            triangle_faces: vec![FaceId::from_raw(7), FaceId::from_raw(9)],
            ..Mesh::default()
        };
        let text = String::from_utf8(write(&mesh, "plate\ng fake\r\n# tail", 0.005)).unwrap();
        assert!(text.lines().any(|l| l == "o plate g fake  # tail"));
        assert_eq!(text.lines().filter(|l| l.starts_with("g ")).collect::<Vec<_>>(), ["g face_7"]);
        assert_eq!(text.lines().filter(|l| l.starts_with("f ")).collect::<Vec<_>>(), ["f 1//1 2//2 3//3"]);
        assert_eq!(text.lines().filter(|l| *l == "vn 0 0 1").count(), 3);
    }
}
