//! Importing SolidWorks material libraries (`.sldmat`).
//!
//! A `.sldmat` file is XML: `classification` elements (the category) hold `material`
//! elements, each with a name, a hatch (`xhatch`), a swatch colour (`swatchcolor RGB`)
//! and physical properties in SI units (`physicalproperties`, one element per property:
//! `EX` elastic modulus, `NUXY` Poisson's ratio, `GXY` shear modulus, `DENS` density,
//! `SIGXT` tensile, `SIGXC` compressive and `SIGYLD` yield strength, `ALPX` thermal
//! expansion, `KX` conductivity, `C` specific heat).
//!
//! Each material becomes a [`Material`] and, when it has a swatch colour, an
//! [`Appearance`] it names as its default. A metal shader (a path under `\metal\`)
//! makes the appearance metallic. Properties this crate has no field for are skipped
//! and listed in the warnings; a property written as 0 is taken as not given, since
//! SolidWorks writes 0 for an empty field. Values outside what any solid has (a density
//! under 50 or over 25 000 kg/m³) are kept as the file says and warned about, because
//! they usually mean a library written in other units.
//!
//! Every value read records the file as its source, with the material's
//! `propertysource` when the file gives one, so an imported value never reads as from
//! an unknown source. The file states no conditions SolidDraft keeps, so none are
//! recorded.

use crate::appearance::{Appearance, AppearanceId};
use crate::colour::Srgb;
use crate::library::{Library, Origin, slug, unique};
use crate::material::{Material, MaterialId};
use crate::provenance::{Property, PropertySource, Provenance};

/// Why a `.sldmat` file could not be imported.
#[derive(Debug, thiserror::Error)]
pub enum SldmatError {
    /// The bytes are not text in an encoding SolidWorks writes.
    #[error("the file is not UTF-8 or UTF-16 text")]
    Encoding,
    /// The text is not well-formed XML.
    #[error("the file is not valid XML: {0}")]
    Xml(String),
    /// The XML is not a SolidWorks material library.
    #[error("the file is not a SolidWorks material library: {0}")]
    NotALibrary(String),
}

/// A `.sldmat` file read as a SolidDraft library.
#[derive(Debug, Clone)]
pub struct Imported {
    /// The library: one material per SolidWorks material, and their appearances. It
    /// has no file yet.
    pub library: Library,
    /// What did not carry over, one line each.
    pub warnings: Vec<String>,
}

/// Reads a `.sldmat` file's bytes. `name` names the library, usually the file's stem.
pub fn import(bytes: &[u8], name: &str) -> Result<Imported, SldmatError> {
    let text = decode(bytes)?;
    let doc = roxmltree::Document::parse(&text).map_err(|e| SldmatError::Xml(e.to_string()))?;
    let root = doc.root_element();
    if root.tag_name().name() != "materials" {
        return Err(SldmatError::NotALibrary(format!("its root element is <{}>", root.tag_name().name())));
    }
    let mut library = Library::new(name, Origin::File);
    library.source = Some(format!("Imported from the SolidWorks library {name}.sldmat"));
    let file = format!("{name}.sldmat");
    let mut warnings = Vec::new();
    for class in root.children().filter(|n| n.has_tag_name("classification")) {
        let category = class.attribute("name").unwrap_or_default();
        for node in class.children().filter(|n| n.has_tag_name("material")) {
            let (material, appearance) = read_material(node, category, &file, &library, &mut warnings);
            if let Some(a) = appearance {
                library.appearances.push(a);
            }
            library.materials.push(material);
        }
    }
    if library.materials.is_empty() {
        return Err(SldmatError::NotALibrary("it holds no materials".into()));
    }
    Ok(Imported { library, warnings })
}

fn decode(bytes: &[u8]) -> Result<String, SldmatError> {
    let utf16 = |be: bool| {
        let units = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| if be { u16::from_be_bytes(c) } else { u16::from_le_bytes(c) });
        char::decode_utf16(units).collect::<Result<String, _>>().map_err(|_| SldmatError::Encoding)
    };
    match bytes {
        [0xff, 0xfe, ..] => utf16(false),
        [0xfe, 0xff, ..] => utf16(true),
        [0xef, 0xbb, 0xbf, rest @ ..] => String::from_utf8(rest.to_vec()).map_err(|_| SldmatError::Encoding),
        _ => String::from_utf8(bytes.to_vec()).map_err(|_| SldmatError::Encoding),
    }
}

fn read_material(
    node: roxmltree::Node,
    category: &str,
    file: &str,
    library: &Library,
    warnings: &mut Vec<String>,
) -> (Material, Option<Appearance>) {
    let name = node.attribute("name").unwrap_or("Unnamed material").trim().to_owned();
    let id = MaterialId(unique(&slug(&name), |id| library.materials.iter().any(|m| m.id.0 == id)));
    let mut m = Material::new(id.0.clone(), name.clone());
    m.category = category.to_owned();
    if let Some(s) = node.attribute("propertysource").map(str::trim).filter(|s| !s.is_empty()) {
        m.source = Some(s.to_owned());
    }
    let child = |tag: &str| node.children().find(|n| n.has_tag_name(tag));
    let source =
        Provenance::from_source(PropertySource::Sldmat { file: file.to_owned(), property_source: m.source.clone() });
    if let Some(props) = child("physicalproperties") {
        for p in props.children().filter(roxmltree::Node::is_element) {
            let tag = p.tag_name().name();
            let Some(value) = p.attribute("value") else { continue };
            let Ok(v) = value.trim().parse::<f64>() else {
                warnings.push(format!("{name}: {tag} {value:?} is not a number"));
                continue;
            };
            let property = match tag {
                "EX" => Property::YoungsModulus,
                "NUXY" => Property::Poisson,
                "GXY" => Property::ShearModulus,
                "DENS" => Property::Density,
                "SIGXT" => Property::Tensile,
                "SIGXC" => Property::Compressive,
                "SIGYLD" => Property::Yield,
                "ALPX" => Property::ThermalExpansion,
                "KX" => Property::Conductivity,
                "C" => Property::SpecificHeat,
                "materialmodel" | "REFTEMP" => continue,
                other => {
                    warnings.push(format!("{name}: {other} is not carried over"));
                    continue;
                }
            };
            m.set_value(property, (v != 0.0).then_some(v), source.clone());
        }
    }
    if let Some(d) = m.density_kg_m3
        && !(50.0..=25_000.0).contains(&d)
    {
        warnings.push(format!("{name}: a density of {d} kg/m³ is unlike any solid; check the file's units"));
    }
    for p in m.problems() {
        warnings.push(format!("{name}: {p}"));
    }
    if let Some(h) = child("xhatch").and_then(|h| h.attribute("name")) {
        // "ANSI38 (Aluminum)": the pattern is the first word.
        m.hatch = h.split_whitespace().next().map(str::to_owned);
    }
    let colour = child("swatchcolor")
        .or_else(|| child("swcolor"))
        .and_then(|c| c.attribute("RGB"))
        .and_then(|rgb| format!("#{rgb}").parse::<Srgb>().ok());
    let appearance = colour.map(|c| {
        let metal = node
            .descendants()
            .filter(|n| n.tag_name().name().starts_with("pwshader"))
            .filter_map(|n| n.attribute("path"))
            .any(|p| p.to_ascii_lowercase().replace('/', "\\").starts_with("\\metal\\"));
        let mut a = Appearance::new(id.0.clone(), name.clone(), c);
        if metal {
            a.metallic = 1.0;
        }
        m.default_appearance = Some(AppearanceId(a.id.0.clone()));
        a
    });
    (m, appearance)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0"?>
<mstns:materials xmlns:mstns="http://www.solidworks.com/sldmaterials" xmlns:sldcolorswatch="http://www.solidworks.com/sldcolorswatch" version="2008.03">
  <curves id="curve0"><point x="1.0" y="1.0"/></curves>
  <classification name="Steel">
    <material name="0.7040 (GGG 40)">
      <shaders><pwshader2 path="\metal\steel\polished steel.p2m" name="polished steel"/></shaders>
      <swatchcolor RGB="c5c5c5"><sldcolorswatch:Optical Ambient="0.52" Transparency="0.0"/></swatchcolor>
      <xhatch name="ANSI31 (Iron BrickStone)" angle="0.000000" scale="1.000000"/>
      <physicalproperties>
        <EX displayname="Elastic Modulus" value="165000003584.000000"/>
        <NUXY displayname="Poissons Ratio" value="0.280000"/>
        <DENS displayname="Density" value="7300.000000"/>
        <SIGXC displayname="Compressive strength" value="0"/>
        <VISC displayname="Viscosity" value="1"/>
      </physicalproperties>
    </material>
    <material name="0.7040 (GGG 40)">
      <physicalproperties><DENS value="0.22"/></physicalproperties>
    </material>
  </classification>
  <classification name="Plastics">
    <material name="ABS" propertysource="Supplier sheet 4">
      <shaders><pwshader2 path="\plastic\high gloss\abs.p2m"/></shaders>
      <swatchcolor RGB="F0F0E6"/>
      <physicalproperties><DENS value="1050"/></physicalproperties>
    </material>
  </classification>
</mstns:materials>"#;

    #[test]
    fn materials_appearances_and_warnings_carry_over() {
        let out = import(SAMPLE.as_bytes(), "Cast irons").unwrap();
        let lib = &out.library;
        assert_eq!(lib.materials.len(), 3);
        let ggg = &lib.materials[0];
        assert_eq!(ggg.id.0, "0-7040-ggg-40");
        assert_eq!(ggg.category, "Steel");
        assert_eq!(ggg.density_kg_m3, Some(7300.0));
        assert_eq!(ggg.youngs_modulus_pa, Some(165000003584.0));
        assert_eq!(ggg.compressive_pa, None, "0 means not given");
        assert_eq!(ggg.hatch.as_deref(), Some("ANSI31"));
        assert_eq!(lib.materials[1].id.0, "0-7040-ggg-40-2", "a repeated name gets its own id");
        let steel = lib.appearance(ggg.default_appearance.as_ref().unwrap()).unwrap();
        assert_eq!((steel.base_color, steel.metallic), (Srgb::new(0xc5, 0xc5, 0xc5), 1.0));
        let abs = &lib.materials[2];
        assert_eq!(abs.source.as_deref(), Some("Supplier sheet 4"));
        assert_eq!(lib.appearance(abs.default_appearance.as_ref().unwrap()).unwrap().metallic, 0.0);
        let from_file = PropertySource::Sldmat { file: "Cast irons.sldmat".into(), property_source: None };
        assert_eq!(ggg.source_of(Property::Density), &from_file);
        assert_eq!(ggg.source_of(Property::YoungsModulus), &from_file);
        assert!(ggg.source_of(Property::Compressive).is_unknown(), "a value not given has no source");
        assert!(!ggg.provenance.contains_key(&Property::Compressive));
        let sheet = PropertySource::Sldmat {
            file: "Cast irons.sldmat".into(),
            property_source: Some("Supplier sheet 4".into()),
        };
        assert_eq!(abs.source_of(Property::Density), &sheet, "the file's own property source is kept");
        assert_eq!(out.warnings.len(), 2, "{:?}", out.warnings);
        assert!(out.warnings[0].contains("VISC"));
        assert!(out.warnings[1].contains("unlike any solid"));
        assert!(lib.problems().is_empty());
    }

    #[test]
    fn utf16_files_and_bad_input() {
        let mut bytes = vec![0xff, 0xfe];
        bytes.extend(SAMPLE.encode_utf16().flat_map(u16::to_le_bytes));
        assert_eq!(import(&bytes, "x").unwrap().library.materials.len(), 3);
        assert!(matches!(import(b"<a/>", "x"), Err(SldmatError::NotALibrary(_))));
        assert!(matches!(import(b"<materials/>", "x"), Err(SldmatError::NotALibrary(_))));
        assert!(matches!(import(b"<materials", "x"), Err(SldmatError::Xml(_))));
        assert!(matches!(import(&[0xc3, 0x28], "x"), Err(SldmatError::Encoding)));
    }
}
