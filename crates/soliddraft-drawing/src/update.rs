//! Associativity: bringing a drawing up to date with its part.
//!
//! An update regenerates the line work of every view whose source digest differs from
//! the part's, then re-resolves every annotation's references against the new body.
//! Exactly one match moves the anchor and re-measures the value; a value that moved is
//! marked changed until someone accepts it. No match, or several, marks the annotation
//! dangling: it stays on the sheet with its last geometry and the cause in words, until
//! it is reattached, deleted or kept as a note. Nothing is removed by an update.

use serde::{Deserialize, Serialize};

use soliddraft_kernel_api::Point2;

use crate::annotation::{Annotation, AnnotationKind, AnnotationState, Attach, DimValue};
use crate::entity::{EntityKind, EntityRef};
use crate::ids::{AnnotationId, ViewId};
use crate::library::{self, Symbols};
use crate::measure::{AnchorGeometry, anchor_point, extension_direction, measure, value_text};
use crate::part::{Entity, Part, PartIndex, Resolution, feature_name, round};
use crate::project::{self, Contexts, Projected, ViewContext};
use crate::view::ViewKind;
use crate::{ContentHash, Drawing, DrawingError};

/// What an update did.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UpdateReport {
    /// The part revision the drawing was pinned at before.
    pub digest_before: ContentHash,
    /// The part revision it is pinned at now.
    pub digest_after: ContentHash,
    /// Views whose line work was regenerated.
    pub views_updated: usize,
    /// Dimensions whose displayed value changed in this update.
    pub changed: Vec<ChangedValue>,
    /// Annotations with references that do not resolve, after this update.
    pub dangling: Vec<DanglingAnnotation>,
    /// Annotations whose references all resolved.
    pub resolved: usize,
    /// Views that could not be made from the part any more, such as an auxiliary view
    /// whose edge is gone or a section whose plane no longer meets the part. They keep
    /// their last line work until they are repaired or deleted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dangling_views: Vec<DanglingView>,
    /// Each regenerated view's projection: its transform and the circles it sees, for
    /// tools that pick on the views.
    #[serde(skip)]
    pub projections: Vec<(ViewId, Projected)>,
}

/// A view that lost what it is made from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DanglingView {
    /// The view.
    pub view: ViewId,
    /// Why, in words.
    pub cause: String,
}

/// A dimension whose value moved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChangedValue {
    /// The annotation.
    pub annotation: AnnotationId,
    /// Its view.
    pub view: ViewId,
    /// The value shown before.
    pub previous: String,
    /// The value shown now.
    pub now: String,
}

/// An annotation that lost references.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DanglingAnnotation {
    /// The annotation.
    pub annotation: AnnotationId,
    /// Its view.
    pub view: ViewId,
    /// Indices of the lost anchors.
    pub lost: Vec<usize>,
    /// Why, in words.
    pub cause: String,
}

/// Brings `drawing` up to date with `part`: regenerates out-of-date views, and preview
/// views when `part` asks for certified line work,
/// re-resolves every annotation of every view that is not excluded from updates, and
/// pins the drawing's model reference at the part's revision. This is the drawing half
/// of a Rebuild; the caller binds and re-paths the reference (see
/// `soliddraft_document::resolve::repin`).
pub fn update_from_model(drawing: &mut Drawing, part: &Part) -> Result<UpdateReport, DrawingError> {
    let index = PartIndex::build(part)?;
    let mut report = UpdateReport {
        digest_before: drawing.model.pinned.clone(),
        digest_after: part.digest.clone(),
        views_updated: 0,
        changed: Vec::new(),
        dangling: Vec::new(),
        resolved: 0,
        dangling_views: Vec::new(),
        projections: Vec::new(),
    };
    let mut contexts = Contexts::new();
    let ids: Vec<ViewId> = drawing.sheets.iter().flat_map(|s| &s.views).map(|v| v.id).collect();
    let mut failed: Vec<ViewId> = Vec::new();
    for id in ids {
        let view = drawing.view(id).expect("listed");
        if view.excluded_from_update {
            continue;
        }
        if view.parent().is_some_and(|p| failed.contains(&p)) {
            failed.push(id);
            report
                .dangling_views
                .push(DanglingView { view: id, cause: "the view it is made from could not be updated".into() });
            continue;
        }
        if let Err(e) = project::compute_in(drawing, part, &index, id, &mut contexts) {
            if !may_dangle(view) {
                return Err(e);
            }
            failed.push(id);
            report.dangling_views.push(DanglingView { view: id, cause: view_cause(&e) });
            continue;
        }
        let ctx = &contexts[&id];
        follow_parent(drawing, id, &contexts);
        let view = drawing.view_mut(id).expect("listed");
        // Certified line work replaces a preview of the same revision, never the reverse.
        if view.geometry.source_digest != part.digest.as_str()
            || (view.geometry.unproved && !ctx.projected.geometry.unproved)
        {
            view.geometry = ctx.projected.geometry.clone();
            report.views_updated += 1;
        }
        report.projections.push((id, ctx.projected.clone()));
    }
    for sheet in &mut drawing.sheets {
        let symbols = library::template(sheet.template.as_str()).style_file().symbols.clone();
        for a in &mut sheet.annotations {
            let Some(ctx) = contexts.get(&a.view) else { continue };
            if failed.contains(&a.view) {
                continue;
            }
            if let Some(change) = re_resolve(a, &index, ctx, &symbols) {
                report.changed.push(change);
            }
        }
    }
    collect(drawing, &mut report);
    drawing.model.pinned = part.digest.clone();
    Ok(report)
}

/// True for views made from something that can go away: derived views, and views with
/// a crop or broken-out section.
fn may_dangle(view: &crate::View) -> bool {
    view.parent().is_some_and(|_| !matches!(view.kind, ViewKind::Projected { .. }))
        || view.crop.is_some()
        || !view.broken_out.is_empty()
}

/// Why a view could not be made, in words for the Needs attention panel.
fn view_cause(e: &DrawingError) -> String {
    match e {
        DrawingError::LostReference(why) => {
            let mut c = why.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }
        DrawingError::Kernel(k) => format!("The part cannot be cut there any more: {k}"),
        other => other.to_string(),
    }
}

/// Redraws the marks a derived view keeps on its parent from where they are on the
/// part: a section's cutting line and a detail's centre.
pub(crate) fn follow_parent(drawing: &mut Drawing, id: ViewId, contexts: &Contexts) {
    let Some(view) = drawing.view(id) else { return };
    let Some(pctx) = view.parent().and_then(|p| contexts.get(&p)) else { return };
    let xf = pctx.xf;
    let view = drawing.view_mut(id).expect("found");
    match &mut view.kind {
        ViewKind::Section { line, model_line, .. } if model_line.len() == line.len() => {
            for (p, m) in line.iter_mut().zip(model_line.iter()) {
                *p = xf.point(*m);
            }
        }
        ViewKind::Detail { center, profile, model_center: Some(m), .. } => {
            let to = xf.point(*m);
            let (du, dv) = (to.u - center.u, to.v - center.v);
            *center = to;
            for p in profile {
                *p = Point2::new(round(p.u + du), round(p.v + dv));
            }
        }
        _ => {}
    }
}

/// Points an auxiliary view at another edge, the repair for a lost reference edge, and
/// remakes its line work.
pub fn reattach_view(drawing: &mut Drawing, part: &Part, id: ViewId, entity: EntityRef) -> Result<(), DrawingError> {
    let view = drawing.view_mut(id).ok_or(DrawingError::UnknownView(id))?;
    let ViewKind::Auxiliary { edge, .. } = &mut view.kind else {
        return Err(DrawingError::BadRequest(format!("{id} is not an auxiliary view")));
    };
    *edge = entity;
    regenerate_view(drawing, part, id)
}

/// Remakes one view's line work from the part, keeping everything else about it.
pub fn regenerate_view(drawing: &mut Drawing, part: &Part, id: ViewId) -> Result<(), DrawingError> {
    let index = PartIndex::build(part)?;
    let mut contexts = Contexts::new();
    project::compute_in(drawing, part, &index, id, &mut contexts)?;
    follow_parent(drawing, id, &contexts);
    let geometry = contexts[&id].projected.geometry.clone();
    drawing.view_mut(id).ok_or(DrawingError::UnknownView(id))?.geometry = geometry;
    Ok(())
}

fn collect(drawing: &Drawing, report: &mut UpdateReport) {
    for a in drawing.sheets.iter().flat_map(|s| &s.annotations) {
        match &a.state {
            AnnotationState::Dangling { lost, cause } => report.dangling.push(DanglingAnnotation {
                annotation: a.id,
                view: a.view,
                lost: lost.clone(),
                cause: cause.clone(),
            }),
            _ if !a.anchors.is_empty() => report.resolved += 1,
            _ => {}
        }
    }
}

/// Re-resolves one annotation; returns the change when its displayed value moved.
pub(crate) fn re_resolve(
    a: &mut Annotation,
    index: &PartIndex,
    ctx: &ViewContext,
    symbols: &Symbols,
) -> Option<ChangedValue> {
    if a.anchors.is_empty() {
        return None;
    }
    let found: Vec<Result<Entity, String>> = a
        .anchors
        .iter()
        .map(|anchor| match index.resolve(&anchor.entity) {
            Resolution::Found(e) => Ok(e),
            Resolution::Lost(why) => Err(why),
        })
        .collect();
    let lost: Vec<usize> = found.iter().enumerate().filter(|(_, r)| r.is_err()).map(|(i, _)| i).collect();
    if !lost.is_empty() {
        let refs: Vec<&EntityRef> = lost.iter().map(|i| &a.anchors[*i].entity).collect();
        let causes: Vec<String> = lost.iter().filter_map(|i| found[*i].clone().err()).collect();
        a.state = AnnotationState::Dangling { lost: lost.clone(), cause: cause(a.anchors.len(), &refs, &causes) };
        return None;
    }
    let entities: Vec<Entity> = found.into_iter().map(|r| r.expect("no losses")).collect();
    for (anchor, e) in a.anchors.iter_mut().zip(&entities) {
        if let Some(r) = index.entity_ref(*e) {
            anchor.entity = r;
        }
    }
    remeasure(a, index, ctx, &entities, symbols)
}

/// Re-caches the anchors' points and re-measures a dimension whose anchors all resolve to
/// `entities`; sets the state and returns the change when the displayed value moved.
pub(crate) fn remeasure(
    a: &mut Annotation,
    index: &PartIndex,
    ctx: &ViewContext,
    entities: &[Entity],
    symbols: &Symbols,
) -> Option<ChangedValue> {
    let geometry: Vec<AnchorGeometry> = a
        .anchors
        .iter()
        .zip(entities)
        .map(|(anchor, e)| AnchorGeometry {
            curve: match e {
                Entity::Edge(id) => index.edge_curve(*id),
                Entity::Face(_) => None,
            },
            point: anchor.entity.fingerprint.point,
            attach: anchor.attach,
        })
        .collect();
    let (before_state, mut change) = (a.state.clone(), None);
    match &mut a.body {
        AnnotationKind::Dimension(d) => {
            let across = extension_direction(&d.kind, Default::default(), Default::default());
            for (anchor, g) in a.anchors.iter_mut().zip(&geometry) {
                anchor.cached = ctx.xf.point(anchor_point(g, &ctx.xf, across));
            }
            let old = value_text(d);
            if let (DimValue::Measured { value }, Some(new)) =
                (&mut d.value, measure(&d.kind, d.dim_type, &geometry, &ctx.xf))
            {
                *value = crate::model_items::round_value(new);
            }
            let now = value_text(d);
            let accepted = match &before_state {
                AnnotationState::Changed { previous } => previous.clone(),
                _ => old.clone(),
            };
            a.state =
                if now == accepted { AnnotationState::Ok } else { AnnotationState::Changed { previous: accepted } };
            if now != old {
                change = Some(ChangedValue { annotation: a.id, view: a.view, previous: old, now });
            }
        }
        AnnotationKind::DimensionSet(set) => {
            let across = match set.orientation {
                crate::LinearOrientation::Vertical => Point2::new(1.0, 0.0),
                _ => Point2::new(0.0, 1.0),
            };
            for (anchor, g) in a.anchors.iter_mut().zip(&geometry) {
                anchor.cached = ctx.xf.point(anchor_point(g, &ctx.xf, across));
            }
            let old = set.values_text();
            if let Some(values) = crate::measure::measure_set(set, &geometry, &ctx.xf) {
                set.values = values.into_iter().map(crate::model_items::round_value).collect();
            }
            let now = set.values_text();
            let accepted = match &before_state {
                AnnotationState::Changed { previous } => previous.clone(),
                _ => old.clone(),
            };
            a.state =
                if now == accepted { AnnotationState::Ok } else { AnnotationState::Changed { previous: accepted } };
            if now != old {
                change = Some(ChangedValue { annotation: a.id, view: a.view, previous: old, now });
            }
        }
        body => {
            for (anchor, g) in a.anchors.iter_mut().zip(&geometry) {
                anchor.cached = ctx.xf.point(anchor_point(g, &ctx.xf, Default::default()));
            }
            a.state = AnnotationState::Ok;
            match body {
                AnnotationKind::CentreMark => {
                    if let Some(anchor) = a.anchors.first() {
                        a.placement = anchor.cached;
                    }
                }
                AnnotationKind::HoleCallout { text } => {
                    let circle = entities.first().and_then(|e| match e {
                        Entity::Edge(id) => {
                            ctx.projected.circles.iter().find(|c| c.entity.kernel_hint == Some(id.raw()))
                        }
                        Entity::Face(_) => None,
                    });
                    let fresh = circle.and_then(|c| index.namer()?.hole_callout(c, &ctx.projected, symbols));
                    if let Some(fresh) = fresh
                        && fresh != *text
                    {
                        let previous = std::mem::replace(text, fresh.clone());
                        a.state = AnnotationState::Changed { previous: previous.clone() };
                        change = Some(ChangedValue { annotation: a.id, view: a.view, previous, now: fresh });
                    }
                }
                _ => {}
            }
        }
    }
    change
}

/// The Needs attention text for lost references, such as "Both references were edges of
/// slot, which no longer exists".
fn cause(total: usize, lost: &[&EntityRef], causes: &[String]) -> String {
    let gone: Vec<String> = lost.iter().map(|r| feature_name(r)).collect();
    let all_missing = causes.iter().all(|c| c.ends_with("which no longer exists"));
    let kind = |r: &EntityRef| match r.kind {
        EntityKind::Edge => "edge",
        EntityKind::Face => "face",
        EntityKind::Vertex => "vertex",
    };
    if all_missing {
        let mut features: Vec<String> = gone.clone();
        features.sort_unstable();
        features.dedup();
        let names = features.join(" and ");
        let exists = if features.len() == 1 { "exists" } else { "exist" };
        return match (lost.len(), total) {
            (1, 1) => format!("The reference was an {} of {names}, which no longer {exists}", kind(lost[0])),
            (2, 2) => format!("Both references were {}s of {names}, which no longer {exists}", kind(lost[0])),
            (n, t) if n == t => format!("All {n} references were on {names}, which no longer {exists}"),
            (n, t) => format!("{n} of {t} references were on {names}, which no longer {exists}"),
        };
    }
    let mut unique: Vec<&String> = causes.iter().collect();
    unique.dedup();
    let text = unique.into_iter().cloned().collect::<Vec<_>>().join("; ");
    let mut chars = text.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => text,
    }
}

/// Attaches end `end` of annotation `id` to `entity` and, once nothing is lost,
/// re-measures it: the Reattach repair.
pub fn reattach(
    drawing: &mut Drawing,
    part: &Part,
    id: AnnotationId,
    end: usize,
    entity: EntityRef,
) -> Result<(), DrawingError> {
    let index = PartIndex::build(part)?;
    let view = drawing.annotation(id).ok_or(DrawingError::UnknownAnnotation(id))?.view;
    let ctx = ViewContext::compute(drawing, part, &index, view)?;
    let template = drawing.sheet_of(view).map(|s| s.template.as_str()).unwrap_or("");
    let symbols = library::template(template).style_file().symbols.clone();
    let a = drawing.annotation_mut(id).ok_or(DrawingError::UnknownAnnotation(id))?;
    let anchor = a.anchors.get_mut(end).ok_or(DrawingError::BadRequest(format!("{id} has no end {end}")))?;
    anchor.entity = entity;
    // A round takes its centre, a line its middle, unless the end picks a side.
    if let Resolution::Found(Entity::Edge(edge)) = index.resolve(&anchor.entity)
        && let Some(curve) = index.edge_curve(edge)
    {
        let round = crate::measure::radius_of(curve).is_some();
        if round && !matches!(anchor.attach, Attach::Min | Attach::Max) {
            anchor.attach = Attach::Centre;
        } else if !round && matches!(anchor.attach, Attach::Centre | Attach::Silhouette) {
            anchor.attach = Attach::Mid;
        }
        let g = AnchorGeometry { curve: Some(curve), point: anchor.entity.fingerprint.point, attach: anchor.attach };
        anchor.cached = ctx.xf.point(anchor_point(&g, &ctx.xf, Default::default()));
    }
    let resolved: Vec<Option<Entity>> = a
        .anchors
        .iter()
        .map(|x| match index.resolve(&x.entity) {
            Resolution::Found(e) => Some(e),
            Resolution::Lost(_) => None,
        })
        .collect();
    if resolved[end].is_none() {
        return Err(DrawingError::BadRequest(format!("the new reference for end {end} of {id} does not resolve")));
    }
    if let AnnotationState::Dangling { lost, cause } = &a.state {
        let lost: Vec<usize> = lost.iter().copied().filter(|i| *i != end && resolved[*i].is_none()).collect();
        if !lost.is_empty() {
            a.state = AnnotationState::Dangling { lost, cause: cause.clone() };
            return Ok(());
        }
    }
    let entities: Vec<Entity> = resolved.into_iter().collect::<Option<_>>().unwrap_or_default();
    if entities.len() == a.anchors.len() {
        // Keep the dimension's last accepted value as the comparison point.
        if let AnnotationState::Dangling { .. } = a.state {
            a.state = AnnotationState::Ok;
        }
        remeasure(a, &index, &ctx, &entities, &symbols);
        // Reattaching is a deliberate edit: the value it gives is the accepted one.
        if let AnnotationState::Changed { .. } = a.state {
            a.state = AnnotationState::Ok;
        }
    }
    Ok(())
}

/// The name of the edge nearest a model point, for picking a reattach target without a
/// window.
pub fn edge_near(part: &Part, at: soliddraft_kernel_api::Vec3) -> Result<EntityRef, DrawingError> {
    let index = PartIndex::build(part)?;
    let (id, _) = index
        .edges()
        .map(|(id, c)| {
            (id, c.sample(0.0).windows(2).map(|w| segment_distance(at, w[0], w[1])).fold(f64::INFINITY, f64::min))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .ok_or(DrawingError::BadRequest("the part has no edges".into()))?;
    index.edge_ref(id).ok_or(DrawingError::BadRequest("edge vanished".into()))
}

fn segment_distance(
    p: soliddraft_kernel_api::Vec3,
    a: soliddraft_kernel_api::Vec3,
    b: soliddraft_kernel_api::Vec3,
) -> f64 {
    let ab = b - a;
    let len2 = ab.dot(ab);
    let t = if len2 > 0.0 { ((p - a).dot(ab) / len2).clamp(0.0, 1.0) } else { 0.0 };
    p.distance(a + ab * t)
}
