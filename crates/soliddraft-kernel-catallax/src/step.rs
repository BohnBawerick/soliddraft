//! STEP AP242 export through Catallax's exporter, with its proven deviation report.

use catallax_app::IncrementalApplicationBody;
use catallax_numbers::Rational;
use soliddraft_kernel_api::{KernelError, KernelResult, StepExport, StepOptions, StepReport};

use crate::{numerical, query_error};

/// Writes the exact body as STEP. The deviation is Catallax's proven bound, rounded up to
/// a decimal and to the next `f64`, never down.
pub(super) fn export(app: &IncrementalApplicationBody, options: &StepOptions) -> KernelResult<StepExport> {
    if !options.tolerance.is_finite() || options.tolerance <= 0.0 {
        return Err(KernelError::InvalidInput(format!(
            "the STEP tolerance is {} mm; it must be more than zero",
            options.tolerance
        )));
    }
    let tolerance =
        Rational::parse(&options.tolerance.to_string()).map_err(|e| KernelError::InvalidInput(e.to_string()))?;
    let catallax_options = catallax_step::StepOptions {
        product_name: options.product_name.clone(),
        curve_tolerance: tolerance.clone(),
        ..catallax_step::StepOptions::default()
    };
    let mut bytes = Vec::new();
    let built = app.checkpoint.built();
    let answer = numerical("export_step", |dec| {
        catallax_step::write_step(&built.model, &[built.result], &catallax_options, dec, &mut bytes)
    })
    .map_err(|e| query_error("export_step", e))?;
    let text = String::from_utf8(bytes).map_err(|e| KernelError::QueryFailed {
        query: "export_step".into(),
        code: 0,
        message: e.to_string(),
    })?;
    let report = answer.value;
    let max_deviation = report.max_deviation.to_decimal_bounds(40).1;
    Ok(StepExport {
        text,
        report: StepReport {
            max_deviation_mm: f64_up(&report.max_deviation),
            max_deviation,
            within_tolerance: report.max_deviation <= tolerance,
            solids: report.solids,
            faces: report.faces,
            edges: report.edges,
            vertices: report.vertices,
            fitted_curves: report.approximated_curves,
            added_vertices: report.added_vertices,
            added_edges: report.added_edges,
            warnings: answer.marks.iter().map(|m| format!("{}: {}", m.code, m.detail)).collect(),
        },
    })
}

/// The smallest `f64` not below `value`.
fn f64_up(value: &Rational) -> f64 {
    let nearest = value.to_f64_nearest();
    match Rational::from_f64_exact(nearest) {
        Some(exact) if &exact < value => nearest.next_up(),
        _ => nearest,
    }
}
