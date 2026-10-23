//! Materials: what a part is made of.
//!
//! A [`Material`] holds physical properties in SI units, with the unit in each field's
//! name. Every engineering value is optional: a value the source does not give stays
//! empty, and whatever needs it (mass needs the density) reports it as unknown rather
//! than guessing. A material suggests an appearance for parts made of it, but how a
//! surface looks is the [`Appearance`](crate::Appearance)'s business.
//!
//! Each value can also record where it comes from and what it holds under (see
//! [`crate::provenance`]). A value that records nothing has an unknown source.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::appearance::AppearanceId;
use crate::problem::Problem;
use crate::provenance::{Property, PropertySource, Provenance};

/// A material's id: stable, unique among the materials an app has loaded, and not the
/// material's name. Documents refer to materials by it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MaterialId(pub String);

impl MaterialId {
    /// The id as written.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for MaterialId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A physical material.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Material {
    /// The material's id.
    pub id: MaterialId,
    /// The name people see: "Aluminium 6061-T6".
    pub name: String,
    /// The group a library lists it under: "Aluminium alloys".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub category: String,
    /// Where the values come from, precise enough to check them: a handbook and table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// The terms the values are used under, when the source sets any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub licence: Option<String>,
    /// Density, kg/m³.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub density_kg_m3: Option<f64>,
    /// Young's modulus (elastic modulus in tension), Pa.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub youngs_modulus_pa: Option<f64>,
    /// Poisson's ratio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poisson: Option<f64>,
    /// Shear modulus, Pa.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shear_modulus_pa: Option<f64>,
    /// Yield strength in tension, Pa.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yield_pa: Option<f64>,
    /// Ultimate tensile strength, Pa.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tensile_pa: Option<f64>,
    /// Ultimate compressive strength, Pa.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compressive_pa: Option<f64>,
    /// Linear coefficient of thermal expansion, 1/K.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thermal_expansion_per_k: Option<f64>,
    /// Thermal conductivity, W/(m·K).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conductivity_w_mk: Option<f64>,
    /// Specific heat capacity, J/(kg·K).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub specific_heat_j_kgk: Option<f64>,
    /// The appearance a part made of this material gets when it has none of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_appearance: Option<AppearanceId>,
    /// The hatch pattern a drawing's section view fills this material's cut faces with,
    /// by name; the drawing's default hatch when empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hatch: Option<String>,
    /// Where each value comes from and what it holds under, by property. A value with
    /// no entry here has an unknown source; see [`Material::provenance_of`].
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub provenance: BTreeMap<Property, Provenance>,
}

impl Material {
    /// A material with only an id and a name; every value unknown.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Material {
        Material { id: MaterialId(id.into()), name: name.into(), ..Material::default() }
    }

    /// A value.
    pub fn value(&self, property: Property) -> Option<f64> {
        match property {
            Property::Density => self.density_kg_m3,
            Property::YoungsModulus => self.youngs_modulus_pa,
            Property::Poisson => self.poisson,
            Property::ShearModulus => self.shear_modulus_pa,
            Property::Yield => self.yield_pa,
            Property::Tensile => self.tensile_pa,
            Property::Compressive => self.compressive_pa,
            Property::ThermalExpansion => self.thermal_expansion_per_k,
            Property::Conductivity => self.conductivity_w_mk,
            Property::SpecificHeat => self.specific_heat_j_kgk,
        }
    }

    /// A value's field.
    pub fn value_mut(&mut self, property: Property) -> &mut Option<f64> {
        match property {
            Property::Density => &mut self.density_kg_m3,
            Property::YoungsModulus => &mut self.youngs_modulus_pa,
            Property::Poisson => &mut self.poisson,
            Property::ShearModulus => &mut self.shear_modulus_pa,
            Property::Yield => &mut self.yield_pa,
            Property::Tensile => &mut self.tensile_pa,
            Property::Compressive => &mut self.compressive_pa,
            Property::ThermalExpansion => &mut self.thermal_expansion_per_k,
            Property::Conductivity => &mut self.conductivity_w_mk,
            Property::SpecificHeat => &mut self.specific_heat_j_kgk,
        }
    }

    /// Where a value comes from and what it holds under. A value that records none
    /// reads as from an unknown source under no stated conditions, whatever the
    /// material's `source` text says.
    pub fn provenance_of(&self, property: Property) -> Provenance {
        self.provenance.get(&property).cloned().unwrap_or_default()
    }

    /// Where a value comes from; [`PropertySource::Unknown`] when nothing is recorded.
    pub fn source_of(&self, property: Property) -> &PropertySource {
        const UNKNOWN: &PropertySource = &PropertySource::Unknown;
        self.provenance.get(&property).map_or(UNKNOWN, |p| &p.source)
    }

    /// Sets a value from `provenance`, or removes the value and its provenance with
    /// `None`. An unknown source under no stated conditions is not written down, so it
    /// reads the same as a value that never recorded one.
    pub fn set_value(&mut self, property: Property, value: Option<f64>, provenance: Provenance) {
        *self.value_mut(property) = value;
        if value.is_some() && provenance != Provenance::default() {
            self.provenance.insert(property, provenance);
        } else {
            self.provenance.remove(&property);
        }
    }

    /// Sets a value the user typed in, in place of what the source gave, or removes it
    /// with `None`. The old source and conditions go with the old value.
    pub fn set_override(&mut self, property: Property, value: Option<f64>, note: Option<String>) {
        self.set_value(property, value, Provenance::from_source(PropertySource::UserOverride { note }));
    }

    /// What is wrong with the material: an empty id or name, or a value that is not a
    /// number or is out of its physical range. Empty when it is usable as it is.
    pub fn problems(&self) -> Vec<Problem> {
        let mut out = Vec::new();
        if self.id.0.trim().is_empty() {
            out.push(Problem::new("id", "is empty"));
        }
        if self.name.trim().is_empty() {
            out.push(Problem::new("name", "is empty"));
        }
        let positive = [
            ("density_kg_m3", self.density_kg_m3),
            ("youngs_modulus_pa", self.youngs_modulus_pa),
            ("shear_modulus_pa", self.shear_modulus_pa),
            ("yield_pa", self.yield_pa),
            ("tensile_pa", self.tensile_pa),
            ("compressive_pa", self.compressive_pa),
            ("conductivity_w_mk", self.conductivity_w_mk),
            ("specific_heat_j_kgk", self.specific_heat_j_kgk),
        ];
        for (field, v) in positive {
            if let Some(v) = v
                && !(v.is_finite() && v > 0.0)
            {
                out.push(Problem::new(field, format!("is {v}; it must be above 0")));
            }
        }
        // Poisson's ratio of a stable isotropic material lies between -1 and 0.5.
        if let Some(v) = self.poisson
            && !(v > -1.0 && v <= 0.5)
        {
            out.push(Problem::new("poisson", format!("is {v}; it must be above -1 and at most 0.5")));
        }
        // Some materials shrink as they warm, so only a number is required.
        if let Some(v) = self.thermal_expansion_per_k
            && !v.is_finite()
        {
            out.push(Problem::new("thermal_expansion_per_k", format!("is {v}; it must be a number")));
        }
        for (property, p) in &self.provenance {
            if self.value(*property).is_none() {
                out.push(Problem::new("provenance", format!("names {property}, which has no value")));
            }
            for message in p.conditions.problems() {
                out.push(Problem::new("provenance", format!("{property}: {message}")));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::Conditions;

    #[test]
    fn unknown_values_stay_unknown() {
        let m: Material = toml::from_str(
            r#"
            id = "pine"
            name = "Pine, eastern white"
            category = "Woods"
            source = "USDA Wood Handbook (FPL-GTR-282), table 5-3a"
            density_kg_m3 = 400.0
            "#,
        )
        .unwrap();
        assert_eq!(m.density_kg_m3, Some(400.0));
        assert_eq!(m.yield_pa, None);
        assert!(m.problems().is_empty());
        let json = serde_json::to_value(&m).unwrap();
        assert!(json.get("yield_pa").is_none(), "an unknown value is left out, not written as 0");
        assert_eq!(serde_json::from_value::<Material>(json).unwrap(), m);
    }

    #[test]
    fn values_out_of_range_are_problems() {
        let mut m = Material::new("x", "X");
        m.density_kg_m3 = Some(0.0);
        m.poisson = Some(0.6);
        m.youngs_modulus_pa = Some(f64::NAN);
        m.thermal_expansion_per_k = Some(-2e-6);
        let fields: Vec<&str> = m.problems().iter().map(|p| p.field).collect();
        assert_eq!(fields, ["density_kg_m3", "youngs_modulus_pa", "poisson"]);
        assert_eq!(Material::new(" ", "").problems().len(), 2);
    }

    #[test]
    fn values_without_provenance_have_an_unknown_source() {
        let m: Material = toml::from_str(
            r#"
            id = "steel"
            name = "Steel"
            source = "MIL-HDBK-5J, table 2.2.1.0(b)"
            density_kg_m3 = 7861.0
            "#,
        )
        .unwrap();
        for p in Property::ALL {
            assert!(m.source_of(p).is_unknown(), "{p} is not guessed from the material's source text");
            assert_eq!(m.provenance_of(p), Provenance::default());
        }
        let json = serde_json::to_value(&m).unwrap();
        assert!(json.get("provenance").is_none(), "a material with none writes none");
    }

    #[test]
    fn overrides_replace_the_source_and_the_conditions() {
        let mut m = Material::new("al", "Aluminium 6061-T6");
        let handbook = Provenance {
            source: PropertySource::Reference { citation: "MIL-HDBK-5J, table 3.6.2.0(b)".into() },
            conditions: Conditions {
                temperature_k: Some(293.15),
                treatment: Some("T6".into()),
                ..Conditions::default()
            },
        };
        m.set_value(Property::Yield, Some(2.41e8), handbook.clone());
        assert_eq!(m.provenance_of(Property::Yield), handbook);
        m.set_override(Property::Yield, Some(2.6e8), Some("mill certificate".into()));
        assert_eq!(m.yield_pa, Some(2.6e8));
        let p = m.provenance_of(Property::Yield);
        assert_eq!(p.source, PropertySource::UserOverride { note: Some("mill certificate".into()) });
        assert!(p.conditions.is_empty(), "the handbook's conditions went with its value");
        m.set_value(Property::Density, Some(2700.0), Provenance::default());
        assert!(!m.provenance.contains_key(&Property::Density), "an unknown source is not written down");
        m.set_override(Property::Yield, None, None);
        assert_eq!((m.yield_pa, m.provenance.len()), (None, 0));
    }

    #[test]
    fn provenance_must_match_a_value_and_hold_sound_conditions() {
        let mut m = Material::new("x", "X");
        m.provenance.insert(Property::Density, Provenance::from_source(PropertySource::Unknown));
        m.set_value(
            Property::Tensile,
            Some(3e8),
            Provenance {
                source: PropertySource::Unknown,
                conditions: Conditions { temperature_k: Some(0.0), ..Conditions::default() },
            },
        );
        let messages: Vec<String> = m.problems().iter().map(ToString::to_string).collect();
        assert_eq!(
            messages,
            [
                "provenance names density_kg_m3, which has no value",
                "provenance tensile_pa: temperature_k is 0; it must be above 0"
            ]
        );
    }
}
