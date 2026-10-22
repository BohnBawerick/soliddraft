//! `soliddraft doc` and `soliddraft locations`: documents, their references and File
//! locations, without a window.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use serde_json::{Value, json};
use soliddraft_document::manifest::Package;
use soliddraft_document::resolve::{rebind, repair};
use soliddraft_document::{
    DocKind, DocRef, FileLocations, FolderIndex, LocationKind, RefStatus, Resolution, Resolver, Target, Via,
    identify_file,
};
use soliddraft_drawing::file;
use soliddraft_kernel_api::Kernel;
use soliddraft_kernel_catallax::CatallaxKernel;

use crate::draw::{missing_text, search_folders};

pub(crate) const USAGE: &str = "\
usage: soliddraft doc info <file> [--json]
       soliddraft doc refs <file> [--search <folder>]... [--json]
       soliddraft doc rebuild <drawing | assembly> [--search <folder>]... [--json]
       soliddraft doc repair <drawing> --to <part> [--replace] [--json]
       soliddraft doc migrate <file>... [--json]
       soliddraft doc where-used <file> [--search <folder>]... [--json]
       soliddraft locations [list] [--json]
       soliddraft locations add|remove <kind> <path>

info        The document's id, kind, format, writer, revision and references.

refs        Resolves each reference the document holds: its open state against the
            revision it pins. A reference is current, has a newer version (the
            document shows the pinned revision until a Rebuild), is missing (with
            candidates to repair it with), or points at a source that is not a
            SolidDraft document. A reference found somewhere else than its stored
            path says where. Exits 1 when a reference is missing.

rebuild     Brings a drawing to the current revision of its part; the same as
            `soliddraft draw --update`, whose options it takes. For an assembly,
            the same as `soliddraft asm rebuild`.

repair      Points a drawing's missing part reference at another file. The file
            must be the same part (same id) unless --replace says to use a
            different one. The drawing keeps showing the revision it pinned until
            a Rebuild. A .dwdrawing drawing is written as .dwdrw beside it.

migrate     Writes documents from before the envelope in the current format: a
            .dwpart part becomes a .dwprt beside it, keeping the id it opens with; a
            .dwdrawing drawing becomes a .dwdrw beside it, its part reference bound
            to the part's id when the part is where the drawing says.

where-used  The documents that reference this one, in its folder and subfolders,
            the --search folders and the File locations search folders.

locations   File locations: the folders and files where reference files are found,
            per kind. Kinds: referenced_documents, material_libraries,
            appearance_libraries, part_templates, assembly_templates,
            drawing_templates, sheet_formats, drafting_styles,
            configuration_tables.

  --search  another folder to look for documents in (repeatable)
  --json    print JSON
";

struct Args {
    files: Vec<PathBuf>,
    search: Vec<PathBuf>,
    json: bool,
    to: Option<PathBuf>,
    replace: bool,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut a = Args { files: Vec::new(), search: Vec::new(), json: false, to: None, replace: false };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = |flag: &str| it.next().cloned().ok_or(format!("{flag} needs a value"));
        match arg.as_str() {
            "--json" => a.json = true,
            "--replace" => a.replace = true,
            "--search" => a.search.push(PathBuf::from(value("--search")?)),
            "--to" => a.to = Some(PathBuf::from(value("--to")?)),
            s if s.starts_with('-') => return Err(format!("unknown option {s}")),
            s => a.files.push(PathBuf::from(s)),
        }
    }
    Ok(a)
}

/// `soliddraft doc <command> ...`.
pub(crate) fn run(args: &[String]) -> ExitCode {
    let Some(command) = args.first() else {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    };
    if command == "rebuild" && args[1..].iter().any(|a| DocKind::of_path(Path::new(a)) == Some(DocKind::Assembly)) {
        return crate::asm::run(args);
    }
    if command == "rebuild" {
        let mut forward = vec!["--update".to_owned()];
        forward.extend(args[1..].iter().cloned());
        return crate::draw::run(&forward);
    }
    let a = match parse(&args[1..]) {
        Ok(a) => a,
        Err(e) => {
            eprint!("{e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let one = |a: &Args| -> Result<PathBuf, String> {
        match &a.files[..] {
            [f] => Ok(f.clone()),
            _ => Err(format!("{command} takes one file")),
        }
    };
    let result = match command.as_str() {
        "info" => one(&a).and_then(|f| info(&f, a.json)),
        "refs" => one(&a).and_then(|f| refs(&f, &a)),
        "repair" => one(&a).and_then(|f| repair_drawing(&f, &a)),
        "migrate" if !a.files.is_empty() => migrate(&a),
        "where-used" => one(&a).and_then(|f| where_used(&f, &a)),
        _ => {
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    finish(result)
}

fn finish(result: Result<bool, String>) -> ExitCode {
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("soliddraft doc: {e}");
            ExitCode::FAILURE
        }
    }
}

fn reference_json(r: &DocRef) -> Value {
    serde_json::to_value(r).expect("references serialise")
}

fn info(path: &Path, as_json: bool) -> Result<bool, String> {
    let identity = identify_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let package = Package::from_bytes(&bytes).map_err(|e| e.to_string())?;
    let manifest = package.manifest();
    let report = json!({
        "file": path.display().to_string(),
        "id": identity.id,
        "kind": identity.kind,
        "legacy": identity.legacy,
        "format": manifest.map(|m| m.format.clone()),
        "writer": manifest.map(|m| m.writer.clone()),
        "kernel": manifest.and_then(|m| m.kernel.clone()),
        "revision": identity.revision,
        "geometry_hash": manifest.and_then(|m| m.geometry_hash.clone()),
        "references": identity.references.iter().map(reference_json).collect::<Vec<_>>(),
    });
    if as_json {
        println!("{report}");
    } else {
        let format = manifest.map(|m| m.format.as_str()).unwrap_or("from before the envelope");
        println!("{}: {} {} ({format})", path.display(), identity.kind, identity.id);
        println!("  revision {}", identity.revision.short());
        for r in &identity.references {
            println!("  references {} {} at rev {}", r.kind, r.path, r.pinned.short());
        }
    }
    Ok(true)
}

fn status_json(r: &DocRef, resolution: &Resolution) -> Value {
    let mut out = json!({ "reference": reference_json(r) });
    match resolution {
        Resolution::Found(t) => {
            let status = match t.status(r) {
                RefStatus::Current => json!("current"),
                RefStatus::NewerAvailable { .. } => json!("newer_available"),
                RefStatus::Unverified => json!("unverified"),
            };
            out["status"] = status;
            out["found"] = json!(t.path().map(|p| p.display().to_string()));
            out["via"] = json!(via_name(t.via));
            out["now"] = json!(t.revision());
            out["new_path"] = json!(t.new_path);
            out["duplicates"] = json!(t.duplicates.iter().map(|p| p.display().to_string()).collect::<Vec<_>>());
        }
        Resolution::Missing(m) => {
            out["status"] = json!("missing");
            out["problem"] = json!(missing_text(r, m));
            out["candidates"] = json!(
                m.candidates
                    .iter()
                    .map(
                        |c| json!({"path": c.path.display().to_string(), "id": c.identity.id, "same_name": c.same_name})
                    )
                    .collect::<Vec<_>>()
            );
        }
    }
    out
}

fn via_name(via: Via) -> &'static str {
    match via {
        Via::OpenDocument => "open_document",
        Via::StoredPath => "stored_path",
        Via::SavedSibling => "saved_sibling",
        Via::Index => "index",
    }
}

fn status_text(r: &DocRef, resolution: &Resolution) -> String {
    match resolution {
        Resolution::Found(t) => {
            let at = found_at(t);
            match t.status(r) {
                RefStatus::Current => format!("current at rev {}{at}", r.pinned.short()),
                RefStatus::NewerAvailable { now } => {
                    format!("newer version available: pinned at rev {}, now rev {}{at}", r.pinned.short(), now.short())
                }
                RefStatus::Unverified => format!("found, not a SolidDraft document{at}"),
            }
        }
        Resolution::Missing(m) => missing_text(r, m),
    }
}

fn found_at(t: &Target) -> String {
    match &t.new_path {
        Some(p) => format!(" (found at {p})"),
        None => String::new(),
    }
}

fn refs(path: &Path, a: &Args) -> Result<bool, String> {
    let identity = identify_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let resolver = Resolver::new(Some(path)).with_search_folders(search_folders(&a.search));
    let mut ok = true;
    let mut out = Vec::new();
    for r in &identity.references {
        let resolution = resolver.resolve(r);
        ok &= matches!(resolution, Resolution::Found(_));
        if a.json {
            out.push(status_json(r, &resolution));
        } else {
            println!("{} {}: {}", r.kind, r.path, status_text(r, &resolution));
        }
    }
    if a.json {
        println!("{}", json!({ "file": path.display().to_string(), "references": out }));
    }
    Ok(ok)
}

fn repair_drawing(path: &Path, a: &Args) -> Result<bool, String> {
    let to = a.to.as_deref().ok_or("repair needs --to <part>")?;
    let (mut drawing, meta) = file::read(path).map_err(|e| e.to_string())?;
    let identity = repair(&mut drawing.model, path, to, a.replace).map_err(|e| e.to_string())?;
    let kernel = meta.kernel.clone();
    let saved = soliddraft_document::kind::saved_path(path, DocKind::Drawing);
    file::write(&saved, &drawing, &meta.rewritten(&kernel, drawing.meta.units)).map_err(|e| e.to_string())?;
    let status = if identity.revision == drawing.model.pinned { "current" } else { "newer_available" };
    if a.json {
        println!(
            "{}",
            json!({"drawing": saved.display().to_string(), "reference": reference_json(&drawing.model), "status": status})
        );
    } else {
        println!("{}: the part is now {}", saved.display(), drawing.model.path);
        if status != "current" {
            println!("  the drawing shows rev {} until a Rebuild", drawing.model.pinned.short());
        }
    }
    Ok(true)
}

fn migrate(a: &Args) -> Result<bool, String> {
    let kernel: Arc<dyn Kernel> = Arc::new(CatallaxKernel::new());
    let mut out = Vec::new();
    for path in &a.files {
        let identity = identify_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let written = if !identity.legacy {
            None
        } else {
            match identity.kind {
                DocKind::Part => {
                    let doc = soliddraft_model::Document::open(path, kernel.clone())
                        .map_err(|e| format!("{}: {e}", path.display()))?;
                    let to = soliddraft_document::kind::saved_path(path, DocKind::Part);
                    doc.save(&to).map_err(|e| format!("{}: {e}", to.display()))?;
                    Some(to)
                }
                DocKind::Drawing => {
                    let (mut drawing, meta) = file::read(path).map_err(|e| e.to_string())?;
                    let resolver = Resolver::new(Some(path)).with_search_folders(search_folders(&a.search));
                    if let Resolution::Found(t) = resolver.resolve(&drawing.model) {
                        rebind(&mut drawing.model, &t);
                    }
                    let kernel = meta.kernel.clone();
                    let to = soliddraft_document::kind::saved_path(path, DocKind::Drawing);
                    file::write(&to, &drawing, &meta.rewritten(&kernel, drawing.meta.units))
                        .map_err(|e| e.to_string())?;
                    Some(to)
                }
                // Assemblies have had a manifest from their first version.
                DocKind::Assembly => None,
            }
        };
        if a.json {
            out.push(json!({
                "file": path.display().to_string(),
                "id": identity.id,
                "written": written.as_ref().map(|p| p.display().to_string()),
            }));
        } else {
            match &written {
                Some(to) => println!("{} -> {} ({} {})", path.display(), to.display(), identity.kind, identity.id),
                None => println!("{}: already in the current format", path.display()),
            }
        }
    }
    if a.json {
        println!("{}", json!(out));
    }
    Ok(true)
}

fn where_used(path: &Path, a: &Args) -> Result<bool, String> {
    let identity = identify_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let mut roots = vec![dir];
    roots.extend(search_folders(&a.search));
    let index = FolderIndex::scan(roots);
    let users: Vec<_> = index.where_used(identity.id);
    if a.json {
        let list: Vec<Value> = users
            .iter()
            .map(|e| json!({"file": e.path.display().to_string(), "id": e.identity.id, "kind": e.identity.kind}))
            .collect();
        println!("{}", json!({"file": path.display().to_string(), "id": identity.id, "used_by": list}));
    } else if users.is_empty() {
        println!("{}: not referenced by any document found", path.display());
    } else {
        for e in users {
            println!("{} {}", e.identity.kind, e.path.display());
        }
    }
    Ok(true)
}

/// `soliddraft locations ...`.
pub(crate) fn locations(args: &[String]) -> ExitCode {
    let json = args.iter().any(|a| a == "--json");
    let words: Vec<&str> = args.iter().map(String::as_str).filter(|a| *a != "--json").collect();
    let result = match words[..] {
        [] | ["list"] => Ok(list_locations(&soliddraft_app::settings::file_locations(), json)),
        [verb @ ("add" | "remove"), kind, path] => change_location(verb, kind, path),
        _ => {
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    finish(result)
}

fn list_locations(locations: &FileLocations, json: bool) -> bool {
    if json {
        println!("{}", serde_json::to_string(locations).expect("locations serialise"));
        return true;
    }
    for kind in LocationKind::ALL {
        let entries = locations.get(kind);
        if entries.is_empty() {
            println!("{} ({}): built-in only", kind.label(), kind.key());
        } else {
            println!("{} ({}):", kind.label(), kind.key());
            for e in entries {
                println!("  {}", e.display());
            }
        }
    }
    true
}

fn change_location(verb: &str, kind: &str, path: &str) -> Result<bool, String> {
    let kind = LocationKind::from_key(kind).ok_or_else(|| format!("unknown kind {kind}"))?;
    let path = std::path::absolute(path).map_err(|e| format!("{path}: {e}"))?;
    let mut locations = soliddraft_app::settings::file_locations();
    let changed = if verb == "add" { locations.add(kind, path.clone()) } else { locations.remove(kind, &path) };
    if !changed {
        let why = if verb == "add" { "already listed" } else { "not listed" };
        return Err(format!("{} is {why} under {}", path.display(), kind.key()));
    }
    soliddraft_app::settings::save_file_locations(&locations)?;
    println!(
        "{}: {}",
        kind.label(),
        locations.get(kind).iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
    );
    Ok(true)
}
