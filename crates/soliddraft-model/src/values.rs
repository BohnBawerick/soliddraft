//! Named values and dimension bindings stored with the part's truth.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use soliddraft_sketch::{ConstraintId, ConstraintKind, SolveStatus};

use crate::value_expr::{self, Expression, NamedValue, Quantity, check_name, evaluate, rename_tokens};
use crate::{Document, Feature, FeatureId, FeatureKind};

/// A driven feature input, identified independently of its display name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Input {
    /// A sketch constraint with a numeric dimension.
    Dimension {
        /// Stable sketch identity.
        feature: FeatureId,
        /// Stable dimension identity.
        constraint: ConstraintId,
    },
    /// An extrusion's depth in millimetres.
    Depth {
        /// Stable extrusion identity.
        feature: FeatureId,
    },
}

impl Input {
    /// The feature that owns this input.
    pub fn feature(&self) -> FeatureId {
        match *self {
            Self::Dimension { feature, .. } | Self::Depth { feature } => feature,
        }
    }
}

/// Authored values and bindings. Empty state is omitted from old-format truth.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Values {
    entries: Vec<NamedValue>,
    bindings: Vec<Binding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) tracked: Vec<crate::tracked::Row>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    input: Input,
    expression: Expression,
}

impl Values {
    /// Whether there is no value data to persist.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.bindings.is_empty() && self.tracked.is_empty()
    }

    pub(crate) fn set(&mut self, name: &str, text: &str) -> Result<(), String> {
        check_name(name)?;
        if self.tracked.iter().any(|r| r.name == name) {
            return Err(format!("{name} is tracked and cannot be set"));
        }
        if !self.entries.iter().any(|v| v.name == name) {
            if self.entries.len() + self.tracked.len() >= 256 {
                return Err("at most 256 named values are supported".into());
            }
            let id = self
                .entries
                .iter()
                .map(|v| v.id)
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or("value identifiers exhausted")?;
            self.entries.push(NamedValue { id, name: name.into(), expression: Expression::zero() });
        }
        let expression = self.parse(text)?;
        self.entries.iter_mut().find(|v| v.name == name).expect("inserted above").expression = expression;
        self.resolve()?;
        Ok(())
    }

    pub(crate) fn expression(&self, name: &str) -> Option<&str> {
        self.entries.iter().find(|v| v.name == name).map(|v| v.expression.text.as_str())
    }

    pub(crate) fn overlay(&self, overrides: &BTreeMap<String, String>) -> Result<Self, String> {
        let mut next = self.clone();
        for (name, text) in overrides {
            let expression = self.parse(text)?;
            let entry =
                next.entries.iter_mut().find(|v| v.name == *name).ok_or_else(|| format!("unknown value {name}"))?;
            entry.expression = expression;
        }
        next.resolve()?;
        Ok(next)
    }

    pub(crate) fn drive(&mut self, input: Input, text: &str) -> Result<(), String> {
        let expression = self.parse(text)?;
        if let Some(binding) = self.bindings.iter_mut().find(|b| b.input == input) {
            binding.expression = expression;
        } else {
            self.bindings.push(Binding { input, expression });
        }
        Ok(())
    }

    pub(crate) fn rename(&mut self, name: &str, to: &str) -> Result<(), String> {
        check_name(to)?;
        if self.tracked.iter().any(|r| r.name == to && r.name != name) {
            return Err(format!("the name {to} is already taken"));
        }
        if let Some(row) = self.tracked.iter_mut().find(|r| r.name == name) {
            if self.entries.iter().any(|v| v.name == to) {
                return Err(format!("the name {to} is already taken"));
            }
            row.name = to.into();
            return Ok(());
        }
        let index = self.entries.iter().position(|v| v.name == name).ok_or_else(|| format!("no value named {name}"))?;
        if self.entries.iter().enumerate().any(|(i, v)| i != index && v.name == to) {
            return Err(format!("the name {to} is already taken"));
        }
        self.entries[index].name = to.into();
        for expression in self
            .entries
            .iter_mut()
            .map(|v| &mut v.expression)
            .chain(self.bindings.iter_mut().map(|b| &mut b.expression))
        {
            expression.text = rename_tokens(&expression.text, name, to);
        }
        Ok(())
    }

    fn parse(&self, text: &str) -> Result<Expression, String> {
        value_expr::parse(text, &|token| {
            if self.tracked.iter().any(|r| r.name == token) {
                return Err(format!("{token} is tracked and cannot drive an expression"));
            }
            self.entries.iter().find(|v| v.name == token).map(|v| v.id).ok_or_else(|| format!("unknown value {token}"))
        })
        .map(Expression::with_converted_literal)
    }

    fn resolve(&self) -> Result<BTreeMap<u32, Quantity>, String> {
        value_expr::resolve(&self.entries)
    }

    /// Named values in creation order, with resolved document units.
    pub fn summary(&self) -> serde_json::Value {
        let resolved = self.resolve().expect("committed values are checked");
        self.entries
            .iter()
            .map(|v| {
                let q = resolved[&v.id];
                serde_json::json!({"name":v.name,"expression":v.expression.text,"value":q.value,
                "unit": value_expr::unit_name(q)})
            })
            .collect()
    }

    /// Expression text by the feature's input labels.
    pub fn expressions(&self, feature: &Feature) -> BTreeMap<String, String> {
        self.bindings
            .iter()
            .filter(|b| b.input.feature() == feature.id)
            .filter_map(|b| {
                let key = match b.input {
                    Input::Depth { .. } => "depth".into(),
                    Input::Dimension { constraint, .. } => {
                        let FeatureKind::Sketch(sketch) = &feature.kind else {
                            return None;
                        };
                        (sketch
                            .sketch
                            .constraints
                            .iter()
                            .filter(|c| c.value().is_some())
                            .position(|c| c.id == constraint)?
                            + 1)
                        .to_string()
                    }
                };
                Some((key, b.expression.text.clone()))
            })
            .collect()
    }

    /// Checks persisted expressions and bindings, including their stable references.
    pub(crate) fn validate(&self, features: &[Feature]) -> Result<Vec<Feature>, String> {
        let mut ids = Vec::new();
        let mut names = Vec::new();
        for v in &self.entries {
            check_name(&v.name)?;
            if ids.contains(&v.id) || names.contains(&v.name.as_str()) {
                return Err("duplicate value id or name".into());
            }
            ids.push(v.id);
            names.push(v.name.as_str());
        }
        for row in &self.tracked {
            check_name(&row.name)?;
            if names.contains(&row.name.as_str()) {
                return Err(format!("duplicate value name {}", row.name));
            }
            names.push(&row.name);
            row.source.validate()?;
            if let Some(check) = &row.check
                && crate::tracked::Check::parse(&check.text, row.source.unit())? != *check
            {
                return Err("check text and resolved limit disagree".into());
            }
        }
        if names.len() > 256 {
            return Err("too many named values".into());
        }
        let mut inputs = Vec::new();
        for b in &self.bindings {
            if inputs.contains(&b.input) {
                return Err("an input has two drivers".into());
            }
            inputs.push(b.input.clone());
        }
        for e in self.entries.iter().map(|v| &v.expression).chain(self.bindings.iter().map(|b| &b.expression)) {
            if self.parse(&e.text)?.tree != e.tree {
                return Err("expression text and references disagree".into());
            }
        }
        let resolved = self.resolve()?;
        let mut next = features.to_vec();
        for b in &self.bindings {
            let q = evaluate(&b.expression, &resolved)?;
            let f = next.iter_mut().find(|f| f.id == b.input.feature()).ok_or("driven feature is missing")?;
            match b.input {
                Input::Depth { .. } => {
                    let FeatureKind::Extrude(extrude) = &mut f.kind else {
                        return Err("depth requires an extrude".into());
                    };
                    require(q, false)?;
                    if q.value <= 0.0 {
                        return Err("depth must be positive".into());
                    }
                    extrude.end = crate::EndCondition::Blind { depth: q.value };
                }
                Input::Dimension { constraint, .. } => {
                    let FeatureKind::Sketch(sketch) = &mut f.kind else {
                        return Err("dimension requires a sketch".into());
                    };
                    let c = sketch
                        .sketch
                        .constraint_mut(constraint)
                        .filter(|c| c.value().is_some())
                        .ok_or("no such dimension")?;
                    require(q, matches!(c.kind, ConstraintKind::Angle { .. }))?;
                    if c.value() != Some(q.value) {
                        c.set_value(q.value);
                        soliddraft_sketch::solve::solve(&mut sketch.sketch, None, true);
                        if let SolveStatus::Failed(e) = &sketch.sketch.status {
                            return Err(e.clone());
                        }
                    }
                }
            }
        }
        Ok(next)
    }
}

impl Document {
    /// The document's named values and bindings.
    pub fn values(&self) -> &Values {
        self.configurations.lookup(&self.values)
    }
}

/// Parses a single check limit using the value parser's units and decimal conversion.
pub(crate) fn parse_limit(text: &str, unit: &str) -> Result<f64, String> {
    let expression = Values::default().parse(text)?;
    if !expression.is_literal() {
        return Err("a check needs one literal limit".into());
    }
    let q = evaluate(&expression, &BTreeMap::new())?;
    let (dimension, name) = match unit {
        "mm" => ((1, 0), "a length"),
        "deg" => ((0, 1), "an angle"),
        _ => ((2, 0), "a area"),
    };
    if !expression.is_bare_literal() && (q.length, q.angle) != dimension {
        return Err(format!("the check needs {name} limit"));
    }
    Ok(q.value)
}

fn require(q: Quantity, angle: bool) -> Result<(), String> {
    let expected = if angle { (0, 1) } else { (1, 0) };
    if (q.length, q.angle) != expected {
        return Err(format!("the input needs a {}", if angle { "angle" } else { "length" }));
    }
    Ok(())
}
