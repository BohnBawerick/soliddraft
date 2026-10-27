//! Damaged document files: truncated, corrupted and wrong-version `.dwprt`, `.dwasm` and
//! `.dwdrw` files are refused with a typed error that says what is wrong, and the file
//! on disk is never changed, replaced or joined by a stray file, whatever command or
//! entry point met it.
//!
//! Every damaged file is opened through the libraries and the application's own open
//! functions, then handed to the `soliddraft` binary's reading and writing commands as a
//! separate process. Before and after each attempt the file's bytes and hash and its
//! folder's listing are compared.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::Arc;

use serde_json::{Value, json};
use soliddraft_assembly::{AssemblyDocument, AssemblyPackage, FileError as AssemblyFileError, LoadError, OpenOptions};
use soliddraft_document::{ContentHash, DocKind, EnvelopeError};
use soliddraft_drawing::DrawingError;
use soliddraft_kernel_api::Kernel;
use soliddraft_kernel_catallax::CatallaxKernel;
use soliddraft_model::sketch::{P2, Sketch, solve::solve};
use soliddraft_model::{
    Command, DatumPlane, Document, EndCondition, ExtrudeFeature, ExtrudeOp, FeatureKind, FileError, ProfileSource,
    SketchFeature, SketchPlane,
};

/// How a refusal must read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Refusal {
    /// The file is damaged: not an archive, cut short, or an entry that does not parse.
    Damaged,
    /// The file is intact but this version cannot read it: another version or kind.
    Version,
}

struct Case {
    label: String,
    bytes: Vec<u8>,
    refusal: Refusal,
    /// Whether the binary's commands also meet this file: one variant per kind of damage
    /// is enough there, since the libraries already met them all.
    commands: bool,
}

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("soliddraft-damaged-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    /// The names in the folder, sorted.
    fn listing(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// Runs `soliddraft` in this folder with its own settings.
    fn run(&self, args: &[&str]) -> std::process::Output {
        Process::new(env!("CARGO_BIN_EXE_soliddraft"))
            .args(args)
            .current_dir(&self.0)
            .env("SOLIDDRAFT_CONFIG_DIR", self.0.join(".config"))
            .output()
            .expect("running soliddraft")
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn kernel() -> Arc<dyn Kernel> {
    Arc::new(CatallaxKernel::new())
}

// ----- valid files -----

fn valid_part() -> Vec<u8> {
    let mut doc = Document::new("Block", kernel());
    let mut sketch = Sketch::new();
    sketch.rectangle(P2::new(0.0, 0.0), 40.0, 30.0).unwrap();
    solve(&mut sketch, None, true);
    let plane = SketchPlane::Datum { datum: DatumPlane::Top, offset: 0.0 };
    let mut add = |kind| {
        doc.execute(Command::AddFeature { kind, name: None, position: None, via_assistant: false })
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
    doc.to_bytes().unwrap()
}

/// A drawing of the part `block.dwprt`, made with the binary as a user would.
fn valid_drawing(part: &[u8]) -> Vec<u8> {
    let dir = Dir::new("drawing-source");
    std::fs::write(dir.path("block.dwprt"), part).unwrap();
    let args = ["draw", "block.dwprt", "--out", "frame.dwdrw"];
    let out = dir.run(&args);
    assert!(out.status.success(), "soliddraft {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    std::fs::read(dir.path("frame.dwdrw")).unwrap()
}

/// An assembly that places the part `block.dwprt`, made with the binary as a user would.
fn valid_assembly(part: &[u8]) -> Vec<u8> {
    let dir = Dir::new("assembly-source");
    std::fs::write(dir.path("block.dwprt"), part).unwrap();
    for args in [&["asm", "new", "frame.dwasm"][..], &["asm", "insert", "frame.dwasm", "block.dwprt"]] {
        let out = dir.run(args);
        assert!(out.status.success(), "soliddraft {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
    std::fs::read(dir.path("frame.dwasm")).unwrap()
}

// ----- damage -----

fn entries(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("a valid archive");
    (0..zip.len())
        .map(|i| {
            let mut file = zip.by_index(i).unwrap();
            let mut data = Vec::new();
            file.read_to_end(&mut data).unwrap();
            (file.name().expect("a named entry").into_owned(), data)
        })
        .collect()
}

fn archive(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, data) in entries {
        zip.start_file(name.as_str(), zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

/// The archive with `edit` applied to the entry `name`; `None` removes the entry.
fn with_entry(bytes: &[u8], name: &str, edit: impl FnOnce(Vec<u8>) -> Option<Vec<u8>>) -> Vec<u8> {
    let mut all = entries(bytes);
    let at = all.iter().position(|(n, _)| n == name).unwrap_or_else(|| panic!("no entry {name}"));
    match edit(all[at].1.clone()) {
        Some(data) => all[at].1 = data,
        None => {
            all.remove(at);
        }
    }
    archive(&all)
}

fn with_json(bytes: &[u8], name: &str, edit: impl FnOnce(&mut Value)) -> Vec<u8> {
    with_entry(bytes, name, |data| {
        let mut value: Value = serde_json::from_slice(&data).unwrap();
        edit(&mut value);
        Some(serde_json::to_vec(&value).unwrap())
    })
}

/// Deterministic noise.
fn noise(len: usize) -> Vec<u8> {
    let mut x = 0x2545_f491_4f6c_dd1du64;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

/// Whether the archive holds the entry and it reads to its end with a good checksum.
fn reads(bytes: &[u8], name: &str) -> bool {
    let Ok(mut zip) = zip::ZipArchive::new(std::io::Cursor::new(bytes)) else { return false };
    let Ok(mut file) = zip.by_name(name) else { return false };
    file.read_to_end(&mut Vec::new()).is_ok()
}

/// Offset and length of an entry's stored bytes within the archive.
fn stored_range(bytes: &[u8], name: &str) -> (usize, usize) {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let file = zip.by_name(name).unwrap();
    (file.data_start().expect("a stored entry") as usize, file.compressed_size() as usize)
}

fn cases(kind: DocKind, valid: &[u8]) -> Vec<Case> {
    let truth = kind.truth_entry();
    let manifest = "manifest.json";
    let case = |label: &str, bytes: Vec<u8>, refusal| Case { label: label.to_owned(), bytes, refusal, commands: false };
    let mut all = Vec::new();

    // Truncated: cut anywhere, including inside the archive's end record.
    let len = valid.len();
    let mut cuts = vec![0, 1, 3, 4, 21, 22, len / 4, len / 2, len * 3 / 4, len - 100, len - 22, len - 1];
    cuts.sort();
    cuts.dedup();
    for cut in cuts.into_iter().filter(|c| *c < len) {
        all.push(case(&format!("truncated to {cut} of {len} bytes"), valid[..cut].to_vec(), Refusal::Damaged));
    }

    // Corrupted: not an archive, a broken archive, entries that do not read.
    all.push(case("a text file", b"this is not a SolidDraft document\n".to_vec(), Refusal::Damaged));
    all.push(case("all zero bytes", vec![0; len], Refusal::Damaged));
    all.push(case("noise", noise(len), Refusal::Damaged));
    let mut no_end = valid.to_vec();
    let end = no_end.windows(4).rposition(|w| w == b"PK\x05\x06").expect("an end record");
    no_end[end..end + 4].copy_from_slice(b"XXXX");
    all.push(case("the archive's end record destroyed", no_end, Refusal::Damaged));
    let (start, size) = stored_range(valid, truth);
    // A flipped byte can land in the padding of the compressed stream and change nothing;
    // only flips that make the entry unreadable are variants.
    for (what, at) in [("start", start), ("middle", start + size / 2), ("end", start + size - 1)] {
        let mut flipped = valid.to_vec();
        flipped[at] ^= 0x5a;
        if reads(&flipped, truth) {
            assert_ne!(what, "middle", "a flip in the middle of {truth} changed nothing");
            continue;
        }
        all.push(case(&format!("a byte flipped at the {what} of {truth}"), flipped, Refusal::Damaged));
    }
    all.push(case(
        &format!("{truth} is not JSON"),
        with_entry(valid, truth, |_| Some(b"{ not json".to_vec())),
        Refusal::Damaged,
    ));
    all.push(case(&format!("{truth} is empty"), with_entry(valid, truth, |_| Some(Vec::new())), Refusal::Damaged));
    all.push(case(
        &format!("{truth} is a list"),
        with_entry(valid, truth, |_| Some(b"[1, 2, 3]".to_vec())),
        Refusal::Damaged,
    ));
    all.push(case(&format!("{truth} is missing"), with_entry(valid, truth, |_| None), Refusal::Damaged));
    all.push(case(
        &format!("{truth} cut in half"),
        with_entry(valid, truth, |d| Some(d[..d.len() / 2].to_vec())),
        Refusal::Damaged,
    ));
    all.push(case(
        "manifest.json is not JSON",
        with_entry(valid, manifest, |_| Some(b"\xff\xfe nope".to_vec())),
        Refusal::Damaged,
    ));
    all.push(case(
        "manifest.json is empty of fields",
        with_entry(valid, manifest, |_| Some(b"{}".to_vec())),
        Refusal::Damaged,
    ));
    all.push(case("manifest.json is missing", with_entry(valid, manifest, |_| None), Refusal::Damaged));
    all.push(case(
        "manifest.json has an id that is not an id",
        with_json(valid, manifest, |m| m["id"] = json!("not-an-id")),
        Refusal::Damaged,
    ));
    all.push(case(
        "manifest.json has sections of the wrong type",
        with_json(valid, manifest, |m| m["sections"] = json!("none")),
        Refusal::Damaged,
    ));

    // Wrong version or kind: intact files this version must refuse as such.
    let major = kind.format().rsplit_once('/').unwrap().0.to_owned();
    all.push(case(
        "envelope from a newer version",
        with_json(valid, manifest, |m| m["envelope"] = json!("soliddraft-document/2")),
        Refusal::Version,
    ));
    all.push(case(
        "envelope of a foreign format",
        with_json(valid, manifest, |m| m["envelope"] = json!("other-document/1")),
        Refusal::Version,
    ));
    all.push(case(
        "a required section this version does not know",
        with_json(valid, manifest, |m| {
            m["sections"].as_array_mut().unwrap().push(json!({ "name": "future/", "required": true }))
        }),
        Refusal::Version,
    ));
    let other = DocKind::ALL.into_iter().find(|k| *k != kind).unwrap();
    all.push(case(
        &format!("the manifest says the file is a {other}"),
        with_json(valid, manifest, |m| m["kind"] = json!(other)),
        Refusal::Version,
    ));
    for tag in [format!("{major}/3"), format!("{major}/99"), format!("{major}/0"), "other-format/1".to_owned()] {
        all.push(case(
            &format!("{truth} and manifest.json say {tag}"),
            with_json(&with_json(valid, truth, |t| t["format"] = json!(tag)), manifest, |m| m["format"] = json!(tag)),
            Refusal::Version,
        ));
        if kind == DocKind::Drawing {
            // A drawing's truth carries no tag of its own: the manifest's is the version.
            all.push(case(
                &format!("manifest.json says {tag}"),
                with_json(valid, manifest, |m| m["format"] = json!(tag)),
                Refusal::Version,
            ));
        } else {
            all.push(case(
                &format!("{truth} says {tag}"),
                with_json(valid, truth, |t| t["format"] = json!(tag)),
                Refusal::Version,
            ));
        }
    }
    let half = format!("truncated to {} of {len} bytes", len / 2);
    for c in &mut all {
        c.commands = [
            half.as_str(),
            "truncated to 0 ",
            "truncated to 1 ",
            "end record",
            "middle of",
            "is not JSON",
            "is missing",
            "newer version",
            "required section",
            "/3",
            "/0",
        ]
        .iter()
        .any(|k| c.label.contains(k));
    }
    all
}

// ----- the libraries and the application's open functions -----

/// Entries named as missing are damage, whichever variant carries the message.
fn says_missing(message: &str) -> bool {
    message.starts_with("no ") || message.contains(": no ")
}

fn envelope_refusal(e: &EnvelopeError) -> Refusal {
    match e {
        EnvelopeError::Unsupported(m) if !says_missing(m) => Refusal::Version,
        _ => Refusal::Damaged,
    }
}

fn part_refusal(e: &FileError) -> Refusal {
    match e {
        // A truth entry with no format tag at all is not another version of anything.
        FileError::Format { found } if !found.is_empty() => Refusal::Version,
        FileError::Unsupported(m) if !says_missing(m) => Refusal::Version,
        _ => Refusal::Damaged,
    }
}

fn assembly_refusal(e: &AssemblyFileError) -> Refusal {
    match e {
        AssemblyFileError::Format { found } if !found.is_empty() => Refusal::Version,
        AssemblyFileError::Envelope(e) => envelope_refusal(e),
        _ => Refusal::Damaged,
    }
}

fn drawing_refusal(e: &DrawingError) -> Refusal {
    match e {
        // `Format` carries both: a file that is not an archive, or lacks an entry, says so
        // in its message.
        DrawingError::Format(m) if !m.contains("not a drawing file") && !says_missing(m) => Refusal::Version,
        _ => Refusal::Damaged,
    }
}

/// Opens `path` every way the libraries and the application do; each must refuse with
/// the expected kind of error and a message.
fn assert_libraries_refuse(kind: DocKind, path: &Path, bytes: &[u8], case: &Case) {
    let want = case.refusal;
    let check = |entry: &str, got: Refusal, message: String| {
        assert!(!message.trim().is_empty(), "{kind} {}: {entry} refused with no message", case.label);
        assert!(
            !message.contains("is ,") && !message.contains("is  "),
            "{kind} {}: {entry} names a blank: {message}",
            case.label
        );
        assert_eq!(got, want, "{kind} {}: {entry} refused as {got:?}: {message}", case.label);
    };
    match kind {
        DocKind::Part => {
            let e = Document::from_bytes(bytes, kernel()).err().unwrap_or_else(|| panic!("{} opened", case.label));
            check("Document::from_bytes", part_refusal(&e), e.to_string());
            let e = Document::open(path, kernel()).err().unwrap_or_else(|| panic!("{} opened", case.label));
            check("Document::open", part_refusal(&e), e.to_string());
            let e = soliddraft_app::file::open(path, kernel(), None)
                .err()
                .unwrap_or_else(|| panic!("{} opened", case.label));
            assert!(e.contains(&path.display().to_string()), "{}: the app's message names the file: {e}", case.label);
        }
        DocKind::Drawing => {
            let e =
                soliddraft_drawing::file::from_bytes(bytes).err().unwrap_or_else(|| panic!("{} opened", case.label));
            check("file::from_bytes", drawing_refusal(&e), e.to_string());
            let e = soliddraft_drawing::file::read(path).err().unwrap_or_else(|| panic!("{} opened", case.label));
            check("file::read", drawing_refusal(&e), e.to_string());
            let e = soliddraft_app::drawing::DrawingDoc::open(path)
                .err()
                .unwrap_or_else(|| panic!("{} opened", case.label));
            assert!(!e.trim().is_empty(), "{}: the app's drawing message is empty", case.label);
            let e = soliddraft_app::drawing::DrawingDoc::from_bytes(bytes)
                .err()
                .unwrap_or_else(|| panic!("{} opened", case.label));
            assert!(!e.trim().is_empty(), "{}: the app's drawing message is empty", case.label);
        }
        DocKind::Assembly => {
            let e =
                AssemblyPackage::from_bytes(bytes.to_vec()).err().unwrap_or_else(|| panic!("{} opened", case.label));
            check("AssemblyPackage::from_bytes", assembly_refusal(&e), e.to_string());
            let e = AssemblyPackage::open(path).err().unwrap_or_else(|| panic!("{} opened", case.label));
            check("AssemblyPackage::open", assembly_refusal(&e), e.to_string());
            let e = AssemblyDocument::open(path, OpenOptions::default())
                .err()
                .unwrap_or_else(|| panic!("{} opened", case.label));
            let LoadError::File(e) = e else { panic!("{}: refused as {e:?}", case.label) };
            check("AssemblyDocument::open", assembly_refusal(&e), e.to_string());
        }
    }
}

// ----- the command line -----

/// A command line of the binary, and whether it reads the file's manifest alone.
struct Line {
    args: Vec<String>,
    manifest_only: bool,
}

/// The commands that read or write a document of `kind`, named after the damaged file.
fn commands(kind: DocKind, name: &str, occurrence: &str) -> Vec<Line> {
    let words = |line: &[&str]| Line {
        args: line.iter().map(|w| w.replace("FILE", name).replace("OCC", occurrence)).collect::<Vec<_>>(),
        manifest_only: false,
    };
    // These read only the manifest: they report a file whose manifest reads, even when
    // what the file holds is damaged or from another version. Opening it is what refuses.
    let manifest = |line: &[&str]| Line { manifest_only: true, ..words(line) };
    let mut all = vec![
        manifest(&["doc", "info", "FILE"]),
        manifest(&["doc", "refs", "FILE"]),
        manifest(&["doc", "where-used", "FILE"]),
        manifest(&["doc", "migrate", "FILE"]),
    ];
    match kind {
        DocKind::Part => all.extend([
            words(&["part", "FILE"]),
            words(&["part", "--json", "FILE"]),
            words(&["quality", "FILE"]),
            words(&["quality", "FILE", "--set", "3"]),
            words(&["material", "info", "FILE"]),
            words(&["material", "faces", "FILE"]),
            words(&["material", "set", "FILE", "none"]),
            words(&["material", "paint", "FILE", "none"]),
            words(&["material", "update", "FILE"]),
            words(&["draw", "FILE", "--template", "iso-a3", "--out", "made.dwdrw"]),
            manifest(&["asm", "insert", "frame.dwasm", "FILE"]),
        ]),
        DocKind::Drawing => all.extend([
            words(&["doc", "rebuild", "FILE"]),
            words(&["doc", "repair", "FILE", "--to", "block.dwprt"]),
            words(&["doc", "repair", "FILE", "--to", "block.dwprt", "--replace"]),
            words(&["draw", "--update", "FILE"]),
        ]),
        DocKind::Assembly => all.extend([
            words(&["doc", "rebuild", "FILE"]),
            words(&["asm", "info", "FILE"]),
            words(&["asm", "tree", "FILE"]),
            words(&["asm", "insert", "FILE", "block.dwprt"]),
            words(&["asm", "move", "FILE", "OCC", "--by", "1,0,0"]),
            words(&["asm", "hide", "FILE", "OCC"]),
            words(&["asm", "suppress", "FILE", "OCC"]),
            words(&["asm", "colour", "FILE", "OCC", "--none"]),
            words(&["asm", "quality", "FILE", "--every", "3"]),
            words(&["asm", "rename", "FILE", "OCC", "Renamed"]),
            words(&["asm", "delete", "FILE", "OCC"]),
            words(&["asm", "rebuild", "FILE"]),
        ]),
    }
    all
}

fn assert_commands_refuse(kind: DocKind, dir: &Dir, name: &str, occurrence: &str, original: &[u8], case: &Case) {
    let listing = dir.listing();
    let manifest_reads = soliddraft_document::identify_file(&dir.path(name)).is_ok();
    for Line { args, manifest_only } in commands(kind, name, occurrence) {
        let may_succeed = manifest_only && manifest_reads;
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = dir.run(&refs);
        let stderr = String::from_utf8_lossy(&out.stderr);
        let what = format!("{kind} ({}): soliddraft {}", case.label, args.join(" "));
        let code = out.status.code();
        let allowed: &[i32] = if may_succeed { &[0, 1, 2] } else { &[1, 2] };
        assert!(code.is_some_and(|c| allowed.contains(&c)), "{what} exited with {code:?}: {stderr}");
        assert!(!stderr.contains("panicked"), "{what} panicked: {stderr}");
        assert!(may_succeed || !stderr.trim().is_empty(), "{what} said nothing");
        assert_eq!(std::fs::read(dir.path(name)).unwrap(), original, "{what} changed the file");
        assert_eq!(
            ContentHash::of_bytes(&std::fs::read(dir.path(name)).unwrap()),
            ContentHash::of_bytes(original),
            "{what} changed the file's hash"
        );
        assert_eq!(dir.listing(), listing, "{what} left or removed files: {stderr}");
    }
}

fn run_kind(kind: DocKind, valid: &[u8], support: &[(&str, &[u8])]) {
    let dir = Dir::new(&kind.to_string());
    for (name, bytes) in support {
        std::fs::write(dir.path(name), bytes).unwrap();
    }
    // The valid file opens, so every refusal below is the damage's.
    let name = format!("damaged.{}", kind.extension());
    let path = dir.path(&name);
    std::fs::write(&path, valid).unwrap();
    let check = commands(kind, &name, "").remove(0).args;
    let out = dir.run(&check.iter().map(String::as_str).collect::<Vec<_>>());
    assert!(out.status.success(), "the undamaged {kind} does not open: {}", String::from_utf8_lossy(&out.stderr));

    // The assembly's first occurrence, which the commands that act on one name.
    let occurrence = match kind {
        DocKind::Assembly => {
            let out = dir.run(&["asm", "tree", &name, "--json"]);
            let tree: Value = serde_json::from_slice(&out.stdout).expect("the tree prints JSON");
            tree[0]["path"].as_str().expect("the assembly places a part").to_owned()
        }
        _ => String::new(),
    };
    // Each command is well formed: on the undamaged file none of them is a usage error, so
    // a refusal below is the damage's.
    for Line { args, .. } in commands(kind, &name, &occurrence) {
        std::fs::write(&path, valid).unwrap();
        let out = dir.run(&args.iter().map(String::as_str).collect::<Vec<_>>());
        let stderr = String::from_utf8_lossy(&out.stderr);
        eprintln!(
            "control {kind}: soliddraft {} -> {:?} {}",
            args.join(" "),
            out.status.code(),
            stderr.lines().next().unwrap_or("")
        );
        assert_ne!(out.status.code(), Some(2), "soliddraft {args:?} is not a command: {stderr}");
    }
    std::fs::write(&path, valid).unwrap();
    let all = cases(kind, valid);
    let with_commands = all.iter().filter(|c| c.commands).count();
    assert!(with_commands >= 10, "{kind}: only {with_commands} variants reach the commands");
    eprintln!(
        "{kind}: {} variants, {with_commands} also through {} commands each",
        all.len(),
        commands(kind, &name, &occurrence).len()
    );
    for case in all {
        std::fs::write(&path, &case.bytes).unwrap();
        let before = std::fs::read(&path).unwrap();
        assert_libraries_refuse(kind, &path, &before, &case);
        assert_eq!(std::fs::read(&path).unwrap(), before, "{kind} ({}): the libraries changed the file", case.label);
        if case.commands {
            assert_commands_refuse(kind, &dir, &name, &occurrence, &before, &case);
        }
    }
}

#[test]
fn damaged_parts_are_refused_and_left_alone() {
    let part = valid_part();
    run_kind(DocKind::Part, &part, &[("block.dwprt", &part), ("frame.dwasm", &valid_assembly(&part))]);
}

#[test]
fn damaged_drawings_are_refused_and_left_alone() {
    let part = valid_part();
    run_kind(DocKind::Drawing, &valid_drawing(&part), &[("block.dwprt", &part)]);
}

#[test]
fn damaged_assemblies_are_refused_and_left_alone() {
    let part = valid_part();
    run_kind(DocKind::Assembly, &valid_assembly(&part), &[("block.dwprt", &part)]);
}

#[test]
fn the_damage_variants_are_all_damaged_files() {
    // Guards the generator: a variant that happened to equal the valid file would pass
    // for the wrong reason.
    for (kind, valid) in [
        (DocKind::Part, valid_part()),
        (DocKind::Drawing, valid_drawing(&valid_part())),
        (DocKind::Assembly, valid_assembly(&valid_part())),
    ] {
        let all = cases(kind, &valid);
        assert!(all.len() > 30, "{kind}: {} variants", all.len());
        assert!(all.iter().all(|c| c.bytes != valid), "{kind}: a variant equals the valid file");
        let mut labels: Vec<_> = all.iter().map(|c| c.label.clone()).collect();
        labels.sort();
        labels.dedup();
        assert_eq!(labels.len(), all.len(), "{kind}: labels repeat");
        assert!(all.iter().any(|c| c.refusal == Refusal::Version) && all.iter().any(|c| c.refusal == Refusal::Damaged));
    }
}
