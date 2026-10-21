//! The bracket fixture, serialisation, templates and commands that need no model.

use std::path::PathBuf;

use soliddraft_drawing::fixture::bracket_a3;
use soliddraft_drawing::{
    AnnotationState, Command, ContentHash, DocKind, DocRef, Drawing, DrawingError, Projection, Template, ViewId,
    ViewKind, apply,
};
use soliddraft_kernel_api::Point2;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/bracket-a3.json")
}

/// The checked-in JSON is what the builder makes. Set `SOLIDDRAFT_BLESS=1` to rewrite it.
#[test]
fn fixture_json_matches_builder() {
    let json = bracket_a3().to_json().unwrap() + "\n";
    let path = fixture_path();
    if std::env::var_os("SOLIDDRAFT_BLESS").is_some() {
        std::fs::write(&path, &json).unwrap();
    }
    let on_disk = std::fs::read_to_string(&path).expect("fixture missing; run with SOLIDDRAFT_BLESS=1");
    assert!(on_disk == json, "{} is stale; run with SOLIDDRAFT_BLESS=1", path.display());
}

#[test]
fn json_round_trips() {
    let drawing = bracket_a3();
    let back = Drawing::from_json(&drawing.to_json().unwrap()).unwrap();
    assert_eq!(back, drawing);
    let on_disk = std::fs::read_to_string(fixture_path()).unwrap();
    assert_eq!(Drawing::from_json(&on_disk).unwrap(), drawing);
}

#[test]
fn fixture_has_the_views_and_annotations_a_ui_needs() {
    let d = bracket_a3();
    let sheet = &d.sheets[0];
    assert_eq!(sheet.projection, Projection::FirstAngle);
    assert_eq!(sheet.views.len(), 4);
    assert!(matches!(d.view(ViewId(3)).unwrap().kind, ViewKind::Projected { parent: ViewId(1), .. }));
    assert!(sheet.views.iter().all(|v| v.geometry.curves.iter().any(|c| c.visible)));
    assert!(sheet.views.iter().any(|v| v.geometry.curves.iter().any(|c| !c.visible)));
    let dims = sheet.annotations.iter().filter(|a| matches!(a.body, soliddraft_drawing::AnnotationKind::Dimension(_)));
    assert_eq!(dims.count(), 5);
    let attention: Vec<_> = d.annotations_needing_attention().collect();
    assert_eq!(attention.len(), 2);
    assert!(attention.iter().any(|a| matches!(a.state, AnnotationState::Dangling { .. })));
    assert!(attention.iter().any(|a| matches!(a.state, AnnotationState::Changed { .. })));
    assert_eq!(d.next_view_id(), ViewId(5));
}

/// Every view lies inside the border and clear of the title block.
#[test]
fn fixture_views_fit_the_sheet() {
    let d = bracket_a3();
    let template = Template::by_id("iso-a3").unwrap();
    let (m, size, tb) = (template.margins, template.size, template.title_block);
    for v in &d.sheets[0].views {
        let [x0, y0, x1, y1] = v.geometry.bbox;
        let (x0, y0, x1, y1) = (x0 + v.position.u, y0 + v.position.v, x1 + v.position.u, y1 + v.position.v);
        assert!(x0 > m.left && y0 > m.bottom && x1 < size.width_mm - m.right && y1 < size.height_mm - m.top);
        let clear_of_title = x1 < tb.position.u || y0 > tb.position.v + tb.height;
        assert!(clear_of_title, "view {} overlaps the title block", v.id.0);
    }
}

#[test]
fn builtin_templates() {
    let all = Template::builtin();
    let ids: Vec<_> = all.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["iso-a3", "iso-a4", "ansi-a", "ansi-b"]);
    assert_eq!(all[0].projection, Projection::FirstAngle);
    assert_eq!(all[3].projection, Projection::ThirdAngle);
    assert_eq!((all[2].size.width_mm, all[2].size.height_mm), (279.4, 215.9));
    assert!(Template::by_id("nope").is_none());
    for t in &all {
        assert!(t.title_block.fields.iter().any(|f| f.link == "${title}"));
    }
}

#[test]
fn new_drawing_has_one_empty_sheet() {
    let model = DocRef::unbound(DocKind::Part, "p.json", ContentHash::of_bytes(b"d"));
    let d = Drawing::new(model, &Template::by_id("ansi-b").unwrap());
    assert_eq!(d.sheets.len(), 1);
    assert_eq!(d.sheets[0].projection, Projection::ThirdAngle);
    assert_eq!(d.sheets[0].next_view_id(), ViewId(1));
    assert!(d.settings.automatic_update);
}

#[test]
fn apply_moves_views_and_asks_for_the_model_where_it_needs_one() {
    let mut d = bracket_a3();
    let to = Point2::new(120.0, 200.0);
    apply(&mut d, &Command::MoveView { view: ViewId(1), to }).unwrap();
    assert_eq!(d.view(ViewId(1)).unwrap().position, to);
    let missing = apply(&mut d, &Command::MoveView { view: ViewId(99), to });
    assert!(matches!(missing, Err(DrawingError::UnknownView(ViewId(99)))));
    let update = apply(&mut d, &Command::UpdateFromModel);
    assert!(matches!(update, Err(DrawingError::NeedsModel(_))));
}

#[test]
fn commands_serialise_with_a_type_tag() {
    let c = Command::BreakAlignment { view: ViewId(2) };
    let json = serde_json::to_string(&c).unwrap();
    assert_eq!(json, r#"{"type":"break_alignment","view":2}"#);
    assert_eq!(serde_json::from_str::<Command>(&json).unwrap(), c);
}
