//! `assembly.json`: what the user authored in an assembly.
//!
//! An assembly holds:
//!
//! - **sources**: the documents it places (parts and sub-assemblies), each a [`DocRef`]
//!   pinned at the revision the assembly was last rebuilt against, under a [`SourceId`];
//! - **components**: placements of sources, each with a stable [`ComponentId`], an
//!   optional name, a rigid [`Placement`] and its hidden and suppressed flags;
//! - **patterns**: generators that repeat seed components (linear in one or two
//!   directions, or circular), stored as the rule, never as the expanded copies;
//! - **overrides**: sparse changes to occurrences below the assembly's own components,
//!   addressed by [`OccurrencePath`]: hide one part of a sub-assembly, suppress one
//!   bolt of a pattern, move one member;
//! - **appearances**: assembly-level colours on occurrences. In the assembly, an
//!   occurrence with a colour shows every face of every part under it in that colour,
//!   replacing the parts' own colours; the part files keep theirs. The
//!   nearest occurrence with a colour wins. The assembly keeps a copy of every
//!   appearance it uses in `snapshots`, as parts do;
//! - **mates**: authored relationships that decide placements ([`crate::mates`]), and the
//!   typed **values** that can drive them. A mate of a family this version does not know
//!   is kept as read and never counted as solved;
//! - reserved lists for configurations and display states, kept verbatim until those
//!   formats are defined.
//!
//! Beside the truth, an assembly carries its display settings ([`Assembly::settings`]):
//! a quality for every part, and components shown at their own quality. They are kept
//! in the file's `settings.json`, outside both hashes, so changing them never gives the
//! assembly a new revision.
//!
//! Units are millimetres and degrees.

use std::collections::{HashMap, HashSet};

use glam::{DQuat, DVec3};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use soliddraft_document::{ConfigScope, ConfigurationId, DocKind, DocRef};
use soliddraft_materials::{Appearance, AppearanceId, Snapshots, scope};
use soliddraft_model::{Expression, ValueTable};
use soliddraft_tessellation::Settings;

use crate::mates::{Mate, MateEntry};
use crate::path::{ComponentId, OccurrencePath, PatternId, SourceId, Step};
use crate::placement::Placement;

/// The format tag this version writes.
pub const FORMAT: &str = "soliddraft-assembly/1";

/// An assembly's truth.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assembly {
    /// The format tag, [`FORMAT`].
    pub format: String,
    /// The assembly's name, a label.
    pub name: String,
    /// The units of every length and angle in the file.
    pub units: Units,
    /// The documents the assembly places, by id order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<Source>,
    /// The components, by id order: the tree's order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<Component>,
    /// The patterns, by id order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub patterns: Vec<Pattern>,
    /// Changes to occurrences below the assembly's own components, by path order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<Override>,
    /// Assembly-level colours on occurrences, by path order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub appearances: Vec<OccurrenceAppearance>,
    /// Copies of the appearances `appearances` uses, as parts keep theirs.
    #[serde(default, skip_serializing_if = "Snapshots::is_empty")]
    pub snapshots: Snapshots,
    /// Reserved for configurations (named override sets). Kept as read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub configurations: Vec<Value>,
    /// Reserved for display states (visibility, colours, exploded poses). Kept as read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub display_states: Vec<Value>,
    /// The mates: known ones by id, then those of a later version as read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mates: Vec<MateEntry>,
    /// Typed values that can drive mates.
    #[serde(default, skip_serializing_if = "ValueTable::is_empty")]
    pub values: ValueTable,
    /// The next free id for a component, pattern, source, mate or value. Ids are never
    /// reused.
    pub next_id: u32,
    /// Display settings, from `settings.json`: not part of the truth or its hashes.
    /// In an assembly, a viewport quality other than auto shows every part at it
    /// instead of at its own, and each override shows one component and everything
    /// under it at its own quality; the nearest override wins. Only the open
    /// assembly's settings apply: a sub-assembly's are read when it is opened on its
    /// own.
    #[serde(skip)]
    pub settings: Settings,
}

/// The units of an assembly file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Units {
    /// Millimetres and degrees.
    #[default]
    Mm,
}

/// A document the assembly places.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    /// The source's id within the assembly.
    pub id: SourceId,
    /// The part or sub-assembly, pinned at a revision. Its `configuration` picks the
    /// configuration placed (reserved: always the default for now).
    pub document: DocRef,
}

/// One placement of a source.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Component {
    /// The component's id within the assembly.
    pub id: ComponentId,
    /// A name the user gave it. Without one the tree shows the source's name and the
    /// id, like "Bracket<12>".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// What is placed.
    pub source: SourceId,
    /// Where, relative to the assembly's origin.
    #[serde(default, skip_serializing_if = "Placement::is_identity")]
    pub placement: Placement,
    /// Not drawn. Hidden components still count for mass and bills of materials.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden: bool,
    /// Left out: not drawn, not loaded, not counted.
    #[serde(default, skip_serializing_if = "is_false")]
    pub suppressed: bool,
}

impl Component {
    /// A visible, unsuppressed, unnamed component.
    pub fn new(id: ComponentId, source: SourceId, placement: Placement) -> Component {
        Component { id, name: None, source, placement, hidden: false, suppressed: false }
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// A pattern: copies of seed components by a rule.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pattern {
    /// The pattern's id within the assembly.
    pub id: PatternId,
    /// A name the user gave it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The components repeated. Each member repeats every seed.
    pub seeds: Vec<ComponentId>,
    /// The rule.
    pub rule: PatternRule,
}

/// How a pattern places its members. Member 0 is the seed itself; the rule generates
/// members 1 and up.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub enum PatternRule {
    /// Copies along one direction, and optionally a second.
    Linear {
        /// The first direction.
        direction: [f64; 3],
        /// The distance between neighbours along it, in millimetres.
        spacing: f64,
        /// How many along it, the seed included.
        count: u32,
        /// The second direction, if any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        second: Option<Direction>,
    },
    /// Copies around an axis.
    Circular {
        /// A point on the axis.
        origin: [f64; 3],
        /// The axis direction; members turn about it right-handed.
        axis: [f64; 3],
        /// The angle between neighbours, in degrees.
        angle: f64,
        /// How many, the seed included.
        count: u32,
    },
}

/// The second direction of a linear pattern.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Direction {
    /// The direction.
    pub direction: [f64; 3],
    /// The distance between neighbours along it, in millimetres.
    pub spacing: f64,
    /// How many along it, the first row included.
    pub count: u32,
}

impl PatternRule {
    /// The number of members, the seed included.
    pub fn len(&self) -> u32 {
        match self {
            PatternRule::Linear { count, second, .. } => {
                count.saturating_mul(second.as_ref().map(|s| s.count).unwrap_or(1))
            }
            PatternRule::Circular { count, .. } => *count,
        }
    }

    /// True for a pattern with no members at all (invalid).
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The placement that takes the seed to member `index`, in the assembly's frame.
    pub fn member(&self, index: u32) -> Placement {
        match self {
            PatternRule::Linear { direction, spacing, count, second } => {
                let (i, j) = (index % count.max(&1), index / count.max(&1));
                let mut t = unit(direction) * (*spacing * i as f64);
                if let Some(s) = second {
                    t += unit(&s.direction) * (s.spacing * j as f64);
                }
                Placement::translation(t)
            }
            PatternRule::Circular { origin, axis, angle, .. } => {
                let o = DVec3::from_array(*origin);
                let r = DQuat::from_axis_angle(unit(axis), (angle * index as f64).to_radians());
                // About the axis through `origin`: move it to the origin, turn, move back.
                Placement::new(r, o - r * o)
            }
        }
    }

    fn check(&self) -> Result<(), String> {
        let dir_ok = |d: &[f64; 3]| d.iter().all(|v| v.is_finite()) && DVec3::from_array(*d).length() > 1e-12;
        match self {
            PatternRule::Linear { direction, spacing, count, second } => {
                if !dir_ok(direction) || !spacing.is_finite() || *count == 0 {
                    return Err("a linear pattern needs a direction, a finite spacing and a count".into());
                }
                if let Some(s) = second
                    && (!dir_ok(&s.direction) || !s.spacing.is_finite() || s.count == 0)
                {
                    return Err("a linear pattern's second direction needs a direction, spacing and count".into());
                }
            }
            PatternRule::Circular { origin, axis, angle, count } => {
                if !dir_ok(axis) || !origin.iter().all(|v| v.is_finite()) || !angle.is_finite() || *count == 0 {
                    return Err("a circular pattern needs an axis, a finite angle and a count".into());
                }
            }
        }
        if self.len() as u64 > MAX_PATTERN_MEMBERS {
            return Err(format!("a pattern may have at most {MAX_PATTERN_MEMBERS} members"));
        }
        Ok(())
    }
}

/// The most members one pattern may generate.
pub const MAX_PATTERN_MEMBERS: u64 = 1 << 24;

fn unit(d: &[f64; 3]) -> DVec3 {
    DVec3::from_array(*d).normalize_or_zero()
}

/// A change to one occurrence below the assembly's own components, or to one pattern
/// member. Unset fields leave the occurrence as its own assembly has it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Override {
    /// The occurrence, from this assembly.
    pub path: OccurrencePath,
    /// Hide or show it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    /// Suppress or unsuppress it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suppressed: Option<bool>,
    /// Place it here instead, relative to the assembly that contains it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<Placement>,
}

impl Override {
    /// An override that changes nothing yet.
    pub fn at(path: OccurrencePath) -> Override {
        Override { path, hidden: None, suppressed: None, placement: None }
    }

    /// True when it changes nothing.
    pub fn is_empty(&self) -> bool {
        self.hidden.is_none() && self.suppressed.is_none() && self.placement.is_none()
    }
}

/// An assembly-level colour: every face of every part under the occurrence at `path`
/// shows `appearance`, replacing the parts' own colours, in the configurations `scope`
/// names.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OccurrenceAppearance {
    /// The occurrence: one of the assembly's components or anything below it.
    pub path: OccurrencePath,
    /// The appearance; the assembly holds its copy.
    pub appearance: AppearanceId,
    /// Which configurations of this assembly it holds in.
    #[serde(flatten)]
    pub scope: ConfigScope,
}

/// Why an assembly's truth is not consistent.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("the assembly is inconsistent: {0}")]
pub struct Invalid(pub String);

impl Assembly {
    /// An empty assembly.
    pub fn new(name: impl Into<String>) -> Assembly {
        Assembly {
            format: FORMAT.to_owned(),
            name: name.into(),
            units: Units::Mm,
            sources: Vec::new(),
            components: Vec::new(),
            patterns: Vec::new(),
            overrides: Vec::new(),
            appearances: Vec::new(),
            snapshots: Snapshots::default(),
            configurations: Vec::new(),
            display_states: Vec::new(),
            mates: Vec::new(),
            values: ValueTable::default(),
            next_id: 1,
            settings: Settings::default(),
        }
    }

    /// Takes the next free id.
    pub fn take_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Adds a source for `document` and returns its id, or the id of the source that
    /// already names the same document and configuration.
    pub fn add_source(&mut self, document: DocRef) -> SourceId {
        if let Some(s) = self.sources.iter().find(|s| {
            s.document.id.is_some()
                && s.document.id == document.id
                && s.document.configuration == document.configuration
        }) {
            return s.id;
        }
        let id = SourceId(self.take_id());
        self.sources.push(Source { id, document });
        id
    }

    /// Adds a component and returns its id.
    pub fn add_component(&mut self, source: SourceId, placement: Placement) -> ComponentId {
        let id = ComponentId(self.take_id());
        self.components.push(Component::new(id, source, placement));
        id
    }

    /// Adds a pattern and returns its id.
    pub fn add_pattern(&mut self, seeds: Vec<ComponentId>, rule: PatternRule) -> PatternId {
        let id = PatternId(self.take_id());
        self.patterns.push(Pattern { id, name: None, seeds, rule });
        id
    }

    /// The source with id `id`.
    pub fn source(&self, id: SourceId) -> Option<&Source> {
        self.sources.binary_search_by_key(&id, |s| s.id).ok().map(|i| &self.sources[i])
    }

    /// The component with id `id`.
    pub fn component(&self, id: ComponentId) -> Option<&Component> {
        self.components.binary_search_by_key(&id, |c| c.id).ok().map(|i| &self.components[i])
    }

    /// The component with id `id`, to change.
    pub fn component_mut(&mut self, id: ComponentId) -> Option<&mut Component> {
        self.components.binary_search_by_key(&id, |c| c.id).ok().map(|i| &mut self.components[i])
    }

    /// The pattern with id `id`.
    pub fn pattern(&self, id: PatternId) -> Option<&Pattern> {
        self.patterns.binary_search_by_key(&id, |p| p.id).ok().map(|i| &self.patterns[i])
    }

    /// The override at `path`, if any.
    pub fn override_at(&self, path: &OccurrencePath) -> Option<&Override> {
        self.overrides.binary_search_by(|o| o.path.cmp(path)).ok().map(|i| &self.overrides[i])
    }

    /// The override at `path`, made if missing.
    pub fn override_mut(&mut self, path: &OccurrencePath) -> &mut Override {
        let i = match self.overrides.binary_search_by(|o| o.path.cmp(path)) {
            Ok(i) => i,
            Err(i) => {
                self.overrides.insert(i, Override::at(path.clone()));
                i
            }
        };
        &mut self.overrides[i]
    }

    /// Drops overrides that change nothing.
    pub fn prune_overrides(&mut self) {
        self.overrides.retain(|o| !o.is_empty());
    }

    /// Puts every list in its canonical order, so the same assembly always writes the
    /// same bytes.
    pub fn normalize(&mut self) {
        self.sources.sort_by_key(|s| s.id);
        self.components.sort_by_key(|c| c.id);
        self.patterns.sort_by_key(|p| p.id);
        self.overrides.sort_by(|a, b| a.path.cmp(&b.path));
        self.appearances.sort_by(|a, b| a.path.cmp(&b.path));
        self.mates.sort_by_key(MateEntry::order);
        self.snapshots.normalise();
    }

    /// The mates this version understands.
    pub fn known_mates(&self) -> impl Iterator<Item = &Mate> {
        self.mates.iter().filter_map(MateEntry::known)
    }

    /// The appearance the occurrence at `path` is given here, in `configuration`
    /// (`None` for the default): an assignment for named
    /// configurations wins over one for all of them, and of equals the later one.
    pub fn colour_at(&self, path: &OccurrencePath, configuration: Option<&ConfigurationId>) -> Option<&Appearance> {
        let a = scope::pick(self.appearances.iter().filter(|c| c.path == *path), |c| &c.scope, configuration)?;
        self.appearance(&a.appearance)
    }

    /// The copy of an appearance.
    pub fn appearance(&self, id: &AppearanceId) -> Option<&Appearance> {
        self.snapshots.appearance(id)
    }

    /// Gives the occurrence at `path` a colour for every configuration, or takes its
    /// colour away. Keeps a copy of the appearance and drops copies nothing uses.
    pub fn set_colour(&mut self, path: &OccurrencePath, appearance: Option<Appearance>) {
        self.appearances.retain(|c| !(c.path == *path && c.scope == ConfigScope::All));
        if let Some(a) = appearance {
            let id = a.id.clone();
            self.snapshots.put_appearance(a);
            let at = self.appearances.partition_point(|c| c.path <= *path);
            self.appearances
                .insert(at, OccurrenceAppearance { path: path.clone(), appearance: id, scope: ConfigScope::All });
        }
        let used: Vec<AppearanceId> = self.appearances.iter().map(|c| c.appearance.clone()).collect();
        self.snapshots.retain_used(std::iter::empty(), used.iter());
    }

    /// Checks what the schema cannot: ids unique and below `next_id`, every reference
    /// between the lists pointing at something, pattern rules sound, and every
    /// override's first step naming a component or pattern member of this assembly.
    /// Lists must be in canonical order ([`Assembly::normalize`]).
    pub fn validate(&self) -> Result<(), Invalid> {
        let bad = |m: String| Err(Invalid(m));
        let mut ids = HashSet::new();
        let all = self
            .sources
            .iter()
            .map(|s| s.id.0)
            .chain(self.components.iter().map(|c| c.id.0))
            .chain(self.patterns.iter().map(|p| p.id.0))
            .chain(self.known_mates().map(|m| m.id().0))
            .chain(self.values.ids());
        for id in all {
            if !ids.insert(id) {
                return bad(format!("id {id} is used twice"));
            }
            if id >= self.next_id {
                return bad(format!("id {id} is not below next_id {}", self.next_id));
            }
        }
        if !self.sources.is_sorted_by_key(|s| s.id)
            || !self.components.is_sorted_by_key(|c| c.id)
            || !self.patterns.is_sorted_by_key(|p| p.id)
            || !self.overrides.is_sorted_by(|a, b| a.path < b.path)
        {
            return bad("lists are not in id order, or two overrides share a path".into());
        }
        for s in &self.sources {
            if !matches!(s.document.kind, DocKind::Part | DocKind::Assembly) {
                return bad(format!(
                    "source {} is a {}; an assembly places parts and assemblies",
                    s.id, s.document.kind
                ));
            }
            if !s.document.is_bound() {
                return bad(format!("source {} has no document id", s.id));
            }
        }
        for c in &self.components {
            if self.source(c.source).is_none() {
                return bad(format!("component {} places source {}, which does not exist", c.id, c.source));
            }
        }
        let mut seeded: HashMap<PatternId, &Pattern> = HashMap::new();
        for p in &self.patterns {
            p.rule.check().map_err(|m| Invalid(format!("pattern {}: {m}", p.id)))?;
            if p.seeds.is_empty() {
                return bad(format!("pattern {} has no seeds", p.id));
            }
            if let Some(s) = p.seeds.iter().find(|s| self.component(**s).is_none()) {
                return bad(format!("pattern {} repeats component {s}, which does not exist", p.id));
            }
            seeded.insert(p.id, p);
        }
        if !self.appearances.is_sorted_by(|a, b| a.path <= b.path)
            || !self.snapshots.appearances.is_sorted_by(|a, b| a.id < b.id)
        {
            return bad("appearances are not in path order, or two appearance copies share an id".into());
        }
        for c in &self.appearances {
            if self.appearance(&c.appearance).is_none() {
                return bad(format!(
                    "the colour at {} uses appearance {}, which the assembly has no copy of",
                    c.path, c.appearance
                ));
            }
            match c.path.steps().first() {
                Some(first) if self.component(first.component).is_some() => {}
                _ => return bad(format!("the colour at {} is not on an occurrence of this assembly", c.path)),
            }
        }
        self.validate_mates()?;
        for o in &self.overrides {
            let Some(first) = o.path.steps().first() else {
                return bad("an override has an empty path".into());
            };
            if self.component(first.component).is_none() {
                return bad(format!(
                    "override {} starts at component {}, which does not exist",
                    o.path, first.component
                ));
            }
            match first.member {
                None if o.path.steps().len() == 1 => {
                    return bad(format!(
                        "override {} names a component of this assembly; change the component",
                        o.path
                    ));
                }
                None => {}
                Some((pid, index)) => {
                    let ok =
                        seeded.get(&pid).is_some_and(|p| p.seeds.contains(&first.component) && index < p.rule.len());
                    if !ok {
                        return bad(format!("override {} names a pattern member that does not exist", o.path));
                    }
                }
            }
        }
        Ok(())
    }

    /// Checks every mate names components and sources that exist, and that its
    /// expressions parse back to what is stored and come to lengths.
    fn validate_mates(&self) -> Result<(), Invalid> {
        let bad = |m: String| Err(Invalid(m));
        if !self.mates.is_sorted_by_key(MateEntry::order) {
            return bad("mates are not in id order".into());
        }
        let mut names = HashSet::new();
        for mate in self.known_mates() {
            if mate.name().trim().is_empty() || !names.insert(mate.name()) {
                return bad(format!("mate {} has no name of its own", mate.id()));
            }
            for c in mate.components() {
                if self.component(c).is_none() {
                    return bad(format!("mate {} ties component {c}, which does not exist", mate.name()));
                }
            }
            if let Mate::Distance { a, b, distance, .. } = mate {
                for end in [a, b] {
                    let placed = end.holder().and_then(|c| self.component(c)).and_then(|c| self.source(c.source));
                    // A face inside a sub-assembly is checked against the sub-assembly when it
                    // is solved; this file holds only its first step.
                    let nested = end.path.steps().len() > 1;
                    let ok = match placed {
                        Some(s) if nested => s.document.kind == DocKind::Assembly,
                        Some(s) => s.document.id == Some(end.document),
                        None => false,
                    };
                    if !ok {
                        return bad(format!("mate {} names a face of another document", mate.name()));
                    }
                }
                self.values.length(distance).map_err(|e| Invalid(format!("mate {}: {e}", mate.name())))?;
            }
        }
        let expressions: Vec<&Expression> = self.known_mates().filter_map(Mate::expression).collect();
        self.values.validate(expressions.into_iter()).map_err(Invalid)
    }

    /// The number of occurrences this assembly places directly: its components and the
    /// members its patterns generate, suppressed ones included.
    pub fn direct_occurrences(&self) -> u64 {
        self.components.len() as u64
            + self.patterns.iter().map(|p| p.seeds.len() as u64 * (p.rule.len() as u64).saturating_sub(1)).sum::<u64>()
    }

    /// Every step one level down: each component, then each generated pattern member,
    /// with its placement in this assembly and whether it is suppressed (overrides of
    /// pattern members applied).
    pub fn direct_steps(&self) -> impl Iterator<Item = (Step, Placement, SourceId)> + '_ {
        let components = self.components.iter().map(|c| (Step::component(c.id), c.placement, c.source));
        let members = self.patterns.iter().flat_map(move |p| {
            p.seeds.iter().filter_map(move |s| self.component(*s)).flat_map(move |seed| {
                (1..p.rule.len()).map(move |i| {
                    let step = Step::member(seed.id, p.id, i);
                    (step, p.rule.member(i).compose(&seed.placement), seed.source)
                })
            })
        });
        components.chain(members)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use soliddraft_document::{ContentHash, DocumentId};

    fn part() -> DocRef {
        DocRef::new(DocumentId::new(), DocKind::Part, "bolt.dwprt", ContentHash::of_bytes(b"bolt"))
    }

    fn sample() -> Assembly {
        let mut a = Assembly::new("Frame");
        let bolt = a.add_source(part());
        let c = a.add_component(bolt, Placement::translation(DVec3::new(10.0, 0.0, 0.0)));
        a.add_pattern(
            vec![c],
            PatternRule::Linear { direction: [1.0, 0.0, 0.0], spacing: 20.0, count: 4, second: None },
        );
        a
    }

    #[test]
    fn assemblies_round_trip_through_json() {
        let a = sample();
        a.validate().unwrap();
        let text = serde_json::to_string_pretty(&a).unwrap();
        let back: Assembly = serde_json::from_str(&text).unwrap();
        assert_eq!(back, a);
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["patterns"][0]["rule"]["kind"], "linear");
        assert!(v.get("mates").is_none() && v.get("overrides").is_none());
        assert_eq!(a.sources.len(), 1);
    }

    #[test]
    fn sources_are_shared_by_document() {
        let mut a = Assembly::new("A");
        let r = part();
        let s1 = a.add_source(r.clone());
        assert_eq!(a.add_source(r), s1, "the same document and configuration, one source");
        assert_ne!(a.add_source(part()), s1);
    }

    #[test]
    fn reserved_lists_are_kept() {
        let mut v = serde_json::to_value(sample()).unwrap();
        v["mates"] = json!([{"kind": "coincident", "future": true}]);
        v["configurations"] = json!([{"id": "long"}]);
        let a: Assembly = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(serde_json::to_value(&a).unwrap(), v);
    }

    #[test]
    fn patterns_generate_members() {
        let lin = PatternRule::Linear {
            direction: [2.0, 0.0, 0.0],
            spacing: 5.0,
            count: 3,
            second: Some(Direction { direction: [0.0, 1.0, 0.0], spacing: 7.0, count: 2 }),
        };
        assert_eq!(lin.len(), 6);
        assert_eq!(lin.member(4).translation, DVec3::new(5.0, 7.0, 0.0));
        let circ = PatternRule::Circular { origin: [10.0, 0.0, 0.0], axis: [0.0, 0.0, 1.0], angle: 90.0, count: 4 };
        let p = circ.member(1).apply(DVec3::new(20.0, 0.0, 0.0));
        assert!((p - DVec3::new(10.0, 10.0, 0.0)).length() < 1e-9, "{p}");
        let a = sample();
        assert_eq!(a.direct_occurrences(), 4);
        let steps: Vec<_> = a.direct_steps().collect();
        assert_eq!(steps.len(), 4);
        assert_eq!(steps[3].0.to_string(), "2:3.3");
        assert_eq!(steps[3].1.translation, DVec3::new(70.0, 0.0, 0.0));
    }

    #[test]
    fn validation_catches_dangling_ids_and_bad_overrides() {
        let mut a = sample();
        a.components[0].source = SourceId(99);
        assert!(a.validate().unwrap_err().0.contains("source 99"));

        let mut a = sample();
        a.next_id = 2;
        assert!(a.validate().unwrap_err().0.contains("next_id"));

        let mut a = sample();
        a.override_mut(&"2".parse().unwrap()).hidden = Some(true);
        assert!(a.validate().unwrap_err().0.contains("change the component"));

        let mut a = sample();
        a.override_mut(&"2:3.3".parse().unwrap()).suppressed = Some(true);
        a.override_mut(&"2/7".parse().unwrap()).hidden = Some(true);
        a.validate().unwrap();
        a.override_mut(&"2:3.4".parse().unwrap()).suppressed = Some(true);
        assert!(a.validate().unwrap_err().0.contains("pattern member"));

        let mut a = sample();
        a.sources[0].document.kind = DocKind::Drawing;
        assert!(a.validate().is_err());

        let mut a = sample();
        a.patterns[0].rule = PatternRule::Linear { direction: [0.0; 3], spacing: 1.0, count: 2, second: None };
        assert!(a.validate().unwrap_err().0.contains("direction"));
    }

    #[test]
    fn colours_keep_their_appearances() {
        use soliddraft_materials::Srgb;
        let mut a = sample();
        let p: OccurrencePath = "2".parse().unwrap();
        let blue = Appearance::new("blue", "Blue", Srgb::new(30, 80, 200));
        a.set_colour(&p, Some(blue.clone()));
        a.validate().unwrap();
        assert_eq!(a.colour_at(&p, None), Some(&blue));
        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(v["appearances"][0], json!({"path": "2", "appearance": "blue", "scope": "all"}));
        assert_eq!(v["snapshots"]["appearances"][0]["id"], "blue");
        assert_eq!(serde_json::from_value::<Assembly>(v).unwrap(), a);
        a.set_colour(&p, None);
        assert!(a.appearances.is_empty() && a.snapshots.is_empty(), "an unused copy goes");
        a.appearances.push(OccurrenceAppearance {
            path: p,
            appearance: AppearanceId("gone".into()),
            scope: ConfigScope::All,
        });
        assert!(a.validate().is_err());
    }

    #[test]
    fn empty_overrides_are_pruned() {
        let mut a = sample();
        let p: OccurrencePath = "2/5".parse().unwrap();
        a.override_mut(&p);
        assert!(a.override_at(&p).is_some());
        a.prune_overrides();
        assert!(a.override_at(&p).is_none());
    }
}
