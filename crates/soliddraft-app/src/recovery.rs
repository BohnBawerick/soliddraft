//! Autosave and crash recovery.
//!
//! Each running app keeps a session folder in the recovery folder,
//! `<recovery folder>/session-<pid>-<nanos>/`, and holds a lock on its `session.lock`
//! while it runs. Autosave writes the open part's and the open drawing's recovery copies
//! there (`soliddraft_document::recovery`) when [`Trigger`] says one is due. The bytes
//! are made on the interface thread without the slow entries (display meshes, the
//! drawing's thumbnail); a writer thread puts them on disk, so the interface never
//! waits for it. Saving a document deletes its copy, and so does replacing it with
//! another (New, Open). Quitting deletes the session folder.
//!
//! At start, a session folder whose lock is free belonged to an app that ended without
//! quitting. Its copies that are newer than their documents
//! ([`soliddraft_document::Standing::offered`]) move
//! into this session's `offered/` folder and are listed for restore; the others are
//! deleted. Damaged copies move to `<recovery folder>/damaged/` and are reported once.
//! An offer still undecided at quit stays in `offered/`, so the next start finds it.
//! The user's own files are never written.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use soliddraft_document::recovery::{self, Record, RecoveryCopy};
use soliddraft_document::{AutosavePolicy, DocKind, DocumentId, Trigger};

use crate::document::Document;
use crate::drawing::document::DrawingDoc;
use crate::state::{AppAction, AppState};

const LOCK: &str = "session.lock";
const OFFERED: &str = "offered";
const DAMAGED: &str = "damaged";
const SESSION_PREFIX: &str = "session-";

enum Job {
    Write(Record, Vec<u8>),
    Remove(DocumentId, DocKind),
    Flush(Sender<()>),
}

/// One document autosave watches: which it is, the edit count last seen and whether a
/// copy of it is on disk.
#[derive(Default)]
struct Watch {
    id: Option<DocumentId>,
    edits: u64,
    trigger: Trigger,
    written: bool,
}

/// The running session's recovery folder, its writer and what it found at start.
pub struct Recovery {
    root: PathBuf,
    dir: PathBuf,
    _lock: File,
    jobs: Option<Sender<Job>>,
    reports: Receiver<String>,
    writer: Option<JoinHandle<()>>,
    pub policy: AutosavePolicy,
    part: Watch,
    drawing: Watch,
    /// Copies from sessions that ended without quitting, newest first.
    pub offers: Vec<RecoveryCopy>,
    /// Damaged copies found at start, as sentences.
    pub problems: Vec<String>,
    /// Copies that could not be made or written since the app last looked, as sentences.
    pub failures: Vec<String>,
    /// The Recover documents window is open.
    pub show: bool,
}

impl Recovery {
    /// Starts this session's folder under `root` and takes over the folders of sessions
    /// that ended without quitting.
    pub fn start(root: &Path, policy: AutosavePolicy) -> std::io::Result<Recovery> {
        std::fs::create_dir_all(root)?;
        let (dir, lock) = claim(root)?;
        let (jobs, reports, writer) = writer(dir.clone());
        let mut recovery = Recovery {
            root: root.to_path_buf(),
            dir,
            _lock: lock,
            jobs: Some(jobs),
            reports,
            writer: Some(writer),
            policy,
            part: Watch::default(),
            drawing: Watch::default(),
            offers: Vec::new(),
            problems: Vec::new(),
            failures: Vec::new(),
            show: false,
        };
        recovery.adopt_ended_sessions();
        recovery.show = !recovery.offers.is_empty() || !recovery.problems.is_empty();
        Ok(recovery)
    }

    /// This session's folder.
    pub fn folder(&self) -> &Path {
        &self.dir
    }

    /// Looks at the open part and drawing after a frame: counts their edits, writes a
    /// copy of each that is due and deletes the copy of one that was saved or closed.
    /// Returns how long until the next copy is due, so an idle app can wake for it.
    pub fn tick(
        &mut self,
        part: &Document,
        drawing: Option<&DrawingDoc>,
        kernel: &str,
        now: Instant,
    ) -> Option<Duration> {
        self.collect_failures();
        let policy = self.policy;
        let jobs = self.jobs.clone();
        let send = |job| {
            if let Some(jobs) = &jobs {
                let _ = jobs.send(job);
            }
        };
        let id = part.model().id();
        if watch(&mut self.part, DocKind::Part, Some((id, part.edits, part.dirty)), &policy, now, &send) {
            let model = part.model();
            let extras = soliddraft_model::file::Extras { no_display_data: true, ..Default::default() };
            match model.to_bytes_with(&extras) {
                Ok(bytes) => {
                    let record =
                        Record::new(id, DocKind::Part, &part.name, part.path.as_deref(), model.semantic_hash());
                    send(Job::Write(record, bytes));
                }
                Err(e) => self.failures.push(format!("Autosave could not copy {}: {e}", part.name)),
            }
        }
        let seen = drawing.map(|d| (d.id, d.edits, d.dirty));
        if watch(&mut self.drawing, DocKind::Drawing, seen, &policy, now, &send)
            && let Some(d) = drawing
        {
            match d.to_bytes(kernel, false).and_then(|bytes| {
                let revision = d.drawing.semantic_hash().map_err(|e| e.to_string())?;
                Ok((Record::new(d.id, DocKind::Drawing, &d.drawing.meta.title, d.path.as_deref(), revision), bytes))
            }) {
                Ok((record, bytes)) => send(Job::Write(record, bytes)),
                Err(e) => self.failures.push(format!("Autosave could not copy {}: {e}", d.drawing.meta.title)),
            }
        }
        [&self.part, &self.drawing].into_iter().filter_map(|w| w.trigger.wait(&policy, now)).min()
    }

    /// Takes an offered copy back: returns its record and document bytes, and moves the
    /// copy into this session's folder as the document's own, so it stays protected
    /// until the restored document is saved. The caller opens the bytes and then
    /// reports the edit count with [`Recovery::restored`].
    pub fn restore(&mut self, file: &Path) -> Result<(Record, Vec<u8>), String> {
        let (record, bytes) = recovery::read(file).map_err(|e| format!("cannot restore {}: {e}", file.display()))?;
        let own = self.dir.join(recovery::file_name(record.id, record.kind));
        // The writer may hold an older copy of the same document; this one wins.
        self.flush();
        let _ = std::fs::remove_file(&own);
        std::fs::rename(file, &own).map_err(|e| format!("cannot restore {}: {e}", file.display()))?;
        self.offers.retain(|c| c.file != file);
        Ok((record, bytes))
    }

    /// The restored document is open with `edits` edits counted; its copy is on disk.
    pub fn restored(&mut self, id: DocumentId, kind: DocKind, edits: u64) {
        let watch = if kind == DocKind::Drawing { &mut self.drawing } else { &mut self.part };
        if let Some(old) = watch.id.filter(|old| *old != id && watch.written) {
            self.send(Job::Remove(old, kind));
        }
        let watch = if kind == DocKind::Drawing { &mut self.drawing } else { &mut self.part };
        *watch = Watch { id: Some(id), edits, trigger: Trigger::default(), written: true };
    }

    /// Deletes an offered copy the user does not want.
    pub fn discard(&mut self, file: &Path) -> Result<(), String> {
        self.offers.retain(|c| c.file != file);
        match std::fs::remove_file(file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(format!("cannot discard {}: {e}", file.display()))
            }
            _ => Ok(()),
        }
    }

    /// Waits until every copy asked for so far is on disk.
    pub fn flush(&mut self) {
        let (done, wait) = channel();
        self.send(Job::Flush(done));
        let _ = wait.recv();
        self.collect_failures();
    }

    /// The app is quitting: finishes the writes, then deletes this session's copies and
    /// its folder. Offers nobody decided on stay for the next start.
    pub fn close(mut self) {
        self.jobs = None;
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
        let scan = recovery::scan(&self.dir);
        for file in scan.copies.iter().map(|c| &c.file).chain(scan.damaged.iter().map(|d| &d.file)) {
            let _ = std::fs::remove_file(file);
        }
        let offered = self.dir.join(OFFERED);
        let _ = std::fs::remove_dir(&offered);
        if !offered.exists() {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn send(&self, job: Job) {
        if let Some(jobs) = &self.jobs {
            let _ = jobs.send(job);
        }
    }

    fn collect_failures(&mut self) {
        while let Ok(failure) = self.reports.try_recv() {
            log::warn!("{failure}");
            self.failures.push(failure);
        }
    }

    /// Takes over every session folder whose app is gone, and lists what it offers.
    fn adopt_ended_sessions(&mut self) {
        let offered = self.dir.join(OFFERED);
        let Ok(entries) = std::fs::read_dir(&self.root) else { return };
        for entry in entries.filter_map(Result::ok) {
            let folder = entry.path();
            let is_session = entry.file_name().to_string_lossy().starts_with(SESSION_PREFIX);
            if !is_session || folder == self.dir || !folder.is_dir() {
                continue;
            }
            let Ok(lock) = OpenOptions::new().read(true).write(true).open(folder.join(LOCK)) else {
                // A session that stopped before it made its lock left an empty folder.
                let _ = std::fs::remove_dir(&folder);
                continue;
            };
            if lock.try_lock().is_err() {
                continue; // That app is still running.
            }
            for sub in [folder.join(OFFERED), folder.clone()] {
                let scan = recovery::scan(&sub);
                for damaged in scan.damaged {
                    let name = damaged.file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    self.problems
                        .push(format!("A recovery copy is damaged and was skipped: {name} ({})", damaged.error));
                    let kept = self.root.join(DAMAGED);
                    let _ = std::fs::create_dir_all(&kept);
                    let _ = std::fs::rename(&damaged.file, kept.join(name));
                }
                for copy in scan.copies {
                    if copy.standing().offered() {
                        let _ = std::fs::create_dir_all(&offered);
                        let to = offered.join(copy.file.file_name().unwrap_or_default());
                        // Two ended sessions with the same document: the newer copy wins.
                        let newer = recovery::read(&to).is_ok_and(|(r, _)| r.written_ms > copy.record.written_ms);
                        if newer || std::fs::rename(&copy.file, &to).is_err() {
                            let _ = std::fs::remove_file(&copy.file);
                        }
                    } else {
                        let _ = std::fs::remove_file(&copy.file);
                    }
                }
            }
            drop(lock);
            let _ = std::fs::remove_dir_all(&folder);
        }
        // Offers kept from an earlier start are weighed again: the file may have been saved since.
        for copy in recovery::scan(&offered).copies {
            if copy.standing().offered() {
                self.offers.push(copy);
            } else {
                let _ = std::fs::remove_file(&copy.file);
            }
        }
    }
}

/// Makes and locks a new session folder under `root`.
fn claim(root: &Path) -> std::io::Result<(PathBuf, File)> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let dir = root.join(format!("{SESSION_PREFIX}{}-{nanos}", std::process::id()));
        let made = std::fs::create_dir(&dir).and_then(|()| File::create(dir.join(LOCK)));
        match made {
            Ok(lock) => match lock.try_lock() {
                Ok(()) => return Ok((dir, lock)),
                // Another app starting at the same moment took it for an ended session.
                Err(_) if attempt < 5 => continue,
                Err(e) => return Err(std::io::Error::other(e.to_string())),
            },
            Err(_) if attempt < 5 => continue,
            Err(e) => return Err(e),
        }
    }
}

/// Starts the writer thread for the session folder `dir`.
fn writer(dir: PathBuf) -> (Sender<Job>, Receiver<String>, JoinHandle<()>) {
    let (jobs, inbox) = channel::<Job>();
    let (report, failures) = channel();
    let thread = std::thread::Builder::new()
        .name("autosave".into())
        .spawn(move || {
            for job in inbox {
                let failed = match job {
                    Job::Write(record, bytes) => recovery::write(&dir, &record, &bytes)
                        .err()
                        .map(|e| format!("Autosave could not write the recovery copy of {}: {e}", record.name)),
                    Job::Remove(id, kind) => recovery::remove(&dir, id, kind)
                        .err()
                        .map(|e| format!("Autosave could not delete a recovery copy: {e}")),
                    Job::Flush(done) => {
                        let _ = done.send(());
                        None
                    }
                };
                if let Some(failed) = failed {
                    let _ = report.send(failed);
                }
            }
        })
        .expect("the autosave thread starts");
    (jobs, failures, thread)
}

/// Follows one document slot. `seen` is the document in it now: id, edit count and
/// whether it has unsaved changes. Returns true when a copy is due.
fn watch(
    w: &mut Watch,
    kind: DocKind,
    seen: Option<(DocumentId, u64, bool)>,
    policy: &AutosavePolicy,
    now: Instant,
    send: &impl Fn(Job),
) -> bool {
    let id = seen.map(|(id, ..)| id);
    if w.id != id {
        // Another document took the slot, or it closed: the old copy is not wanted.
        if let Some(old) = w.id.filter(|_| w.written) {
            send(Job::Remove(old, kind));
        }
        *w = Watch { id, edits: seen.map_or(0, |(_, edits, _)| edits), ..Watch::default() };
        if seen.is_some_and(|(_, _, dirty)| dirty) {
            w.trigger.edited(now);
        }
    }
    let Some((id, edits, dirty)) = seen else { return false };
    if !dirty {
        // Saved, or every edit undone back to the file.
        if w.written {
            send(Job::Remove(id, kind));
            w.written = false;
        }
        w.trigger.reset();
        w.edits = edits;
        return false;
    }
    if edits != w.edits {
        w.edits = edits;
        w.trigger.edited(now);
    }
    if w.trigger.due(policy, now) {
        w.trigger.reset();
        w.written = true;
        return true;
    }
    false
}

/// The Recover documents window: the copies found after a crash, each with Restore and
/// Discard, and the damaged copies that were skipped. Its visual design belongs to the
/// design lane; this is the least that lets the plumbing be used.
pub fn window(ctx: &egui::Context, state: &mut AppState) {
    let Some(r) = state.recovery.as_mut().filter(|r| r.show) else { return };
    let mut open = true;
    let mut later = false;
    let mut actions = Vec::new();
    egui::Window::new("Recover documents").collapsible(false).resizable(false).open(&mut open).show(ctx, |ui| {
        if r.offers.is_empty() {
            ui.label("No documents to recover.");
        } else {
            ui.label("SolidDraft did not close properly. These documents have changes that were not saved:");
        }
        for copy in &r.offers {
            let record = &copy.record;
            let place = record.path.as_ref().map_or_else(|| "never saved".to_owned(), |p| p.display().to_string());
            ui.horizontal(|ui| {
                ui.label(format!("{} · {} · {place} · {}", record.name, record.kind.label(), ago(record.written())));
                if ui.button("Restore").clicked() {
                    actions.push(AppAction::RestoreRecovery(copy.file.clone()));
                }
                if ui.button("Discard").clicked() {
                    actions.push(AppAction::DiscardRecovery(copy.file.clone()));
                }
            });
        }
        if !r.problems.is_empty() {
            ui.separator();
            for problem in &r.problems {
                ui.label(problem);
            }
        }
        ui.separator();
        later = ui.button("Later").on_hover_text("Keep the copies for the next start").clicked();
    });
    if !open || later {
        r.show = false;
        // Damaged copies are reported once; they stay in the damaged folder.
        r.problems.clear();
    }
    for action in actions {
        state.push(action);
    }
}

/// "just now", "5 min ago", "3 h ago" or "2 days ago".
fn ago(t: std::time::SystemTime) -> String {
    let s = t.elapsed().unwrap_or_default().as_secs();
    match s {
        0..60 => "just now".to_owned(),
        60..3600 => format!("{} min ago", s / 60),
        3600..86400 => format!("{} h ago", s / 3600),
        _ => format!("{} days ago", s / 86400),
    }
}
