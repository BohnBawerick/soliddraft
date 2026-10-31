//! Forming a sub-assembly: chosen parts of the root move into a new assembly file of
//! their own, which the root then places where they were.
//!
//! Nothing moves in the world. The new file's frame is the accepted placement of the
//! occurrence named as its origin, and each part keeps its root placement through that
//! frame. Every reference keeps its identity: the parts keep their component ids inside
//! the new file and their sources keep their documents and pins; mates between two chosen
//! parts move with them under their own ids, and mates that cross the new file's boundary
//! stay in the root with their endpoints' paths rewritten. A part no mate holds stays as
//! free as it was. The result's [`Migration`] says where every reference went.
//!
//! Forming is prepared, then committed ([`form`]): both candidate truths, their
//! structures and their solves are made and checked before anything is written; the new
//! file is published without replacing any file; only then does the root change, in one
//! undo step. A refusal at any point leaves the accepted root and every file as they were.
//! See `docs/subassemblies.md`.

mod form;

use std::path::PathBuf;

use serde::Serialize;
use soliddraft_document::{ContentHash, DocumentId};

use crate::mates::{MateId, SolverStatus};
use crate::path::{ComponentId, OccurrencePath, SourceId};

pub use form::{FormPlan, Formed};

/// What to form: the root's part occurrences, in order, the file to make, and the
/// occurrence whose placement becomes the new file's frame.
#[derive(Clone, Debug, PartialEq)]
pub struct FormRequest {
    /// The root's own part components, each once.
    pub paths: Vec<OccurrencePath>,
    /// The new assembly file. It must not exist.
    pub path: PathBuf,
    /// One of `paths`: its accepted placement becomes the new file's frame.
    pub origin: OccurrencePath,
}

/// Why forming was refused. The accepted root and every file are as they were.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FormCode {
    /// The request names nothing usable: an empty, repeated or unknown selection.
    Invalid,
    /// The selection holds something this version cannot carry into a new file whole.
    Unsupported,
    /// The new structure would contain itself.
    Cycle,
    /// A document the selection places cannot be found.
    MissingReference,
    /// The root or a selected source changed between preparing and committing.
    StaleRevision,
    /// The new file's name is taken.
    DestinationExists,
    /// The new file could not be written.
    Io,
}

impl FormCode {
    /// The code as text.
    pub fn as_str(self) -> &'static str {
        match self {
            FormCode::Invalid => "invalid",
            FormCode::Unsupported => "unsupported",
            FormCode::Cycle => "cycle",
            FormCode::MissingReference => "missing_reference",
            FormCode::StaleRevision => "stale_revision",
            FormCode::DestinationExists => "destination_exists",
            FormCode::Io => "io",
        }
    }
}

/// A refused formation: its code, what to tell the user, and what caused it.
#[derive(Clone, Debug, PartialEq, Serialize, thiserror::Error)]
#[error("{}: {message}", code.as_str())]
pub struct FormRefusal {
    /// The kind of refusal.
    #[serde(rename = "refused")]
    pub code: FormCode,
    /// The problem, for people.
    pub message: String,
    /// What caused it: the path, file or document it names.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
    /// For a stale plan: what was prepared on, and what is there now.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    /// For a stale plan: what is there now.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual: Option<String>,
}

impl FormRefusal {
    pub(crate) fn new(code: FormCode, message: impl Into<String>) -> FormRefusal {
        FormRefusal { code, message: message.into(), cause: None, expected: None, actual: None }
    }

    pub(crate) fn caused_by(mut self, cause: impl Into<String>) -> FormRefusal {
        self.cause = Some(cause.into());
        self
    }
}

/// One occurrence the formation moved.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MovedOccurrence {
    /// Its path in the root before.
    pub from: OccurrencePath,
    /// Its path in the root after.
    pub to: OccurrencePath,
    /// Its path in the new file.
    pub child: OccurrencePath,
}

/// Where a mate went.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MovedMate {
    /// Its id, the same before and after.
    pub id: MateId,
    /// Its name.
    pub name: String,
    /// The document that held it before: the root.
    pub owner_before: DocumentId,
    /// The document that holds it now: the root, or the new file.
    pub owner_after: DocumentId,
    /// Its endpoints' paths from their owner, before and after; empty for an Origin mate.
    pub endpoints_before: Vec<OccurrencePath>,
    /// The same after.
    pub endpoints_after: Vec<OccurrencePath>,
}

/// Every reference the formation moved, for whatever else names them.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Migration {
    /// The root's document, which keeps its identity.
    pub root: DocumentId,
    /// The new file's document.
    pub child: DocumentId,
    /// The new file's revision.
    pub child_revision: ContentHash,
    /// The root's new component that places it.
    pub component: ComponentId,
    /// The root's new source that references it.
    pub source: SourceId,
    /// The sources that moved into the new file, under the same ids.
    pub sources: Vec<SourceId>,
    /// Each selected occurrence, in the order given.
    pub occurrences: Vec<MovedOccurrence>,
    /// Every mate of the root, by id.
    pub mates: Vec<MovedMate>,
}

impl Migration {
    /// Where an occurrence path of the root before forming is after it: the moved
    /// occurrences and anything under them go under the new component; others stay.
    pub fn map(&self, path: &OccurrencePath) -> OccurrencePath {
        for m in &self.occurrences {
            if path.steps().starts_with(m.from.steps()) {
                let rest = &path.steps()[m.from.steps().len()..];
                return OccurrencePath([m.to.steps(), rest].concat());
            }
        }
        path.clone()
    }
}

/// How each side solves once formed.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FormStatus {
    /// The root with the new file placed in it.
    pub root: SolverStatus,
    /// The new file on its own.
    pub child: SolverStatus,
}
