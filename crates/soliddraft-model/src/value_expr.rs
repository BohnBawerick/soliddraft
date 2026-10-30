//! Typed expressions over named values: the one grammar, unit model and evaluator the
//! part's values and an assembly's share.
//!
//! An expression keeps the text as written beside a tree that refers to other values by
//! stable id, so renaming a value changes its label and never what depends on it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const MAX_DEPENDENCY_DEPTH: usize = 32;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NamedValue {
    pub(crate) id: u32,
    pub(crate) name: String,
    pub(crate) expression: Expression,
}

/// An expression: the text as stored, and what it means.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expression {
    pub(crate) text: String,
    pub(crate) tree: Expr,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum Expr {
    Number(f64, i32, i32),
    Ref(u32),
    Op(char, Box<Expr>, Box<Expr>),
    Neg(Box<Expr>),
}

#[derive(Clone, Copy)]
pub(crate) struct Quantity {
    pub(crate) value: f64,
    pub(crate) length: i32,
    pub(crate) angle: i32,
}

impl Quantity {
    fn document_units(mut self) -> Self {
        if self.length == 0 && self.angle == 0 {
            self.length = 1;
        }
        self
    }
}

/// Every value's quantity, or why they do not resolve.
pub(crate) fn resolve(entries: &[NamedValue]) -> Result<BTreeMap<u32, Quantity>, String> {
    fn visit(
        entries: &[NamedValue],
        id: u32,
        visiting: &mut Vec<u32>,
        done: &mut BTreeMap<u32, (Quantity, usize)>,
    ) -> Result<(Quantity, usize), String> {
        if let Some(cached) = done.get(&id) {
            return Ok(*cached);
        }
        if visiting.contains(&id) {
            return Err("value cycle".into());
        }
        if visiting.len() >= MAX_DEPENDENCY_DEPTH {
            return Err("value graph is too deep".into());
        }
        let v = entries.iter().find(|v| v.id == id).ok_or("missing value reference")?;
        visiting.push(id);
        let mut depth = 1;
        let q = evaluate_tree(&v.expression.tree, &mut |other| {
            let (quantity, dependency_depth) = visit(entries, other, visiting, done)?;
            depth = depth.max(dependency_depth + 1);
            Ok(quantity)
        })?;
        visiting.pop();
        if depth > MAX_DEPENDENCY_DEPTH {
            return Err("value graph is too deep".into());
        }
        done.insert(id, (q, depth));
        Ok((q, depth))
    }
    let mut done = BTreeMap::new();
    for v in entries {
        visit(entries, v.id, &mut Vec::new(), &mut done)?;
    }
    Ok(done.into_iter().map(|(id, (quantity, _))| (id, quantity)).collect())
}

fn bare_literal(tree: &Expr) -> bool {
    match tree {
        Expr::Number(_, 0, 0) => true,
        Expr::Neg(inner) => bare_literal(inner),
        _ => false,
    }
}

/// What an expression comes to, given the values it may read.
pub(crate) fn evaluate(expression: &Expression, resolved: &BTreeMap<u32, Quantity>) -> Result<Quantity, String> {
    evaluate_tree(&expression.tree, &mut |id| resolved.get(&id).copied().ok_or("missing value".into()))
}

fn evaluate_tree(tree: &Expr, reference: &mut impl FnMut(u32) -> Result<Quantity, String>) -> Result<Quantity, String> {
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

pub(crate) fn check_name(name: &str) -> Result<(), String> {
    let mut bytes = name.bytes();
    if !bytes.next().is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        || !bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
        || name.len() > 128
        || unit(name).is_some()
    {
        return Err(format!("{name}: a value name must be an identifier and cannot be a unit"));
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

pub(crate) fn rename_tokens(text: &str, from: &str, to: &str) -> String {
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
    lookup: &'a dyn Fn(&str) -> Result<u32, String>,
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
                    (self.lookup)(token).map(Expr::Ref)
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

/// Parses `text`, reading names through `lookup`. The expression keeps the text as written.
pub(crate) fn parse(text: &str, lookup: &dyn Fn(&str) -> Result<u32, String>) -> Result<Expression, String> {
    if text.len() > 2048 {
        return Err("expression is too long".into());
    }
    let tokens = tokenize(text)?;
    if tokens.is_empty() || tokens.len() > 64 {
        return Err("expression needs between 1 and 64 tokens".into());
    }
    let mut parser = Parser { tokens: &tokens, at: 0, lookup };
    let tree = parser.sum()?;
    if parser.at != tokens.len() {
        return Err(format!("unexpected token {}", tokens[parser.at]));
    }
    Ok(Expression { text: text.to_owned(), tree })
}

impl Expression {
    /// An expression that says 0.
    pub(crate) fn zero() -> Expression {
        Expression { text: "0".into(), tree: Expr::Number(0.0, 0, 0) }
    }

    /// The text as stored.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The expression with a unit literal's text in document units. A unit literal is
    /// converted as decimal arithmetic before its first f64 parse.
    pub(crate) fn with_converted_literal(mut self) -> Expression {
        if let Expr::Number(value, 1, 0) = self.tree
            && let Ok(tokens) = tokenize(&self.text)
            && tokens.len() == 2
            && unit(tokens[1]).is_some()
        {
            self.text = format!("{value} mm");
        }
        self
    }

    /// True when it reads no value.
    pub(crate) fn is_literal(&self) -> bool {
        fn literal(tree: &Expr) -> bool {
            match tree {
                Expr::Number(..) => true,
                Expr::Neg(inner) => literal(inner),
                _ => false,
            }
        }
        literal(&self.tree)
    }

    /// True when it is a number with no unit.
    pub(crate) fn is_bare_literal(&self) -> bool {
        bare_literal(&self.tree)
    }

    /// The ids of the values it reads, once each, in the order written.
    pub fn references(&self) -> Vec<u32> {
        fn walk(tree: &Expr, out: &mut Vec<u32>) {
            match tree {
                Expr::Number(..) => {}
                Expr::Ref(id) if out.contains(id) => {}
                Expr::Ref(id) => out.push(*id),
                Expr::Neg(inner) => walk(inner, out),
                Expr::Op(_, a, b) => {
                    walk(a, out);
                    walk(b, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.tree, &mut out);
        out
    }
}

/// A table of named values for a document that binds them to inputs of its own. Ids come
/// from the owner, which keeps them stable and never reuses one. Text is kept as written.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ValueTable {
    entries: Vec<NamedValue>,
}

impl ValueTable {
    /// True with no values.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The id of the value called `name`.
    pub fn id_of(&self, name: &str) -> Option<u32> {
        self.entries.iter().find(|v| v.name == name).map(|v| v.id)
    }

    /// The name of the value with id `id`.
    pub fn name_of(&self, id: u32) -> Option<&str> {
        self.entries.iter().find(|v| v.id == id).map(|v| v.name.as_str())
    }

    /// Parses `text` against this table.
    pub fn parse(&self, text: &str) -> Result<Expression, String> {
        parse(text, &|token| self.id_of(token).ok_or_else(|| format!("unknown value {token}")))
    }

    /// Gives the value called `name` a new expression, adding it under `new_id()` if it is
    /// new, and checks that every value still resolves.
    pub fn set(&mut self, name: &str, text: &str, new_id: impl FnOnce() -> u32) -> Result<(), String> {
        check_name(name)?;
        if self.id_of(name).is_none() {
            if self.entries.len() >= 256 {
                return Err("at most 256 named values are supported".into());
            }
            self.entries.push(NamedValue { id: new_id(), name: name.into(), expression: Expression::zero() });
        }
        let expression = self.parse(text)?;
        self.entries.iter_mut().find(|v| v.name == name).expect("found or added above").expression = expression;
        resolve(&self.entries)?;
        Ok(())
    }

    /// Renames a value in the table and in `others`, the expressions that read it from
    /// outside. References are by id, so only their text changes.
    pub fn rename<'a>(
        &mut self,
        name: &str,
        to: &str,
        others: impl Iterator<Item = &'a mut Expression>,
    ) -> Result<(), String> {
        check_name(to)?;
        if !self.entries.iter().any(|v| v.name == name) {
            return Err(format!("no value named {name}"));
        }
        if name != to && self.entries.iter().any(|v| v.name == to) {
            return Err(format!("the name {to} is already taken"));
        }
        for expression in self.entries.iter_mut().map(|v| &mut v.expression) {
            expression.text = rename_tokens(&expression.text, name, to);
        }
        for expression in others {
            expression.text = rename_tokens(&expression.text, name, to);
        }
        self.entries.iter_mut().find(|v| v.name == name).expect("checked above").name = to.into();
        Ok(())
    }

    /// What `expression` comes to as a length, in millimetres. A bare number is in
    /// document units, as everywhere.
    pub fn length(&self, expression: &Expression) -> Result<f64, String> {
        let q = evaluate(expression, &resolve(&self.entries)?)?;
        if (q.length, q.angle) != (1, 0) {
            return Err("the input needs a length".into());
        }
        Ok(q.value)
    }

    /// Checks ids and names are unique, that `expressions` and every value parse back to
    /// what is stored, and that every value resolves.
    pub fn validate<'a>(&self, expressions: impl Iterator<Item = &'a Expression>) -> Result<(), String> {
        for (i, v) in self.entries.iter().enumerate() {
            check_name(&v.name)?;
            if self.entries[..i].iter().any(|o| o.id == v.id || o.name == v.name) {
                return Err("duplicate value id or name".into());
            }
        }
        let agrees = |e: &Expression| Ok::<_, String>(self.parse(&e.text)?.tree == e.tree);
        for v in &self.entries {
            if !agrees(&v.expression)? {
                return Err("expression text and references disagree".into());
            }
        }
        for e in expressions {
            if !agrees(e)? {
                return Err("expression text and references disagree".into());
            }
        }
        resolve(&self.entries).map(|_| ())
    }

    /// The ids in use.
    pub fn ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.entries.iter().map(|v| v.id)
    }

    /// Every value in creation order, with its resolved document units.
    pub fn summary(&self) -> Value {
        let resolved = resolve(&self.entries).expect("committed values are checked");
        self.entries
            .iter()
            .map(|v| {
                let q = resolved[&v.id];
                json!({"id": v.id, "name": v.name, "expression": v.expression.text, "value": q.value, "unit": unit_name(q)})
            })
            .collect()
    }
}

/// The name of a quantity's unit.
pub(crate) fn unit_name(q: Quantity) -> &'static str {
    match (q.length, q.angle) {
        (1, 0) => "mm",
        (0, 1) => "deg",
        (0, 0) => "",
        _ => "compound",
    }
}
