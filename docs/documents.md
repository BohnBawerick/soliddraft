# Documents and references

Parts, assemblies and drawings are separate files. A drawing or an assembly
references the parts it shows, by id. This page is the schema of those files and the rules
for following a reference. The code is `crates/soliddraft-document`; the part and drawing
crates write their files through it.

## Kinds and file names

| Kind | Saved as | Also opens | Truth entry | Format |
|---|---|---|---|---|
| Part | `.dwprt` | `.dwpart` | `part.json` | `soliddraft-part/2` |
| Assembly | `.dwasm` | | `assembly.json` | `soliddraft-assembly/1` |
| Drawing | `.dwdrw` | `.dwdrawing` | `drawing.json` | `soliddraft-drawing/2` |

A file under an old extension opens, and saving it writes the current name beside it. The
old file stays where it was. Assemblies are described in [assemblies.md](assemblies.md).

Files with `draftwell-document`, `draftwell-part`, `draftwell-assembly` or
`draftwell-drawing` format prefixes still open. Saving writes the SolidDraft prefixes
and keeps the document id and revision. Document settings also accept the earlier
`draftwell-settings/1` tag. The supported major versions have not changed.

## Damaged and newer files

A file that is truncated, is not an archive, has an entry that does not parse or is missing one is refused with an error that says the file is damaged. A file from a newer SolidDraft (a newer envelope, a format major version this build does not read, a required section it does not know) or of another kind is refused with an error that says so. A truth entry with no format tag says that it has none. Nothing that refuses a file writes to it, replaces it or leaves a stray file beside it; the tests are in `crates/soliddraft-cli/tests/damaged_files.rs` (see [ci.md](ci.md)).

## The envelope

Every document file is a zip whose first entry is `manifest.json`:

```json
{
  "envelope": "soliddraft-document/1",
  "id": "3f0c2a4e-7b1d-4c55-9a0e-2d6f8e1b9c47",
  "kind": "part",
  "format": "soliddraft-part/2",
  "writer": "soliddraft 0.1.0",
  "kernel": "smlib c9c5979",
  "semantic_hash": "blake3:...",
  "geometry_hash": "blake3:...",
  "sections": [
    { "name": "part.json", "required": true }
  ],
  "references": []
}
```

- `id` is a random UUID (version 4) minted when the document is created. Renaming or moving
  the file keeps it. Saving under another name in the app keeps it too; only Save as copy
  mints a new one (`soliddraft_model::Document::new_identity`). The id is not the file name
  and not the part's name.
- `format` is the truth entry's format. Its major version is the number after the slash. A
  reader refuses a major version it does not know.
- `kernel` names the backend whose results the caches hold. Opening never depends on it.
- `sections` lists every entry, or every entry under a folder when the name ends in `/`. A
  reader refuses a file with a required section it does not know, because it would damage
  it. It drops optional sections it does not know, such as a newer build's caches. The
  writer refuses to write an entry that no section covers.
- `references` copies the references the truth holds, so an index can answer "where used"
  from manifests alone.

A part file can also have `settings.json` (optional): viewport quality, export accuracy
and whether to save display data, written only when one differs from its default. It is
outside both hashes, so changing a setting never gives the part a new revision. A reader
that cannot read it opens the part with the default settings. With "save display data"
on (the default), it also has `cache/mesh/coarse.bin` and `cache/mesh/standard.bin`
(optional): its meshes at two display levels. See [tessellation.md](tessellation.md).

The drawing file has `drawing.json` (required), `cache/views/<id>.json` (optional, each
view's line work) and `thumbnail.png` (optional). Entries carry a fixed timestamp, so the
same document gives the same bytes.

`thumbnail.png` is the same optional section in every kind: a small picture a start page
or a file browser can show without opening the document
(`soliddraft_document::thumbnail`). A drawing's is its first sheet, 512 pixels wide. A
part saved from the app gets a 512 by 384 picture from the viewport's own renderer,
drawn into a texture with the part fitted from the default view; a part saved without a
GPU, or from the command line, has none. Thumbnails are outside both hashes.

## Hashes

The semantic hash is the document's revision. It is BLAKE3 over the canonical JSON of the
truth entry without its `format` key: object keys sorted, no whitespace, numbers as
`serde_json` writes them. Caches, thumbnails, the writer and the format tag are not in it,
so migrating a file does not change its revision, and two files with the same design have
the same revision however they were formatted. Several truth entries hash in name order,
each preceded by its name.

The geometry hash leaves out what does not decide the shape. For a part that is the part's
name, each feature's name and `via_assistant` flag, every `label` (the words a
reference keeps for repair prompts), and the material and appearances (`material`,
`appearances` and `snapshots`, see [materials.md](materials.md)). Renaming a feature or
painting a face changes the semantic hash and not the geometry hash.

Neither hash depends on Rust `Debug` output.

## References

```json
{
  "id": "3f0c2a4e-7b1d-4c55-9a0e-2d6f8e1b9c47",
  "kind": "part",
  "path": "../parts/bracket.dwprt",
  "pinned": "blake3:...",
  "configuration": "c-long",
  "display_state": "exploded",
  "locator": "..."
}
```

- `id` is what the target must be. A reference without an id is unbound: it was migrated
  from a drawing that recorded none, or it points at a test recipe, which has none. It
  resolves by its path only and is bound the first time it resolves to a document.
- `path` is relative to the referencing file's folder, with forward slashes.
- `pinned` is the target's semantic hash when the referencing document was last rebuilt
  against it. The referencing document shows that revision until a Rebuild.
- `configuration` and `display_state` are reserved and left out when empty. Phase 0 writes
  neither and keeps any value it reads. A configuration is a named set of overrides on a
  part or assembly; a display state (visibility, colours, an exploded pose) is a separate
  mechanism, so a colour change never invalidates geometry.
- `locator` is reserved for a revision store's own address of the pinned revision, such as
  a Git commit.

A drawing's `model` is a reference. Phase 2 turns it into a list (`models`), one per part
or assembly on the sheet.

## Finding a reference's target

`Resolver::resolve` tries, in order:

1. a document open in this session with the reference's id, because an edited, unsaved part
   must win over its file;
2. the stored path, accepted only when the file there has the reference's id and kind. When
   the stored path has an old extension, the file beside it under the current one comes
   first if it has the same id: it is the same document, saved since;
3. the folder index: the referencing file's folder and its subfolders, then the
   referenced-documents folders from File locations. This finds renamed and moved files.
   When copies share the id, the one at the pinned revision wins, then the nearest;
4. otherwise the reference is missing.

A file with the right name and another id is never taken. It shows up as a repair
candidate. Resolving never rewrites the stored path: a target found elsewhere reports its
new relative path (`Target::new_path`), and the referencing document stores it when it is
next saved or rebuilt (`resolve::rebind`).

A found target's status against the pin is `Current`, `NewerAvailable { now }` or
`Unverified` (a recipe, which has no revision). A missing one carries what was wrong at the
stored path and the candidates: documents of the reference's kind in reach, the same file
name first.

`resolve::repair` points a reference at a file the user picked. The file must have the
reference's kind, and the same id unless the user asked to replace the document. The
pinned revision stays, so the drawing goes on showing what it showed until a Rebuild.
`resolve::repin` is what a Rebuild does to each reference: rebind, then pin at the target's
current revision.

## Pinned revisions and Rebuild

A drawing opens showing the part revision it is pinned at: its views' line work is cached
in the file and was made from that revision. An edited part changes nothing in the drawing
until a Rebuild (`soliddraft draw --update`, `soliddraft doc rebuild`, Update views in the
app) regenerates the views, re-resolves the annotations and re-pins the reference. While
the part and the drawing are both open in the app, automatic update still follows each
rebuild of that part, as before.

Exact work on a pinned revision that is not current (measuring it, projecting a new view
of it) needs that revision's bytes. They come from a `RevisionStore`:

```rust
pub trait RevisionStore: Send + Sync {
    fn name(&self) -> &'static str;
    fn fetch(&self, id: DocumentId, revision: &ContentHash) -> Result<Vec<u8>, StoreError>;
    fn record(&self, id: DocumentId, revision: &ContentHash, bytes: &[u8]);
}
```

Phase 0 has `CacheOnlyStore`: the revisions this session opened or saved, nothing older.
When it does not have one, the error says to Rebuild. A Git store (desktop only) comes
next, and a SolidDraft PDM later; the screens stay the same whichever store is behind them.

## File locations

One setting holds, per kind of reference file, an ordered list of folders and files. The
app keeps it under `file_locations` in `settings.json`:

```json
{
  "file_locations": {
    "referenced_documents": ["/shared/parts"],
    "drawing_templates": ["/shared/templates", "/shared/templates/a3-custom.toml"],
    "drafting_styles": ["/shared/styles"]
  }
}
```

| Key | What | Files |
|---|---|---|
| `referenced_documents` | folders searched for moved references | searched, not listed |
| `material_libraries` | material libraries | `.toml` |
| `appearance_libraries` | appearance libraries | `.toml` |
| `part_templates` | templates for new parts | `.dwprt` |
| `assembly_templates` | templates for new assemblies | `.dwasm` |
| `drawing_templates` | sheet templates for new drawings | `.toml` |
| `sheet_formats` | sheet formats and title blocks | `.toml` |
| `drafting_styles` | dimension styles | `.toml` |
| `configuration_tables` | configuration tables | `.csv`, `.toml` |

Built-in content stays compiled in, and these add to it. A listed folder contributes its
files with the kind's extensions, sorted by name. Keys a newer build added are kept when an
older one saves.

## Reserved for future formats

- **Configurations and display states.** The `DocRef` fields above, and the
  `ConfigurationId` and `DisplayStateId` types.
- **Configuration scopes.** Every material and appearance assignment in a part carries a
  `ConfigScope`: `"scope": "all"` (the default) or
  `"scope": "only", "configurations": ["c1"]` (see [materials.md](materials.md)).
  Assembly-level appearances carry one too (see [assemblies.md](assemblies.md#colours)).

## Files from before phase 0

- A `.dwpart` part (`recipe.json` with `soliddraft-part/1` and `meta.json`) opens. Its id
  is derived from its content (a version 8 UUID from the kind and the semantic hash), so
  every open of the same unchanged file agrees on it, and the first save writes it into
  the manifest. Two byte-identical old files get the same id, as two copies of a new file
  do.
- A `.dwdrawing` drawing (`meta.json` with `soliddraft-drawing/1`) opens the same way. Its
  model reference held the part's display name as the id and an FNV hash of `Debug` text
  as the digest. It becomes an unbound reference with the same path, pinned at
  `fnv64:<old digest>`, which never equals a current revision. The drawing shows its
  cached views and reads as older than its part until the first Rebuild binds the
  reference and re-pins it.
- `soliddraft doc migrate <file>...` writes old files in the current format beside them.

## Saving

Every document writer, the drawing exports and the settings file go through
`save_atomic`: the bytes go to a hidden temporary file in the target's folder, are flushed,
and the temporary file is renamed over the target. A failed save leaves the previous file
whole.

## Backups

Saving from the app keeps the previous version of the file when the new bytes differ
(`soliddraft_document::Backups`). Backup 1 is the newest; older ones move up a number and
those past the count are deleted. Saving unchanged bytes keeps the backups as they are.
Backups sit beside the document as `bracket.dwprt.1.bak`, or in a backup folder as
`bracket.dwprt.<folder tag>.1.bak`, where the tag tells apart documents with the same
name in different folders. The `.bak` ending keeps a backup out of the folder index, so a
reference never resolves to one. A backup that cannot be made does not stop the save;
the status bar says so.

## Autosave and recovery

While a part or drawing has unsaved changes, autosave writes a recovery copy of it into
this run's session folder, `recovery/session-<pid>-<time>/` in the config directory. A
copy is due when the interval has passed since the first unsaved edit, or sooner after
the set number of edits (`soliddraft_document::autosave`). The copy is made without the
display meshes or the drawing's thumbnail, and a writer thread puts it on disk, so the
interface does not wait. It is written atomically and never touches the user's file.
Saving a document, or replacing it with New or Open, deletes its copy; quitting deletes
the session folder.

A recovery copy, `<id>.dwprt.recovery` or `<id>.dwdrw.recovery`, is a zip of
`recovery.json` (`soliddraft-recovery/1`: id, kind, name, the file's path, when it was
written, its revision) and `document`, the document file's bytes.

Each running app holds a lock on its session folder's `session.lock`. At start, a session
folder whose lock is free belonged to an app that ended without quitting. Its copies are
offered for restore when the document was never saved, its file is gone or unreadable,
or the file has another revision and was saved before the copy was written. Copies whose
work the file already holds are deleted. Damaged copies are reported once and moved to
`recovery/damaged/`. A restored document opens with unsaved changes at its old path, and
its copy stays protected until it is saved. Offers left undecided stay for the next
start.

The settings file holds the policy under `documents`:

```json
{
  "documents": {
    "autosave": { "enabled": true, "interval_seconds": 120, "edits": 10 },
    "recovery_folder": "/fast/disk/soliddraft-recovery",
    "backups": { "count": 1, "folder": "/backups/soliddraft" }
  }
}
```

Every key is optional. `edits: 0` waits for the interval only, and `count: 0` keeps no
backups. The browser build has no autosave or backups: its files are downloads.

## Command line

```
soliddraft doc info <file> [--json]
soliddraft doc refs <file> [--search <folder>]... [--json]
soliddraft doc rebuild <drawing> [--search <folder>]... [--json]
soliddraft doc repair <drawing> --to <part> [--replace] [--json]
soliddraft doc migrate <file>... [--json]
soliddraft doc where-used <file> [--search <folder>]... [--json]
soliddraft locations [list] [--json]
soliddraft locations add|remove <kind> <path>
soliddraft draw <part.dwprt | recipe.json> ...
```

`doc refs` exits with status 1 when a reference is missing.

## In the app

- **Open** takes a drawing as well as a part (`File > Open`, a path on the command line,
  the recent list). A drawing opens showing the part revision it is pinned at. Its
  reference is resolved at once (`crates/soliddraft-app/src/drawing/reference.rs`), in the
  order above, and the answer is kept until something it was made from changes: the open
  part and its revision, either file's path, the stored reference or File locations.
  When the part is a file and the part tab holds nothing unsaved, the part opens in the
  part tab too. That is first contact: the drawing keeps its views and marks the newer
  version instead of updating by itself. Edits to the open part
  from then on follow the drawing's automatic update setting, as before.
- **Newer version available.** When the part's revision is not the pinned one, the
  marker shows in the drawing tree (the model row under the document, with the
  out-of-date glyph and the revision, and the header), the status bar, a bar over the
  sheet with a Rebuild button, the view rows and their hatched borders, and the Model
  reference panel (select the model row). The marker reads the resolver's answer, so it
  shows for a part that is not open, from its file.
- **Rebuild** sits where SolidWorks keeps it: `Edit > Rebuild drawing` (Ctrl+B, also U),
  the Model group of the Views tab, the bar over the sheet and the model row's menu. It
  re-projects the views, re-resolves the annotations, re-pins the reference at the part's
  current revision and binds it to where the part was found, as one undo step. When the
  part is not open but its file is found, the file opens in the part tab first and the
  rebuild follows. A missing part asks for a repair instead.
- **Repair reference.** A part that cannot be found opens the dialog, also reachable from
  the ribbon, the model row and the bar. It says what was wrong at the stored path (no
  file, a different part, another kind, unreadable) and that nothing with the id was in
  the drawing's folder or the search folders; lists the parts in reach as cards, the same
  name first, each with Use this part; takes a path typed or browsed by hand; and links
  to File locations, so a search folder can be added. A same-named file with another id
  is taken only through Replace model or the tick box. The pinned revision is kept, so
  the drawing goes on showing what it showed until a Rebuild. A reference that resolves
  while the dialog is open (a search folder was added) closes it. **Replace model** in the
  Model group is the same dialog with any part accepted.
- **File locations** (`Tools > File locations…`) is the page of the setting above: the
  kinds on the left, the folders and files of the chosen kind on the right, with Add
  folder, Add file, a typed path, Remove, Move up and Move down. Built-in content is
  listed read-only above the user's entries. Search folders apply when the page is saved;
  templates, sheet formats and styles are read when the app starts.
