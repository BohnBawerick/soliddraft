//! Fuzzing the command line: the `soliddraft` binary run as separate processes on
//! generated arguments for every subcommand, over good, empty, damaged, missing and
//! folder-shaped files, with extreme, malformed and misplaced numbers.
//!
//! A run may succeed or fail, but it must end by itself within ten minutes, exit with
//! status 0, 1 or 2 (never a panic, an abort or a signal), say why when it fails (a `check`
//! reports its findings on standard output), and keep every fixture file in its folder.
//!
//! Left out because their arguments are sizes chosen by the caller on purpose or they
//! need the machine: `asm generate`, `asm bench`, `screenshot`, `key` and `mcp --http`.
//!
//! A fixed number of cases from a fixed seed run by default; `FUZZ_TRACE=1 --nocapture` prints each
//! run's exit status and arguments. To search further:
//! `PROPTEST_CASES=2000 PROPTEST_RNG_SEED=7 cargo test -p soliddraft-cli --test fuzz_arguments`.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command as Process, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use proptest::prelude::*;
use proptest::test_runner::RngSeed;
use serde_json::Value;
use soliddraft_kernel_api::Kernel;
use soliddraft_kernel_catallax::CatallaxKernel;
use soliddraft_model::sketch::{P2, Sketch, solve::solve};
use soliddraft_model::{
    Command, DatumPlane, Document, EndCondition, ExtrudeFeature, ExtrudeOp, FeatureKind, ProfileSource, SketchFeature,
    SketchPlane,
};

/// Cases, and the seed they come from, unless the environment says otherwise.
fn config() -> ProptestConfig {
    // A failing input is printed with the test's seed; nothing is written beside the sources.
    let mut config = ProptestConfig { failure_persistence: None, ..ProptestConfig::default() };
    if std::env::var_os("PROPTEST_CASES").is_none() {
        config.cases = 100;
    }
    if std::env::var_os("PROPTEST_RNG_SEED").is_none() {
        config.rng_seed = RngSeed::Fixed(0x43_4c_49);
    }
    config
}

/// The hang guard every test shares: a wait this long means stuck, not slow.
const LIMIT: Duration = Duration::from_secs(600);

// ----- the folder every run starts from -----

/// Files that exist at the start of every run, by name, and the path of the assembly's
/// first occurrence.
struct Fixtures {
    files: Vec<(&'static str, Vec<u8>)>,
    occurrence: String,
}

fn fixtures() -> &'static Fixtures {
    static FIXTURES: std::sync::OnceLock<Fixtures> = std::sync::OnceLock::new();
    FIXTURES.get_or_init(|| {
        let kernel: Arc<dyn Kernel> = Arc::new(CatallaxKernel::new());
        let mut part = Document::new("Block", kernel);
        let mut sketch = Sketch::new();
        sketch.rectangle(P2::new(0.0, 0.0), 40.0, 30.0).unwrap();
        solve(&mut sketch, None, true);
        let plane = SketchPlane::Datum { datum: DatumPlane::Top, offset: 0.0 };
        let mut add = |kind| {
            part.execute(Command::AddFeature { kind, name: None, position: None, via_assistant: false })
                .unwrap()
                .created
                .unwrap()
        };
        let s = add(FeatureKind::Sketch(SketchFeature { plane, sketch }));
        add(FeatureKind::Extrude(ExtrudeFeature {
            profile: ProfileSource::Sketch { feature: s },
            operation: ExtrudeOp::Boss,
            end: EndCondition::Blind { depth: 10.0 },
            reverse: false,
        }));
        let part = part.to_bytes().unwrap();
        let recipe = std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../recipes/box.json")).unwrap();
        let dir = Dir::new("fixtures", &[]);
        let run = |args: &[&str]| {
            let out = dir.run(args);
            assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        std::fs::write(dir.0.join("part.dwprt"), &part).unwrap();
        run(&["draw", "part.dwprt", "--out", "drawing.dwdrw"]);
        run(&["asm", "new", "assembly.dwasm"]);
        run(&["asm", "insert", "assembly.dwasm", "part.dwprt"]);
        let read = |name: &str| std::fs::read(dir.0.join(name)).unwrap();
        let drawing = read("drawing.dwdrw");
        let assembly = read("assembly.dwasm");
        let tree = dir.run(&["asm", "tree", "assembly.dwasm", "--json"]);
        let tree: Value = serde_json::from_slice(&tree.stdout).expect("the tree prints JSON");
        let occurrence = tree[0]["path"].as_str().expect("the assembly places a part").to_owned();
        let files = vec![
            ("part.dwprt", part.clone()),
            ("other.dwprt", part.clone()),
            ("drawing.dwdrw", drawing.clone()),
            ("assembly.dwasm", assembly),
            ("recipe.json", recipe),
            ("empty.dwprt", Vec::new()),
            ("half.dwprt", part[..part.len() / 2].to_vec()),
            ("half.dwdrw", drawing[..drawing.len() / 2].to_vec()),
            ("garbage.json", b"{ \"steps\": [ nope".to_vec()),
            ("library.toml", b"[[materials]]\nid = \n".to_vec()),
        ];
        Fixtures { files, occurrence }
    })
}

struct Dir(PathBuf);

impl Dir {
    /// A new folder holding `files`, and a `folder/` and `home/` beside them.
    fn new(name: &str, files: &[(&str, Vec<u8>)]) -> Dir {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("soliddraft-fuzz-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("folder")).unwrap();
        for (file, bytes) in files {
            std::fs::write(dir.join(file), bytes).unwrap();
        }
        Dir(dir)
    }

    fn command(&self, args: &[&str]) -> Process {
        // On Linux the address space is capped so that a run asked for something absurd
        // fails by itself instead of taking the machine's memory.
        #[cfg(target_os = "linux")]
        let mut command = {
            let mut c = Process::new("sh");
            c.args(["-c", "ulimit -v 16000000; exec \"$@\"", "sh", env!("CARGO_BIN_EXE_soliddraft")]);
            c
        };
        #[cfg(not(target_os = "linux"))]
        let mut command = Process::new(env!("CARGO_BIN_EXE_soliddraft"));
        command
            .args(args)
            .current_dir(&self.0)
            .env("SOLIDDRAFT_CONFIG_DIR", self.0.join("home/config"))
            .env("HOME", self.0.join("home"))
            .env("XDG_CONFIG_HOME", self.0.join("home/xdg"))
            .env("XDG_DATA_HOME", self.0.join("home/xdg-data"))
            .env("APPDATA", self.0.join("home/appdata"))
            .env("USERPROFILE", self.0.join("home"))
            .env_remove("SOLIDDRAFT_PROVIDER");
        command
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        self.command(args).stdin(Stdio::null()).output().expect("running soliddraft")
    }

    /// Every file and folder under this one, relative, sorted.
    fn listing(&self) -> Vec<String> {
        fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                out.push(path.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/"));
                if path.is_dir() {
                    walk(&path, base, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.0, &self.0, &mut out);
        out.sort();
        out
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ----- arguments -----

fn pick(words: &[&str]) -> BoxedStrategy<String> {
    prop::sample::select(words.iter().map(|w| w.to_string()).collect::<Vec<_>>()).boxed()
}

/// A file name: mostly one of `good`, now and then a damaged or empty one, one that
/// does not exist, a folder or a bare word.
fn file_of(good: &[&str], damaged: &[&str]) -> BoxedStrategy<String> {
    let mut options = vec![(12, pick(good))];
    if !damaged.is_empty() {
        options.push((3, pick(damaged)));
    }
    options.push((
        2,
        pick(&[
            "missing.dwprt",
            "missing.json",
            "folder",
            "folder/",
            ".",
            "..",
            "",
            "-",
            "part.dwpart",
            "x.txt",
            "folder/new.dwprt",
        ]),
    ));
    options.push((1, any::<String>().boxed()));
    prop::strategy::Union::new_weighted(options).boxed()
}

fn part_file() -> BoxedStrategy<String> {
    file_of(&["part.dwprt", "other.dwprt"], &["empty.dwprt", "half.dwprt"])
}

fn drawing_file() -> BoxedStrategy<String> {
    file_of(&["drawing.dwdrw"], &["half.dwdrw", "empty.dwprt"])
}

fn assembly_file() -> BoxedStrategy<String> {
    file_of(&["assembly.dwasm"], &["half.dwprt", "empty.dwprt"])
}

fn recipe_file() -> BoxedStrategy<String> {
    file_of(&["recipe.json"], &["garbage.json", "empty.dwprt"])
}

/// A document of any kind.
fn document_file() -> BoxedStrategy<String> {
    prop_oneof![part_file(), drawing_file(), assembly_file()].boxed()
}

/// A number as someone might type it, and as nobody should.
fn number() -> BoxedStrategy<String> {
    prop_oneof![
        6 => (0u32..2000).prop_map(|n| (n as f64 / 8.0).to_string()),
        4 => pick(&[
            "0", "-0", "-1", "1", "2", "10", "11", "1e308", "-1e308", "1e-308", "5e-324", "1e999", "-1e999", "nan", "NaN",
            "inf", "-inf", "infinity", "1.7976931348623157e308", "4294967295", "4294967296", "18446744073709551615",
            "18446744073709551616", "9223372036854775807", "9223372036854775808", "-9223372036854775808", "0x10", "1_000",
            "1,5", "١٢٣", "+5", ".5", "5.", "--5", "", " ", "1e", "e1", "0.0000000001",
        ]),
        1 => any::<String>(),
    ]
    .boxed()
}

/// A point `x,y,z`, with the wrong number of parts or parts that are not numbers now and then.
fn point() -> BoxedStrategy<String> {
    prop_oneof![
        8 => (number(), number(), number()).prop_map(|(x, y, z)| format!("{x},{y},{z}")),
        1 => prop::collection::vec(number(), 0..5).prop_map(|n| n.join(",")),
        1 => any::<String>(),
    ]
    .boxed()
}

fn text() -> BoxedStrategy<String> {
    prop_oneof![
        4 => pick(&["a", "bracket", "Boss-Extrude1", "F1", "1", "0", "all", "none", "auto", "iso-a3", "steel", "a b"]),
        2 => "\\PC{0,16}",
        1 => any::<String>(),
        1 => Just("x".repeat(5000)),
    ]
    .boxed()
}

fn view() -> BoxedStrategy<String> {
    prop_oneof![
        6 => pick(&["front", "back", "top", "bottom", "left", "right", "iso", "A", "B", "V1", "1"]),
        1 => point(),
        1 => text(),
    ]
    .boxed()
}

/// A flag with a value from `value`, or on its own.
fn flag(name: &'static str, value: BoxedStrategy<String>) -> BoxedStrategy<Vec<String>> {
    prop_oneof![
        6 => value.prop_map(move |v| vec![name.to_owned(), v]),
        1 => Just(vec![name.to_owned()]),
    ]
    .boxed()
}

fn switch(name: &'static str) -> BoxedStrategy<Vec<String>> {
    Just(vec![name.to_owned()]).boxed()
}

/// Up to `max` of the `parts`, in any order, joined.
fn some_of(parts: Vec<BoxedStrategy<Vec<String>>>, max: usize) -> BoxedStrategy<Vec<String>> {
    prop::collection::vec(prop::strategy::Union::new(parts), 0..=max).prop_map(|v| v.concat()).boxed()
}

fn files(of: BoxedStrategy<String>, max: usize) -> BoxedStrategy<Vec<String>> {
    prop::collection::vec(of, 0..=max).boxed()
}

fn words(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

/// `a`, then each of the generated lists, as one argument list.
fn command(head: Vec<String>, tails: Vec<BoxedStrategy<Vec<String>>>) -> BoxedStrategy<Vec<String>> {
    tails.into_iter().fold(Just(head).boxed(), |all, tail| {
        (all, tail)
            .prop_map(|(mut a, b)| {
                a.extend(b);
                a
            })
            .boxed()
    })
}

fn quality() -> BoxedStrategy<String> {
    prop_oneof![
        4 => pick(&["auto", "1", "5", "10", "0", "11", "-1", "chord=0.1", "chord=0.1,angle=15", "chord=0", "chord=-1", "chord=1e308",
            "angle=0", "angle=360", "angle=nan", "edge=0.5,aspect=3", "aspect=0", "chord=", "=", ",", "chord=1,chord=2", "bogus=1"]),
        2 => (number(), number()).prop_map(|(c, a)| format!("chord={c},angle={a}")),
        1 => text(),
    ]
    .boxed()
}

fn draw_options() -> BoxedStrategy<Vec<String>> {
    let spec = |name: &'static str, parts: Vec<BoxedStrategy<String>>| {
        flag(name, prop::collection::vec(prop::strategy::Union::new(parts), 1..=5).prop_map(|p| p.join(":")).boxed())
    };
    some_of(
        vec![
            flag("--template", pick(&["iso-a3", "iso-a4", "ansi-a", "ansi-b", "nope", "", "ISO-A3"])),
            flag("--views", prop::collection::vec(view(), 0..4).prop_map(|v| v.join(",")).boxed()),
            flag("--dims", pick(&["model", "none", "x"])),
            flag("--hidden", pick(&["removed", "visible", "x"])),
            flag("--title", text()),
            flag("--number", text()),
            flag("--designer", text()),
            flag("--date", pick(&["2026-10-14", "2026-13-45", "0000-00-00", "x", "", "99999-01-01"])),
            flag(
                "--out",
                pick(&["o.dwdrw", "o.svg", "o.pdf", "o.dxf", "o.txt", "o", "folder", "folder/o.svg", "", "part.dwprt"]),
            ),
            switch("--json"),
            spec("--section", vec![view(), point(), pick(&["h", "v", "x", "flip", "h@2"]), number().boxed()]),
            spec("--detail", vec![view(), point(), number(), number()]),
            spec("--aux", vec![view(), point(), pick(&["flip", "x"])]),
            spec("--crop", vec![view(), point(), point()]),
            spec("--broken-out", vec![view(), point(), number(), number()]),
            spec(
                "--display",
                vec![view(), pick(&["hidden=removed", "hidden=visible", "tangent=phantom", "tangent=x", "x"])],
            ),
            spec(
                "--ordinate",
                vec![view(), pick(&["h", "v"]), prop::collection::vec(point(), 0..4).prop_map(|p| p.join(";")).boxed()],
            ),
            spec(
                "--chain",
                vec![
                    view(),
                    pick(&["h", "v", "h-15", "v+20", "h-x"]),
                    prop::collection::vec(point(), 0..4).prop_map(|p| p.join(";")).boxed(),
                ],
            ),
            spec(
                "--gdt",
                vec![
                    view(),
                    point(),
                    pick(&["position", "flatness", "x"]),
                    pick(&["0.05", "dia0.1", "dia0.1(M)", "x", ""]),
                    pick(&["A", "A,B(M),C", "x"]),
                ],
            ),
            spec("--datum", vec![view(), point(), pick(&["A", "Z", "", "AB", "1"])]),
            spec(
                "--finish",
                vec![view(), point(), pick(&["Ra 1.6", "", "x"]), pick(&["any", "removal", "no-removal", "x"])],
            ),
            spec("--revision", vec![pick(&["A", "B", ""]), text(), pick(&["2026-10-14", "x"]), text()]),
            spec("--revision-symbol", vec![view(), point(), pick(&["A", ""])]),
            flag("--delete", number()),
            flag("--keep-as-note", number()),
            flag(
                "--reattach",
                prop::collection::vec(prop_oneof![number(), point()], 1..4).prop_map(|p| p.join(":")).boxed(),
            ),
            switch("--accept"),
            flag("--search", file_of(&["folder"], &[])),
            flag("--kernel-log", pick(&["log.json", "folder", "folder/log.json", ""])),
        ],
        6,
    )
}

/// Occurrence paths: the assembly's real one, others that are not, and malformed ones.
fn paths() -> BoxedStrategy<Vec<String>> {
    let real = fixtures().occurrence.clone();
    prop::collection::vec(
        prop_oneof![
            6 => Just(real.clone()),
            2 => Just(format!("{real}:1.2")),
            2 => pick(&["1", "2", "0", "1/1", "4/12", "4/12:9.3", "1:2", "-1", "x", "", "1/", "99999999999999999999"]),
        ],
        0..3,
    )
    .boxed()
}

fn arguments() -> BoxedStrategy<Vec<String>> {
    let json = || prop::option::of(switch("--json")).prop_map(|j| j.unwrap_or_default()).boxed();
    let search =
        || prop::option::of(flag("--search", file_of(&["folder"], &[]))).prop_map(|j| j.unwrap_or_default()).boxed();
    let asm = |verb: &str, tails: Vec<BoxedStrategy<Vec<String>>>| command(words(&["asm", verb]), tails);
    let one = |file: BoxedStrategy<String>| file.prop_map(|f| vec![f]).boxed();
    let verb = |verbs: &[&str]| pick(verbs).prop_map(|v| vec![v]).boxed();
    prop_oneof![
        // replay and view
        2 => command(words(&["replay"]), vec![some_of(vec![switch("--json"), switch("--steps"), flag("--bogus", text())], 3), files(recipe_file(), 3)]),
        3 => command(
            words(&["view"]),
            vec![
                one(recipe_file()),
                some_of(
                    vec![
                        flag("--dir", prop::collection::vec(view(), 0..4).prop_map(|v| v.join(",")).boxed()),
                        flag("--up", point()),
                        switch("--seams"), switch("--smooth"), switch("--mesh"), switch("--json"), switch("--curves"),
                        flag("--tolerance", number()),
                        flag("--section", point()),
                        flag("--svg", pick(&["v.svg", "{view}.svg", "folder", "folder/{view}.svg", ""])),
                    ],
                    6,
                ),
            ],
        ),
        // parts
        2 => command(words(&["part"]), vec![some_of(vec![switch("--json")], 1), files(part_file(), 3)]),
        3 => command(
            words(&["quality"]),
            vec![
                one(part_file()),
                some_of(
                    vec![
                        flag("--set", quality()),
                        flag("--export", prop_oneof![(number(), number()).prop_map(|(c, a)| format!("{c},{a}")), number(), text()].boxed()),
                        flag("--display-data", pick(&["on", "off", "x", ""])),
                        switch("--json"),
                    ],
                    4,
                ),
            ],
        ),
        3 => command(
            words(&["material"]),
            vec![
                verb(&["list", "show", "appearances", "libraries", "check", "import", "info", "faces", "set", "paint", "update", "bogus", ""]),
                prop_oneof![
                    3 => files(part_file(), 2),
                    1 => files(file_of(&["library.toml"], &["garbage.json"]), 2),
                ]
                .boxed(),
                some_of(
                    vec![
                        flag("--category", text()), flag("--into", pick(&["library.toml", "folder/lib.toml", "folder", ""])),
                        flag("--config", text()), flag("--feature", pick(&["F1", "1", "Boss-Extrude1", "x"])), flag("--face", number()),
                        switch("--json"),
                    ],
                    3,
                ),
                prop::collection::vec(prop_oneof![text(), pick(&["none", "steel", "default", "1"])], 0..2).boxed(),
            ],
        ),
        // drawings
        4 => command(words(&["draw"]), vec![one(prop_oneof![3 => part_file(), 1 => recipe_file()].boxed()), draw_options()]),
        3 => command(words(&["draw", "--update"]), vec![one(drawing_file()), draw_options()]),
        // documents
        4 => command(
            words(&["doc"]),
            vec![
                verb(&["info", "refs", "rebuild", "repair", "migrate", "where-used", "bogus", ""]),
                files(document_file(), 2),
                some_of(vec![json(), search(), flag("--to", part_file()), switch("--replace"), switch("--accept")], 4),
            ],
        ),
        1 => command(
            words(&["locations"]),
            vec![
                prop::collection::vec(
                    prop_oneof![
                        pick(&["list", "add", "remove", "referenced_documents", "material_libraries", "drafting_styles", "bogus", "folder", "x.toml", "", "--json"]),
                        part_file(),
                    ],
                    0..4,
                )
                .boxed(),
            ],
        ),
        // assemblies
        1 => asm("new", vec![one(file_of(&["new.dwasm", "folder/new.dwasm"], &["part.dwprt", "assembly.dwasm"])), some_of(vec![flag("--name", text())], 1)]),
        2 => asm("info", vec![one(assembly_file()), some_of(vec![json(), search()], 2)]),
        2 => asm("tree", vec![one(assembly_file()), some_of(vec![flag("--depth", number()), json()], 2)]),
        3 => asm(
            "insert",
            vec![
                one(assembly_file()),
                prop::collection::vec(prop_oneof![part_file(), assembly_file()], 0..3).boxed(),
                some_of(vec![flag("--at", point()), flag("--rotate", prop_oneof![(point(), number()).prop_map(|(p, d)| format!("{p},{d}")), number(), text()].boxed())], 2),
            ],
        ),
        3 => asm(
            "pattern",
            vec![
                one(assembly_file()),
                paths(),
                some_of(
                    vec![
                        flag("--linear", point()), flag("--spacing", number()), flag("--count", number()),
                        flag("--second", point()), flag("--spacing2", number()), flag("--count2", number()),
                        flag("--circular", prop::collection::vec(number(), 0..8).prop_map(|n| n.join(",")).boxed()),
                        flag("--angle", number()),
                    ],
                    6,
                ),
            ],
        ),
        2 => asm("move", vec![one(assembly_file()), paths(), some_of(vec![flag("--to", point()), flag("--by", point()), flag("--rotate", point())], 3)]),
        2 => command(words(&["asm"]), vec![verb(&["hide", "show", "suppress", "unsuppress", "delete"]), one(assembly_file()), paths()]),
        1 => asm("colour", vec![one(assembly_file()), paths(), some_of(vec![flag("--rgb", pick(&["#ff0000", "#fff", "red", "#gggggg", "", "#ffffffff"])), flag("--metallic", number()), flag("--roughness", number()), switch("--none")], 3)]),
        1 => asm("quality", vec![one(assembly_file()), paths(), some_of(vec![flag("--set", quality()), flag("--every", quality()), switch("--none"), json()], 3)]),
        1 => asm("rename", vec![one(assembly_file()), paths(), prop::collection::vec(text(), 0..2).boxed()]),
        2 => asm("rebuild", vec![one(assembly_file()), some_of(vec![json(), search()], 2)]),
        // the server, with nothing on standard input
        1 => command(
            words(&["mcp"]),
            vec![some_of(
                vec![
                    switch("--demo"), flag("--open", part_file()), flag("--bundle", pick(&["bundle", "folder", "folder/b", "part.dwprt", ""])),
                    flag("--max-work", number()), flag("--bogus", text()),
                ],
                4,
            )],
        ),
        // the registry, and the rest
        2 => command(words(&["commands"]), vec![some_of(vec![switch("--json")], 1), prop::collection::vec(text(), 0..4).boxed()]),
        1 => prop::collection::vec(prop_oneof![pick(&["--version", "--help", "-h", "help", "bogus", "", "-", "--"]), text()], 0..4).boxed(),
        1 => prop::collection::vec(
            prop_oneof![pick(&["replay", "view", "part", "doc", "asm", "draw", "material", "quality", "commands", "locations", "mcp"]), text(), number()],
            1..5,
        )
        .boxed(),
    ]
    .boxed()
}

// ----- the run -----

fn assert_well_behaved(dir: &Dir, args: &[String], listing_before: &[String]) -> Result<(), String> {
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut child = dir
        .command(&refs)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    // Both pipes are drained as the run goes, so a chatty command cannot fill one and stall.
    let drain = |mut pipe: Box<dyn std::io::Read + Send>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            String::from_utf8_lossy(&bytes).into_owned()
        })
    };
    let stdout = drain(Box::new(child.stdout.take().unwrap()));
    let stderr = drain(Box::new(child.stderr.take().unwrap()));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if started.elapsed() > LIMIT {
            let _ = child.kill();
            return Err(format!("did not end within {LIMIT:?}"));
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let (stdout, stderr) = (stdout.join().unwrap(), stderr.join().unwrap());
    let code = status.code();
    if std::env::var_os("FUZZ_TRACE").is_some() {
        eprintln!("{code:?} {args:?}");
    }
    if !matches!(code, Some(0..=2)) {
        return Err(format!("exited with {status}: {}", tail(&stderr)));
    }
    if stderr.contains("panicked") || stderr.contains("RUST_BACKTRACE") {
        return Err(format!("panicked: {}", tail(&stderr)));
    }
    if code != Some(0) && stderr.trim().is_empty() && stdout.trim().is_empty() {
        return Err(format!("failed with status {code:?} and said nothing"));
    }
    // Whatever it wrote, it wrote inside its folder; nothing of the fixtures is gone.
    let after = dir.listing();
    for name in listing_before {
        if !after.contains(name) {
            return Err(format!("{name} is gone"));
        }
    }
    Ok(())
}

fn tail(text: &str) -> String {
    let start = text.char_indices().rev().nth(600).map_or(0, |(i, _)| i);
    text[start..].to_owned()
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn no_arguments_make_the_command_line_panic_or_hang(args in arguments()) {
        // Arguments cannot hold a NUL; the operating system refuses them before any program runs.
        prop_assume!(args.iter().all(|a| !a.contains('\0')));
        let dir = Dir::new("run", &fixtures().files);
        let before = dir.listing();
        if let Err(problem) = assert_well_behaved(&dir, &args, &before) {
            prop_assert!(false, "soliddraft {:?}: {}", args, problem);
        }
    }
}

/// End to end: a line of garbage on standard input does not end the stdio server; the
/// request after it is answered.
#[test]
fn the_stdio_server_answers_after_a_line_that_is_not_text() {
    let dir = Dir::new("stdio", &[]);
    let mut child =
        dir.command(&["mcp"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(b"\xff\xfe\x00 not text\n").unwrap();
    stdin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"ping\"}\n").unwrap();
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    let replies: Vec<Value> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).expect("protocol only"))
        .collect();
    assert!(out.status.success(), "{:?}", out.status);
    assert_eq!(replies.len(), 2, "{replies:?}");
    assert_eq!(replies[0]["error"]["code"], -32700);
    assert_eq!(replies[1]["id"], 9);
}

/// What `--json` promises, on the good fixtures: every line of standard output is JSON.
#[test]
fn json_flags_print_json() {
    let dir = Dir::new("json", &fixtures().files);
    let lines: Vec<Vec<&str>> = vec![
        vec!["replay", "--json", "recipe.json"],
        vec!["view", "recipe.json", "--json"],
        vec!["part", "--json", "part.dwprt"],
        vec!["quality", "part.dwprt", "--json"],
        vec!["commands", "--json", "extrude"],
        vec!["material", "list", "--json"],
        vec!["material", "info", "part.dwprt", "--json"],
        vec!["doc", "info", "part.dwprt", "--json"],
        vec!["doc", "info", "drawing.dwdrw", "--json"],
        vec!["doc", "refs", "assembly.dwasm", "--json"],
        vec!["doc", "where-used", "part.dwprt", "--json"],
        vec!["asm", "info", "assembly.dwasm", "--json"],
        vec!["asm", "tree", "assembly.dwasm", "--json"],
        vec!["asm", "rebuild", "assembly.dwasm", "--json"],
        vec!["draw", "part.dwprt", "--out", "again.dwdrw", "--json"],
        vec!["locations", "list", "--json"],
    ];
    for line in lines {
        let out = dir.run(&line);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{line:?}: {}", String::from_utf8_lossy(&out.stderr));
        assert!(!stdout.trim().is_empty(), "{line:?} printed nothing");
        for text in stdout.lines().filter(|l| !l.trim().is_empty()) {
            serde_json::from_str::<Value>(text)
                .unwrap_or_else(|e| panic!("{line:?} printed a line that is not JSON ({e}): {text}"));
        }
    }
}
