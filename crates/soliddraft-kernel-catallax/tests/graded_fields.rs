//! The V7d graded fields through the kernel interface the application uses: the six
//! designed states D0-D2 and G0-G2 over sketch-and-extrude regions, each with its native
//! field step, certified labelled export, field moments and mass stand-in, every action
//! within one application allowance. Expected values are the V7d design's independent
//! references; none is read from the kernel.

use catallax_numbers::{Rational, with_operation_budget};
use serde_json::{Value, json};
use soliddraft_kernel_api::*;
use soliddraft_kernel_catallax::CatallaxKernel;

/// One complete application action stays below 20,000,000 units.
const APP_LIMIT: u64 = 19_999_999;

fn q(text: &str) -> Rational {
    Rational::parse(text).unwrap()
}

/// A closed rational interval.
#[derive(Clone, Debug)]
struct Interval {
    lo: Rational,
    hi: Rational,
}

impl Interval {
    fn exact(value: Rational) -> Self {
        Interval { lo: value.clone(), hi: value }
    }
    fn of(range: &ExactRange) -> Self {
        Interval { lo: q(&range.lo), hi: q(&range.hi) }
    }
    fn parse(lo: &str, hi: &str) -> Self {
        Interval { lo: q(lo), hi: q(hi) }
    }
    fn width(&self) -> Rational {
        &self.hi - &self.lo
    }
    fn contains(&self, inner: &Interval) -> bool {
        self.lo <= inner.lo && inner.hi <= self.hi
    }
    fn meets(&self, other: &Interval) -> bool {
        self.lo <= other.hi && other.lo <= self.hi
    }
    fn add(&self, other: &Interval) -> Interval {
        Interval { lo: &self.lo + &other.lo, hi: &self.hi + &other.hi }
    }
    fn sub(&self, other: &Interval) -> Interval {
        Interval { lo: &self.lo - &other.hi, hi: &self.hi - &other.lo }
    }
    fn mul(&self, other: &Interval) -> Interval {
        let products = [&self.lo * &other.lo, &self.lo * &other.hi, &self.hi * &other.lo, &self.hi * &other.hi];
        Interval { lo: products.iter().min().unwrap().clone(), hi: products.iter().max().unwrap().clone() }
    }
    fn half(&self) -> Interval {
        let half = q("1/2");
        Interval { lo: &self.lo * &half, hi: &self.hi * &half }
    }
    fn scale(&self, factor: &Rational) -> Interval {
        Interval { lo: &self.lo * factor, hi: &self.hi * factor }
    }
}

/// `coefficient` times pi, enclosed by pi's sixteen-digit decimal bounds.
fn times_pi(coefficient: &str) -> Interval {
    Interval::parse("3141592653589793/1000000000000000", "3141592653589794/1000000000000000").scale(&q(coefficient))
}

/// Where a designed state's region and field run: `cycle` 0 is the design's z axis; 1
/// and 2 apply the cyclic permutation (x, y, z) -> (z, x, y) once and twice.
#[derive(Clone, Copy)]
struct Cycle(usize);

impl Cycle {
    /// The new index of old coordinate `i`.
    fn axis(self, i: usize) -> usize {
        (i + self.0) % 3
    }
    fn grade_axis(self) -> GradeAxis {
        GradeAxis::ALL[self.axis(2)]
    }
    /// A rectangle in the plane across the cycled z axis at `start`, extruded `depth`
    /// along it. `widths` are the extents along the design's x and y.
    fn region(self, k: &CatallaxKernel, tag: &str, widths: [f64; 2], start: f64, depth: f64) -> BodyId {
        let (plane, width, height) = match self.0 {
            0 => (Plane::XY.with_origin(Vec3::new(0.0, 0.0, start)), widths[0], widths[1]),
            1 => (Plane::YZ.with_origin(Vec3::new(start, 0.0, 0.0)), widths[0], widths[1]),
            _ => (Plane::ZX.with_origin(Vec3::new(0.0, start, 0.0)), widths[0], widths[1]),
        };
        let sheet = k.create_body_from_profile(
            &OpTag::new(tag),
            &Profile::rectangle(plane, Point2::new(0.0, 0.0), width, height),
        );
        let sheet = sheet.unwrap().body;
        let body = k.extrude(&OpTag::new(tag), sheet, &Extrude { distance: depth, direction: None }).unwrap().body;
        k.release(sheet).unwrap();
        body
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Family {
    Lattice,
    Gyroid,
}

/// One designed state: its region along the design's z axis and its grade.
struct State {
    name: &'static str,
    family: Family,
    widths: [f64; 2],
    start: f64,
    depth: f64,
    grade: LinearGrade,
}

fn d(name: &'static str, depth: f64, start: f64, end: f64) -> State {
    State {
        name,
        family: Family::Lattice,
        widths: [6.0, 6.0],
        start: 20.0,
        depth,
        grade: LinearGrade { axis: GradeAxis::Z, from: 20.0, to: 30.0, start, end },
    }
}

fn g(name: &'static str, x_width: f64, start: f64, end: f64) -> State {
    State {
        name,
        family: Family::Gyroid,
        widths: [x_width, 2.0],
        start: -1.0,
        depth: 2.0,
        grade: LinearGrade { axis: GradeAxis::Z, from: -1.0, to: 1.0, start, end },
    }
}

fn states() -> [State; 6] {
    [
        d("D0", 10.0, 1.0, 2.0),
        d("D1", 12.0, 1.0, 2.0),
        d("D2", 10.0, 2.0, 1.0),
        g("G0", 2.0, -0.1, 0.1),
        g("G1", 2.4, -0.1, 0.1),
        g("G2", 2.0, 0.1, -0.1),
    ]
}

/// Builds a state, cycled, at `accuracy` and the design's 0.1 mm minimum detail.
fn build(k: &CatallaxKernel, state: &State, cycle: Cycle, accuracy: f64) -> KernelResult<OpResult> {
    let start = state.start;
    let region = cycle.region(k, state.name, state.widths, start, state.depth);
    let grade = Grade::Linear(LinearGrade { axis: cycle.grade_axis(), ..state.grade });
    let tag = OpTag::new(format!("{}.field", state.name));
    match state.family {
        Family::Lattice => k.lattice(
            &tag,
            region,
            &Lattice {
                cell: LatticeCell::Cubic,
                cell_size_mm: 100.0,
                radius_mm: grade,
                frame: FieldFrame::Part,
                accuracy_mm: accuracy,
                min_detail_mm: 0.1,
            },
        ),
        Family::Gyroid => k.gyroid(
            &tag,
            region,
            &Gyroid {
                kind: GyroidKind::Network,
                period_mm: 10.0,
                level: grade,
                frame: FieldFrame::Part,
                accuracy_mm: accuracy,
                min_detail_mm: 0.1,
            },
        ),
    }
}

/// The field step of a built body's native recipe.
fn field_step(k: &CatallaxKernel, body: BodyId) -> Value {
    let recipe = k.body_recipe(body).unwrap();
    let result = recipe["result"].clone();
    recipe["steps"].as_array().unwrap().iter().find(|s| s["id"] == result).unwrap().clone()
}

/// Every number spelled as its exact rational, so "0.1" and "1/10" compare equal.
fn canonical(value: &Value) -> Value {
    match value {
        Value::String(text) => Rational::parse(text).map_or_else(|_| value.clone(), |r| json!(r.to_string())),
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        Value::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), canonical(v))).collect()),
        other => other.clone(),
    }
}

/// The design's field steps of D0-D2 and G0-G2, as its recipes spell them.
fn designed_step(name: &str) -> Value {
    let identity = json!({"rotation":["1","0","0","0"],"translation":["0","0","0"]});
    let lattice = |start: &str, end: &str| {
        json!({"id":"lattice","op":"lattice","cell":"cubic","size":"100","frame":identity,
            "radius":{"linear":{"from":["0","0","20"],"to":["0","0","30"],"start":start,"end":end}}})
    };
    let gyroid = |start: &str, end: &str| {
        json!({"id":"gyroid","op":"gyroid","period":"10","kind":"network","frame":identity,
            "level":{"linear":{"from":["0","0","-1"],"to":["0","0","1"],"start":start,"end":end}}})
    };
    match name {
        "D0" | "D1" => lattice("1", "2"),
        "D2" => lattice("2", "1"),
        "G0" | "G1" => gyroid("-1/10", "1/10"),
        "G2" => gyroid("1/10", "-1/10"),
        _ => unreachable!("{name}"),
    }
}

#[test]
fn designed_states_compile_the_designed_native_field_steps() {
    for state in states() {
        let k = CatallaxKernel::new();
        let body = build(&k, &state, Cycle(0), 0.1).unwrap().body;
        let mut step = field_step(&k, body);
        // The region is the app's sketch and extrusion, not the design's stock block.
        step.as_object_mut().unwrap().remove("region");
        assert_eq!(canonical(&step), canonical(&designed_step(state.name)), "{}", state.name);
    }
}

/// The label fields of a certified field STL, read from the bytes.
fn header(bytes: &[u8]) -> (bool, u8, (i64, u64), (u64, u64), u32) {
    let n = i64::from_le_bytes(bytes[12..20].try_into().unwrap());
    let d = u64::from_le_bytes(bytes[20..28].try_into().unwrap());
    let cells = u64::from_le_bytes(bytes[28..36].try_into().unwrap());
    let vertices = u64::from_le_bytes(bytes[36..44].try_into().unwrap());
    let count = u32::from_le_bytes(bytes[80..84].try_into().unwrap());
    (&bytes[..8] == b"CXPRINT1", bytes[8], (n, d), (cells, vertices), count)
}

#[test]
fn designed_states_export_certified_labelled_meshes_within_the_app_allowance() {
    for state in states() {
        let accuracies: &[(f64, (i64, u64))] =
            if matches!(state.name, "D0" | "G0") { &[(0.1, (1, 10)), (0.5, (1, 2))] } else { &[(0.1, (1, 10))] };
        for &(accuracy, exact) in accuracies {
            let k = CatallaxKernel::new();
            let body = build(&k, &state, Cycle(0), accuracy).unwrap().body;
            let out = k.export_field_stl(body).unwrap();
            let r = &out.report;
            let label = format!("{} at {accuracy}", state.name);
            assert_eq!(r.label, "catallax-print-stl/1", "{label}");
            assert_eq!((r.route.as_str(), r.approximate), ("field", true), "{label}");
            assert_eq!(r.requested_accuracy_mm, format!("{}/{}", exact.0, exact.1), "{label}");
            assert_eq!(r.minimum_detail_mm, "1/10", "{label}");
            assert!(r.closed && r.non_self_intersecting && r.connected && r.components == 1, "{label}: {r:?}");
            assert!(0.0 < r.deviation_bound_mm && r.deviation_bound_mm <= accuracy, "{label}: {r:?}");
            assert_eq!((r.max_cells, r.max_vertices), (2_000_000, 1_000_000), "{label}");
            assert!(0 < r.work && r.work <= APP_LIMIT, "{label}: {} work", r.work);
            let (magic, route, requested, caps, count) = header(&out.bytes);
            assert!(magic && route == 2, "{label}");
            assert_eq!(requested, exact, "{label}");
            assert_eq!(caps, (2_000_000, 1_000_000), "{label}");
            assert_eq!((count as usize, out.bytes.len()), (r.triangles, 84 + 50 * r.triangles), "{label}");
            assert_eq!(f64::from_le_bytes(out.bytes[68..76].try_into().unwrap()), r.deviation_bound_mm, "{label}");
            use sha2::Digest;
            assert_eq!(format!("{:x}", sha2::Sha256::digest(&out.bytes)), r.sha256, "{label}");
        }
    }
}

/// The designed moments of one state, in the design's frame.
struct Expected {
    volume: Interval,
    centroid: [Interval; 3],
    /// Exact references contain the inertia (D) or overlap it (G).
    inertia: [[Interval; 3]; 3],
    /// G only: the exact raw second moments, diagonal first, then xy, xz, yz.
    raw: Option<[Rational; 6]>,
}

fn expected(name: &str) -> Expected {
    let zero = || Interval::exact(Rational::zero());
    let d = |v: &str, cz: &str, ixx: &str, izz: &str| Expected {
        volume: times_pi(v),
        centroid: [zero(), zero(), Interval::exact(q(cz))],
        inertia: [[times_pi(ixx), zero(), zero()], [zero(), times_pi(ixx), zero()], [zero(), zero(), times_pi(izz)]],
        raw: None,
    };
    let g = |v: &str, raw: [&str; 3], rows: [[&str; 2]; 9]| {
        let r = |i: usize| Interval::parse(rows[i][0], rows[i][1]);
        Expected {
            volume: Interval::exact(q(v)),
            centroid: [r(0), r(1), r(2)],
            inertia: [[r(3), r(6), r(7)], [r(6), r(4), r(8)], [r(7), r(8), r(5)]],
            raw: Some([q(raw[0]), q(raw[1]), q(raw[2]), Rational::zero(), Rational::zero(), Rational::zero()]),
        }
    };
    match name {
        "D0" => d("70/3", "365/14", "1321/7", "31"),
        "D1" => d("94/3", "2569/94", "48517/141", "47"),
        "D2" => d("70/3", "335/14", "1321/7", "31"),
        "G0" => g(
            "4",
            ["4/3", "4/3", "4/3"],
            [
                ["-7323723/25000000", "-140831983/500000000"],
                ["-293376209/1000000000", "-282357203/1000000000"],
                ["-118964903/500000000", "-113220591/500000000"],
                ["2095945899/1000000000", "2142661869/1000000000"],
                ["2096948019/1000000000", "2144225871/1000000000"],
                ["1979111989/1000000000", "2030425947/1000000000"],
                ["159059699/500000000", "171888487/500000000"],
                ["127560643/500000000", "139402559/500000000"],
                ["63937299/250000000", "279211777/1000000000"],
            ],
        ),
        "G1" => g(
            "24/5",
            ["288/125", "8/5", "8/5"],
            [
                ["-10000177/25000000", "-386465873/1000000000"],
                ["-268183199/1000000000", "-128022813/500000000"],
                ["-43057467/200000000", "-203326091/1000000000"],
                ["52645997/20000000", "1343437931/500000000"],
                ["2913499361/1000000000", "1494326311/500000000"],
                ["2790746121/1000000000", "2872406877/1000000000"],
                ["29685869/62500000", "257460427/500000000"],
                ["377177257/1000000000", "206679499/500000000"],
                ["24989163/100000000", "277134941/1000000000"],
            ],
        ),
        "G2" => g(
            "4",
            ["4/3", "4/3", "4/3"],
            [
                ["-25674223/100000000", "-49246889/200000000"],
                ["-256756431/1000000000", "-247014493/1000000000"],
                ["-6191257/20000000", "-18749169/62500000"],
                ["126228411/62500000", "2062633937/1000000000"],
                ["1009841873/500000000", "2064172967/1000000000"],
                ["2139304919/1000000000", "2180076419/1000000000"],
                ["243293907/1000000000", "131840437/500000000"],
                ["295468239/1000000000", "12716457/40000000"],
                ["148202127/500000000", "31792901/100000000"],
            ],
        ),
        _ => unreachable!("{name}"),
    }
}

/// Checks returned moments, read in the cycled frame, against the design's references.
fn check_moments(name: &str, cycle: Cycle, mass: &FieldMass) {
    let label = format!("{name} cycled {}", cycle.0);
    let reference = expected(name);
    let volume = Interval::of(&mass.volume_mm3);
    let centroid_ranges = mass.centroid_mm.as_ref().unwrap();
    let inertia_ranges = mass.inertia_mm5.as_ref().unwrap();
    // Read the returned values back into the design's frame.
    let centroid: [Interval; 3] = std::array::from_fn(|i| Interval::of(&centroid_ranges[cycle.axis(i)]));
    let inertia: [[Interval; 3]; 3] =
        std::array::from_fn(|i| std::array::from_fn(|j| Interval::of(&inertia_ranges[cycle.axis(i)][cycle.axis(j)])));
    assert_eq!(mass.scope, "original-field-definition", "{label}");
    assert_eq!(mass.requested_widths, ["1/100", "1/1000", "1/10"].map(str::to_owned), "{label}");
    assert!(volume.lo > Rational::zero(), "{label}: a positive volume is proven");
    assert!(volume.width() <= q("1/100"), "{label}: volume width {}", volume.width());
    assert!(centroid.iter().all(|c| c.width() <= q("1/1000")), "{label}: centroid {centroid:?}");
    assert!(inertia.iter().flatten().all(|c| c.width() <= q("1/10")), "{label}: inertia {inertia:?}");
    assert!(mass.work <= APP_LIMIT, "{label}: {} work", mass.work);
    match &reference.raw {
        None => {
            assert!(volume.contains(&reference.volume), "{label}: volume {volume:?}");
            for (i, (c, row)) in centroid.iter().zip(&inertia).enumerate() {
                assert!(c.contains(&reference.centroid[i]), "{label}: centroid {i} {c:?}");
                for (j, value) in row.iter().enumerate() {
                    assert!(value.contains(&reference.inertia[i][j]), "{label}: inertia {i}{j}");
                }
            }
        }
        Some(raw) => {
            assert!(volume.contains(&reference.volume), "{label}: volume {volume:?}");
            for (i, (c, row)) in centroid.iter().zip(&inertia).enumerate() {
                assert!(c.meets(&reference.centroid[i]), "{label}: centroid {i} {c:?}");
                for (j, value) in row.iter().enumerate() {
                    assert!(value.meets(&reference.inertia[i][j]), "{label}: inertia {i}{j}");
                }
            }
            // Raw second moments from the returned tensor: the sum of the other two raw
            // diagonals is the diagonal inertia plus the parallel-axis term, and a raw
            // product is the negated inertia product plus V times both coordinates.
            let p: [Interval; 3] = std::array::from_fn(|i| {
                let (j, k) = ((i + 1) % 3, (i + 2) % 3);
                inertia[i][i].add(&volume.mul(&centroid[j].mul(&centroid[j]).add(&centroid[k].mul(&centroid[k]))))
            });
            let diagonal: [Interval; 3] = std::array::from_fn(|i| {
                let (j, k) = ((i + 1) % 3, (i + 2) % 3);
                p[j].add(&p[k]).sub(&p[i]).half()
            });
            for i in 0..3 {
                assert!(diagonal[i].contains(&Interval::exact(raw[i].clone())), "{label}: raw {i} {:?}", diagonal[i]);
            }
            for (n, (i, j)) in [(0, 1), (0, 2), (1, 2)].into_iter().enumerate() {
                let product = volume.mul(&centroid[i].mul(&centroid[j])).sub(&inertia[i][j]);
                assert!(product.contains(&Interval::exact(raw[3 + n].clone())), "{label}: raw {i}{j} {product:?}");
            }
        }
    }
}

#[test]
fn designed_states_have_certified_moments_of_their_own_definition() {
    for state in states() {
        let k = CatallaxKernel::new();
        let body = build(&k, &state, Cycle(0), 0.1).unwrap().body;
        let mass = k.field_mass(body, &FieldMassAccuracy::default()).unwrap();
        check_moments(state.name, Cycle(0), &mass);
        // The display region's placeholder is never a measured property.
        assert!(k.mass_properties(body).unwrap_err().to_string().contains("explicit field mass request"));
    }
}

#[test]
fn cyclic_permutations_move_the_region_grade_and_moments_together() {
    for state in states().into_iter().filter(|s| matches!(s.name, "D0" | "G0")) {
        for cycle in [Cycle(1), Cycle(2)] {
            let k = CatallaxKernel::new();
            let body = build(&k, &state, cycle, 0.1).unwrap().body;
            let out = k.export_field_stl(body).unwrap();
            assert!(out.report.connected && out.report.work <= APP_LIMIT, "{} {}", state.name, cycle.0);
            check_moments(state.name, cycle, &k.field_mass(body, &FieldMassAccuracy::default()).unwrap());
        }
    }
}

/// Counts a STEP entity by its exact type name.
fn entities(text: &str, name: &str) -> usize {
    text.matches(&format!("={name}(")).count()
}

#[test]
fn designed_states_write_mass_stand_ins_inside_their_field_enclosures() {
    for state in states() {
        let k = CatallaxKernel::new();
        let body = build(&k, &state, Cycle(0), 0.1).unwrap().body;
        let out = k.export_field_stand_in(body, &FieldMassAccuracy::default()).unwrap();
        let r = &out.report;
        let label = state.name;
        assert_eq!(r.label, "mass stand-in — not field boundary geometry");
        assert_eq!(r.representation, "massless-surface-envelope-plus-unit-density-solids");
        assert!(!r.density_metadata);
        assert!(r.work <= APP_LIMIT, "{label}: {} work", r.work);
        check_moments(label, Cycle(0), &r.field);
        let field = Interval::of(&r.field.volume_mm3);
        assert!(field.contains(&Interval::of(&r.exchanged.volume_mm3)), "{label}");
        assert!(field.contains(&Interval::exact(q(&r.proxy.volume_mm3))), "{label}");
        let centroid = r.field.centroid_mm.as_ref().unwrap();
        let inertia = r.field.inertia_mm5.as_ref().unwrap();
        for (i, (c, row)) in centroid.iter().zip(inertia).enumerate() {
            assert!(Interval::of(c).contains(&Interval::of(&r.exchanged.centroid_mm[i])), "{label}");
            for (j, value) in row.iter().enumerate() {
                let outer = Interval::of(value);
                assert!(outer.contains(&Interval::of(&r.exchanged.inertia_mm5[i][j])), "{label} {i}{j}");
                assert!(outer.contains(&Interval::exact(q(&r.proxy.inertia_mm5[i][j]))), "{label} {i}{j}");
            }
        }
        assert!(q(&r.inertia_residual) < q("1/1000000000"), "{label}");
        let text = std::str::from_utf8(&out.bytes).unwrap();
        assert_eq!(entities(text, "SHELL_BASED_SURFACE_MODEL"), r.surface_envelopes, "{label}");
        assert_eq!(entities(text, "MANIFOLD_SOLID_BREP"), r.mass_bodies, "{label}");
        assert!(r.mass_bodies >= 1 && r.surface_envelopes == 1, "{label}");
        use sha2::Digest;
        assert_eq!(format!("{:x}", sha2::Sha256::digest(&out.bytes)), r.sha256, "{label}");
    }
}

#[test]
fn equal_endpoints_are_the_constant_and_out_of_range_grades_are_coverage_refusals() {
    // A grade with equal endpoints is its constant: the same certified bytes.
    for (state, constant) in [(d("D0", 10.0, 1.0, 1.0), 1.0), (g("G0", 2.0, 0.0, 0.0), 0.0)] {
        let k = CatallaxKernel::new();
        let graded = build(&k, &state, Cycle(0), 0.1).unwrap().body;
        let region = Cycle(0).region(&k, "constant", state.widths, state.start, state.depth);
        let tag = OpTag::new("constant");
        let constant = match state.family {
            Family::Lattice => k.lattice(
                &tag,
                region,
                &Lattice {
                    cell: LatticeCell::Cubic,
                    cell_size_mm: 100.0,
                    radius_mm: Grade::Constant(constant),
                    frame: FieldFrame::Part,
                    accuracy_mm: 0.1,
                    min_detail_mm: 0.1,
                },
            ),
            Family::Gyroid => k.gyroid(
                &tag,
                region,
                &Gyroid {
                    kind: GyroidKind::Network,
                    period_mm: 10.0,
                    level: Grade::Constant(constant),
                    frame: FieldFrame::Part,
                    accuracy_mm: 0.1,
                    min_detail_mm: 0.1,
                },
            ),
        }
        .unwrap()
        .body;
        // The constant's route at these controls may refuse; the grade then refuses alike.
        let export = |body| k.export_field_stl(body).map(|out| out.bytes).map_err(|e| e.to_string());
        assert_eq!(export(graded), export(constant), "{}", state.name);
    }
    // The design's graded-domain rows: the values pass through exactly, never clamped.
    for (state, code) in [
        (d("L-K-over", 10.0, 1.0, 6.00001), "lattice.grade-range-unsupported"),
        (d("L-radius-over", 10.0, 24.0, 25.000001), "lattice.grade-range-unsupported"),
        (g("G-K-over", 2.0, -0.1, 0.100001), "gyroid.grade-range-unsupported"),
        (g("G-level-over", 2.0, 0.24, 0.250001), "gyroid.grade-range-unsupported"),
        (d("L-zero-span", 10.0, 1.0, 2.0), "lattice.impossible"),
        (g("G-zero-span", 2.0, -0.1, 0.1), "gyroid.impossible"),
    ] {
        let k = CatallaxKernel::new();
        let state = if state.name.ends_with("zero-span") {
            State { grade: LinearGrade { to: state.grade.from, ..state.grade }, ..state }
        } else {
            state
        };
        let error = build(&k, &state, Cycle(0), 0.1).unwrap_err().to_string();
        assert!(error.contains(code), "{}: {error}", state.name);
    }
    // The edges of the certified range build.
    for state in
        [d("L-K-edge", 10.0, 1.0, 6.0), d("L-radius-edge", 10.0, 24.0, 25.0), g("G-level-edge", 2.0, 0.24, 0.25)]
    {
        let k = CatallaxKernel::new();
        build(&k, &state, Cycle(0), 0.1).unwrap_or_else(|e| panic!("{}: {e}", state.name));
    }
}

#[test]
fn a_zero_allowance_refuses_each_field_action_with_the_operation_budget() {
    let k = CatallaxKernel::new();
    let body = build(&k, &d("D0", 10.0, 1.0, 2.0), Cycle(0), 0.1).unwrap().body;
    let accuracy = FieldMassAccuracy::default();
    let (export, work) = with_operation_budget(0, || k.export_field_stl(body).map(|_| ()));
    assert!(export.unwrap_err().to_string().contains("numbers.operation-budget") && work.used == 0);
    let (mass, work) = with_operation_budget(0, || k.field_mass(body, &accuracy).map(|_| ()));
    assert!(mass.unwrap_err().to_string().contains("numbers.operation-budget") && work.used == 0);
    let (stand_in, work) = with_operation_budget(0, || k.export_field_stand_in(body, &accuracy).map(|_| ()));
    assert!(stand_in.unwrap_err().to_string().contains("numbers.operation-budget") && work.used == 0);
    // Nested in a caller's allowance, an action spends it and gets no fresh one.
    let (mass, work) = with_operation_budget(APP_LIMIT, || k.field_mass(body, &accuracy));
    assert_eq!(mass.unwrap().work, work.used);
}

#[test]
fn nothing_reads_a_terminal_field_after_it() {
    let k = CatallaxKernel::new();
    let field = build(&k, &d("D0", 10.0, 1.0, 2.0), Cycle(0), 0.1).unwrap().body;
    let tool = Cycle(0).region(&k, "tool", [2.0, 2.0], 22.0, 2.0);
    let after = |result: KernelResult<OpResult>| {
        let error = result.unwrap_err().to_string();
        assert!(error.contains("lattice.cad-after-lattice"), "{error}");
    };
    after(k.boolean(&OpTag::new("join"), BooleanOp::Union, field, tool));
    after(k.boolean(&OpTag::new("cut"), BooleanOp::Subtract, tool, field));
    after(k.pattern(
        &OpTag::new("pattern"),
        field,
        &PatternLayout::Linear { step: Vec3::new(10.0, 0.0, 0.0), count: 2 },
    ));
    let gyroid = Gyroid {
        kind: GyroidKind::Network,
        period_mm: 10.0,
        level: Grade::Constant(0.0),
        frame: FieldFrame::Part,
        accuracy_mm: 0.1,
        min_detail_mm: 0.1,
    };
    after(k.gyroid(&OpTag::new("second"), field, &gyroid));
}
