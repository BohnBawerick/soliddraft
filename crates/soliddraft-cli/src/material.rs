//! `soliddraft material`: material and appearance libraries, and a part's material,
//! appearances and mass, without a window.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use serde_json::{Value, json};
use soliddraft_document::{ConfigScope, ConfigurationId};
use soliddraft_kernel_api::{FaceId, Kernel};
use soliddraft_kernel_smlib::SmlibKernel;
use soliddraft_materials::{
    Appearance, AppearanceId, Catalog, Library, LibraryMatch, Material, MaterialId, Origin, sldmat,
};
use soliddraft_model::{AppearanceTarget, Command, Document, file};

pub(crate) const USAGE: &str = "\
usage: soliddraft material list [--category <name>] [--json]
       soliddraft material show <id> [--json]
       soliddraft material appearances [--json]
       soliddraft material libraries [--json]
       soliddraft material check <library.toml>... [--json]
       soliddraft material import <file.sldmat> [--into <library.toml>] [--json]
       soliddraft material info <part> [--config <name>] [--json]
       soliddraft material faces <part> [--json]
       soliddraft material set <part> <material-id | none> [--config <name>]
       soliddraft material paint <part> <appearance-id | none>
                                [--feature <F1 | name> | --face <n>] [--config <name>]
       soliddraft material update <part> [--json]

list         The materials in every library, by library and category: the built-in
             ones, My library and the libraries File locations lists.
show         One material or appearance, with every value and its source.
appearances  The appearances in every library.
libraries    The libraries loaded, where from, and any problems found loading them.
check        Reads library files and reports problems: bad values, ids used twice,
             default appearances no library has. Exits 1 when there are any.
import       Reads a SolidWorks .sldmat library into My library, or into the
             library file --into names (made when missing). Entries whose id is
             taken get a new one.
info         A part's material, appearances, mass, and copies that differ from
             their library or are lost.
faces        The faces of a part's body, numbered for `paint --face`.
set          Sets a part's material from the libraries, or removes it. The part
             keeps a copy, so it does not change when the library does.
paint        Applies an appearance to a whole part, to every face a feature made
             (--feature), or to one face (--face, numbered as `faces` lists them),
             or removes the appearance there with `none`.
update       Takes the library's current values for every material and
             appearance whose copy in the part differs from its library.

set, paint and update save the part in place; a .dwpart part is saved as .dwprt
beside it.

  --config  the configuration the assignment holds in (all of them by default)
  --json    print JSON
";

#[derive(Default)]
struct Args {
    words: Vec<String>,
    json: bool,
    category: Option<String>,
    into: Option<PathBuf>,
    config: Option<String>,
    feature: Option<String>,
    face: Option<usize>,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = |flag: &str| it.next().cloned().ok_or(format!("{flag} needs a value"));
        match arg.as_str() {
            "--json" => a.json = true,
            "--category" => a.category = Some(value("--category")?),
            "--into" => a.into = Some(PathBuf::from(value("--into")?)),
            "--config" => a.config = Some(value("--config")?),
            "--feature" => a.feature = Some(value("--feature")?),
            "--face" => {
                let v = value("--face")?;
                a.face = Some(v.parse().map_err(|_| format!("--face takes a face number, not {v}"))?);
            }
            s if s.starts_with('-') => return Err(format!("unknown option {s}")),
            s => a.words.push(s.to_owned()),
        }
    }
    Ok(a)
}

/// `soliddraft material <command> ...`.
pub(crate) fn run(args: &[String]) -> ExitCode {
    let a = match parse(args) {
        Ok(a) => a,
        Err(e) => {
            eprint!("{e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let words: Vec<&str> = a.words.iter().map(String::as_str).collect();
    let result = match words[..] {
        ["list"] => Ok(list(&catalog(), &a)),
        ["show", id] => show(&catalog(), id, a.json),
        ["appearances"] => Ok(appearances(&catalog(), a.json)),
        ["libraries"] => Ok(libraries(&catalog(), a.json)),
        ["check", ref files @ ..] if !files.is_empty() => Ok(check(files, a.json)),
        ["import", file] => import(Path::new(file), &a),
        ["info", part] => info(Path::new(part), &a),
        ["faces", part] => faces(Path::new(part), a.json),
        ["set", part, id] => set(Path::new(part), id, &a),
        ["paint", part, id] => paint(Path::new(part), id, &a),
        ["update", part] => update(Path::new(part), a.json),
        _ => {
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("soliddraft material: {e}");
            ExitCode::FAILURE
        }
    }
}

fn catalog() -> Catalog {
    soliddraft_app::settings::material_catalog()
}

fn library_json(l: &Library) -> Value {
    json!({
        "name": l.name,
        "origin": l.origin,
        "path": l.path,
        "read_only": l.is_read_only(),
        "materials": l.materials.len(),
        "appearances": l.appearances.len(),
    })
}

fn list(c: &Catalog, a: &Args) -> bool {
    let wanted = |m: &Material| a.category.as_ref().is_none_or(|cat| m.category.eq_ignore_ascii_case(cat));
    let rows: Vec<(&Library, &Material)> = c.materials().filter(|(_, m)| wanted(m)).collect();
    if a.json {
        let out: Vec<Value> =
            rows.iter().map(|(l, m)| json!({"library": l.name, "origin": l.origin, "material": m})).collect();
        println!("{}", Value::Array(out));
        return true;
    }
    let mut last: Option<(&str, &str)> = None;
    for (l, m) in rows {
        if last != Some((l.name.as_str(), m.category.as_str())) {
            let category = if m.category.is_empty() { "Uncategorised" } else { &m.category };
            println!("{} ({}) / {category}", l.name, l.origin.label());
            last = Some((&l.name, &m.category));
        }
        let density = m.density_kg_m3.map(|d| format!("{d} kg/m³")).unwrap_or_else(|| "density unknown".into());
        println!("  {:<26} {:<44} {density}", m.id.as_str(), m.name);
    }
    true
}

fn show(c: &Catalog, id: &str, json_out: bool) -> Result<bool, String> {
    if let Some((l, m)) = c.material(&MaterialId(id.into())) {
        if json_out {
            println!("{}", json!({"library": l.name, "material": m}));
        } else {
            println!("{} ({}), material in {}", m.name, m.id, l.name);
            print!("{}", fields(m));
        }
        return Ok(true);
    }
    if let Some((l, a)) = c.appearance(&AppearanceId(id.into())) {
        if json_out {
            println!("{}", json!({"library": l.name, "appearance": a}));
        } else {
            println!("{} ({}), appearance in {}", a.name, a.id, l.name);
            print!("{}", fields(a));
        }
        return Ok(true);
    }
    Err(format!("no library has a material or appearance {id}"))
}

/// An entry's values, one per line, without its id and name.
fn fields(v: &impl serde::Serialize) -> String {
    let Value::Object(map) = serde_json::to_value(v).expect("entries serialise") else { return String::new() };
    map.iter().filter(|(k, _)| *k != "id" && *k != "name").map(|(k, v)| format!("  {k} = {v}\n")).collect()
}

fn appearances(c: &Catalog, json_out: bool) -> bool {
    if json_out {
        let out: Vec<Value> = c.appearances().map(|(l, a)| json!({"library": l.name, "appearance": a})).collect();
        println!("{}", Value::Array(out));
        return true;
    }
    let mut last = None;
    for (l, a) in c.appearances() {
        if last != Some(l.name.as_str()) {
            println!("{} ({})", l.name, l.origin.label());
            last = Some(&l.name);
        }
        println!(
            "  {:<22} {:<28} {} metallic {} roughness {}{}",
            a.id.as_str(),
            a.name,
            a.base_color,
            a.metallic,
            a.roughness,
            if a.opacity < 1.0 { format!(" opacity {}", a.opacity) } else { String::new() }
        );
    }
    true
}

fn libraries(c: &Catalog, json_out: bool) -> bool {
    if json_out {
        let issues: Vec<String> = c.issues.iter().map(ToString::to_string).collect();
        let libs: Vec<Value> = c.libraries.iter().map(library_json).collect();
        println!("{}", json!({"libraries": libs, "issues": issues}));
        return true;
    }
    for l in &c.libraries {
        let place = l.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "compiled in".into());
        println!(
            "{} ({}): {} materials, {} appearances, {place}",
            l.name,
            l.origin.label(),
            l.materials.len(),
            l.appearances.len()
        );
    }
    if c.user_library().is_none()
        && let Some(p) = soliddraft_app::settings::user_library_path()
    {
        println!("My library: none yet; it will be {}", p.display());
    }
    for i in &c.issues {
        println!("problem: {i}");
    }
    true
}

fn check(files: &[&str], json_out: bool) -> bool {
    let mut libraries = Vec::new();
    let mut issues = Vec::new();
    for f in files {
        match Library::load(Path::new(f), Origin::File) {
            Ok(l) => libraries.push(l),
            Err(e) => issues.push(e.to_string()),
        }
    }
    // Check against the built-in libraries too, so a default appearance from them counts.
    let mut all = Catalog::built_in_libraries();
    all.extend(libraries);
    let c = Catalog::from_libraries(all, Vec::new());
    issues.extend(c.issues.iter().map(ToString::to_string));
    if json_out {
        println!("{}", json!({"problems": issues}));
    } else if issues.is_empty() {
        println!("no problems");
    } else {
        for i in &issues {
            println!("{i}");
        }
    }
    issues.is_empty()
}

fn import(file: &Path, a: &Args) -> Result<bool, String> {
    let bytes = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "import".into());
    let imported = sldmat::import(&bytes, &stem).map_err(|e| format!("{}: {e}", file.display()))?;
    let mut target = match &a.into {
        Some(p) if p.exists() => Library::load(p, Origin::File).map_err(|e| e.to_string())?,
        Some(p) => {
            let mut l =
                Library::new(p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or(stem), Origin::File);
            l.path = Some(p.clone());
            l
        }
        None => soliddraft_app::settings::user_library()?,
    };
    let c = catalog();
    let target_path = target.path.clone();
    // Ids the target's own file holds are handled by the merge; other libraries' ids
    // are taken.
    let elsewhere = |l: &Library| target_path.is_none() || l.path != target_path;
    let material_taken =
        |id: &MaterialId| c.libraries.iter().filter(|l| elsewhere(l)).any(|l| l.material(id).is_some());
    let appearance_taken =
        |id: &AppearanceId| c.libraries.iter().filter(|l| elsewhere(l)).any(|l| l.appearance(id).is_some());
    let (materials, appearances) = (imported.library.materials.len(), imported.library.appearances.len());
    let renamed = target.merge(imported.library, &material_taken, &appearance_taken);
    target.save().map_err(|e| e.to_string())?;
    let path = target.path.clone().unwrap_or_default();
    if a.json {
        println!(
            "{}",
            json!({
                "library": path,
                "materials": materials,
                "appearances": appearances,
                "renamed": renamed,
                "warnings": imported.warnings,
            })
        );
    } else {
        println!("imported {materials} materials and {appearances} appearances into {}", path.display());
        for (old, new) in &renamed {
            println!("  {old} is taken; imported as {new}");
        }
        for w in &imported.warnings {
            println!("  warning: {w}");
        }
    }
    Ok(true)
}

pub(crate) fn open(part: &Path) -> Result<Document, String> {
    let kernel: Arc<dyn Kernel> = Arc::new(SmlibKernel::new());
    Document::open(part, kernel).map_err(|e| format!("{}: {e}", part.display()))
}

/// Saves a part in place, or as .dwprt beside a .dwpart one. Returns where.
pub(crate) fn save(doc: &Document, part: &Path) -> Result<PathBuf, String> {
    let target = if part.extension().is_some_and(|e| e == file::LEGACY_EXTENSION) {
        part.with_extension(file::EXTENSION)
    } else {
        part.to_path_buf()
    };
    doc.save(&target).map_err(|e| format!("{}: {e}", target.display()))?;
    Ok(target)
}

fn scope(a: &Args) -> ConfigScope {
    match &a.config {
        Some(c) => ConfigScope::Only(vec![ConfigurationId(c.clone())]),
        None => ConfigScope::All,
    }
}

fn configuration(a: &Args) -> Option<ConfigurationId> {
    a.config.clone().map(ConfigurationId)
}

fn info(part: &Path, a: &Args) -> Result<bool, String> {
    let doc = open(part)?;
    let c = catalog();
    let config = configuration(a);
    let material = doc.material(config.as_ref());
    let mass = doc.mass(config.as_ref());
    let table = doc.appearance_table(config.as_ref());
    let state = doc.materials();
    let differs: Vec<Value> = state
        .snapshots
        .materials
        .iter()
        .filter_map(|m| match c.match_material(m) {
            LibraryMatch::Differs { library, .. } => Some(json!({"material": m.id, "library": library})),
            _ => None,
        })
        .chain(state.snapshots.appearances.iter().filter_map(|x| match c.match_appearance(x) {
            LibraryMatch::Differs { library, .. } => Some(json!({"appearance": x.id, "library": library})),
            _ => None,
        }))
        .collect();
    if a.json {
        let lost: Vec<Value> = table
            .lost
            .iter()
            .map(|l| json!({"assignment": state.appearances[l.index], "message": l.message}))
            .collect();
        let report = json!({
            "part": doc.name(),
            "material": material,
            "mass": mass.as_ref().ok(),
            "mass_unknown": mass.as_ref().err().map(ToString::to_string),
            "appearance": table.appearances[0],
            "assignments": state.appearances,
            "lost": lost,
            "differs_from_library": differs,
        });
        println!("{report}");
        return Ok(true);
    }
    println!("{} ({})", doc.name(), part.display());
    match material {
        Some(m) => println!("  material: {} ({})", m.name, m.id),
        None => println!("  material: not set"),
    }
    match &mass {
        Ok(m) => println!(
            "  mass: {:.6} kg, volume {:.3} mm³, centre of mass ({:.3}, {:.3}, {:.3}) mm",
            m.mass_kg, m.volume_mm3, m.centre_mm[0], m.centre_mm[1], m.centre_mm[2]
        ),
        Err(e) => println!("  mass: unknown ({e})"),
    }
    if mass.as_ref().is_ok_and(|m| m.inertia_kg_mm2.is_none()) {
        println!("  inertia: not available yet from Catallax");
    }
    println!("  appearance: {} ({})", table.appearances[0].name, table.appearances[0].id);
    for x in &state.appearances {
        let place = match &x.target {
            AppearanceTarget::Part => "part".to_owned(),
            AppearanceTarget::Feature { feature } => {
                format!("feature {}", doc.feature(*feature).map(|f| f.name.as_str()).unwrap_or("(deleted)"))
            }
            AppearanceTarget::Face { face } => format!("face {}", face.label),
        };
        let scope = match &x.scope {
            ConfigScope::All => String::new(),
            ConfigScope::Only(c) => {
                format!(" in {}", c.iter().map(|c| c.0.as_str()).collect::<Vec<_>>().join(", "))
            }
        };
        println!("  {place}: {}{scope}", x.appearance);
    }
    for l in &table.lost {
        println!("  lost: {}", l.message);
    }
    for d in &differs {
        let id = d.get("material").or_else(|| d.get("appearance")).and_then(Value::as_str).unwrap_or_default();
        println!("  {id} differs from {}; `soliddraft material update` takes the library's values", d["library"]);
    }
    Ok(true)
}

/// The final body's faces, in topology order, with their labels.
fn body_faces(doc: &Document) -> Result<Vec<(FaceId, String)>, String> {
    let body = doc.body().ok_or("the part has no body")?;
    Ok(doc
        .face_labels(body)
        .into_iter()
        .map(|(f, l)| (f, l.map(|l| l.text).unwrap_or_else(|| "(no feature made it)".into())))
        .collect())
}

fn faces(part: &Path, json_out: bool) -> Result<bool, String> {
    let doc = open(part)?;
    let list = body_faces(&doc)?;
    let table = doc.appearance_table(None);
    if json_out {
        let out: Vec<Value> = list
            .iter()
            .enumerate()
            .map(|(i, (f, label))| json!({"face": i + 1, "label": label, "appearance": table.of(*f).id}))
            .collect();
        println!("{}", Value::Array(out));
    } else {
        for (i, (f, label)) in list.iter().enumerate() {
            println!("  {:>3}  {label:<44} {}", i + 1, table.of(*f).id);
        }
    }
    Ok(true)
}

fn set(part: &Path, id: &str, a: &Args) -> Result<bool, String> {
    let mut doc = open(part)?;
    let command = if id == "none" {
        Command::SetMaterial { material: None, default_appearance: None, scope: scope(a) }
    } else {
        let c = catalog();
        let Some(Command::SetMaterial { material, default_appearance, .. }) =
            Command::set_material_from(&c, &MaterialId(id.into()))
        else {
            return Err(format!("no library has the material {id}"));
        };
        Command::SetMaterial { material, default_appearance, scope: scope(a) }
    };
    let done = doc.execute(command).map_err(|e| e.to_string())?;
    let saved = save(&doc, part)?;
    println!("{}: {}", saved.display(), done.label);
    Ok(true)
}

fn paint(part: &Path, id: &str, a: &Args) -> Result<bool, String> {
    let mut doc = open(part)?;
    let target = match (&a.feature, a.face) {
        (Some(_), Some(_)) => return Err("give --feature or --face, not both".into()),
        (Some(f), None) => {
            let found = doc.features().iter().find(|x| x.id.to_string().eq_ignore_ascii_case(f) || x.name == *f);
            let feature = found.ok_or_else(|| format!("the part has no feature {f}"))?.id;
            AppearanceTarget::Feature { feature }
        }
        (None, Some(n)) => {
            let list = body_faces(&doc)?;
            let (face, _) = n
                .checked_sub(1)
                .and_then(|i| list.get(i))
                .ok_or_else(|| format!("the body has faces 1 to {}", list.len()))?;
            AppearanceTarget::Face { face: doc.face_ref(*face).map_err(|e| e.to_string())? }
        }
        (None, None) => AppearanceTarget::Part,
    };
    let appearance: Option<Appearance> = if id == "none" {
        None
    } else {
        let c = catalog();
        Some(
            c.appearance(&AppearanceId(id.into()))
                .ok_or_else(|| format!("no library has the appearance {id}"))?
                .1
                .clone(),
        )
    };
    let done =
        doc.execute(Command::SetAppearance { target, appearance, scope: scope(a) }).map_err(|e| e.to_string())?;
    let saved = save(&doc, part)?;
    println!("{}: {}", saved.display(), done.label);
    Ok(true)
}

fn update(part: &Path, json_out: bool) -> Result<bool, String> {
    let mut doc = open(part)?;
    let c = catalog();
    let state = doc.materials();
    let materials: Vec<Material> = state
        .snapshots
        .materials
        .iter()
        .filter_map(|m| match c.match_material(m) {
            LibraryMatch::Differs { current, .. } => Some(current),
            _ => None,
        })
        .collect();
    let appearances: Vec<Appearance> = state
        .snapshots
        .appearances
        .iter()
        .filter_map(|x| match c.match_appearance(x) {
            LibraryMatch::Differs { current, .. } => Some(current),
            _ => None,
        })
        .collect();
    let ids: Vec<String> =
        materials.iter().map(|m| m.id.0.clone()).chain(appearances.iter().map(|x| x.id.0.clone())).collect();
    if ids.is_empty() {
        if json_out {
            println!("{}", json!({"updated": ids}));
        } else {
            println!("every copy matches its library");
        }
        return Ok(true);
    }
    doc.execute(Command::UpdateCopies { materials, appearances }).map_err(|e| e.to_string())?;
    let saved = save(&doc, part)?;
    if json_out {
        println!("{}", json!({"updated": ids, "file": saved}));
    } else {
        println!("{}: updated {}", saved.display(), ids.join(", "));
    }
    Ok(true)
}
