//! `soliddraft asm`: assemblies without a window.
//!
//! Every command that changes an assembly opens it, makes the change through the same
//! [`AssemblyDocument`] calls the interface uses, and saves it atomically.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use glam::{DQuat, DVec3};
use serde_json::{Value, json};
use soliddraft_assembly::{
    AssemblyDocument, ComponentId, Direction, Flags, Freshness, KernelSource, LoadState, OccurrenceIndex,
    OccurrencePath, OpenOptions, PatternRule, Placement, RefChange,
};
use soliddraft_document::{DocKind, DocRef, identify_file};
use soliddraft_kernel_api::Kernel;
use soliddraft_kernel_catallax::CatallaxKernel;
use soliddraft_materials::{Appearance, Srgb};
use soliddraft_tessellation::Quality;

use crate::draw::search_folders;

pub(crate) const USAGE: &str = "\
usage: soliddraft asm new <file.dwasm> [--name <name>]
       soliddraft asm info <file.dwasm> [--json]
       soliddraft asm tree <file.dwasm> [--depth <n>] [--json]
       soliddraft asm insert <file.dwasm> <part.dwprt | sub.dwasm> [--at x,y,z] [--rotate ax,ay,az,deg]
       soliddraft asm pattern <file.dwasm> <component>... --linear dx,dy,dz --spacing <mm> --count <n>
                 [--second dx,dy,dz --spacing2 <mm> --count2 <n>]
       soliddraft asm pattern <file.dwasm> <component>... --circular ox,oy,oz,ax,ay,az --angle <deg> --count <n>
       soliddraft asm move <file.dwasm> <path>... (--to x,y,z | --by x,y,z) [--rotate ax,ay,az,deg]
       soliddraft asm hide|show|suppress|unsuppress <file.dwasm> <path>...
       soliddraft asm colour <file.dwasm> <path>... (--rgb '#rrggbb' [--metallic <0-1>] [--roughness <0-1>] | --none)
       soliddraft asm quality <file.dwasm> [<path>... (--set <quality> | --none)] [--every <quality>] [--json]
       soliddraft asm rename <file.dwasm> <component> <name>
       soliddraft asm delete <file.dwasm> <component>...
       soliddraft asm rebuild <file.dwasm> [--search <folder>]... [--json]
       soliddraft asm generate <folder> [--unique <n>] [--placements <n>] [--modules <n>] [--no-part-files] [--seed <n>]
       soliddraft asm bench <file.dwasm> [--size <w>x<h>] [--frames <n>] [--json]

new         Writes an empty assembly.

info        Opens the assembly, loads everything it shows and reports its id,
            revision, counts, loading states, references that have a newer version
            or are missing, how long each phase took and roughly how much memory
            the open assembly holds.

tree        Lists the occurrences with their paths, to the given depth (all by
            default).

insert      Places a part or an assembly, pinned at its current revision. --at is
            the position in millimetres, --rotate an axis and an angle in degrees.

pattern     Repeats components of the assembly: a linear pattern in one or two
            directions, or a circular one about an axis through a point. Counts
            include the seed.

move        Places occurrences: --to sets the position relative to the assembly
            that holds each (with --rotate, the rotation too); --by moves them in
            the root's frame.

hide, show, suppress, unsuppress
            Change occurrences. An occurrence below the root's own components is
            changed by an override in this assembly; its own file is untouched.

colour      Gives occurrences an assembly-level colour: every face of every part
            under each occurrence shows it in this assembly, replacing the parts'
            own colours. The part files keep theirs. --none takes it away.

quality     Shows how finely parts are tessellated in this assembly. With paths and
            --set, those occurrences and everything under them show at that
            quality instead of their parts' own; --none takes the override away.
            --every sets a quality for every part (auto: each part's own). A
            quality is auto, a slider position 1 to 10, or chord=MM,angle=DEG,...
            as for `soliddraft quality`. The settings are saved in the assembly but
            are not part of its revision.

rename      Names one of the root's components.

delete      Deletes components of the root, with patterns that only repeat them.

rebuild     Re-pins every reference at its document's current revision, refreshes
            the display copies and saves. Exits 1 when a reference is missing.

generate    Writes a large test assembly into the folder: unique part files under
            parts/, sub-assemblies under modules/ and root.dwasm. By default
            100,000 unique parts in 50 modules and 1,000,000 part occurrences.
            Each module places its parts once and stacks every one of them with
            one pattern. --no-part-files leaves the part files out (the
            assemblies open from their own copies all the same).

bench       Opens an assembly and uses it as a person would, through the app's
            viewport renderer on this machine's GPU, headless: frames while it
            loads and while the camera orbits, then select, move a part, move its
            whole module, hide and show, colour, and undo. Reports every phase's
            time, frame-time percentiles, frames over 100 ms, what was drawn, and
            memory.

Paths are occurrence ids from the root, such as 4/12 or 4/12:9.3 (member 3 of
pattern 9 of component 12); `asm tree` prints them.

  --search  another folder to look for documents in (repeatable)
  --json    print JSON
";

struct Args {
    files: Vec<String>,
    json: bool,
    search: Vec<PathBuf>,
    values: Vec<(String, String)>,
}

impl Args {
    fn value(&self, flag: &str) -> Option<&str> {
        self.values.iter().find(|(f, _)| f == flag).map(|(_, v)| v.as_str())
    }
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut a = Args { files: Vec::new(), json: false, search: Vec::new(), values: Vec::new() };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--json" => a.json = true,
            "--search" => a.search.push(PathBuf::from(it.next().ok_or("--search needs a folder")?)),
            f @ ("--none" | "--no-part-files") => a.values.push((f.to_owned(), String::new())),
            f @ ("--name" | "--depth" | "--at" | "--rotate" | "--linear" | "--spacing" | "--count" | "--second"
            | "--spacing2" | "--count2" | "--circular" | "--angle" | "--to" | "--by" | "--rgb" | "--metallic"
            | "--roughness" | "--unique" | "--placements" | "--modules" | "--seed" | "--size" | "--frames"
            | "--set" | "--every") => {
                let v = it.next().ok_or(format!("{f} needs a value"))?;
                a.values.push((f.to_owned(), v.clone()));
            }
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            s => a.files.push(s.to_owned()),
        }
    }
    Ok(a)
}

fn numbers<const N: usize>(flag: &str, text: &str) -> Result<[f64; N], String> {
    let v: Vec<f64> = text
        .split(',')
        .map(|t| t.trim().parse::<f64>())
        .collect::<Result<_, _>>()
        .map_err(|_| format!("{flag} takes {N} numbers separated by commas"))?;
    v.try_into().map_err(|_| format!("{flag} takes {N} numbers separated by commas"))
}

fn rotation(a: &Args) -> Result<Option<DQuat>, String> {
    a.value("--rotate")
        .map(|t| {
            let [x, y, z, deg] = numbers::<4>("--rotate", t)?;
            let axis = DVec3::new(x, y, z);
            if axis.length() < 1e-12 {
                return Err("--rotate needs a non-zero axis".to_owned());
            }
            Ok(DQuat::from_axis_angle(axis.normalize(), deg.to_radians()))
        })
        .transpose()
}

fn kernel() -> Arc<dyn Kernel> {
    Arc::new(CatallaxKernel::new())
}

fn options(a: &Args) -> OpenOptions {
    OpenOptions {
        search: search_folders(&a.search),
        display: Some(Arc::new(KernelSource::new(kernel()))),
        check_freshness: true,
        ..OpenOptions::default()
    }
}

/// `soliddraft asm <command> ...`.
pub(crate) fn run(args: &[String]) -> ExitCode {
    let Some(command) = args.first() else {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    };
    let a = match parse(&args[1..]) {
        Ok(a) => a,
        Err(e) => {
            eprint!("{e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let Some(file) = a.files.first().map(PathBuf::from) else {
        eprint!("{command} needs an assembly file\n{USAGE}");
        return ExitCode::from(2);
    };
    let rest = &a.files[1..];
    let result = match command.as_str() {
        "new" => new(&file, &a),
        "info" => info(&file, &a),
        "tree" => tree(&file, &a),
        "insert" => edit(&file, &a, |doc| insert(doc, rest, &a)),
        "pattern" => edit(&file, &a, |doc| pattern(doc, rest, &a)),
        "move" => edit(&file, &a, |doc| move_to(doc, rest, &a)),
        "hide" | "show" => {
            edit(&file, &a, |doc| doc.set_hidden(&paths(rest)?, command == "hide").map_err(|e| e.to_string()))
        }
        "suppress" | "unsuppress" => {
            edit(&file, &a, |doc| doc.set_suppressed(&paths(rest)?, command == "suppress").map_err(|e| e.to_string()))
        }
        "colour" | "color" => edit(&file, &a, |doc| colour(doc, rest, &a)),
        "quality" if rest.is_empty() && a.value("--every").is_none() => quality_info(&file, &a),
        "quality" => edit(&file, &a, |doc| quality(doc, rest, &a)),
        "rename" => edit(&file, &a, |doc| match rest {
            [c, name] => doc.rename(component(c)?, Some(name.clone())).map_err(|e| e.to_string()),
            _ => Err("rename takes a component id and a name".into()),
        }),
        "delete" => edit(&file, &a, |doc| {
            let ids = rest.iter().map(|c| component(c)).collect::<Result<Vec<_>, _>>()?;
            doc.delete(&ids).map_err(|e| e.to_string())
        }),
        "rebuild" => rebuild(&file, &a),
        "generate" => generate(&file, &a),
        "bench" => bench(&file, &a),
        _ => {
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("soliddraft asm: {e}");
            ExitCode::FAILURE
        }
    }
}

fn paths(texts: &[String]) -> Result<Vec<OccurrencePath>, String> {
    if texts.is_empty() {
        return Err("name at least one occurrence path".into());
    }
    texts.iter().map(|t| t.parse::<OccurrencePath>().map_err(|e| e.to_string())).collect()
}

fn component(text: &str) -> Result<ComponentId, String> {
    text.parse::<u32>().map(ComponentId).map_err(|_| format!("{text} is not a component id"))
}

fn open(file: &Path, a: &Args) -> Result<AssemblyDocument, String> {
    let mut doc = AssemblyDocument::open(file, options(a)).map_err(|e| format!("{}: {e}", file.display()))?;
    doc.finish_loading();
    Ok(doc)
}

fn edit(file: &Path, a: &Args, f: impl FnOnce(&mut AssemblyDocument) -> Result<(), String>) -> Result<bool, String> {
    let mut doc = open(file, a)?;
    f(&mut doc)?;
    doc.finish_loading();
    let saved = doc.save(None).map_err(|e| e.to_string())?;
    println!("saved {} (revision {})", saved.display(), doc.revision().short());
    Ok(true)
}

fn new(file: &Path, a: &Args) -> Result<bool, String> {
    let name = a.value("--name").map(str::to_owned).unwrap_or_else(|| {
        file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Assembly".into())
    });
    let mut doc = AssemblyDocument::new(name);
    let saved = doc.save(Some(file)).map_err(|e| e.to_string())?;
    println!("wrote {} ({})", saved.display(), doc.id());
    Ok(true)
}

fn insert(doc: &mut AssemblyDocument, rest: &[String], a: &Args) -> Result<(), String> {
    let [target] = rest else { return Err("insert takes one part or assembly file".into()) };
    let target = PathBuf::from(target);
    let identity = identify_file(&target).map_err(|e| format!("{}: {e}", target.display()))?;
    if !matches!(identity.kind, DocKind::Part | DocKind::Assembly) {
        return Err(format!("{} is a {}, not a part or an assembly", target.display(), identity.kind));
    }
    let here = doc.path().ok_or("the assembly has no file")?.to_path_buf();
    let mut r = DocRef::new(identity.id, identity.kind, "", identity.revision);
    r.set_path(&here, &target);
    let at = a.value("--at").map(|t| numbers::<3>("--at", t)).transpose()?.unwrap_or([0.0; 3]);
    let placement = Placement::new(rotation(a)?.unwrap_or(DQuat::IDENTITY), DVec3::from_array(at));
    let id = doc.insert(r, placement).map_err(|e| e.to_string())?;
    println!("inserted {} as component {id}", target.display());
    Ok(())
}

fn pattern(doc: &mut AssemblyDocument, rest: &[String], a: &Args) -> Result<(), String> {
    let seeds = rest.iter().map(|c| component(c)).collect::<Result<Vec<_>, _>>()?;
    let count = |flag: &str| -> Result<u32, String> {
        a.value(flag)
            .ok_or(format!("the pattern needs {flag}"))?
            .parse()
            .map_err(|_| format!("{flag} takes a whole number"))
    };
    let number = |flag: &str| -> Result<f64, String> {
        a.value(flag).ok_or(format!("the pattern needs {flag}"))?.parse().map_err(|_| format!("{flag} takes a number"))
    };
    let rule = if let Some(d) = a.value("--linear") {
        let second = match a.value("--second") {
            Some(s) => Some(Direction {
                direction: numbers::<3>("--second", s)?,
                spacing: number("--spacing2")?,
                count: count("--count2")?,
            }),
            None => None,
        };
        PatternRule::Linear {
            direction: numbers::<3>("--linear", d)?,
            spacing: number("--spacing")?,
            count: count("--count")?,
            second,
        }
    } else if let Some(c) = a.value("--circular") {
        let [ox, oy, oz, ax, ay, az] = numbers::<6>("--circular", c)?;
        PatternRule::Circular {
            origin: [ox, oy, oz],
            axis: [ax, ay, az],
            angle: number("--angle")?,
            count: count("--count")?,
        }
    } else {
        return Err("the pattern needs --linear or --circular".into());
    };
    let id = doc.add_pattern(seeds, rule).map_err(|e| e.to_string())?;
    println!("added pattern {id}");
    Ok(())
}

fn move_to(doc: &mut AssemblyDocument, rest: &[String], a: &Args) -> Result<(), String> {
    let paths = paths(rest)?;
    let rot = rotation(a)?;
    match (a.value("--to"), a.value("--by")) {
        (Some(t), None) => {
            let to = DVec3::from_array(numbers::<3>("--to", t)?);
            let mut moves = Vec::new();
            for p in &paths {
                let row = doc.table().find(p).ok_or(format!("no occurrence at {p}"))?;
                let r = rot.unwrap_or(doc.table().local(row).rotation);
                moves.push((p.clone(), Placement::new(r, to)));
            }
            doc.move_to(&moves).map_err(|e| e.to_string())
        }
        (None, Some(b)) => {
            let by = Placement::new(rot.unwrap_or(DQuat::IDENTITY), DVec3::from_array(numbers::<3>("--by", b)?));
            doc.move_by(&paths, &by).map_err(|e| e.to_string())
        }
        _ => Err("move takes --to or --by".into()),
    }
}

fn colour(doc: &mut AssemblyDocument, rest: &[String], a: &Args) -> Result<(), String> {
    let paths = paths(rest)?;
    let appearance = match (a.value("--rgb"), a.value("--none")) {
        (Some(rgb), None) => {
            let base: Srgb = rgb.parse().map_err(|e: soliddraft_materials::ParseColourError| e.to_string())?;
            let factor = |flag: &str, default: f32| -> Result<f32, String> {
                a.value(flag)
                    .map(|v| v.parse::<f32>().map_err(|_| format!("{flag} takes a number")))
                    .transpose()
                    .map(|v| v.unwrap_or(default))
            };
            let id = format!("assembly-{}", base.to_string().trim_start_matches('#'));
            let mut ap = Appearance::new(id, base.to_string(), base);
            ap.metallic = factor("--metallic", 0.0)?;
            ap.roughness = factor("--roughness", 0.5)?;
            if ap.metallic > 0.0 || ap.roughness != 0.5 {
                ap.id = soliddraft_materials::AppearanceId(format!("{}-m{}-r{}", ap.id, ap.metallic, ap.roughness));
            }
            Some(ap)
        }
        (None, Some(_)) => None,
        _ => return Err("colour takes --rgb or --none".into()),
    };
    doc.set_colour(&paths, appearance).map_err(|e| e.to_string())
}

fn quality(doc: &mut AssemblyDocument, rest: &[String], a: &Args) -> Result<(), String> {
    if let Some(q) = a.value("--every") {
        doc.set_viewport_quality(q.parse()?).map_err(|e| e.to_string())?;
    }
    if rest.is_empty() {
        return Ok(());
    }
    let paths = paths(rest)?;
    let q = match (a.value("--set"), a.value("--none")) {
        (Some(q), None) => Some(q.parse::<Quality>()?),
        (None, Some(_)) => None,
        _ => return Err("quality with paths takes --set or --none".into()),
    };
    doc.set_quality(&paths, q).map_err(|e| e.to_string())
}

/// The assembly's quality settings, and the definitions shown at a quality other than
/// their parts' own with their triangle counts.
fn quality_info(file: &Path, a: &Args) -> Result<bool, String> {
    let doc = open(file, a)?;
    let s = doc.settings();
    let defs: Vec<(String, String, usize)> = doc
        .store()
        .iter()
        .filter_map(|(_, d)| {
            let q = d.quality.as_ref()?;
            Some((d.label.clone(), q.describe(), d.mesh.as_ref().map_or(0, |m| m.triangle_count())))
        })
        .collect();
    if a.json {
        let out = json!({
            "every": s.viewport,
            "overrides": s.overrides,
            "definitions": defs.iter().map(|(l, q, t)| json!({"label": l, "quality": q, "triangles": t})).collect::<Vec<_>>(),
        });
        println!("{out}");
    } else {
        println!("{}", doc.assembly().name);
        let every = if s.viewport == Quality::Auto { "each part's own".to_owned() } else { s.viewport.describe() };
        println!("  every part  {every}");
        for o in &s.overrides {
            println!("  [{}]  {}", o.path, o.quality.describe());
        }
        for (label, q, t) in &defs {
            println!("  {label} at {q}: {t} triangles");
        }
    }
    Ok(true)
}

fn count(a: &Args, flag: &str, default: usize) -> Result<usize, String> {
    a.value(flag)
        .map(|v| v.replace('_', "").parse().map_err(|_| format!("{flag} takes a whole number")))
        .transpose()
        .map(|v| v.unwrap_or(default))
}

fn generate(dir: &Path, a: &Args) -> Result<bool, String> {
    let spec = crate::generate::Spec {
        unique: count(a, "--unique", 100_000)?,
        placements: count(a, "--placements", 1_000_000)?,
        modules: count(a, "--modules", 50)?,
        part_files: a.value("--no-part-files").is_none(),
        seed: count(a, "--seed", 1)? as u64,
    };
    let g = crate::generate::generate(dir, &spec)?;
    println!(
        "wrote {} ({} unique parts, about {} placements in {} modules; parts {:.1} s, assemblies {:.1} s, {:.0} MiB of assemblies)",
        g.root.display(),
        spec.unique,
        spec.placements,
        spec.modules,
        g.parts_seconds,
        g.assemblies_seconds,
        g.bytes as f64 / (1024.0 * 1024.0)
    );
    Ok(true)
}

fn bench(file: &Path, a: &Args) -> Result<bool, String> {
    let size = match a.value("--size") {
        Some(s) => {
            let (w, h) = s.split_once('x').ok_or("--size takes <width>x<height>")?;
            [
                w.parse().map_err(|_| "--size takes <width>x<height>")?,
                h.parse().map_err(|_| "--size takes <width>x<height>")?,
            ]
        }
        None => [1280, 800],
    };
    let report = crate::bench::run(file, size, count(a, "--frames", 120)?)?;
    if a.json {
        println!("{report}");
    } else {
        println!("{}", serde_json::to_string_pretty(&report).expect("a report serialises"));
    }
    Ok(report["stalls"].as_u64() == Some(0))
}

fn mib(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn info(file: &Path, a: &Args) -> Result<bool, String> {
    let started = Instant::now();
    let mut doc = AssemblyDocument::open(file, options(a)).map_err(|e| format!("{}: {e}", file.display()))?;
    let structure = started.elapsed();
    let phases = doc.finish_loading();
    let total = started.elapsed();
    let store = doc.store();
    let counts = doc.table().counts();
    let states = store.counts();
    let (mut newer, mut missing) = (Vec::new(), Vec::new());
    for (_, d) in store.iter() {
        match &d.freshness {
            Freshness::NewerAvailable { now } => newer.push((d.label.clone(), d.key.revision.clone(), now.clone())),
            Freshness::Missing => missing.push(d.label.clone()),
            _ => {}
        }
    }
    let failed: Vec<(String, String)> = store
        .iter()
        .filter(|(_, d)| d.state == LoadState::Failed)
        .map(|(_, d)| (d.label.clone(), d.problem.clone().unwrap_or_default()))
        .collect();
    let triangles: usize = doc
        .table()
        .all_flags()
        .iter()
        .zip(doc.table().definitions())
        .filter(|(f, _)| f.drawn_part())
        .filter_map(|(_, d)| store.get(*d).mesh.as_ref().map(|m| m.triangle_count()))
        .sum();
    let memory = doc.table().heap_bytes() + store.heap_bytes();
    let a_ = doc.assembly();
    if a.json {
        println!(
            "{}",
            json!({
                "file": file.display().to_string(),
                "id": doc.id(),
                "name": a_.name,
                "revision": doc.revision(),
                "sources": a_.sources.len(),
                "components": a_.components.len(),
                "patterns": a_.patterns.len(),
                "definitions": store.len(),
                "occurrences": { "parts": counts.parts, "drawn": counts.drawn, "assemblies": counts.assemblies,
                                 "hidden": counts.hidden, "suppressed": counts.suppressed },
                "states": LoadState::ALL.iter().zip(states).map(|(s, n)| (s.as_str().to_owned(), Value::from(n))).collect::<serde_json::Map<_, _>>(),
                "triangles_drawn": triangles,
                "newer": newer.iter().map(|(l, p, n)| json!({"document": l, "pinned": p, "now": n})).collect::<Vec<_>>(),
                "missing": missing,
                "failed": failed.iter().map(|(l, p)| json!({"document": l, "problem": p})).collect::<Vec<_>>(),
                "seconds": { "structure": structure.as_secs_f64(), "display": phases.map(|p| p.0.as_secs_f64()),
                             "freshness": phases.map(|p| p.1.as_secs_f64()), "total": total.as_secs_f64() },
                "memory_mib": mib(memory),
            })
        );
    } else {
        println!("{} ({}) {}", a_.name, file.display(), doc.id());
        println!("  revision {}", doc.revision());
        println!(
            "  {} sources, {} components, {} patterns, {} definitions",
            a_.sources.len(),
            a_.components.len(),
            a_.patterns.len(),
            store.len()
        );
        println!(
            "  occurrences: {} parts ({} drawn, {} hidden, {} suppressed), {} assemblies",
            counts.parts, counts.drawn, counts.hidden, counts.suppressed, counts.assemblies
        );
        let states: Vec<String> = LoadState::ALL
            .iter()
            .zip(states)
            .filter(|(_, n)| *n > 0)
            .map(|(s, n)| format!("{n} {}", s.as_str()))
            .collect();
        println!("  definitions: {}", states.join(", "));
        println!("  {triangles} triangles drawn");
        for (l, p, n) in &newer {
            println!("  newer version: {l} pinned at {} is now {}", p.short(), n.short());
        }
        for l in &missing {
            println!("  missing: {l}");
        }
        for (l, p) in &failed {
            println!("  failed: {l}: {p}");
        }
        let phase =
            |d: Option<std::time::Duration>| d.map(|d| format!("{:.3} s", d.as_secs_f64())).unwrap_or("-".into());
        println!(
            "  opened in {:.3} s (structure {:.3} s, display {}, freshness {}), about {:.0} MiB",
            total.as_secs_f64(),
            structure.as_secs_f64(),
            phase(phases.map(|p| p.0)),
            phase(phases.map(|p| p.1)),
            mib(memory)
        );
    }
    Ok(missing.is_empty() && failed.is_empty())
}

fn tree(file: &Path, a: &Args) -> Result<bool, String> {
    let doc = AssemblyDocument::open(file, OpenOptions { search: search_folders(&a.search), ..OpenOptions::default() })
        .map_err(|e| format!("{}: {e}", file.display()))?;
    let depth: usize =
        a.value("--depth").map(|d| d.parse().map_err(|_| "--depth takes a number")).transpose()?.unwrap_or(usize::MAX);
    let t = doc.table();
    let mut rows = Vec::new();
    let mut stack: Vec<(OccurrenceIndex, usize)> =
        t.children(OccurrenceIndex::ROOT).collect::<Vec<_>>().into_iter().rev().map(|r| (r, 1)).collect();
    while let Some((r, d)) = stack.pop() {
        rows.push((r, d));
        if d < depth {
            stack.extend(t.children(r).collect::<Vec<_>>().into_iter().rev().map(|c| (c, d + 1)));
        }
    }
    if a.json {
        let list: Vec<Value> = rows
            .iter()
            .map(|(r, d)| {
                let f = t.flags(*r);
                let p = t.world(*r);
                json!({
                    "path": t.path(*r).to_string(), "depth": d, "label": doc.label(*r),
                    "kind": if f.has(Flags::ASSEMBLY) { "assembly" } else { "part" },
                    "hidden": f.has(Flags::HIDDEN), "suppressed": f.has(Flags::SUPPRESSED),
                    "colour": t.colour(*r).map(|a| a.base_color.to_string()),
                    "quality": t.quality(*r).map(Quality::describe),
                    "translation": p.translation.to_array(), "rotation": p.rotation.to_array(),
                })
            })
            .collect();
        println!("{}", json!(list));
    } else {
        println!("{}", doc.assembly().name);
        for (r, d) in rows {
            let f = t.flags(r);
            let mut marks = String::new();
            if f.has(Flags::HIDDEN_SELF) {
                marks.push_str(" (hidden)");
            }
            if f.has(Flags::SUPPRESSED_SELF) {
                marks.push_str(" (suppressed)");
            }
            if f.has(Flags::COLOURED_SELF)
                && let Some(a) = t.colour(r)
            {
                marks.push_str(&format!(" ({})", a.base_color));
            }
            println!("{}{}  [{}]{marks}", "  ".repeat(d), doc.label(r), t.path(r));
        }
    }
    Ok(true)
}

fn rebuild(file: &Path, a: &Args) -> Result<bool, String> {
    let mut doc = open(file, a)?;
    let report = doc.rebuild();
    if let Some(refusal) = &report.refusal {
        return Err(refusal.clone());
    }
    doc.finish_loading();
    let saved = doc.save(None).map_err(|e| e.to_string())?;
    if a.json {
        println!(
            "{}",
            json!({
                "file": saved.display().to_string(),
                "revision": doc.revision(),
                "sources": report.sources.iter().map(|s| json!({
                    "source": s.source.0, "document": s.label,
                    "status": match s.status { RefChange::Unchanged => "unchanged", RefChange::Updated => "updated", RefChange::Missing => "missing" },
                    "before": s.before, "after": s.after, "moved_to": s.moved,
                })).collect::<Vec<_>>(),
            })
        );
    } else {
        for s in &report.sources {
            match s.status {
                RefChange::Updated => println!("  {}: {} -> {}", s.label, s.before.short(), s.after.short()),
                RefChange::Missing => println!("  {}: missing, still pinned at {}", s.label, s.before.short()),
                RefChange::Unchanged => {}
            }
            if let Some(p) = &s.moved {
                println!("  {}: found at {p}", s.label);
            }
        }
        println!(
            "rebuilt {}: {} updated, {} unchanged, {} missing; saved {}",
            file.display(),
            report.updated(),
            report.sources.len() - report.updated() - report.missing(),
            report.missing(),
            saved.display()
        );
    }
    Ok(report.missing() == 0)
}
