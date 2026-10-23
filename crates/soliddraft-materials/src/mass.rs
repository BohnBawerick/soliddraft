//! Mass from density.
//!
//! The kernel gives a body's volume, centroid and inertia at unit density. A material's
//! density turns them into a mass: volume in mm³ times density in kg/m³ times 1e-9
//! gives kg, and the inertia scales the same way, to kg·mm². A part without a material,
//! or whose material has no density, has an unknown mass, never a guessed one; an
//! assembly that holds such a part has an unknown mass too.
//!
//! [`combine`] adds placed masses with their rotation and the parallel-axis shift, for
//! assemblies.

use serde::Serialize;
use soliddraft_kernel_api::MassProperties;

use crate::material::Material;

/// kg/m³ to kg/mm³.
const PER_MM3: f64 = 1e-9;

/// The mass properties of a body or an assembly, at a known density.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Mass {
    /// Mass, kg.
    pub mass_kg: f64,
    /// Volume, mm³.
    pub volume_mm3: f64,
    /// Centre of mass, mm, in the part's (or assembly's) coordinates.
    pub centre_mm: [f64; 3],
    /// Inertia tensor about the centre of mass, kg·mm². Absent when unavailable from the kernel.
    pub inertia_kg_mm2: Option<[[f64; 3]; 3]>,
}

/// Why a mass is unknown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum Unknown {
    /// The part has no material.
    NoMaterial,
    /// The part's material has no density.
    NoDensity {
        /// The material's name.
        material: String,
    },
    /// Some parts of an assembly have an unknown mass.
    Parts {
        /// How many.
        count: usize,
    },
}

impl std::fmt::Display for Unknown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unknown::NoMaterial => f.write_str("assign a material"),
            Unknown::NoDensity { material } => write!(f, "{material} has no density"),
            Unknown::Parts { count: 1 } => f.write_str("one part has an unknown mass"),
            Unknown::Parts { count } => write!(f, "{count} parts have an unknown mass"),
        }
    }
}

/// The mass of a body made of `material`, or why it is unknown.
pub fn of_body(props: &MassProperties, material: Option<&Material>) -> Result<Mass, Unknown> {
    let material = material.ok_or(Unknown::NoMaterial)?;
    let density = material.density_kg_m3.ok_or_else(|| Unknown::NoDensity { material: material.name.clone() })?;
    Ok(at_density(props, density))
}

/// The mass of a body at `density_kg_m3`.
pub fn at_density(props: &MassProperties, density_kg_m3: f64) -> Mass {
    let k = density_kg_m3 * PER_MM3;
    let c = props.centroid;
    Mass {
        mass_kg: props.volume * k,
        volume_mm3: props.volume,
        centre_mm: [c.x, c.y, c.z],
        inertia_kg_mm2: props.inertia.map(|tensor| tensor.map(|row| row.map(|v| v * k))),
    }
}

/// A mass placed by a rigid transform: rotated by `rotation` (a proper rotation
/// matrix, row-major) and then moved by `translation_mm`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    /// The mass in its own coordinates.
    pub mass: Mass,
    /// Rotation, row-major.
    pub rotation: [[f64; 3]; 3],
    /// Translation, mm.
    pub translation_mm: [f64; 3],
}

/// The total of several placed masses: mass and volume add, the centre is the
/// mass-weighted mean, and each inertia is rotated into the common axes and moved to
/// the common centre (the parallel-axis theorem). Unknown masses make the total
/// unknown, and say how many there were. An empty list has zero mass at the origin.
pub fn combine(items: impl IntoIterator<Item = Result<Placed, Unknown>>) -> Result<Mass, Unknown> {
    let mut known = Vec::new();
    let mut unknown = 0;
    for item in items {
        match item {
            Ok(p) => known.push(p),
            Err(_) => unknown += 1,
        }
    }
    if unknown > 0 {
        return Err(Unknown::Parts { count: unknown });
    }
    // Each mass with its centre and inertia in the common axes.
    let placed: Vec<Mass> = known
        .iter()
        .map(|p| {
            let r = p.rotation;
            Mass {
                centre_mm: add(mul_vec(r, p.mass.centre_mm), p.translation_mm),
                inertia_kg_mm2: p.mass.inertia_kg_mm2.map(|tensor| mul(mul(r, tensor), transpose(r))),
                ..p.mass
            }
        })
        .collect();
    let total: f64 = placed.iter().map(|p| p.mass_kg).sum();
    let volume: f64 = placed.iter().map(|p| p.volume_mm3).sum();
    let centre = if total > 0.0 {
        let mut c = [0.0; 3];
        for p in &placed {
            for (k, ck) in c.iter_mut().enumerate() {
                *ck += p.mass_kg * p.centre_mm[k] / total;
            }
        }
        c
    } else {
        [0.0; 3]
    };
    let mut inertia = [[0.0; 3]; 3];
    for p in &placed {
        let Some(tensor) = p.inertia_kg_mm2 else {
            return Ok(Mass { mass_kg: total, volume_mm3: volume, centre_mm: centre, inertia_kg_mm2: None });
        };
        let d = [p.centre_mm[0] - centre[0], p.centre_mm[1] - centre[1], p.centre_mm[2] - centre[2]];
        let d2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
        for a in 0..3 {
            for b in 0..3 {
                let delta = if a == b { 1.0 } else { 0.0 };
                inertia[a][b] += tensor[a][b] + p.mass_kg * (d2 * delta - d[a] * d[b]);
            }
        }
    }
    Ok(Mass { mass_kg: total, volume_mm3: volume, centre_mm: centre, inertia_kg_mm2: Some(inertia) })
}

fn mul(a: [[f64; 3]; 3], b: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut out = [[0.0; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

fn transpose(a: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut out = [[0.0; 3]; 3];
    for (i, row) in a.iter().enumerate() {
        for (j, v) in row.iter().enumerate() {
            out[j][i] = *v;
        }
    }
    out
}

fn mul_vec(a: [[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    a.map(|row| row[0] * v[0] + row[1] * v[1] + row[2] * v[2])
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

#[cfg(test)]
mod tests {
    use super::*;
    use soliddraft_kernel_api::Vec3;

    const IDENTITY: [[f64; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

    /// A 10 x 20 x 30 mm box centred at the origin, at unit density.
    fn block() -> MassProperties {
        let v = 10.0 * 20.0 * 30.0;
        let (a, b, c): (f64, f64, f64) = (10.0, 20.0, 30.0);
        MassProperties {
            volume: v,
            area: 2.0 * (a * b + b * c + a * c),
            centroid: Vec3::new(0.0, 0.0, 0.0),
            inertia: Some([
                [v * (b * b + c * c) / 12.0, 0.0, 0.0],
                [0.0, v * (a * a + c * c) / 12.0, 0.0],
                [0.0, 0.0, v * (a * a + b * b) / 12.0],
            ]),
        }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1e-12)
    }

    #[test]
    fn density_gives_mass_and_unknown_stays_unknown() {
        let mut steel = Material::new("steel", "Steel");
        steel.density_kg_m3 = Some(7850.0);
        let m = of_body(&block(), Some(&steel)).unwrap();
        // 6000 mm³ of steel: 6e-6 m³ x 7850 kg/m³ = 0.0471 kg.
        assert!(close(m.mass_kg, 0.0471), "{}", m.mass_kg);
        // Ixx = m (b² + c²) / 12 = 0.0471 x 1300 / 12 kg·mm².
        assert!(close(m.inertia_kg_mm2.unwrap()[0][0], 0.0471 * 1300.0 / 12.0));
        assert_eq!(of_body(&block(), None), Err(Unknown::NoMaterial));
        let oak = Material::new("oak", "Oak");
        assert_eq!(of_body(&block(), Some(&oak)).unwrap_err().to_string(), "Oak has no density");
    }

    #[test]
    fn a_recorded_source_leaves_the_mass_as_it_was() {
        use crate::provenance::{Property, PropertySource, Provenance};
        let mut steel = Material::new("steel", "Steel");
        steel.density_kg_m3 = Some(7850.0);
        let before = of_body(&block(), Some(&steel)).unwrap();
        let handbook = PropertySource::Reference { citation: "MIL-HDBK-5J".into() };
        steel.set_value(Property::Density, Some(7850.0), Provenance::from_source(handbook));
        assert_eq!(of_body(&block(), Some(&steel)).unwrap(), before);
    }

    #[test]
    fn absent_inertia_stays_absent_through_density_and_assembly_transforms() {
        let mut props = block();
        props.inertia = None;
        let mass = at_density(&props, 7850.0);
        assert!(mass.inertia_kg_mm2.is_none());
        let placed = Placed { mass, rotation: IDENTITY, translation_mm: [100.0, 20.0, 0.0] };
        let combined = combine([Ok(placed), Ok(Placed { mass: at_density(&block(), 7850.0), ..placed })]).unwrap();
        assert!(combined.mass_kg > mass.mass_kg);
        assert!(combined.inertia_kg_mm2.is_none());
    }

    #[test]
    fn combining_rotates_and_shifts_inertia() {
        let m = at_density(&block(), 1e9); // 1 kg/mm³: mass 6000 kg
        // A quarter turn about z swaps the x and y moments.
        let quarter = [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
        let turned = combine([Ok(Placed { mass: m, rotation: quarter, translation_mm: [0.0; 3] })]).unwrap();
        assert!(close(turned.inertia_kg_mm2.unwrap()[0][0], m.inertia_kg_mm2.unwrap()[1][1]));
        assert!(close(turned.inertia_kg_mm2.unwrap()[1][1], m.inertia_kg_mm2.unwrap()[0][0]));
        // Two blocks 100 mm apart along x: centre between them, Iyy and Izz gain
        // 2 m (50 mm)², Ixx unchanged.
        let a = Placed { mass: m, rotation: IDENTITY, translation_mm: [0.0, 0.0, 0.0] };
        let b = Placed { mass: m, rotation: IDENTITY, translation_mm: [100.0, 0.0, 0.0] };
        let pair = combine([Ok(a), Ok(b)]).unwrap();
        assert!(close(pair.mass_kg, 12000.0));
        assert_eq!(pair.centre_mm, [50.0, 0.0, 0.0]);
        assert!(close(pair.inertia_kg_mm2.unwrap()[0][0], 2.0 * m.inertia_kg_mm2.unwrap()[0][0]));
        assert!(close(
            pair.inertia_kg_mm2.unwrap()[1][1],
            2.0 * m.inertia_kg_mm2.unwrap()[1][1] + 2.0 * 6000.0 * 2500.0
        ));
        assert_eq!(pair.inertia_kg_mm2.unwrap()[0][1], 0.0);
        assert_eq!(
            combine([Ok(a), Err(Unknown::NoMaterial), Err(Unknown::NoMaterial)]),
            Err(Unknown::Parts { count: 2 })
        );
        assert_eq!(combine([]).unwrap().mass_kg, 0.0);
    }
}
