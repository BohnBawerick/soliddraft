//! Assemblies in the window: File > New assembly, Make assembly from the open part,
//! Insert component (a file picker, or part files dropped on the window), the component
//! tree, typed placement, and opening and saving `.dwasm` files. The format, loading,
//! edits and undo are `soliddraft-assembly`'s (see `docs/assemblies.md`); this module
//! holds the session the window shows and draws it with the part viewport's renderer.
//!
//! Hooks into the shell are small: `Tab::Assembly` in `app.rs`, one arm each in
//! `ribbon.rs`, the tree and property tabs and the status bar, one `AppAction::Assembly`
//! variant and one `AssemblyState` field in `state.rs`.

use crate::app::Tab;
use crate::icons::{self, Icon};
use crate::state::{AppAction, AppState, NoteKind};
use crate::theme;
use crate::tree::{RowSpec, row};
use crate::viewport::ViewportState;
use crate::viewport::assembly::{AssemblyFrame, Detail};
use egui::{Align2, Rect, Sense, Stroke, pos2, vec2};
use egui_dock::DockState;
use glam::{DMat4, DQuat, DVec3, EulerRot};
use soliddraft_assembly::{
    AssemblyDocument, DefinitionIndex, Flags, Freshness, KernelSource, LoadState, MeshBounds, OccurrenceIndex,
    OccurrencePath, OpenOptions, Opening, Placement, Scene, SharedMesh,
};
use soliddraft_document::kind::ASSEMBLY_EXTENSION;
use soliddraft_document::{DocKind, DocRef};
use soliddraft_tessellation::{DeviceBudget, Level};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long a frame spends taking in meshes the loader found.
const POLL_BUDGET: Duration = Duration::from_millis(4);
/// How many arrived meshes a frame hands the GPU.
const MESHES_PER_FRAME: usize = 256;
/// The gap left between the assembly and a component inserted beside it, in millimetres.
const INSERT_GAP: f64 = 20.0;

/// What the assembly tab does.
#[derive(Clone, Debug, PartialEq)]
pub enum AssemblyAction {
    /// A new, empty assembly in place of the open one.
    New,
    /// A new assembly that places the open part, which must be saved.
    FromPart,
    /// Opens an assembly file in place of the open one.
    Open(PathBuf),
    /// Places part or assembly files; `None` asks with a file dialog.
    Insert(Option<Vec<PathBuf>>),
    /// Saves to the assembly's own file, or to this one; with neither, asks.
    Save(Option<PathBuf>),
    /// Asks where to save.
    SaveAs,
    Select(Option<OccurrencePath>),
    /// Moves an occurrence to a placement relative to the assembly that contains it.
    Place(OccurrencePath, Placement),
    Undo,
    Redo,
    ZoomFit,
}

/// The open assembly, one being opened, and whether the assembly is the active document.
#[derive(Default)]
pub struct AssemblyState {
    pub session: Option<AssemblySession>,
    /// An assembly file whose structure is being read on another thread.
    pub opening: Option<(PathBuf, Opening)>,
    pub active: bool,
}

impl AssemblyState {
    /// True while the assembly tab is the document being worked on.
    pub fn is_active(&self) -> bool {
        self.active && self.session.is_some()
    }
}

/// An open assembly as the window shows it.
pub struct AssemblySession {
    pub doc: AssemblyDocument,
    /// The instances the renderer draws, shared with it.
    pub scene: Arc<Mutex<Scene>>,
    /// The assembly's own camera, independent of the part's.
    pub viewport: ViewportState,
    /// The selected occurrence, by path so it outlives a restructure.
    pub selected: Option<OccurrencePath>,
    /// The placement fields of the selection as typed.
    pub draft: Option<PlacementDraft>,
    /// Sub-assemblies open in the tree.
    pub expanded: HashSet<OccurrencePath>,
    /// What the scene shows as selected and hovered.
    shown: (Option<OccurrenceIndex>, Option<OccurrenceIndex>),
    /// Meshes taken from the scene for the renderer, kept until a painted frame took them.
    outbox: Outbox,
    /// The camera frames the parts once the first meshes arrive.
    fit_pending: bool,
}

/// The selection's placement as the property panel edits it.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacementDraft {
    pub path: OccurrencePath,
    /// Millimetres.
    pub translation: [f64; 3],
    /// Degrees about x, then y, then z.
    pub rotation: [f64; 3],
    /// The placement the fields were filled from.
    pub source: Placement,
}

impl PlacementDraft {
    pub fn of(path: OccurrencePath, p: &Placement) -> PlacementDraft {
        let (x, y, z) = p.rotation.to_euler(EulerRot::XYZ);
        let tidy = |v: f64| if v.abs() < 1e-9 { 0.0 } else { v };
        PlacementDraft {
            path,
            translation: p.translation.to_array().map(tidy),
            rotation: [x, y, z].map(|a| tidy(a.to_degrees())),
            source: *p,
        }
    }

    /// True when the fields say something other than the placement they came from.
    pub fn edited(&self) -> bool {
        let filled = PlacementDraft::of(self.path.clone(), &self.source);
        filled.translation != self.translation || filled.rotation != self.rotation
    }

    pub fn placement(&self) -> Placement {
        let [x, y, z] = self.rotation.map(f64::to_radians);
        Placement::new(DQuat::from_euler(EulerRot::XYZ, x, y, z), DVec3::from_array(self.translation))
    }
}

/// Meshes on their way to the GPU. egui can discard a pass before its paint callbacks
/// run; what a discarded pass carried goes again with the next.
#[derive(Default)]
struct Outbox {
    meshes: Vec<(DefinitionIndex, SharedMesh)>,
    levels: Vec<(DefinitionIndex, Level, SharedMesh)>,
    deviations: Vec<(DefinitionIndex, [f32; 3])>,
    /// The pass the last batch went with.
    sent_at: Option<u64>,
}

impl AssemblySession {
    fn new(mut doc: AssemblyDocument, scene: Scene) -> AssemblySession {
        let fit_pending = true;
        let mut viewport = ViewportState::default();
        viewport.camera.fit(None);
        let scene = Arc::new(Mutex::new(scene));
        doc.poll(POLL_BUDGET);
        AssemblySession {
            doc,
            scene,
            viewport,
            selected: None,
            draft: None,
            expanded: HashSet::new(),
            shown: (None, None),
            outbox: Outbox::default(),
            fit_pending,
        }
    }

    /// The name the tab and the tree show, with a dot for unsaved edits.
    pub fn title(&self) -> String {
        let name = &self.doc.assembly().name;
        if self.doc.is_dirty() { format!("{name} •") } else { name.clone() }
    }

    /// The selected occurrence's row, if it is still there.
    pub fn selected_row(&self) -> Option<OccurrenceIndex> {
        let path = self.selected.as_ref()?;
        self.doc.table().find(path).filter(|r| !self.doc.table().flags(*r).has(Flags::REMOVED))
    }

    /// The root's own components, in tree order, with their rows.
    pub fn components(&self) -> Vec<OccurrenceIndex> {
        let table = self.doc.table();
        table.children(OccurrenceIndex::ROOT).filter(|r| !table.flags(*r).has(Flags::REMOVED)).collect()
    }

    /// The box around everything loaded, in the assembly's frame.
    fn bounds(&self) -> Option<(glam::Vec3, glam::Vec3)> {
        let b = self.doc.table().bounds(OccurrenceIndex::ROOT);
        (!b.is_empty()).then(|| (b.min.as_vec3(), b.max.as_vec3()))
    }

    /// Takes in what the loader found and what edits changed, once a frame.
    fn update(&mut self) {
        self.doc.poll(POLL_BUDGET);
        let mut scene = self.scene.lock().expect("the scene lock");
        scene.update(&mut self.doc);
        let selected = self.selected_row();
        if self.shown.0 != selected {
            scene.set_selection(self.doc.table(), selected.as_slice());
            self.shown.0 = selected;
        }
        drop(scene);
        if self.fit_pending && self.bounds().is_some() && !self.doc.is_loading() {
            self.viewport.camera.fit(self.bounds());
            self.fit_pending = false;
        }
        // The fields follow the selection, and its placement after an undo or a move.
        let local = selected.map(|r| (self.doc.table().path(r), self.doc.table().local(r)));
        self.draft = match (local, self.draft.take()) {
            (Some((path, p)), Some(d)) if d.path == path && d.source == p => Some(d),
            (Some((path, p)), _) => Some(PlacementDraft::of(path, &p)),
            (None, _) => None,
        };
    }

    /// What this pass hands the renderer: the meshes that arrived, and again those a
    /// discarded pass carried.
    fn frame(&mut self, ctx: &egui::Context) -> AssemblyFrame {
        let pass = ctx.cumulative_pass_nr();
        let resend = ctx.current_pass_index() > 0 && self.outbox.sent_at.is_some_and(|p| p + 1 == pass);
        if !resend {
            self.outbox.meshes.clear();
            self.outbox.levels.clear();
            self.outbox.deviations.clear();
        }
        let (meshes, levels) = {
            let mut scene = self.scene.lock().expect("the scene lock");
            (scene.take_new_meshes(MESHES_PER_FRAME), scene.take_new_levels())
        };
        let store = self.doc.store();
        for d in meshes {
            if let Some(m) = store.get(d).mesh.clone() {
                self.outbox.deviations.push((d, store.get(d).deviations()));
                self.outbox.meshes.push((d, m));
            }
        }
        for (d, l) in levels {
            if let Some(m) = store.get(d).level_mesh(l).cloned() {
                self.outbox.levels.push((d, l, m));
            }
        }
        if !self.outbox.meshes.is_empty() || !self.outbox.levels.is_empty() {
            ctx.request_repaint();
        }
        self.outbox.sent_at = Some(pass);
        AssemblyFrame {
            scene: self.scene.clone(),
            meshes: self.outbox.meshes.clone(),
            // The viewport sets the camera.
            view_proj: DMat4::IDENTITY,
            eye: DVec3::ZERO,
            detail: Detail::default(),
            levels: self.outbox.levels.clone(),
            deviations: self.outbox.deviations.clone(),
            budget: DeviceBudget::default(),
        }
    }
}

/// What opening and inserting may consult: the referenced documents folders, the
/// revisions this session has seen, and the part's kernel for parts without display
/// data.
fn options(state: &AppState) -> OpenOptions {
    OpenOptions {
        search: crate::settings::file_locations().search_folders().to_vec(),
        revisions: Some(state.revisions.clone()),
        display: Some(Arc::new(KernelSource::new(state.doc.kernel().clone()))),
        check_freshness: true,
        ..OpenOptions::default()
    }
}

/// Runs once a frame before the panels: tracks which document is active, takes an
/// assembly that finished opening, and brings the open one up to date.
pub fn frame(state: &mut AppState, dock: &mut DockState<Tab>, ctx: &egui::Context) {
    if let Some((_, tab)) = dock.find_active_focused() {
        match tab {
            Tab::Assembly => {
                state.assembly.active = true;
                state.drawing.active = false;
            }
            Tab::Viewport | Tab::Drawing => state.assembly.active = false,
            _ => {}
        }
    }
    if dock.find_tab(&Tab::Assembly).is_none() {
        state.assembly.active = false;
    }
    let opened = state.assembly.opening.as_ref().map(|(path, opening)| (path.clone(), opening.try_take()));
    match opened {
        Some((path, Some(Ok((doc, scene))))) => {
            state.assembly.opening = None;
            let n = doc.assembly().components.len();
            show(state, dock, AssemblySession::new(doc, scene));
            crate::file::remember(&path);
            state.recent = crate::file::recent();
            state.note(format!("Opened {} · {n} component{}", path.display(), plural(n)), NoteKind::Ok);
        }
        Some((path, Some(Err(e)))) => {
            state.assembly.opening = None;
            state.note(format!("{}: {e}", path.display()), NoteKind::Error);
        }
        Some((_, None)) => ctx.request_repaint_after(Duration::from_millis(16)),
        None => {}
    }
    if let Some(s) = state.assembly.session.as_mut() {
        s.update();
        if s.doc.is_loading() {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }
}

/// Files dropped on the window: placed in the active assembly, or else the first one
/// that opens is opened.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub fn dropped(state: &mut AppState, files: Vec<PathBuf>) {
    if state.assembly.is_active() {
        let placeable: Vec<PathBuf> = files
            .into_iter()
            .filter(|p| matches!(DocKind::of_path(p), Some(DocKind::Part | DocKind::Assembly)))
            .collect();
        if placeable.is_empty() {
            state.note("Drop part or assembly files to insert them", NoteKind::Info);
        } else {
            state.push(AppAction::Assembly(AssemblyAction::Insert(Some(placeable))));
        }
    } else if let Some(p) = files.into_iter().find(|p| DocKind::of_path(p).is_some()) {
        state.push(AppAction::Open(Some(p)));
    }
}

/// Puts a session in place of the open assembly and shows its tab.
fn show(state: &mut AppState, dock: &mut DockState<Tab>, session: AssemblySession) {
    state.assembly.session = Some(session);
    state.assembly.active = true;
    state.drawing.active = false;
    show_tab(dock);
}

/// Opens the assembly tab beside the part and makes it the active document.
fn show_tab(dock: &mut DockState<Tab>) {
    if let Some(path) = dock.find_tab(&Tab::Assembly) {
        let _ = dock.set_active_tab(path);
        dock.set_focused_node_and_surface(egui_dock::NodePath { surface: path.surface, node: path.node });
        return;
    }
    if let Some(path) = dock.find_tab(&Tab::Viewport) {
        dock.set_focused_node_and_surface(egui_dock::NodePath { surface: path.surface, node: path.node });
    }
    dock.push_to_focused_leaf(Tab::Assembly);
}

/// The name a new assembly takes: Assembly1, or the next number not open.
fn new_name(state: &AppState) -> String {
    match state.assembly.session.as_ref().map(|s| s.doc.assembly().name.clone()) {
        Some(open) if open.starts_with("Assembly") => {
            let n = open.trim_start_matches("Assembly").parse::<u32>().unwrap_or(1);
            format!("Assembly{}", n + 1)
        }
        _ => "Assembly1".to_owned(),
    }
}

fn new_session(state: &AppState, name: String) -> AssemblySession {
    let mut doc = AssemblyDocument::new_with(name, options(state));
    let scene = Scene::new(&mut doc);
    AssemblySession::new(doc, scene)
}

pub fn apply(state: &mut AppState, dock: &mut DockState<Tab>, action: AssemblyAction) {
    match action {
        AssemblyAction::New => {
            let session = new_session(state, new_name(state));
            let name = session.doc.assembly().name.clone();
            show(state, dock, session);
            state.note(format!("New assembly {name} · insert a part to start"), NoteKind::Info);
        }
        AssemblyAction::FromPart => {
            let Some(part) = state.doc.path.clone().filter(|_| !state.doc.dirty) else {
                return state.note("Save the part before making an assembly of it", NoteKind::Info);
            };
            let session = new_session(state, format!("{} assembly", state.doc.name));
            show(state, dock, session);
            insert(state, &[part]);
        }
        AssemblyAction::Open(path) => {
            state.note(format!("Opening {}…", path.display()), NoteKind::Info);
            let opening = AssemblyDocument::open_in_background(&path, options(state));
            state.assembly.opening = Some((path, opening));
        }
        AssemblyAction::Insert(files) => {
            let Some(files) = files.or_else(|| {
                crate::platform::open_paths(
                    "Insert component",
                    "SolidDraft parts and assemblies",
                    &[crate::file::EXTENSION, ASSEMBLY_EXTENSION],
                )
            }) else {
                return;
            };
            insert(state, &files);
        }
        AssemblyAction::Save(path) => save(state, path),
        AssemblyAction::SaveAs => {
            let Some(s) = state.assembly.session.as_ref() else { return };
            let name = format!("{}.{ASSEMBLY_EXTENSION}", s.doc.assembly().name);
            if let Some(path) =
                crate::platform::save_path("Save assembly", "SolidDraft assembly", ASSEMBLY_EXTENSION, &name)
            {
                save(state, Some(path));
            }
        }
        AssemblyAction::Select(path) => {
            if let Some(s) = state.assembly.session.as_mut() {
                s.selected = path;
            }
        }
        AssemblyAction::Place(path, placement) => {
            let Some(s) = state.assembly.session.as_mut() else { return };
            match s.doc.move_to(&[(path, placement)]) {
                Ok(()) => state.note("Moved", NoteKind::Ok),
                Err(e) => state.note(e.to_string(), NoteKind::Error),
            }
        }
        AssemblyAction::Undo | AssemblyAction::Redo => {
            let Some(s) = state.assembly.session.as_mut() else { return };
            let undo = action == AssemblyAction::Undo;
            let label = if undo { s.doc.undo_label() } else { s.doc.redo_label() }.map(str::to_owned);
            let done = if undo { s.doc.undo() } else { s.doc.redo() };
            match (done, label) {
                (true, Some(l)) => state.note(format!("{} {l}", if undo { "Undid" } else { "Redid" }), NoteKind::Info),
                _ => state.note(if undo { "Nothing to undo" } else { "Nothing to redo" }, NoteKind::Info),
            }
        }
        AssemblyAction::ZoomFit => {
            if let Some(s) = state.assembly.session.as_mut() {
                let bounds = s.bounds();
                s.viewport.camera.fit(bounds);
            }
        }
    }
}

/// Places each file in the open assembly, pinned at its revision now. The first
/// component goes at the origin and each later one beside what is there, along x.
fn insert(state: &mut AppState, files: &[PathBuf]) {
    let revisions = state.revisions.clone();
    let Some(s) = state.assembly.session.as_mut() else { return };
    let mut placed = Vec::new();
    let mut problems = Vec::new();
    for file in files {
        match place(s, &*revisions, file) {
            Ok(path) => placed.push(path),
            Err(e) => problems.push(format!("{}: {e}", file.display())),
        }
    }
    if let Some(last) = placed.last() {
        s.selected = Some(last.clone());
        s.fit_pending = true;
    }
    match (placed.len(), problems.first()) {
        (_, Some(p)) => state.note(p.clone(), NoteKind::Error),
        (1, None) => {
            let name = files[0].file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            state.note(format!("Inserted {name}"), NoteKind::Ok)
        }
        (n, None) => state.note(format!("Inserted {n} components"), NoteKind::Ok),
    }
}

/// Places one file: reads it for its id and revision, hands its bytes to the revision
/// store so an assembly with no folder yet can show it, and places it.
fn place(
    s: &mut AssemblySession,
    revisions: &dyn soliddraft_document::RevisionStore,
    file: &Path,
) -> Result<OccurrencePath, String> {
    let bytes = std::fs::read(file).map_err(|e| e.to_string())?;
    let identity = soliddraft_document::identify(std::io::Cursor::new(&bytes)).map_err(|e| e.to_string())?;
    if !matches!(identity.kind, DocKind::Part | DocKind::Assembly) {
        return Err(format!("a {} cannot be placed in an assembly", identity.kind));
    }
    revisions.record(identity.id, &identity.revision, &bytes);
    let absolute = std::path::absolute(file).map_err(|e| e.to_string())?;
    let mut r =
        DocRef::new(identity.id, identity.kind, absolute.to_string_lossy().replace('\\', "/"), identity.revision);
    if let Some(here) = s.doc.path() {
        r.set_path(here, &absolute);
    }
    // Beside what is there: the part's own box comes from its saved display data.
    s.doc.finish_loading();
    let there = s.doc.table().bounds(OccurrenceIndex::ROOT);
    let at = if s.components().is_empty() || there.is_empty() {
        DVec3::ZERO
    } else {
        let own_min = soliddraft_model::display::read(&bytes, Level::Standard)
            .ok()
            .flatten()
            .map(|(m, _)| m.bounds().min.x)
            .unwrap_or(0.0);
        DVec3::new(there.max.x + INSERT_GAP - own_min, 0.0, 0.0)
    };
    let id = s.doc.insert(r, Placement::translation(at)).map_err(|e| e.to_string())?;
    Ok(OccurrencePath::root().child(soliddraft_assembly::Step::component(id)))
}

fn save(state: &mut AppState, path: Option<PathBuf>) {
    let Some(s) = state.assembly.session.as_mut() else { return };
    if path.is_none() && s.doc.path().is_none() {
        return apply_save_as(state);
    }
    match s.doc.save(path.as_deref()) {
        Ok(saved) => {
            if let Ok(bytes) = std::fs::read(&saved) {
                state.revisions.record(s.doc.id(), &s.doc.revision(), &bytes);
            }
            crate::file::remember(&saved);
            state.recent = crate::file::recent();
            state.note(format!("Saved {}", saved.display()), NoteKind::Ok);
        }
        Err(e) => state.note(format!("cannot save the assembly: {e}"), NoteKind::Error),
    }
}

fn apply_save_as(state: &mut AppState) {
    state.push(AppAction::Assembly(AssemblyAction::SaveAs));
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// The assembly tab: the viewport with the assembly's parts.
pub fn viewport(ui: &mut egui::Ui, state: &mut AppState) {
    let Some(s) = state.assembly.session.as_mut() else { return };
    let ctx = ui.ctx().clone();
    let frame = s.frame(&ctx);
    let hover_label = s.shown.1.map(|r| s.doc.label(r));
    let empty = s.components().is_empty();
    let events = s.viewport.show_assembly(ui, frame, hover_label.as_deref());
    let picked = |s: &AssemblySession, texel: Option<[u32; 2]>| {
        let scene = s.scene.lock().expect("the scene lock");
        texel.and_then(|t| scene.decode(t)).map(|p| p.occurrence)
    };
    let hovered = picked(s, events.hover_texel);
    if hovered != s.shown.1 {
        s.scene.lock().expect("the scene lock").set_hover(s.doc.table(), hovered);
        s.shown.1 = hovered;
    }
    let rect = s.viewport.rect;
    let mut actions = Vec::new();
    if let Some(texel) = events.clicked_texel {
        let row = picked(s, texel);
        // A click selects the root's component the part belongs to, as SolidWorks does.
        let top = row.map(|mut r| {
            while let Some(p) = s.doc.table().parent(r).filter(|p| *p != OccurrenceIndex::ROOT) {
                r = p;
            }
            s.doc.table().path(r)
        });
        actions.push(AppAction::Assembly(AssemblyAction::Select(top)));
    }
    if let Some(v) = events.set_view {
        s.viewport.camera.set_view(v);
    }
    if events.fit {
        actions.push(AppAction::Assembly(AssemblyAction::ZoomFit));
    }
    if let Some(d) = events.set_display {
        s.viewport.display = d;
    }
    if events.toggle_projection {
        s.viewport.camera.orthographic = !s.viewport.camera.orthographic;
    }
    for a in actions {
        state.push(a);
    }
    let dropping = ctx.input(|i| !i.raw.hovered_files.is_empty());
    if empty || dropping {
        let text = if dropping {
            "Drop to insert the parts"
        } else {
            "Insert component, or drop part files here, to place parts"
        };
        let painter = ui.painter().with_clip_rect(rect);
        let galley = painter.layout_no_wrap(text.to_owned(), theme::font_medium(13.0), theme::muted());
        let r = Rect::from_center_size(rect.center(), galley.size() + vec2(28.0, 14.0));
        painter.rect_filled(r, egui::CornerRadius::same(99), theme::hud_bg());
        painter.rect_stroke(r, egui::CornerRadius::same(99), Stroke::new(1.0, theme::line()), egui::StrokeKind::Inside);
        painter.galley(r.center() - galley.size() / 2.0, galley, theme::muted());
    }
}

/// The assembly tree: the assembly, then a row per component; a sub-assembly opens to
/// show its own.
pub fn tree(ui: &mut egui::Ui, state: &mut AppState) {
    let Some(s) = state.assembly.session.as_mut() else { return };
    let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
    ui.painter().text(
        pos2(head.left() + 12.0, head.center().y),
        Align2::LEFT_CENTER,
        s.title(),
        theme::font_semibold(12.5),
        theme::ink(),
    );
    let counts = s.doc.table().counts();
    let (status, color) = if s.doc.is_loading() {
        ("Loading…".to_owned(), theme::muted())
    } else {
        let failed = s.doc.store().counts()[5];
        if failed > 0 {
            (format!("{failed} not loaded"), theme::error())
        } else {
            (format!("{} part{}", counts.parts, plural(counts.parts)), theme::ok())
        }
    };
    ui.painter().text(
        pos2(head.right() - 12.0, head.center().y),
        Align2::RIGHT_CENTER,
        status,
        theme::font_medium(11.0),
        color,
    );
    ui.painter().hline(head.x_range(), head.bottom() - 0.5, Stroke::new(1.0, theme::line()));
    let mut actions = Vec::new();
    egui::ScrollArea::vertical().id_salt("assembly_tree").auto_shrink([false, false]).show(ui, |ui| {
        ui.add_space(4.0);
        let selected = s.selected_row();
        let root = s.doc.assembly().name.clone();
        let r = row(ui, RowSpec { icon: Icon::Tree, name: &root, selected: selected.is_none(), ..RowSpec::plain() });
        if r.clicked() {
            actions.push(AppAction::Assembly(AssemblyAction::Select(None)));
        }
        let mut stack: Vec<(OccurrenceIndex, u8)> = s.components().into_iter().rev().map(|r| (r, 1)).collect();
        while let Some((r, indent)) = stack.pop() {
            let table = s.doc.table();
            let flags = table.flags(r);
            let path = table.path(r);
            let def = s.doc.store().get(table.definition(r));
            let sub = flags.has(Flags::ASSEMBLY);
            let open = sub && s.expanded.contains(&path);
            let name = s.doc.label(r);
            let problem = match (&def.state, &def.freshness) {
                (LoadState::Failed, _) => Some(def.problem.clone().unwrap_or_else(|| "Not loaded".into())),
                _ => None,
            };
            let warn = match (&def.state, &def.freshness) {
                (LoadState::Stale, _) => def.problem.clone(),
                (_, Freshness::NewerAvailable { .. }) => Some("A newer version is saved; Rebuild to use it".into()),
                (_, Freshness::Missing) => Some("The file cannot be found; the assembly shows its copy".into()),
                _ => None,
            };
            let sub_text = if flags.has(Flags::HIDDEN_SELF) { "hidden" } else { "" };
            let resp = row(
                ui,
                RowSpec {
                    indent,
                    chevron: sub.then_some(open),
                    icon: if sub { Icon::Tree } else { Icon::Extrude },
                    name: &name,
                    sub: sub_text,
                    selected: Some(r) == selected,
                    dim: flags.has(Flags::HIDDEN) || flags.has(Flags::SUPPRESSED),
                    error: problem.as_deref(),
                    warn: warn.as_deref(),
                    suppressed: flags.has(Flags::SUPPRESSED_SELF),
                    ..RowSpec::plain()
                },
            );
            let resp = match problem.as_deref().or(warn.as_deref()) {
                Some(why) => resp.on_hover_text(why),
                None => resp,
            };
            if resp.clicked() {
                if sub
                    && resp.interact_pointer_pos().is_some_and(|p| p.x < resp.rect.left() + 18.0 + indent as f32 * 20.0)
                {
                    if !s.expanded.remove(&path) {
                        s.expanded.insert(path.clone());
                    }
                } else {
                    actions.push(AppAction::Assembly(AssemblyAction::Select(Some(path.clone()))));
                }
            }
            if open {
                let children: Vec<OccurrenceIndex> =
                    table.children(r).filter(|c| !table.flags(*c).has(Flags::REMOVED)).collect();
                stack.extend(children.into_iter().rev().map(|c| (c, indent + 1)));
            }
        }
    });
    for a in actions {
        state.push(a);
    }
}

/// The property panel: the selected component's placement, typed; or the assembly's
/// summary.
pub fn properties(ui: &mut egui::Ui, state: &mut AppState) {
    let Some(s) = state.assembly.session.as_mut() else { return };
    let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
    let title = match s.selected_row() {
        Some(r) => s.doc.label(r),
        None => s.doc.assembly().name.clone(),
    };
    icons::paint(
        ui.painter(),
        if s.selected_row().is_some() { Icon::Extrude } else { Icon::Tree },
        Rect::from_center_size(pos2(head.left() + 20.0, head.center().y), vec2(16.0, 16.0)),
        theme::ink(),
    );
    ui.painter().text(
        pos2(head.left() + 34.0, head.center().y),
        Align2::LEFT_CENTER,
        title,
        theme::font_semibold(12.5),
        theme::ink(),
    );
    ui.painter().hline(head.x_range(), head.bottom() - 0.5, Stroke::new(1.0, theme::line()));
    let mut actions = Vec::new();
    egui::Frame::new().inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
        let Some(draft) = s.draft.as_mut() else {
            let counts = s.doc.table().counts();
            theme::section_label(ui, "Assembly");
            ui.label(format!("{} component{}", s.components().len(), plural(s.components().len())));
            ui.label(format!("{} part{} placed", counts.parts, plural(counts.parts)));
            ui.label(match s.doc.path() {
                Some(p) => format!("Saved as {}", p.display()),
                None => "Not saved yet".to_owned(),
            });
            ui.add_space(6.0);
            ui.label(egui::RichText::new("Select a component to place it.").color(theme::muted()));
            return;
        };
        theme::section_label(ui, "Placement");
        ui.label(
            egui::RichText::new("Relative to the assembly that holds it. Each change is one undo step.")
                .size(11.0)
                .color(theme::muted()),
        );
        ui.add_space(4.0);
        // A change applies once the field lets go: a drag ends or typing leaves it.
        let mut busy = false;
        egui::Grid::new("assembly_placement").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            let rows = ["X", "Y", "Z"].into_iter().zip(draft.translation.iter_mut()).map(|(a, v)| (a, v, "mm"));
            let turns = ["Rotate X", "Rotate Y", "Rotate Z"].into_iter().zip(draft.rotation.iter_mut());
            for (label, value, unit) in rows.chain(turns.map(|(a, v)| (a, v, "°"))) {
                ui.label(label);
                let resp = ui.add(egui::DragValue::new(value).speed(1.0).max_decimals(4).suffix(format!(" {unit}")));
                busy |= resp.dragged() || resp.has_focus();
                ui.end_row();
            }
        });
        if !busy && draft.edited() {
            actions.push(AppAction::Assembly(AssemblyAction::Place(draft.path.clone(), draft.placement())));
        }
    });
    // The fields fill again from the moved placement, or the old one when it was refused.
    if !actions.is_empty() {
        s.draft = None;
    }
    for a in actions {
        state.push(a);
    }
}

/// The status bar while the assembly is active.
pub fn status_bar(ui: &mut egui::Ui, state: &mut AppState) {
    let Some(s) = state.assembly.session.as_ref() else { return };
    let muted = |t: String| egui::RichText::new(t).size(11.5).color(theme::muted());
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 18.0;
        ui.add_space(4.0);
        let counts = s.doc.table().counts();
        if s.doc.is_loading() {
            ui.label(egui::RichText::new("Loading parts…").family(theme::semibold()).size(11.5).color(theme::muted()));
        } else {
            ui.label(egui::RichText::new("Assembly").family(theme::semibold()).size(11.5).color(theme::ok()));
        }
        let n = s.components().len();
        ui.label(muted(format!("{n} component{}", plural(n))));
        ui.label(muted(format!("{} part{} placed", counts.parts, plural(counts.parts))));
        ui.label(muted("Editing assembly".to_owned()));
        if let Some(note) = &state.note
            && note.at.elapsed() < Duration::from_secs(6)
        {
            let color = match note.kind {
                NoteKind::Info => theme::muted(),
                NoteKind::Ok => theme::ok(),
                NoteKind::Error => theme::error(),
            };
            ui.label(egui::RichText::new(&note.text).size(11.5).color(color));
        }
    });
}
