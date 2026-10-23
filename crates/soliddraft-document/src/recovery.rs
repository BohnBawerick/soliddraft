//! Recovery copies: what autosave writes while a document has unsaved changes, so the
//! work survives a crash.
//!
//! A recovery copy is one file in a recovery folder, `<id>.<extension>.recovery`, such
//! as `3f0c2a4e-….dwprt.recovery`. It is a zip of two entries:
//!
//! - `recovery.json`: the [`Record`]: the document's id, kind and name, the path of the
//!   user's file (none for a document never saved), when the copy was written and the
//!   copy's revision.
//! - `document`: the document file's bytes, stored as the document's own writer made
//!   them. Restoring opens these bytes as the document.
//!
//! Copies are written atomically ([`save_atomic`]) into the recovery folder only; the
//! user's own file is never touched. A copy that cannot be read whole (a torn write
//! from before atomic saves, a full disk, a file from a newer SolidDraft) is reported
//! by [`scan`] and skipped, never opened.
//!
//! Which copies to offer after a crash is [`RecoveryCopy::standing`]: a copy of a
//! document never saved, or whose file is gone, or whose file has another revision and
//! was last written before the copy. The app keeps one recovery folder per running
//! session and decides which sessions ended without closing.

use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter};

use crate::hash::ContentHash;
use crate::id::DocumentId;
use crate::kind::{DocKind, format_major};
use crate::manifest::{EnvelopeError, identify, identify_file};
use crate::save::save_atomic;

/// The format tag of `recovery.json`.
pub const FORMAT: &str = "soliddraft-recovery/1";
/// The extension recovery copies end in, without the dot.
pub const EXTENSION: &str = "recovery";

const RECORD: &str = "recovery.json";
const DOCUMENT: &str = "document";

/// `recovery.json`: what a recovery copy holds and where its document lives.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// Always [`FORMAT`] for copies this version writes.
    pub format: String,
    /// The document's id.
    pub id: DocumentId,
    /// The document's kind.
    pub kind: DocKind,
    /// The name the app showed for the document.
    pub name: String,
    /// The user's file, or `None` for a document that was never saved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// When the copy was written, in milliseconds since the Unix epoch.
    pub written_ms: u64,
    /// The copy's revision: the semantic hash of the document it holds.
    pub revision: ContentHash,
}

impl Record {
    /// A record for a copy written now.
    pub fn new(id: DocumentId, kind: DocKind, name: &str, path: Option<&Path>, revision: ContentHash) -> Record {
        Record {
            format: FORMAT.to_owned(),
            id,
            kind,
            name: name.to_owned(),
            path: path.map(Path::to_path_buf),
            written_ms: millis(SystemTime::now()),
            revision,
        }
    }

    /// When the copy was written.
    pub fn written(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_millis(self.written_ms)
    }
}

/// Why a recovery copy could not be written or read.
#[derive(Debug, thiserror::Error)]
pub enum RecoveryError {
    /// Reading or writing failed.
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// The copy is not a zip archive, or is cut short.
    #[error("damaged: {0}")]
    Archive(#[from] zip::result::ZipError),
    /// `recovery.json` is not valid.
    #[error("damaged record: {0}")]
    Json(#[from] serde_json::Error),
    /// The copy is not one this version reads, or its parts disagree.
    #[error("{0}")]
    Invalid(String),
}

impl From<EnvelopeError> for RecoveryError {
    fn from(e: EnvelopeError) -> RecoveryError {
        RecoveryError::Invalid(format!("the document inside is damaged: {e}"))
    }
}

/// The file name of the recovery copy of a document.
pub fn file_name(id: DocumentId, kind: DocKind) -> String {
    format!("{id}.{}.{EXTENSION}", kind.extension())
}

/// Writes the recovery copy of `document` (a document file's bytes) into `folder`,
/// atomically, and returns its path.
pub fn write(folder: &Path, record: &Record, document: &[u8]) -> Result<PathBuf, RecoveryError> {
    let deflated = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .last_modified_time(DateTime::default());
    // The document is already a zip; compressing it again gains nothing.
    let stored = deflated.compression_method(CompressionMethod::Stored);
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file(RECORD, deflated)?;
    zip.write_all(serde_json::to_string_pretty(record)?.as_bytes())?;
    zip.start_file(DOCUMENT, stored)?;
    zip.write_all(document)?;
    let bytes = zip.finish()?.into_inner();
    let path = folder.join(file_name(record.id, record.kind));
    save_atomic(&path, &bytes)?;
    Ok(path)
}

/// Reads a recovery copy: its record and the document's bytes. The document must be a
/// document file with the record's id and kind.
pub fn read(file: &Path) -> Result<(Record, Vec<u8>), RecoveryError> {
    let mut zip = ZipArchive::new(std::io::BufReader::new(std::fs::File::open(file)?))?;
    let record: Record = serde_json::from_slice(&entry(&mut zip, RECORD)?)?;
    if format_major(&record.format, "soliddraft-recovery") != Some(1) {
        return Err(RecoveryError::Invalid(format!("the copy is {}, this version reads {FORMAT}", record.format)));
    }
    let document = entry(&mut zip, DOCUMENT)?;
    let identity = identify(Cursor::new(&document))?;
    if identity.id != record.id || identity.kind != record.kind {
        return Err(RecoveryError::Invalid("the document inside is not the one the record names".into()));
    }
    Ok((record, document))
}

fn entry<R: Read + std::io::Seek>(zip: &mut ZipArchive<R>, name: &str) -> Result<Vec<u8>, RecoveryError> {
    let mut bytes = Vec::new();
    zip.by_name(name)?.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Deletes the recovery copy of a document from `folder`. A copy that is not there is
/// not an error.
pub fn remove(folder: &Path, id: DocumentId, kind: DocKind) -> std::io::Result<()> {
    match std::fs::remove_file(folder.join(file_name(id, kind))) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// A recovery copy found in a folder.
#[derive(Clone, Debug, PartialEq)]
pub struct RecoveryCopy {
    /// Its record.
    pub record: Record,
    /// The copy's file.
    pub file: PathBuf,
}

/// Where a recovery copy stands against the user's file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standing {
    /// The document was never saved: the copy is all there is.
    NeverSaved,
    /// The document's file is no longer where it was.
    FileMissing,
    /// The copy has changes the file does not: another revision, written after the
    /// file was last saved, or the file cannot be read.
    Newer,
    /// The file has the copy's revision: nothing to recover.
    Saved,
    /// The file has another revision and was saved after the copy was written.
    Older,
}

impl Standing {
    /// True when the copy holds work the file does not, so it is offered for restore.
    pub fn offered(self) -> bool {
        matches!(self, Standing::NeverSaved | Standing::FileMissing | Standing::Newer)
    }
}

impl RecoveryCopy {
    /// Where the copy stands against the user's file now.
    pub fn standing(&self) -> Standing {
        let Some(path) = &self.record.path else { return Standing::NeverSaved };
        let Ok(meta) = std::fs::metadata(path) else { return Standing::FileMissing };
        match identify_file(path) {
            Ok(saved) if saved.revision == self.record.revision => Standing::Saved,
            Ok(_) if meta.modified().is_ok_and(|m| millis(m) > self.record.written_ms) => Standing::Older,
            _ => Standing::Newer,
        }
    }

    /// The document's bytes, read again from the copy.
    pub fn document(&self) -> Result<Vec<u8>, RecoveryError> {
        read(&self.file).map(|(_, bytes)| bytes)
    }
}

/// A file in a recovery folder that could not be read as a recovery copy.
#[derive(Clone, Debug, PartialEq)]
pub struct Damaged {
    /// The file.
    pub file: PathBuf,
    /// What was wrong with it.
    pub error: String,
}

/// What a recovery folder holds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scan {
    /// The copies that read whole, newest first.
    pub copies: Vec<RecoveryCopy>,
    /// The files that did not.
    pub damaged: Vec<Damaged>,
}

/// Reads every recovery copy in `folder`. A damaged copy is listed in
/// [`Scan::damaged`] and skipped; a folder that does not exist holds nothing.
pub fn scan(folder: &Path) -> Scan {
    let mut found = Scan::default();
    let Ok(entries) = std::fs::read_dir(folder) else { return found };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == EXTENSION))
        .collect();
    files.sort();
    for file in files {
        match read(&file) {
            Ok((record, _)) => found.copies.push(RecoveryCopy { record, file }),
            Err(e) => found.damaged.push(Damaged { file, error: e.to_string() }),
        }
    }
    found.copies.sort_by_key(|c| std::cmp::Reverse(c.record.written_ms));
    found
}

fn millis(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Entry, Manifest, Section, write_package};

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("soliddraft-recovery-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A part file whose truth is `truth`.
    fn part(id: DocumentId, truth: &[u8]) -> (Vec<u8>, ContentHash) {
        let revision = ContentHash::of_bytes(truth);
        let mut m = Manifest::new(id, DocKind::Part, revision.clone());
        m.sections = vec![Section::required("part.json")];
        (write_package(&m, &[Entry::new("part.json", truth)]).unwrap(), revision)
    }

    #[test]
    fn copies_round_trip_and_are_replaced_in_place() {
        let dir = folder("round-trip");
        let id = DocumentId::new();
        let (bytes, revision) = part(id, b"{\"v\":1}");
        let record = Record::new(id, DocKind::Part, "Bracket", None, revision);
        let file = write(&dir, &record, &bytes).unwrap();
        assert_eq!(file.file_name().unwrap().to_string_lossy(), format!("{id}.dwprt.recovery"));
        assert_eq!(read(&file).unwrap(), (record.clone(), bytes));
        let (newer, revision) = part(id, b"{\"v\":2}");
        write(&dir, &Record::new(id, DocKind::Part, "Bracket", None, revision), &newer).unwrap();
        let found = scan(&dir);
        assert_eq!(found.copies.len(), 1, "one copy per document");
        assert_eq!(found.copies[0].document().unwrap(), newer);
        assert_eq!(found.copies[0].standing(), Standing::NeverSaved);
        remove(&dir, id, DocKind::Part).unwrap();
        remove(&dir, id, DocKind::Part).unwrap();
        assert_eq!(scan(&dir), Scan::default());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_copy_is_offered_only_when_it_is_newer_than_the_file() {
        let dir = folder("standing");
        let id = DocumentId::new();
        let user = dir.join("user/bracket.dwprt");
        let (saved, saved_revision) = part(id, b"{\"v\":1}");
        save_atomic(&user, &saved).unwrap();
        let (edited, edited_revision) = part(id, b"{\"v\":2}");
        let standing = |revision: &ContentHash, written: SystemTime| {
            let mut record = Record::new(id, DocKind::Part, "bracket", Some(&user), revision.clone());
            record.written_ms = millis(written);
            let file = write(&dir, &record, if *revision == saved_revision { &saved } else { &edited }).unwrap();
            RecoveryCopy { record, file }.standing()
        };
        let later = SystemTime::now() + Duration::from_secs(60);
        let earlier = SystemTime::now() - Duration::from_secs(3600);
        assert_eq!(standing(&edited_revision, later), Standing::Newer);
        assert_eq!(standing(&saved_revision, later), Standing::Saved, "the file holds it already");
        assert_eq!(standing(&edited_revision, earlier), Standing::Older, "the file was saved since");
        std::fs::write(&user, b"not a zip").unwrap();
        assert_eq!(standing(&edited_revision, earlier), Standing::Newer, "an unreadable file loses to the copy");
        std::fs::remove_file(&user).unwrap();
        assert_eq!(standing(&edited_revision, earlier), Standing::FileMissing);
        assert!(Standing::Newer.offered() && !Standing::Saved.offered() && !Standing::Older.offered());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn damaged_copies_are_reported_and_skipped() {
        let dir = folder("damaged");
        let id = DocumentId::new();
        let (bytes, revision) = part(id, b"{}");
        let good = write(&dir, &Record::new(id, DocKind::Part, "good", None, revision.clone()), &bytes).unwrap();
        // Cut short, as a write without the rename could have left it.
        let other = DocumentId::new();
        let (bytes, _) = part(other, b"{}");
        let torn = write(&dir, &Record::new(other, DocKind::Part, "torn", None, revision.clone()), &bytes).unwrap();
        let whole = std::fs::read(&torn).unwrap();
        std::fs::write(&torn, &whole[..whole.len() / 2]).unwrap();
        // Not a zip at all.
        std::fs::write(dir.join("noise.dwdrw.recovery"), b"noise").unwrap();
        // A zip whose document is someone else's.
        let stranger = DocumentId::new();
        let (bytes, _) = part(DocumentId::new(), b"{}");
        write(&dir, &Record::new(stranger, DocKind::Part, "stranger", None, revision), &bytes).unwrap();
        // Files that are not copies are none of the scan's business.
        std::fs::write(dir.join("session.lock"), b"").unwrap();

        let found = scan(&dir);
        assert_eq!(found.copies.iter().map(|c| &c.file).collect::<Vec<_>>(), [&good]);
        let mut damaged: Vec<_> = found.damaged.iter().map(|d| d.file.file_name().unwrap().to_owned()).collect();
        damaged.sort();
        let mut expected = vec![
            torn.file_name().unwrap().to_owned(),
            "noise.dwdrw.recovery".into(),
            file_name(stranger, DocKind::Part).into(),
        ];
        expected.sort();
        assert_eq!(damaged, expected);
        assert!(found.damaged.iter().all(|d| !d.error.is_empty()));
        let _ = std::fs::remove_dir_all(dir);
    }
}
