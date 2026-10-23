//! Where a material's values come from, and what they hold under.
//!
//! A [`Material`](crate::Material) keeps its values in plain fields, which mass and
//! everything else read. Beside them it can keep a [`Provenance`] for each value: its
//! [`PropertySource`] and the [`Conditions`] it holds under, such as a temperature, a
//! product form or a heat treatment. A value with no provenance has an unknown source
//! and no stated conditions; nothing is inferred from the material's own `source` text
//! or from the library it sits in.

use std::fmt;

use serde::{Deserialize, Serialize};

/// One of a material's values, named as its field is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Property {
    /// Density, kg/m³.
    #[serde(rename = "density_kg_m3")]
    Density,
    /// Young's modulus, Pa.
    #[serde(rename = "youngs_modulus_pa")]
    YoungsModulus,
    /// Poisson's ratio.
    #[serde(rename = "poisson")]
    Poisson,
    /// Shear modulus, Pa.
    #[serde(rename = "shear_modulus_pa")]
    ShearModulus,
    /// Yield strength in tension, Pa.
    #[serde(rename = "yield_pa")]
    Yield,
    /// Ultimate tensile strength, Pa.
    #[serde(rename = "tensile_pa")]
    Tensile,
    /// Ultimate compressive strength, Pa.
    #[serde(rename = "compressive_pa")]
    Compressive,
    /// Linear coefficient of thermal expansion, 1/K.
    #[serde(rename = "thermal_expansion_per_k")]
    ThermalExpansion,
    /// Thermal conductivity, W/(m·K).
    #[serde(rename = "conductivity_w_mk")]
    Conductivity,
    /// Specific heat capacity, J/(kg·K).
    #[serde(rename = "specific_heat_j_kgk")]
    SpecificHeat,
}

impl Property {
    /// Every property, in the order the material's fields are written.
    pub const ALL: [Property; 10] = [
        Property::Density,
        Property::YoungsModulus,
        Property::Poisson,
        Property::ShearModulus,
        Property::Yield,
        Property::Tensile,
        Property::Compressive,
        Property::ThermalExpansion,
        Property::Conductivity,
        Property::SpecificHeat,
    ];

    /// The field's name as written in files: "density_kg_m3".
    pub fn field(self) -> &'static str {
        match self {
            Property::Density => "density_kg_m3",
            Property::YoungsModulus => "youngs_modulus_pa",
            Property::Poisson => "poisson",
            Property::ShearModulus => "shear_modulus_pa",
            Property::Yield => "yield_pa",
            Property::Tensile => "tensile_pa",
            Property::Compressive => "compressive_pa",
            Property::ThermalExpansion => "thermal_expansion_per_k",
            Property::Conductivity => "conductivity_w_mk",
            Property::SpecificHeat => "specific_heat_j_kgk",
        }
    }

    /// The name people see: "Density".
    pub fn label(self) -> &'static str {
        match self {
            Property::Density => "Density",
            Property::YoungsModulus => "Young's modulus",
            Property::Poisson => "Poisson's ratio",
            Property::ShearModulus => "Shear modulus",
            Property::Yield => "Yield strength",
            Property::Tensile => "Tensile strength",
            Property::Compressive => "Compressive strength",
            Property::ThermalExpansion => "Thermal expansion",
            Property::Conductivity => "Thermal conductivity",
            Property::SpecificHeat => "Specific heat",
        }
    }

    /// The value's unit, empty for a ratio: "kg/m³".
    pub fn unit(self) -> &'static str {
        match self {
            Property::Density => "kg/m³",
            Property::YoungsModulus
            | Property::ShearModulus
            | Property::Yield
            | Property::Tensile
            | Property::Compressive => "Pa",
            Property::Poisson => "",
            Property::ThermalExpansion => "1/K",
            Property::Conductivity => "W/(m·K)",
            Property::SpecificHeat => "J/(kg·K)",
        }
    }
}

impl fmt::Display for Property {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.field())
    }
}

/// Where a value comes from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PropertySource {
    /// Nobody recorded where the value comes from. What a value with no provenance
    /// reads as.
    #[default]
    Unknown,
    /// Taken from another library's entry, which records nothing more.
    Library {
        /// The library's name.
        library: String,
    },
    /// Typed in by the user in place of what the source gave.
    UserOverride {
        /// Why, when the user said.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// Read from a SolidWorks `.sldmat` library.
    Sldmat {
        /// The file's name: "Cast irons.sldmat".
        file: String,
        /// The material's `propertysource` in the file, when it gives one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        property_source: Option<String>,
    },
    /// A standard, handbook or data sheet, cited precisely enough to check the value.
    Reference {
        /// "MIL-HDBK-5J, table 2.2.1.0(b)".
        citation: String,
    },
}

impl PropertySource {
    /// True when nobody recorded where the value comes from.
    pub fn is_unknown(&self) -> bool {
        *self == PropertySource::Unknown
    }
}

impl fmt::Display for PropertySource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PropertySource::Unknown => f.write_str("source unknown"),
            PropertySource::Library { library } => write!(f, "from the library {library}"),
            PropertySource::UserOverride { note: None } => f.write_str("set by the user"),
            PropertySource::UserOverride { note: Some(n) } => write!(f, "set by the user: {n}"),
            PropertySource::Sldmat { file, property_source: None } => write!(f, "imported from {file}"),
            PropertySource::Sldmat { file, property_source: Some(s) } => write!(f, "imported from {file} ({s})"),
            PropertySource::Reference { citation } => write!(f, "from {citation}"),
        }
    }
}

/// What a value holds under. Every condition is optional, and one not given is not
/// stated by the source, not assumed to be room temperature or any other default.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Conditions {
    /// The temperature the value holds at, K; the bottom of the range when
    /// `temperature_to_k` is given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature_k: Option<f64>,
    /// The top of the temperature range the value holds over, K.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature_to_k: Option<f64>,
    /// The product form: "sheet", "plate", "extrusion", "casting".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub form: Option<String>,
    /// The heat treatment or temper: "T6", "annealed", "normalized and tempered".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub treatment: Option<String>,
    /// The direction the value holds in: "longitudinal", "along the grain".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
}

impl Conditions {
    /// True when no condition is stated.
    pub fn is_empty(&self) -> bool {
        *self == Conditions::default()
    }

    /// What is wrong with the conditions: a temperature that is not above 0 K, a range
    /// whose top is below its bottom, or a top with no bottom.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (name, t) in [("temperature_k", self.temperature_k), ("temperature_to_k", self.temperature_to_k)] {
            if let Some(t) = t
                && !(t.is_finite() && t > 0.0)
            {
                out.push(format!("{name} is {t}; it must be above 0"));
            }
        }
        match (self.temperature_k, self.temperature_to_k) {
            (None, Some(_)) => out.push("temperature_to_k is given without temperature_k".into()),
            (Some(from), Some(to)) if to < from => {
                out.push(format!("temperature_to_k is {to}, below temperature_k {from}"));
            }
            _ => {}
        }
        out
    }
}

impl fmt::Display for Conditions {
    /// "at 293.15 K, sheet, T6, longitudinal"; empty when no condition is stated.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::new();
        match (self.temperature_k, self.temperature_to_k) {
            (Some(from), Some(to)) => parts.push(format!("from {from} K to {to} K")),
            (Some(t), None) => parts.push(format!("at {t} K")),
            (None, Some(to)) => parts.push(format!("up to {to} K")),
            (None, None) => {}
        }
        parts.extend([&self.form, &self.treatment, &self.direction].into_iter().flatten().cloned());
        f.write_str(&parts.join(", "))
    }
}

/// A value's source and the conditions it holds under.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    /// Where the value comes from.
    #[serde(default)]
    pub source: PropertySource,
    /// What it holds under.
    #[serde(default, skip_serializing_if = "Conditions::is_empty")]
    pub conditions: Conditions,
}

impl Provenance {
    /// A value from `source`, under no stated conditions.
    pub fn from_source(source: PropertySource) -> Provenance {
        Provenance { source, conditions: Conditions::default() }
    }
}

impl fmt::Display for Provenance {
    /// "imported from Shop.sldmat, at 293.15 K".
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.source)?;
        if !self.conditions.is_empty() {
            write!(f, ", {}", self.conditions)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn property_names_are_the_field_names() {
        for p in Property::ALL {
            assert_eq!(serde_json::to_value(p).unwrap(), p.field());
            assert_eq!(serde_json::from_value::<Property>(p.field().into()).unwrap(), p);
        }
    }

    #[test]
    fn sources_and_conditions_read_and_print() {
        let p: Provenance = toml::from_str(
            r#"
            source = { kind = "reference", citation = "MIL-HDBK-5J, table 3.2.3.0(b)" }
            conditions = { temperature_k = 293.15, form = "sheet", treatment = "T3", direction = "longitudinal" }
            "#,
        )
        .unwrap();
        assert_eq!(p.to_string(), "from MIL-HDBK-5J, table 3.2.3.0(b), at 293.15 K, sheet, T3, longitudinal");
        assert!(p.conditions.problems().is_empty());
        let bare: Provenance = toml::from_str("").unwrap();
        assert!(bare.source.is_unknown(), "a provenance with no source has an unknown one");
        assert_eq!(serde_json::to_string(&bare).unwrap(), r#"{"source":{"kind":"unknown"}}"#);
        let range = Conditions { temperature_k: Some(323.15), temperature_to_k: Some(373.15), ..Conditions::default() };
        assert_eq!(range.to_string(), "from 323.15 K to 373.15 K");
        let user = PropertySource::UserOverride { note: Some("measured on the batch".into()) };
        assert_eq!(user.to_string(), "set by the user: measured on the batch");
    }

    #[test]
    fn bad_temperatures_are_problems() {
        let c = Conditions { temperature_k: Some(-5.0), temperature_to_k: Some(f64::NAN), ..Conditions::default() };
        assert_eq!(c.problems().len(), 2);
        let c = Conditions { temperature_k: Some(400.0), temperature_to_k: Some(300.0), ..Conditions::default() };
        assert_eq!(c.problems(), ["temperature_to_k is 300, below temperature_k 400"]);
        let c = Conditions { temperature_to_k: Some(300.0), ..Conditions::default() };
        assert_eq!(c.problems(), ["temperature_to_k is given without temperature_k"]);
    }
}
