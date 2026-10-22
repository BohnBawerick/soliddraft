//! The phase 0 interface, driven through egui_kittest against the real kernel and the
//! real wgpu pipeline: a drawing saved against one revision of its part opens pinned
//! at that revision after the part moved on, with the "newer version available"
//! marker; Rebuild brings it to the current part. The part file moved away and a
//! different part put in its place opens the repair dialog, which takes a path by
//! hand; a search folder added on the File locations page finds a moved part by its
//! id. Frames go to `target/exit-test/references/`.
//!
//! Skips itself when no GPU adapter is available (plain CI runners).

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use soliddraft_app::app::{Automation, SolidDraftApp};
use soliddraft_app::document::{Command, FeatureKind};
use soliddraft_app::drawing::{DrawingAction, DrawingSel, LinkState};
use soliddraft_app::state::AppAction;
use soliddraft_document::{DocKind, LocationKind, Via, identify_file};
use std::path::{Path, PathBuf};

fn gpu_available() -> bool {
    let _ = env_logger::try_init();
    let instance = egui_wgpu::wgpu::Instance::default();
    !pollster::block_on(instance.enumerate_adapters(egui_wgpu::wgpu::Backends::all())).is_empty()
}

fn out_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/exit-test/references");
    std::fs::create_dir_all(&dir).unwrap();
    // The resolver reports normalised paths; compare against the same.
    std::fs::canonicalize(&dir).unwrap()
}

type App<'a> = Harness<'a, SolidDraftApp>;

fn harness() -> App<'static> {
    Harness::builder()
        .with_size(vec2(1440.0, 900.0))
        .with_pixels_per_point(1.0)
        .with_render_every_step(true)
        .with_step_dt(1.0 / 60.0)
        .wgpu()
        .build_eframe(|cc| {
            SolidDraftApp::new(
                cc,
                soliddraft_app::demo_document_on(std::sync::Arc::new(soliddraft_kernel_smlib::SmlibKernel::new())),
                Automation::default(),
            )
        })
}

fn save(harness: &mut App<'_>, name: &str) {
    let image = harness.render().expect("render");
    let path = out_dir().join(format!("{name}.png"));
    image.save(&path).expect("save png");
    eprintln!("wrote {}", path.display());
}

fn settle(harness: &mut App<'_>) {
    harness.run_steps(24);
}

fn push(harness: &mut App<'_>, action: AppAction) {
    harness.state_mut().state.push(action);
    harness.run_steps(3);
}

fn drawing(harness: &mut App<'_>, action: DrawingAction) {
    push(harness, AppAction::Drawing(action));
}

/// Clicks the first button with this label; ribbon items come before panel buttons.
fn click_button(harness: &mut App<'_>, label: &str) {
    harness
        .get_all_by_role_and_label(egui::accesskit::Role::Button, label)
        .next()
        .unwrap_or_else(|| panic!("no button labelled {label}"))
        .click();
    harness.run_steps(4);
}

fn type_into(harness: &mut App<'_>, label: &str, text: &str) {
    harness.get_all_by_label(label).last().unwrap_or_else(|| panic!("no field {label}")).click();
    harness.run_steps(2);
    harness.get_all_by_label(label).last().unwrap_or_else(|| panic!("no field {label}")).type_text(text);
    harness.run_steps(2);
}

fn session<'a>(harness: &'a App<'_>) -> &'a soliddraft_app::drawing::DrawingSession {
    harness.state().state.drawing.session.as_ref().expect("a drawing is open")
}

fn part_path(harness: &App<'_>) -> Option<PathBuf> {
    harness.state().state.doc.path.clone()
}

/// The GPU harnesses share one adapter; one test at a time.
static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn gpu_lock() -> std::sync::MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(|e| e.into_inner())
}

fn reset(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).unwrap();
}

#[test]
fn drawing_opens_pinned_rebuilds_repairs_and_lists_locations() {
    if !gpu_available() {
        eprintln!("no GPU adapter; skipping the references test");
        return;
    }
    let _gpu = gpu_lock();
    let dir = out_dir();
    let config = dir.join("config");
    let work = dir.join("work");
    let elsewhere = dir.join("elsewhere");
    let archive = dir.join("archive");
    let library = dir.join("materials");
    for d in [&config, &work, &elsewhere, &archive, &library] {
        reset(d);
    }
    soliddraft_app::file::set_config_dir(Some(config.clone()));
    let part_file = work.join("bracket.dwprt");
    let drawing_file = work.join("bracket.dwdrw");

    let mut harness = harness();
    harness.run_steps(6);

    // 1. Save the part, make a drawing of it and save the drawing.
    push(&mut harness, AppAction::SaveAs(Some(part_file.clone())));
    assert_eq!(part_path(&harness).as_deref(), Some(part_file.as_path()));
    let part_id = harness.state().state.doc.model().id();
    drawing(&mut harness, DrawingAction::MakeDrawing);
    drawing(&mut harness, DrawingAction::CreateDrawing);
    settle(&mut harness);
    assert!(harness.state().state.drawing.is_active(), "the drawing tab should be active");
    drawing(&mut harness, DrawingAction::Save(Some(drawing_file.clone())));
    let pinned = session(&harness).doc.drawing.model.pinned.clone();
    assert_eq!(session(&harness).doc.drawing.model.id, Some(part_id));
    assert_eq!(session(&harness).link_state(), LinkState::Current);
    assert!(drawing_file.is_file());

    // 2. Close the drawing, make the plate thicker and save the part: the file moves on.
    drawing(&mut harness, DrawingAction::CloseDrawing);
    let (id, params) = {
        let f = &harness.state().state.doc.features[0];
        let FeatureKind::Extrude(p) = &f.kind else { panic!("the first feature is an extrude") };
        (f.id, *p)
    };
    let params = soliddraft_app::document::ExtrudeParams { depth: params.depth + 10.0, ..params };
    push(&mut harness, AppAction::Command(Command::EditExtrude { id, params }));
    push(&mut harness, AppAction::Save);
    let newer = harness.state().state.doc.model().semantic_hash();
    assert_ne!(newer, pinned, "the part has a new revision");

    // 3. Forget the part, open the drawing: it shows the pinned revision, loads its part
    //    and marks the newer version; nothing is rebuilt by itself.
    push(&mut harness, AppAction::NewPart);
    push(&mut harness, AppAction::Open(Some(drawing_file.clone())));
    settle(&mut harness);
    assert!(harness.state().state.drawing.is_active());
    assert_eq!(part_path(&harness).as_deref(), Some(part_file.as_path()), "the drawing's part was opened");
    {
        let s = session(&harness);
        assert_eq!(s.doc.drawing.model.pinned, pinned, "still pinned at the saved revision");
        assert_eq!(s.link_state(), LinkState::Newer { now: newer.clone() });
        assert!(s.doc.is_stale(), "the views read as older than the part");
        assert!(s.doc.last_update.is_none(), "no rebuild by itself");
        assert_eq!(s.marker().map(|m| m.0), Some("Newer version available".to_owned()));
    }
    assert!(harness.get_all_by_role_and_label(egui::accesskit::Role::Button, "Rebuild").count() >= 2, "ribbon and bar");
    save(&mut harness, "r01-newer-version");
    drawing(&mut harness, DrawingAction::Select(Some(DrawingSel::Model)));
    settle(&mut harness);
    assert!(harness.get_all_by_role_and_label(egui::accesskit::Role::Button, "Replace model…").count() >= 1);
    save(&mut harness, "r07-model-reference");
    drawing(&mut harness, DrawingAction::Select(None));

    // 4. Rebuild: the views move to the current revision, as one undo step.
    click_button(&mut harness, "Rebuild");
    settle(&mut harness);
    {
        let s = session(&harness);
        assert_eq!(s.doc.drawing.model.pinned, newer, "re-pinned at the current revision");
        assert_eq!(s.link_state(), LinkState::Current);
        assert!(!s.doc.is_stale());
        let r = s.doc.last_update.as_ref().expect("an update report");
        assert_eq!(r.views, 4);
        assert_eq!(s.doc.undo_label(), Some("Update from model"));
    }
    save(&mut harness, "r02-rebuilt");
    drawing(&mut harness, DrawingAction::Save(None));
    assert_eq!(identify_file(&drawing_file).unwrap().references[0].pinned, newer);

    // 5. Move the part away and put a different part under its name: the drawing opens
    //    with the repair dialog, listing the impostor as a candidate.
    push(&mut harness, AppAction::NewPart);
    std::fs::rename(&part_file, elsewhere.join("bracket.dwprt")).unwrap();
    let mut other = soliddraft_app::demo_document_on(std::sync::Arc::new(soliddraft_kernel_smlib::SmlibKernel::new()));
    soliddraft_app::file::save(&mut other, &part_file).unwrap();
    assert_ne!(other.model().id(), part_id);
    push(&mut harness, AppAction::Open(Some(drawing_file.clone())));
    settle(&mut harness);
    {
        let s = session(&harness);
        assert_eq!(s.link_state(), LinkState::Missing);
        let m = s.missing().expect("missing");
        assert!(matches!(m.stored, soliddraft_document::resolve::StoredPathProblem::DifferentDocument(_)), "{m:?}");
        assert!(m.candidates.iter().any(|c| c.same_name), "the impostor is a candidate");
        assert!(harness.state().state.drawing.repair.is_some(), "the repair dialog opened");
        assert_ne!(part_path(&harness).as_deref(), Some(part_file.as_path()), "the impostor was not opened");
    }
    assert_eq!(harness.get_all_by_role_and_label(egui::accesskit::Role::Button, "Use this part").count(), 1);
    save(&mut harness, "r03-repair-dialog");

    // 6. Pick the file by hand: the reference keeps its id, stores the new path, and the
    //    part opens; the drawing is at the file's revision, so nothing is stale.
    type_into(&mut harness, "Repair path", &elsewhere.join("bracket.dwprt").display().to_string());
    click_button(&mut harness, "Use this file");
    settle(&mut harness);
    assert!(harness.state().state.drawing.repair.is_none(), "the dialog closed");
    {
        let s = session(&harness);
        assert_eq!(s.doc.drawing.model.path, "../elsewhere/bracket.dwprt");
        assert_eq!(s.doc.drawing.model.id, Some(part_id));
        assert!(s.doc.dirty);
        assert_eq!(s.link_state(), LinkState::Current, "{:?}", s.link);
    }
    assert_eq!(part_path(&harness).as_deref(), Some(elsewhere.join("bracket.dwprt").as_path()), "the part opened");
    save(&mut harness, "r04-repaired");
    drawing(&mut harness, DrawingAction::Save(None));
    let saved = identify_file(&drawing_file).unwrap();
    assert_eq!(saved.kind, DocKind::Drawing);
    assert_eq!(saved.references[0].path, "../elsewhere/bracket.dwprt");

    // 7. File locations: add a material library folder on the page and save it.
    push(&mut harness, AppAction::ShowFileLocations(None));
    settle(&mut harness);
    click_button(&mut harness, "Material libraries");
    type_into(&mut harness, "Location path", &library.display().to_string());
    click_button(&mut harness, "Add path");
    settle(&mut harness);
    save(&mut harness, "r05-file-locations");
    click_button(&mut harness, "OK");
    settle(&mut harness);
    assert!(harness.state().state.locations_dialog.is_none());
    let locations = soliddraft_app::settings::file_locations_in(&config.join("settings.json"));
    assert_eq!(locations.get(LocationKind::MaterialLibraries), std::slice::from_ref(&library));
    assert_eq!(locations.files(LocationKind::MaterialLibraries), Vec::<PathBuf>::new(), "an empty folder, no files");

    // 8. Move the part again, outside the drawing's folder. The drawing opens with the
    //    dialog; a search folder added from it finds the part by id, and the dialog goes.
    push(&mut harness, AppAction::NewPart);
    std::fs::rename(elsewhere.join("bracket.dwprt"), archive.join("bracket.dwprt")).unwrap();
    push(&mut harness, AppAction::Open(Some(drawing_file.clone())));
    settle(&mut harness);
    assert_eq!(session(&harness).link_state(), LinkState::Missing);
    assert!(harness.state().state.drawing.repair.is_some());
    click_button(&mut harness, "File locations…");
    settle(&mut harness);
    let d = harness.state().state.locations_dialog.as_ref().expect("the page opened from the dialog");
    assert_eq!(d.kind, LocationKind::ReferencedDocuments);
    type_into(&mut harness, "Location path", &archive.display().to_string());
    click_button(&mut harness, "Add path");
    click_button(&mut harness, "OK");
    settle(&mut harness);
    {
        let s = session(&harness);
        assert_eq!(s.link_state(), LinkState::Current, "{:?}", s.link);
        let t = s.model_target().expect("found");
        assert_eq!(t.via, Via::Index, "found by id in the search folder");
        assert_eq!(t.new_path.as_deref(), Some("../archive/bracket.dwprt"));
        assert!(harness.state().state.drawing.repair.is_none(), "the dialog closed by itself");
    }
    save(&mut harness, "r06-found-by-id");
    // Rebuild opens the part from where it was found, then rebinds the stored path.
    click_button(&mut harness, "Rebuild");
    settle(&mut harness);
    assert_eq!(part_path(&harness).as_deref(), Some(archive.join("bracket.dwprt").as_path()));
    assert_eq!(session(&harness).doc.drawing.model.path, "../archive/bracket.dwprt");
    assert_eq!(session(&harness).link_state(), LinkState::Current);
}
