//! What the user chose to show or hide in a part: features, reference planes and origin
//! axes, by stable identity.
//!
//! The choices are display data, not part of the model. A part file keeps them in
//! `view.json`, an optional section outside both hashes, so hiding a feature never gives
//! the part a new revision and an undo never takes a choice back. A choice outlives its
//! feature: an id an undo removed keeps it, so the redo that brings the feature back
//! brings it back hidden. An item with no choice follows the application's defaults.
//!
//! A reader that cannot read the section opens the part with no choices, as a file
//! without one does.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::feature::{DatumPlane, FeatureId};

/// The entry a part keeps its view choices in.
pub const ENTRY: &str = "view.json";
/// The format tag of the view choices this version writes.
pub const FORMAT: &str = "soliddraft-view/1";

/// The three origin axes. Every part has them, so only a view choice ever names one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatumAxis {
    /// The X axis.
    X,
    /// The Y axis.
    Y,
    /// The Z axis.
    Z,
}

/// Something the user can show or hide.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewItem {
    /// A feature, by its id.
    Feature(FeatureId),
    /// A reference plane.
    Plane(DatumPlane),
    /// An origin axis.
    Axis(DatumAxis),
}

/// The part's explicit show and hide choices.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Visibility {
    choices: BTreeMap<ViewItem, bool>,
}

/// `view.json`: the items shown and hidden, each list in item order.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    format: String,
    #[serde(default)]
    shown: Vec<ViewItem>,
    #[serde(default)]
    hidden: Vec<ViewItem>,
}

impl Visibility {
    /// The choice for `item`: shown, hidden, or `None` to follow the defaults.
    pub fn choice(&self, item: ViewItem) -> Option<bool> {
        self.choices.get(&item).copied()
    }

    /// Records a choice for `item`, or forgets it with `None`.
    pub fn set(&mut self, item: ViewItem, shown: Option<bool>) {
        match shown {
            Some(shown) => self.choices.insert(item, shown),
            None => self.choices.remove(&item),
        };
    }

    /// True when nothing was chosen, so a file needs no `view.json`.
    pub fn is_empty(&self) -> bool {
        self.choices.is_empty()
    }

    /// The entry's text: pretty JSON with a final newline, the same bytes for the same
    /// choices.
    pub fn to_json(&self) -> String {
        let pick = |shown: bool| self.choices.iter().filter(|(_, s)| **s == shown).map(|(item, _)| *item).collect();
        let entry = Entry { format: FORMAT.into(), shown: pick(true), hidden: pick(false) };
        serde_json::to_string_pretty(&entry).expect("view choices always serialise") + "\n"
    }

    /// Reads the entry's text. Another format, an unknown key or an item both shown and
    /// hidden is refused rather than misread.
    pub fn from_json(text: &str) -> Result<Visibility, String> {
        let entry: Entry = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if entry.format != FORMAT {
            return Err(format!("the view choices are {:?}; this version reads {FORMAT}", entry.format));
        }
        let mut view = Visibility::default();
        for (items, shown) in [(entry.shown, true), (entry.hidden, false)] {
            for item in items {
                if view.choices.insert(item, shown).is_some() {
                    return Err(format!("{item:?} is listed twice"));
                }
            }
        }
        Ok(view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_round_trip_and_unreadable_entries_are_refused() {
        let mut view = Visibility::default();
        assert!(view.is_empty());
        view.set(ViewItem::Feature(FeatureId(3)), Some(false));
        view.set(ViewItem::Axis(DatumAxis::Y), Some(true));
        view.set(ViewItem::Plane(DatumPlane::Top), Some(false));
        let text = view.to_json();
        assert_eq!(Visibility::from_json(&text).unwrap(), view);
        assert!(text.contains("\"feature\": 3") && text.contains("\"axis\": \"y\""), "{text}");
        view.set(ViewItem::Axis(DatumAxis::Y), None);
        assert_eq!(view.choice(ViewItem::Axis(DatumAxis::Y)), None);

        assert!(Visibility::from_json(&text.replace(FORMAT, "soliddraft-view/2")).is_err());
        assert!(
            Visibility::from_json(r#"{"format":"soliddraft-view/1","shown":[{"axis":"x"}],"hidden":[{"axis":"x"}]}"#)
                .is_err()
        );
        assert!(Visibility::from_json(r#"{"format":"soliddraft-view/1","colour":[]}"#).is_err());
    }
}
