//! Render settings: the three looks (Fast, Studio, Realistic) and the capability tier
//! of the device they run on. A preset is a bundle of lighting values the renderer
//! reads each frame; the tier decides which passes the device can run at all.

use egui_wgpu::wgpu;
use serde::{Deserialize, Serialize};

/// How much effort the viewport spends on the look. Studio is the default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    /// Colour, environment lighting and edges; no shadows or occlusion.
    Fast,
    /// Fast plus soft shadows from the fixed light and light ambient occlusion.
    #[default]
    Studio,
    /// Studio with a stronger shadow light, stronger reflections and occlusion.
    Realistic,
}

impl Preset {
    pub const ALL: [Preset; 3] = [Preset::Fast, Preset::Studio, Preset::Realistic];

    pub fn label(&self) -> &'static str {
        match self {
            Preset::Fast => "Fast",
            Preset::Studio => "Studio",
            Preset::Realistic => "Realistic",
        }
    }

    /// Parses a preset name as typed on the command line or in a page address.
    pub fn parse(name: &str) -> Option<Preset> {
        Preset::ALL.into_iter().find(|p| p.label().eq_ignore_ascii_case(name.trim()))
    }

    /// Whether the preset draws edges unless the user asks for them. Realistic is for
    /// presentation and leaves them off.
    pub fn shows_edges(&self) -> bool {
        !matches!(self, Preset::Realistic)
    }

    /// The lighting values of this preset on a device of `tier`.
    pub fn look(&self, tier: Tier) -> Look {
        let ao = tier.has_ambient_occlusion();
        match self {
            Preset::Fast => Look {
                shadows: false,
                shadow_strength: 0.0,
                shadow_softness: 0.0,
                ambient_occlusion: 0.0,
                environment: 0.55,
                reflections: 1.0,
                rig: 1.0,
                sun: 0.0,
            },
            Preset::Studio => Look {
                shadows: true,
                shadow_strength: 0.45,
                shadow_softness: 1.0,
                ambient_occlusion: if ao { 0.6 } else { 0.0 },
                environment: 0.36,
                reflections: 1.0,
                rig: 0.85,
                sun: 0.95,
            },
            Preset::Realistic => Look {
                shadows: true,
                shadow_strength: 0.7,
                shadow_softness: 0.6,
                ambient_occlusion: if ao { 1.0 } else { 0.0 },
                environment: 0.3,
                reflections: 1.6,
                rig: 0.7,
                sun: 1.25,
            },
        }
    }
}

/// Lighting values the renderer reads each frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// Whether the fixed light casts shadows on the ground and the parts.
    pub shadows: bool,
    /// How dark a fully shadowed spot on the ground gets, 0 to 1.
    pub shadow_strength: f32,
    /// Filter radius of the shadow edge, relative to Studio's.
    pub shadow_softness: f32,
    /// Strength of the screen-space ambient occlusion, 0 for none.
    pub ambient_occlusion: f32,
    /// Scale of the light from the environment.
    pub environment: f32,
    /// Scale of the environment's specular reflections.
    pub reflections: f32,
    /// Scale of the studio rig, the key, fill and rim lights that follow the camera.
    pub rig: f32,
    /// Intensity of the fixed light above the model; 0 leaves the studio rig alone.
    pub sun: f32,
}

/// What the device can run. WebGL2 has no compute, no storage buffers and one-pixel
/// lines; everything the viewport draws has to work there, and the extra passes run
/// only on the full tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// WebGL2 or native OpenGL ES: shading, shadows and wide lines, no occlusion pass.
    Basic,
    /// WebGPU, Vulkan, Metal or Direct3D 12: every pass.
    Full,
}

impl Tier {
    pub fn of(backend: wgpu::Backend) -> Tier {
        match backend {
            wgpu::Backend::Gl | wgpu::Backend::Noop => Tier::Basic,
            _ => Tier::Full,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Tier::Basic => "basic (WebGL2)",
            Tier::Full => "full",
        }
    }

    pub fn has_ambient_occlusion(&self) -> bool {
        *self == Tier::Full
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn studio_is_the_default_and_names_parse() {
        assert_eq!(Preset::default(), Preset::Studio);
        for p in Preset::ALL {
            assert_eq!(Preset::parse(&p.label().to_lowercase()), Some(p));
        }
        assert_eq!(Preset::parse("cinema"), None);
    }

    #[test]
    fn the_basic_tier_drops_occlusion_and_keeps_shadows() {
        let full = Preset::Studio.look(Tier::Full);
        let basic = Preset::Studio.look(Tier::Basic);
        assert!(full.ambient_occlusion > 0.0);
        assert_eq!(basic.ambient_occlusion, 0.0);
        assert!(basic.shadows);
        assert!(!Preset::Fast.look(Tier::Full).shadows);
        assert_eq!(Tier::of(wgpu::Backend::Gl), Tier::Basic);
        assert_eq!(Tier::of(wgpu::Backend::BrowserWebGpu), Tier::Full);
    }

    #[test]
    fn realistic_is_stronger_than_studio() {
        let (s, r) = (Preset::Studio.look(Tier::Full), Preset::Realistic.look(Tier::Full));
        assert!(r.shadow_strength > s.shadow_strength);
        assert!(r.reflections > s.reflections);
        assert!(r.ambient_occlusion > s.ambient_occlusion);
        assert!(!Preset::Realistic.shows_edges());
    }
}
