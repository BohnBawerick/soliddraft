//! Auto relations around the sketcher's inference, and drawing new entities as
//! construction geometry.
//!
//! With auto relations on, the sketcher offers its usual inferences in their usual
//! order (endpoint, midpoint, level or plumb, on a curve) and a click keeps the
//! relation it showed. With them off, the point stays under the pointer and nothing is
//! inferred; relations already in the sketch, and ones added by hand, still solve. Both
//! switches are interaction state: they change no document and add no undo step.

use crate::sketch::P2;
use crate::state::{SketchSession, Snap};
use egui::{Rect, pos2, vec2};

/// What the sketcher offers at the cursor, given what it inferred there.
pub fn offered(auto_relations: bool, raw: P2, inferred: (P2, Vec<Snap>)) -> (P2, Vec<Snap>) {
    if auto_relations { inferred } else { (raw, Vec::new()) }
}

/// The two switches, under the sketch status in the viewport's corner.
pub fn switches(ui: &mut egui::Ui, s: &mut SketchSession, rect: Rect) {
    let at = Rect::from_min_size(pos2(rect.left() + 12.0, rect.top() + 58.0), vec2(150.0, 22.0));
    ui.put(at, egui::Checkbox::new(&mut s.auto_relations, "Auto relations"));
    ui.put(at.translate(vec2(0.0, 24.0)), egui::Checkbox::new(&mut s.construction, "Draw construction"));
}
