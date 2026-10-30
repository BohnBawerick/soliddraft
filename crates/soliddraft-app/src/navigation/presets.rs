//! The nine offered presets, from the navigation research's recommended preset table.
//! Each row cites the product's documented default; where the product documents no key,
//! there is no row.

use super::{
    Action, Binding, Button, Capability, DragAxis, LimitKind, Limitation, Mods, NavContext, Preset, WheelPolicy,
    ZoomAnchor, ZoomDirection,
};
use egui::Key;
use std::sync::LazyLock;

const L: Button = Button::Left;
const M: Button = Button::Middle;
const R: Button = Button::Right;

/// Known presets that are not offered, and why.
pub(super) const WITHHELD: [(&str, &str); 2] = [
    ("freecad_cad", "FreeCAD's CAD navigation needs ordered button presses, which SolidDraft does not recognise yet"),
    (
        "catia_profile_experimental",
        "the CATIA profile needs ordered button presses, and which CATIA release it describes is not confirmed",
    ),
];

fn limit(kind: LimitKind, text: &'static str) -> Limitation {
    Limitation { kind, text }
}

/// The wheel and a pinch zoom in every preset.
fn zoom_rows() -> [Binding; 2] {
    [Binding::wheel("wheel", 1), Binding::pinch("pinch", 2)]
}

pub(super) static PRESETS: LazyLock<Vec<Preset>> = LazyLock::new(|| {
    vec![legacy(), fusion(), solidworks(), onshape(), inventor(), creo(), nx12(), solid_edge(), blender()]
});

fn legacy() -> Preset {
    let mut bindings = vec![
        Binding::drag("orbit", &[L], Mods::NONE, Action::Orbit, 10).in_context(NavContext::Model),
        Binding::drag("pan_middle", &[M], Mods::NONE, Action::Pan, 11),
        Binding::drag("pan_right", &[R], Mods::NONE, Action::Pan, 12),
        Binding::drag("pan_shift_left", &[L], Mods::SHIFT, Action::Pan, 20),
        Binding::key("fit", Key::Z, Mods::NONE, Action::Fit, 30),
        Binding::key("normal", Key::N, Mods::NONE, Action::NormalTo, 31),
    ];
    bindings.extend(zoom_rows());
    Preset {
        id: "legacy_soliddraft",
        label: "SolidDraft (legacy)",
        release_scope: "SolidDraft's original controls, kept for existing installations.",
        wheel: WheelPolicy::SoliddraftDefault,
        bindings,
        limitations: vec![limit(
            LimitKind::Note,
            "Left drag orbits outside a sketch; in a sketch the left button belongs to the sketch tools.",
        )],
    }
}

fn fusion() -> Preset {
    let mut bindings = vec![
        Binding::drag("pan", &[M], Mods::NONE, Action::Pan, 10),
        Binding::drag("orbit", &[M], Mods::SHIFT, Action::Orbit, 20),
        Binding::key("fit", Key::F6, Mods::NONE, Action::Fit, 30),
    ];
    bindings.extend(zoom_rows());
    Preset {
        id: "fusion",
        label: "Fusion 360",
        release_scope: "Autodesk Fusion's default mouse controls. The Mac trackpad gestures are a separate profile.",
        wheel: WheelPolicy::SoliddraftDefault,
        bindings,
        limitations: vec![
            limit(
                LimitKind::Unresolved,
                "Ctrl+Shift+middle drag is not bound: Autodesk documents it both as zoom and as zoom to a box.",
            ),
            limit(
                LimitKind::Optional,
                "Fusion's Mac trackpad gestures (two-finger pan, Shift+two-finger orbit, pinch zoom) are a separate trackpad profile.",
            ),
            limit(LimitKind::Note, "Look At is the Normal to command; Fusion documents no default key for it."),
            limit(
                LimitKind::Note,
                "Fusion does not document its wheel direction; the wheel direction setting applies.",
            ),
        ],
    }
}

fn solidworks() -> Preset {
    let mut bindings = vec![
        Binding::drag("orbit", &[M], Mods::NONE, Action::Orbit, 10),
        Binding::drag("roll", &[M], Mods::ALT, Action::Roll, 20).needs(Capability::CameraRoll),
        Binding::drag("pan", &[M], Mods::CTRL, Action::Pan, 21),
        Binding::drag("zoom", &[M], Mods::SHIFT, Action::DragZoom, 22).anchored(ZoomAnchor::Center),
        Binding::key("fit", Key::F, Mods::NONE, Action::Fit, 30),
        Binding::key("normal", Key::Num8, Mods::CTRL, Action::NormalTo, 31),
    ];
    bindings.extend(zoom_rows());
    Preset {
        id: "solidworks",
        label: "SOLIDWORKS",
        release_scope: "SOLIDWORKS 2025 and 2026 default mouse controls and view keys.",
        wheel: WheelPolicy::SoliddraftDefault,
        bindings,
        limitations: vec![
            limit(
                LimitKind::NeedsCameraRoll,
                "Alt+middle drag rolls the view in SOLIDWORKS. SolidDraft's camera cannot roll yet, so it does nothing.",
            ),
            limit(
                LimitKind::Note,
                "SOLIDWORKS' factory wheel direction is not confirmed; the wheel direction setting applies.",
            ),
        ],
    }
}

fn onshape() -> Preset {
    let mut bindings = vec![
        Binding::drag("orbit", &[R], Mods::NONE, Action::Orbit, 10),
        Binding::drag("pan", &[M], Mods::NONE, Action::Pan, 11),
        Binding::drag("pan_ctrl_right", &[R], Mods::CTRL, Action::Pan, 20),
        Binding::double_click("fit_double_click", M, Action::Fit, 30),
        Binding::key("fit", Key::F, Mods::NONE, Action::Fit, 31),
        Binding::key("normal", Key::N, Mods::NONE, Action::NormalToggle, 32),
    ];
    bindings.extend(zoom_rows());
    Preset {
        id: "onshape",
        label: "Onshape",
        release_scope: "Onshape's documented mouse controls and view keys.",
        wheel: WheelPolicy::Explicit { forward: ZoomDirection::In },
        bindings,
        limitations: vec![
            limit(
                LimitKind::Note,
                "Ctrl is the physical Control key on every platform, as Onshape documents, macOS included.",
            ),
            limit(
                LimitKind::Note,
                "A right click without a drag stays free for a context menu; only a right drag orbits.",
            ),
            limit(LimitKind::SelectionUnverified, "Onshape's click-to-add selection is not part of this preset."),
        ],
    }
}

fn inventor() -> Preset {
    let mut bindings = vec![
        Binding::drag("pan", &[M], Mods::NONE, Action::Pan, 10),
        Binding::drag("orbit", &[M], Mods::SHIFT, Action::Orbit, 20),
        Binding::drag("orbit_f4", &[L], Mods::NONE, Action::Orbit, 21).held(&[Key::F4]),
        Binding::drag("pan_f2", &[L], Mods::NONE, Action::Pan, 22).held(&[Key::F2]),
        Binding::drag("zoom_f3", &[L], Mods::NONE, Action::DragZoom, 23).held(&[Key::F3]),
        Binding::key("fit", Key::Home, Mods::NONE, Action::Fit, 30),
        Binding::key("look_at", Key::PageUp, Mods::NONE, Action::NormalTo, 31),
        Binding::key("home_view", Key::F6, Mods::NONE, Action::Isometric, 32),
    ];
    bindings.extend(zoom_rows());
    Preset {
        id: "inventor",
        label: "Inventor",
        release_scope: "Autodesk Inventor's documented mouse controls and view keys.",
        wheel: WheelPolicy::Explicit { forward: ZoomDirection::Out },
        bindings,
        limitations: vec![limit(
            LimitKind::Note,
            "F2, F3 and F4 navigate only while held during a left drag; a tap keeps its own command, so F2 still renames.",
        )],
    }
}

fn creo() -> Preset {
    let mut bindings = vec![
        Binding::drag("orbit", &[M], Mods::NONE, Action::Orbit, 10),
        Binding::drag("pan", &[M], Mods::SHIFT, Action::Pan, 20),
        Binding::drag("zoom", &[M], Mods::CTRL, Action::DragZoom, 21).along(DragAxis::Vertical),
        Binding::drag("roll", &[M], Mods::CTRL, Action::Roll, 22)
            .along(DragAxis::Horizontal)
            .needs(Capability::CameraRoll),
        Binding::key("default_view", Key::D, Mods::CTRL, Action::DefaultView, 30),
    ];
    bindings.extend(zoom_rows());
    Preset {
        id: "creo",
        label: "Creo Parametric",
        release_scope: "Creo Parametric 12's documented mouse controls.",
        wheel: WheelPolicy::SoliddraftDefault,
        bindings,
        limitations: vec![
            limit(
                LimitKind::NeedsCameraRoll,
                "A horizontal Ctrl+middle drag turns the view in Creo. SolidDraft's camera cannot roll yet, so it does nothing.",
            ),
            limit(
                LimitKind::HistoricalBinding,
                "Ctrl+D for the default view comes from the Creo 9 quick reference card.",
            ),
            limit(LimitKind::Note, "Refit and View Normal are commands; Creo documents no factory key for them."),
        ],
    }
}

fn nx12() -> Preset {
    let mut bindings = vec![
        Binding::drag("orbit", &[M], Mods::NONE, Action::Orbit, 10),
        Binding::drag("pan", &[M], Mods::SHIFT, Action::Pan, 20),
        Binding::drag("zoom", &[M], Mods::CTRL, Action::DragZoom, 21),
        Binding::drag("pan_middle_right", &[M, R], Mods::NONE, Action::Pan, 22),
        Binding::drag("zoom_left_middle", &[L, M], Mods::NONE, Action::DragZoom, 23),
        Binding::key("normal", Key::F8, Mods::NONE, Action::NormalOrNearest, 30),
        Binding::key("isometric", Key::End, Mods::NONE, Action::Isometric, 31),
    ];
    bindings.extend(zoom_rows());
    Preset {
        id: "nx12",
        label: "NX 12",
        release_scope: "Siemens NX 12 compatibility, from NX 12 teaching material.",
        wheel: WheelPolicy::SoliddraftDefault,
        bindings,
        limitations: vec![limit(
            LimitKind::HistoricalProfile,
            "Based on NX 12 documentation. Current NX releases are not certified.",
        )],
    }
}

fn solid_edge() -> Preset {
    let mut bindings = vec![
        Binding::drag("orbit", &[M], Mods::NONE, Action::Orbit, 10),
        Binding::drag("pan", &[M], Mods::SHIFT, Action::Pan, 20),
        Binding::drag("zoom_ctrl_right", &[R], Mods::CTRL, Action::DragZoom, 21),
        Binding::drag("pan_ctrl_shift_right", &[R], Mods::CTRL | Mods::SHIFT, Action::Pan, 30),
        Binding::double_click("fit_double_click", M, Action::Fit, 40),
    ];
    bindings.extend(zoom_rows());
    Preset {
        id: "solid_edge",
        label: "Solid Edge",
        release_scope: "Solid Edge's documented mouse controls.",
        wheel: WheelPolicy::SoliddraftDefault,
        bindings,
        limitations: vec![
            limit(
                LimitKind::SelectionUnverified,
                "Selection is SolidDraft's; Solid Edge's selection rules in the graphics area are not verified.",
            ),
            limit(LimitKind::Note, "Look At Face is a command; Solid Edge documents no generic key for it."),
        ],
    }
}

fn blender() -> Preset {
    let mut bindings = vec![
        Binding::drag("orbit", &[M], Mods::NONE, Action::Orbit, 10),
        Binding::drag("pan", &[M], Mods::SHIFT, Action::Pan, 20),
        Binding::drag("zoom", &[M], Mods::CTRL, Action::DragZoom, 21),
        Binding::key("fit", Key::Home, Mods::NONE, Action::Fit, 30),
        Binding::key("fit_selected", Key::Period, Mods::NONE, Action::FitSelected, 31)
            .on_numpad()
            .needs(Capability::NumpadLocation),
    ];
    bindings.extend(zoom_rows());
    Preset {
        id: "blender",
        label: "Blender",
        release_scope: "Blender's default keymap, checked against the 4.5.0 keymap.",
        wheel: WheelPolicy::SoliddraftDefault,
        bindings,
        limitations: vec![limit(
            LimitKind::NeedsNumpadLocation,
            "Blender's view keys are on the numeric keypad. Where the keypad cannot be told from the number row they are unavailable, never bound to the digits.",
        )],
    }
}
