//! Persistent measurement requests and session-only proof results.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use soliddraft_kernel_api::{BodyId, EdgeId, FaceId, Measurement, PairEndpoint, PairQuantity, PointProof};

use crate::{Document, EdgeRef, FaceRef};

/// A geometry identity, independent of labels and transient kernel handles.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Source {
    /// Length of an edge.
    Edge {
        /// Persistent reference.
        reference: EdgeRef,
    },
    /// Area of a face.
    Face {
        /// Persistent reference.
        reference: FaceRef,
    },
    /// One scalar between two sources of the same part.
    Pair {
        /// The requested scalar.
        quantity: PairQuantity,
        /// First source.
        a: Box<Endpoint>,
        /// Second source.
        b: Box<Endpoint>,
    },
}

/// One end of a pair request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Endpoint {
    /// A whole edge.
    Edge {
        /// Persistent reference.
        reference: EdgeRef,
    },
    /// A whole face.
    Face {
        /// Persistent reference.
        reference: FaceRef,
    },
    /// An edge's arc-length midpoint.
    EdgeMidpoint {
        /// Persistent reference to the edge.
        reference: EdgeRef,
    },
}

impl Endpoint {
    /// The label saved when the reference was created.
    pub fn label(&self) -> &str {
        match self {
            Self::Edge { reference } | Self::EdgeMidpoint { reference } => &reference.label,
            Self::Face { reference } => &reference.label,
        }
    }

    /// The point selector, if this end is a point of its entity.
    pub fn point(&self) -> Option<&'static str> {
        matches!(self, Self::EdgeMidpoint { .. }).then_some("midpoint")
    }

    /// A missing or ambiguous end is named by its saved label; no nearby entity stands in.
    pub fn resolve(&self, doc: &Document, body: BodyId) -> Result<PairEndpoint, String> {
        match self {
            Self::Edge { reference } => doc.resolve_edge(reference, body).map(PairEndpoint::Edge),
            Self::Face { reference } => doc.resolve_face(reference, body).map(PairEndpoint::Face),
            Self::EdgeMidpoint { reference } => doc.resolve_edge(reference, body).map(PairEndpoint::EdgeMidpoint),
        }
        .map_err(|e| format!("measure.unresolved-reference: {}: {}", self.label(), e.message))
    }
}

/// A resolved entity on the current part body.
pub enum Entity {
    /// An edge length.
    Edge(EdgeId),
    /// A face area.
    Face(FaceId),
    /// A pair scalar.
    Pair(PairQuantity, PairEndpoint, PairEndpoint),
}

impl Source {
    /// The label saved when the reference was created. A pair names both sources.
    pub fn label(&self) -> String {
        match self {
            Self::Edge { reference } => reference.label.clone(),
            Self::Face { reference } => reference.label.clone(),
            Self::Pair { a, b, .. } => format!("{} and {}", a.label(), b.label()),
        }
    }

    /// The quantity requested by this reference.
    pub fn quantity(&self) -> &'static str {
        match self {
            Self::Edge { .. } => "edge_length",
            Self::Face { .. } => "face_area",
            Self::Pair { quantity, .. } => quantity.name(),
        }
    }

    /// The quantity's document unit.
    pub fn unit(&self) -> &'static str {
        match self {
            Self::Edge { .. } => "mm",
            Self::Face { .. } => "mm2",
            Self::Pair { quantity, .. } => quantity.unit(),
        }
    }

    /// Refuses a pair whose point selectors do not suit its quantity. A point distance
    /// joins two midpoints; the other quantities measure whole entities.
    pub fn validate(&self) -> Result<(), String> {
        let Self::Pair { quantity, a, b } = self else { return Ok(()) };
        let points = (a.point().is_some(), b.point().is_some());
        match (quantity, points) {
            (PairQuantity::PointDistance, (true, true)) => Ok(()),
            (PairQuantity::PointDistance, _) => Err("point_distance needs a midpoint on both sources".into()),
            (_, (false, false)) => Ok(()),
            _ => Err(format!("{} measures whole entities and takes no midpoint", quantity.name())),
        }
    }

    /// Resolves against the final body, never a previous feature's body.
    pub fn resolve(&self, doc: &Document, body: BodyId) -> Result<Entity, String> {
        match self {
            Self::Edge { reference } => doc.resolve_edge(reference, body).map(Entity::Edge),
            Self::Face { reference } => doc.resolve_face(reference, body).map(Entity::Face),
            Self::Pair { quantity, a, b } => {
                return Ok(Entity::Pair(*quantity, a.resolve(doc, body)?, b.resolve(doc, body)?));
            }
        }
        .map_err(|e| format!("{}: {}", self.label(), e.message))
    }
}

/// An authored row. Proof results stay outside the part's truth.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Row {
    /// Shared value identifier.
    pub name: String,
    /// Persistent geometry reference.
    pub source: Source,
    /// Optional bound comparison.
    pub check: Option<Check>,
}

/// One comparison, parsed through the document's unit model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    /// Authored comparison text.
    pub text: String,
    op: Operator,
    limit: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
enum Operator {
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
}

impl Check {
    /// Parses `<`, `<=`, `>` or `>=` followed by one limit.
    pub fn parse(text: &str, unit: &str) -> Result<Self, String> {
        let text = text.trim();
        let (op, rest) = [
            ("<=", Operator::LessEqual),
            (">=", Operator::GreaterEqual),
            ("<", Operator::Less),
            (">", Operator::Greater),
        ]
        .into_iter()
        .find_map(|(s, op)| text.strip_prefix(s).map(|rest| (op, rest)))
        .ok_or("a check needs <, <=, > or >= and one limit")?;
        let limit = crate::values::parse_limit(rest.trim(), unit)?;
        Ok(Self { text: text.into(), op, limit })
    }

    /// Answers only what a current enclosure proves.
    pub fn state(&self, measurement: Option<&Measurement>) -> &'static str {
        let Some(m) = measurement else { return "unknown" };
        let (pass, fail) = match self.op {
            Operator::Less => (m.hi < self.limit, m.lo >= self.limit),
            Operator::LessEqual => (m.hi <= self.limit, m.lo > self.limit),
            Operator::Greater => (m.lo > self.limit, m.hi <= self.limit),
            Operator::GreaterEqual => (m.lo >= self.limit, m.hi < self.limit),
        };
        if pass {
            "pass"
        } else if fail {
            "fail"
        } else {
            "unknown"
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct Observation {
    measurement: Option<Measurement>,
    geometry: Option<soliddraft_document::ContentHash>,
    reason: Option<String>,
}

impl Document {
    pub(crate) fn track_request(
        &self,
        state: &mut crate::values::Values,
        name: &str,
        source: &Source,
        check: Option<&str>,
    ) -> Result<String, String> {
        crate::value_expr::check_name(name)?;
        if state.summary().as_array().is_some_and(|rows| rows.iter().any(|r| r["name"] == name))
            || state.tracked.iter().any(|r| r.name == name)
        {
            return Err(format!("the name {name} is already taken"));
        }
        source.validate()?;
        let check = check.map(|text| Check::parse(text, source.unit())).transpose()?;
        self.measure_source(source)?;
        state.tracked.push(Row { name: name.into(), source: source.clone(), check });
        Ok(format!("track {name}"))
    }

    /// Authored measurement requests, in creation order.
    pub fn tracked(&self) -> &[Row] {
        &self.values.tracked
    }

    /// A single proof on the current part, without changing either hash.
    pub fn measure_source(&self, source: &Source) -> Result<Measurement, String> {
        if self.is_dirty() {
            return Err("proof pending".into());
        }
        let body = self.body().ok_or("the part has no body")?;
        match source.resolve(self, body)? {
            Entity::Edge(edge) => self.kernel.edge_length(body, edge),
            Entity::Face(face) => self.kernel.face_area(body, face),
            Entity::Pair(quantity, a, b) => self.kernel.measure_pair(body, a, b, quantity),
        }
        .map_err(|e| format!("{} could not be measured: {e}", source.label()))
    }

    /// The proven midpoint of one straight edge on the current part. A coordinate bundle
    /// is a readout, not a tracked scalar.
    pub fn measure_midpoint(&self, reference: &EdgeRef) -> Result<PointProof, String> {
        if self.is_dirty() {
            return Err("proof pending".into());
        }
        let body = self.body().ok_or("the part has no body")?;
        let edge = self.resolve_edge(reference, body).map_err(|e| format!("{}: {}", reference.label, e.message))?;
        self.kernel.edge_midpoint(body, edge).map_err(|e| format!("{} could not be measured: {e}", reference.label))
    }

    pub(crate) fn refresh_tracked(&mut self) {
        if self.values.tracked.is_empty() {
            self.observations.clear();
            return;
        }
        let geometry = self.geometry_hash();
        for row in &self.values.tracked {
            let result = self.measure_source(&row.source);
            let previous = self.observations.entry(row.name.clone()).or_default();
            match result {
                Ok(measurement) => {
                    *previous =
                        Observation { measurement: Some(measurement), geometry: Some(geometry.clone()), reason: None }
                }
                Err(reason) => previous.reason = Some(reason),
            }
        }
        self.observations.retain(|name, _| self.values.tracked.iter().any(|r| r.name == *name));
    }

    /// Marks results stale while an interactive proof is pending or has failed.
    pub fn stale_tracked(&mut self, reason: &str) {
        for row in &self.values.tracked {
            self.observations.entry(row.name.clone()).or_default().reason = Some(reason.into());
        }
    }

    /// Values followed by tracked rows, with session-only proof evidence.
    pub fn values_summary(&self) -> Value {
        let mut rows = self.values.summary().as_array().cloned().unwrap_or_default();
        let geometry = (!self.values.tracked.is_empty()).then(|| self.geometry_hash());
        for row in &self.values.tracked {
            let o = self.observations.get(&row.name).cloned().unwrap_or_default();
            let current = !self.is_dirty()
                && o.reason.is_none()
                && o.measurement.is_some()
                && o.geometry.as_ref() == geometry.as_ref();
            let reason = if current { None } else { Some(o.reason.as_deref().unwrap_or("proof pending")) };
            let m = o.measurement;
            let mut row_json = json!({"name": row.name, "expression": null, "value": m.map(|m| m.value),
            "unit": row.source.unit(), "tracked": {
                "quantity": row.source.quantity(), "source": row.source.label(),
                "state": if current { "current" } else { "stale" }, "reason": reason,
                "certainty": m.map(|m| m.certainty), "lo": m.map(|m| m.lo), "hi": m.map(|m| m.hi),
                "frame": "part", "geometry": o.geometry,
                "check": row.check.as_ref().map(|c| json!({"text": c.text,
                    "state": c.state(if current { m.as_ref() } else { None })}))
            }});
            if let Source::Pair { a, b, .. } = &row.source {
                row_json["tracked"]["sources"] =
                    json!([a, b].map(|e| json!({"source": e.label(), "point": e.point()})));
            }
            rows.push(row_json);
        }
        Value::Array(rows)
    }
}
