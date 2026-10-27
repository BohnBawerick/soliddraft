//! `soliddraft export`: a part or a recipe's result as STEP, STL, 3MF or OBJ, without a window.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use serde_json::json;
use soliddraft_document::DocKind;
use soliddraft_exchange::{Export, Format, Report};
use soliddraft_kernel_api::Kernel;
use soliddraft_kernel_catallax::CatallaxKernel;
use soliddraft_model::Document;
use soliddraft_tessellation::ExportTolerance;

pub(crate) const USAGE: &str = "\
usage: soliddraft export <part.dwprt | recipe.json> --out <file>... [--json]

Exports a part file, rebuilt on the Catallax kernel, or a recipe's result. The
extension of each --out file picks its format:

  .step, .stp  STEP AP242, exact geometry, with Catallax's proven bound on the
               distance between the written and the exact geometry
  .stl         binary STL from Catallax's mesh
  .3mf         3MF from Catallax's mesh, which must be closed
  .obj         Wavefront OBJ from Catallax's mesh, with face groups and normals

Meshes use the part's export tolerance (`soliddraft quality --export`); a recipe
uses the default, 0.01 mm. Each report gives what was written and how far it may
lie from the exact part. A part with a feature that did not build is refused.
STEP output over its requested tolerance is refused before any file is written.

  --out   a file to write; repeat it for several formats
  --json  print one JSON export report per file
";

/// What `export` was asked for.
#[derive(Debug)]
struct Args {
    input: PathBuf,
    outputs: Vec<(PathBuf, Format)>,
    json: bool,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let (mut input, mut outputs, mut json) = (None, Vec::new(), false);
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--out" => {
                let path = PathBuf::from(it.next().ok_or("--out needs a file")?);
                let format = Format::of_path(&path)
                    .ok_or_else(|| format!("{}: give a .step, .stp, .stl, .3mf or .obj file", path.display()))?;
                outputs.push((path, format));
            }
            flag if flag.starts_with('-') => return Err(format!("unknown option {flag}")),
            path if input.is_none() => input = Some(PathBuf::from(path)),
            extra => return Err(format!("one part or recipe at a time, not also {extra}")),
        }
    }
    let input = input.ok_or("no part or recipe given")?;
    if outputs.is_empty() {
        return Err("no --out file given".into());
    }
    Ok(Args { input, outputs, json })
}

pub(crate) fn run(args: &[String]) -> ExitCode {
    let a = match parse(args) {
        Ok(a) => a,
        Err(e) => {
            eprint!("{e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match export(&a) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn export(a: &Args) -> Result<(), String> {
    let kernel: Arc<dyn Kernel> = Arc::new(CatallaxKernel::new());
    let at = |e: String| format!("{}: {e}", a.input.display());
    let exports = match DocKind::of_path(&a.input) {
        Some(DocKind::Part) => {
            let doc = Document::open(&a.input, kernel.clone()).map_err(|e| at(e.to_string()))?;
            a.outputs.iter().map(|(_, f)| doc.export(*f)).collect::<Result<Vec<_>, _>>().map_err(at)?
        }
        Some(kind) => return Err(at(format!("export takes a part or a recipe, not a {kind}"))),
        None => {
            let text = std::fs::read_to_string(&a.input).map_err(|e| at(e.to_string()))?;
            let recipe = soliddraft_recipe::parse(&text).map_err(|e| at(e.to_string()))?;
            let (_, body) = soliddraft_recipe::build(kernel.as_ref(), &recipe).map_err(|e| at(e.to_string()))?;
            a.outputs
                .iter()
                .map(|(_, f)| {
                    soliddraft_exchange::export(kernel.as_ref(), body, &recipe.name, *f, &ExportTolerance::default())
                })
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| at(e.to_string()))?
        }
    };
    publish(a, &exports)
}

fn publish(a: &Args, exports: &[Export]) -> Result<(), String> {
    // A successful kernel call can still report a bound over the requested tolerance.
    // Validate the whole batch before creating or replacing any destination.
    for ((path, _), out) in a.outputs.iter().zip(exports) {
        if let Report::Step(report) = &out.report
            && !report.within_tolerance
        {
            return Err(format!(
                "{}: STEP export refused: proven deviation {} mm exceeds the requested tolerance",
                path.display(),
                report.max_deviation
            ));
        }
    }
    for ((path, format), out) in a.outputs.iter().zip(exports) {
        write(path, out)?;
        report(path, *format, out, a.json);
    }
    Ok(())
}

fn write(path: &Path, out: &Export) -> Result<(), String> {
    soliddraft_document::save_atomic(path, &out.bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

fn report(path: &Path, format: Format, out: &Export, json: bool) {
    if json {
        println!("{}", json!({"file": path, "format": format, "bytes": out.bytes.len(), "report": out.report}));
    } else {
        println!("{}: {}, {}", path.display(), format.name(), out.report.summary());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn outputs_pick_their_formats() {
        let a = parse(&args("bracket.json --out a.step --out b.STL --out c.3mf --out d.OBJ --json")).unwrap();
        assert_eq!(a.outputs.iter().map(|(_, f)| *f).collect::<Vec<_>>(), Format::ALL);
        assert!(a.json);
        assert!(parse(&args("bracket.json --out a.iges")).is_err());
        assert!(parse(&args("bracket.json")).unwrap_err().contains("--out"));
        assert!(parse(&args("--out a.step")).unwrap_err().contains("no part"));
        assert!(parse(&args("a.json b.json --out a.step")).unwrap_err().contains("one part"));
    }

    #[test]
    fn over_tolerance_step_leaves_every_destination_untouched() {
        let dir = std::env::temp_dir().join(format!("soliddraft-export-refusal-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = dir.join("first.step");
        let second = dir.join("second.step");
        let a = Args {
            input: "unused.json".into(),
            outputs: vec![(first.clone(), Format::Step), (second.clone(), Format::Step)],
            json: true,
        };
        let step = |within_tolerance| Export {
            bytes: b"must not be published".to_vec(),
            report: Report::Step(soliddraft_kernel_api::StepReport {
                max_deviation: if within_tolerance { "0" } else { "0.001" }.into(),
                max_deviation_mm: if within_tolerance { 0.0 } else { 0.001 },
                within_tolerance,
                solids: 1,
                faces: 6,
                edges: 12,
                vertices: 8,
                fitted_curves: 0,
                added_vertices: 0,
                added_edges: 0,
                warnings: Vec::new(),
            }),
        };
        let exports = [step(true), step(false)];
        let error = publish(&a, &exports).unwrap_err();
        assert!(error.contains("second.step"));
        assert!(error.contains("exceeds the requested tolerance"));
        assert!(!first.exists());
        assert!(!second.exists());

        std::fs::write(&first, b"existing first").unwrap();
        std::fs::write(&second, b"existing second").unwrap();
        assert!(publish(&a, &exports).is_err());
        assert_eq!(std::fs::read(&first).unwrap(), b"existing first");
        assert_eq!(std::fs::read(&second).unwrap(), b"existing second");

        // A fully accepted batch still publishes the exact bytes it was given.
        let accepted = [step(true), step(true)];
        publish(&a, &accepted).unwrap();
        assert_eq!(std::fs::read(&first).unwrap(), accepted[0].bytes);
        assert_eq!(std::fs::read(&second).unwrap(), accepted[1].bytes);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
