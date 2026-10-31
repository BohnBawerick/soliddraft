//! The registry's commands as tools over the app's state, for MCP clients and scripts.
//!
//! [`CommandTools`] is the part's command layer: the assistant's tools and the recipe
//! verbs. [`AppTools`] serves every command whose registry row has a tool: those, and
//! the file, export and drawing commands the ribbon runs, over the whole [`AppState`]
//! with its drawing. The MCP server in the app and on the command line, and
//! `soliddraft run`, serve it.
//!
//! A tool takes by name what the ribbon takes from the selection, and runs the same
//! typed commands and drawing helpers the app's action runs, so it is checked and undone
//! as the ribbon's is (the row's [`Undo`](crate::commands::Undo)). A command that needs
//! the window has no tool; its row says why.

use std::path::PathBuf;

use serde_json::{Value, json};
use soliddraft_assistant::tools::schema::{any_of, array, boolean, integer, number, one_of, string};
use soliddraft_assistant::{Caller, ToolHost, ToolOutput, ToolSpec};
use soliddraft_document::DocKind;
use soliddraft_drawing::display::{annotation_name, field_value};
use soliddraft_drawing::export::Format as SheetFormat;
use soliddraft_drawing::measure::value_text;
use soliddraft_drawing::project::{self, view_label};
use soliddraft_drawing::{
    AnnotationId, AnnotationKind, AnnotationState, Boundary, Command as DrawingCommand, Drawing, HiddenLines,
    PartIndex, SHEET, TangentEdges, ViewDisplay, ViewId, ViewKind, derived,
};
use soliddraft_exchange::Format as ExportFormat;
use soliddraft_kernel_api::{Point2, Vec3};

use crate::command_tools::{CommandTools, Surface, tool_specs};
use crate::document::{Command, Document};
use crate::drawing::document::DrawingDoc;
use crate::drawing::part::PartContext;
use crate::drawing::reference::LinkState;
use crate::drawing::{self, DrawingSession, NewDrawing};
use crate::state::AppState;

const NO_DRAWING: &str = "no drawing is open: make_drawing makes one of the part, and open opens a .dwdrw file";

/// A view, by its id or by a label only it has. Labels repeat: two sheets can each have
/// a Front view, so model_summary lists every view with its id.
fn view_schema(what: &str) -> Vec<Value> {
    vec![
        integer(&format!("{what}: the view's id, as model_summary lists it under drawing")),
        string(&format!(
            "{what}: a label only that view has, such as Front view or Section A-A, ignoring case; a label several \
             views share is refused, and their ids are listed"
        )),
    ]
}

fn view_param() -> (&'static str, Value) {
    ("view", any_of(view_schema("The view")))
}

fn path_param(what: &str) -> Vec<(&'static str, Value)> {
    vec![("path", string(what))]
}

/// The `open` tool's parameters.
pub fn open_params() -> Vec<(&'static str, Value)> {
    path_param("A part, drawing or assembly file path, ending in .dwprt, .dwdrw or .dwasm")
}

/// The `export_step` tool's parameters.
pub fn step_path_params() -> Vec<(&'static str, Value)> {
    path_param("The file to write, ending in .step or .stp")
}

/// The `export_stl` tool's parameters.
pub fn stl_path_params() -> Vec<(&'static str, Value)> {
    path_param("The file to write, ending in .stl")
}

/// The `export_3mf` tool's parameters.
pub fn three_mf_path_params() -> Vec<(&'static str, Value)> {
    path_param("The file to write, ending in .3mf")
}

/// The `export_obj` tool's parameters.
pub fn obj_path_params() -> Vec<(&'static str, Value)> {
    path_param("The file to write, ending in .obj")
}

/// The `drawing_export_pdf` tool's parameters.
pub fn pdf_path_params() -> Vec<(&'static str, Value)> {
    path_param("The file to write, ending in .pdf")
}

/// The `drawing_export_dxf` tool's parameters.
pub fn dxf_path_params() -> Vec<(&'static str, Value)> {
    path_param("The file to write, ending in .dxf")
}

/// The `drawing_export_svg` tool's parameters.
pub fn svg_path_params() -> Vec<(&'static str, Value)> {
    path_param("The file to write, ending in .svg")
}

/// The `drawing_save` tool's parameters.
pub fn drawing_path_params() -> Vec<(&'static str, Value)> {
    path_param("A drawing file path ending in .dwdrw")
}

/// The `make_drawing` tool's parameters.
pub fn make_drawing_params() -> Vec<(&'static str, Value)> {
    vec![(
        "template",
        string(
            "The sheet template's id: iso-a3 or iso-a4 (first angle), ansi-a or ansi-b (third angle), or one File \
             locations adds",
        ),
    )]
}

/// The parameters of a tool that takes one view.
pub fn view_params() -> Vec<(&'static str, Value)> {
    vec![view_param()]
}

/// The `drawing_hidden_lines` tool's parameters.
pub fn hidden_lines_params() -> Vec<(&'static str, Value)> {
    vec![view_param(), ("shown", boolean("true draws the hidden lines dashed, false removes them"))]
}

/// The `drawing_tangent_edges` tool's parameters.
pub fn tangent_edges_params() -> Vec<(&'static str, Value)> {
    vec![view_param(), ("tangent", one_of(&["removed", "phantom", "visible"], "How the view draws tangent edges"))]
}

/// The `drawing_crop` tool's parameters.
pub fn crop_params() -> Vec<(&'static str, Value)> {
    vec![
        view_param(),
        (
            "crop",
            any_of(vec![
                array(
                    number("mm"),
                    "The rectangle to keep, as two opposite corners [u1, v1, u2, v2] in sheet millimetres from the \
                     view's centre, each side at least 1 mm",
                ),
                json!({ "type": "null", "description": "Shows the whole view again" }),
            ]),
        ),
    ]
}

/// The `drawing_section` tool's parameters.
pub fn section_params() -> Vec<(&'static str, Value)> {
    vec![
        ("view", any_of(view_schema("The view to cut"))),
        (
            "through",
            array(number("mm"), "A model point [x, y, z] the cutting plane passes through, such as a hole's centre"),
        ),
        (
            "direction",
            one_of(&["horizontal", "vertical"], "The cutting line's direction across the view, on the sheet"),
        ),
        ("flip", boolean("true looks the other way along the cutting plane")),
    ]
}

/// The `drawing_auto_update` tool's parameters.
pub fn auto_update_params() -> Vec<(&'static str, Value)> {
    vec![("on", boolean("true rebuilds the drawing after every change of the open part"))]
}

/// The `drawing_auto_centre_marks` tool's parameters.
pub fn centre_marks_params() -> Vec<(&'static str, Value)> {
    vec![(
        "view",
        any_of(
            view_schema("The view whose circles get marks")
                .into_iter()
                .chain([json!({ "type": "null", "description": "Every view of the current sheet" })])
                .collect(),
        ),
    )]
}

/// The `drawing_delete` tool's parameters.
pub fn drawing_delete_params() -> Vec<(&'static str, Value)> {
    let or_null = |s: Value| any_of(vec![s, json!({ "type": "null" })]);
    vec![
        (
            "view",
            any_of(
                view_schema("The view to delete, with its annotations")
                    .into_iter()
                    .chain([json!({ "type": "null", "description": "Deletes the annotation instead" })])
                    .collect(),
            ),
        ),
        ("annotation", or_null(integer("The annotation's id, as model_summary lists it, or null to delete a view"))),
    ]
}

/// The registry's commands that have a tool, over the app's state.
pub struct AppTools<'a> {
    /// The part, its proposals and the drawing.
    pub state: &'a mut AppState,
}

impl AppTools<'_> {
    fn part(&mut self) -> CommandTools<'_> {
        let st = &mut *self.state;
        CommandTools {
            doc: &mut st.doc,
            book: &mut st.chat.book,
            surface: Surface::Mcp,
            drawing: st.drawing.session.as_mut().map(|s| &mut s.doc),
        }
    }

    /// What `model_summary` says: the part, and the open drawing.
    pub fn summary(&mut self) -> Value {
        let mut v = self.part().summary();
        let mut proof = self.state.doc.proof_state();
        if let Some(drag) = &self.state.drag_preview
            && let Some(request) = drag.current_request()
        {
            proof.requested_edit = Some(request);
            proof.displayed_edit = proof.displayed_edit.or_else(|| drag.displayed_request());
            proof.proved_edit = drag.proved_request();
            proof.transient = true;
            proof.pending = drag.pending();
            proof.failed = matches!(drag.shown(), crate::drag::Shown::Failed { .. });
        }
        v["edit_state"] = serde_json::json!(proof);
        if proof.transient {
            for key in ["volume_mm3", "area_mm2", "faces", "edges"] {
                v["body"][key] = Value::Null;
            }
        }
        if let Some(d) = drawing_json(self.state) {
            v["drawing"] = d;
        }
        if let Some(a) = crate::assembly::tools::summary(self.state) {
            v["assembly"] = a;
        }
        v
    }

    /// Runs the tools this host adds to the part's layer. `None` hands the call on.
    fn run(&mut self, name: &str, input: &Value) -> Option<Result<Value, String>> {
        if let Some(result) = crate::assembly::tools::run(self.state, name, input) {
            return Some(result);
        }
        if let Some(result) = crate::mates::tools::run(self.state, name, input) {
            return Some(result);
        }
        if let Some(result) = crate::grid::tools::run(self.state, name, input) {
            return Some(result);
        }
        if let Some(result) = crate::subassembly::tools::run(self.state, name, input) {
            return Some(result);
        }
        let drawing_active = self.state.drawing.is_active();
        let assembly_active = self.state.assembly.is_active();
        Some(match name {
            "model_summary" => Ok(self.summary()),
            "undo" if assembly_active => crate::assembly::tools::step(self.state, true),
            "redo" if assembly_active => crate::assembly::tools::step(self.state, false),
            "undo" if drawing_active => self.step_drawing(true),
            "redo" if drawing_active => self.step_drawing(false),
            "open" => self.open(input)?,
            "new_part" => self.new_part(),
            "export_step" => self.export_part(ExportFormat::Step, input),
            "export_stl" => self.export_part(ExportFormat::Stl, input),
            "export_3mf" => self.export_part(ExportFormat::ThreeMf, input),
            "export_obj" => self.export_part(ExportFormat::Obj, input),
            "export_stand_in" => self.export_part(ExportFormat::StandIn, input),
            "roll_to_end" => self.roll_to_end(),
            "make_drawing" => self.make_drawing(input),
            "show_drawing" => self.session().map(|_| ()).map(|()| {
                self.state.assembly.active = false;
                self.state.drawing.active = true;
                self.drawing_now()
            }),
            "drawing_show_part" => {
                self.state.assembly.active = false;
                self.state.drawing.active = false;
                Ok(json!({ "active": "part" }))
            }
            "drawing_align" => self.align(input),
            "drawing_break_alignment" => self.break_alignment(input),
            "drawing_hidden_lines" => self.hidden_lines(input),
            "drawing_tangent_edges" => self.tangent_edges(input),
            "drawing_crop" => self.crop(input),
            "drawing_section" => self.section(input),
            "drawing_rebuild" => self.rebuild(),
            "drawing_replace_model" => self.repoint(input, true),
            "drawing_repair_reference" => self.repoint(input, false),
            "drawing_auto_update" => self.auto_update(input),
            "drawing_model_items" => self.session().map(|_| ()).and_then(|()| {
                let said = drawing::import_model_items(self.state)?;
                Ok(self.drawing_said(said))
            }),
            "drawing_auto_centre_marks" => self.centre_marks(input),
            "drawing_add_sheet" => self.session().and_then(drawing::add_sheet).map(|name| self.drawing_said(name)),
            "drawing_export_pdf" => self.export_sheet(SheetFormat::Pdf, input),
            "drawing_export_dxf" => self.export_sheet(SheetFormat::Dxf, input),
            "drawing_export_svg" => self.export_sheet(SheetFormat::Svg, input),
            "drawing_save" => self.save_drawing(input),
            "drawing_close" => self.session().map(|_| ()).map(|()| {
                self.state.drawing.session = None;
                self.state.drawing.active = false;
                json!({ "closed": true })
            }),
            "drawing_delete" => self.delete(input),
            _ => return None,
        })
    }

    fn session(&mut self) -> Result<&mut DrawingSession, String> {
        self.state.drawing.session.as_mut().ok_or_else(|| NO_DRAWING.to_owned())
    }

    fn drawing_now(&self) -> Value {
        json!({ "drawing": drawing_json(self.state) })
    }

    fn drawing_said(&self, said: String) -> Value {
        json!({ "done": said, "drawing": drawing_json(self.state) })
    }

    /// Undo or redo in the drawing, while it is the active document.
    fn step_drawing(&mut self, undo: bool) -> Result<Value, String> {
        let s = self.session()?;
        if undo {
            let label = drawing::undo(s).ok_or("nothing to undo in the drawing")?;
            Ok(json!({ "undone": label, "document": "drawing", "undo": s.doc.undo_label() }))
        } else {
            let label = drawing::redo(s).ok_or("nothing to redo in the drawing")?;
            Ok(json!({ "redone": label, "document": "drawing" }))
        }
    }

    /// A drawing file opens here; a part file opens in the part's layer.
    fn open(&mut self, input: &Value) -> Option<Result<Value, String>> {
        let path = PathBuf::from(input["path"].as_str().unwrap_or_default());
        match DocKind::of_path(&path) {
            Some(DocKind::Part) => None,
            Some(DocKind::Assembly) => Some(crate::assembly::tools::open(self.state, path)),
            Some(DocKind::Drawing) => Some(self.open_drawing(path)),
            _ => Some(Err("the path must name a .dwprt part, .dwdrw drawing or .dwasm assembly file".into())),
        }
    }

    fn open_drawing(&mut self, path: PathBuf) -> Result<Value, String> {
        let doc = DrawingDoc::open(&path)?;
        self.state.assembly.active = false;
        let said = drawing::put_opened(self.state, doc);
        let missing = self.state.drawing.session.as_ref().is_some_and(|s| s.link_state() == LinkState::Missing);
        let part = if missing {
            "not found: drawing_repair_reference points the drawing at the part file".to_owned()
        } else {
            self.open_model()?
        };
        Ok(json!({ "opened": path, "done": said, "part": part, "drawing": drawing_json(self.state) }))
    }

    /// Opens the drawing's part in place of the open part, as the app does after opening
    /// a drawing or repairing its reference. Says which part is open.
    fn open_model(&mut self) -> Result<String, String> {
        if let Some(file) = drawing::model_to_open(self.state)? {
            let caller = Caller::Mcp { client: "drawing".into() };
            let out = self.part().call("open", json!({ "path": file }), &caller);
            if out.is_error {
                return Err(format!("the drawing's part did not open: {}", out.content));
            }
        }
        Ok(self.state.doc.name.clone())
    }

    fn new_part(&mut self) -> Result<Value, String> {
        let st = &mut *self.state;
        let kernel = st.doc.kernel().clone();
        st.editor = None;
        st.sketch = None;
        st.chat.preview = None;
        st.drag_preview = None;
        st.assembly.active = false;
        st.drawing.active = false;
        st.doc = Document::new("Part1", kernel);
        Ok(self.summary())
    }

    /// Writes the part's body as `format`, as File › Export does.
    fn export_part(&mut self, format: ExportFormat, input: &Value) -> Result<Value, String> {
        let path = PathBuf::from(input["path"].as_str().unwrap_or_default());
        if !format.suits(&path) {
            return Err(format!("the path must end in .{} for {}", format.extension(), format.name()));
        }
        let doc = &self.state.doc;
        if doc.display_body().is_none() {
            return Err("the part has no body to export".into());
        }
        let out = self.state.export_part(format)?;
        crate::platform::write_file(&path, &out.bytes)?;
        Ok(json!({ "exported": path, "report": out.report.summary() }))
    }

    fn roll_to_end(&mut self) -> Result<Value, String> {
        if self.state.doc.rollback.is_none() {
            return Err("the rollback bar is already at the end".into());
        }
        self.state.doc.apply(Command::Rollback(None))?;
        Ok(self.summary())
    }

    /// Makes a drawing as the New drawing PropertyManager does with its defaults.
    fn make_drawing(&mut self, input: &Value) -> Result<Value, String> {
        let id = input["template"].as_str().unwrap_or_default().trim();
        let templates = &soliddraft_drawing::library::library().templates;
        if !templates.iter().any(|t| t.id == id) {
            let ids: Vec<&str> = templates.iter().map(|t| t.id.as_str()).collect();
            return Err(format!("no template `{id}` (there are: {})", ids.join(", ")));
        }
        if self.state.doc.display_body().is_none() {
            return Err("Make drawing needs a body; extrude something first".into());
        }
        let mut new = NewDrawing::new();
        new.set_template(id);
        let session = drawing::create_drawing(self.state, &new)?;
        self.state.assembly.active = false;
        self.state.drawing.session = Some(session);
        self.state.drawing.new = None;
        self.state.drawing.active = true;
        Ok(self.drawing_now())
    }

    /// The view `input` names, with the sheet's name for it.
    fn view(&mut self, input: &Value) -> Result<(ViewId, String), String> {
        let s = self.session()?;
        find_view(&s.doc.drawing, &input["view"])
    }

    /// Runs a drawing command as one undo step and answers with the drawing.
    fn command(&mut self, cmd: DrawingCommand) -> Result<Value, String> {
        drawing::run_command(self.state, cmd)?;
        Ok(self.drawing_now())
    }

    fn align(&mut self, input: &Value) -> Result<Value, String> {
        let (view, name) = self.view(input)?;
        let v = self.session()?.doc.drawing.view(view).ok_or(NO_DRAWING)?;
        if v.parent().is_none() {
            return Err(format!("`{name}` has no parent view to align with"));
        }
        if v.alignment.is_some() {
            return Err(format!("`{name}` is aligned with its parent already"));
        }
        self.command(DrawingCommand::Align { view })
    }

    fn break_alignment(&mut self, input: &Value) -> Result<Value, String> {
        let (view, name) = self.view(input)?;
        if self.session()?.doc.drawing.view(view).is_none_or(|v| v.alignment.is_none()) {
            return Err(format!("`{name}` is not aligned with a parent"));
        }
        self.command(DrawingCommand::BreakAlignment { view })
    }

    fn display(&mut self, view: ViewId, change: impl FnOnce(&mut ViewDisplay)) -> Result<Value, String> {
        let mut display = self.session()?.doc.drawing.view(view).ok_or(NO_DRAWING)?.display;
        change(&mut display);
        self.command(DrawingCommand::SetViewDisplay { view, display })
    }

    fn hidden_lines(&mut self, input: &Value) -> Result<Value, String> {
        let (view, _) = self.view(input)?;
        let hidden =
            if input["shown"].as_bool().unwrap_or(false) { HiddenLines::Visible } else { HiddenLines::Removed };
        self.display(view, |d| d.hidden = hidden)
    }

    fn tangent_edges(&mut self, input: &Value) -> Result<Value, String> {
        let (view, _) = self.view(input)?;
        let tangent = match input["tangent"].as_str().unwrap_or_default() {
            "removed" => TangentEdges::Removed,
            "phantom" => TangentEdges::Phantom,
            "visible" => TangentEdges::Visible,
            other => return Err(format!("tangent edges are removed, phantom or visible, not `{other}`")),
        };
        self.display(view, |d| d.tangent = tangent)
    }

    fn crop(&mut self, input: &Value) -> Result<Value, String> {
        let (view, name) = self.view(input)?;
        let cropped = self.session()?.doc.drawing.view(view).is_some_and(|v| v.crop.is_some());
        let crop = match &input["crop"] {
            Value::Null if !cropped => return Err(format!("`{name}` is not cropped")),
            Value::Null => None,
            corners => {
                let n: Vec<f64> = corners.as_array().into_iter().flatten().filter_map(Value::as_f64).collect();
                let [u1, v1, u2, v2] = n[..] else {
                    return Err("a crop is four numbers: two opposite corners [u1, v1, u2, v2]".into());
                };
                if n.iter().any(|x| !x.is_finite()) || (u1 - u2).abs() < 1.0 || (v1 - v2).abs() < 1.0 {
                    return Err("each side of the crop must be at least 1 mm".into());
                }
                Some(Boundary::rectangle(Point2::new(u1, v1), Point2::new(u2, v2)))
            }
        };
        self.command(DrawingCommand::SetViewCrop { view, crop })
    }

    /// Section: cuts a view along a horizontal or vertical line through a model point
    /// and places the section in line with it, as the canvas tool does after its two
    /// clicks. The section is drawn from the open part, which must be the drawing's.
    fn section(&mut self, input: &Value) -> Result<Value, String> {
        let (parent, name) = self.view(input)?;
        let n: Vec<f64> = input["through"].as_array().into_iter().flatten().filter_map(Value::as_f64).collect();
        let [x, y, z] = n[..] else {
            return Err("through is a model point: three numbers [x, y, z]".into());
        };
        if !n.iter().all(|v| v.is_finite()) {
            return Err("through must be three finite numbers".into());
        }
        let horizontal = match input["direction"].as_str().unwrap_or_default() {
            "horizontal" => true,
            "vertical" => false,
            other => return Err(format!("the direction is horizontal or vertical, not `{other}`")),
        };
        let flip = input["flip"].as_bool().unwrap_or(false);
        if !drawing::part_is_model(self.state) {
            return Err("The open part is not the one this drawing shows".into());
        }
        let st = &mut *self.state;
        let ctx = PartContext::new(&st.doc).ok_or("The part has no body; extrude something first")?;
        let session = st.drawing.session.as_mut().ok_or_else(|| NO_DRAWING.to_owned())?;
        if matches!(session.doc.drawing.view(parent).map(|v| &v.kind), Some(ViewKind::Detail { .. })) {
            return Err(format!("`{name}` is a detail view and cannot be sectioned; cut its parent"));
        }
        let sheet = project::sheet_of(&session.doc.drawing, parent).ok_or(NO_DRAWING)?;
        let part = ctx.part();
        let index = PartIndex::build(&part).map_err(|e| e.to_string())?;
        let drawing = &session.doc.drawing;
        let line = derived::cutting_line(drawing, &part, &index, parent, Vec3::new(x, y, z), horizontal)
            .map_err(|e| format!("Section: {e}"))?;
        let (view, vctx) = derived::new_section(drawing, &part, &index, sheet, parent, line, flip, None)
            .map_err(|e| format!("Section: {e}"))?;
        let id = view.id;
        session.doc.remember(id, &vctx.projected);
        drawing::run_command(self.state, DrawingCommand::PlaceView { sheet, view })?;
        let placed = self
            .session()?
            .doc
            .drawing
            .sheet_of(id)
            .and_then(|s| s.view(id).map(|v| view_label(s, v)))
            .unwrap_or_default();
        Ok(json!({ "placed": placed, "id": id.0, "drawing": drawing_json(self.state) }))
    }

    /// Rebuild: opens the drawing's part when it is not the open part, then brings the
    /// views to its revision.
    fn rebuild(&mut self) -> Result<Value, String> {
        self.session()?;
        if !drawing::part_is_model(self.state) {
            if drawing::model_to_open(self.state)?.is_none() {
                return Err(
                    "the drawing's part was not found; drawing_repair_reference points the drawing at it".into()
                );
            }
            self.open_model()?;
        }
        let said = drawing::update_views(self.state)?;
        Ok(self.drawing_said(said))
    }

    fn repoint(&mut self, input: &Value, replace: bool) -> Result<Value, String> {
        self.session()?;
        let path = PathBuf::from(input["path"].as_str().unwrap_or_default());
        if DocKind::of_path(&path) != Some(DocKind::Part) {
            return Err("the path must name a .dwprt part file".into());
        }
        let said = drawing::repoint_model(self.state, &path, replace)?;
        let part = self.open_model()?;
        Ok(json!({ "done": said, "part": part, "drawing": drawing_json(self.state) }))
    }

    fn auto_update(&mut self, input: &Value) -> Result<Value, String> {
        let on = input["on"].as_bool().unwrap_or(false);
        if drawing::set_automatic_update(self.session()?, on)? {
            return self.rebuild();
        }
        Ok(self.drawing_now())
    }

    fn centre_marks(&mut self, input: &Value) -> Result<Value, String> {
        let views = match input["view"] {
            Value::Null => self.session()?.sheet().views.iter().map(|v| v.id).collect(),
            _ => vec![self.view(input)?.0],
        };
        let added = drawing::add_centre_marks(self.session()?, &views);
        Ok(self.drawing_said(format!("{added} centre mark{} added", if added == 1 { "" } else { "s" })))
    }

    fn export_sheet(&mut self, format: SheetFormat, input: &Value) -> Result<Value, String> {
        let path = PathBuf::from(input["path"].as_str().unwrap_or_default());
        if path.extension().is_none_or(|e| !e.eq_ignore_ascii_case(format.extension())) {
            return Err(format!("the path must end in .{}", format.extension()));
        }
        let part = PartContext::new(&self.state.doc);
        let s = self.state.drawing.session.as_mut().ok_or_else(|| NO_DRAWING.to_owned())?;
        let written = drawing::write_sheet(s, part.as_ref(), format, &path)?;
        Ok(json!({
            "exported": path,
            "views": written.views,
            "redrawn_from_certified_mesh": written.redrawn,
            "revision": written.revision.short(),
            "accuracy": written.accuracy,
        }))
    }

    fn save_drawing(&mut self, input: &Value) -> Result<Value, String> {
        let path = PathBuf::from(input["path"].as_str().unwrap_or_default());
        if DocKind::of_path(&path) != Some(DocKind::Drawing) {
            return Err("the path must name a .dwdrw drawing file".into());
        }
        let backend = self.state.doc.kernel().backend();
        let kernel = format!("{} {}", backend.name, backend.version);
        let (path, warning) = drawing::save_session(self.session()?, &path, &kernel)?;
        Ok(json!({ "saved": path, "warning": warning }))
    }

    fn delete(&mut self, input: &Value) -> Result<Value, String> {
        match (input["view"].is_null(), input["annotation"].as_u64()) {
            (false, None) if input["annotation"].is_null() => {
                let (view, _) = self.view(input)?;
                self.command(DrawingCommand::DeleteView { view })
            }
            (true, Some(n)) => {
                let annotation = u32::try_from(n).map(AnnotationId).map_err(|_| format!("no annotation {n}"))?;
                if self.session()?.doc.drawing.annotation(annotation).is_none() {
                    return Err(format!("no annotation {n}; model_summary lists them under drawing"));
                }
                self.command(DrawingCommand::DeleteAnnotation { annotation })
            }
            _ => Err("name either a view or an annotation, and set the other to null".into()),
        }
    }
}

impl ToolHost for AppTools<'_> {
    fn tools(&self) -> Vec<ToolSpec> {
        tool_specs(Surface::Mcp)
    }

    fn call(&mut self, name: &str, input: Value, caller: &Caller) -> ToolOutput {
        let opening_part = name == "open"
            && DocKind::of_path(std::path::Path::new(input["path"].as_str().unwrap_or_default()))
                == Some(DocKind::Part);
        let out = match name {
            // A copy in another file is a save in that file, as in the app.
            "save_as" => self.part().call("save", input, caller),
            _ => match self.run(name, &input) {
                Some(Ok(v)) => ToolOutput::json(&v),
                Some(Err(e)) => ToolOutput::error(e),
                None => self.part().call(name, input, caller),
            },
        };
        if opening_part && !out.is_error {
            self.state.assembly.active = false;
            self.state.drawing.active = false;
        }
        // What the window's frame does after a change: the drawing follows its part.
        drawing::follow_part(self.state);
        out
    }
}

/// The view `wanted` means, on any sheet: its id, or a label no other view has,
/// ignoring case. Returns it with a name for messages. A label several views share is
/// refused with their ids, so a command never lands on one of them by chance.
fn find_view(drawing: &Drawing, wanted: &Value) -> Result<(ViewId, String), String> {
    let all = || drawing.sheets.iter().flat_map(|sheet| sheet.views.iter().map(move |v| (sheet, v)));
    let listed = || {
        let views: Vec<String> =
            all().map(|(sheet, v)| format!("{} (id {}, {})", view_label(sheet, v), v.id.0, sheet.name)).collect();
        if views.is_empty() { "no views".to_owned() } else { views.join(", ") }
    };
    if let Some(n) = wanted.as_u64() {
        return all()
            .find(|(_, v)| u64::from(v.id.0) == n)
            .map(|(sheet, v)| (v.id, view_label(sheet, v)))
            .ok_or_else(|| format!("no view has id {n} (the drawing has: {})", listed()));
    }
    let name = wanted.as_str().unwrap_or_default().trim();
    let found: Vec<_> = all().filter(|(sheet, v)| view_label(sheet, v).eq_ignore_ascii_case(name)).collect();
    match found[..] {
        [(sheet, v)] => Ok((v.id, view_label(sheet, v))),
        [] => Err(format!("no view named `{name}` (the drawing has: {})", listed())),
        _ => {
            let ids: Vec<String> = found.iter().map(|(sheet, v)| format!("{} ({})", v.id.0, sheet.name)).collect();
            Err(format!("`{name}` names {} views, ids {}; give the id of the one meant", found.len(), ids.join(", ")))
        }
    }
}

/// The serde tag of a view kind or annotation body: "section", "dimension".
fn kind_name<T: serde::Serialize>(kind: &T) -> Value {
    serde_json::to_value(kind).ok().and_then(|v| v.get("type").cloned()).unwrap_or(Value::Null)
}

/// The open drawing as agents read it: its sheets with their views and annotations by
/// name, and how it stands against the part. Each view says whether its line work is at
/// the pinned revision and whether it is a screen preview; each annotation says whether
/// its references resolve and what it measures now, and the lists of changed and
/// dangling annotations repeat what the Needs attention panel shows.
fn drawing_json(state: &AppState) -> Option<Value> {
    let s = state.drawing.session.as_ref()?;
    let d = &s.doc.drawing;
    let style = s.doc.style();
    let pinned = d.model.pinned.as_str();
    let (mut changed, mut dangling) = (Vec::new(), Vec::new());
    let sheets: Vec<Value> = d
        .sheets
        .iter()
        .enumerate()
        .map(|(index, sheet)| {
            let name_of = |id: ViewId| sheet.view(id).map(|v| view_label(sheet, v));
            let views: Vec<Value> = sheet
                .views
                .iter()
                .map(|v| {
                    json!({
                        "id": v.id.0,
                        "name": view_label(sheet, v),
                        "kind": kind_name(&v.kind),
                        "parent": v.parent().and_then(name_of),
                        "parent_id": v.parent().map(|p| p.0),
                        "aligned": v.alignment.is_some(),
                        "hidden_lines": v.display.hidden == HiddenLines::Visible,
                        "tangent_edges": match v.display.tangent {
                            TangentEdges::Removed => "removed",
                            TangentEdges::Phantom => "phantom",
                            TangentEdges::Visible => "visible",
                        },
                        "cropped": v.crop.is_some(),
                        "excluded_from_update": v.excluded_from_update,
                        "up_to_date": v.excluded_from_update || v.geometry.source_digest == pinned,
                        "preview": v.geometry.unproved,
                        "curves": v.geometry.curves.len(),
                        "accuracy": v.geometry.accuracy(),
                    })
                })
                .collect();
            let annotations: Vec<Value> = sheet
                .annotations
                .iter()
                .map(|a| {
                    let view = if a.view == SHEET { Some("sheet".to_owned()) } else { name_of(a.view) };
                    let view_id = (a.view != SHEET).then_some(a.view.0);
                    let name = annotation_name(a, style);
                    let value = match &a.body {
                        AnnotationKind::Dimension(dim) => Some(value_text(dim)),
                        AnnotationKind::DimensionSet(set) => Some(set.values_text()),
                        AnnotationKind::HoleCallout { text } => Some(text.clone()),
                        _ => None,
                    };
                    let references: Vec<&str> = a.anchors.iter().map(|anchor| anchor.entity.role.as_str()).collect();
                    let (state, previous, cause) = match &a.state {
                        AnnotationState::Ok => ("ok", None, None),
                        AnnotationState::Changed { previous } => {
                            changed.push(json!({ "id": a.id.0, "name": name, "previous": previous, "now": value }));
                            ("changed", Some(previous.clone()), None)
                        }
                        AnnotationState::Dangling { cause, .. } => {
                            dangling.push(json!({ "id": a.id.0, "name": name, "cause": cause }));
                            ("dangling", None, Some(cause.clone()))
                        }
                    };
                    json!({
                        "id": a.id.0,
                        "name": name,
                        "kind": kind_name(&a.body),
                        "view": view,
                        "view_id": view_id,
                        "value": value,
                        "references": references,
                        "state": state,
                        "previous": previous,
                        "cause": cause,
                    })
                })
                .collect();
            let title_block: serde_json::Map<String, Value> =
                soliddraft_drawing::library::template(sheet.template.as_str())
                    .title_block
                    .fields
                    .iter()
                    .map(|field| (field.key.clone(), field_value(d, index, field).into()))
                    .collect();
            json!({
                "name": sheet.name, "views": views, "annotations": annotations,
                "title_block": title_block, "title_bindings": sheet.title_bindings,
            })
        })
        .collect();
    let part = match s.link_state() {
        LinkState::Current => "current",
        LinkState::Newer { .. } => "newer: drawing_rebuild brings the views to it",
        LinkState::Missing => "missing: drawing_repair_reference points the drawing at it",
        LinkState::Unverified => "not a SolidDraft document",
        LinkState::Unknown => "not resolved",
    };
    Some(json!({
        "title": d.meta.title,
        "path": s.doc.path,
        "active": state.drawing.is_active(),
        "sheet": s.sheet().name,
        "sheets": sheets,
        "up_to_date": s.is_up_to_date(),
        "pinned_revision": d.model.pinned.short(),
        "source": {
            "revision": d.model.pinned.as_str(),
            "configuration": d.model.configuration,
            "properties": d.source_properties,
        },
        "part_revision": s.doc.seen_digest.short(),
        "views_match_pin": d.views_match_pin(),
        "changed": changed,
        "dangling": dangling,
        "part": part,
        "automatic_update": d.settings.automatic_update,
        "undo": s.doc.undo_label(),
    }))
}
