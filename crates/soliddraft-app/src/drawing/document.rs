//! The open drawing: the `soliddraft-drawing` document, its undo stack, its file, and what
//! the interface keeps beside it (the views' transforms and circles for picking, the
//! last update for the banner). Every change goes through [`DrawingDoc::apply`] with a
//! `soliddraft-drawing` [`Command`].

use super::part::PartContext;
use soliddraft_drawing::file::{self, FileMeta};
use soliddraft_drawing::library::{self, StyleFile, TemplateFile};
use soliddraft_drawing::measure::display_text;
use soliddraft_drawing::project::{self, CircleInfo, Projected, ViewTransform};
use soliddraft_drawing::{
    AnnotationId, AnnotationState, Command, ContentHash, Dimension, DocumentId, Drawing, UndoStack, View, ViewDisplay,
    ViewId, ViewKind, ViewScale,
};
use soliddraft_kernel_api::Point2;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use web_time::Instant;

/// What the last update from the model did, for the banner.
#[derive(Clone, Debug)]
pub struct UpdateReport {
    pub from_revision: ContentHash,
    pub to_revision: ContentHash,
    /// What changed in the part, in the part's words.
    pub change: String,
    pub views: usize,
    pub changed: Vec<AnnotationId>,
    pub dangling: Vec<AnnotationId>,
    pub at: Instant,
    pub dismissed: bool,
}

/// The open drawing document.
pub struct DrawingDoc {
    /// The drawing's id, kept in its file's manifest.
    pub id: DocumentId,
    pub drawing: Drawing,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    stack: UndoStack,
    /// Model-to-view transforms of the views, from the last projection. Not saved.
    pub transforms: HashMap<ViewId, ViewTransform>,
    /// Circles seen face-on per view, from the last projection. Not saved.
    pub circles: HashMap<ViewId, Vec<CircleInfo>>,
    pub last_update: Option<UpdateReport>,
    /// The part revision the drawing was last compared with.
    pub seen_digest: ContentHash,
    /// How many commands the part's log held at the last update, so the banner can say
    /// what changed since. Not saved.
    pub log_mark: usize,
}

impl DrawingDoc {
    pub fn new(drawing: Drawing) -> DrawingDoc {
        let seen = drawing.model.pinned.clone();
        DrawingDoc {
            id: DocumentId::new(),
            drawing,
            path: None,
            dirty: false,
            stack: UndoStack::new(),
            transforms: HashMap::new(),
            circles: HashMap::new(),
            last_update: None,
            seen_digest: seen,
            log_mark: 0,
        }
    }

    pub fn template(&self) -> &'static TemplateFile {
        library::template(self.drawing.sheets.first().map(|s| s.template.as_str()).unwrap_or("iso-a3"))
    }

    pub fn style(&self) -> &'static StyleFile {
        self.template().style_file()
    }

    pub fn can_undo(&self) -> bool {
        self.stack.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.stack.can_redo()
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.stack.undo_name()
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.stack.redo_name()
    }

    pub fn undo(&mut self) -> Option<String> {
        let name = self.stack.undo(&mut self.drawing)?;
        self.dirty = true;
        Some(name.to_owned())
    }

    pub fn redo(&mut self) -> Option<String> {
        let name = self.stack.redo(&mut self.drawing)?;
        self.dirty = true;
        Some(name.to_owned())
    }

    /// True when the part changed since the drawing's views were made: the drawing
    /// still shows the revision it is pinned at.
    pub fn is_stale(&self) -> bool {
        self.seen_digest != self.drawing.model.pinned
    }

    /// Views whose geometry is older than the part.
    pub fn stale_views(&self) -> Vec<ViewId> {
        let digest = self.seen_digest.as_str();
        self.drawing
            .sheets
            .iter()
            .flat_map(|s| &s.views)
            .filter(|v| v.geometry.source_digest != digest && !v.excluded_from_update)
            .map(|v| v.id)
            .collect()
    }

    pub fn attention_count(&self) -> usize {
        self.drawing.annotations_needing_attention().count()
    }

    pub fn dangling_count(&self) -> usize {
        self.drawing
            .annotations_needing_attention()
            .filter(|a| matches!(a.state, AnnotationState::Dangling { .. }))
            .count()
    }

    /// Applies a command, with the part when the command reads it.
    pub fn apply(&mut self, cmd: Command, part: Option<&PartContext<'_>>) -> Result<(), String> {
        let from = self.drawing.model.pinned.clone();
        let p = part.map(|c| c.part());
        // Commands that remake one view's line work through the kernel; its transform
        // and circles are remembered again afterwards, since a flipped section or a
        // cropped view sees the part from a new frame.
        let remade = match &cmd {
            Command::EditView { view, .. }
            | Command::SetViewCrop { view, .. }
            | Command::AddBrokenOut { view, .. }
            | Command::RemoveBrokenOut { view, .. }
            | Command::ReattachView { view, .. } => Some(*view),
            Command::SetViewDisplay { view, display } => {
                let before = self.drawing.view(*view).map(|v| v.display);
                before.filter(|b| b.tangent != display.tangent || b.seams != display.seams).map(|_| *view)
            }
            _ => None,
        };
        let report = self.stack.run(&mut self.drawing, &cmd, p.as_ref()).map_err(|e| e.to_string())?;
        self.dirty = true;
        if let (Some(id), Some(p)) = (remade, p.as_ref())
            && let Ok(index) = soliddraft_drawing::PartIndex::build(p)
            && let Ok(ctx) = project::ViewContext::compute(&self.drawing, p, &index, id)
        {
            self.remember(id, &ctx.projected);
        }
        if let (Some(r), Some(ctx)) = (report, part) {
            for (id, projected) in &r.projections {
                self.remember(*id, projected);
            }
            self.seen_digest = ctx.digest.clone();
            let changed = r.changed.iter().map(|c| c.annotation).collect();
            let dangling = r.dangling.iter().map(|d| d.annotation).collect();
            let log = ctx.doc.command_log();
            let change = log.get(self.log_mark..).map(|l| l.join(", ")).unwrap_or_default();
            self.log_mark = log.len();
            self.last_update = Some(UpdateReport {
                from_revision: from,
                to_revision: ctx.digest.clone(),
                change,
                views: r.projections.len(),
                changed,
                dangling,
                at: Instant::now(),
                dismissed: false,
            });
        }
        Ok(())
    }

    /// The last projection of a view, rebuilt from the cached geometry.
    pub fn projected_of(&self, view: ViewId) -> Option<Projected> {
        let v = self.drawing.view(view)?;
        let transform = *self.transforms.get(&view)?;
        Some(Projected {
            geometry: v.geometry.clone(),
            transform,
            circles: self.circles.get(&view).cloned().unwrap_or_default(),
        })
    }

    /// The next free annotation id.
    pub fn next_annotation_id(&self) -> AnnotationId {
        self.drawing.next_annotation_id()
    }

    /// Projects a new view of the part for placing. The view is not added.
    pub fn project_new(
        &self,
        ctx: &PartContext<'_>,
        sheet: usize,
        kind: &ViewKind,
        display: ViewDisplay,
        scale: ViewScale,
    ) -> Result<(View, Projected), String> {
        let s = self.drawing.sheets.get(sheet).ok_or("no such sheet")?;
        let mut view = View {
            id: self.drawing.next_view_id(),
            kind: kind.clone(),
            position: Point2::new(0.0, 0.0),
            alignment: None,
            scale,
            display,
            excluded_from_update: false,
            geometry: Default::default(),
            label: None,
            crop: None,
            broken_out: Vec::new(),
        };
        let mut with_view = s.clone();
        with_view.views.push(view.clone());
        let spec = project::spec_of(&with_view, &view).ok_or("the view has no orientation")?;
        let k = project::effective_scale(&with_view, &view);
        let part = ctx.part();
        let index = soliddraft_drawing::PartIndex::build(&part).map_err(|e| e.to_string())?;
        let p = project::project_part(&part, &index, &spec, k).map_err(|e| e.to_string())?;
        view.geometry = p.geometry.clone();
        Ok((view, p))
    }

    /// Remembers a projection's transform and circles after its view was placed.
    pub fn remember(&mut self, view: ViewId, p: &Projected) {
        self.transforms.insert(view, p.transform);
        self.circles.insert(view, p.circles.clone());
    }

    /// A dimension's display text in the drawing's style.
    pub fn dimension_text(&self, d: &Dimension) -> String {
        display_text(d, &self.style().symbols)
    }

    /// Writes the drawing file and remembers where. The id is kept: saving under
    /// another name is a rename, not a copy.
    pub fn save(&mut self, path: &Path, kernel: &str) -> Result<(), String> {
        let meta = FileMeta { id: self.id, ..FileMeta::new(kernel, self.drawing.meta.units) };
        let bytes = file::to_bytes(&self.drawing, &meta).map_err(|e| e.to_string())?;
        crate::platform::write_file(path, &bytes)?;
        self.path = Some(path.to_path_buf());
        self.dirty = false;
        Ok(())
    }

    /// Opens a drawing file. It shows the part revision it is pinned at; whether the
    /// part has moved on is the resolver's answer (see `super::resolve_model`).
    pub fn open(path: &Path) -> Result<DrawingDoc, String> {
        let (drawing, meta) = file::read(path).map_err(|e| e.to_string())?;
        let mut doc = DrawingDoc::new(drawing);
        doc.id = meta.id;
        doc.path = Some(path.to_path_buf());
        Ok(doc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soliddraft_drawing::fixture::bracket_a3;
    use soliddraft_drawing::{AnnotationKind, Scale};

    #[test]
    fn aligned_views_slide_along_their_guide_and_follow_their_parent() {
        let mut doc = DrawingDoc::new(bracket_a3());
        let (front, top, left) = (ViewId(1), ViewId(2), ViewId(3));
        let p0 = doc.drawing.view(front).unwrap().position;
        // Top is aligned vertically under Front: it may move up and down only.
        doc.apply(Command::MoveView { view: top, to: Point2::new(150.0, 80.0) }, None).unwrap();
        assert_eq!(doc.drawing.view(top).unwrap().position, Point2::new(p0.u, 80.0));
        // Moving Front drags both children.
        doc.apply(Command::MoveView { view: front, to: Point2::new(110.0, 200.0) }, None).unwrap();
        assert_eq!(doc.drawing.view(top).unwrap().position, Point2::new(110.0, 90.0));
        assert_eq!(doc.drawing.view(left).unwrap().position.v, 200.0);
        // Breaking the alignment frees the view.
        doc.apply(Command::BreakAlignment { view: left }, None).unwrap();
        doc.apply(Command::MoveView { view: left, to: Point2::new(300.0, 150.0) }, None).unwrap();
        assert_eq!(doc.drawing.view(left).unwrap().position, Point2::new(300.0, 150.0));
        assert_eq!(doc.undo_label(), Some("Move view"));
        doc.undo();
        assert_eq!(doc.drawing.view(left).unwrap().position.v, 200.0);
        doc.redo();
        assert_eq!(doc.drawing.view(left).unwrap().position.u, 300.0);
    }

    #[test]
    fn freeze_as_note_keeps_the_text_and_drops_the_references() {
        let mut doc = DrawingDoc::new(bracket_a3());
        let dangling = doc
            .drawing
            .annotations_needing_attention()
            .find(|a| matches!(a.state, AnnotationState::Dangling { .. }))
            .unwrap()
            .id;
        doc.apply(Command::FreezeAsNote { annotation: dangling }, None).unwrap();
        let a = doc.drawing.annotation(dangling).unwrap();
        assert!(matches!(&a.body, AnnotationKind::Note { text, .. } if text == "12 (ref)"));
        assert!(a.anchors.is_empty());
        assert_eq!(doc.attention_count(), 1);
    }

    #[test]
    fn sheet_scale_rescales_views_that_follow_it() {
        let mut doc = DrawingDoc::new(bracket_a3());
        let width = |doc: &DrawingDoc, id: u32| {
            let b = doc.drawing.view(ViewId(id)).unwrap().geometry.bbox;
            b[2] - b[0]
        };
        let (w0, i0) = (width(&doc, 1), width(&doc, 4));
        doc.apply(Command::SetSheetScale { sheet: 0, scale: Scale::new(1, 2) }, None).unwrap();
        assert!((width(&doc, 1) - w0 / 2.0).abs() < 1e-3);
        assert!((width(&doc, 4) - i0).abs() < 1e-9, "a custom-scale view keeps its size");
    }
}
