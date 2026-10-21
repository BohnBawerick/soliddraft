//! Writers for a sheet's display list: SVG for previews and golden files, PDF for print,
//! DXF for shops, and a PNG thumbnail for the drawing file. They write
//! [`DisplayList::printable`] items only, so the interface's own marks stay on screen.

pub mod arcs;
pub mod dxf;
pub mod pdf;
pub mod svg;
pub mod thumbnail;

use std::path::Path;

use soliddraft_kernel_api::Point2;

pub use dxf::dxf;
pub use pdf::{PdfInfo, pdf};
pub use svg::svg;

use crate::DrawingError;
use crate::display::{CAP_HEIGHT, DisplayList, Role, TextItem, VAlign, text_width};
use crate::library::Align;

/// A file format a sheet can be written as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Format {
    /// PDF, one page per sheet.
    Pdf,
    /// DXF R2000 in millimetres.
    Dxf,
    /// SVG at paper size.
    Svg,
}

impl Format {
    /// Every format.
    pub const ALL: [Format; 3] = [Format::Pdf, Format::Dxf, Format::Svg];

    /// The file extension, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Format::Pdf => "pdf",
            Format::Dxf => "dxf",
            Format::Svg => "svg",
        }
    }

    /// The name shown in menus.
    pub fn label(self) -> &'static str {
        match self {
            Format::Pdf => "PDF",
            Format::Dxf => "DXF",
            Format::Svg => "SVG",
        }
    }

    /// The format a file name's extension asks for.
    pub fn from_extension(ext: &str) -> Option<Format> {
        Format::ALL.into_iter().find(|f| f.extension().eq_ignore_ascii_case(ext))
    }
}

/// One sheet's bytes in `format`. `title` goes into the PDF's document properties.
pub fn bytes(list: &DisplayList, format: Format, title: &str) -> Result<Vec<u8>, DrawingError> {
    match format {
        Format::Pdf => pdf(std::slice::from_ref(list), &PdfInfo { title: title.to_owned(), author: String::new() }),
        Format::Dxf => dxf(list),
        Format::Svg => Ok(svg(list).into_bytes()),
    }
}

/// Writes one sheet in `format` to `path`, creating its directory.
pub fn write(list: &DisplayList, format: Format, title: &str, path: &Path) -> Result<(), DrawingError> {
    let data = bytes(list, format, title)?;
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
    {
        std::fs::create_dir_all(dir).map_err(|e| DrawingError::Io(format!("cannot create {}: {e}", dir.display())))?;
    }
    std::fs::write(path, data).map_err(|e| DrawingError::Io(format!("cannot write {}: {e}", path.display())))
}

/// The DXF layer of a role.
pub fn layer(role: Role) -> &'static str {
    match role {
        Role::Visible => "VISIBLE",
        Role::Hidden => "HIDDEN",
        Role::Phantom => "PHANTOM",
        Role::Centre => "CENTER",
        Role::Dimension | Role::Error => "DIMENSION",
        Role::Border | Role::Thin | Role::Label | Role::Changed | Role::Stale => "FORMAT",
        Role::Text => "TEXT",
        Role::Hatch => "HATCH",
        Role::CuttingPlane => "CUTTING",
        Role::Boundary => "THIN",
    }
}

/// Every layer, in drawing order.
pub const LAYERS: [&str; 10] =
    ["FORMAT", "HATCH", "HIDDEN", "PHANTOM", "THIN", "VISIBLE", "CENTER", "CUTTING", "DIMENSION", "TEXT"];

/// RGB of a role on paper: red for annotations that lost a reference, black otherwise.
pub fn colour(role: Role) -> [u8; 3] {
    match role {
        Role::Error => [200, 30, 30],
        _ => [0, 0, 0],
    }
}

/// Where a text's baseline starts and its font size: the item's position moved by its
/// alignment along and across the rotated baseline.
pub fn baseline(t: &TextItem) -> (Point2, f64) {
    let width = text_width(&t.text, t.height);
    let along = match t.halign {
        Align::Left => 0.0,
        Align::Centre => -width / 2.0,
        Align::Right => -width,
    };
    let across = match t.valign {
        VAlign::Bottom => 0.0,
        VAlign::Centre => -t.height / 2.0,
        VAlign::Top => -t.height,
    };
    let (sin, cos) = t.angle.to_radians().sin_cos();
    let at = Point2::new(t.pos.u + along * cos - across * sin, t.pos.v + along * sin + across * cos);
    (at, t.height / CAP_HEIGHT)
}
