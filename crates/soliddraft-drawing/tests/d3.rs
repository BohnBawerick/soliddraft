//! Phase D3 without the C++ kernel, on a kernel of boxes: detail and auxiliary views,
//! crops, dimension sets, symbols, revisions, several sheets with location labels, and
//! DXF dimensions and hatches. Sections need booleans; the `soliddraft draw` exit test
//! covers them on SMLib.

mod support;

use soliddraft_drawing::display::{Item, Role, layout_sheet, location_labels};
use soliddraft_drawing::export;
use soliddraft_drawing::{
    AnnotationKind, AnnotationState, Boundary, Command, ContentHash, DatumRef, DocKind, DocRef, DocumentId, Drawing,
    DrawingMeta, GdtCharacteristic, HiddenLines, LinearOrientation, MaterialCondition, NewDrawing, Part, PartIndex,
    Polygon, RevisionRow, SetKind, SurfaceProcess, Template, UndoStack, ViewId, ViewKind, annotate, apply, derived,
    edge_near, next_letter, project, update_from_model,
};
use soliddraft_kernel_api::{Point2, Vec3};
use support::{BoxKernel, box_part};

const FRONT: ViewId = ViewId(1);

fn new_drawing() -> NewDrawing {
    let template = Template::by_id("iso-a3").unwrap();
    NewDrawing {
        views: soliddraft_drawing::default_views(&template),
        template,
        model: DocRef::new(DocumentId::new(), DocKind::Part, "box.json", ContentHash::of_bytes(b"")),
        meta: DrawingMeta { title: "Box".into(), ..Default::default() },
        hidden: HiddenLines::Visible,
        model_items: false,
        centre_marks: false,
        scale: None,
    }
}

fn drawing(part: &Part) -> Drawing {
    soliddraft_drawing::make_drawing(part, &new_drawing()).unwrap().drawing
}

fn texts(d: &Drawing, sheet: usize) -> Vec<String> {
    layout_sheet(d, sheet)
        .items
        .into_iter()
        .filter_map(|i| match i {
            Item::Text(t) => Some(t.text),
            _ => None,
        })
        .collect()
}

fn run(stack: &mut UndoStack, d: &mut Drawing, part: &Part, c: Command) {
    stack.run(d, &c, Some(part)).unwrap_or_else(|e| panic!("{}: {e}", c.name()));
}

#[test]
fn letters_skip_i_o_and_q() {
    assert_eq!(next_letter("A"), "B");
    assert_eq!(next_letter("H"), "J");
    assert_eq!(next_letter("N"), "P");
    assert_eq!(next_letter("P"), "R");
    assert_eq!(next_letter("Z"), "AA");
}

#[test]
fn a_detail_magnifies_inside_its_circle_and_stays_on_the_part() {
    let k = BoxKernel::new();
    let part = box_part(&k, "block", Vec3::new(40.0, 30.0, 10.0));
    let mut d = drawing(&part);
    let index = PartIndex::build(&part).unwrap();
    let ctx = derived::context(&d, &part, &index, FRONT).unwrap();
    // The front view's top right corner.
    let corner = Vec3::new(40.0, 0.0, 10.0);
    let (view, _) =
        derived::new_detail(&d, &part, &index, 0, FRONT, ctx.xf.point(corner), 5.0, Vec::new(), 2.0, None).unwrap();
    let id = view.id;
    let mut stack = UndoStack::new();
    run(&mut stack, &mut d, &part, Command::PlaceView { sheet: 0, view });
    assert_eq!(d.next_view_label, "B", "placing detail A moves the letter on");
    let scale = project::view_scale(&d, id).unwrap();
    assert!((scale - 2.0 * ctx.xf.scale).abs() < 1e-12, "a detail follows its parent's scale times its factor");
    let v = d.view(id).unwrap();
    let Some(Boundary::Circle { radius, .. }) = &v.geometry.outline else { panic!("{:?}", v.geometry.outline) };
    assert!((radius - 10.0).abs() < 1e-9);
    for c in &v.geometry.curves {
        for poly in soliddraft_drawing::display::flatten(&c.path, 0.01) {
            for p in poly {
                assert!(p.x.hypot(p.y) <= 10.0 + 1e-6, "{p:?} is outside the detail circle");
            }
        }
    }
    assert!(!v.geometry.curves.is_empty(), "the corner's edges are in the detail");
    let shown = texts(&d, 0);
    assert!(shown.contains(&"DETAIL A".to_owned()), "{shown:?}");
    assert!(shown.iter().any(|t| t.starts_with("SCALE ")), "{shown:?}");
    assert!(layout_sheet(&d, 0).items.iter().any(|i| i.role() == Role::Boundary), "the circle on the front view");

    // The part grows: the circle stays on the same corner of the part.
    let wider = box_part(&k, "block", Vec3::new(60.0, 30.0, 10.0));
    let report = update_from_model(&mut d, &wider).unwrap();
    assert!(report.dangling_views.is_empty(), "{report:?}");
    let index = PartIndex::build(&wider).unwrap();
    let ctx = derived::context(&d, &wider, &index, FRONT).unwrap();
    let ViewKind::Detail { center, .. } = &d.view(id).unwrap().kind else { panic!() };
    assert_eq!(*center, ctx.xf.point(corner));
}

#[test]
fn an_auxiliary_view_looks_square_to_its_edge_and_dangles_when_it_goes() {
    let k = BoxKernel::new();
    let part = box_part(&k, "block", Vec3::new(40.0, 30.0, 10.0));
    let mut d = drawing(&part);
    let index = PartIndex::build(&part).unwrap();
    // The front view's right edge: the view looks at it from the right.
    let edge = edge_near(&part, Vec3::new(40.0, 0.0, 5.0)).unwrap();
    let (view, ctx) = derived::new_auxiliary(&d, &part, &index, 0, FRONT, edge, false, None).unwrap();
    assert!(ctx.xf.frame.direction.distance(Vec3::X) < 1e-9, "{:?}", ctx.xf.frame);
    let id = view.id;
    let mut stack = UndoStack::new();
    run(&mut stack, &mut d, &part, Command::PlaceView { sheet: 0, view });
    assert!(texts(&d, 0).contains(&"VIEW A".to_owned()));
    assert!(layout_sheet(&d, 0).items.iter().any(|i| i.role() == Role::CuttingPlane), "the arrow on the front view");

    // A part made by another feature: the edge is gone; the view keeps its line work.
    let other = box_part(&k, "other", Vec3::new(40.0, 30.0, 10.0));
    let before = d.view(id).unwrap().geometry.clone();
    let report = update_from_model(&mut d, &other).unwrap();
    assert_eq!(report.dangling_views.len(), 1, "{report:?}");
    assert_eq!(report.dangling_views[0].view, id);
    assert!(report.dangling_views[0].cause.contains("block"), "{}", report.dangling_views[0].cause);
    assert_eq!(d.view(id).unwrap().geometry, before);
    // Reattach it to the new part's edge.
    let edge = edge_near(&other, Vec3::new(40.0, 0.0, 5.0)).unwrap();
    run(&mut stack, &mut d, &other, Command::ReattachView { view: id, edge });
    assert!(update_from_model(&mut d, &other).unwrap().dangling_views.is_empty());
}

#[test]
fn crops_keep_only_what_is_inside() {
    let k = BoxKernel::new();
    let part = box_part(&k, "block", Vec3::new(40.0, 30.0, 10.0));
    let mut d = drawing(&part);
    let full = d.view(FRONT).unwrap().geometry.curves.len();
    let crop = Boundary::rectangle(Point2::new(-200.0, -200.0), Point2::new(0.0, 200.0));
    let mut stack = UndoStack::new();
    run(&mut stack, &mut d, &part, Command::SetViewCrop { view: FRONT, crop: Some(crop) });
    let v = d.view(FRONT).unwrap();
    assert!(!v.geometry.curves.is_empty());
    for c in &v.geometry.curves {
        assert!(kurbo::Shape::bounding_box(&c.path).x1 <= 1e-6, "a curve right of the crop");
    }
    assert!(v.geometry.bbox[2] <= 1e-6);
    run(&mut stack, &mut d, &part, Command::SetViewCrop { view: FRONT, crop: None });
    assert_eq!(d.view(FRONT).unwrap().geometry.curves.len(), full);
    assert!(matches!(
        apply(&mut d, &Command::SetViewCrop { view: FRONT, crop: None }),
        Err(soliddraft_drawing::DrawingError::NeedsModel(_))
    ));
}

#[test]
fn dimension_sets_measure_and_follow_the_part() {
    let k = BoxKernel::new();
    let part = box_part(&k, "block", Vec3::new(40.0, 30.0, 10.0));
    let mut d = drawing(&part);
    let index = PartIndex::build(&part).unwrap();
    let ctx = derived::context(&d, &part, &index, FRONT).unwrap();
    // The middles of the front face's edges: each names one edge of the box after it
    // grows, where a corner would be shared by three.
    let (left, right) = (Vec3::new(0.0, 0.0, 5.0), Vec3::new(40.0, 0.0, 5.0));
    let (bottom, top) = (Vec3::new(20.0, 0.0, 0.0), Vec3::new(20.0, 0.0, 10.0));
    let mut stack = UndoStack::new();
    let mut ids = Vec::new();
    for (kind, orientation, points) in [
        (SetKind::Ordinate, LinearOrientation::Horizontal, vec![left, right]),
        (SetKind::Baseline, LinearOrientation::Vertical, vec![bottom, top]),
        (SetKind::Chain, LinearOrientation::Horizontal, vec![left, right]),
    ] {
        let a = annotate::dimension_set(&d, &index, &ctx, kind, orientation, &points, 12.0).unwrap();
        ids.push(a.id);
        run(&mut stack, &mut d, &part, Command::AddAnnotation { annotation: a });
    }
    let value = |d: &Drawing, i: usize| match &d.annotation(ids[i]).unwrap().body {
        AnnotationKind::DimensionSet(s) => s.values.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(value(&d, 0), vec![40.0]);
    assert_eq!(value(&d, 1), vec![10.0]);
    assert_eq!(value(&d, 2), vec![40.0]);
    let list = layout_sheet(&d, 0);
    assert_eq!(list.dimensions.len(), 3, "{:?}", list.dimensions);
    assert!(
        list.dimensions.iter().any(|r| matches!(
            r.geometry,
            soliddraft_drawing::display::DimensionGeometry::Ordinate { along_x: true, .. }
        ))
    );
    assert!(texts(&d, 0).contains(&"0".to_owned()), "the ordinate origin is marked 0");

    let wider = box_part(&k, "block", Vec3::new(42.0, 30.0, 10.0));
    let report = update_from_model(&mut d, &wider).unwrap();
    assert_eq!(value(&d, 0), vec![42.0]);
    let changed: Vec<&str> = report.changed.iter().map(|c| c.now.as_str()).collect();
    assert_eq!(changed, ["42", "42"], "{report:?}");
    assert!(matches!(&d.annotation(ids[0]).unwrap().state, AnnotationState::Changed { previous } if previous == "40"));
    assert_eq!(d.annotation(ids[1]).unwrap().state, AnnotationState::Ok);
}

#[test]
fn symbols_revisions_and_their_table() {
    let k = BoxKernel::new();
    let part = box_part(&k, "block", Vec3::new(40.0, 30.0, 10.0));
    let mut d = drawing(&part);
    let index = PartIndex::build(&part).unwrap();
    let ctx = derived::context(&d, &part, &index, FRONT).unwrap();
    let mut stack = UndoStack::new();
    let top_edge = Vec3::new(20.0, 0.0, 10.0);
    let bodies = [
        AnnotationKind::Gdt {
            characteristic: GdtCharacteristic::Position,
            tolerance: 0.1,
            diameter: true,
            material: MaterialCondition::Maximum,
            datums: vec![DatumRef::new("A"), DatumRef { letter: "B".into(), material: MaterialCondition::Least }],
        },
        AnnotationKind::Datum { letter: d.next_datum_label.clone() },
        AnnotationKind::SurfaceFinish {
            text: "Ra 1.6".into(),
            process: SurfaceProcess::Removal,
            method: "Ground".into(),
            lay: Some(soliddraft_drawing::Lay::Perpendicular),
            allowance: String::new(),
            all_around: false,
        },
        AnnotationKind::RevisionSymbol { revision: "B".into() },
    ];
    let mut gdt = None;
    for body in bodies {
        let a = annotate::attached(&d, &index, &ctx, top_edge, body, Point2::new(10.0, 10.0)).unwrap();
        gdt.get_or_insert(a.id);
        run(&mut stack, &mut d, &part, Command::AddAnnotation { annotation: a });
    }
    assert_eq!(d.next_datum_label, "B", "datum A moves the letter on");
    for (rev, what) in [("A", "First issue"), ("B", "Wider top")] {
        let row = RevisionRow {
            revision: rev.into(),
            description: what.into(),
            date: "2026-10-14".into(),
            ..Default::default()
        };
        run(&mut stack, &mut d, &part, Command::AddRevision { sheet: 0, row });
    }
    assert_eq!(d.meta.revision, "B");
    let shown = texts(&d, 0);
    for t in ["Ø0.1", "M", "L", "Ra 1.6", "Ground", "⊥", "REVISIONS", "First issue", "Wider top"] {
        assert!(shown.contains(&t.to_owned()), "{t} not on the sheet: {shown:?}");
    }
    // The revision table lists the zone of revision B's symbol.
    let zone = soliddraft_drawing::display::zone_at(
        soliddraft_drawing::library::template("iso-a3"),
        Point2::new(
            d.view(FRONT).unwrap().position.u + ctx.xf.point(top_edge).u + 10.0,
            d.view(FRONT).unwrap().position.v + ctx.xf.point(top_edge).v + 10.0,
        ),
    );
    assert!(shown.contains(&zone), "zone {zone} not in {shown:?}");
    // Symbols follow the part like any annotation, and edit in place.
    let report = update_from_model(&mut d, &box_part(&k, "block", Vec3::new(40.0, 30.0, 12.0))).unwrap();
    assert!(report.dangling.is_empty(), "{report:?}");
    let gdt = gdt.unwrap();
    let edited = AnnotationKind::Gdt {
        characteristic: GdtCharacteristic::Flatness,
        tolerance: 0.05,
        diameter: false,
        material: MaterialCondition::Regardless,
        datums: Vec::new(),
    };
    apply(&mut d, &Command::EditAnnotation { annotation: gdt, body: edited }).unwrap();
    assert!(apply(&mut d, &Command::EditAnnotation { annotation: gdt, body: AnnotationKind::CentreMark }).is_err());
    apply(&mut d, &Command::FreezeAsNote { annotation: gdt }).unwrap();
    assert!(
        matches!(&d.annotation(gdt).unwrap().body, AnnotationKind::Note { text, .. } if text == "Flatness 0.05 (ref)")
    );
}

#[test]
fn a_detail_on_sheet_two_and_its_parent_point_at_each_other() {
    let k = BoxKernel::new();
    let part = box_part(&k, "block", Vec3::new(40.0, 30.0, 10.0));
    let mut d = drawing(&part);
    let mut stack = UndoStack::new();
    run(&mut stack, &mut d, &part, Command::AddSheet { name: "Sheet2".into(), template: "iso-a3".into() });
    let index = PartIndex::build(&part).unwrap();
    let ctx = derived::context(&d, &part, &index, FRONT).unwrap();
    let (view, _) = derived::new_detail(
        &d,
        &part,
        &index,
        1,
        FRONT,
        ctx.xf.point(Vec3::new(40.0, 0.0, 10.0)),
        5.0,
        Vec::new(),
        2.0,
        None,
    )
    .unwrap();
    let id = view.id;
    run(&mut stack, &mut d, &part, Command::PlaceView { sheet: 1, view });
    let labels = location_labels(&d);
    assert_eq!(labels.len(), 1, "{labels:?}");
    let l = &labels[0];
    assert_eq!((l.view, l.parent, l.view_sheet, l.parent_sheet), (id, FRONT, 1, 0));
    let (on_parent, on_view) = (l.on_parent().unwrap(), l.on_view().unwrap());
    assert_eq!(on_parent, format!("SEE SHEET 2, ZONE {}", l.view_zone));
    assert_eq!(on_view, format!("FROM SHEET 1, ZONE {}", l.parent_zone));
    assert!(texts(&d, 0).contains(&on_parent), "the parent's sheet says where the detail is");
    assert!(texts(&d, 1).contains(&on_view), "the detail's sheet says where it comes from");
    assert!(texts(&d, 1).contains(&"DETAIL A".to_owned()));
    // The title block counts both sheets.
    assert!(texts(&d, 1).iter().any(|t| t == "2 / 2"), "{:?}", texts(&d, 1));
    // The parent's sheet cannot go while the detail is made from it.
    assert!(apply(&mut d, &Command::DeleteSheet { sheet: 0 }).is_err());
    // The update follows across sheets, and the file keeps both sheets.
    let report = update_from_model(&mut d, &box_part(&k, "block", Vec3::new(40.0, 30.0, 12.0))).unwrap();
    assert!(report.dangling_views.is_empty(), "{report:?}");
    let bytes = soliddraft_drawing::file::to_bytes(&d, &soliddraft_drawing::file::FileMeta::new("boxes", d.meta.units))
        .unwrap();
    let (back, _) = soliddraft_drawing::file::from_bytes(&bytes).unwrap();
    assert_eq!(back, d);
    // Moving the detail back to sheet 1 drops the labels.
    apply(&mut d, &Command::MoveViewToSheet { view: id, sheet: 0, to: Point2::new(300.0, 200.0) }).unwrap();
    assert!(location_labels(&d)[0].on_parent().is_none());
    apply(&mut d, &Command::DeleteSheet { sheet: 1 }).unwrap();
    assert_eq!(d.sheets.len(), 1);
}

#[test]
fn the_dxf_keeps_dimensions_and_hatches_as_such() {
    let k = BoxKernel::new();
    let part = box_part(&k, "block", Vec3::new(40.0, 30.0, 10.0));
    let mut d =
        soliddraft_drawing::make_drawing(&part, &NewDrawing { model_items: true, ..new_drawing() }).unwrap().drawing;
    let index = PartIndex::build(&part).unwrap();
    let ctx = derived::context(&d, &part, &index, FRONT).unwrap();
    let set = annotate::dimension_set(
        &d,
        &index,
        &ctx,
        SetKind::Ordinate,
        LinearOrientation::Horizontal,
        &[Vec3::ZERO, Vec3::new(40.0, 0.0, 0.0)],
        12.0,
    )
    .unwrap();
    apply(&mut d, &Command::AddAnnotation { annotation: set }).unwrap();
    // A hatched square on the front view, as a section would have.
    let square = vec![Point2::new(-5.0, -5.0), Point2::new(5.0, -5.0), Point2::new(5.0, 5.0), Point2::new(-5.0, 5.0)];
    d.view_mut(FRONT).unwrap().geometry.hatch.push(Polygon { outer: square, holes: Vec::new(), clip: None });
    let list = layout_sheet(&d, 0);
    assert!(list.items.iter().any(|i| i.role() == Role::Hatch));
    assert_eq!(list.hatches.len(), 1);
    let bytes = export::dxf(&list).unwrap();
    let counts = export::dxf::count_entities(&bytes).unwrap();
    assert_eq!(counts.get("HATCH/HATCH"), Some(&1), "{counts:?}");
    assert!(!counts.contains_key("HATCH/LINE"), "hatch lines go out as a HATCH: {counts:?}");
    let dims = counts.get("DIMENSION/DIMENSION").copied().unwrap_or(0);
    assert_eq!(dims, list.dimensions.len(), "{counts:?}");
    assert!(dims >= 2, "{counts:?}");
    let back = dxf::Drawing::load(&mut bytes.as_slice()).unwrap();
    let ordinate = back.entities().find_map(|e| match &e.specific {
        dxf::entities::EntityType::OrdinateDimension(o) => Some(o),
        _ => None,
    });
    let ordinate = ordinate.expect("the ordinate dimension");
    assert!(ordinate.dimension_base.is_ordinate_x_type);
    assert!((ordinate.dimension_base.actual_measurement - 40.0).abs() < 1e-9);
    assert!(back.blocks().any(|b| b.name == ordinate.dimension_base.block_name));
}
