//! Reading an operation's fields from JSON, with messages that say what was expected.

use peet_math::{DVec2, DVec3};
use peet_sketch::expr::Units;
use serde_json::{Map, Value};

/// The fields of one JSON operation. Fields are taken as they are read; [`Args::check`]
/// reports any that nothing asked for, so a misspelt field is an error, not ignored.
pub struct Args {
    op: String,
    map: Map<String, Value>,
    /// Every field the operation asked for, for the message about unknown ones.
    known: Vec<String>,
}

impl Args {
    pub fn new(op: &str, map: Map<String, Value>) -> Self {
        Self {
            op: op.to_owned(),
            map,
            known: Vec::new(),
        }
    }

    pub fn has(&self, key: &str) -> bool {
        self.map.contains_key(key)
    }

    fn note(&mut self, key: &str) {
        if !self.known.iter().any(|k| k == key) {
            self.known.push(key.to_owned());
        }
    }

    /// The field, if it is there and not `null`.
    pub fn take(&mut self, key: &str) -> Option<Value> {
        self.note(key);
        self.map.remove(key).filter(|v| !v.is_null())
    }

    /// Like [`Args::take`], but keeps an explicit `null` (it means "none").
    pub fn take_nullable(&mut self, key: &str) -> Option<Value> {
        self.note(key);
        self.map.remove(key)
    }

    pub fn require(&mut self, key: &str) -> Result<Value, String> {
        self.take(key)
            .ok_or_else(|| format!("'{}' needs a '{key}' field.", self.op))
    }

    /// The field read with `parse`, with the field's name in front of any error.
    pub fn parsed<T>(
        &mut self,
        key: &str,
        parse: impl FnOnce(&Value) -> Result<T, String>,
    ) -> Result<Option<T>, String> {
        self.take(key)
            .map(|v| parse(&v).map_err(|e| format!("{key}: {e}")))
            .transpose()
    }

    /// Like [`Args::parsed`], for a field that must be there.
    pub fn required<T>(
        &mut self,
        key: &str,
        parse: impl FnOnce(&Value) -> Result<T, String>,
    ) -> Result<T, String> {
        let v = self.require(key)?;
        parse(&v).map_err(|e| format!("{key}: {e}"))
    }

    pub fn string(&mut self, key: &str) -> Result<Option<String>, String> {
        self.parsed(key, |v| text(v).map(str::to_owned))
    }

    pub fn flag(&mut self, key: &str, default: bool) -> Result<bool, String> {
        Ok(self.parsed(key, boolean)?.unwrap_or(default))
    }

    /// Fails if a field is left that nothing asked for. Call it once every field has been
    /// taken.
    pub fn check(&self) -> Result<(), String> {
        let Some(unknown) = self.map.keys().next() else {
            return Ok(());
        };
        let mut known = self.known.clone();
        known.sort();
        Err(format!(
            "'{}' has no field '{unknown}'. Its fields are: {}.",
            self.op,
            if known.is_empty() {
                "(none)".to_owned()
            } else {
                known.join(", ")
            }
        ))
    }
}

pub fn number(v: &Value) -> Result<f64, String> {
    v.as_f64()
        .filter(|n| n.is_finite())
        .ok_or_else(|| format!("expected a number, not {v}"))
}

pub fn integer(v: &Value) -> Result<u32, String> {
    v.as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| format!("expected a whole number, not {v}"))
}

pub fn boolean(v: &Value) -> Result<bool, String> {
    v.as_bool()
        .ok_or_else(|| format!("expected true or false, not {v}"))
}

pub fn text(v: &Value) -> Result<&str, String> {
    v.as_str().ok_or_else(|| format!("expected text, not {v}"))
}

pub fn list(v: &Value) -> Result<&[Value], String> {
    v.as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| format!("expected a list, not {v}"))
}

/// `[x, y]` or `[x, y, z]`, as written.
pub fn coordinates<const N: usize>(v: &Value) -> Result<[f64; N], String> {
    let items = list(v)?;
    if items.len() != N {
        return Err(format!("expected {N} coordinates, not {v}"));
    }
    let mut out = [0.0; N];
    for (o, item) in out.iter_mut().zip(items) {
        *o = number(item)?;
    }
    Ok(out)
}

/// A point in document units, as mm.
pub fn mm2(p: [f64; 2], units: &Units) -> DVec2 {
    DVec2::new(units.to_mm(p[0]), units.to_mm(p[1]))
}

/// A point in document units, as mm.
pub fn mm3(p: [f64; 3], units: &Units) -> DVec3 {
    DVec3::new(units.to_mm(p[0]), units.to_mm(p[1]), units.to_mm(p[2]))
}

pub fn round(v: f64) -> f64 {
    let r = (v * 1e6).round() / 1e6;
    if r == 0.0 { 0.0 } else { r }
}

/// A length in mm as a number in document units.
pub fn length_out(mm: f64, units: &Units) -> Value {
    Value::from(round(units.from_mm(mm)))
}

pub fn point2_out(p: DVec2, units: &Units) -> Value {
    Value::from(vec![length_out(p.x, units), length_out(p.y, units)])
}

pub fn point3_out(p: DVec3, units: &Units) -> Value {
    Value::from(vec![
        length_out(p.x, units),
        length_out(p.y, units),
        length_out(p.z, units),
    ])
}

pub fn direction_out(d: DVec3) -> Value {
    Value::from(vec![round(d.x), round(d.y), round(d.z)])
}
