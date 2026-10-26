//! MCP evidence is captured after execution, including proposals rejected on scratch parts.

use std::collections::BTreeSet;
use std::{path::PathBuf, sync::Arc};

use serde_json::{Value, json};
use soliddraft_app::app_tools::AppTools;
use soliddraft_assistant::{Caller, ToolHost, ToolOutput, ToolSpec};
use soliddraft_kernel_api::counting::{Counting, Work};
use soliddraft_kernel_catallax::CatallaxKernel;
use soliddraft_model::{
    Document, EndCondition, FaceRef, FeatureKind, FeatureStatus, PlaneRef, ReferenceSlot, SketchPlane, feature_of_tag,
};

pub(crate) struct Evidence<'a> {
    host: AppTools<'a>,
    kernel: Option<Arc<CatallaxKernel>>,
    /// The kernel calls each command makes, counted outside the application.
    work: Option<Arc<Counting>>,
    directory: Option<PathBuf>,
    sequence: usize,
    max_work: u64,
    failed: bool,
    renderer: Option<crate::screenshot::Renderer>,
}

impl<'a> Evidence<'a> {
    pub(crate) fn new(
        host: AppTools<'a>,
        kernel: Option<Arc<CatallaxKernel>>,
        work: Option<Arc<Counting>>,
        directory: Option<PathBuf>,
        max_work: u64,
    ) -> Result<Self, String> {
        if let Some(dir) = &directory {
            std::fs::create_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        // Opening a part is not a command's work.
        if let Some(w) = &work {
            w.take();
        }
        Ok(Self { host, kernel, work, directory, sequence: 0, max_work, failed: false, renderer: None })
    }

    fn snapshot(&mut self, name: &str, input: &Value, output: &ToolOutput) -> Result<(), Box<dyn std::error::Error>> {
        let (Some(root), Some(kernel)) = (&self.directory, &self.kernel) else { return Ok(()) };
        let work = self.work.as_ref().map(|w| w.take()).unwrap_or_default();
        self.sequence += 1;
        let dir = root.join(format!("{:04}", self.sequence));
        std::fs::create_dir(&dir)?;
        let summary = self.host.summary();
        let doc = &self.host.state.doc;
        std::fs::write(dir.join("part.json"), doc.model().part_json())?;
        let (_, bytes) = doc.model().snapshot().encode()?;
        soliddraft_document::save_atomic(&dir.join("part.dwprt"), &bytes)?;
        if let Some(body) = doc.display_body() {
            write_json(dir.join("recipe.json"), &kernel.body_recipe(body)?)?;
        }
        let references = references(doc.model());
        let mut diagnostics = kernel.take_diagnostics()?;
        if let Some(questions) = diagnostics["questions"].as_array_mut() {
            for (i, question) in questions.iter_mut().enumerate() {
                question["max_work"] = json!(self.max_work);
                write_json(dir.join(format!("question-{i:04}.json")), question)?;
            }
        }
        write_json(dir.join("kernel.json"), &diagnostics)?;
        write_json(
            dir.join("command.json"),
            &json!({
                "tool":name,"arguments":input,"is_error":output.is_error,"output":output.content,
                "backend":doc.kernel().backend(),"summary":summary,"references":references,
                "rebuild":rebuild_report(doc.model(), &work),
            }),
        )?;
        // Saving the bundle's part file is not the next command's work.
        if let Some(w) = &self.work {
            w.take();
        }
        Ok(())
    }

    fn scenario_capture(&mut self, prepare: bool) -> Result<Value, Box<dyn std::error::Error>> {
        let root = self.directory.as_ref().ok_or("scenario capture requires --bundle")?;
        if prepare {
            self.renderer = Some(pollster::block_on(crate::screenshot::Renderer::new())?);
            return Ok(json!({"prepared":true}));
        }
        let dir = root.join(format!("{:04}", self.sequence));
        let doc = &self.host.state.doc;
        self.renderer
            .as_mut()
            .ok_or("call scenario_prepare before capture")?
            .capture(doc, &dir.join("viewport.png"))?;
        // The viewport and drawing views show preview meshes, which claim no bounds.
        let result = json!({"backend":doc.kernel().backend(),"method":"viewport","mesh":"unproved preview","live_document":true});
        write_json(dir.join("display.json"), &result)?;
        Ok(result)
    }

    fn scenario_view(&self) -> Result<Value, Box<dyn std::error::Error>> {
        let root = self.directory.as_ref().ok_or("scenario view requires --bundle")?;
        let dir = root.join(format!("{:04}", self.sequence));
        let doc = &self.host.state.doc;
        let body = doc.display_body().ok_or("part has no displayed solid")?;
        // The live hidden-line view stands for a drawing view on screen, so it reads the preview.
        let spec = soliddraft_kernel_api::drawing::ViewSpec {
            preview: true,
            ..soliddraft_kernel_api::drawing::ViewSpec::standard(
                soliddraft_kernel_api::drawing::StandardView::Isometric,
            )
        };
        let curves = doc.kernel().view_curves(body, &spec)?;
        let frame = spec.frame().ok_or("invalid scenario view frame")?;
        std::fs::write(dir.join("view.svg"), crate::view::svg(&curves, &[], &frame))?;
        let result =
            json!({"backend":doc.kernel().backend(),"method":"mesh","mesh":"unproved preview","live_document":true});
        write_json(dir.join("view.json"), &result)?;
        Ok(result)
    }

    fn scenario_measure(&mut self) -> Result<Value, Box<dyn std::error::Error>> {
        let root = self.directory.as_ref().ok_or("scenario measurement requires --bundle")?;
        let dir = root.join(format!("{:04}", self.sequence));
        let doc = &self.host.state.doc;
        let mass = doc.display_body().map(|body| doc.kernel().mass_properties(body)).transpose()?;
        let result = json!({
            "backend":doc.kernel().backend(), "live_document":true,
            "volume_mm3":mass.map(|m| m.volume),
            "area_mm2":mass.map(|m| m.area),
            "centre_of_mass":mass.map(|m| [m.centroid.x,m.centroid.y,m.centroid.z]),
        });
        write_json(dir.join("measurement.json"), &result)?;
        Ok(result)
    }
}

/// What a command rebuilt: the features whose kernel operations ran, in tree order
/// (with the ids of features of scratch parts, such as a rejected proposal's, after
/// them), how many operations ran and how many meshes were made.
fn rebuild_report(doc: &Document, work: &[Work]) -> Value {
    let mut ids = BTreeSet::new();
    let mut operations = 0;
    let mut meshes = 0;
    for w in work {
        match w {
            Work::Operation(tag) => {
                operations += 1;
                ids.extend(feature_of_tag(tag));
            }
            Work::Mesh(_) => meshes += 1,
        }
    }
    let mut features: Vec<String> =
        doc.features().iter().filter(|f| ids.remove(&f.id)).map(|f| f.name.clone()).collect();
    features.extend(ids.into_iter().map(|id| id.to_string()));
    json!({"features_recomputed":features,"operations":operations,"meshes_rebuilt":meshes})
}

/// Reference evidence is collected only for an explicitly requested command bundle,
/// after the edit. Survival assertions live in the scenario runner, not in rebuild.
fn references(doc: &soliddraft_model::Document) -> Value {
    fn origin(r: &FaceRef) -> String {
        format!("{} {} #{}", r.feature, r.generator, r.ordinal + 1)
    }
    fn plane(p: &PlaneRef) -> Option<(ReferenceSlot, String, Vec<&FaceRef>)> {
        match p {
            PlaneRef::Face { face } => Some((ReferenceSlot::Plane, origin(face), vec![face])),
            PlaneRef::Datum { .. } => None,
        }
    }
    let mut out = Vec::new();
    for f in doc.features() {
        let refs: Vec<(ReferenceSlot, String, Vec<&FaceRef>)> = match &f.kind {
            FeatureKind::Sketch(s) => match &s.plane {
                SketchPlane::Face { face } => vec![(ReferenceSlot::SketchPlane, origin(face), vec![face])],
                SketchPlane::Datum { datum, .. } => {
                    vec![(ReferenceSlot::SketchPlane, format!("datum:{datum:?}"), Vec::new())]
                }
            },
            FeatureKind::Fillet(b) | FeatureKind::Chamfer(b) => b
                .edges
                .iter()
                .enumerate()
                .map(|(i, e)| {
                    let text = format!("{} | {}", origin(&e.faces[0]), origin(&e.faces[1]));
                    (ReferenceSlot::Edge { index: i as u32 }, text, e.faces.iter().collect())
                })
                .collect(),
            FeatureKind::Extrude(e) => match &e.end {
                EndCondition::UpToFace { face } => vec![(ReferenceSlot::Limit, origin(face), vec![face])],
                _ => Vec::new(),
            },
            FeatureKind::Hole(h) => vec![(ReferenceSlot::Plane, origin(&h.face), vec![&h.face])],
            FeatureKind::Mirror(m) => plane(&m.plane).into_iter().collect(),
            FeatureKind::Draft(d) => d
                .faces
                .iter()
                .enumerate()
                .map(|(i, face)| (ReferenceSlot::Face { index: i as u32 }, origin(face), vec![face]))
                .chain(plane(&d.neutral))
                .collect(),
            FeatureKind::Pattern(_) => Vec::new(),
        };
        if refs.is_empty() {
            continue;
        }
        let status = doc.status(f.id);
        let topology = doc.body_before(f.id).and_then(|b| doc.kernel().topology(b).ok());
        for (slot, text, faces) in refs {
            let repair = match status {
                Some(FeatureStatus::RepairNeeded(items)) => items.iter().find(|r| r.slot == slot),
                _ => None,
            };
            let built =
                matches!(status, Some(FeatureStatus::Ok | FeatureStatus::RepairNeeded(_) | FeatureStatus::Failed(_)));
            let origins: Vec<_> = faces
                .iter()
                .map(|r| {
                    doc.generated_faces(r.feature)
                        .iter()
                        .find(|n| n.generator == r.generator && n.ordinal == r.ordinal)
                        .map(|n| &n.origin)
                })
                .collect();
            let carriers: Vec<Vec<_>> = origins
                .iter()
                .map(|origin| {
                    topology
                        .as_ref()
                        .map(|t| t.faces.iter().filter(|f| origin.is_some_and(|o| f.origins.contains(o))).collect())
                        .unwrap_or_default()
                })
                .collect();
            let matches_origin = if !matches!(status, Some(FeatureStatus::Ok)) {
                None
            } else {
                Some(match &f.kind {
                    FeatureKind::Sketch(s) => doc.built_sketch(f.id).is_some_and(|built| match &s.plane {
                        SketchPlane::Datum { datum, offset } => {
                            let p = datum.plane();
                            built.plane == p.with_origin(p.origin + p.normal * *offset)
                        }
                        SketchPlane::Face { .. } => carriers.iter().all(|set| {
                            !set.is_empty()
                                && set.iter().all(|f| {
                                    f.normal == Some(built.plane.normal)
                                        && (f.centroid - built.plane.origin).dot(built.plane.normal).abs() <= 1e-6
                                })
                        }),
                    }),
                    _ => carriers.iter().all(|set| !set.is_empty()),
                })
            };
            out.push(json!({
                "feature_id": f.id.to_string(),
                "feature": f.name,
                "slot": slot.to_string(),
                "origin": text,
                "of": faces.iter().map(|r| doc.feature(r.feature).map(|g| g.name.clone())).collect::<Vec<_>>(),
                "resolved": built.then_some(repair.is_none()),
                "kernel_origins": origins,
                "carrier_counts": carriers.iter().map(Vec::len).collect::<Vec<_>>(),
                "carrier_origin_counts": carriers.iter().map(|set| set.iter().map(|f| f.origins.len()).collect::<Vec<_>>()).collect::<Vec<_>>(),
                "matches_origin": matches_origin,
                "repair": repair.map(|r| r.message.clone()),
            }));
        }
    }
    Value::Array(out)
}

fn write_json(path: PathBuf, value: &Value) -> Result<(), std::io::Error> {
    std::fs::write(path, serde_json::to_vec_pretty(value)?)
}

impl ToolHost for Evidence<'_> {
    fn tools(&self) -> Vec<ToolSpec> {
        let mut tools = self.host.tools();
        if self.directory.is_some() {
            for (name, description) in [
                ("scenario_prepare", "Prepare the retained offscreen renderer before scenario timing."),
                ("scenario_display", "Capture the live document's viewport without reopening a file."),
                ("scenario_view", "Compute the live body's hidden-line SVG, separately from the click's viewport."),
                ("scenario_measure", "Explicitly measure the live body after the timed edit and display."),
            ] {
                tools.push(ToolSpec {
                    name: name.into(),
                    description: description.into(),
                    input_schema: json!({"type":"object","properties":{},"required":[],"additionalProperties":false}),
                });
            }
        }
        tools
    }

    fn call(&mut self, name: &str, input: Value, caller: &Caller) -> ToolOutput {
        if self.failed {
            return ToolOutput::error("bundle write failed; start a new run");
        }
        if self.directory.is_some()
            && matches!(name, "scenario_prepare" | "scenario_display" | "scenario_view" | "scenario_measure")
        {
            let capture = (|| {
                let result = match name {
                    "scenario_measure" => self.scenario_measure(),
                    "scenario_view" => self.scenario_view(),
                    _ => self.scenario_capture(name == "scenario_prepare"),
                };
                if name != "scenario_prepare" {
                    let root = self.directory.as_ref().ok_or("scenario capture requires --bundle")?;
                    let dir = root.join(format!("{:04}", self.sequence));
                    let stem = match name {
                        "scenario_measure" => "measurement",
                        "scenario_view" => "view",
                        _ => "display",
                    };
                    let diagnostics = self.kernel.as_ref().ok_or("capture kernel missing")?.take_diagnostics()?;
                    write_json(dir.join(format!("{stem}.kernel.json")), &diagnostics)?;
                }
                result
            })();
            // Evidence rendering is not the next command's rebuild work.
            if let Some(work) = &self.work {
                work.take();
            }
            return match capture {
                Ok(result) => ToolOutput::json(&result),
                Err(error) => ToolOutput::error(format!("scenario capture failed: {error}")),
            };
        }
        let result = self.host.call(name, input.clone(), caller);
        if let Err(error) = self.snapshot(name, &input, &result) {
            self.failed = true;
            return ToolOutput::error(format!("bundle write failed after {name}: {error}"));
        }
        result
    }
}
