//! Assignments that hold in some configurations.
//!
//! A part's material and its appearances can differ between configurations. Every assignment therefore carries a [`ConfigScope`]: all
//! configurations, the default, or only the named ones. Until configurations exist
//! every assignment is written with the "all" scope, and a part is always shown in its
//! default configuration.

use soliddraft_document::{ConfigScope, ConfigurationId};

/// True when an assignment with `scope` holds in `configuration` (`None` for the
/// document's default configuration, which only "all" assignments reach).
pub fn applies(scope: &ConfigScope, configuration: Option<&ConfigurationId>) -> bool {
    match scope {
        ConfigScope::All => true,
        ConfigScope::Only(names) => configuration.is_some_and(|c| names.contains(c)),
    }
}

/// How specific a scope is: an assignment for named configurations wins over one for
/// all of them, whatever their order.
pub fn rank(scope: &ConfigScope) -> u8 {
    match scope {
        ConfigScope::All => 0,
        ConfigScope::Only(_) => 1,
    }
}

/// The assignment that holds in `configuration`, among several for the same target:
/// the most specific scope wins, and among equals the later one, the most recent.
pub fn pick<'a, T>(
    items: impl IntoIterator<Item = &'a T>,
    scope_of: impl Fn(&T) -> &ConfigScope,
    configuration: Option<&ConfigurationId>,
) -> Option<&'a T> {
    let mut best: Option<&T> = None;
    for item in items {
        let scope = scope_of(item);
        if applies(scope, configuration) && best.is_none_or(|b| rank(scope) >= rank(scope_of(b))) {
            best = Some(item);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_configurations_win_over_all() {
        let long = ConfigurationId("long".into());
        let short = ConfigurationId("short".into());
        let items = [(ConfigScope::Only(vec![long.clone()]), "red"), (ConfigScope::All, "grey")];
        let pick = |c| pick(&items, |i| &i.0, c).map(|i| i.1);
        assert_eq!(pick(Some(&long)), Some("red"), "the named scope wins though it comes first");
        assert_eq!(pick(Some(&short)), Some("grey"));
        assert_eq!(pick(None), Some("grey"), "the default configuration sees only \"all\"");
        let later = [(ConfigScope::All, "grey"), (ConfigScope::All, "blue")];
        assert_eq!(super::pick(&later, |i| &i.0, None).map(|i| i.1), Some("blue"), "the later of equals");
        assert!(super::pick(&items[..1], |i| &i.0, Some(&short)).is_none());
    }
}
