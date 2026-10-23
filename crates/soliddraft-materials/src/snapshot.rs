//! The copies of materials and appearances a document keeps.
//!
//! A document holds, by value, every material and appearance it uses. Its look and its
//! mass then never depend on a library being present or unchanged: a part opened on
//! another machine, or after a library was edited, shows what it showed when it was
//! saved. [`Catalog::match_material`](crate::Catalog::match_material) says when a copy
//! differs from its library, and updating it is the user's choice.

use serde::{Deserialize, Serialize};

use crate::appearance::{Appearance, AppearanceId};
use crate::material::{Material, MaterialId};

/// A document's copies of the materials and appearances it uses, each list sorted by
/// id so the same set always writes the same JSON.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshots {
    /// The materials.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub materials: Vec<Material>,
    /// The appearances.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub appearances: Vec<Appearance>,
}

impl Snapshots {
    /// True when the document uses no material and no appearance.
    pub fn is_empty(&self) -> bool {
        self.materials.is_empty() && self.appearances.is_empty()
    }

    /// The copy of a material.
    pub fn material(&self, id: &MaterialId) -> Option<&Material> {
        self.materials.iter().find(|m| &m.id == id)
    }

    /// The copy of an appearance.
    pub fn appearance(&self, id: &AppearanceId) -> Option<&Appearance> {
        self.appearances.iter().find(|a| &a.id == id)
    }

    /// Stores a copy of a material, replacing any copy with its id.
    pub fn put_material(&mut self, material: Material) {
        match self.materials.binary_search_by(|m| m.id.cmp(&material.id)) {
            Ok(i) => self.materials[i] = material,
            Err(i) => self.materials.insert(i, material),
        }
    }

    /// Stores a copy of an appearance, replacing any copy with its id.
    pub fn put_appearance(&mut self, appearance: Appearance) {
        match self.appearances.binary_search_by(|a| a.id.cmp(&appearance.id)) {
            Ok(i) => self.appearances[i] = appearance,
            Err(i) => self.appearances.insert(i, appearance),
        }
    }

    /// Drops the copies nothing uses any more. A kept material's default appearance is
    /// kept with it, since a part of that material with no appearance of its own shows
    /// it.
    pub fn retain_used<'a>(
        &mut self,
        materials: impl IntoIterator<Item = &'a MaterialId>,
        appearances: impl IntoIterator<Item = &'a AppearanceId>,
    ) {
        let materials: Vec<&MaterialId> = materials.into_iter().collect();
        self.materials.retain(|m| materials.contains(&&m.id));
        let mut keep: Vec<&AppearanceId> = appearances.into_iter().collect();
        keep.extend(self.materials.iter().filter_map(|m| m.default_appearance.as_ref()));
        self.appearances.retain(|a| keep.contains(&&a.id));
    }

    /// Sorts both lists by id and drops repeated ids, keeping the last copy: what a
    /// reader does with a file written by hand.
    pub fn normalise(&mut self) {
        self.materials.reverse();
        self.materials.sort_by(|a, b| a.id.cmp(&b.id));
        self.materials.dedup_by(|a, b| a.id == b.id);
        self.appearances.reverse();
        self.appearances.sort_by(|a, b| a.id.cmp(&b.id));
        self.appearances.dedup_by(|a, b| a.id == b.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Srgb;

    #[test]
    fn copies_stay_sorted_and_unused_ones_go() {
        let mut s = Snapshots::default();
        let mut steel = Material::new("steel", "Steel");
        steel.default_appearance = Some(AppearanceId("grey".into()));
        s.put_material(steel);
        s.put_material(Material::new("brass", "Brass"));
        s.put_appearance(Appearance::new("red", "Red", Srgb::new(200, 0, 0)));
        s.put_appearance(Appearance::new("grey", "Grey", Srgb::new(128, 128, 128)));
        s.put_appearance(Appearance::new("blue", "Blue", Srgb::new(0, 0, 200)));
        let ids: Vec<&str> = s.materials.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["brass", "steel"]);
        s.put_material(Material::new("brass", "Brass, renamed"));
        assert_eq!(s.materials.len(), 2);
        assert_eq!(s.material(&MaterialId("brass".into())).unwrap().name, "Brass, renamed");

        s.retain_used([&MaterialId("steel".into())], [&AppearanceId("red".into())]);
        let ids: Vec<&str> = s.appearances.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["grey", "red"], "steel's default appearance stays with it");
        assert_eq!(s.materials.len(), 1);
        s.retain_used([], []);
        assert!(s.is_empty());
        assert_eq!(serde_json::to_string(&s).unwrap(), "{}");
    }

    /// The `snapshots` key of a part saved before values recorded their source.
    const SAVED: &str = r##"{"materials":[{"id":"aluminium-6061-t6","name":"Aluminium 6061-T6","category":"Aluminium alloys","source":"MIL-HDBK-5J, table 3.6.2.0(b)","density_kg_m3":2712.0,"youngs_modulus_pa":68260000000.0,"poisson":0.33,"yield_pa":241300000.0,"default_appearance":"aluminium-machined","hatch":"ANSI38"}],"appearances":[{"id":"aluminium-machined","name":"Aluminium, machined","base_color":"#d6d8da","metallic":1.0,"roughness":0.35,"opacity":1.0}]}"##;

    #[test]
    fn a_saved_document_reads_and_writes_as_it_was() {
        use crate::provenance::Property;
        let s: Snapshots = serde_json::from_str(SAVED).unwrap();
        assert_eq!(serde_json::to_string(&s).unwrap(), SAVED, "the same bytes come back");
        let m = &s.materials[0];
        assert_eq!(m.density_kg_m3, Some(2712.0));
        for p in Property::ALL {
            assert!(m.source_of(p).is_unknown(), "{p} reads as unknown, not as the entry's source text");
        }
    }

    #[test]
    fn sources_and_conditions_round_trip_through_a_document() {
        use crate::provenance::{Conditions, Property, PropertySource, Provenance};
        let mut s: Snapshots = serde_json::from_str(SAVED).unwrap();
        let mut m = s.materials[0].clone();
        m.set_value(
            Property::Yield,
            Some(2.413e8),
            Provenance {
                source: PropertySource::Reference { citation: "MIL-HDBK-5J, table 3.6.2.0(b)".into() },
                conditions: Conditions {
                    temperature_k: Some(294.0),
                    treatment: Some("T6".into()),
                    ..Conditions::default()
                },
            },
        );
        m.set_override(Property::Density, Some(2700.0), None);
        s.put_material(m);
        let json = serde_json::to_string(&s).unwrap();
        let back: Snapshots = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
        let m = &back.materials[0];
        assert_eq!(m.provenance_of(Property::Yield).conditions.treatment.as_deref(), Some("T6"));
        assert_eq!(m.source_of(Property::Density), &PropertySource::UserOverride { note: None });
        assert!(m.source_of(Property::Poisson).is_unknown());
    }

    #[test]
    fn hand_written_repeats_keep_the_last() {
        let mut s = Snapshots {
            materials: vec![Material::new("b", "First"), Material::new("a", "A"), Material::new("b", "Second")],
            appearances: Vec::new(),
        };
        s.normalise();
        let names: Vec<&str> = s.materials.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["A", "Second"]);
    }
}
