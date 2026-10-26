//! `soliddraft draw`: a drawing of a part or a recipe's result without a window, and its
//! Rebuild when the part changes.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::Serialize;
use soliddraft_app::drawing::part::PartContext;
use soliddraft_document::reference::relative_path;
use soliddraft_document::resolve::{Missing, StoredPathProblem, rebind};
use soliddraft_document::{ContentHash, DocKind, DocRef, DocumentId, Resolution, Resolver};
use soliddraft_drawing::export::{self, Format, PdfInfo};
use soliddraft_drawing::file::{self, FileMeta};
use soliddraft_drawing::{
    AnnotationId, AnnotationKind, Boundary, BrokenOut, Command, DatumRef, Drawing, DrawingMeta, GdtCharacteristic,
    HiddenLines, LinearOrientation, MaterialCondition, ModelDimension, ModelMeasure, NewDrawing, Part, PartIndex,
    RevisionRow, SetKind, SurfaceProcess, TangentEdges, Template, UndoStack, UpdateReport, ViewId, annotate, derived,
    display, project,
};
use soliddraft_kernel_api::drawing::StandardView;
use soliddraft_kernel_api::{BooleanOp, Kernel, Point2, Vec3};
use soliddraft_kernel_catallax::CatallaxKernel;
use soliddraft_recipe::{Command as Step, Recipe, Shape};

pub(crate) const USAGE: &str = "\
usage: soliddraft draw <part.dwprt | recipe.json> [--template <id>] [--views <view>,...] [--dims model|none]
                      [--hidden removed|visible] [--title <text>] [--number <text>]
                      [--designer <name>] [--date <yyyy-mm-dd>] [--out <file>]... [--json]
                      [derived views, view edits and annotations, below]
       soliddraft draw --update <drawing.dwdrw> [--accept] [--delete <id>]...
                      [--keep-as-note <id>]... [--reattach <id>:<end>:<x,y,z>]...
                      [--search <folder>]... [--out <file>]... [--json]

Makes a drawing of a part file (.dwprt, or .dwpart from before) or of a recipe's
result on the Catallax kernel: the views projected with hidden lines, scaled and
placed on the template's sheet, and the part's feature dimensions brought in
(Model items). Writes the drawing file and its SVG, PDF and DXF. The drawing
references the part by id and pins the part's revision.

  --template   iso-a3 (default, first angle), iso-a4, ansi-a or ansi-b (third angle)
  --views      front, back, top, bottom, left, right, iso; the first orthographic
               view is the base view and the others fold off it (default: front, top,
               the side view right of the front view, and iso)
  --dims       model brings in the part's dimensions (default); none leaves them out
  --hidden     hidden lines in orthographic views (default removed)
  --title      title block title (default: the part's name, capitalised)
  --date       title block date (default: SOURCE_DATE_EPOCH, else today)
  --out        a file to write, by extension: .dwdrw, .svg, .pdf or .dxf;
               without --out, all four go to the current directory, named after
               the part
  --json       print a JSON report

Derived views, each placed beside its parent or at a free place; add @<n> to put
it on sheet n, which is made from the same template if it does not exist yet:

  --section <view>:<x,y,z>:h|v[:flip]       a section along a horizontal or
                                             vertical cutting line through the model
                                             point; flip looks the other way
  --detail <view>:<x,y,z>:<radius>:<factor> a detail of the circle round the model
                                             point, radius in model mm
  --aux <view>:<x,y,z>[:flip]               an auxiliary view square to the edge
                                             nearest the model point

<view> is front, back, top, bottom, left, right or iso, a section or detail
letter, or a view id. View edits:

  --crop <view>:<x,y,z>:<x,y,z>             keep the rectangle between two model points
  --broken-out <view>:<x,y,z>:<radius>:<depth>
                                             cut away a circle to a depth (model mm)
  --display <view>:hidden=removed|visible   hidden lines
  --display <view>:tangent=removed|phantom|visible
                                             tangent edges

Annotations, attached to the edges nearest model points:

  --ordinate <view>:h|v:<x,y,z>;<x,y,z>;...  ordinate dimensions from the first point
  --baseline <view>:h|v:<x,y,z>;<x,y,z>;...  baseline dimensions from the first point
  --chain <view>:h|v:<x,y,z>;<x,y,z>;...     chain dimensions point to point; h-15 or
                                             v+20 puts the values that far beyond
                                             the points (default 12 mm, above or right)
  --gdt <view>:<x,y,z>:<characteristic>:<tolerance>[:<datums>]
                                             a feature control frame: characteristic
                                             as position, flatness, perpendicularity,
                                             ...; tolerance as 0.05, dia0.1 or
                                             dia0.1(M); datums as A,B(M),C
  --datum <view>:<x,y,z>[:<letter>]         a datum feature symbol
  --finish <view>:<x,y,z>:<requirement>[:any|removal|no-removal]
                                             a surface finish symbol, such as Ra 1.6
  --revision <rev>:<description>[:<date>[:<approved>]]
                                             a row of the revision table
  --revision-symbol <view>:<x,y,z>:<rev>    a revision symbol

Add @dx,dy to --gdt, --datum, --finish and --revision-symbol to place the symbol
that far from its anchor, in sheet millimetres.

With several sheets, SVG and DXF files are written one per sheet: the second
sheet of bracket.svg is bracket-2.svg. The PDF has one page per sheet.

--update is the drawing's Rebuild. It finds the drawing's part (by its stored
path, else by id in the drawing's folder, its subfolders and the File locations
search folders), regenerates the views and re-resolves the dimensions against the
part as it is now, pins the drawing at that revision and writes the drawing back.
A drawing is otherwise left at the revision it was pinned at, whatever the part
became. It reports values that changed and dimensions that lost their
references; nothing is deleted unless asked:

  --accept        accept every changed value
  --delete        delete an annotation (repeatable)
  --keep-as-note  replace an annotation by a note with its text (repeatable)
  --reattach      attach end <end> of annotation <id> to the edge nearest the
                  model point x,y,z (repeatable)
  --kernel-log    write Catallax per-call evidence to this JSON file
  --search        another folder to look for the part in (repeatable)
";

struct Options {
    source: Option<String>,
    update: Option<String>,
    template: String,
    views: Option<Vec<StandardView>>,
    model_items: bool,
    hidden: HiddenLines,
    title: Option<String>,
    number: String,
    designer: String,
    date: Option<String>,
    outs: Vec<String>,
    json: bool,
    kernel_log: Option<PathBuf>,
    accept: bool,
    delete: Vec<u32>,
    freeze: Vec<u32>,
    reattach: Vec<(u32, usize, Vec3)>,
    /// Folders to look for a moved part in, beside File locations.
    search: Vec<PathBuf>,
    /// Derived views, view edits and annotations, in the order given.
    steps: Vec<Step2>,
}

/// One derived view, view edit or annotation asked for on the command line.
#[derive(Clone, Debug)]
enum Step2 {
    Section { parent: String, through: Vec3, horizontal: bool, flip: bool, sheet: Option<usize> },
    Detail { parent: String, center: Vec3, radius: f64, factor: f64, sheet: Option<usize> },
    Aux { parent: String, near: Vec3, flip: bool, sheet: Option<usize> },
    Crop { view: String, a: Vec3, b: Vec3 },
    BrokenOut { view: String, at: Vec3, radius: f64, depth: f64 },
    Display { view: String, key: String, value: String },
    Set { kind: SetKind, view: String, orientation: LinearOrientation, offset: f64, points: Vec<Vec3> },
    Gdt { view: String, at: Vec3, body: AnnotationKind, offset: Option<Point2> },
    Datum { view: String, at: Vec3, letter: Option<String>, offset: Option<Point2> },
    Finish { view: String, at: Vec3, text: String, process: SurfaceProcess, offset: Option<Point2> },
    Revision(RevisionRow),
    RevisionSymbol { view: String, at: Vec3, revision: String, offset: Option<Point2> },
}

fn point(s: &str) -> Result<Vec3, String> {
    let v: Vec<f64> =
        s.split(',').map(|c| c.trim().parse().map_err(|_| format!("bad point {s}"))).collect::<Result<_, _>>()?;
    match v[..] {
        [x, y, z] => Ok(Vec3::new(x, y, z)),
        _ => Err(format!("a point is x,y,z, not {s}")),
    }
}

fn number(s: &str, what: &str) -> Result<f64, String> {
    s.trim().parse().map_err(|_| format!("{what} needs a number, not {s}"))
}

/// Splits `value@n` into the value and sheet n (counted from 1).
fn on_sheet(value: &str) -> Result<(&str, Option<usize>), String> {
    match value.rsplit_once('@') {
        Some((v, n)) => {
            let n: usize = n.parse().map_err(|_| format!("bad sheet number in {value}"))?;
            if n == 0 {
                return Err("sheets are counted from 1".into());
            }
            Ok((v, Some(n - 1)))
        }
        None => Ok((value, None)),
    }
}

/// "h", "h-15" or "v+20": a direction and how far beyond the points the values go.
fn direction_and_offset(s: &str) -> Result<(LinearOrientation, f64), String> {
    let (dir, rest) = s.split_at(s.len().min(1));
    let offset = if rest.is_empty() { 12.0 } else { number(rest, "a dimension offset")? };
    Ok((orientation(dir)?, offset))
}

/// Splits `value@dx,dy` into the value and an offset in sheet millimetres.
fn with_offset(value: &str) -> Result<(&str, Option<Point2>), String> {
    match value.rsplit_once('@') {
        Some((v, o)) => {
            let [dx, dy] = o.split(',').collect::<Vec<_>>()[..] else {
                return Err(format!("an offset is @dx,dy, not @{o}"));
            };
            Ok((v, Some(Point2::new(number(dx, "an offset")?, number(dy, "an offset")?))))
        }
        None => Ok((value, None)),
    }
}

fn orientation(s: &str) -> Result<LinearOrientation, String> {
    match s {
        "h" => Ok(LinearOrientation::Horizontal),
        "v" => Ok(LinearOrientation::Vertical),
        other => Err(format!("a direction is h or v, not {other}")),
    }
}

/// "dia0.1(M)" as (value, diameter, material).
fn tolerance(s: &str) -> Result<(f64, bool, MaterialCondition), String> {
    let (s, diameter) = match s.strip_prefix("dia") {
        Some(rest) => (rest, true),
        None => (s, false),
    };
    let (s, material) = modifier(s);
    Ok((number(s, "a tolerance")?, diameter, material))
}

fn modifier(s: &str) -> (&str, MaterialCondition) {
    if let Some(v) = s.strip_suffix("(M)") {
        (v, MaterialCondition::Maximum)
    } else if let Some(v) = s.strip_suffix("(L)") {
        (v, MaterialCondition::Least)
    } else {
        (s, MaterialCondition::Regardless)
    }
}

fn parse_step(flag: &str, value: &str) -> Result<Step2, String> {
    let bad = |what: &str| format!("{flag} needs {what}, not {value}");
    match flag {
        "--section" => {
            let (v, sheet) = on_sheet(value)?;
            let parts: Vec<&str> = v.split(':').collect();
            let (parent, through, dir, flip) = match parts[..] {
                [p, x, d] => (p, x, d, false),
                [p, x, d, "flip"] => (p, x, d, true),
                _ => return Err(bad("<view>:<x,y,z>:h|v[:flip]")),
            };
            let horizontal = orientation(dir)? == LinearOrientation::Horizontal;
            Ok(Step2::Section { parent: parent.into(), through: point(through)?, horizontal, flip, sheet })
        }
        "--detail" => {
            let (v, sheet) = on_sheet(value)?;
            let [parent, c, r, f] = v.split(':').collect::<Vec<_>>()[..] else {
                return Err(bad("<view>:<x,y,z>:<radius>:<factor>"));
            };
            Ok(Step2::Detail {
                parent: parent.into(),
                center: point(c)?,
                radius: number(r, "a detail radius")?,
                factor: number(f, "a detail factor")?,
                sheet,
            })
        }
        "--aux" => {
            let (v, sheet) = on_sheet(value)?;
            let parts: Vec<&str> = v.split(':').collect();
            let (parent, near, flip) = match parts[..] {
                [p, x] => (p, x, false),
                [p, x, "flip"] => (p, x, true),
                _ => return Err(bad("<view>:<x,y,z>[:flip]")),
            };
            Ok(Step2::Aux { parent: parent.into(), near: point(near)?, flip, sheet })
        }
        "--crop" => {
            let [view, a, b] = value.split(':').collect::<Vec<_>>()[..] else {
                return Err(bad("<view>:<x,y,z>:<x,y,z>"));
            };
            Ok(Step2::Crop { view: view.into(), a: point(a)?, b: point(b)? })
        }
        "--broken-out" => {
            let [view, at, r, depth] = value.split(':').collect::<Vec<_>>()[..] else {
                return Err(bad("<view>:<x,y,z>:<radius>:<depth>"));
            };
            Ok(Step2::BrokenOut {
                view: view.into(),
                at: point(at)?,
                radius: number(r, "a radius")?,
                depth: number(depth, "a depth")?,
            })
        }
        "--display" => {
            let Some((view, setting)) = value.split_once(':') else { return Err(bad("<view>:<key>=<value>")) };
            let Some((key, v)) = setting.split_once('=') else { return Err(bad("<view>:<key>=<value>")) };
            Ok(Step2::Display { view: view.into(), key: key.into(), value: v.into() })
        }
        "--ordinate" | "--baseline" | "--chain" => {
            let kind = match flag {
                "--ordinate" => SetKind::Ordinate,
                "--baseline" => SetKind::Baseline,
                _ => SetKind::Chain,
            };
            let [view, dir, pts] = value.splitn(3, ':').collect::<Vec<_>>()[..] else {
                return Err(bad("<view>:h|v[<offset>]:<x,y,z>;<x,y,z>;..."));
            };
            let points = pts.split(';').map(point).collect::<Result<Vec<_>, _>>()?;
            let (orientation, offset) = direction_and_offset(dir)?;
            Ok(Step2::Set { kind, view: view.into(), orientation, offset, points })
        }
        "--gdt" => {
            let (value, offset) = with_offset(value)?;
            let parts: Vec<&str> = value.split(':').collect();
            let (view, at, ch, tol, datums) = match parts[..] {
                [v, a, c, t] => (v, a, c, t, ""),
                [v, a, c, t, d] => (v, a, c, t, d),
                _ => return Err(bad("<view>:<x,y,z>:<characteristic>:<tolerance>[:<datums>]")),
            };
            let characteristic =
                GdtCharacteristic::from_key(ch).ok_or(format!("unknown geometric characteristic {ch}"))?;
            let (tolerance, diameter, material) = tolerance(tol)?;
            let datums = datums
                .split(',')
                .filter(|d| !d.is_empty())
                .map(|d| {
                    let (letter, material) = modifier(d);
                    DatumRef { letter: letter.into(), material }
                })
                .collect();
            let body = AnnotationKind::Gdt { characteristic, tolerance, diameter, material, datums };
            Ok(Step2::Gdt { view: view.into(), at: point(at)?, body, offset })
        }
        "--datum" => {
            let (value, offset) = with_offset(value)?;
            let parts: Vec<&str> = value.split(':').collect();
            let (view, at, letter) = match parts[..] {
                [v, a] => (v, a, None),
                [v, a, l] => (v, a, Some(l.to_owned())),
                _ => return Err(bad("<view>:<x,y,z>[:<letter>]")),
            };
            Ok(Step2::Datum { view: view.into(), at: point(at)?, letter, offset })
        }
        "--finish" => {
            let (value, offset) = with_offset(value)?;
            let parts: Vec<&str> = value.split(':').collect();
            let (view, at, text, process) = match parts[..] {
                [v, a, t] => (v, a, t, SurfaceProcess::Any),
                [v, a, t, p] => (
                    v,
                    a,
                    t,
                    match p {
                        "any" => SurfaceProcess::Any,
                        "removal" => SurfaceProcess::Removal,
                        "no-removal" => SurfaceProcess::NoRemoval,
                        other => return Err(format!("a surface process is any, removal or no-removal, not {other}")),
                    },
                ),
                _ => return Err(bad("<view>:<x,y,z>:<requirement>[:any|removal|no-removal]")),
            };
            Ok(Step2::Finish { view: view.into(), at: point(at)?, text: text.into(), process, offset })
        }
        "--revision" => {
            let parts: Vec<&str> = value.splitn(4, ':').collect();
            let field = |i: usize| parts.get(i).map(|s| s.to_string()).unwrap_or_default();
            if parts.len() < 2 {
                return Err(bad("<rev>:<description>[:<date>[:<approved>]]"));
            }
            Ok(Step2::Revision(RevisionRow {
                revision: field(0),
                description: field(1),
                date: field(2),
                approved: field(3),
                zone: String::new(),
            }))
        }
        "--revision-symbol" => {
            let (value, offset) = with_offset(value)?;
            let [view, at, rev] = value.split(':').collect::<Vec<_>>()[..] else {
                return Err(bad("<view>:<x,y,z>:<rev>"));
            };
            Ok(Step2::RevisionSymbol { view: view.into(), at: point(at)?, revision: rev.into(), offset })
        }
        _ => Err(format!("unknown option {flag}")),
    }
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut o = Options {
        source: None,
        update: None,
        template: "iso-a3".into(),
        views: None,
        model_items: true,
        hidden: HiddenLines::Removed,
        title: None,
        number: String::new(),
        designer: String::new(),
        date: None,
        outs: Vec::new(),
        json: false,
        kernel_log: None,
        accept: false,
        delete: Vec::new(),
        freeze: Vec::new(),
        reattach: Vec::new(),
        search: Vec::new(),
        steps: Vec::new(),
    };
    let mut it = args.iter();
    let id = |s: String, name: &str| s.parse::<u32>().map_err(|_| format!("{name} needs an annotation id"));
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().cloned().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "--update" => o.update = Some(value("--update")?),
            "--template" => o.template = value("--template")?,
            "--views" => {
                let names = value("--views")?;
                let views = names
                    .split(',')
                    .map(|n| StandardView::from_name(n.trim()).ok_or(format!("unknown view {n}")))
                    .collect::<Result<Vec<_>, _>>()?;
                o.views = Some(views);
            }
            "--dims" => {
                o.model_items = match value("--dims")?.as_str() {
                    "model" => true,
                    "none" => false,
                    other => return Err(format!("--dims is model or none, not {other}")),
                }
            }
            "--hidden" => {
                o.hidden = match value("--hidden")?.as_str() {
                    "removed" => HiddenLines::Removed,
                    "visible" => HiddenLines::Visible,
                    other => return Err(format!("--hidden is removed or visible, not {other}")),
                }
            }
            "--title" => o.title = Some(value("--title")?),
            "--number" => o.number = value("--number")?,
            "--designer" => o.designer = value("--designer")?,
            "--date" => o.date = Some(value("--date")?),
            "--out" => o.outs.push(value("--out")?),
            "--search" => o.search.push(PathBuf::from(value("--search")?)),
            "--json" => o.json = true,
            "--kernel-log" => o.kernel_log = Some(PathBuf::from(value("--kernel-log")?)),
            "--accept" => o.accept = true,
            "--delete" => o.delete.push(id(value("--delete")?, "--delete")?),
            "--keep-as-note" => o.freeze.push(id(value("--keep-as-note")?, "--keep-as-note")?),
            "--reattach" => {
                let v = value("--reattach")?;
                let parts: Vec<&str> = v.splitn(3, ':').collect();
                let [a, e, p] = parts[..] else { return Err("--reattach needs <id>:<end>:<x,y,z>".into()) };
                let point: Vec<f64> =
                    p.split(',').map(|c| c.trim().parse().map_err(|_| "bad point")).collect::<Result<_, _>>()?;
                let [x, y, z] = point[..] else { return Err("--reattach needs a point x,y,z".into()) };
                let end = e.parse().map_err(|_| "--reattach needs an end index")?;
                o.reattach.push((id(a.to_owned(), "--reattach")?, end, Vec3::new(x, y, z)));
            }
            flag @ ("--section" | "--detail" | "--aux" | "--crop" | "--broken-out" | "--display" | "--ordinate"
            | "--baseline" | "--chain" | "--gdt" | "--datum" | "--finish" | "--revision"
            | "--revision-symbol") => {
                let v = value(flag)?;
                o.steps.push(parse_step(flag, &v)?);
            }
            s if s.starts_with('-') => return Err(format!("unknown option {s}")),
            s if o.source.is_none() => o.source = Some(s.to_owned()),
            s => return Err(format!("unexpected argument {s}")),
        }
    }
    if o.update.is_some() && !o.steps.is_empty() {
        return Err("derived views and annotations are for a new drawing, not --update".into());
    }
    match (&o.source, &o.update) {
        (None, None) => Err("no part or recipe given".into()),
        (Some(_), Some(_)) => Err("give a part, a recipe or --update <drawing>, not one and the other".into()),
        _ => Ok(o),
    }
}

pub(crate) fn run(args: &[String]) -> ExitCode {
    let o = match parse(args) {
        Ok(o) => o,
        Err(e) => {
            eprint!("{e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let result = if o.update.is_some() { update(&o) } else { create(&o) };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("soliddraft draw: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The JSON report of `draw`.
#[derive(Serialize)]
struct CreateReport<'a> {
    backend: &'a str,
    drawing: &'a str,
    template: &'a str,
    scale: String,
    views: usize,
    sheets: usize,
    dimensions_placed: usize,
    dimensions_skipped: &'a [String],
    annotations: usize,
    location_labels: Vec<LocationOut>,
    written: &'a [String],
}

/// A location label pair in the JSON report.
#[derive(Serialize)]
struct LocationOut {
    view: u32,
    parent: u32,
    on_parent: Option<String>,
    on_view: Option<String>,
}

fn create(o: &Options) -> Result<(), String> {
    let source_path = PathBuf::from(o.source.as_deref().expect("parse checks"));
    let template = Template::by_id(&o.template).ok_or(format!("unknown template {}", o.template))?;
    with_part(&source_path, o.kernel_log.as_deref(), |part, source| {
        let stem = source_path.file_stem().and_then(|s| s.to_str()).unwrap_or("drawing").to_owned();
        let outs = if o.outs.is_empty() {
            [file::EXTENSION, "svg", "pdf", "dxf"].iter().map(|e| format!("{stem}.{e}")).collect()
        } else {
            o.outs.clone()
        };
        let drawing_path = outs
            .iter()
            .find(|p| DocKind::of_path(Path::new(p)) == Some(DocKind::Drawing))
            .map(|p| soliddraft_document::kind::saved_path(Path::new(p), DocKind::Drawing));
        let base = drawing_path.as_deref().and_then(Path::parent).unwrap_or(Path::new(""));
        let path = relative_path(base, &source_path);
        let model = match source.id {
            Some(id) => DocRef::new(id, DocKind::Part, path, source.revision.clone()),
            None => DocRef::unbound(DocKind::Part, path, source.revision.clone()),
        };
        let meta = DrawingMeta {
            title: o.title.clone().unwrap_or_else(|| capitalise(&source.name)),
            number: o.number.clone(),
            revision: "A".into(),
            designer: o.designer.clone(),
            date: o.date.clone().unwrap_or_else(today),
            units: Default::default(),
        };
        let new = NewDrawing {
            views: o.views.clone().unwrap_or_else(|| soliddraft_drawing::default_views(&template)),
            template: template.clone(),
            model,
            meta,
            hidden: o.hidden,
            model_items: o.model_items,
            centre_marks: false,
            scale: None,
        };
        let made = soliddraft_drawing::make_drawing(part, &new).map_err(|e| e.to_string())?;
        let (mut drawing, import) = (made.drawing, made.import);
        apply_steps(&mut drawing, part, &o.steps)?;
        write_outputs(&drawing, &outs, &source.backend)?;
        let name = drawing_path.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
        if o.json {
            let report = CreateReport {
                backend: &source.backend,
                drawing: &name,
                template: &o.template,
                scale: drawing.sheets[0].scale.to_string(),
                views: drawing.sheets.iter().map(|s| s.views.len()).sum(),
                sheets: drawing.sheets.len(),
                dimensions_placed: import.placed,
                dimensions_skipped: &import.skipped,
                annotations: drawing.sheets.iter().map(|s| s.annotations.len()).sum(),
                location_labels: display::location_labels(&drawing)
                    .into_iter()
                    .map(|l| LocationOut {
                        view: l.view.0,
                        parent: l.parent.0,
                        on_parent: l.on_parent(),
                        on_view: l.on_view(),
                    })
                    .collect(),
                written: &outs,
            };
            println!("{}", serde_json::to_string(&report).expect("reports serialise"));
        } else {
            let views: usize = drawing.sheets.iter().map(|s| s.views.len()).sum();
            let sheets =
                if drawing.sheets.len() > 1 { format!(" on {} sheets", drawing.sheets.len()) } else { String::new() };
            println!(
                "{}: {views} views{sheets} at {}, {} dimensions; wrote {}",
                source.name,
                drawing.sheets[0].scale,
                import.placed,
                outs.join(", ")
            );
            for s in &import.skipped {
                println!("  not dimensioned: {s}");
            }
        }
        Ok(())
    })
}

/// What a drawing is made of, as the drawing records it.
pub(crate) struct Source {
    /// The part's id; a recipe has none.
    pub id: Option<DocumentId>,
    /// The part's revision: its semantic hash, or a recipe's canonical hash.
    pub revision: ContentHash,
    /// The part's name.
    pub name: String,
    /// The kernel backend, as the drawing file records it.
    pub backend: String,
}

/// Opens the part file at `path` (or replays the recipe there) on the Catallax kernel and
/// runs `f` on the part.
fn with_part<R>(
    path: &Path,
    kernel_log: Option<&Path>,
    f: impl FnOnce(&Part<'_>, &Source) -> Result<R, String>,
) -> Result<R, String> {
    let kernel = std::sync::Arc::new(if kernel_log.is_some() {
        CatallaxKernel::with_operation_log()
    } else {
        CatallaxKernel::new()
    });
    let result = (|| match DocKind::of_path(path) {
        Some(DocKind::Part) => {
            let doc = soliddraft_app::file::open(path, kernel.clone())?;
            let ctx = PartContext::new(&doc).ok_or_else(|| format!("{}: the part has no body", path.display()))?;
            let backend = doc.kernel().backend();
            let source = Source {
                id: Some(ctx.id),
                revision: ctx.digest.clone(),
                name: doc.name.clone(),
                backend: format!("{} {}", backend.name, backend.version),
            };
            f(&ctx.part(), &source)
        }
        Some(kind) => Err(format!("{}: a drawing is made of a part, not a {kind}", path.display())),
        None => {
            let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let recipe = soliddraft_recipe::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
            let value: serde_json::Value =
                serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
            let revision = ContentHash::of_json(&value);
            let (part, backend) = build(&kernel, &recipe, path, revision.clone())?;
            let source = Source { id: None, revision, name: recipe.name.clone(), backend };
            let out = f(&part, &source);
            let _ = kernel.release(part.body);
            out
        }
    })();
    if let Some(path) = kernel_log {
        let log = kernel.take_diagnostics().map_err(|e| e.to_string())?;
        let text = serde_json::to_string(&log).expect("diagnostics serialize");
        std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    result
}

/// The JSON report of `draw --update`.
#[derive(Serialize)]
struct UpdateOut<'a> {
    drawing: &'a str,
    /// The part's path as the drawing stores it now.
    model: &'a str,
    #[serde(flatten)]
    update: &'a UpdateReport,
    repairs: Vec<String>,
}

/// The search folders: `--search` and File locations' referenced-documents folders.
pub(crate) fn search_folders(extra: &[PathBuf]) -> Vec<PathBuf> {
    let mut folders = extra.to_vec();
    folders.extend(soliddraft_app::settings::file_locations().search_folders().iter().cloned());
    folders
}

/// Why a reference is missing, with the candidates, in words.
pub(crate) fn missing_text(r: &DocRef, m: &Missing) -> String {
    let why = match &m.stored {
        StoredPathProblem::NotFound => "no file there".to_owned(),
        StoredPathProblem::DifferentDocument(id) => format!("a different document is there ({})", id.short()),
        StoredPathProblem::WrongKind(k) => format!("a {k} is there"),
        StoredPathProblem::Unreadable(e) => e.clone(),
    };
    let mut text = format!("the {} {} is missing: {why}", r.kind, r.path);
    if !m.candidates.is_empty() {
        let names: Vec<String> = m.candidates.iter().map(|c| c.path.display().to_string()).collect();
        text += &format!("; candidates: {}", names.join(", "));
    }
    text
}

fn update(o: &Options) -> Result<(), String> {
    let path = PathBuf::from(o.update.as_deref().expect("parse checks"));
    let (mut drawing, meta) = file::read(&path).map_err(|e| e.to_string())?;
    let resolver = Resolver::new(Some(&path)).with_search_folders(search_folders(&o.search));
    let target = match resolver.resolve(&drawing.model) {
        Resolution::Found(t) => t,
        Resolution::Missing(m) => {
            return Err(format!(
                "{}; repair it with `soliddraft doc repair {} --to <part>`",
                missing_text(&drawing.model, &m),
                path.display()
            ));
        }
    };
    let source_path = target.path().expect("a file was found").to_path_buf();
    with_part(&source_path, o.kernel_log.as_deref(), |part, source| {
        let mut stack = soliddraft_drawing::UndoStack::new();
        let report = stack
            .run(&mut drawing, &Command::UpdateFromModel, Some(part))
            .map_err(|e| e.to_string())?
            .expect("an update reports");
        rebind(&mut drawing.model, &target);
        let mut repairs = Vec::new();
        let run = |stack: &mut soliddraft_drawing::UndoStack, drawing: &mut Drawing, c: Command| {
            stack.run(drawing, &c, Some(part)).map(|_| ()).map_err(|e| e.to_string())
        };
        if o.accept {
            for c in &report.changed {
                run(&mut stack, &mut drawing, Command::AcceptChange { annotation: c.annotation })?;
                repairs.push(format!("accepted {}", c.annotation));
            }
        }
        for (id, end, at) in &o.reattach {
            let entity = soliddraft_drawing::edge_near(part, *at).map_err(|e| e.to_string())?;
            let annotation = AnnotationId(*id);
            run(&mut stack, &mut drawing, Command::Reattach { annotation, end: *end, entity })?;
            repairs.push(format!("reattached end {end} of {annotation}"));
        }
        for id in &o.freeze {
            run(&mut stack, &mut drawing, Command::FreezeAsNote { annotation: AnnotationId(*id) })?;
            repairs.push(format!("kept annotation {id} as a note"));
        }
        for id in &o.delete {
            run(&mut stack, &mut drawing, Command::DeleteAnnotation { annotation: AnnotationId(*id) })?;
            repairs.push(format!("deleted annotation {id}"));
        }
        // A drawing from before the envelope is saved under the current name beside it.
        let saved = soliddraft_document::kind::saved_path(&path, DocKind::Drawing);
        let meta = meta.rewritten(&source.backend, drawing.meta.units);
        file::write(&saved, &drawing, &meta).map_err(|e| e.to_string())?;
        write_outputs(&drawing, &o.outs, &source.backend)?;
        // The report after repairs: what still needs attention.
        let mut final_report = report.clone();
        final_report.dangling.retain(|d| drawing.annotation(d.annotation).is_some_and(|a| a.needs_attention()));
        let name = saved.display().to_string();
        if o.json {
            let out = UpdateOut { drawing: &name, model: &drawing.model.path, update: &final_report, repairs };
            println!("{}", serde_json::to_string(&out).expect("reports serialise"));
        } else {
            println!(
                "{name}: rebuilt against {} rev {} (was rev {}): {} views updated, {} values changed, {} dangling",
                drawing.model.path,
                report.digest_after.short(),
                report.digest_before.short(),
                report.views_updated,
                report.changed.len(),
                final_report.dangling.len()
            );
            for c in &report.changed {
                println!("  changed {}: {} -> {}", c.annotation, c.previous, c.now);
            }
            for d in &final_report.dangling {
                println!("  dangling {}: {}", d.annotation, d.cause);
            }
            for r in &repairs {
                println!("  {r}");
            }
        }
        Ok(())
    })
}

/// The file sheet `i` of a one-sheet format goes to: `out` for the first sheet,
/// `<stem>-<n>.<ext>` for sheet n after it.
fn sheet_path(out: &Path, i: usize) -> PathBuf {
    if i == 0 {
        return out.to_path_buf();
    }
    let stem = out.file_stem().and_then(|s| s.to_str()).unwrap_or("drawing");
    let ext = out.extension().and_then(|e| e.to_str()).unwrap_or("");
    out.with_file_name(format!("{stem}-{}.{ext}", i + 1))
}

/// Writes the drawing in each format `outs` names. A drawing file written here is a new
/// document (Save as copy), with its own id. Every file is written atomically.
fn write_outputs(drawing: &Drawing, outs: &[String], backend: &str) -> Result<(), String> {
    let lists: Vec<_> = (0..drawing.sheets.len()).map(|i| display::layout_sheet(drawing, i)).collect();
    for out in outs {
        let path = Path::new(out);
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let files: Vec<(PathBuf, Vec<u8>)> = match (ext, Format::from_extension(ext)) {
            _ if DocKind::of_path(path) == Some(DocKind::Drawing) => {
                let meta = FileMeta::new(backend, drawing.meta.units);
                let to = soliddraft_document::kind::saved_path(path, DocKind::Drawing);
                vec![(to, file::to_bytes(drawing, &meta).map_err(|e| e.to_string())?)]
            }
            (_, Some(Format::Pdf)) => {
                let info = PdfInfo { title: drawing.meta.title.clone(), author: drawing.meta.designer.clone() };
                vec![(path.to_path_buf(), export::pdf(&lists, &info).map_err(|e| e.to_string())?)]
            }
            (_, Some(format)) => lists
                .iter()
                .enumerate()
                .map(|(i, list)| {
                    export::bytes(list, format, &drawing.meta.title)
                        .map(|b| (sheet_path(path, i), b))
                        .map_err(|e| e.to_string())
                })
                .collect::<Result<_, _>>()?,
            (_, None) => return Err(format!("{out}: write .dwdrw, .svg, .pdf or .dxf")),
        };
        for (p, bytes) in files {
            soliddraft_document::save_atomic(&p, &bytes).map_err(|e| format!("{}: {e}", p.display()))?;
        }
    }
    Ok(())
}

/// The view a command-line name means: a standard view's name, a section, detail or
/// view letter, or a view id.
fn find_view(drawing: &Drawing, name: &str) -> Result<ViewId, String> {
    if let Ok(id) = name.parse::<u32>()
        && drawing.view(ViewId(id)).is_some()
    {
        return Ok(ViewId(id));
    }
    for sheet in &drawing.sheets {
        for v in &sheet.views {
            if v.letter().is_some_and(|l| l.eq_ignore_ascii_case(name)) {
                return Ok(v.id);
            }
        }
    }
    let wanted = StandardView::from_name(name).ok_or(format!("no view {name}"))?;
    for (i, sheet) in drawing.sheets.iter().enumerate() {
        for v in &sheet.views {
            if !matches!(
                v.kind,
                soliddraft_drawing::ViewKind::Base { .. } | soliddraft_drawing::ViewKind::Projected { .. }
            ) {
                continue;
            }
            let spec = project::spec_in(drawing, i, v);
            if spec.and_then(|s| project::standard_of(s.direction, s.up)) == Some(wanted) {
                return Ok(v.id);
            }
        }
    }
    Err(format!("the drawing has no {name} view"))
}

/// Adds sheets made from the first sheet's template until there are `n` + 1.
fn ensure_sheet(stack: &mut UndoStack, drawing: &mut Drawing, part: &Part, n: usize) -> Result<(), String> {
    while drawing.sheets.len() <= n {
        let c = Command::AddSheet {
            name: format!("Sheet{}", drawing.sheets.len() + 1),
            template: drawing.sheets[0].template.clone(),
        };
        stack.run(drawing, &c, Some(part)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Makes the derived views, view edits and annotations the command line asks for, in
/// order, through the drawing's commands.
fn apply_steps(drawing: &mut Drawing, part: &Part, steps: &[Step2]) -> Result<(), String> {
    let index = PartIndex::build(part).map_err(|e| e.to_string())?;
    let mut stack = UndoStack::new();
    let err = |e: soliddraft_drawing::DrawingError| e.to_string();
    let sheet_for = |drawing: &Drawing, parent: ViewId, asked: Option<usize>| {
        asked.unwrap_or_else(|| project::sheet_of(drawing, parent).unwrap_or(0))
    };
    for step in steps {
        let command = match step {
            Step2::Section { parent, through, horizontal, flip, sheet } => {
                let parent = find_view(drawing, parent)?;
                let sheet = sheet_for(drawing, parent, *sheet);
                ensure_sheet(&mut stack, drawing, part, sheet)?;
                let line = derived::cutting_line(drawing, part, &index, parent, *through, *horizontal).map_err(err)?;
                let (view, _) =
                    derived::new_section(drawing, part, &index, sheet, parent, line, *flip, None).map_err(err)?;
                Command::PlaceView { sheet, view }
            }
            Step2::Detail { parent, center, radius, factor, sheet } => {
                let parent = find_view(drawing, parent)?;
                let sheet = sheet_for(drawing, parent, *sheet);
                ensure_sheet(&mut stack, drawing, part, sheet)?;
                let ctx = derived::context(drawing, part, &index, parent).map_err(err)?;
                let (view, _) = derived::new_detail(
                    drawing,
                    part,
                    &index,
                    sheet,
                    parent,
                    ctx.xf.point(*center),
                    radius * ctx.xf.scale,
                    Vec::new(),
                    *factor,
                    None,
                )
                .map_err(err)?;
                Command::PlaceView { sheet, view }
            }
            Step2::Aux { parent, near, flip, sheet } => {
                let parent = find_view(drawing, parent)?;
                let sheet = sheet_for(drawing, parent, *sheet);
                ensure_sheet(&mut stack, drawing, part, sheet)?;
                let edge = soliddraft_drawing::edge_near(part, *near).map_err(err)?;
                let (view, _) =
                    derived::new_auxiliary(drawing, part, &index, sheet, parent, edge, *flip, None).map_err(err)?;
                Command::PlaceView { sheet, view }
            }
            Step2::Crop { view, a, b } => {
                let view = find_view(drawing, view)?;
                let ctx = derived::context(drawing, part, &index, view).map_err(err)?;
                Command::SetViewCrop { view, crop: Some(Boundary::rectangle(ctx.xf.point(*a), ctx.xf.point(*b))) }
            }
            Step2::BrokenOut { view, at, radius, depth } => {
                let view = find_view(drawing, view)?;
                let ctx = derived::context(drawing, part, &index, view).map_err(err)?;
                let boundary = Boundary::Circle { center: ctx.xf.point(*at), radius: radius * ctx.xf.scale };
                Command::AddBrokenOut { view, broken_out: BrokenOut { boundary, depth: *depth } }
            }
            Step2::Display { view, key, value } => {
                let view = find_view(drawing, view)?;
                let mut display = drawing.view(view).expect("found").display;
                match (key.as_str(), value.as_str()) {
                    ("hidden", "removed") => display.hidden = HiddenLines::Removed,
                    ("hidden", "visible") => display.hidden = HiddenLines::Visible,
                    ("tangent", "removed") => display.tangent = TangentEdges::Removed,
                    ("tangent", "phantom") => display.tangent = TangentEdges::Phantom,
                    ("tangent", "visible") => display.tangent = TangentEdges::Visible,
                    _ => {
                        return Err(format!(
                            "--display takes hidden=removed|visible or tangent=removed|phantom|visible, not {key}={value}"
                        ));
                    }
                }
                Command::SetViewDisplay { view, display }
            }
            Step2::Set { kind, view, orientation, offset, points } => {
                let view = find_view(drawing, view)?;
                let ctx = derived::context(drawing, part, &index, view).map_err(err)?;
                let annotation = annotate::dimension_set(drawing, &index, &ctx, *kind, *orientation, points, *offset)
                    .map_err(err)?;
                Command::AddAnnotation { annotation }
            }
            Step2::Gdt { view, at, body, offset } => {
                let view = find_view(drawing, view)?;
                let ctx = derived::context(drawing, part, &index, view).map_err(err)?;
                let annotation = annotate::attached(
                    drawing,
                    &index,
                    &ctx,
                    *at,
                    body.clone(),
                    offset.unwrap_or(Point2::new(12.0, 12.0)),
                )
                .map_err(err)?;
                Command::AddAnnotation { annotation }
            }
            Step2::Datum { view, at, letter, offset } => {
                let view = find_view(drawing, view)?;
                let ctx = derived::context(drawing, part, &index, view).map_err(err)?;
                let letter = letter.clone().unwrap_or_else(|| drawing.next_datum_label.clone());
                let offset = offset.unwrap_or(Point2::new(-10.0, -14.0));
                let annotation =
                    annotate::attached(drawing, &index, &ctx, *at, AnnotationKind::Datum { letter }, offset)
                        .map_err(err)?;
                Command::AddAnnotation { annotation }
            }
            Step2::Finish { view, at, text, process, offset } => {
                let view = find_view(drawing, view)?;
                let ctx = derived::context(drawing, part, &index, view).map_err(err)?;
                let body = AnnotationKind::SurfaceFinish {
                    text: text.clone(),
                    process: *process,
                    method: String::new(),
                    lay: None,
                    allowance: String::new(),
                    all_around: false,
                };
                let offset = offset.unwrap_or(Point2::new(8.0, 8.0));
                let annotation = annotate::attached(drawing, &index, &ctx, *at, body, offset).map_err(err)?;
                Command::AddAnnotation { annotation }
            }
            Step2::Revision(row) => Command::AddRevision { sheet: 0, row: row.clone() },
            Step2::RevisionSymbol { view, at, revision, offset } => {
                let view = find_view(drawing, view)?;
                let ctx = derived::context(drawing, part, &index, view).map_err(err)?;
                let body = AnnotationKind::RevisionSymbol { revision: revision.clone() };
                let offset = offset.unwrap_or(Point2::new(10.0, 10.0));
                let annotation = annotate::attached(drawing, &index, &ctx, *at, body, offset).map_err(err)?;
                Command::AddAnnotation { annotation }
            }
        };
        stack.run(drawing, &command, Some(part)).map_err(err)?;
    }
    Ok(())
}

/// Replays the recipe and describes its result as a part: the body, its geometric
/// digest and the dimensions of its features.
fn build<'k>(
    kernel: &'k CatallaxKernel,
    recipe: &Recipe,
    path: &Path,
    revision: ContentHash,
) -> Result<(Part<'k>, String), String> {
    let (report, body) = soliddraft_recipe::build(kernel, recipe).map_err(|e| format!("{}: {e}", path.display()))?;
    let backend = format!("{} {}", report.backend.name, report.backend.version);
    // The CLI only writes files, so its views come from the certified mesh.
    let part =
        Part { kernel, body, digest: revision, dimensions: feature_dimensions(recipe), namer: None, preview: false };
    Ok((part, backend))
}

/// The dimensions a recipe's features define: each rectangle's width and height, each
/// circle's diameter (identical cut circles counted as one), and each extrusion that adds
/// material dimensioned from the part's base along its direction, so heights share one
/// datum.
pub(crate) fn feature_dimensions(recipe: &Recipe) -> Vec<ModelDimension> {
    let step = |id: &str| recipe.steps.iter().find(|s| s.id == id);
    let mut input = step(&recipe.result);
    let mut datum = None;
    // Follow the result's target to its base profile, so display bounds never set a dimension datum.
    for _ in &recipe.steps {
        match input.map(|s| &s.command) {
            Some(Step::Profile { plane, .. }) => {
                datum = Some(plane.plane().origin);
                break;
            }
            Some(command) => input = command.inputs().first().and_then(|id| step(id)),
            None => break,
        }
    }
    let mut out: Vec<ModelDimension> = Vec::new();
    let mut cut_circles: Vec<(usize, f64, Vec3)> = Vec::new();
    for s in &recipe.steps {
        let Step::Extrude { profile, distance, direction } = &s.command else { continue };
        let Some(Step::Profile { plane, shape }) = step(profile).map(|p| &p.command) else { continue };
        let plane = plane.plane();
        let dir = direction.and_then(|d| d.normalized()).unwrap_or(plane.normal);
        let cut = recipe
            .steps
            .iter()
            .any(|b| matches!(&b.command, Step::Boolean { kind: BooleanOp::Subtract, tool, .. } if tool == &s.id));
        let feature = recipe
            .steps
            .iter()
            .find(|b| matches!(&b.command, Step::Boolean { tool, .. } if tool == &s.id))
            .map(|b| b.id.clone())
            .unwrap_or_else(|| s.id.clone());
        match shape {
            Shape::Rectangle { center, width, height } => {
                let (x, y) = (plane.x_dir, plane.y_dir());
                let c = plane.to_world(*center);
                for (name, axis, half) in [("width", x, width / 2.0), ("height", y, height / 2.0)] {
                    out.push(ModelDimension {
                        feature: feature.clone(),
                        name: name.into(),
                        measure: ModelMeasure::Distance {
                            axis,
                            from: c - axis * half,
                            to: c + axis * half,
                            sketch_normal: Some(plane.normal),
                        },
                        count: 1,
                    });
                }
            }
            Shape::Circle { center, radius } => {
                let c = plane.to_world(*center);
                if cut
                    && let Some(group) = cut_circles
                        .iter()
                        .find(|(_, r, n)| (r - radius).abs() < 1e-9 && n.cross(plane.normal).length() < 1e-9)
                {
                    out[group.0].count += 1;
                    continue;
                }
                if cut {
                    cut_circles.push((out.len(), *radius, plane.normal));
                }
                out.push(ModelDimension {
                    feature: feature.clone(),
                    name: "diameter".into(),
                    measure: ModelMeasure::Diameter { center: c, normal: plane.normal, radius: *radius },
                    count: 1,
                });
            }
            Shape::Loops { .. } => {}
        }
        if !cut {
            let base = datum.unwrap_or(plane.origin).dot(dir);
            let from = plane.origin + dir * (base - plane.origin.dot(dir));
            out.push(ModelDimension {
                feature: s.id.clone(),
                name: "depth".into(),
                measure: ModelMeasure::Distance {
                    axis: dir,
                    from,
                    to: plane.origin + dir * *distance,
                    sketch_normal: None,
                },
                count: 1,
            });
        }
    }
    out
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Today's date, or the date of `SOURCE_DATE_EPOCH` when set, so builds can be
/// reproducible.
fn today() -> String {
    let secs =
        std::env::var("SOURCE_DATE_EPOCH").ok().and_then(|s| s.trim().parse::<i64>().ok()).unwrap_or_else(|| {
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
        });
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

/// Proleptic Gregorian date of a day count from 1970-01-01 (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `target` relative to the directory `base`, with forward slashes.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_from_day_counts() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_740), (2026, 10, 14));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }

    #[test]
    fn the_bracket_recipe_defines_its_dimensions() {
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../recipes/bracket.json")).unwrap();
        let recipe = soliddraft_recipe::parse(&text).unwrap();
        let dims = feature_dimensions(&recipe);
        let names: Vec<String> = dims.iter().map(|d| format!("{} {} x{}", d.feature, d.name, d.count)).collect();
        assert!(names.contains(&"hole1 diameter x4".to_owned()), "{names:?}");
        assert!(names.contains(&"slot width x1".to_owned()) && names.contains(&"slot height x1".to_owned()));
        let boss = dims.iter().find(|d| d.feature == "boss" && d.name == "depth").unwrap();
        let ModelMeasure::Distance { from, to, .. } = boss.measure else { panic!() };
        assert_eq!((from.z, to.z), (0.0, 30.0), "heights are from the base");
        let plate = dims.iter().find(|d| d.feature == "plate" && d.name == "depth").unwrap();
        let ModelMeasure::Distance { from, to, .. } = plate.measure else { panic!() };
        assert_eq!((from.z, to.z), (0.0, 10.0));
    }

    #[test]
    fn recipe_depths_use_the_base_profile_datum() {
        let mut recipe = soliddraft_recipe::parse(include_str!("../../../recipes/box.json")).unwrap();
        let Step::Profile { plane, .. } = &mut recipe.steps[0].command else { panic!() };
        *plane =
            soliddraft_recipe::PlaneSpec::Full(soliddraft_kernel_api::Plane::XY.with_origin(Vec3::new(0.0, 0.0, 7.0)));
        let dims = feature_dimensions(&recipe);
        let depth = dims.iter().find(|d| d.name == "depth").unwrap();
        let ModelMeasure::Distance { from, to, .. } = depth.measure else { panic!() };
        assert_eq!((from.z, to.z), (7.0, 17.0));
    }
}
