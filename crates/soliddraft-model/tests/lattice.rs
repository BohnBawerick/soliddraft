//! The terminal lattice stage: it is always last, CAD after it is refused, the user can
//! return before it without losing its settings, and the settings survive save and reopen.

use std::sync::Arc;

use soliddraft_kernel_catallax::CatallaxKernel;
use soliddraft_model::sketch::{P2, Sketch, solve::solve};
use soliddraft_model::{
    Command, CommandError, DatumPlane, Document, EndCondition, ExtrudeFeature, ExtrudeOp, FeatureId, FeatureKind,
    FeatureStatus, LatticeFeature, ProfileSource, SketchFeature, SketchPlane,
};

fn kernel() -> Arc<CatallaxKernel> {
    Arc::new(CatallaxKernel::new())
}

fn doc() -> Document {
    Document::new("Part1", kernel())
}

fn try_add(doc: &mut Document, kind: FeatureKind, position: Option<usize>) -> Result<FeatureId, CommandError> {
    doc.execute(Command::AddFeature { kind, name: None, position, via_assistant: false }).map(|e| e.created.unwrap())
}

fn add(doc: &mut Document, kind: FeatureKind) -> FeatureId {
    try_add(doc, kind, None).unwrap()
}

fn rectangle(w: f64) -> FeatureKind {
    let mut s = Sketch::new();
    s.rectangle(P2::new(0.0, 0.0), w, 10.0).unwrap();
    solve(&mut s, None, true);
    FeatureKind::Sketch(SketchFeature { plane: SketchPlane::Datum { datum: DatumPlane::Top, offset: 0.0 }, sketch: s })
}

fn boss(sketch: FeatureId, depth: f64) -> FeatureKind {
    FeatureKind::Extrude(ExtrudeFeature {
        profile: ProfileSource::Sketch { feature: sketch },
        operation: ExtrudeOp::Boss,
        end: EndCondition::Blind { depth },
        reverse: false,
    })
}

fn lattice() -> FeatureKind {
    FeatureKind::Lattice(LatticeFeature { accuracy_mm: 0.05, min_detail_mm: 0.8 })
}

/// A block followed by a lattice.
fn latticed() -> (Document, FeatureId, FeatureId) {
    let mut d = doc();
    let s = add(&mut d, rectangle(20.0));
    let b = add(&mut d, boss(s, 5.0));
    let l = add(&mut d, lattice());
    (d, b, l)
}

fn status(d: &Document, id: FeatureId) -> FeatureStatus {
    d.statuses().into_iter().find(|(i, _)| *i == id).unwrap().1.clone()
}

#[test]
fn the_lattice_reports_the_missing_backend_and_keeps_the_body() {
    let (d, b, l) = latticed();
    let FeatureStatus::Failed(message) = status(&d, l) else { panic!("{:?}", status(&d, l)) };
    assert!(message.starts_with("unsupported: lattice"), "{message}");
    assert!(status(&d, b).is_ok());
    assert!(d.body().is_some(), "the body before the lattice stays visible");
}

#[test]
fn a_lattice_needs_a_body() {
    let mut d = doc();
    let l = add(&mut d, lattice());
    assert_eq!(status(&d, l), FeatureStatus::Failed("there is no body to lattice".into()));
}

#[test]
fn settings_must_be_positive() {
    let mut d = doc();
    for (a, m) in [(0.0, 0.5), (0.1, -1.0), (f64::NAN, 0.5)] {
        let kind = FeatureKind::Lattice(LatticeFeature { accuracy_mm: a, min_detail_mm: m });
        assert!(matches!(try_add(&mut d, kind, None), Err(CommandError::Invalid(_))), "{a} {m}");
    }
    assert!(d.features().is_empty());
}

#[test]
fn cad_after_the_lattice_is_refused_and_changes_nothing() {
    let (mut d, _, l) = latticed();
    let count = d.features().len();
    let depth = d.undo_depth();
    assert_eq!(try_add(&mut d, rectangle(4.0), None), Err(CommandError::AfterLattice(l)));
    assert_eq!(try_add(&mut d, rectangle(4.0), Some(count)), Err(CommandError::AfterLattice(l)));
    assert_eq!(d.features().len(), count);
    assert_eq!(d.undo_depth(), depth);
}

#[test]
fn a_second_lattice_or_a_lattice_that_is_not_last_is_refused() {
    let (mut d, _, _) = latticed();
    assert_eq!(try_add(&mut d, lattice(), None), Err(CommandError::LatticeNotLast));
    let mut e = doc();
    let s = add(&mut e, rectangle(20.0));
    add(&mut e, boss(s, 5.0));
    assert!(matches!(try_add(&mut e, lattice(), Some(1)), Err(CommandError::AfterLattice(_))));
}

#[test]
fn moving_cad_past_the_lattice_is_refused() {
    let (mut d, b, l) = latticed();
    let last = d.features().len() - 1;
    assert_eq!(
        d.execute(Command::MoveFeature { id: b, position: last }).map(|_| ()),
        Err(CommandError::AfterLattice(l))
    );
    assert_eq!(d.execute(Command::MoveFeature { id: l, position: 0 }).map(|_| ()), Err(CommandError::AfterLattice(l)));
}

#[test]
fn a_feature_cannot_change_into_or_out_of_a_lattice() {
    let (mut d, b, l) = latticed();
    assert!(matches!(
        d.execute(Command::EditFeature { id: l, kind: boss(FeatureId(1), 1.0) }),
        Err(CommandError::Invalid(_))
    ));
    assert!(matches!(d.execute(Command::EditFeature { id: b, kind: lattice() }), Err(CommandError::Invalid(_))));
}

#[test]
fn returning_before_the_lattice_allows_cad_and_keeps_the_settings() {
    let (mut d, b, l) = latticed();
    d.return_before_lattice().unwrap();
    assert_eq!(status(&d, l), FeatureStatus::RolledBack);
    assert!(status(&d, b).is_ok());
    let sketch = add(&mut d, rectangle(4.0));
    let order: Vec<_> = d.features().iter().map(|f| f.id).collect();
    assert_eq!(order.last(), Some(&l), "new CAD goes in before the lattice");
    assert_eq!(order[order.len() - 2], sketch);
    assert_eq!(d.feature(l).unwrap().kind, lattice(), "accuracy and detail are kept");
    d.execute(Command::RollTo { before: None }).unwrap();
    assert!(matches!(status(&d, l), FeatureStatus::Failed(_)), "the lattice builds again after the bar rolls forward");
}

#[test]
fn returning_without_a_lattice_is_refused() {
    let mut d = doc();
    assert!(matches!(d.return_before_lattice(), Err(CommandError::Invalid(_))));
}

#[test]
fn deleting_the_lattice_lets_cad_continue_and_undo_puts_it_back_last() {
    let (mut d, _, l) = latticed();
    d.execute(Command::DeleteFeature { id: l }).unwrap();
    assert_eq!(d.lattice(), None);
    let s = add(&mut d, rectangle(3.0));
    d.undo().unwrap();
    assert!(d.feature(s).is_none());
    d.undo().unwrap();
    assert_eq!(d.lattice(), Some(l));
    assert_eq!(d.features().last().unwrap().id, l);
}

#[test]
fn accuracy_and_detail_survive_save_and_reopen() {
    let (d, _, l) = latticed();
    let reopened = Document::from_bytes(&d.to_bytes().unwrap(), kernel()).unwrap();
    assert_eq!(reopened.lattice(), Some(l));
    assert_eq!(reopened.feature(l).unwrap().kind, lattice());
    assert_eq!(reopened.features().last().unwrap().id, l);
    assert!(d.part_json().contains("\"accuracy_mm\":0.05") || d.part_json().contains("\"accuracy_mm\": 0.05"));
}

#[test]
fn a_saved_file_with_cad_after_the_lattice_is_refused_on_open() {
    let (d, _, _) = latticed();
    let mut part: serde_json::Value = serde_json::from_str(&d.part_json()).unwrap();
    let features = part["features"].as_array_mut().unwrap();
    let last = features.pop().unwrap();
    features.insert(features.len() - 1, last);
    assert!(Document::from_part_json(&part.to_string(), kernel()).is_err());
}

#[test]
fn a_returned_part_reopens_with_the_bar_and_the_settings() {
    let (mut d, _, l) = latticed();
    d.return_before_lattice().unwrap();
    let reopened = Document::from_bytes(&d.to_bytes().unwrap(), kernel()).unwrap();
    assert_eq!(status(&reopened, l), FeatureStatus::RolledBack);
    assert_eq!(reopened.feature(l).unwrap().kind, lattice());
}
