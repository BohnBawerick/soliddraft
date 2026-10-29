//! Application workspace, action dispatch and presentation.

mod dispatch;
mod presentation;
mod workspace;

pub use presentation::docks::Tab;
pub use workspace::{Automation, SolidDraftApp, open_at_start};
