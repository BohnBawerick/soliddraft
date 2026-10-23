//! Autosave, crash recovery and backups, driven the way the app drives them each frame,
//! without a window. A crash is a session dropped without `close`: its folder and
//! copies stay, and its lock is released as the operating system releases it when a
//! process dies.

use soliddraft_app::document::{Command, Document, ExtrudeParams, PlaneKind, ProfileKind};
use soliddraft_app::drawing::document::DrawingDoc;
use soliddraft_app::file;
use soliddraft_app::recovery::Recovery;
use soliddraft_document::recovery::file_name;
use soliddraft_document::{AutosavePolicy, DocKind, Standing};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const KERNEL: &str = "catallax test";

/// One config folder for the whole test binary: saves read the backup settings and
/// write the recent list there.
fn config() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("soliddraft-recovery-config-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        file::set_config_dir(Some(dir.clone()));
        dir
    })
}

fn folder(name: &str) -> PathBuf {
    config();
    let dir = std::env::temp_dir().join(format!("soliddraft-recovery-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn block(depth: f64) -> Command {
    Command::AddExtrude {
        params: ExtrudeParams {
            sketch: None,
            profile: ProfileKind::Rectangle { width: 40.0, height: 30.0 },
            plane: PlaneKind::Top,
            depth,
            flip: false,
        },
        via_assistant: false,
    }
}

fn part() -> Document {
    let mut doc = Document::new("Bracket", soliddraft_app::kernel());
    doc.apply(block(10.0)).unwrap();
    doc
}

fn sessions(root: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("session-"))
        .collect();
    found.sort();
    found
}

#[test]
fn a_crash_leaves_copies_that_the_next_start_restores() {
    let root = folder("crash");
    let policy = AutosavePolicy { enabled: true, interval_seconds: 60, edits: 0 };
    let mut session = Recovery::start(&root, policy).unwrap();
    assert!(session.offers.is_empty() && !session.show, "nothing to recover on a first start");
    let doc = part();
    let mut drawing = DrawingDoc::new(soliddraft_drawing::fixture::bracket_a3());
    drawing.touch();
    let t0 = Instant::now();
    assert_eq!(session.tick(&doc, Some(&drawing), KERNEL, t0), Some(Duration::from_secs(60)));
    session.tick(&doc, Some(&drawing), KERNEL, t0 + Duration::from_secs(30));
    session.flush();
    assert!(!session.folder().join(file_name(doc.model().id(), DocKind::Part)).exists(), "not due yet");
    session.tick(&doc, Some(&drawing), KERNEL, t0 + Duration::from_secs(60));
    session.flush();
    assert!(session.folder().join(file_name(doc.model().id(), DocKind::Part)).exists());
    assert!(session.folder().join(file_name(drawing.id, DocKind::Drawing)).exists());
    drop(session);

    let mut next = Recovery::start(&root, policy).unwrap();
    assert!(next.show, "the window opens for the offers");
    assert_eq!(next.offers.len(), 2);
    assert!(next.offers.iter().all(|c| c.standing() == Standing::NeverSaved));
    assert_eq!(sessions(&root), [next.folder().to_path_buf()], "the ended session's folder is gone");

    let offer = next.offers.iter().find(|c| c.record.kind == DocKind::Part).unwrap().clone();
    assert_eq!(offer.record.name, "Bracket");
    let (record, bytes) = next.restore(&offer.file).unwrap();
    let back = file::open_bytes(&record.name, &bytes, soliddraft_app::kernel()).unwrap();
    assert_eq!(back.model().id(), doc.model().id());
    assert_eq!(back.model().semantic_hash(), doc.model().semantic_hash());
    assert_eq!(back.features.len(), 1);
    assert!(back.display_body().is_some(), "the part rebuilds from its copy");
    next.restored(record.id, DocKind::Part, back.edits);

    let offer = next.offers.iter().find(|c| c.record.kind == DocKind::Drawing).unwrap().clone();
    let (_, bytes) = next.restore(&offer.file).unwrap();
    let restored = DrawingDoc::from_bytes(&bytes).unwrap();
    assert_eq!(restored.id, drawing.id);
    assert_eq!(restored.drawing.semantic_hash().unwrap(), drawing.drawing.semantic_hash().unwrap());
    assert!(next.offers.is_empty());

    // The restored documents' copies belong to this session now: a second crash keeps them.
    drop(next);
    let third = Recovery::start(&root, policy).unwrap();
    assert_eq!(third.offers.len(), 2);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn copies_follow_saves_and_only_newer_ones_are_offered() {
    let root = folder("saves");
    let user = folder("saves-user").join("bracket.dwprt");
    let policy = AutosavePolicy { enabled: true, interval_seconds: 3600, edits: 2 };
    let mut doc = part();
    file::save(&mut doc, &user).unwrap();
    let mut session = Recovery::start(&root, policy).unwrap();
    let copy = session.folder().join(file_name(doc.model().id(), DocKind::Part));
    let t0 = Instant::now();
    assert_eq!(session.tick(&doc, None, KERNEL, t0), None, "a saved part waits for nothing");

    // Two edits write a copy before the interval.
    doc.apply(block(5.0)).unwrap();
    session.tick(&doc, None, KERNEL, t0);
    session.flush();
    assert!(!copy.exists(), "one edit");
    doc.undo().unwrap();
    session.tick(&doc, None, KERNEL, t0);
    session.flush();
    assert!(copy.exists(), "the second edit");

    // Saving deletes the copy.
    file::save(&mut doc, &user).unwrap();
    session.tick(&doc, None, KERNEL, t0);
    session.flush();
    assert!(!copy.exists());
    let saved = std::fs::read(&user).unwrap();

    // Two edits in one frame write a copy, then a crash.
    doc.redo().unwrap();
    doc.apply(block(8.0)).unwrap();
    session.tick(&doc, None, KERNEL, t0);
    session.flush();
    assert!(copy.exists());
    assert_eq!(std::fs::read(&user).unwrap(), saved, "autosave never writes the user's file");
    drop(session);

    let next = Recovery::start(&root, policy).unwrap();
    assert_eq!(next.offers.len(), 1);
    assert_eq!(next.offers[0].standing(), Standing::Newer);
    assert_eq!(next.offers[0].record.path.as_deref(), Some(user.as_path()));
    drop(next);

    // Once the file holds the same work, the copy is no longer offered.
    file::save(&mut doc, &user).unwrap();
    let last = Recovery::start(&root, policy).unwrap();
    assert!(last.offers.is_empty() && !last.show);
    last.close();
    assert!(sessions(&root).is_empty(), "a clean close leaves nothing");
    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(user.parent().unwrap());
}

#[test]
fn damaged_copies_are_reported_and_skipped_and_running_sessions_are_left_alone() {
    let root = folder("damaged");
    let policy = AutosavePolicy { enabled: true, interval_seconds: 1, edits: 1 };
    // A session that crashed while its disk filled up.
    let ended = root.join("session-1-1");
    std::fs::create_dir_all(&ended).unwrap();
    std::fs::write(ended.join("session.lock"), b"").unwrap();
    std::fs::write(ended.join("3f0c2a4e-7b1d-4c55-9a0e-2d6f8e1b9c47.dwprt.recovery"), b"PK\x03\x04cut").unwrap();
    // A session that is still running, with a copy of its own.
    let mut running = Recovery::start(&root, policy).unwrap();
    assert_eq!(running.problems.len(), 1, "{:?}", running.problems);
    assert!(running.problems[0].contains("damaged"), "{}", running.problems[0]);
    assert!(running.offers.is_empty() && running.show);
    assert!(root.join("damaged/3f0c2a4e-7b1d-4c55-9a0e-2d6f8e1b9c47.dwprt.recovery").exists(), "kept aside");
    let doc = part();
    running.tick(&doc, None, KERNEL, Instant::now());
    running.flush();

    let second = Recovery::start(&root, policy).unwrap();
    assert!(second.offers.is_empty(), "a running session's copies are not offered");
    assert!(second.problems.is_empty(), "a damaged copy is reported once");
    assert!(running.folder().join(file_name(doc.model().id(), DocKind::Part)).exists());
    second.close();
    running.close();
    assert!(sessions(&root).is_empty());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn saves_keep_backups_and_part_files_carry_thumbnails() {
    let dir = folder("backups");
    let path = dir.join("bracket.dwprt");
    let mut doc = part();
    file::save(&mut doc, &path).unwrap();
    assert!(!dir.join("bracket.dwprt.1.bak").exists(), "a new file has no earlier version");
    let first = std::fs::read(&path).unwrap();
    doc.apply(block(4.0)).unwrap();
    let png = [0x89, b'P', b'N', b'G'];
    let extras = file::Extras { thumbnail: Some(&png[..]), ..Default::default() };
    assert_eq!(file::save_with(&mut doc, &path, &extras).unwrap(), None);
    assert_eq!(std::fs::read(dir.join("bracket.dwprt.1.bak")).unwrap(), first, "the previous version");
    assert_eq!(soliddraft_document::thumbnail(&path).unwrap().as_deref(), Some(png.as_slice()));
    assert_eq!(soliddraft_document::thumbnail(&dir.join("bracket.dwprt.1.bak")).unwrap(), None);
    let back = file::open(&path, soliddraft_app::kernel()).unwrap();
    assert_eq!(back.features.len(), 2, "a part with a thumbnail opens");
    let _ = std::fs::remove_dir_all(dir);
}
