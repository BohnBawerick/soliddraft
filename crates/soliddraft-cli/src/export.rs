//! `soliddraft export`: a part or a recipe's result as STEP, STL, 3MF or OBJ, without a window.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use serde_json::json;
use soliddraft_document::DocKind;
use soliddraft_exchange::{Export, Format, Report};
use soliddraft_kernel_api::Kernel;
use soliddraft_kernel_catallax::{CatallaxKernel, RequestWork};
use soliddraft_model::Document;
use soliddraft_tessellation::ExportTolerance;

pub(crate) const USAGE: &str = "\
usage: soliddraft export <part.dwprt | recipe.json> --out <file>... [--stand-in <file.step>] [--max-work <units>] [--json]

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

A part whose last feature is a lattice or gyroid exports its certified, labelled
STL. Its mass stand-in is written only when asked for with --stand-in: the
field's source envelope as a massless surface and unit-density proxy solids
whose proven moments lie inside the field's, never the field's boundary.

  --out       a file to write; repeat it for several formats
  --stand-in  a .step file for a field part's mass stand-in
  --max-work  run the whole request, opening and rebuilding the part included,
              on one deterministic work allowance; refuse and write nothing
              when it runs out. --json reports the work used
  --json      print one JSON export report per file
";

/// What `export` was asked for.
#[derive(Debug)]
struct Args {
    input: PathBuf,
    outputs: Vec<(PathBuf, Format)>,
    json: bool,
    max_work: Option<u64>,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let (mut input, mut outputs, mut json, mut max_work) = (None, Vec::new(), false, None);
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--max-work" => {
                let value = it.next().ok_or("--max-work needs a number of units")?;
                max_work =
                    Some(value.parse().map_err(|_| format!("--max-work {value}: give a whole number of units"))?);
            }
            "--out" => {
                let path = PathBuf::from(it.next().ok_or("--out needs a file")?);
                let format = Format::of_path(&path)
                    .ok_or_else(|| format!("{}: give a .step, .stp, .stl, .3mf or .obj file", path.display()))?;
                outputs.push((path, format));
            }
            "--stand-in" => {
                let path = PathBuf::from(it.next().ok_or("--stand-in needs a file")?);
                if !Format::StandIn.suits(&path) {
                    return Err(format!("{}: a mass stand-in is a .step or .stp file", path.display()));
                }
                outputs.push((path, Format::StandIn));
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
    Ok(Args { input, outputs, json, max_work })
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
    let Some(limit) = a.max_work else { return build(a).and_then(|exports| publish(a, &exports, None)) };
    let (exports, work) = soliddraft_kernel_catallax::with_request_allowance(limit, || build(a));
    if work.exhausted {
        return Err(format!(
            "{}: numbers.operation-budget: the request's {limit}-unit work allowance ran out",
            a.input.display()
        ));
    }
    publish(a, &exports?, Some(work))
}

/// Opens or builds the input and makes every requested file, writing nothing.
fn build(a: &Args) -> Result<Vec<Export>, String> {
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
    Ok(exports)
}

fn publish(a: &Args, exports: &[Export], work: Option<RequestWork>) -> Result<(), String> {
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
        report(path, *format, out, a.json, work);
    }
    Ok(())
}

fn write(path: &Path, out: &Export) -> Result<(), String> {
    soliddraft_document::save_atomic(path, &out.bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

fn report(path: &Path, format: Format, out: &Export, json: bool, work: Option<RequestWork>) {
    if json {
        let mut line = json!({"file": path, "format": format, "bytes": out.bytes.len(), "report": out.report});
        if let Some(work) = work {
            line["request_work"] = json!({"used": work.used, "limit": work.limit, "exhausted": work.exhausted});
        }
        println!("{line}");
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
        let a = parse(&args("part.dwprt --stand-in mass.stp --out part.stl")).unwrap();
        assert_eq!(a.outputs.iter().map(|(_, f)| *f).collect::<Vec<_>>(), [Format::StandIn, Format::Stl]);
        assert!(parse(&args("part.dwprt --stand-in mass.stl")).unwrap_err().contains(".step or .stp"));
        assert_eq!(parse(&args("part.dwprt --out p.stl --max-work 19999999")).unwrap().max_work, Some(19_999_999));
        assert!(parse(&args("part.dwprt --out p.stl --max-work lots")).unwrap_err().contains("whole number"));
    }

    #[test]
    fn over_tolerance_step_leaves_every_destination_untouched() {
        let dir = std::env::temp_dir().join(format!("soliddraft-export-refusal-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = dir.join("first.stl");
        let second = dir.join("second.step");
        let a = Args {
            input: "unused.json".into(),
            outputs: vec![(first.clone(), Format::Stl), (second.clone(), Format::Step)],
            json: true,
            max_work: None,
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
                removed_faces: 0,
                removed_edges: 0,
                warnings: Vec::new(),
            }),
        };
        let mesh = Export {
            bytes: b"mesh must not be published".to_vec(),
            report: Report::Mesh(soliddraft_exchange::MeshReport {
                triangles: 12,
                vertices: 8,
                chord_height: 0.01,
                closed: true,
                unavailable_controls: Vec::new(),
            }),
        };
        let exports = [mesh, step(false)];
        let error = publish(&a, &exports, None).unwrap_err();
        assert!(error.contains("second.step"));
        assert!(error.contains("0.001"));
        assert!(error.contains("exceeds the requested tolerance"));
        assert!(!first.exists());
        assert!(!second.exists());

        std::fs::write(&first, b"existing first").unwrap();
        std::fs::write(&second, b"existing second").unwrap();
        assert!(publish(&a, &exports, None).is_err());
        assert_eq!(std::fs::read(&first).unwrap(), b"existing first");
        assert_eq!(std::fs::read(&second).unwrap(), b"existing second");

        // A fully accepted batch still publishes the exact bytes it was given.
        let first = dir.join("first.step");
        let a = Args { outputs: vec![(first.clone(), Format::Step), (second.clone(), Format::Step)], ..a };
        let accepted = [step(true), step(true)];
        publish(&a, &accepted, None).unwrap();
        assert_eq!(std::fs::read(&first).unwrap(), accepted[0].bytes);
        assert_eq!(std::fs::read(&second).unwrap(), accepted[1].bytes);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
