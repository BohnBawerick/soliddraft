//! Phase D1 without the C++ kernel: making a drawing, Model items, the associativity
//! update and its repairs, view alignment and scale, the file and the exporters, on a
//! kernel of boxes.

mod support;

use soliddraft_drawing::display::{Item, Role, layout_sheet};
use soliddraft_drawing::export::{self, PdfInfo};
use soliddraft_drawing::file::{self, FileMeta};
use soliddraft_drawing::{
    AlignAxis, AnnotationKind, AnnotationState, Command, ContentHash, DimValue, DocKind, DocRef, DocumentId, Drawing,
    DrawingError, DrawingMeta, HiddenLines, ImportReport, NewDrawing, Part, Projection, Scale, Template, UndoStack,
    ViewId, ViewKind, ViewScale, apply, update_from_model,
};
use soliddraft_kernel_api::{Point2, Vec3};
use support::{BoxKernel, box_part};

fn new_drawing(template: &str) -> NewDrawing {
    let template = Template::by_id(template).unwrap();
    NewDrawing {
        views: soliddraft_drawing::default_views(&template),
        template,
        model: DocRef::new(DocumentId::new(), DocKind::Part, "box.json", ContentHash::of_bytes(b"")),
        meta: DrawingMeta { title: "Box".into(), ..Default::default() },
        hidden: HiddenLines::Visible,
        model_items: true,
        centre_marks: false,
        scale: None,
    }
}

fn make_drawing(part: &Part, new: &NewDrawing) -> Result<(Drawing, ImportReport), DrawingError> {
    soliddraft_drawing::make_drawing(part, new).map(|m| (m.drawing, m.import))
}

fn values(d: &Drawing) -> Vec<(u32, f64)> {
    let mut v: Vec<(u32, f64)> = d.sheets[0]
        .annotations
        .iter()
        .filter_map(|a| match &a.body {
            AnnotationKind::Dimension(dim) => match dim.value {
                DimValue::Measured { value } => Some((a.id.0, value)),
                _ => None,
            },
            _ => None,
        })
        .collect();
    v.sort_by_key(|a| a.0);
    v
}

#[test]
fn a_drawing_places_aligned_views_and_dimensions_each_once() {
    let k = BoxKernel::new();
    let part = box_part(&k, "block", Vec3::new(40.0, 30.0, 10.0));
    let (d, report) = make_drawing(&part, &new_drawing("iso-a3")).unwrap();
    let sheet = &d.sheets[0];
    assert_eq!(sheet.views.len(), 4);
    assert_eq!(sheet.scale, Scale::new(2, 1), "a small part goes up to 2:1 on A3");
    let [front, top, left, iso] = [1, 2, 3, 4].map(|i| d.view(ViewId(i)).unwrap());
    assert!(matches!(front.kind, ViewKind::Base { .. }));
    assert!(matches!(top.kind, ViewKind::Projected { parent: ViewId(1), .. }));
    // First angle: the top view goes below the front view, the left view to its right.
    assert_eq!(top.alignment.unwrap().axis, AlignAxis::Vertical);
    assert_eq!(top.position.u, front.position.u);
    assert!(top.position.v < front.position.v);
    assert_eq!(left.position.v, front.position.v);
    assert!(left.position.u > front.position.u);
    assert!(matches!(iso.scale, ViewScale::Sheet | ViewScale::Custom { .. }));
    assert!(iso.position.u > left.position.u, "the pictorial view goes top right");
    // Width and height on the top view, depth on the front view; the repeat is dropped.
    assert_eq!((report.placed, report.duplicates), (3, 1), "{report:?}");
    assert_eq!(values(&d).iter().map(|v| v.1).collect::<Vec<_>>(), [40.0, 30.0, 10.0]);
    let on = |id: u32| d.sheets[0].annotations.iter().find(|a| a.id.0 == id).unwrap().view;
    assert_eq!((on(1), on(2), on(3)), (ViewId(2), ViewId(2), ViewId(1)));
    // Everything lies on the sheet, clear of the title block.
    let tb = &Template::by_id("iso-a3").unwrap().title_block;
    for v in &sheet.views {
        let [x0, y0, x1, y1] = v.geometry.bbox;
        let (x0, y0, x1, y1) = (x0 + v.position.u, y0 + v.position.v, x1 + v.position.u, y1 + v.position.v);
        assert!(x0 > 20.0 && y0 > 10.0 && x1 < 410.0 && y1 < 287.0, "view {} off the sheet", v.id.0);
        assert!(x1 < tb.position.u || y0 > tb.position.v + tb.height, "view {} on the title block", v.id.0);
    }
}

#[test]
fn third_angle_puts_the_top_view_above_and_the_right_view_right() {
    let k = BoxKernel::new();
    let part = box_part(&k, "block", Vec3::new(40.0, 30.0, 10.0));
    let (d, _) = make_drawing(&part, &new_drawing("ansi-b")).unwrap();
    assert_eq!(d.sheets[0].projection, Projection::ThirdAngle);
    let [front, top, right] = [1, 2, 3].map(|i| d.view(ViewId(i)).unwrap());
    assert!(top.position.v > front.position.v && top.position.u == front.position.u);
    assert!(right.position.u > front.position.u && right.position.v == front.position.v);
}

#[test]
fn an_update_reports_changed_values_and_keeps_them_highlighted_until_accepted() {
    let k = BoxKernel::new();
    let part = box_part(&k, "block", Vec3::new(40.0, 30.0, 10.0));
    let (mut d, _) = make_drawing(&part, &new_drawing("iso-a3")).unwrap();
    let same = update_from_model(&mut d, &part).unwrap();
    assert!(same.changed.is_empty() && same.dangling.is_empty() && same.views_updated == 0, "{same:?}");

    let thicker = box_part(&k, "block", Vec3::new(40.0, 30.0, 12.0));
    let mut stack = UndoStack::new();
    let before = d.clone();
    let r = stack.run(&mut d, &Command::UpdateFromModel, Some(&thicker)).unwrap().unwrap();
    assert_eq!(r.views_updated, 4);
    assert_eq!(r.changed.len(), 1, "{r:?}");
    assert_eq!((r.changed[0].previous.as_str(), r.changed[0].now.as_str()), ("10", "12"));
    assert!(r.dangling.is_empty());
    assert_eq!(d.model.pinned, thicker.digest, "the update re-pins the drawing");
    let changed = d.annotation(r.changed[0].annotation).unwrap();
    assert_eq!(changed.state, AnnotationState::Changed { previous: "10".into() });
    let list = layout_sheet(&d, 0);
    assert!(list.items.iter().any(|i| matches!(i, Item::Text(t) if t.text == "12")));
    assert!(list.items.iter().any(|i| i.role() == Role::Changed), "the changed value is highlighted");

    // The update undoes as one step, geometry and references with it.
    assert_eq!(stack.undo(&mut d), Some("Update from model"));
    assert_eq!(d, before);
    stack.redo(&mut d);
    apply(&mut d, &Command::AcceptChange { annotation: r.changed[0].annotation }).unwrap();
    assert!(d.annotations_needing_attention().next().is_none());
}

#[test]
fn lost_references_dangle_and_are_repaired_never_deleted() {
    let k = BoxKernel::new();
    let part = box_part(&k, "block", Vec3::new(40.0, 30.0, 10.0));
    let (mut d, _) = make_drawing(&part, &new_drawing("iso-a3")).unwrap();
    let before = values(&d);
    let other = box_part(&k, "slab", Vec3::new(40.0, 30.0, 10.0));
    let r = update_from_model(&mut d, &other).unwrap();
    assert_eq!(r.dangling.len(), 3, "{r:?}");
    assert!(r.dangling[0].cause.contains("block, which no longer exists"), "{}", r.dangling[0].cause);
    assert_eq!(values(&d), before, "dangling dimensions keep their last values");
    assert_eq!(d.sheets[0].annotations.len(), 3, "nothing is deleted");
    let list = layout_sheet(&d, 0);
    assert!(list.items.iter().any(|i| i.role() == Role::Error), "dangling dimensions are drawn as errors");

    // Reattach both ends of the first to edges of the new part; the others go by delete
    // and keep as note.
    let first = d.sheets[0].annotations[0].id;
    for end in 0..2 {
        let at = d.sheets[0].annotations[0].anchors[end].entity.fingerprint.point;
        let entity = soliddraft_drawing::edge_near(&other, at).unwrap();
        let mut stack = UndoStack::new();
        stack.run(&mut d, &Command::Reattach { annotation: first, end, entity }, Some(&other)).unwrap();
    }
    assert_eq!(d.annotation(first).unwrap().state, AnnotationState::Ok);
    let (second, third) = (d.sheets[0].annotations[1].id, d.sheets[0].annotations[2].id);
    apply(&mut d, &Command::DeleteAnnotation { annotation: second }).unwrap();
    apply(&mut d, &Command::FreezeAsNote { annotation: third }).unwrap();
    assert!(matches!(&d.annotation(third).unwrap().body, AnnotationKind::Note { text, .. } if text == "10 (ref)"));
    let after = update_from_model(&mut d, &other).unwrap();
    assert!(after.dangling.is_empty() && after.changed.is_empty(), "{after:?}");
    assert!(matches!(apply(&mut d, &Command::UpdateFromModel), Err(DrawingError::NeedsModel(_))));
}

#[test]
fn views_move_along_their_alignment_and_rescale_with_their_parent() {
    let k = BoxKernel::new();
    let part = box_part(&k, "block", Vec3::new(40.0, 30.0, 10.0));
    let (mut d, _) = make_drawing(&part, &new_drawing("iso-a3")).unwrap();
    let (front, top, left) = (ViewId(1), ViewId(2), ViewId(3));
    let p = |d: &Drawing, v: ViewId| d.view(v).unwrap().position;
    let (f0, t0, l0) = (p(&d, front), p(&d, top), p(&d, left));
    // The top view slides only vertically.
    apply(&mut d, &Command::MoveView { view: top, to: Point2::new(t0.u + 50.0, t0.v - 5.0) }).unwrap();
    assert_eq!(p(&d, top), Point2::new(t0.u, t0.v - 5.0));
    // Moving the front view carries its aligned children.
    apply(&mut d, &Command::MoveView { view: front, to: Point2::new(f0.u + 10.0, f0.v + 3.0) }).unwrap();
    assert_eq!(p(&d, left), Point2::new(l0.u + 10.0, l0.v + 3.0));
    apply(&mut d, &Command::BreakAlignment { view: left }).unwrap();
    apply(&mut d, &Command::MoveView { view: left, to: Point2::new(300.0, 100.0) }).unwrap();
    assert_eq!(p(&d, left), Point2::new(300.0, 100.0));

    // Halving the front view's scale halves its children that follow it.
    let width = |d: &Drawing, v: ViewId| {
        let b = d.view(v).unwrap().geometry.bbox;
        b[2] - b[0]
    };
    let (wf, wt) = (width(&d, front), width(&d, top));
    apply(&mut d, &Command::SetViewScale { view: front, scale: ViewScale::Custom { factor: 1.0 } }).unwrap();
    assert!((width(&d, front) - wf / 2.0).abs() < 1e-3 && (width(&d, top) - wt / 2.0).abs() < 1e-3);
    // The dimensions on the top view stay measured in model millimetres.
    assert_eq!(values(&d)[0].1, 40.0);
    let list = layout_sheet(&d, 0);
    assert!(list.items.iter().any(|i| matches!(i, Item::Text(t) if t.text == "SCALE 1:1")));
}

#[test]
fn the_file_and_the_exports_carry_the_drawing() {
    let k = BoxKernel::new();
    let part = box_part(&k, "block", Vec3::new(40.0, 30.0, 10.0));
    let (d, _) = make_drawing(&part, &new_drawing("iso-a3")).unwrap();
    let bytes = file::to_bytes(&d, &FileMeta::new("boxes test", d.meta.units)).unwrap();
    let (back, meta) = file::from_bytes(&bytes).unwrap();
    assert_eq!(back, d);
    assert_eq!(meta.kernel, "boxes test");

    let list = layout_sheet(&d, 0);
    for role in [Role::Visible, Role::Hidden, Role::Dimension, Role::Border] {
        assert!(list.items.iter().any(|i| i.role() == role), "{role:?}");
    }
    let svg = export::svg(&list);
    assert!(svg.contains(">Box</text>") && svg.contains(">40</text>"));

    let pdf =
        export::pdf(std::slice::from_ref(&list), &PdfInfo { title: "Box".into(), author: String::new() }).unwrap();
    let doc = lopdf::Document::load_mem(&pdf).unwrap();
    assert_eq!(doc.get_pages().len(), 1);
    let text = doc.extract_text(&[1]).unwrap();
    assert!(text.contains("Box") && text.contains("40"), "{text}");

    let dxf = export::dxf(&list).unwrap();
    let back = dxf::Drawing::load(&mut dxf.as_slice()).unwrap();
    assert!(back.entities().all(|e| !matches!(e.specific, dxf::entities::EntityType::Spline(_))));
    assert!(back.entities().any(|e| e.common.layer == "VISIBLE"));
}
