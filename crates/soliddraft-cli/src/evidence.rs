//! MCP evidence is captured after execution, including proposals rejected on scratch parts.

use std::{path::PathBuf, sync::Arc};

use serde_json::{Value, json};
use soliddraft_app::command_tools::CommandTools;
use soliddraft_assistant::{Caller, ToolHost, ToolOutput, ToolSpec};
use soliddraft_kernel_catallax::CatallaxKernel;

pub(crate) struct Evidence<'a> {
    host: CommandTools<'a>,
    kernel: Option<Arc<CatallaxKernel>>,
    directory: Option<PathBuf>,
    sequence: usize,
    max_work: u64,
    failed: bool,
}

impl<'a> Evidence<'a> {
    pub(crate) fn new(
        host: CommandTools<'a>,
        kernel: Option<Arc<CatallaxKernel>>,
        directory: Option<PathBuf>,
        max_work: u64,
    ) -> Result<Self, String> {
        if let Some(dir) = &directory {
            std::fs::create_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        Ok(Self { host, kernel, directory, sequence: 0, max_work, failed: false })
    }

    fn snapshot(&mut self, name: &str, input: &Value, output: &ToolOutput) -> Result<(), Box<dyn std::error::Error>> {
        let (Some(root), Some(kernel)) = (&self.directory, &self.kernel) else { return Ok(()) };
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
            }),
        )?;
        Ok(())
    }
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
