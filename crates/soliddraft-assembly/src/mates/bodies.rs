//! The part occurrences a solve moves, wherever they sit in the tree.
//!
//! The root's mates name its own parts, or parts inside its sub-assemblies by path, and
//! each sub-assembly brings its own mates, read with its path in front. A sub-assembly's
//! frame stays where it is authored: a solve moves the part occurrences inside it, and the
//! root keeps what it moves as overrides, never touching the sub-assembly's own file.

use std::collections::BTreeMap;

use soliddraft_document::resolve::{Location, Resolution, Resolver};
use soliddraft_document::{DocKind, DocRef};
use soliddraft_model::FaceRef;

use crate::document::AssemblyDocument;
use crate::mates::reference::check_endpoint;
use crate::mates::solver::Body;
use crate::mates::{FaceEndpoint, Mate, MateEntry, MateRefusal, RefusalCode};
use crate::occurrence::{Flags, OccurrenceIndex, OccurrenceTable};
use crate::path::{OccurrencePath, Step};
use crate::placement::Placement;
use crate::schema::Assembly;
use crate::store::DefinitionStore;

/// An assembly's truth over the structure it was flattened into: `truth` decides the
/// root's components and overrides, and each sub-assembly's own truth the rest.
pub(crate) struct Scope<'a> {
    pub truth: &'a Assembly,
    pub table: &'a OccurrenceTable,
    pub store: &'a DefinitionStore,
}

/// One face a mate reads.
pub(crate) struct End {
    pub body: Body,
    /// The part, pinned where its containing assembly pins it.
    pub source: DocRef,
    /// The sub-assemblies on the way to it, outermost first: each holds the next one's
    /// reference, and the last holds `source`.
    pub chain: Vec<DocRef>,
    pub face: FaceRef,
}

/// A mate, read from the assembly that holds it and placed in the root.
#[allow(clippy::large_enum_variant, reason = "a mate list is short and read one by one")]
pub(crate) enum Gathered {
    Origin { mate: String, body: Body, axes: bool, at: Placement },
    Distance { mate: String, ends: [End; 2], sign: i8, distance: f64 },
}

/// What a solve works on.
pub(crate) struct Problem {
    /// The mated parts at their placements in the root's frame.
    pub bodies: BTreeMap<Body, Placement>,
    pub mates: Vec<Gathered>,
    /// Every part occurrence that is not suppressed: what has freedoms to count.
    pub parts: usize,
}

fn invalid(message: String) -> MateRefusal {
    MateRefusal::invalid(message)
}

impl<'a> Scope<'a> {
    pub fn new(truth: &'a Assembly, table: &'a OccurrenceTable, store: &'a DefinitionStore) -> Scope<'a> {
        Scope { truth, table, store }
    }

    /// The truth of the assembly the first `depth` steps of `path` lead to: the root's for
    /// none, otherwise the sub-assembly's own.
    fn assembly_at(&self, path: &OccurrencePath, depth: usize) -> Option<&'a Assembly> {
        if depth == 0 {
            return Some(self.truth);
        }
        let row = self.table.find(&OccurrencePath(path.steps()[..depth].to_vec()))?;
        self.store.get(self.table.definition(row)).assembly.as_deref()
    }

    /// Where the occurrence at `path` sits in its containing assembly, as that assembly
    /// and those inside the root have it: the root's own overrides are left out.
    pub fn base(&self, path: &OccurrencePath) -> Option<Placement> {
        let steps = path.steps();
        let (last, depth) = (steps.last()?, steps.len() - 1);
        // An outer assembly's override wins over an inner one's.
        for k in 1..depth {
            let rest = OccurrencePath(steps[k..].to_vec());
            if let Some(p) = self.assembly_at(path, k)?.override_at(&rest).and_then(|o| o.placement) {
                return Some(p);
            }
        }
        let holder = self.assembly_at(path, depth)?;
        match last.member {
            None => holder.component(last.component).map(|c| c.placement),
            Some(_) => None,
        }
    }

    /// Where the occurrence at `path` sits in its containing assembly, the root's
    /// overrides included.
    pub fn local(&self, path: &OccurrencePath) -> Option<Placement> {
        match path.steps() {
            [] => Some(Placement::IDENTITY),
            [step] if step.member.is_none() => self.truth.component(step.component).map(|c| c.placement),
            _ => match self.truth.override_at(path).and_then(|o| o.placement) {
                Some(p) => Some(p),
                None => self.base(path),
            },
        }
    }

    /// The frame of the assembly that holds the occurrence at `path`, in the root's.
    pub fn frame(&self, path: &OccurrencePath) -> Option<Placement> {
        let steps = path.steps();
        let mut at = Placement::IDENTITY;
        for k in 1..steps.len() {
            at = at.compose(&self.local(&OccurrencePath(steps[..k].to_vec()))?);
        }
        Some(at)
    }

    /// The occurrence at `path` in the root's frame.
    pub fn world(&self, path: &OccurrencePath) -> Option<Placement> {
        Some(self.frame(path)?.compose(&self.local(path)?))
    }

    /// The references of the sub-assemblies on the way to `path`, outermost first.
    fn chain(&self, path: &OccurrencePath) -> Vec<DocRef> {
        let steps = path.steps();
        (1..steps.len())
            .filter_map(|k| {
                let outer = self.assembly_at(path, k - 1)?;
                let c = outer.component(steps[k - 1].component)?;
                Some(outer.source(c.source)?.document.clone())
            })
            .collect()
    }

    /// The reference of the part placed at `path`, as the assembly holding it has it.
    pub fn part_source(&self, path: &OccurrencePath) -> Result<DocRef, MateRefusal> {
        let steps = path.steps();
        let Some(last) = steps.last().filter(|_| steps.iter().all(|s| s.member.is_none())) else {
            return Err(invalid(format!("mates reach components, not pattern members: {path}")));
        };
        let holder = self
            .assembly_at(path, steps.len() - 1)
            .ok_or_else(|| invalid(format!("the assembly holding {path} is not loaded")))?;
        let c = holder.component(last.component).ok_or_else(|| invalid(format!("no occurrence at {path}")))?;
        let source = holder.source(c.source).ok_or_else(|| invalid(format!("occurrence {path} has no source")))?;
        if let Some(configuration) = &source.document.configuration {
            let message = format!(
                "occurrence {path} names configuration {}; this version mates default configurations only",
                configuration.0
            );
            return Err(MateRefusal::new(RefusalCode::UnsupportedConfiguration, message));
        }
        if source.document.kind != DocKind::Part {
            return Err(invalid(format!("{path} is a sub-assembly; mates reach the parts inside it")));
        }
        Ok(source.document.clone())
    }

    /// Every assembly whose mates the root solves: the root, then each sub-assembly
    /// occurrence that is not suppressed, with its path.
    fn holders(&self) -> Vec<(OccurrencePath, &'a Assembly)> {
        let mut out = vec![(OccurrencePath::root(), self.truth)];
        for row in self.table.subtree(OccurrenceIndex::ROOT).into_iter().skip(1) {
            let f = self.table.flags(row);
            if !f.has(Flags::ASSEMBLY) || f.has(Flags::SUPPRESSED) || f.has(Flags::REMOVED) {
                continue;
            }
            if let Some(a) = self.store.get(self.table.definition(row)).assembly.as_deref() {
                out.push((self.table.path(row), a));
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// The part occurrences that move on their own: not suppressed, and not pattern
    /// members, which follow their seeds.
    fn parts(&self) -> usize {
        let mut count = 0;
        let mut stack = vec![OccurrenceIndex::ROOT];
        while let Some(row) = stack.pop() {
            for child in self.table.children(row) {
                let f = self.table.flags(child);
                if f.has(Flags::SUPPRESSED) || f.has(Flags::REMOVED) || self.table.step(child).member.is_some() {
                    continue;
                }
                if f.has(Flags::ASSEMBLY) {
                    stack.push(child);
                } else {
                    count += 1;
                }
            }
        }
        count
    }
}

/// A moved part as its root keeps it: where it now sits in the assembly that holds it,
/// and where that assembly itself places it.
pub(crate) struct Kept {
    pub path: Body,
    pub local: Placement,
    pub base: Option<Placement>,
}

/// Where each part a solve moved now sits in the assembly that holds it. Every frame
/// above a part stays put, so the part's own placement takes the whole move.
pub(crate) fn locals(scope: &Scope, moved: &BTreeMap<Body, Placement>) -> Vec<Kept> {
    moved
        .iter()
        .filter_map(|(path, world)| {
            let frame = scope.frame(path)?;
            Some(Kept { path: path.clone(), local: frame.inverse().compose(world), base: scope.base(path) })
        })
        .collect()
}

/// Writes moved parts into the root's truth: on the component for its own parts, and as
/// an override for a part inside a sub-assembly. An override that would only repeat the
/// sub-assembly's own placement is dropped.
pub(crate) fn keep(truth: &mut Assembly, kept: Vec<Kept>) {
    for k in kept {
        match k.path.steps() {
            [step] => {
                if let Some(c) = truth.component_mut(step.component) {
                    c.placement = k.local;
                }
            }
            _ => truth.override_mut(&k.path).placement = (k.base != Some(k.local)).then_some(k.local),
        }
    }
    truth.prune_overrides();
}

/// `path` under `prefix`.
fn under(prefix: &OccurrencePath, path: &OccurrencePath) -> OccurrencePath {
    OccurrencePath([prefix.steps(), path.steps()].concat())
}

/// A mate's name as the root reports it: a sub-assembly's mates say where they are.
fn label(prefix: &OccurrencePath, name: &str) -> String {
    if prefix.is_root() { name.to_owned() } else { format!("{name} ({prefix})") }
}

impl AssemblyDocument {
    /// The face `e` names, on the part at `body`, as a solve reads it.
    pub(crate) fn end_of(&self, scope: &Scope, body: Body, e: &FaceEndpoint, mate: &str) -> Result<End, MateRefusal> {
        let named = |r: MateRefusal| MateRefusal { mates: vec![mate.to_owned()], ..r };
        check_endpoint(e).map_err(named)?;
        let source = scope.part_source(&body).map_err(named)?;
        if source.id != Some(e.document) {
            return Err(named(invalid(format!("{mate}: {body} places another document"))));
        }
        Ok(End { chain: scope.chain(&body), body, source, face: e.face.clone() })
    }

    /// Reads every mate `scope` holds, with its faces and the placements it starts from.
    /// Proves nothing and opens no file.
    pub(crate) fn gather(&self, scope: &Scope) -> Result<Problem, MateRefusal> {
        let mut bodies = BTreeMap::new();
        let mut mates = Vec::new();
        for (prefix, holder) in scope.holders() {
            if let Some(future) = holder.mates.iter().find(|m| m.known().is_none()) {
                return Err(MateRefusal::new(
                    RefusalCode::UnknownMate,
                    format!("this version cannot solve the mate {}", serde_json::to_string(future).unwrap_or_default()),
                ));
            }
            crate::mates::reference::check_configurations(holder)?;
            for mate in holder.mates.iter().filter_map(MateEntry::known) {
                let name = label(&prefix, mate.name());
                let mut place = |body: &Body| {
                    let at = scope.world(body).ok_or_else(|| invalid(format!("{name}: no occurrence at {body}")))?;
                    bodies.insert(body.clone(), at);
                    Ok::<_, MateRefusal>(())
                };
                match mate {
                    Mate::Origin { component, align_axes, .. } => {
                        let body = prefix.child(Step::component(*component));
                        place(&body)?;
                        let at = scope.frame(&body).ok_or_else(|| invalid(format!("{name}: no frame for {body}")))?;
                        mates.push(Gathered::Origin { mate: name, body, axes: *align_axes, at });
                    }
                    Mate::Distance { a, b, alignment, distance, .. } => {
                        let d = holder.values.length(distance).map_err(|e| super::command::value_refusal(&name, e))?;
                        let (ba, bb) = (under(&prefix, &a.path), under(&prefix, &b.path));
                        place(&ba)?;
                        place(&bb)?;
                        let ends = [self.end_of(scope, ba, a, &name)?, self.end_of(scope, bb, b, &name)?];
                        mates.push(Gathered::Distance { mate: name, ends, sign: alignment.sign(), distance: d });
                    }
                }
            }
        }
        Ok(Problem { bodies, mates, parts: scope.parts() })
    }

    /// The mates each part occurrence takes part in, by path: the root's by name, a
    /// sub-assembly's with its path after the name. Reads no part and proves nothing;
    /// empty when the mates cannot be read.
    pub fn mate_participation(&self) -> BTreeMap<OccurrencePath, Vec<String>> {
        let mut out: BTreeMap<OccurrencePath, Vec<String>> = BTreeMap::new();
        let Ok(problem) = self.gather(&Scope::new(self.assembly(), self.table(), self.store())) else { return out };
        for mate in problem.mates {
            match mate {
                Gathered::Origin { mate, body, .. } => out.entry(body).or_default().push(mate),
                Gathered::Distance { mate, ends, .. } => {
                    for end in ends {
                        out.entry(end.body).or_default().push(mate.clone());
                    }
                }
            }
        }
        out
    }

    /// The part `end` reads, with its path as seen from the root's file, so it is found
    /// where the assembly holding it keeps it. Kept as it is when a file on the way cannot
    /// be found: the folder index still finds it by id.
    pub(crate) fn rebased(&self, end: &End) -> DocRef {
        let Some(root) = self.path() else { return end.source.clone() };
        let mut holder = root.to_path_buf();
        for sub in &end.chain {
            let resolver = Resolver::new(Some(&holder))
                .with_search_folders(self.options.search.iter())
                .with_open(self.options.open.iter().cloned());
            let file = match resolver.resolve(sub) {
                Resolution::Found(t) => match t.location {
                    Location::File(p) => Some(p),
                    Location::Open(d) => d.path,
                },
                Resolution::Missing(_) => None,
            };
            match file {
                Some(f) => holder = f,
                None => return end.source.clone(),
            }
        }
        let target = end.source.stored_path(&holder);
        let mut r = end.source.clone();
        r.set_path(root, &target);
        r
    }
}
