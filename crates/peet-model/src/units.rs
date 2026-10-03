//! The one place where feature values meet the expression engine and the document units.

use peet_sketch::expr::{Parameters, QuantityKind};

use crate::feature::ScalarKind;

fn quantity_kind(kind: ScalarKind) -> QuantityKind {
    match kind {
        ScalarKind::Length => QuantityKind::Length,
        ScalarKind::Angle => QuantityKind::Angle,
        ScalarKind::Number => QuantityKind::Number,
    }
}

/// Evaluates `source` against the parameter table, giving mm or degrees. A plain number
/// is taken in the document's length unit (or degrees).
pub(crate) fn evaluate(source: &str, kind: ScalarKind, params: &Parameters) -> Result<f64, String> {
    params
        .evaluate_as(source, quantity_kind(kind))
        .map_err(|e| e.message)
}

/// A length in document units, without the unit suffix (for edit fields).
pub(crate) fn length_value_text(mm: f64, params: &Parameters) -> String {
    params.units.format_length_value(mm)
}
