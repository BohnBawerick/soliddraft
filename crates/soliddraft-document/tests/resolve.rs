//! The resolver on real files: a drawing's reference to a part followed through a
//! rename, a move into a subfolder, a move into a search folder, a deletion, a
//! same-named impostor, an edit, an open unsaved edit and a migration; repairs and
//! where-used.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use soliddraft_document::manifest::{Entry, semantic_hash, write_package};
use soliddraft_document::resolve::{StoredPathProblem, repair, repin};
use soliddraft_document::{
    ContentHash, DocKind, DocRef, DocumentId, FolderIndex, Location, Manifest, OpenDocument, RefStatus, Resolution,
    Resolver, Section, Via, save_atomic,
};

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let dir = std::env::temp_dir().join(format!("soliddraft-resolve-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn truth(depth: f64) -> Value {
    json!({"format": "soliddraft-part/2", "name": "bracket", "features": [{"depth": depth}]})
}

/// Writes a part file with id `id` and returns its revision.
fn write_part(path: &Path, id: DocumentId, depth: f64) -> ContentHash {
    let t = truth(depth);
    let mut m = Manifest::new(id, DocKind::Part, semantic_hash(DocKind::Part, &t));
    m.sections = vec![Section::required("part.json")];
    let bytes = write_package(&m, &[Entry::new("part.json", serde_json::to_vec(&t).unwrap())]).unwrap();
    save_atomic(path, &bytes).unwrap();
    m.semantic_hash
}

/// Writes a drawing file that references `r`.
fn write_drawing(path: &Path, r: &DocRef) {
    let t = json!({"format": "soliddraft-drawing/2", "model": r});
    let mut m = Manifest::new(DocumentId::new(), DocKind::Drawing, semantic_hash(DocKind::Drawing, &t));
    m.sections = vec![Section::required("drawing.json")];
    m.references = vec![r.clone()];
    let bytes = write_package(&m, &[Entry::new("drawing.json", serde_json::to_vec(&t).unwrap())]).unwrap();
    save_atomic(path, &bytes).unwrap();
}

fn found(r: Resolution) -> soliddraft_document::Target {
    match r {
        Resolution::Found(t) => t,
        Resolution::Missing(m) => panic!("missing: {m:?}"),
    }
}

#[test]
fn a_reference_follows_renames_and_moves_and_reports_a_missing_file() {
    let dir = Dir::new("moves");
    let id = DocumentId::new();
    let rev = write_part(&dir.path("bracket.dwprt"), id, 10.0);
    let drawing = dir.path("bracket.dwdrw");
    let r = DocRef::new(id, DocKind::Part, "bracket.dwprt", rev.clone());

    // Where it was saved: found by its stored path, at the pinned revision.
    let t = found(Resolver::new(Some(&drawing)).resolve(&r));
    assert_eq!((t.via, t.status(&r), t.new_path.clone()), (Via::StoredPath, RefStatus::Current, None));

    // Renamed in the same folder: found by id through the index, with the new path.
    std::fs::rename(dir.path("bracket.dwprt"), dir.path("plate.dwprt")).unwrap();
    let t = found(Resolver::new(Some(&drawing)).resolve(&r));
    assert_eq!(t.via, Via::Index);
    assert_eq!(t.new_path.as_deref(), Some("plate.dwprt"));
    assert_eq!(t.status(&r), RefStatus::Current);

    // Moved into a subfolder.
    std::fs::create_dir_all(dir.path("parts")).unwrap();
    std::fs::rename(dir.path("plate.dwprt"), dir.path("parts/plate.dwprt")).unwrap();
    let t = found(Resolver::new(Some(&drawing)).resolve(&r));
    assert_eq!((t.via, t.new_path.as_deref()), (Via::Index, Some("parts/plate.dwprt")));

    // Moved out of the drawing's folder: found only through a search folder.
    let library = Dir::new("moves-library");
    std::fs::rename(dir.path("parts/plate.dwprt"), library.path("plate.dwprt")).unwrap();
    assert!(matches!(Resolver::new(Some(&drawing)).resolve(&r), Resolution::Missing(_)));
    let t = found(Resolver::new(Some(&drawing)).with_search_folders([&library.0]).resolve(&r));
    assert_eq!(t.via, Via::Index);
    assert_eq!(t.path(), Some(library.path("plate.dwprt").as_path()));

    // Deleted: missing, with the other parts in reach offered as candidates, the same
    // name first.
    std::fs::remove_file(library.path("plate.dwprt")).unwrap();
    write_part(&dir.path("parts/zeta.dwprt"), DocumentId::new(), 1.0);
    write_part(&dir.path("parts/bracket.dwprt"), DocumentId::new(), 1.0);
    let Resolution::Missing(m) = Resolver::new(Some(&drawing)).resolve(&r) else { panic!("should be missing") };
    assert_eq!(m.stored, StoredPathProblem::NotFound);
    let names: Vec<_> = m.candidates.iter().map(|c| c.path.file_name().unwrap().to_owned()).collect();
    assert_eq!(names, ["bracket.dwprt", "zeta.dwprt"]);
    assert!(m.candidates[0].same_name && !m.candidates[1].same_name);
}

#[test]
fn a_same_named_file_with_another_id_is_never_taken() {
    let dir = Dir::new("impostor");
    let id = DocumentId::new();
    let rev = write_part(&dir.path("bracket.dwprt"), id, 10.0);
    let drawing = dir.path("d.dwdrw");
    let r = DocRef::new(id, DocKind::Part, "bracket.dwprt", rev);
    // Another part saved over the name.
    let other = DocumentId::new();
    write_part(&dir.path("bracket.dwprt"), other, 10.0);
    let Resolution::Missing(m) = Resolver::new(Some(&drawing)).resolve(&r) else { panic!("an impostor was accepted") };
    assert_eq!(m.stored, StoredPathProblem::DifferentDocument(other));
    assert_eq!(m.candidates.len(), 1, "it is offered for repair, not taken");
    // A drawing at the path is the wrong kind.
    write_drawing(&dir.path("bracket.dwprt"), &r);
    let Resolution::Missing(m) = Resolver::new(Some(&drawing)).resolve(&r) else { panic!("a drawing was accepted") };
    assert_eq!(m.stored, StoredPathProblem::WrongKind(DocKind::Drawing));
}

#[test]
fn an_edited_part_reads_as_newer_and_an_open_one_wins_over_its_file() {
    let dir = Dir::new("edits");
    let id = DocumentId::new();
    let pinned = write_part(&dir.path("bracket.dwprt"), id, 10.0);
    let drawing = dir.path("d.dwdrw");
    let mut r = DocRef::new(id, DocKind::Part, "bracket.dwprt", pinned.clone());
    let saved = write_part(&dir.path("bracket.dwprt"), id, 12.0);
    let t = found(Resolver::new(Some(&drawing)).resolve(&r));
    assert_eq!(t.status(&r), RefStatus::NewerAvailable { now: saved.clone() });

    // Open with unsaved edits: the open document wins.
    let unsaved = ContentHash::of_json(&truth(14.0));
    let open =
        OpenDocument { id, kind: DocKind::Part, path: Some(dir.path("bracket.dwprt")), revision: unsaved.clone() };
    let t = found(Resolver::new(Some(&drawing)).with_open([open.clone()]).resolve(&r));
    assert_eq!(t.via, Via::OpenDocument);
    assert!(matches!(&t.location, Location::Open(d) if *d == open));
    assert_eq!(t.status(&r), RefStatus::NewerAvailable { now: unsaved.clone() });

    // A Rebuild re-pins: the reference is current again.
    assert!(repin(&mut r, &t));
    assert_eq!(r.pinned, unsaved);
    assert_eq!(t.status(&r), RefStatus::Current);
}

#[test]
fn a_reference_to_a_dwpart_follows_its_dwprt_successor() {
    let dir = Dir::new("migrated");
    let id = DocumentId::new();
    let old = write_part(&dir.path("bracket.dwpart"), id, 10.0);
    let new = write_part(&dir.path("bracket.dwprt"), id, 10.0);
    assert_eq!(old, new);
    let drawing = dir.path("d.dwdrw");
    let r = DocRef::new(id, DocKind::Part, "bracket.dwpart", old);
    let t = found(Resolver::new(Some(&drawing)).resolve(&r));
    assert_eq!((t.via, t.new_path.as_deref()), (Via::SavedSibling, Some("bracket.dwprt")));
}

#[test]
fn unbound_references_resolve_by_path_and_bind() {
    let dir = Dir::new("unbound");
    let drawing = dir.path("d.dwdrw");
    // A test recipe is not a document: found, without an identity.
    std::fs::write(dir.path("bracket.json"), "{}").unwrap();
    let recipe = DocRef::unbound(DocKind::Part, "bracket.json", ContentHash::of_bytes(b"{}"));
    let t = found(Resolver::new(Some(&drawing)).resolve(&recipe));
    assert_eq!((t.via, t.status(&recipe), t.id()), (Via::StoredPath, RefStatus::Unverified, None));

    // A migrated reference to a part binds on the first Rebuild.
    let id = DocumentId::new();
    let rev = write_part(&dir.path("bracket.dwprt"), id, 10.0);
    let mut r = DocRef::unbound(DocKind::Part, "bracket.dwprt", ContentHash::foreign("fnv64", "00ff"));
    let t = found(Resolver::new(Some(&drawing)).resolve(&r));
    assert!(matches!(t.status(&r), RefStatus::NewerAvailable { .. }), "an old digest never matches");
    assert!(repin(&mut r, &t));
    assert_eq!((r.id, r.pinned.clone()), (Some(id), rev));
}

#[test]
fn repairs_check_the_kind_and_ask_before_taking_another_document() {
    let dir = Dir::new("repair");
    let id = DocumentId::new();
    let pinned = write_part(&dir.path("bracket.dwprt"), id, 10.0);
    let drawing = dir.path("d.dwdrw");
    let mut r = DocRef::new(id, DocKind::Part, "gone/bracket.dwprt", pinned.clone());

    // The same part, wherever the user found it.
    let identity = repair(&mut r, &drawing, &dir.path("bracket.dwprt"), false).unwrap();
    assert_eq!((identity.id, r.path.as_str(), r.pinned.clone()), (id, "bracket.dwprt", pinned.clone()));

    // Another part only on purpose; the pinned revision stays until a Rebuild.
    let other = DocumentId::new();
    write_part(&dir.path("other.dwprt"), other, 5.0);
    let err = repair(&mut r, &drawing, &dir.path("other.dwprt"), false).unwrap_err();
    assert!(err.to_string().contains("different document"), "{err}");
    repair(&mut r, &drawing, &dir.path("other.dwprt"), true).unwrap();
    assert_eq!((r.id, r.path.as_str(), r.pinned.clone()), (Some(other), "other.dwprt", pinned));

    // Never a document of another kind.
    write_drawing(&dir.path("x.dwdrw"), &r);
    assert!(repair(&mut r, &drawing, &dir.path("x.dwdrw"), true).is_err());
}

#[test]
fn the_index_answers_where_used() {
    let dir = Dir::new("where-used");
    let id = DocumentId::new();
    let rev = write_part(&dir.path("parts/bracket.dwprt"), id, 10.0);
    write_part(&dir.path("parts/other.dwprt"), DocumentId::new(), 1.0);
    let by_id = DocRef::new(id, DocKind::Part, "../parts/bracket.dwprt", rev.clone());
    write_drawing(&dir.path("drawings/a.dwdrw"), &by_id);
    let by_path = DocRef::unbound(DocKind::Part, "parts/bracket.dwprt", rev);
    write_drawing(&dir.path("b.dwdrw"), &by_path);
    let index = FolderIndex::scan([&dir.0]);
    assert_eq!(index.entries().len(), 4);
    let mut users: Vec<_> = index.where_used(id).iter().map(|e| e.path.file_name().unwrap().to_owned()).collect();
    users.sort();
    assert_eq!(users, ["a.dwdrw", "b.dwdrw"]);
    assert_eq!(index.of_kind(DocKind::Drawing).count(), 2);
}
