//! Making a drawing: which views fold off which, the sheet scale, and where each view
//! goes on the sheet.
//!
//! The first orthographic view asked for is the base view. Every other orthographic view
//! that is a fold of it becomes a projected view aligned with it, on the side the
//! sheet's projection angle puts it; pictorial views and any others are base views of
//! their own. Orthographic views sit on a grid around the base view, columns as wide as
//! their widest view and rows as tall as their tallest, with room for dimensions between
//! them. The sheet scale is the largest of the template's scales at which that grid fits
//! the drawing area without covering the title block. Pictorial views go in the space
//! left at the top right, at the largest scale up to the sheet's that fits there.

use serde::{Deserialize, Serialize};
use soliddraft_kernel_api::Point2;
use soliddraft_kernel_api::drawing::{StandardView, ViewCurves, ViewSpec};

use crate::ids::ViewId;
use crate::model_items::{self, ImportReport};
use crate::part::{Part, PartIndex};
use crate::project::{self, Projected, ViewContext};
use crate::sheet::Scale;
use crate::template::Template;
use crate::view::{
    AlignAxis, Alignment, FoldDirection, HiddenLines, Orientation, View, ViewDisplay, ViewKind, ViewScale,
};
use crate::{DocRef, Drawing, DrawingError, DrawingMeta};

/// What to put on a new drawing: the New drawing PropertyManager's choices.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NewDrawing {
    /// The template.
    pub template: Template,
    /// The views, the base view first.
    pub views: Vec<StandardView>,
    /// The part. Its pinned revision is set from the part the drawing is made of.
    pub model: DocRef,
    /// Document properties.
    pub meta: DrawingMeta,
    /// Hidden lines in orthographic views. Pictorial views always remove them.
    pub hidden: HiddenLines,
    /// Bring in the part's feature dimensions.
    pub model_items: bool,
    /// Put centre marks on the circles orthographic views see face-on.
    #[serde(default)]
    pub centre_marks: bool,
    /// The sheet scale, or `None` to pick the largest of the template's scales that
    /// fits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<Scale>,
}

/// A new drawing with what making it found.
#[derive(Clone, Debug, PartialEq)]
pub struct MadeDrawing {
    /// The drawing.
    pub drawing: Drawing,
    /// What Model items did.
    pub import: ImportReport,
    /// Each view's projection, for tools that pick on the views.
    pub projections: Vec<(ViewId, Projected)>,
}

/// Sheet millimetres kept between neighbouring orthographic views and around the grid,
/// for dimensions.
const VIEW_GAP: f64 = 30.0;
const GRID_MARGIN: f64 = 25.0;
/// Room kept around pictorial views, which get no dimensions.
const PICTORIAL_MARGIN: f64 = 5.0;
/// Space kept above the title block.
const TITLE_CLEARANCE: f64 = 10.0;

/// The standard views to place for a template when none are asked for: front, top, the
/// side view that goes right of the front view, and the isometric.
pub fn default_views(template: &Template) -> Vec<StandardView> {
    let side = match template.projection {
        crate::Projection::FirstAngle => StandardView::Left,
        crate::Projection::ThirdAngle => StandardView::Right,
    };
    vec![StandardView::Front, StandardView::Top, side, StandardView::Isometric]
}

/// Makes a drawing of `part` with one sheet, the views placed and scaled, and, when
/// asked, centre marks and the model items.
pub fn make_drawing(part: &Part, new: &NewDrawing) -> Result<MadeDrawing, DrawingError> {
    if new.views.is_empty() {
        return Err(DrawingError::BadRequest("no views asked for".into()));
    }
    let index = PartIndex::build(part)?;
    let mut drawing = Drawing::new(new.model.clone(), &new.template);
    drawing.meta = new.meta.clone();
    drawing.accept_source(part);
    let projection = new.template.projection;

    // Which views fold off the base view.
    let specs: Vec<ViewSpec> = new.views.iter().map(|v| ViewSpec::standard(*v)).collect();
    let base = specs.iter().position(|s| !project::is_pictorial(s)).unwrap_or(0);
    let base_frame = specs[base].frame().ok_or(DrawingError::BadRequest("base view".into()))?;
    let mut planned: Vec<(StandardView, Option<FoldDirection>)> = Vec::new();
    for (i, (v, spec)) in new.views.iter().zip(&specs).enumerate() {
        let fold = (i != base && !project::is_pictorial(spec))
            .then(|| {
                [FoldDirection::Left, FoldDirection::Right, FoldDirection::Up, FoldDirection::Down].into_iter().find(
                    |f| {
                        let (d, u) = project::fold(&base_frame, *f, projection);
                        let (want, got) = (spec.frame(), ViewSpec::new(d, u).frame());
                        want.zip(got).is_some_and(|(w, g)| {
                            w.direction.distance(g.direction) < 1e-9 && w.up.distance(g.up) < 1e-9
                        })
                    },
                )
            })
            .flatten();
        planned.push((*v, fold));
    }
    // The base view goes first so projected views follow their parent.
    planned.swap(0, base);

    // The kernel's curves for every view, unscaled.
    let mut curves: Vec<ViewCurves> = Vec::new();
    for (v, _) in &planned {
        curves.push(project::view_curves(part, &ViewSpec::standard(*v))?);
    }
    let extent = |c: &ViewCurves| (c.bbox_2d[2] - c.bbox_2d[0], c.bbox_2d[3] - c.bbox_2d[1]);

    // The grid of orthographic views around the base view.
    let cell = |fold: Option<FoldDirection>, i: usize| -> Option<(i32, i32)> {
        match fold {
            _ if i == 0 => Some((0, 0)),
            Some(FoldDirection::Left) => Some((-1, 0)),
            Some(FoldDirection::Right) => Some((1, 0)),
            Some(FoldDirection::Up) => Some((0, 1)),
            Some(FoldDirection::Down) => Some((0, -1)),
            _ => None,
        }
    };
    let cells: Vec<Option<(i32, i32)>> = planned.iter().enumerate().map(|(i, (_, f))| cell(*f, i)).collect();
    let mut col_w = [0.0f64; 3];
    let mut row_h = [0.0f64; 3];
    for (c, ext) in cells.iter().zip(curves.iter().map(extent)) {
        if let Some((x, y)) = c {
            col_w[(x + 1) as usize] = col_w[(x + 1) as usize].max(ext.0);
            row_h[(1 - y) as usize] = row_h[(1 - y) as usize].max(ext.1);
        }
    }
    let used = |sizes: &[f64; 3]| sizes.iter().filter(|s| **s > 0.0).count().max(1) as f64;
    let grid = |s: f64| {
        (
            s * col_w.iter().sum::<f64>() + VIEW_GAP * (used(&col_w) - 1.0) + 2.0 * GRID_MARGIN,
            s * row_h.iter().sum::<f64>() + VIEW_GAP * (used(&row_h) - 1.0) + 2.0 * GRID_MARGIN,
        )
    };

    let t = &new.template;
    // Inside the frame and its zone band.
    let band = crate::library::library().templates.iter().find(|f| f.id == t.id.as_str()).map_or(0.0, |f| f.zones.band);
    let area = Rect {
        x0: t.margins.left + band,
        y0: t.margins.bottom + band,
        x1: t.size.width_mm - t.margins.right - band,
        y1: t.size.height_mm - t.margins.top - band,
    };
    let tb = &t.title_block;
    let title = Rect {
        x0: tb.position.u,
        y0: tb.position.v,
        x1: tb.position.u + tb.width,
        y1: tb.position.v + tb.height + TITLE_CLEARANCE,
    };
    // The grid fits when it is inside the drawing area and none of its occupied cells,
    // with room for dimensions around them, covers the title block.
    let fits = |s: f64| {
        let (w, h) = grid(s);
        if w > area.width() || h > area.height() {
            return false;
        }
        let edges = |sizes: &[f64; 3], start: f64, sign: f64| {
            let mut out = [(0.0, 0.0); 3];
            let mut at = start + sign * GRID_MARGIN;
            for (i, size) in sizes.iter().enumerate() {
                if *size > 0.0 {
                    let end = at + sign * s * size;
                    out[i] = (at.min(end), at.max(end));
                    at = end + sign * VIEW_GAP;
                }
            }
            out
        };
        let (cols, rows) = (edges(&col_w, area.x0, 1.0), edges(&row_h, area.y1, -1.0));
        cells.iter().flatten().all(|(cx, cy)| {
            let ((x0, x1), (y0, y1)) = (cols[(cx + 1) as usize], rows[(1 - cy) as usize]);
            let m = GRID_MARGIN;
            !Rect { x0: x0 - m, y0: y0 - m, x1: x1 + m, y1: y1 + m }.overlaps(&title)
        })
    };
    let mut scales = t.scales.clone();
    scales.sort_by(|a, b| b.factor().total_cmp(&a.factor()));
    let sheet_scale = new.scale.unwrap_or_else(|| {
        scales
            .iter()
            .copied()
            .find(|s| fits(s.factor()))
            .unwrap_or_else(|| scales.last().copied().unwrap_or(Scale::FULL))
    });
    let s = sheet_scale.factor();
    drawing.sheets[0].scale = sheet_scale;

    // Cell centres, from the grid's top-left corner.
    let (grid_w, _) = grid(s);
    let mut col_x = [0.0; 3];
    let mut x = area.x0 + GRID_MARGIN;
    for c in 0..3 {
        if col_w[c] > 0.0 {
            col_x[c] = x + s * col_w[c] / 2.0;
            x += s * col_w[c] + VIEW_GAP;
        }
    }
    let mut row_y = [0.0; 3];
    let mut y = area.y1 - GRID_MARGIN;
    for r in 0..3 {
        if row_h[r] > 0.0 {
            row_y[r] = y - s * row_h[r] / 2.0;
            y -= s * row_h[r] + VIEW_GAP;
        }
    }

    // The free region for pictorial views: right of the grid, above the title block.
    let free = Rect { x0: area.x0 + grid_w, y0: title.y1, x1: area.x1, y1: area.y1 };
    let pictorial: Vec<usize> = cells.iter().enumerate().filter(|(_, c)| c.is_none()).map(|(i, _)| i).collect();
    let slot_h = free.height() / pictorial.len().max(1) as f64;

    let mut ids = Vec::new();
    for (i, ((v, fold), c)) in planned.iter().zip(&cells).enumerate() {
        let id = ViewId(i as u32 + 1);
        ids.push(id);
        let is_pictorial = project::is_pictorial(&ViewSpec::standard(*v));
        let display = if is_pictorial {
            ViewDisplay::PICTORIAL
        } else {
            ViewDisplay { hidden: new.hidden, ..ViewDisplay::ORTHOGRAPHIC }
        };
        let (kind, alignment, scale, position) = match (c, fold) {
            (Some((cx, cy)), Some(f)) if i != 0 => {
                let axis = if *cy == 0 { AlignAxis::Horizontal } else { AlignAxis::Vertical };
                let position = Point2::new(col_x[(cx + 1) as usize], row_y[(1 - cy) as usize]);
                (
                    ViewKind::Projected { parent: ViewId(1), direction: *f },
                    Some(Alignment { parent: ViewId(1), axis }),
                    ViewScale::Parent,
                    position,
                )
            }
            (Some((cx, cy)), _) => (
                ViewKind::Base { orientation: Orientation::Standard { view: *v } },
                None,
                ViewScale::Sheet,
                Point2::new(col_x[(cx + 1) as usize], row_y[(1 - cy) as usize]),
            ),
            (None, _) => {
                let k = pictorial.iter().position(|p| *p == i).unwrap_or(0) as f64;
                let region =
                    Rect { x0: free.x0, y0: free.y1 - slot_h * (k + 1.0), x1: free.x1, y1: free.y1 - slot_h * k };
                let (w, h) = extent(&curves[i]);
                let fit = |f: f64| {
                    f * w + 2.0 * PICTORIAL_MARGIN <= region.width()
                        && f * h + 2.0 * PICTORIAL_MARGIN <= region.height()
                };
                let own = scales.iter().copied().filter(|x| x.factor() <= s).find(|x| fit(x.factor()));
                let own = own.unwrap_or_else(|| scales.last().copied().unwrap_or(Scale::FULL));
                let scale =
                    if own == sheet_scale { ViewScale::Sheet } else { ViewScale::Custom { factor: own.factor() } };
                // Top of its region, centred across it.
                let y = region.y1 - PICTORIAL_MARGIN - own.factor() * h / 2.0;
                (
                    ViewKind::Base { orientation: Orientation::Standard { view: *v } },
                    None,
                    scale,
                    Point2::new(crate::part::round((region.x0 + region.x1) / 2.0), crate::part::round(y)),
                )
            }
        };
        drawing.sheets[0].views.push(View {
            id,
            kind,
            position: Point2::new(crate::part::round(position.u), crate::part::round(position.v)),
            alignment,
            scale,
            display,
            excluded_from_update: false,
            geometry: Default::default(),
            label: None,
            crop: None,
            broken_out: Vec::new(),
        });
    }

    // Line work at each view's scale.
    let mut contexts = Vec::new();
    for (id, c) in ids.iter().zip(curves) {
        let spec = project::view_spec(&drawing, *id)?;
        let xf = project::transform(&c, &spec, project::view_scale(&drawing, *id)?)?;
        let projected = project::project(&index, &c, &xf, part.digest.as_str());
        drawing.view_mut(*id).expect("just placed").geometry = projected.geometry.clone();
        let pictorial = project::is_pictorial(&spec);
        contexts.push(ViewContext { id: *id, curves: c, xf, pictorial, projected });
    }

    if new.centre_marks {
        for ctx in contexts.iter().filter(|c| !c.pictorial) {
            let first = drawing.next_annotation_id().0;
            let marks = project::centre_marks(&ctx.projected, ctx.id, first);
            drawing.sheets[0].annotations.extend(marks);
        }
    }
    let import = if new.model_items {
        model_items::import(&mut drawing, 0, part, &index, &contexts)?
    } else {
        ImportReport::default()
    };
    let projections = contexts.into_iter().map(|c| (c.id, c.projected)).collect();
    Ok(MadeDrawing { drawing, import, projections })
}

#[derive(Clone, Copy, Debug)]
struct Rect {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl Rect {
    fn width(&self) -> f64 {
        self.x1 - self.x0
    }

    fn height(&self) -> f64 {
        self.y1 - self.y0
    }

    fn overlaps(&self, o: &Rect) -> bool {
        self.x0 < o.x1 && o.x0 < self.x1 && self.y0 < o.y1 && o.y0 < self.y1
    }
}
