//! MCP evidence is captured after execution, including proposals rejected on scratch parts.

use std::collections::BTreeSet;
use std::{path::PathBuf, sync::Arc};

use serde_json::{Value, json};
use soliddraft_app::command_tools::CommandTools;
use soliddraft_assistant::{Caller, ToolHost, ToolOutput, ToolSpec};
use soliddraft_kernel_api::counting::{Counting, Work};
use soliddraft_kernel_catallax::CatallaxKernel;
use soliddraft_model::{Document, feature_of_tag};

pub(crate) struct Evidence<'a> {
    host: CommandTools<'a>,
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
        Ok(Self { host, kernel, work, directory, sequence: 0, max_work, failed: false, renderer: None })
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
                "backend":doc.kernel().backend(),"summary":summary,
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
        let doc = &*self.host.doc;
        let body = doc.display_body().ok_or("part has no displayed solid")?;
        self.renderer
            .as_mut()
            .ok_or("call scenario_prepare before capture")?
            .capture(doc, &dir.join("viewport.png"))?;
        let spec =
            soliddraft_kernel_api::drawing::ViewSpec::standard(soliddraft_kernel_api::drawing::StandardView::Isometric);
        let curves = doc.kernel().view_curves(body, &spec)?;
        let frame = spec.frame().ok_or("invalid scenario view frame")?;
        std::fs::write(dir.join("view.svg"), crate::view::svg(&curves, &[], &frame))?;
        let diagnostics = self.kernel.as_ref().ok_or("capture kernel missing")?.take_diagnostics()?;
        write_json(dir.join("display.kernel.json"), &diagnostics)?;
        let result = json!({"backend":doc.kernel().backend(),"method":"mesh","live_document":true});
        write_json(dir.join("display.json"), &result)?;
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

fn write_json(path: PathBuf, value: &Value) -> Result<(), std::io::Error> {
    std::fs::write(path, serde_json::to_vec_pretty(value)?)
}

impl ToolHost for Evidence<'_> {
    fn tools(&self) -> Vec<ToolSpec> {
        let mut tools = self.host.tools();
        if self.directory.is_some() {
            for (name, description) in [
                ("scenario_prepare", "Prepare the retained offscreen renderer before scenario timing."),
                ("scenario_display", "Capture the live document's viewport and SVG without reopening a file."),
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
        if self.directory.is_some() && matches!(name, "scenario_prepare" | "scenario_display") {
            let capture = self.scenario_capture(name == "scenario_prepare");
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
