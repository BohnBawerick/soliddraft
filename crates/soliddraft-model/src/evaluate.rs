//! Feature evaluation and rebuild scheduling.

use std::collections::HashMap;
use std::sync::Arc;

use soliddraft_kernel_api::{
    BodyId, BooleanOp, CurveKind, Draft, EdgeId, Extrude, ExtrudeLimit, FacePoint, FaceRole, Gyroid, Hole,
    HolePlacement, ImportOptions, ImportStepRequest, ImportStepResult, KernelError, KernelResult, Lattice, Mirror,
    OpResult, OpTag, Origin, PatternLayout, Plane, SurfaceKind, Vec3,
};

use crate::feature::{
    BlendFeature, DraftFeature, EndCondition, ExtrudeFeature, ExtrudeOp, Feature, FeatureId, FeatureKind,
    GyroidFeature, HoleFeature, ImportStepFeature, LatticeFeature, MirrorFeature, PatternFeature, PatternSpec,
    PlaneRef, ProfileSource, RevolveFeature, SketchFeature, SketchPlane,
};
use crate::rebuild::BuildCache;
use crate::reference::{self, Generator, NamedFace, ReferenceSlot, RepairNeeded, RepairReason, View};

use crate::document::{BuiltSketch, Document, FeatureStatus};

/// One feature's rebuild result.
#[derive(Clone, Debug)]
pub(crate) struct Built {
    pub status: FeatureStatus,
    /// The part body after this feature.
    pub output: Option<BodyId>,
    /// The body this feature made. The rebuild cache owns it and releases it.
    pub owned: Option<BodyId>,
    /// The names of the faces this feature generated, with the origins they stand for.
    pub names: Vec<NamedFace>,
    pub sketch: Option<BuiltSketch>,
    /// Which kept build this is, so a feature that reads it can key on it. Zero for a
    /// result the cache does not keep.
    pub version: u64,
    pub import_record: Option<soliddraft_kernel_api::RepairRecord>,
}

impl Built {
    fn pass(status: FeatureStatus, current: Option<BodyId>) -> Built {
        Built { status, output: current, owned: None, names: Vec::new(), sketch: None, version: 0, import_record: None }
    }

    fn failed(current: Option<BodyId>, message: impl Into<String>) -> Built {
        Built::pass(FeatureStatus::Failed(message.into()), current)
    }
}

impl Document {
    /// A copy of the part that shares this one's feature builds and bodies, so the copy
    /// builds only what changes in it. It has the same id, features and settings, and
    /// an empty undo history, as a part reopened from its file has. For dry runs, such
    /// as an assistant proposal tried before it is applied.
    pub fn fork(&self) -> Document {
        Document {
            id: self.id,
            name: self.name.clone(),
            features: self.features.clone(),
            values: self.values.clone(),
            observations: self.observations.clone(),
            configurations: self.configurations.clone(),
            properties: self.properties.clone(),
            table_edits: self.table_edits,
            datum_names: self.datum_names.clone(),
            rollback: self.rollback,
            next_id: self.next_id,
            materials: self.materials.clone(),
            settings: self.settings.clone(),
            view: self.view.clone(),
            kernel: self.kernel.clone(),
            built: self.built.clone(),
            dirty: self.dirty,
            cache: self.cache.clone(),
            undo: Vec::new(),
            redo: Vec::new(),
            log: Vec::new(),
            last_solve: None,
            journal: None,
        }
    }

    /// Reuses a fork's successful builds on this kernel without changing the document.
    /// Exact input keys decide reuse even if the document changed after the dry run.
    pub fn reuse_builds_from(&mut self, source: &Document) {
        if self.id == source.id && Arc::ptr_eq(&self.kernel, &source.kernel) {
            self.cache.import(&source.cache);
            self.cache.trim(&self.built);
        }
    }

    /// Adopts a completed fork of the current definitions without evaluating them again.
    /// The caller must also reject completions for an obsolete application revision.
    /// History, identity and authored settings stay on this document. Imported bodies
    /// become live before trimming, so a small budget cannot evict them during adoption.
    pub fn rebuild_reusing(&mut self, source: &Document) -> Result<(), String> {
        if self.id != source.id || !Arc::ptr_eq(&self.kernel, &source.kernel) {
            return Err("the completed build belongs to another document or kernel".into());
        }
        if source.is_dirty() {
            return Err("the completed build still has unbuilt definitions".into());
        }
        let definition = crate::exact::bytes(&(&self.features, self.rollback));
        if definition.is_none() || definition != crate::exact::bytes(&(&source.features, source.rollback)) {
            return Err("the completed build does not match the current feature definitions".into());
        }
        let versions = self.cache.import(&source.cache);
        if source.built.iter().any(|built| built.version != 0 && !versions.contains_key(&built.version)) {
            self.cache.trim(&self.built);
            return Err("the completed build does not share this document's retained inputs".into());
        }
        let journal = self.journal_command(|| serde_json::json!({"op":"rebuild"}));
        journal.finish({
            self.built = source
                .built
                .iter()
                .map(|built| {
                    let mut built = built.clone();
                    if built.version != 0 {
                        built.version = versions[&built.version];
                    }
                    built
                })
                .collect();
            self.dirty = self.built.len();
            self.cache.trim(&self.built);
            self.refresh_tracked();
            Ok(())
        })
    }

    /// True while the document keeps `body`: a feature build it holds made it. A body
    /// no document keeps is released and does not come back.
    pub fn keeps(&self, body: BodyId) -> bool {
        self.cache.holds(body)
    }

    /// The bytes the kept feature builds retain, and the budget they are trimmed to.
    pub fn rebuild_cache_usage(&self) -> (u64, u64) {
        (self.cache.retained(), self.cache.budget())
    }

    /// Sets how many bytes of feature builds the document keeps for reuse. The builds of
    /// the current tree are always kept. A smaller budget applies at once.
    pub fn set_rebuild_budget(&mut self, bytes: u64) {
        self.cache.set_budget(bytes);
        self.cache.trim(&self.built);
    }

    /// True when the current definitions have not been rebuilt.
    pub fn is_dirty(&self) -> bool {
        self.dirty < self.features.len() || self.built.len() != self.features.len()
    }

    /// Rebuilds from the first stale feature to the end of the tree, reusing every
    /// feature build whose exact inputs the cache holds.
    pub fn rebuild(&mut self) {
        let journal = self.journal_command(|| serde_json::json!({"op":"rebuild"}));
        let _ = journal.finish({
            self.rebuild_inner();
            self.refresh_tracked();
            Ok::<_, String>(())
        });
    }

    fn rebuild_inner(&mut self) {
        let start = self.dirty.min(self.built.len());
        self.built.truncate(start);
        let active = self.active_count();
        let mut cache = std::mem::take(&mut self.cache);
        let mut current = start.checked_sub(1).and_then(|i| self.built[i].output);
        for i in start..self.features.len() {
            let f = self.features[i].clone();
            let built = if i >= active {
                Built::pass(FeatureStatus::RolledBack, current)
            } else if f.suppressed {
                Built::pass(FeatureStatus::Suppressed, current)
            } else {
                self.build(&f, current, &mut cache)
            };
            current = built.output;
            self.built.push(built);
        }
        cache.trim(&self.built);
        self.cache = cache;
        self.dirty = self.built.len();
    }

    /// Marks every feature from `index` down as stale.
    pub(crate) fn touch(&mut self, index: usize) {
        self.dirty = self.dirty.min(index);
        self.stale_tracked("proof pending");
    }

    /// A feature's build: the kept one when the cache has its exact inputs, else a new
    /// one, kept. Checks whose messages name another feature run every time and are not
    /// kept, so a rename shows in them without building anything.
    fn build(&self, f: &Feature, current: Option<BodyId>, cache: &mut BuildCache) -> Built {
        let profile = match &f.kind {
            FeatureKind::Revolve(RevolveFeature { sketch: feature, .. })
            | FeatureKind::Extrude(ExtrudeFeature { profile: ProfileSource::Sketch { feature }, .. }) => {
                match self.profile_input(*feature, current) {
                    Ok(input) => Some(input.version),
                    Err(refused) => return *refused,
                }
            }
            _ => None,
        };
        let key = cache.key(f, current, profile);
        if let Some(built) = cache.get(&key) {
            return built;
        }
        let built = match &f.kind {
            FeatureKind::Sketch(s) => self.build_sketch(s, current),
            FeatureKind::Extrude(e) => self.build_extrude(f, e, current),
            FeatureKind::Revolve(r) => self.build_revolve(f, r, current),
            FeatureKind::Fillet(b) => self.build_blend(f, b, false, current),
            FeatureKind::Chamfer(b) => self.build_blend(f, b, true, current),
            FeatureKind::Hole(h) => self.build_hole(f, h, current),
            FeatureKind::Pattern(p) => self.build_pattern(f, p, current),
            FeatureKind::Mirror(m) => self.build_mirror(f, m, current),
            FeatureKind::Draft(d) => self.build_draft(f, d, current),
            FeatureKind::Shell(s) => self.build_shell(f, s, current),
            FeatureKind::ImportStep(i) => self.build_import(f, i, current),
            FeatureKind::Lattice(l) => self.build_lattice(f, l, current),
            FeatureKind::Gyroid(g) => self.build_gyroid(f, g, current),
        };
        cache.insert(key, built, &self.kernel)
    }

    /// The built sketch an extrusion sweeps, or the extrusion's result when that sketch
    /// is missing, not built or has no closed region.
    fn profile_input(&self, sketch: FeatureId, current: Option<BodyId>) -> Result<&Built, Box<Built>> {
        let sketch_name = || self.feature(sketch).map(|s| s.name.clone()).unwrap_or_else(|| sketch.to_string());
        let failed = |message: String| Err(Box::new(Built::failed(current, message)));
        let Some(input) = self.built_index(sketch).map(|i| &self.built[i]) else {
            return failed(format!("its sketch {} is missing", sketch_name()));
        };
        let Some(sk) = input.sketch.as_ref() else {
            if input.status.is_ok() {
                return failed(format!("{} is not a sketch", sketch_name()));
            }
            return Err(Box::new(Built::pass(FeatureStatus::Blocked(sketch), current)));
        };
        match &sk.regions {
            Ok(r) if !r.is_empty() => Ok(input),
            Ok(_) => failed(format!("{} has no closed region", sketch_name())),
            Err(err) => failed(format!("{} has no closed region: {err}", sketch_name())),
        }
    }

    fn build_sketch(&self, s: &SketchFeature, current: Option<BodyId>) -> Built {
        let (plane, on_face) = match &s.plane {
            SketchPlane::Datum { datum, offset } => {
                let p = datum.plane();
                (p.with_origin(p.origin + p.normal * *offset), false)
            }
            SketchPlane::Face { face } => {
                let repair = |reason, message: String, candidates| {
                    let r = RepairNeeded { slot: ReferenceSlot::SketchPlane, reason, message, candidates };
                    Built::pass(FeatureStatus::RepairNeeded(vec![r]), current)
                };
                let Some(body) = current else {
                    return repair(RepairReason::Missing, "there is no part body to sketch on".into(), vec![]);
                };
                let t = match self.view_of(body) {
                    Ok(v) => v,
                    Err(e) => return Built::failed(current, e.to_string()),
                };
                let id = match reference::resolve_plane(face, &View { topology: &t }, &self.name_map()) {
                    Ok(id) => id,
                    Err(u) => {
                        return Built::pass(
                            FeatureStatus::RepairNeeded(vec![u.into_repair(ReferenceSlot::SketchPlane)]),
                            current,
                        );
                    }
                };
                let info = t.face(id).expect("resolved faces are in the topology");
                let Some(normal) = info.normal else {
                    let detail = "the face is not planar".to_string();
                    return repair(RepairReason::Mismatch { detail: detail.clone() }, detail, vec![]);
                };
                (plane_on_face(normal, info.centroid), true)
            }
        };
        let regions = s.sketch.regions().map_err(|e| e.to_string());
        Built {
            status: FeatureStatus::Ok,
            output: current,
            owned: None,
            names: Vec::new(),
            sketch: Some(BuiltSketch { plane, on_face, regions }),
            version: 0,
            import_record: None,
        }
    }

    fn build_revolve(&self, f: &Feature, r: &RevolveFeature, current: Option<BodyId>) -> Built {
        if !r.angle_deg.is_finite()
            || r.angle_deg <= 0.0
            || r.angle_deg > 360.0
            || !r.axis_origin.iter().chain(&r.axis_direction).all(|v| v.is_finite())
            || r.axis_direction.iter().all(|v| *v == 0.0)
        {
            return Built::failed(current, "revolve needs a finite axis, nonzero direction and angle in (0, 360]");
        }
        let input = match self.profile_input(r.sketch, current) {
            Ok(input) => input,
            Err(refused) => return *refused,
        };
        let sk = input.sketch.as_ref().expect("a profile input is a built sketch");
        let regions = match &sk.regions {
            Ok(regions) if !regions.is_empty() => regions,
            _ => return Built::failed(current, "revolve needs a closed sketch region"),
        };
        let mut mine = None;
        let mut faces = Vec::new();
        let mut run = || -> KernelResult<Option<BodyId>> {
            let k = &self.kernel;
            let mut body = current;
            for (i, region) in regions.iter().enumerate() {
                let tag = |step: &str| OpTag::new(format!("{}.r{i}.{step}", f.id));
                let sheet = k.create_body_from_profile(&tag("profile"), &region.to_profile(sk.plane))?;
                let tool = k.revolve(
                    &tag("sweep"),
                    sheet.body,
                    &soliddraft_kernel_api::Revolve {
                        axis_origin: r.axis_origin.into(),
                        axis_direction: r.axis_direction.into(),
                        angle_deg: r.angle_deg,
                    },
                );
                let _ = k.release(sheet.body);
                let tool = tool?;
                for origin in &tool.record.face_roles {
                    let generator = match origin.role {
                        FaceRole::StartCap => Some(Generator::StartCap),
                        FaceRole::EndCap => Some(Generator::EndCap),
                        FaceRole::Side { loop_index, segment } => region
                            .entity(loop_index as usize, segment as usize)
                            .map(|entity| Generator::Side { entity }),
                        _ => None,
                    };
                    if let Some(generator) = generator {
                        faces.push((Origin { tag: tool.record.tag.clone(), role: origin.role }, generator, i));
                    }
                }
                let next = if let Some(previous) = body {
                    let joined = k.boolean(&tag("union"), BooleanOp::Union, previous, tool.body);
                    let _ = k.release(tool.body);
                    joined?.body
                } else {
                    tool.body
                };
                if let Some(previous) = mine.replace(next) {
                    let _ = k.release(previous);
                }
                body = Some(next);
            }
            Ok(body)
        };
        match run() {
            Ok(output) => Built {
                status: FeatureStatus::Ok,
                output,
                owned: mine,
                names: number(faces),
                sketch: None,
                version: 0,
                import_record: None,
            },
            Err(error) => {
                if let Some(body) = mine {
                    let _ = self.kernel.release(body);
                }
                Built::failed(current, error.to_string())
            }
        }
    }

    fn build_extrude(&self, f: &Feature, e: &ExtrudeFeature, current: Option<BodyId>) -> Built {
        let shape_sketch;
        let sk = match &e.profile {
            ProfileSource::Sketch { feature } => match self.profile_input(*feature, current) {
                Ok(input) => input.sketch.as_ref().expect("a profile input is a built sketch"),
                Err(refused) => return *refused,
            },
            ProfileSource::Shape { datum, shape } => {
                let regions = shape.sketch().and_then(|s| s.regions().map_err(|e| e.to_string()));
                shape_sketch = BuiltSketch { plane: datum.plane(), on_face: false, regions };
                &shape_sketch
            }
        };
        let regions = match &sk.regions {
            Ok(r) if !r.is_empty() => r,
            Ok(_) => return Built::failed(current, "the built-in profile has no closed region"),
            Err(err) => return Built::failed(current, format!("the built-in profile has no closed region: {err}")),
        };
        let dir = if e.reverse { -sk.plane.normal } else { sk.plane.normal };
        // The tool starts exactly on its sketch plane, also on a face of the part; the
        // kernel's coplanar Boolean rules decide the shared face.
        let start = sk.plane;
        let limit = match &e.end {
            EndCondition::UpToNext => Some(ExtrudeLimit::UpToNext),
            EndCondition::UpToFace { face } => {
                let Some(body) = current else {
                    return Built::failed(current, "an extrusion up to a face needs a body");
                };
                match self.resolve_face(face, body) {
                    Ok(face) => Some(ExtrudeLimit::UpToFace { face }),
                    Err(r) => {
                        let r = RepairNeeded { slot: ReferenceSlot::Limit, ..r };
                        return Built::pass(FeatureStatus::RepairNeeded(vec![r]), current);
                    }
                }
            }
            _ => None,
        };
        if limit.is_some() && current.is_none() {
            return Built::failed(current, "an extrusion up to the next face needs a body");
        }
        let distance = match e.end {
            EndCondition::Blind { depth } if depth.is_finite() && depth > 0.0 => depth,
            EndCondition::Blind { depth } => {
                return Built::failed(current, format!("depth must be positive, got {depth}"));
            }
            EndCondition::ThroughAll => {
                let Some(body) = current else {
                    return Built::failed(current, "through all needs a body to go through");
                };
                let bbox = match self.kernel.topology(body) {
                    Ok(t) => t.bbox,
                    Err(err) => return Built::failed(current, err.to_string()),
                };
                let (lo, hi) = (bbox.min, bbox.max);
                let reach = (0..8)
                    .map(|c| {
                        let corner = Vec3::new(
                            if c & 1 == 0 { lo.x } else { hi.x },
                            if c & 2 == 0 { lo.y } else { hi.y },
                            if c & 4 == 0 { lo.z } else { hi.z },
                        );
                        (corner - start.origin).dot(dir)
                    })
                    .fold(f64::NEG_INFINITY, f64::max);
                if reach <= 1e-9 {
                    return Built::failed(current, "the body lies entirely behind the sketch plane");
                }
                reach + 1.0 + 0.01 * bbox.diagonal()
            }
            EndCondition::UpToNext | EndCondition::UpToFace { .. } => 0.0,
        };
        let mut mine: Option<BodyId> = None;
        let mut faces: Vec<(Origin, Generator, usize)> = Vec::new();
        let mut run = |mine: &mut Option<BodyId>| -> KernelResult<Option<BodyId>> {
            let k = &self.kernel;
            let mut body = current;
            for (i, region) in regions.iter().enumerate() {
                let tag = |step: &str| OpTag::new(format!("{}.r{i}.{step}", f.id));
                let sheet = k.create_body_from_profile(&tag("profile"), &region.to_profile(start))?;
                let tool = match (&limit, current) {
                    (Some(limit), Some(target)) => k.extrude_to(&tag("sweep"), sheet.body, target, Some(dir), limit),
                    _ => k.extrude(&tag("sweep"), sheet.body, &Extrude { distance, direction: Some(dir) }),
                };
                let _ = k.release(sheet.body);
                let tool = tool?;
                for o in &tool.record.face_roles {
                    let generator = match o.role {
                        FaceRole::StartCap => Some(Generator::StartCap),
                        FaceRole::EndCap => Some(Generator::EndCap),
                        FaceRole::Side { loop_index, segment } => region
                            .entity(loop_index as usize, segment as usize)
                            .map(|entity| Generator::Side { entity }),
                        _ => None,
                    };
                    if let Some(generator) = generator {
                        faces.push((Origin { tag: tool.record.tag.clone(), role: o.role }, generator, i));
                    }
                }
                let next = match (e.operation, body) {
                    (ExtrudeOp::Boss, None) => tool.body,
                    (ExtrudeOp::Cut, None) => {
                        let _ = k.release(tool.body);
                        return Err(KernelError::InvalidInput("there is no body to cut".into()));
                    }
                    (op, Some(b)) => {
                        let (kind, step) = if op == ExtrudeOp::Boss {
                            (BooleanOp::Union, "union")
                        } else {
                            (BooleanOp::Subtract, "cut")
                        };
                        let joined = k.boolean(&tag(step), kind, b, tool.body);
                        let _ = k.release(tool.body);
                        joined?.body
                    }
                };
                if let Some(old) = mine.replace(next) {
                    let _ = k.release(old);
                }
                body = Some(next);
            }
            Ok(body)
        };
        match run(&mut mine) {
            Ok(output) => Built {
                status: FeatureStatus::Ok,
                output,
                owned: mine,
                names: number(faces),
                sketch: None,
                version: 0,
                import_record: None,
            },
            Err(err) => {
                if let Some(body) = mine {
                    let _ = self.kernel.release(body);
                }
                Built::failed(current, err.to_string())
            }
        }
    }

    fn build_blend(&self, f: &Feature, b: &BlendFeature, chamfer: bool, current: Option<BodyId>) -> Built {
        let what = if chamfer { "chamfer" } else { "fillet" };
        let Some(body) = current else {
            return Built::failed(current, format!("there is no body to {what}"));
        };
        if b.edges.is_empty() {
            return Built::failed(current, format!("the {what} has no edges"));
        }
        let t = match self.view_of(body) {
            Ok(v) => v,
            Err(err) => return Built::failed(current, err.to_string()),
        };
        let names = self.name_map();
        let view = View { topology: &t };
        let mut ids: Vec<EdgeId> = Vec::new();
        let mut repairs = Vec::new();
        for (i, r) in b.edges.iter().enumerate() {
            match reference::resolve_edge(r, &view, &names) {
                Ok(id) => ids.push(id),
                Err(u) => repairs.push(u.into_repair(ReferenceSlot::Edge { index: i as u32 })),
            }
        }
        if !repairs.is_empty() {
            return Built::pass(FeatureStatus::RepairNeeded(repairs), current);
        }
        for (i, id) in ids.iter().enumerate() {
            if let Some(j) = ids[..i].iter().position(|x| x == id) {
                return Built::failed(current, format!("edges {} and {} name the same edge", j + 1, i + 1));
            }
        }
        let tag = OpTag::new(format!("{}.blend", f.id));
        let result = if chamfer {
            self.kernel.chamfer(&tag, body, &ids, b.size)
        } else {
            self.kernel.fillet(&tag, body, &ids, b.size)
        };
        let result = match result {
            Ok(r) => r,
            Err(err) => return Built::failed(current, err.to_string()),
        };
        let faces = result
            .record
            .face_roles
            .iter()
            .filter_map(|o| match o.role {
                FaceRole::Blend { edge } => ids.iter().position(|x| *x == edge).map(|i| {
                    (Origin { tag: result.record.tag.clone(), role: o.role }, Generator::Blend { edge: i as u32 }, 0)
                }),
                _ => None,
            })
            .collect();
        Built {
            status: FeatureStatus::Ok,
            output: Some(result.body),
            owned: Some(result.body),
            names: number(faces),
            sketch: None,
            version: 0,
            import_record: None,
        }
    }

    /// Resolves a feature's plane: a reference plane, or a planar face of `body` with
    /// its outward normal.
    fn plane_of(&self, r: &PlaneRef, body: BodyId) -> Result<Plane, Box<RepairNeeded>> {
        match r {
            PlaneRef::Datum { datum, offset } => {
                let p = datum.plane();
                Ok(p.with_origin(p.origin + p.normal * *offset))
            }
            PlaneRef::Face { face } => {
                let id = self
                    .resolve_face(face, body)
                    .map_err(|r| Box::new(RepairNeeded { slot: ReferenceSlot::Plane, ..r }))?;
                let info = self.kernel.topology(body).ok().and_then(|t| t.face(id).cloned());
                match info.and_then(|f| f.normal.map(|n| (n, f.centroid))) {
                    Some((normal, centroid)) => Ok(plane_on_face(normal, centroid)),
                    None => {
                        let detail = "the face is not planar".to_string();
                        Err(Box::new(RepairNeeded {
                            slot: ReferenceSlot::Plane,
                            reason: RepairReason::Mismatch { detail: detail.clone() },
                            message: detail,
                            candidates: vec![],
                        }))
                    }
                }
            }
        }
    }

    /// The result of an operation that makes a new body from the part's body, with
    /// the faces it made named.
    fn finish(
        &self,
        result: KernelResult<OpResult>,
        current: Option<BodyId>,
        name: impl Fn(&OpResult) -> Vec<NamedFace>,
    ) -> Built {
        match result {
            Ok(r) => Built {
                status: FeatureStatus::Ok,
                output: Some(r.body),
                owned: Some(r.body),
                names: name(&r),
                sketch: None,
                version: 0,
                import_record: None,
            },
            Err(err) => Built::failed(current, err.to_string()),
        }
    }

    fn build_hole(&self, f: &Feature, h: &HoleFeature, current: Option<BodyId>) -> Built {
        let Some(body) = current else { return Built::failed(current, "there is no body to drill") };
        if h.points.is_empty() {
            return Built::failed(current, "the hole has no points");
        }
        let plane = match self.plane_of(&PlaneRef::Face { face: h.face.clone() }, body) {
            Ok(p) => p,
            Err(r) => return Built::pass(FeatureStatus::RepairNeeded(vec![*r]), current),
        };
        let face = self.resolve_face(&h.face, body).ok();
        let placements = h
            .points
            .iter()
            .map(|p| HolePlacement {
                point: plane.origin + plane.x_dir * p.u + plane.y_dir() * p.v,
                direction: -plane.normal,
                on_face: face.map(|face| FacePoint { face, u: p.u, v: p.v }),
            })
            .collect();
        let params = Hole { placements, diameter: h.diameter, depth: h.depth, tip: h.tip, head: h.head };
        let result = self.kernel.hole(&OpTag::new(format!("{}.hole", f.id)), body, &params);
        self.finish(result, current, |r| {
            let faces = r
                .record
                .face_roles
                .iter()
                .filter_map(|o| match o.role {
                    FaceRole::Hole { placement, part } => Some((
                        Origin { tag: r.record.tag.clone(), role: o.role },
                        Generator::Hole { placement, part },
                        0,
                    )),
                    _ => None,
                })
                .collect();
            number(faces)
        })
    }

    fn build_pattern(&self, f: &Feature, p: &PatternFeature, current: Option<BodyId>) -> Built {
        let Some(body) = current else { return Built::failed(current, "there is no body to pattern") };
        let layout = match p.layout {
            PatternSpec::Linear { direction, spacing, count } => {
                let Some(unit) = direction.normalized() else {
                    return Built::failed(current, "the pattern direction is zero");
                };
                PatternLayout::Linear { step: unit * spacing, count }
            }
            PatternSpec::Circular { axis_origin, axis_direction, count, angle } => {
                PatternLayout::Circular { axis_origin, axis_direction, count, angle_deg: angle }
            }
        };
        let result = self.kernel.pattern(&OpTag::new(format!("{}.pattern", f.id)), body, &layout);
        self.finish(result, current, |r| self.copies(r, body))
    }

    fn build_mirror(&self, f: &Feature, m: &MirrorFeature, current: Option<BodyId>) -> Built {
        let Some(body) = current else { return Built::failed(current, "there is no body to mirror") };
        let plane = match self.plane_of(&m.plane, body) {
            Ok(p) => p,
            Err(r) => return Built::pass(FeatureStatus::RepairNeeded(vec![*r]), current),
        };
        let params = Mirror { plane_origin: plane.origin, plane_normal: plane.normal, keep_original: m.keep_original };
        let result = self.kernel.mirror(&OpTag::new(format!("{}.mirror", f.id)), body, &params);
        self.finish(result, current, |r| self.copies(r, body))
    }

    /// Names a pattern's or mirror's copies: each copy's faces are ranked by the names of
    /// the faces of `source` they copy, so the ranks follow origins, not positions.
    fn copies(&self, r: &OpResult, source: BodyId) -> Vec<NamedFace> {
        let view = self.view_of(source).ok();
        let names = self.name_map();
        let mut faces: Vec<(u32, Option<SourceName>, FaceRole)> = Vec::new();
        for o in &r.record.face_roles {
            if let FaceRole::Copy { instance, face } = o.role {
                let key = view.as_ref().and_then(|t| reference::origin_label(face, &View { topology: t }, &names));
                faces.push((instance, key, o.role));
            }
        }
        faces.sort();
        faces.dedup();
        let mut out = Vec::new();
        let mut ordinal = 0;
        for (i, (instance, _, role)) in faces.iter().enumerate() {
            if i > 0 && faces[i - 1].0 != *instance {
                ordinal = 0;
            }
            out.push(NamedFace {
                origin: Origin { tag: r.record.tag.clone(), role: *role },
                generator: Generator::Copy { instance: *instance },
                ordinal,
            });
            ordinal += 1;
        }
        out
    }

    fn build_shell(&self, f: &Feature, s: &crate::ShellFeature, current: Option<BodyId>) -> Built {
        let Some(body) = current else { return Built::failed(current, "there is no body to shell") };
        let mut repairs = Vec::new();
        let mut faces = Vec::new();
        for (i, reference) in s.open_faces.iter().enumerate() {
            match self.resolve_face(reference, body) {
                Ok(face) => faces.push(face),
                Err(repair) => repairs.push(RepairNeeded { slot: ReferenceSlot::Face { index: i as u32 }, ..repair }),
            }
        }
        if !repairs.is_empty() {
            return Built::pass(FeatureStatus::RepairNeeded(repairs), current);
        }
        let result = self.kernel.shell(&OpTag::new(format!("{}.shell", f.id)), body, &faces, s.thickness);
        self.finish(result, current, |r| {
            let view = self.view_of(body).ok();
            let names = self.name_map();
            let mut walls: Vec<_> = r
                .record
                .face_roles
                .iter()
                .filter_map(|o| {
                    let FaceRole::Offset { face } = o.role else { return None };
                    // Never rank an unnamed wall by a transient kernel handle.
                    let source =
                        view.as_ref().and_then(|t| reference::origin_label(face, &View { topology: t }, &names))?;
                    Some((source, o.role))
                })
                .collect();
            walls.sort();
            walls
                .into_iter()
                .enumerate()
                .map(|(ordinal, (_, role))| NamedFace {
                    origin: Origin { tag: r.record.tag.clone(), role },
                    generator: Generator::ShellWall,
                    ordinal: ordinal as u32,
                })
                .collect()
        })
    }

    /// What the part says about the self-intersection of imported body `id`. A body with
    /// curved faces or edges is not checked for it, and is never called validated. A
    /// planar, straight-edged body gets no claim either, until the backend's check is
    /// pinned. `None` when the import did not build.
    pub fn import_check_note(&self, id: FeatureId) -> Option<String> {
        let built = &self.built[self.built_index(id)?];
        if !matches!(self.feature(id)?.kind, FeatureKind::ImportStep(_)) || !built.status.is_ok() {
            return None;
        }
        let topology = self.view_of(built.output?).ok()?;
        let curved = topology.faces.iter().any(|f| f.surface != SurfaceKind::Plane)
            || topology.edges.iter().any(|e| e.curve != CurveKind::Line);
        curved.then(|| {
            "self-intersection not verified: this body has curved faces or edges, which are not checked for \
             self-intersection. It is not marked validated."
                .to_string()
        })
    }

    fn build_lattice(&self, f: &Feature, l: &LatticeFeature, current: Option<BodyId>) -> Built {
        let Some(body) = current else { return Built::failed(current, "there is no body to lattice") };
        let (Some(cell), Some(cell_size_mm), Some(radius_mm)) = (l.cell, l.cell_size_mm, l.radius_mm) else {
            return Built::failed(current, "lattice is missing cell size and density; configure the cubic cell");
        };
        let params = Lattice {
            cell,
            cell_size_mm,
            radius_mm,
            frame: l.frame,
            accuracy_mm: l.accuracy_mm,
            min_detail_mm: l.min_detail_mm,
        };
        let result = self.kernel.lattice(&OpTag::new(format!("{}.lattice", f.id)), body, &params);
        self.finish_field(result, current)
    }

    fn build_gyroid(&self, f: &Feature, g: &GyroidFeature, current: Option<BodyId>) -> Built {
        let Some(body) = current else { return Built::failed(current, "there is no body for the gyroid") };
        let params = Gyroid {
            kind: g.kind,
            period_mm: g.period_mm,
            level: g.level,
            frame: g.frame,
            accuracy_mm: g.accuracy_mm,
            min_detail_mm: g.min_detail_mm,
        };
        let result = self.kernel.gyroid(&OpTag::new(format!("{}.gyroid", f.id)), body, &params);
        self.finish_field(result, current)
    }

    /// A terminal field names one display region and no analytic face.
    fn finish_field(&self, result: KernelResult<OpResult>, current: Option<BodyId>) -> Built {
        self.finish(result, current, |r| {
            vec![NamedFace {
                origin: Origin { tag: r.record.tag.clone(), role: FaceRole::Other },
                generator: Generator::Other,
                ordinal: 0,
            }]
        })
    }

    /// The last rebuild's repair record, including a declined import with no body.
    pub fn import_record(&self, id: FeatureId) -> Option<&soliddraft_kernel_api::RepairRecord> {
        self.built.get(self.built_index(id)?)?.import_record.as_ref()
    }

    fn build_import(&self, f: &Feature, i: &ImportStepFeature, current: Option<BodyId>) -> Built {
        if current.is_some() {
            return Built::failed(current, "an imported body must come first in the part");
        }
        if !i.hash_matches() {
            return Built::failed(current, "the imported file does not match its sha256");
        }
        let decisions = match i.kernel_decisions() {
            Ok(d) => d,
            Err(message) => return Built::failed(current, message),
        };
        let request = ImportStepRequest {
            name: &i.source_name,
            source: &i.source,
            sha256: &i.sha256,
            options: &ImportOptions::default(),
            decisions: &decisions,
        };
        match self.kernel.import_step(&OpTag::new(format!("{}.import", f.id)), &request) {
            Ok(mut r) => {
                let release = |r: &ImportStepResult| {
                    for b in &r.bodies {
                        let _ = self.kernel.release(b.result.body);
                    }
                };
                if !r.left_open.is_empty() {
                    release(&r);
                    let names = r.left_open.join(", ");
                    let mut built = Built::failed(
                        current,
                        format!(
                            "{names} not imported until the gap is resolved: a declined or unanswered repair leaves the solid open and unbuilt. The original file was never changed"
                        ),
                    );
                    built.import_record = Some(r.record);
                    return built;
                }
                if r.bodies.len() != 1 {
                    release(&r);
                    let n = r.bodies.len();
                    return Built::failed(current, format!("the file holds {n} solids; a part holds one body"));
                }
                let body = r.bodies.remove(0).result.body;
                Built {
                    status: FeatureStatus::Ok,
                    output: Some(body),
                    owned: Some(body),
                    names: Vec::new(),
                    sketch: None,
                    version: 0,
                    import_record: Some(r.record),
                }
            }
            Err(KernelError::Unsupported(what)) => {
                Built::failed(current, format!("this build cannot import STEP files yet: the backend has no {what}"))
            }
            Err(err) => Built::failed(current, err.to_string()),
        }
    }

    fn build_draft(&self, f: &Feature, d: &DraftFeature, current: Option<BodyId>) -> Built {
        let Some(body) = current else { return Built::failed(current, "there is no body to draft") };
        if d.faces.is_empty() {
            return Built::failed(current, "the draft has no faces");
        }
        let mut repairs = Vec::new();
        let mut faces = Vec::new();
        for (i, r) in d.faces.iter().enumerate() {
            match self.resolve_face(r, body) {
                Ok(id) => faces.push(id),
                Err(r) => repairs.push(RepairNeeded { slot: ReferenceSlot::Face { index: i as u32 }, ..r }),
            }
        }
        let neutral = match self.plane_of(&d.neutral, body) {
            Ok(p) => Some(p),
            Err(r) => {
                repairs.push(*r);
                None
            }
        };
        let Some(neutral) = neutral.filter(|_| repairs.is_empty()) else {
            return Built::pass(FeatureStatus::RepairNeeded(repairs), current);
        };
        let pull = if d.reverse { -neutral.normal } else { neutral.normal };
        let params = Draft { pull, neutral_origin: neutral.origin, neutral_normal: neutral.normal, angle_deg: d.angle };
        let result = self.kernel.draft(&OpTag::new(format!("{}.draft", f.id)), body, &faces, &params);
        self.finish(result, current, |_| Vec::new())
    }
}

/// The name of a face a copy copies: its feature, generator and ordinal.
type SourceName = (FeatureId, Generator, u32);

pub(crate) fn kernel_repair(slot: ReferenceSlot, e: KernelError) -> RepairNeeded {
    RepairNeeded { slot, reason: RepairReason::Missing, message: e.to_string(), candidates: Vec::new() }
}

/// Ordinals: faces with the same generator are ranked by the region that made them. A
/// region makes each generator once, so the rank never depends on geometry.
fn number(mut faces: Vec<(Origin, Generator, usize)>) -> Vec<NamedFace> {
    faces.sort_by_key(|f| (f.1, f.2));
    let mut counts: HashMap<Generator, u32> = HashMap::new();
    faces
        .into_iter()
        .map(|(origin, generator, _)| {
            let n = counts.entry(generator).or_default();
            let ordinal = *n;
            *n += 1;
            NamedFace { origin, generator, ordinal }
        })
        .collect()
}

/// The sketch plane on a planar face: origin at the world origin projected onto the
/// face, X horizontal for a face that is not horizontal, else the world X axis. The
/// Catallax adapter builds this frame again in exact numbers for hole centres.
pub fn plane_on_face(normal: Vec3, point: Vec3) -> Plane {
    let n = normal;
    let x_dir = if n.z.abs() < 1.0 - 1e-9 {
        Vec3::Z.cross(n).normalized().expect("not parallel to Z")
    } else {
        (Vec3::X - n * Vec3::X.dot(n)).normalized().expect("parallel to Z")
    };
    Plane { origin: n * point.dot(n), normal: n, x_dir }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn face_planes_have_upright_frames() {
        let p = plane_on_face(-Vec3::Y, Vec3::new(3.0, -45.0, 7.0));
        assert!(p.is_well_formed());
        assert!(p.origin.distance(Vec3::new(0.0, -45.0, 0.0)) < 1e-12);
        assert!(p.x_dir.distance(Vec3::X) < 1e-12 && p.y_dir().distance(Vec3::Z) < 1e-12);
        let top = plane_on_face(Vec3::Z, Vec3::new(5.0, 5.0, 10.0));
        assert!(top.origin.distance(Vec3::new(0.0, 0.0, 10.0)) < 1e-12);
        assert!(top.x_dir.distance(Vec3::X) < 1e-12 && top.y_dir().distance(Vec3::Y) < 1e-12);
        let right = plane_on_face(Vec3::X, Vec3::new(60.0, 1.0, 1.0));
        assert!(right.x_dir.distance(Vec3::Y) < 1e-12 && right.y_dir().distance(Vec3::Z) < 1e-12);
    }
}
