//! Bounded repair of an imported STEP body: what the app asks of the kernel and what it
//! gets back, as the JSON shapes of `catallax-import-proposals/1` and
//! `catallax-import-repair/1`.
//!
//! The source bytes are immutable and addressed by their sha256. The repaired body is
//! derived from the source and the user's decisions; it is never written over the source.
//! Numbers are decimal strings in millimetres.

use serde::{Deserialize, Serialize};

use crate::OpResult;

/// The format tag of a change record.
pub const REPAIR_FORMAT: &str = "catallax-import-repair/1";

/// The format tag of a proposal set.
pub const PROPOSALS_FORMAT: &str = "catallax-import-proposals/1";

/// The id of the question about gap `index` of the source with hash `sha256`. It is the
/// key of the saved decision.
pub fn question_id(sha256: &str, index: usize) -> String {
    let prefix: String = sha256.chars().take(12).collect();
    format!("repair:{prefix}:{index}")
}

/// The gap index a question id names, when it is a question about the source with hash
/// `sha256`.
pub fn question_index(sha256: &str, question: &str) -> Option<usize> {
    let rest = question.strip_prefix("repair:")?;
    let (prefix, index) = rest.split_once(':')?;
    if prefix.is_empty() || !sha256.starts_with(prefix) {
        return None;
    }
    index.parse().ok()
}

/// What the app does with a proposal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    /// Within policy: applied and recorded without asking.
    Auto,
    /// Shown to the user, who accepts or declines.
    Ask,
}

/// A repair the kernel measured and classified.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposal {
    /// Index within the set: `g0`, `g1`, ...
    pub id: String,
    /// The question id, from [`question_id`].
    pub question: String,
    /// The source entities involved, such as `#12`.
    pub entities: Vec<String>,
    /// `seam_close`, `body_gap` or `opening`.
    pub kind: String,
    /// The certified lower bound of the gap, in millimetres.
    pub distance_min_mm: String,
    /// The certified upper bound of the gap, in millimetres.
    pub distance_max_mm: String,
    /// The movement the repair would be allowed, in millimetres.
    pub tolerance_mm: String,
    /// Whether it is applied or asked.
    pub disposition: Disposition,
    /// Why.
    pub reason: String,
}

/// Every measured proposal of one source.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalSet {
    /// The sha256 of the source bytes, in lowercase hex.
    pub source_sha256: String,
    /// The proposals, in index order.
    pub proposals: Vec<Proposal>,
}

/// The user's answer to one `ask` proposal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum Decision {
    /// Apply the repair with at most this movement.
    Accept {
        /// The proposal id.
        id: String,
        /// The tolerance, at least the proposal's `distance_max_mm`.
        tolerance_mm: String,
        /// Who approved it.
        approver: String,
    },
    /// Leave the original geometry.
    Decline {
        /// The proposal id.
        id: String,
    },
}

/// How a change was approved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Approval {
    /// Within the automatic cap.
    Automatic,
    /// The user accepted a proposal.
    User,
    /// The user declined a proposal.
    Declined,
}

/// A box in millimetres, as decimal strings: minimum corner then maximum corner.
pub type DecimalBounds = [[String; 3]; 2];

/// One entry of the change record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepairChange {
    /// Index within the record, such as `g0`.
    pub id: String,
    /// The source entities affected.
    pub entities: Vec<String>,
    /// What was done.
    pub kind: String,
    /// The bounds of the affected entities before the change.
    pub bounds_before: DecimalBounds,
    /// The bounds after the change.
    pub bounds_after: DecimalBounds,
    /// The movement measured on the repaired result, in millimetres.
    pub max_movement_mm: String,
    /// The tolerance the change was held to, in millimetres.
    pub tolerance_mm: String,
    /// Why the change was made or asked about.
    pub reason: String,
    /// How it was approved.
    pub approval: Approval,
    /// Who approved it, for a user approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approver: Option<String>,
}

/// The source a change record was made for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepairSource {
    /// The file name as imported.
    pub name: String,
    /// The sha256 of the file bytes, in lowercase hex.
    pub sha256: String,
}

/// The hash of a repaired derivative.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepairDerivative {
    /// The sha256 of the derivative, in lowercase hex.
    pub sha256: String,
}

/// The change record of one import: `catallax-import-repair/1`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepairRecord {
    /// Always [`REPAIR_FORMAT`].
    pub format: String,
    /// The source file.
    pub source: RepairSource,
    /// The repaired derivative, absent when every change was declined.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derivative: Option<RepairDerivative>,
    /// Every change, in order.
    pub changes: Vec<RepairChange>,
}

/// How far from a feature the importer looks for another that might close a gap.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportOptions {
    /// The search radius in millimetres, as a decimal string. It must be the same for
    /// the proposal and the import it is answered in.
    pub search_radius_mm: String,
}

impl Default for ImportOptions {
    fn default() -> ImportOptions {
        ImportOptions { search_radius_mm: "1".into() }
    }
}

/// What an import is asked to read.
#[derive(Clone, Copy, Debug)]
pub struct ImportStepRequest<'a> {
    /// The file name, for messages and the change record.
    pub name: &'a str,
    /// The file bytes, unchanged.
    pub source: &'a [u8],
    /// The sha256 of `source`, in lowercase hex.
    pub sha256: &'a str,
    /// The options the proposals were made with.
    pub options: &'a ImportOptions,
    /// The answers saved so far.
    pub decisions: &'a [Decision],
}

/// One solid of the file, built.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportedBody {
    /// The solid's name in the file.
    pub name: String,
    /// The built body.
    pub result: OpResult,
}

/// What an import gives back.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportStepResult {
    /// The solids that were built. Separate solids stay separate bodies.
    pub bodies: Vec<ImportedBody>,
    /// Solids not built because a gap in them was declined or is still unanswered.
    /// They are not imported until the gap is resolved.
    pub left_open: Vec<String>,
    /// The change record: automatic repairs, accepted repairs and declined proposals.
    pub record: RepairRecord,
}
