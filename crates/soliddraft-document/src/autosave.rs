//! When autosave writes a recovery copy: after an interval with unsaved changes, or
//! sooner after enough edits.
//!
//! [`Trigger`] keeps the count; the app tells it about each edit, asks whether a copy
//! is due, and says when one was written or the document was saved. It reads no clock
//! of its own, so it runs the same in tests.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// The autosave settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutosavePolicy {
    /// Autosave writes recovery copies at all.
    pub enabled: bool,
    /// Seconds from the first unsaved edit to the copy.
    pub interval_seconds: u64,
    /// Edits that write a copy before the interval is up. 0 waits for the interval.
    pub edits: u32,
}

impl Default for AutosavePolicy {
    fn default() -> AutosavePolicy {
        AutosavePolicy { enabled: true, interval_seconds: 120, edits: 10 }
    }
}

impl AutosavePolicy {
    /// The interval as a duration. At least a second, so a zero never writes every frame.
    pub fn interval(&self) -> Duration {
        Duration::from_secs(self.interval_seconds.max(1))
    }
}

/// The edits since the last copy of one document, and when the first of them was made.
#[derive(Clone, Debug, Default)]
pub struct Trigger {
    edits: u32,
    since: Option<Instant>,
}

impl Trigger {
    /// Counts an edit made at `now`.
    pub fn edited(&mut self, now: Instant) {
        self.edits = self.edits.saturating_add(1);
        self.since.get_or_insert(now);
    }

    /// The edits not yet in a copy.
    pub fn pending(&self) -> u32 {
        self.edits
    }

    /// True when a copy should be written at `now`.
    pub fn due(&self, policy: &AutosavePolicy, now: Instant) -> bool {
        self.wait(policy, now) == Some(Duration::ZERO)
    }

    /// How long until a copy is due, or `None` when nothing is waiting. An app that only
    /// draws on input wakes itself after this long, so an idle document still gets its
    /// copy.
    pub fn wait(&self, policy: &AutosavePolicy, now: Instant) -> Option<Duration> {
        let since = self.since.filter(|_| policy.enabled && self.edits > 0)?;
        if policy.edits > 0 && self.edits >= policy.edits {
            return Some(Duration::ZERO);
        }
        Some(policy.interval().saturating_sub(now.saturating_duration_since(since)))
    }

    /// A copy with every edit so far was written, or the document was saved or closed.
    pub fn reset(&mut self) {
        *self = Trigger::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const POLICY: AutosavePolicy = AutosavePolicy { enabled: true, interval_seconds: 60, edits: 3 };

    #[test]
    fn the_interval_runs_from_the_first_unsaved_edit() {
        let t0 = Instant::now();
        let mut trigger = Trigger::default();
        assert!(!trigger.due(&POLICY, t0 + Duration::from_secs(3600)), "nothing to save");
        assert_eq!(trigger.wait(&POLICY, t0), None);
        trigger.edited(t0);
        assert!(!trigger.due(&POLICY, t0 + Duration::from_secs(59)));
        assert_eq!(trigger.wait(&POLICY, t0 + Duration::from_secs(45)), Some(Duration::from_secs(15)));
        // A later edit does not push the copy back.
        trigger.edited(t0 + Duration::from_secs(50));
        assert!(trigger.due(&POLICY, t0 + Duration::from_secs(60)));
        trigger.reset();
        assert!(!trigger.due(&POLICY, t0 + Duration::from_secs(120)), "written: wait for the next edit");
        trigger.edited(t0 + Duration::from_secs(130));
        assert!(!trigger.due(&POLICY, t0 + Duration::from_secs(189)));
        assert!(trigger.due(&POLICY, t0 + Duration::from_secs(190)));
    }

    #[test]
    fn enough_edits_write_before_the_interval() {
        let t0 = Instant::now();
        let mut trigger = Trigger::default();
        trigger.edited(t0);
        trigger.edited(t0);
        assert!(!trigger.due(&POLICY, t0));
        trigger.edited(t0);
        assert_eq!(trigger.pending(), 3);
        assert!(trigger.due(&POLICY, t0), "the third edit");
        let interval_only = AutosavePolicy { edits: 0, ..POLICY };
        assert!(!trigger.due(&interval_only, t0), "0 edits waits for the interval");
        assert!(trigger.due(&interval_only, t0 + Duration::from_secs(60)));
    }

    #[test]
    fn disabled_autosave_is_never_due_and_a_zero_interval_is_a_second() {
        let t0 = Instant::now();
        let mut trigger = Trigger::default();
        (0..10).for_each(|_| trigger.edited(t0));
        let off = AutosavePolicy { enabled: false, ..POLICY };
        assert!(!trigger.due(&off, t0 + Duration::from_secs(3600)));
        assert_eq!(trigger.wait(&off, t0), None);
        let zero = AutosavePolicy { interval_seconds: 0, edits: 0, ..POLICY };
        assert!(!trigger.due(&zero, t0));
        assert!(trigger.due(&zero, t0 + Duration::from_secs(1)));
    }
}
