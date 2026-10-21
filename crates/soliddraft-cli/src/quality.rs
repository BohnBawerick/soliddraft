//! `soliddraft quality`: a part's viewport quality and export accuracy, and what they
//! give its body, without a window.

use std::path::Path;
use std::process::ExitCode;

use serde_json::json;
use soliddraft_tessellation::{ExportTolerance, Quality, Report, Settings};

use crate::material::{open, save};

pub(crate) const USAGE: &str = "\
usage: soliddraft quality <part> [--set <quality>] [--export <chord>,<angle>]
                         [--display-data on|off] [--json]

Reports a part's viewport quality and, for each of its three display levels
(coarse, standard and fine), the largest deviation from the exact surface in
millimetres and the triangle count. With any of the options below it changes
the settings and saves the part in place (a .dwpart part as .dwprt beside it).
Settings are not part of the part's revision: assemblies and drawings that pin
the part see no change.

  --set           auto, a slider position from 1 (coarsest) to 10 (finest),
                  or advanced values as chord=MM,angle=DEG,edge=MM,aspect=R
                  (any of them; chord 0.1 mm and 15 degrees by default)
  --export        the chord height in mm and the angle in degrees for
                  exported meshes, such as 0.01,10
  --display-data  whether saving keeps display meshes in the file
  --json          print JSON
";

#[derive(Default)]
struct Args {
    part: Option<String>,
    set: Option<Quality>,
    export: Option<ExportTolerance>,
    display_data: Option<bool>,
    json: bool,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = |flag: &str| it.next().cloned().ok_or(format!("{flag} needs a value"));
        match arg.as_str() {
            "--json" => a.json = true,
            "--set" => a.set = Some(value("--set")?.parse()?),
            "--export" => {
                let v = value("--export")?;
                let (chord, angle) = v.split_once(',').ok_or(format!("--export {v:?}: give chord,angle"))?;
                let number = |s: &str| s.trim().parse::<f64>().map_err(|_| format!("{s:?} is not a number"));
                let e = ExportTolerance { chord_height: number(chord)?, angle_deg: number(angle)? };
                e.check()?;
                a.export = Some(e);
            }
            "--display-data" => {
                a.display_data = Some(match value("--display-data")?.as_str() {
                    "on" => true,
                    "off" => false,
                    other => return Err(format!("--display-data {other:?}: on or off")),
                })
            }
            flag if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
            part if a.part.is_none() => a.part = Some(part.to_owned()),
            extra => return Err(format!("one part at a time, not also {extra}")),
        }
    }
    Ok(a)
}

pub(crate) fn run(args: &[String]) -> ExitCode {
    let a = match parse(args) {
        Ok(a) if a.part.is_some() => a,
        Ok(_) => {
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
        Err(e) => {
            eprint!("{e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match quality(&a) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("soliddraft quality: {e}");
            ExitCode::FAILURE
        }
    }
}

fn quality(a: &Args) -> Result<(), String> {
    let part = Path::new(a.part.as_deref().expect("checked"));
    let mut doc = open(part)?;
    let revision = doc.semantic_hash();
    let mut settings = doc.settings().clone();
    if let Some(q) = a.set {
        settings.viewport = q;
    }
    if let Some(e) = a.export {
        settings.export = e;
    }
    if let Some(d) = a.display_data {
        settings.save_display_data = d;
    }
    let mut saved = None;
    if settings != *doc.settings() {
        doc.set_settings(settings)?;
        saved = Some(save(&doc, part)?);
    }
    let report = doc.quality_report().transpose().map_err(|e| format!("{}: {e}", part.display()))?;
    debug_assert_eq!(doc.semantic_hash(), revision);
    if a.json {
        let out = json!({
            "file": part.display().to_string(),
            "saved": saved.as_ref().map(|p| p.display().to_string()),
            "revision": revision.to_string(),
            "settings": doc.settings(),
            "report": report,
        });
        println!("{out}");
    } else {
        print_text(part, doc.settings(), report.as_ref(), saved.as_deref());
    }
    Ok(())
}

fn print_text(part: &Path, s: &Settings, report: Option<&Report>, saved: Option<&Path>) {
    println!("{}", part.display());
    println!("  viewport  {}", s.viewport.describe());
    println!("  export    chord {} mm, {}°", s.export.chord_height, s.export.angle_deg);
    println!("  display data {}", if s.save_display_data { "saved" } else { "not saved" });
    match report {
        None => println!("  no body"),
        Some(r) => {
            println!("  size      {:.3} mm diagonal", r.diagonal_mm);
            for l in &r.levels {
                let deviation = l.deviation_mm.map_or("unbounded".to_owned(), |d| format!("{d:.4} mm"));
                println!("  {:<9} deviation {deviation:<12} {} triangles", l.level.as_str(), l.triangles);
            }
        }
    }
    if let Some(p) = saved {
        println!("  saved {}", p.display());
    }
}
