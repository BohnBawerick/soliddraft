//! `soliddraft view`: hidden-line views of a recipe's result, as SVG or JSON.

use std::fmt::Write as _;
use std::process::ExitCode;
use std::time::Instant;

use serde::Serialize;
use soliddraft_kernel_api::drawing::{Curve3, CutRegion, SectionSpec, StandardView, ViewCurves, ViewFrame, ViewSpec};
use soliddraft_kernel_api::{Kernel, Vec3};
use soliddraft_kernel_catallax::CatallaxKernel;

pub(crate) const USAGE: &str = "\
usage: soliddraft view <recipe.json> [--dir <view>[,<view>...]] [--up x,y,z]
                      [--seams] [--smooth] [--tolerance <mm>] [--mesh]
                      [--section x,y,z] [--svg <out.svg>] [--json] [--curves]

Replays a recipe on the Catallax kernel and computes hidden-line views of its result:
visible curves solid, hidden curves dashed, each tagged with the edge or face it
comes from.

  --dir        front, back, top, bottom, right, left, iso, several separated by
               commas, or one direction towards the eye as x,y,z (default front)
  --up         which way is up on the sheet (default z, or y for top and bottom)
  --seams      show seams
  --smooth     show edges between tangent faces
  --tolerance  curve accuracy in mm (default: the kernel's)
  --mesh       use polygon hidden-line removal (the default on Catallax)
  --section    cut the body by the plane through this point that faces the eye,
               remove the material in front of it and hatch the cut faces
  --svg        write the view to this SVG file; with several views, put {view} in
               the name to write one file per view
  --json       print one JSON summary per view: backend, counts, digest and time
  --curves     with --json, include every curve
";

/// One view's summary line.
#[derive(Serialize)]
struct Summary<'a> {
    recipe: &'a str,
    backend: String,
    view: String,
    method: &'static str,
    visible: usize,
    hidden: usize,
    digest: String,
    millis: f64,
    bbox: [f64; 4],
    #[serde(skip_serializing_if = "Option::is_none")]
    curves: Option<&'a ViewCurves>,
    /// For a section: the areas of the cut regions, largest first.
    #[serde(skip_serializing_if = "Option::is_none")]
    regions: Option<Vec<f64>>,
}

struct Options {
    file: String,
    views: Vec<(String, ViewSpec)>,
    svg: Option<String>,
    json: bool,
    curves: bool,
    section: Option<Vec3>,
}

fn vector(s: &str) -> Option<Vec3> {
    let parts: Vec<f64> = s.split(',').map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
    match parts[..] {
        [x, y, z] => Some(Vec3::new(x, y, z)),
        _ => None,
    }
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut file = None;
    let mut dirs = "front".to_string();
    let (mut up, mut seams, mut smooth, mut tolerance) = (None, false, false, 0.0);
    let mut o =
        Options { file: String::new(), views: Vec::new(), svg: None, json: false, curves: false, section: None };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().cloned().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "--dir" => dirs = value("--dir")?,
            "--up" => up = Some(vector(&value("--up")?).ok_or("--up needs x,y,z")?),
            "--seams" => seams = true,
            "--smooth" => smooth = true,
            "--tolerance" => {
                tolerance = value("--tolerance")?.parse().map_err(|_| "--tolerance needs a number of millimetres")?
            }
            "--mesh" => {}
            "--section" => o.section = Some(vector(&value("--section")?).ok_or("--section needs x,y,z")?),
            "--svg" => o.svg = Some(value("--svg")?),
            "--json" => o.json = true,
            "--curves" => o.curves = true,
            s if s.starts_with('-') => return Err(format!("unknown option {s}")),
            s if file.is_none() => file = Some(s.to_string()),
            s => return Err(format!("unexpected argument {s}")),
        }
    }
    o.file = file.ok_or("no recipe given")?;
    // A direction given as x,y,z has commas of its own, so try the whole value first.
    let names: Vec<String> =
        if vector(&dirs).is_some() { vec![dirs.clone()] } else { dirs.split(',').map(str::to_string).collect() };
    for name in names {
        let mut spec = match StandardView::from_name(&name) {
            Some(v) => ViewSpec::standard(v),
            None => {
                let d = vector(&name).ok_or(format!("unknown view {name}"))?;
                ViewSpec::new(d.normalized().ok_or("the view direction is zero")?, Vec3::Z)
            }
        };
        if let Some(u) = up {
            spec.up = u;
        }
        spec.seams = seams;
        spec.smooth_edges = smooth;
        spec.tolerance = tolerance;
        if spec.frame().is_none() {
            return Err(format!("view {name}: the direction is parallel to up"));
        }
        o.views.push((name, spec));
    }
    if o.views.len() > 1 && o.svg.as_ref().is_some_and(|p| !p.contains("{view}")) {
        return Err("with several views, put {view} in the --svg name".into());
    }
    Ok(o)
}

pub(crate) fn run(args: &[String]) -> ExitCode {
    let o = match parse(args) {
        Ok(o) => o,
        Err(e) => {
            eprint!("{e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let kernel = if o.svg.is_some() { CatallaxKernel::with_operation_log() } else { CatallaxKernel::new() };
    let built = std::fs::read_to_string(&o.file)
        .map_err(|e| e.to_string())
        .and_then(|text| soliddraft_recipe::parse(&text).map_err(|e| e.to_string()))
        .and_then(|recipe| soliddraft_recipe::build(&kernel, &recipe).map_err(|e| e.to_string()));
    let (report, body) = match built {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{}: {e}", o.file);
            return ExitCode::FAILURE;
        }
    };
    let mut ok = true;
    for (name, spec) in &o.views {
        let start = Instant::now();
        let result = match o.section {
            Some(origin) => {
                kernel.section_view(body, &SectionSpec { view: *spec, origin }).map(|s| (s.curves, Some(s.regions)))
            }
            None => kernel.view_curves(body, spec).map(|c| (c, None)),
        };
        let millis = start.elapsed().as_secs_f64() * 1000.0;
        let (curves, regions) = match result {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{}: view {name}: {e}", o.file);
                ok = false;
                continue;
            }
        };
        let frame = spec.frame().expect("checked when parsing");
        if let Some(path) = &o.svg {
            let path = path.replace("{view}", name);
            if let Err(e) = std::fs::write(&path, svg(&curves, regions.as_deref().unwrap_or(&[]), &frame)) {
                eprintln!("{path}: {e}");
                ok = false;
            }
        }
        let (visible, hidden) = curves.counts();
        if o.json {
            let summary = Summary {
                recipe: &report.recipe,
                backend: format!("{} {}", report.backend.name, report.backend.version),
                view: name.clone(),
                method: "mesh",
                visible,
                hidden,
                digest: curves.digest(),
                millis,
                bbox: curves.bbox_2d,
                curves: o.curves.then_some(&curves),
                regions: regions.as_ref().map(|r| {
                    let mut areas: Vec<f64> = r.iter().map(|x| (x.area(&frame) * 1e3).round() / 1e3).collect();
                    areas.sort_by(|a, b| b.total_cmp(a));
                    areas
                }),
            };
            println!("{}", serde_json::to_string(&summary).expect("summaries serialise"));
        } else {
            let cut = regions.as_ref().map(|r| format!(", {} cut regions", r.len())).unwrap_or_default();
            println!(
                "{} view {name}: {visible} visible, {hidden} hidden{cut}, digest {}, {millis:.1} ms",
                report.recipe,
                curves.digest()
            );
        }
    }
    let _ = kernel.release(body);
    match kernel.take_diagnostics().and_then(|log| {
        let text = serde_json::to_string(&log).expect("diagnostics serialize");
        if let Some(svg) = &o.svg {
            std::fs::write(format!("{svg}.kernel.json"), text)
                .map_err(|e| soliddraft_kernel_api::KernelError::Unavailable(e.to_string()))?;
        }
        Ok(())
    }) {
        Ok(()) => {}
        Err(error) => {
            eprintln!("view diagnostics: {error}");
            ok = false;
        }
    }
    if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

/// Visible lines 0.5 mm, hidden 0.25 mm dashed, after ISO 128. Units are model mm.
const VISIBLE_STYLE: &str = r##"fill="none" stroke="#000" stroke-width="0.5" stroke-linecap="round""##;
const HIDDEN_STYLE: &str = r##"fill="none" stroke="#000" stroke-width="0.25" stroke-dasharray="3 1.5""##;

/// The view as an SVG drawing at 1:1 in millimetres. Lines, and arcs whose plane faces
/// the eye, are written exactly; other curves as polylines within 0.01 mm. Cut regions
/// are hatched at 45 degrees, 3 mm apart.
pub(crate) fn svg(curves: &ViewCurves, regions: &[CutRegion], frame: &ViewFrame) -> String {
    let margin = 5.0;
    let [x0, y0, x1, y1] = curves.bbox_2d;
    let (w, h) = (x1 - x0 + 2.0 * margin, y1 - y0 + 2.0 * margin);
    let mut out = String::new();
    let _ = writeln!(
        out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.3}mm" height="{h:.3}mm" viewBox="{:.3} {:.3} {w:.3} {h:.3}">"#,
        x0 - margin,
        -y1 - margin
    );
    if !regions.is_empty() {
        out.push_str(
            "<defs><pattern id=\"hatch\" width=\"3\" height=\"3\" patternUnits=\"userSpaceOnUse\" \
             patternTransform=\"rotate(-45)\"><line x1=\"0\" y1=\"0\" x2=\"0\" y2=\"3\" stroke=\"#000\" \
             stroke-width=\"0.25\"/></pattern></defs>\n",
        );
        for r in regions {
            // One closed subpath per loop: each curve after the first continues from
            // where the previous one ended, so its own move-to is dropped.
            let ring = |l: &[soliddraft_kernel_api::drawing::ViewCurve]| {
                let mut d = String::new();
                for (i, c) in l.iter().enumerate() {
                    let part = path_data(&c.curve, frame);
                    if i == 0 {
                        d.push_str(&part);
                    } else {
                        let rest: Vec<&str> = part.split(' ').skip(3).collect();
                        d.push(' ');
                        d.push_str(&rest.join(" "));
                    }
                }
                d.push_str(" Z");
                d
            };
            let mut d = ring(&r.outer);
            for h in &r.holes {
                d.push(' ');
                d.push_str(&ring(h));
            }
            let _ = writeln!(out, r#"<path d="{d}" fill="url(#hatch)" fill-rule="evenodd" stroke="none"/>"#);
        }
    }
    // Hidden first, so visible lines are drawn over them.
    for visible in [false, true] {
        let _ = writeln!(out, "<g {}>", if visible { VISIBLE_STYLE } else { HIDDEN_STYLE });
        for c in curves.curves.iter().filter(|c| c.visible == visible) {
            let _ = writeln!(out, r#"<path d="{}"/>"#, path_data(&c.curve, frame));
        }
        out.push_str("</g>\n");
    }
    out.push_str("</svg>\n");
    out
}

/// SVG path data for a curve, with sheet Y pointing up.
fn path_data(curve: &Curve3, frame: &ViewFrame) -> String {
    let pt = |p: Vec3| {
        let q = frame.project(p);
        format!("{:.4} {:.4}", q.u, -q.v)
    };
    match curve {
        Curve3::Line { a, b } => format!("M {} L {}", pt(*a), pt(*b)),
        Curve3::Arc { normal, radius, start_deg, end_deg, .. } if normal.cross(frame.direction).length() < 1e-9 => {
            // Counter-clockwise about a normal towards the eye is counter-clockwise on the
            // sheet, which is clockwise on SVG's Y-down canvas: sweep flag 1.
            let sweep = u8::from(normal.dot(frame.direction) > 0.0);
            let large = u8::from(end_deg - start_deg > 180.0);
            format!("M {} A {r:.4} {r:.4} 0 {large} {sweep} {}", pt(curve.start()), pt(curve.end()), r = radius)
        }
        Curve3::Circle { normal, .. } if normal.cross(frame.direction).length() < 1e-9 => {
            let (a, b) = (curve.point_at(0.0), curve.point_at(0.5));
            let r = a.distance(b) / 2.0;
            format!("M {} A {r:.4} {r:.4} 0 1 0 {} A {r:.4} {r:.4} 0 1 0 {} Z", pt(a), pt(b), pt(a))
        }
        _ => {
            let mut d = String::new();
            for (i, p) in curve.sample(0.01).into_iter().enumerate() {
                let _ = write!(d, "{}{}", if i == 0 { "M " } else { " L " }, pt(p));
            }
            d
        }
    }
}
