//! Named values and dimension bindings stored with the part's truth.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use soliddraft_sketch::{ConstraintId, ConstraintKind, SolveStatus};

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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NamedValue {
    id: u32,
    name: String,
    expression: Expression,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    input: Input,
    expression: Expression,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Expression {
    text: String,
    tree: Expr,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
enum Expr {
    Number(f64, i32, i32),
    Ref(u32),
    Op(char, Box<Expr>, Box<Expr>),
    Neg(Box<Expr>),
}

#[derive(Clone, Copy)]
struct Quantity {
    value: f64,
    length: i32,
    angle: i32,
}

impl Quantity {
    fn document_units(mut self) -> Self {
        if self.length == 0 && self.angle == 0 {
            self.length = 1;
        }
        self
    }
}

impl Values {
    /// Whether there is no value data to persist.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.bindings.is_empty()
    }

    pub(crate) fn set(&mut self, name: &str, text: &str) -> Result<(), String> {
        check_name(name)?;
        if !self.entries.iter().any(|v| v.name == name) {
            if self.entries.len() >= 256 {
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
            self.entries.push(NamedValue {
                id,
                name: name.into(),
                expression: Expression { text: "0".into(), tree: Expr::Number(0.0, 0, 0) },
            });
        }
        let expression = self.parse(text)?;
        self.entries.iter_mut().find(|v| v.name == name).expect("inserted above").expression = expression;
        self.resolve()?;
        Ok(())
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
        if text.len() > 2048 {
            return Err("expression is too long".into());
        }
        let tokens = tokenize(text)?;
        if tokens.is_empty() || tokens.len() > 64 {
            return Err("expression needs between 1 and 64 tokens".into());
        }
        let mut parser = Parser { tokens: &tokens, at: 0, values: self };
        let tree = parser.sum()?;
        if parser.at != tokens.len() {
            return Err(format!("unexpected token {}", tokens[parser.at]));
        }
        // A unit literal is converted as decimal arithmetic before its first f64 parse.
        let stored = match &tree {
            Expr::Number(value, 1, 0) if tokens.len() == 2 && unit(tokens[1]).is_some() => format!("{value} mm"),
            _ => text.to_owned(),
        };
        Ok(Expression { text: stored, tree })
    }

    fn resolve(&self) -> Result<BTreeMap<u32, Quantity>, String> {
        fn visit(
            values: &Values,
            id: u32,
            visiting: &mut Vec<u32>,
            done: &mut BTreeMap<u32, Quantity>,
        ) -> Result<Quantity, String> {
            if let Some(q) = done.get(&id) {
                return Ok(*q);
            }
            if visiting.contains(&id) {
                return Err("value cycle".into());
            }
            if visiting.len() >= 32 {
                return Err("value graph is too deep".into());
            }
            let v = values.entries.iter().find(|v| v.id == id).ok_or("missing value reference")?;
            visiting.push(id);
            let q = evaluate(&v.expression.tree, &mut |other| visit(values, other, visiting, done))?;
            visiting.pop();
            done.insert(id, q);
            Ok(q)
        }
        let mut done = BTreeMap::new();
        for v in &self.entries {
            visit(self, v.id, &mut Vec::new(), &mut done)?;
        }
        Ok(done)
    }

    /// Named values in creation order, with resolved document units.
    pub fn summary(&self) -> serde_json::Value {
        let resolved = self.resolve().expect("committed values are checked");
        self.entries
            .iter()
            .map(|v| {
                let q = resolved[&v.id];
                serde_json::json!({"name":v.name,"expression":v.expression.text,"value":q.value,
                "unit": match (q.length, q.angle) { (1,0) => "mm", (0,1) => "deg", (0,0) => "", _ => "compound" }})
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
        if ids.len() > 256 {
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
            let q = evaluate(&b.expression.tree, &mut |id| resolved.get(&id).copied().ok_or("missing value".into()))?;
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
        &self.values
    }
}

fn require(q: Quantity, angle: bool) -> Result<(), String> {
    let expected = if angle { (0, 1) } else { (1, 0) };
    if (q.length, q.angle) != expected {
        return Err(format!("the input needs a {}", if angle { "angle" } else { "length" }));
    }
    Ok(())
}

fn bare_literal(tree: &Expr) -> bool {
    match tree {
        Expr::Number(_, 0, 0) => true,
        Expr::Neg(inner) => bare_literal(inner),
        _ => false,
    }
}

fn evaluate(tree: &Expr, reference: &mut impl FnMut(u32) -> Result<Quantity, String>) -> Result<Quantity, String> {
    let q = eval(tree, reference)?;
    Ok(if bare_literal(tree) { q.document_units() } else { q })
}

fn eval(tree: &Expr, reference: &mut impl FnMut(u32) -> Result<Quantity, String>) -> Result<Quantity, String> {
    let q = match tree {
        Expr::Number(value, length, angle) => Quantity { value: *value, length: *length, angle: *angle },
        Expr::Ref(id) => reference(*id)?,
        Expr::Neg(e) => {
            let mut q = eval(e, reference)?;
            q.value = -q.value;
            q
        }
        Expr::Op(op, a, b) => {
            let a_bare = bare_literal(a);
            let b_bare = bare_literal(b);
            let mut a = eval(a, reference)?;
            let mut b = eval(b, reference)?;
            match op {
                '+' | '-' => {
                    if a_bare {
                        a = a.document_units();
                    }
                    if b_bare {
                        b = b.document_units();
                    }
                    if (a.length, a.angle) != (b.length, b.angle) {
                        return Err("incompatible units in sum".into());
                    }
                    a.value = if *op == '+' { a.value + b.value } else { a.value - b.value };
                    a
                }
                '*' => Quantity {
                    value: a.value * b.value,
                    length: a.length.checked_add(b.length).ok_or("unit exponent too large")?,
                    angle: a.angle.checked_add(b.angle).ok_or("unit exponent too large")?,
                },
                '/' => Quantity {
                    value: a.value / b.value,
                    length: a.length.checked_sub(b.length).ok_or("unit exponent too large")?,
                    angle: a.angle.checked_sub(b.angle).ok_or("unit exponent too large")?,
                },
                _ => return Err("unknown operator".into()),
            }
        }
    };
    if !q.value.is_finite() {
        return Err("expression must resolve to a finite number".into());
    }
    Ok(q)
}

fn unit(text: &str) -> Option<(i128, i32, i32, i32)> {
    match text {
        "mm" => Some((1, 0, 1, 0)),
        "cm" => Some((10, 0, 1, 0)),
        "m" => Some((1000, 0, 1, 0)),
        "in" => Some((254, 1, 1, 0)),
        "deg" => Some((1, 0, 0, 1)),
        _ => None,
    }
}

fn check_name(name: &str) -> Result<(), String> {
    let mut bytes = name.bytes();
    if !bytes.next().is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        || !bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
        || name.len() > 128
        || unit(name).is_some()
    {
        return Err("a value name must be an identifier and cannot be a unit".into());
    }
    Ok(())
}

fn tokenize(text: &str) -> Result<Vec<&str>, String> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let start = at;
        if bytes[at].is_ascii_whitespace() {
            at += 1;
            continue;
        }
        if bytes[at].is_ascii_digit() || bytes[at] == b'.' {
            at += 1;
            while at < bytes.len() && (bytes[at].is_ascii_digit() || bytes[at] == b'.') {
                at += 1;
            }
            if at < bytes.len() && matches!(bytes[at], b'e' | b'E') {
                at += 1;
                if at < bytes.len() && matches!(bytes[at], b'+' | b'-') {
                    at += 1;
                }
                while at < bytes.len() && bytes[at].is_ascii_digit() {
                    at += 1;
                }
            }
        } else if bytes[at].is_ascii_alphabetic() || bytes[at] == b'_' {
            at += 1;
            while at < bytes.len() && (bytes[at].is_ascii_alphanumeric() || bytes[at] == b'_') {
                at += 1;
            }
        } else if b"+-*/()".contains(&bytes[at]) {
            at += 1;
        } else {
            return Err("unsupported character in expression".into());
        }
        out.push(&text[start..at]);
    }
    Ok(out)
}

fn rename_tokens(text: &str, from: &str, to: &str) -> String {
    let mut out = String::new();
    let mut at = 0;
    for token in tokenize(text).expect("accepted expression") {
        let start = token.as_ptr() as usize - text.as_ptr() as usize;
        out.push_str(&text[at..start]);
        out.push_str(if token == from { to } else { token });
        at = start + token.len();
    }
    out.push_str(&text[at..]);
    out
}

struct Parser<'a> {
    tokens: &'a [&'a str],
    at: usize,
    values: &'a Values,
}
impl Parser<'_> {
    fn sum(&mut self) -> Result<Expr, String> {
        let mut a = self.product()?;
        while self.tokens.get(self.at).is_some_and(|t| matches!(*t, "+" | "-")) {
            let op = self.tokens[self.at].chars().next().expect("operator");
            self.at += 1;
            a = Expr::Op(op, Box::new(a), Box::new(self.product()?));
        }
        Ok(a)
    }
    fn product(&mut self) -> Result<Expr, String> {
        let mut a = self.atom()?;
        while self.tokens.get(self.at).is_some_and(|t| matches!(*t, "*" | "/")) {
            let op = self.tokens[self.at].chars().next().expect("operator");
            self.at += 1;
            a = Expr::Op(op, Box::new(a), Box::new(self.atom()?));
        }
        Ok(a)
    }
    fn atom(&mut self) -> Result<Expr, String> {
        let token = *self.tokens.get(self.at).ok_or("expected a value")?;
        self.at += 1;
        match token {
            "-" => Ok(Expr::Neg(Box::new(self.atom()?))),
            "+" => self.atom(),
            "(" => {
                let a = self.sum()?;
                if self.tokens.get(self.at) != Some(&")") {
                    return Err("missing )".into());
                }
                self.at += 1;
                Ok(a)
            }
            _ => {
                if token.as_bytes()[0].is_ascii_digit() || token.starts_with('.') {
                    let value: f64 = token.parse().map_err(|_| "invalid numeric literal")?;
                    if !value.is_finite() {
                        return Err("a literal must be finite".into());
                    }
                    if let Some(symbol) = self.tokens.get(self.at).filter(|t| t.as_bytes()[0].is_ascii_alphabetic()) {
                        let (factor, scale, length, angle) =
                            unit(symbol).ok_or_else(|| format!("unknown unit {symbol}"))?;
                        self.at += 1;
                        return Ok(Expr::Number(convert_decimal(token, factor, scale)?, length, angle));
                    }
                    Ok(Expr::Number(value, 0, 0))
                } else {
                    self.values
                        .entries
                        .iter()
                        .find(|v| v.name == token)
                        .map(|v| Expr::Ref(v.id))
                        .ok_or_else(|| format!("unknown value {token}"))
                }
            }
        }
    }
}

fn convert_decimal(text: &str, factor: i128, scale: i32) -> Result<f64, String> {
    let fail = || "unit literal is outside the supported decimal range".to_owned();
    let (mantissa, exponent) = text.split_once(['e', 'E']).unwrap_or((text, "0"));
    let exponent: i32 = exponent.parse().map_err(|_| fail())?;
    let fraction = mantissa.split_once('.').map_or(0, |(_, tail)| tail.len() as i32);
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let n = digits.parse::<i128>().ok().and_then(|n| n.checked_mul(factor)).ok_or_else(fail)?;
    let exponent = exponent
        .checked_sub(fraction)
        .and_then(|e| e.checked_sub(scale))
        .filter(|e| (-300..=300).contains(e))
        .ok_or_else(fail)?;
    let value: f64 = format!("{n}e{exponent}").parse().map_err(|_| fail())?;
    if !value.is_finite() {
        return Err(fail());
    }
    Ok(value)
}
