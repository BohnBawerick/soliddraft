//! 3MF core: an Open Packaging Conventions zip holding one model part with one closed
//! mesh, in millimetres.

use std::fmt::Write as _;
use std::io::{Cursor, Write as _};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipWriter};

use crate::{ExportError, Welded};

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
 <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
 <Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/>
</Types>
"#;

const RELATIONSHIPS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
 <Relationship Target="/3D/3dmodel.model" Id="rel0" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
</Relationships>
"#;

/// The package's path of the model part.
pub(crate) const MODEL: &str = "3D/3dmodel.model";

/// The 3MF package of `mesh`, which must be closed. Entries carry a fixed timestamp, so the
/// same mesh always gives the same bytes.
pub(crate) fn write(mesh: &Welded, name: &str) -> Result<Vec<u8>, ExportError> {
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .last_modified_time(DateTime::default());
    let package = || -> zip::result::ZipResult<Vec<u8>> {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("[Content_Types].xml", options)?;
        zip.write_all(CONTENT_TYPES.as_bytes())?;
        zip.start_file("_rels/.rels", options)?;
        zip.write_all(RELATIONSHIPS.as_bytes())?;
        zip.start_file(MODEL, options)?;
        zip.write_all(model(mesh, name).as_bytes())?;
        Ok(zip.finish()?.into_inner())
    };
    package().map_err(|e| ExportError::Package(e.to_string()))
}

/// The model part's XML. Coordinates print as the shortest decimals that read back to the
/// same 32-bit values.
fn model(mesh: &Welded, name: &str) -> String {
    let name = escape(name);
    let mut s = String::with_capacity(64 * (mesh.positions.len() + mesh.triangles.len()) + 512);
    s += "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n";
    s += "<model unit=\"millimeter\" xml:lang=\"en-US\" xmlns=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\">\n";
    let _ = writeln!(s, " <metadata name=\"Title\">{name}</metadata>");
    s += " <metadata name=\"Application\">SolidDraft</metadata>\n";
    let _ = writeln!(s, " <resources>\n  <object id=\"1\" type=\"model\" name=\"{name}\">\n   <mesh>\n    <vertices>");
    for p in &mesh.positions {
        let _ = writeln!(s, "     <vertex x=\"{}\" y=\"{}\" z=\"{}\"/>", p[0], p[1], p[2]);
    }
    s += "    </vertices>\n    <triangles>\n";
    for t in &mesh.triangles {
        let _ = writeln!(s, "     <triangle v1=\"{}\" v2=\"{}\" v3=\"{}\"/>", t[0], t[1], t[2]);
    }
    s += "    </triangles>\n   </mesh>\n  </object>\n </resources>\n <build>\n  <item objectid=\"1\"/>\n </build>\n</model>\n";
    s
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out += "&amp;",
            '<' => out += "&lt;",
            '>' => out += "&gt;",
            '"' => out += "&quot;",
            '\'' => out += "&apos;",
            // XML 1.0 has no form for other control characters.
            c if c.is_control() && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_escaped() {
        assert_eq!(escape("a<b & \"c\"\u{1}"), "a&lt;b &amp; &quot;c&quot;");
        let mesh = Welded { positions: vec![[0.5, -0.0, 1e-7]], triangles: vec![] };
        let xml = model(&mesh, "R&D");
        assert!(xml.contains("<metadata name=\"Title\">R&amp;D</metadata>"));
        assert!(xml.contains("<vertex x=\"0.5\" y=\"-0\" z=\"0.0000001\"/>"), "{xml}");
    }
}
