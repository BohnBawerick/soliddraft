//! MCP evidence is captured after execution, including proposals rejected on scratch parts.

use std::collections::BTreeSet;
use std::{path::PathBuf, sync::Arc};

use serde_json::{Value, json};
use soliddraft_app::command_tools::CommandTools;
use soliddraft_assistant::{Caller, ToolHost, ToolOutput, ToolSpec};
use soliddraft_kernel_api::counting::{Counting, Work};
use soliddraft_kernel_catallax::CatallaxKernel;
use soliddraft_model::{Document, FaceRef, FeatureKind, FeatureStatus, ReferenceSlot, SketchPlane, feature_of_tag};

pub(crate) struct Evidence<'a> {
    host: CommandTools<'a>,
    kernel: Option<Arc<CatallaxKernel>>,
    /// The kernel calls each command makes, counted outside the application.
    work: Option<Arc<Counting>>,
    directory: Option<PathBuf>,
    sequence: usize,
    max_work: u64,
    failed: bool,
}

impl<'a> Evidence<'a> {
    pub(crate) fn new(
        host: CommandTools<'a>,
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
        Ok(Self { host, kernel, work, directory, sequence: 0, max_work, failed: false })
    }

    fn snapshot(&mut self, name: &str, input: &Value, output: &ToolOutput) -> Result<(), Box<dyn std::error::Error>> {
        let (Some(root), Some(kernel)) = (&self.directory, &self.kernel) else { return Ok(()) };
        let work = self.work.as_ref().map(|w| w.take()).unwrap_or_default();
        self.sequence += 1;
        let dir = root.join(format!("{:04}", self.sequence));
        std::fs::create_dir(&dir)?;
        let doc = &*self.host.doc;
        std::fs::write(dir.join("part.json"), doc.model().part_json())?;
        doc.model().save(dir.join("part.dwprt"))?;
        if let Some(body) = doc.display_body() {
            write_json(dir.join("recipe.json"), &kernel.body_recipe(body)?)?;
        }
        let summary = self.host.summary();
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
                "backend":doc.kernel().backend(),"summary":summary,"references":references(doc.model()),
                "rebuild":rebuild_report(doc.model(), &work),
            }),
        )?;
        // Saving the bundle's part file is not the next command's work.
        if let Some(w) = &self.work {
            w.take();
        }
        Ok(())
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

/// Every persistent reference in the part, with the origin it names and whether the last
/// rebuild resolved it, for scenario checks. It reads the rebuild's statuses and resolves
/// nothing itself. `resolved` is null for a feature that was not built.
fn references(doc: &soliddraft_model::Document) -> Value {
    let origin = |r: &FaceRef| format!("{} {} #{}", r.feature, r.generator, r.ordinal + 1);
    let mut out = Vec::new();
    for f in doc.features() {
        let refs: Vec<(ReferenceSlot, String, Vec<&FaceRef>)> = match &f.kind {
            FeatureKind::Sketch(s) => match &s.plane {
                SketchPlane::Face { face } => vec![(ReferenceSlot::SketchPlane, origin(face), vec![face])],
                SketchPlane::Datum { .. } => Vec::new(),
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
            FeatureKind::Extrude(_) => Vec::new(),
        };
        let status = doc.status(f.id);
        for (slot, text, faces) in refs {
            let repair = match status {
                Some(FeatureStatus::RepairNeeded(items)) => items.iter().find(|r| r.slot == slot),
                _ => None,
            };
            let built = matches!(status, Some(FeatureStatus::Ok | FeatureStatus::RepairNeeded(_) | FeatureStatus::Failed(_)));
            out.push(json!({
                "feature_id": f.id.to_string(),
                "feature": f.name,
                "slot": slot.to_string(),
                "origin": text,
                "of": faces.iter().map(|r| doc.feature(r.feature).map(|g| g.name.clone())).collect::<Vec<_>>(),
                "resolved": built.then_some(repair.is_none()),
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
        self.host.tools()
    }

    fn call(&mut self, name: &str, input: Value, caller: &Caller) -> ToolOutput {
        if self.failed {
            return ToolOutput::error("bundle write failed; start a new run");
        }
        let result = self.host.call(name, input.clone(), caller);
        if let Err(error) = self.snapshot(name, &input, &result) {
            self.failed = true;
            return ToolOutput::error(format!("bundle write failed after {name}: {error}"));
        }
        result
    }
}
