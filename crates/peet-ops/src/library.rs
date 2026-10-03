//! Materials and check limits: the gauge tables (thickness, bend radius and bend model by
//! material and gauge), applying a row to a part, and the limits of the manufacturing
//! checks. They belong to the [`crate::Host`], not to the part.

use std::path::Path;

use peet_document::Document;
use peet_model::{BendModelDef, FeatureId, FeatureKind, Material, Model, Scalar};
use peet_sheetmetal::{BendModel, CheckRules, GaugeEntry, GaugeTable, MaterialLibrary, Rule};
use serde_json::{Map, Value, json};

use crate::args::{Args, length_out, number, round, text};
use crate::host::word_enum;
use crate::value::FeatureSel;

/// How a gauge works out the flat length of its bends.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GaugeBend {
    KFactor(f64),
    /// Bend allowance, in document units.
    Allowance(f64),
    /// Bend deduction, in document units.
    Deduction(f64),
}

/// A row of a material's gauge table, to add or to change. Lengths are in document units.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Gauge {
    /// The material (its table is made if there is none).
    pub material: String,
    /// The name the shop uses: "16 ga", "1.5 mm".
    pub gauge: String,
    /// Needed for a new row.
    pub thickness: Option<f64>,
    /// The default inner bend radius (the thickness, for a new row, if absent).
    pub radius: Option<f64>,
    /// A K-factor of 0.44, for a new row, if absent.
    pub bend: Option<GaugeBend>,
    pub notes: Option<String>,
    /// The material's density in kg/m³ (it is the whole table's, not the row's).
    pub density: Option<f64>,
}

impl Gauge {
    pub(crate) fn parse(a: &mut Args) -> Result<Self, String> {
        let name = |v: &Value| text(v).map(str::to_owned);
        Ok(Self {
            material: a.required("material", name)?,
            gauge: a.required("gauge", name)?,
            thickness: a.parsed("thickness", number)?,
            radius: a.parsed("radius", number)?,
            bend: a.parsed("bend", |v| {
                let entry = v
                    .as_object()
                    .filter(|m| m.len() == 1)
                    .and_then(|m| m.iter().next());
                match entry {
                    Some((k, n)) if k == "k_factor" => number(n).map(GaugeBend::KFactor),
                    Some((k, n)) if k == "allowance" => number(n).map(GaugeBend::Allowance),
                    Some((k, n)) if k == "deduction" => number(n).map(GaugeBend::Deduction),
                    _ => Err(format!(
                        "expected {{\"k_factor\": number}}, {{\"allowance\": number}} or {{\"deduction\": number}}, not {v}"
                    )),
                }
            })?,
            notes: a.string("notes")?,
            density: a.parsed("density", number)?,
        })
    }
}

word_enum! {
    /// One of the limits of the manufacturing checks.
    CheckRule, "a check" {
        /// The shortest flange that can be bent.
        MinFlange = "min_flange",
        /// How close a hole may be to a bend.
        HoleToBend = "hole_to_bend",
        HoleToEdge = "hole_to_edge",
        HoleToHole = "hole_to_hole",
        /// The smallest hole that can be cut.
        MinHole = "min_hole",
        /// How close parts may come when folded.
        Collision = "collision",
    }
}

fn rule_of(rules: &mut CheckRules, rule: CheckRule) -> &mut Rule {
    match rule {
        CheckRule::MinFlange => &mut rules.min_flange,
        CheckRule::HoleToBend => &mut rules.hole_to_bend,
        CheckRule::HoleToEdge => &mut rules.hole_to_edge,
        CheckRule::HoleToHole => &mut rules.hole_to_hole,
        CheckRule::MinHole => &mut rules.min_hole,
        CheckRule::Collision => &mut rules.collision,
    }
}

/// The limits as JSON: each a multiple of the thickness plus a multiple of the bend
/// radius plus a constant in mm.
pub(crate) fn rules_out(rules: &CheckRules) -> Value {
    let mut rules = *rules;
    let mut out = Map::new();
    for rule in CheckRule::ALL {
        let r = *rule_of(&mut rules, *rule);
        out.insert(
            rule.word().to_owned(),
            json!({
                "thickness": r.thickness,
                "radius": r.radius,
                "constant_mm": r.constant,
                "formula": r.formula(),
            }),
        );
    }
    Value::Object(out)
}

/// Changes one limit: the parts given, as multiples of the thickness and of the bend
/// radius and a constant in mm.
pub(crate) fn set_check_rule(
    rules: &mut CheckRules,
    rule: CheckRule,
    thickness: Option<f64>,
    radius: Option<f64>,
    constant: Option<f64>,
) -> Result<Map<String, Value>, String> {
    if [thickness, radius, constant]
        .iter()
        .flatten()
        .any(|v| *v < 0.0)
    {
        return Err("A limit can't be negative.".to_owned());
    }
    let r = rule_of(rules, rule);
    r.thickness = thickness.unwrap_or(r.thickness);
    r.radius = radius.unwrap_or(r.radius);
    r.constant = constant.unwrap_or(r.constant);
    let mut out = Map::new();
    out.insert("rules".to_owned(), rules_out(rules));
    Ok(out)
}

fn entry_out(e: &GaugeEntry, doc: &Document) -> Value {
    let units = &doc.model.parameters.units;
    let bend = match e.model {
        BendModel::KFactor(k) => json!({ "k_factor": round(k) }),
        BendModel::Allowance(v) => json!({ "allowance": length_out(v, units) }),
        BendModel::Deduction(v) => json!({ "deduction": length_out(v, units) }),
    };
    let mut m = Map::new();
    m.insert("gauge".to_owned(), json!(e.gauge));
    m.insert("thickness".to_owned(), length_out(e.thickness, units));
    m.insert("radius".to_owned(), length_out(e.radius, units));
    m.insert("bend".to_owned(), bend);
    if !e.notes.is_empty() {
        m.insert("notes".to_owned(), json!(e.notes));
    }
    Value::Object(m)
}

fn no_material(library: &MaterialLibrary, material: &str) -> String {
    let names: Vec<&str> = library.tables.iter().map(|t| t.material.as_str()).collect();
    format!(
        "There is no material '{material}'. The materials are: {}.",
        if names.is_empty() {
            "(none)".to_owned()
        } else {
            names.join(", ")
        }
    )
}

/// The tables: every material with its gauges, or one material's.
pub(crate) fn materials(
    library: &MaterialLibrary,
    material: Option<&str>,
    doc: &Document,
) -> Result<Map<String, Value>, String> {
    let table_out = |t: &GaugeTable| {
        let gauges: Vec<Value> = t.entries.iter().map(|e| entry_out(e, doc)).collect();
        let mut m = Map::new();
        m.insert("material".to_owned(), json!(t.material));
        if let Some(d) = t.density {
            m.insert("density_kg_m3".to_owned(), json!(d));
        }
        m.insert("gauges".to_owned(), json!(gauges));
        Value::Object(m)
    };
    let tables: Vec<Value> = match material {
        Some(m) => vec![table_out(
            library.table(m).ok_or_else(|| no_material(library, m))?,
        )],
        None => library.tables.iter().map(table_out).collect(),
    };
    let mut out = Map::new();
    out.insert("materials".to_owned(), json!(tables));
    Ok(out)
}

/// A colour as scripts write it: `"#c82828"`, or `[200, 40, 40]`.
pub(crate) fn color(v: &Value) -> Result<[u8; 3], String> {
    let wrong = || format!("expected a colour like \"#c82828\" or [200, 40, 40], not {v}");
    match v {
        Value::String(s) => {
            let hex = s.trim().trim_start_matches('#');
            if hex.len() != 6 || !hex.is_ascii() {
                return Err(wrong());
            }
            let mut out = [0u8; 3];
            for (i, o) in out.iter_mut().enumerate() {
                *o = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).map_err(|_| wrong())?;
            }
            Ok(out)
        }
        Value::Array(items) if items.len() == 3 => {
            let mut out = [0u8; 3];
            for (o, item) in out.iter_mut().zip(items) {
                *o = item
                    .as_u64()
                    .and_then(|n| u8::try_from(n).ok())
                    .ok_or_else(wrong)?;
            }
            Ok(out)
        }
        _ => Err(wrong()),
    }
}

/// A colour as replies give it: `"#c82828"`.
pub(crate) fn color_out(c: [u8; 3]) -> Value {
    json!(format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]))
}

/// A part's material as replies give it.
pub(crate) fn material_out(m: &Material) -> Value {
    json!({ "name": m.name, "density_kg_m3": m.density })
}

/// Sets what the part is made of, or (with no material) clears it. The density is the
/// one given, or the material tables' for that material. Returns the undo label and what
/// was set.
pub(crate) fn set_material(
    library: &MaterialLibrary,
    model: &mut Model,
    material: Option<&str>,
    density: Option<f64>,
) -> Result<(String, Map<String, Value>), String> {
    let mut out = Map::new();
    let Some(name) = material else {
        if density.is_some() {
            return Err("A density needs a material to go with it.".to_owned());
        }
        model.material = None;
        out.insert("material".to_owned(), Value::Null);
        return Ok(("Remove Material".to_owned(), out));
    };
    // A material taken from the tables is called as the tables call it ("mild steel"
    // finds "Mild steel"). One given with its own density is taken as it is written.
    let table = library.table(name).filter(|_| density.is_none());
    let density = match density.or_else(|| table.and_then(|t| t.density)) {
        Some(d) => d,
        None => {
            let known: Vec<&str> = library
                .tables
                .iter()
                .filter(|t| t.density.is_some())
                .map(|t| t.material.as_str())
                .collect();
            return Err(format!(
                "The material tables have no density for '{name}': give a 'density' in kg/m³. They have one for: {}.",
                if known.is_empty() {
                    "(none)".to_owned()
                } else {
                    known.join(", ")
                }
            ));
        }
    };
    let material = Material::new(table.map_or(name, |t| t.material.as_str()), density)?;
    let label = format!("Set Material to {}", material.name);
    out.insert("material".to_owned(), material_out(&material));
    model.material = Some(material);
    Ok((label, out))
}

/// Sets a base flange's thickness, bend radius and bend model from a row of the tables:
/// the gauge named, or the one nearest a thickness. The part's material becomes the
/// table's, if the table has a density. Returns the feature, the undo label and what was
/// applied.
pub(crate) fn apply_material(
    library: &MaterialLibrary,
    doc: &Document,
    model: &mut Model,
    material: &str,
    gauge: Option<&str>,
    thickness: Option<f64>,
    feature: Option<&FeatureSel>,
) -> Result<(FeatureId, String, Value), String> {
    let table = library
        .table(material)
        .ok_or_else(|| no_material(library, material))?;
    let entry = match (gauge, thickness) {
        (Some(g), _) => table.find(g).ok_or_else(|| {
            let names: Vec<&str> = table.entries.iter().map(|e| e.gauge.as_str()).collect();
            format!(
                "{} has no gauge '{g}'. Its gauges are: {}.",
                table.material,
                names.join(", ")
            )
        })?,
        (None, Some(t)) => table
            .nearest(doc.model.parameters.units.to_mm(t))
            .ok_or_else(|| format!("{} has no gauges.", table.material))?,
        (None, None) => {
            return Err("'apply_material' needs a 'gauge' or a 'thickness'.".to_owned());
        }
    };
    let is_base = |id: FeatureId| {
        matches!(
            doc.feature(id).map(|f| &f.kind),
            Some(FeatureKind::BaseFlange(_))
        )
    };
    let target = match feature {
        Some(f) => {
            let id = f.resolve(doc)?;
            if !is_base(id) {
                return Err(format!(
                    "{} is not a base flange: a material is applied to the base flange of a sheet metal body.",
                    doc.model.name_of(id)
                ));
            }
            id
        }
        None => doc
            .model
            .features()
            .map(|f| f.id)
            .find(|id| is_base(*id))
            .ok_or_else(|| {
                "There is no sheet metal body to apply a material to: start one with base_flange."
                    .to_owned()
            })?,
    };
    if let Some(f) = model.feature_mut(target)
        && let FeatureKind::BaseFlange(b) = &mut f.kind
    {
        b.settings.thickness = Scalar::new(entry.thickness);
        b.settings.radius = Scalar::new(entry.radius);
        b.settings.model = match entry.model {
            BendModel::KFactor(k) => BendModelDef::KFactor(Scalar::new(k)),
            BendModel::Allowance(v) => BendModelDef::Allowance(Scalar::new(v)),
            BendModel::Deduction(v) => BendModelDef::Deduction(Scalar::new(v)),
        };
    }
    if let Some(material) = table
        .density
        .and_then(|d| Material::new(&table.material, d).ok())
    {
        model.material = Some(material);
    }
    let label = format!("Apply {} to {}", entry.gauge, doc.model.name_of(target));
    let applied = json!({ "material": table.material, "applied": entry_out(entry, doc) });
    Ok((target, label, applied))
}

/// Adds a row to a material's table, or changes the row with that gauge.
pub(crate) fn set_gauge(
    library: &mut MaterialLibrary,
    doc: &Document,
    gauge: &Gauge,
) -> Result<Map<String, Value>, String> {
    let units = &doc.model.parameters.units;
    let material = gauge.material.trim();
    let name = gauge.gauge.trim();
    if material.is_empty() || name.is_empty() {
        return Err("A material and a gauge need names.".to_owned());
    }
    let bend = gauge.bend.map(|b| match b {
        GaugeBend::KFactor(k) => BendModel::KFactor(k),
        GaugeBend::Allowance(v) => BendModel::Allowance(units.to_mm(v)),
        GaugeBend::Deduction(v) => BendModel::Deduction(units.to_mm(v)),
    });
    let existing = library.table(material).and_then(|t| t.find(name)).cloned();
    let mut entry = match existing {
        Some(e) => e,
        None => {
            let thickness = units.to_mm(gauge.thickness.ok_or_else(|| {
                format!("'{name}' is a new gauge of {material}: it needs a 'thickness'.")
            })?);
            GaugeEntry {
                gauge: name.to_owned(),
                thickness,
                radius: thickness,
                model: BendModel::KFactor(0.44),
                notes: String::new(),
            }
        }
    };
    if let Some(t) = gauge.thickness {
        entry.thickness = units.to_mm(t);
    }
    if let Some(r) = gauge.radius {
        entry.radius = units.to_mm(r);
    }
    if let Some(b) = bend {
        entry.model = b;
    }
    if let Some(n) = &gauge.notes {
        entry.notes.clone_from(n);
    }
    entry.check()?;
    if let Some(d) = gauge.density {
        Material::new(material, d)?;
    }
    let known = library.table(material).map(|t| t.material.clone());
    let table = match known {
        Some(m) => library.tables.iter_mut().find(|t| t.material == m),
        None => {
            library.tables.push(GaugeTable {
                name: material.to_owned(),
                material: material.to_owned(),
                entries: Vec::new(),
                density: None,
            });
            library.tables.last_mut()
        }
    };
    let Some(table) = table else {
        return Err(no_material(library, material));
    };
    let out = entry_out(&entry, doc);
    match table.entries.iter_mut().find(|e| e.gauge == entry.gauge) {
        Some(e) => *e = entry,
        None => table.entries.push(entry),
    }
    if gauge.density.is_some() {
        table.density = gauge.density;
    }
    let mut m = Map::new();
    m.insert("material".to_owned(), json!(table.material));
    if let Some(d) = table.density {
        m.insert("density_kg_m3".to_owned(), json!(d));
    }
    m.insert("gauge".to_owned(), out);
    Ok(m)
}

/// Removes a gauge from a material's table, or the whole table.
pub(crate) fn delete_gauge(
    library: &mut MaterialLibrary,
    material: &str,
    gauge: Option<&str>,
) -> Result<Map<String, Value>, String> {
    let found = library
        .table(material)
        .map(|t| t.material.clone())
        .ok_or_else(|| no_material(library, material))?;
    let mut out = Map::new();
    match gauge {
        None => {
            library.tables.retain(|t| t.material != found);
            out.insert("deleted".to_owned(), json!(found));
        }
        Some(g) => {
            let name = library
                .table(&found)
                .and_then(|t| t.find(g))
                .map(|e| e.gauge.clone())
                .ok_or_else(|| format!("{found} has no gauge '{g}'."))?;
            if let Some(t) = library.tables.iter_mut().find(|t| t.material == found) {
                t.entries.retain(|e| e.gauge != name);
            }
            out.insert("deleted".to_owned(), json!(format!("{found} {name}")));
        }
    }
    Ok(out)
}

/// Replaces the tables with those of a CSV file.
pub(crate) fn import_materials(
    library: &mut MaterialLibrary,
    file: &crate::op::Source,
) -> Result<Map<String, Value>, String> {
    let bytes = file.read()?;
    let text = String::from_utf8_lossy(&bytes);
    let shown = file.shown();
    let read = MaterialLibrary::from_csv(&text).map_err(|errors| {
        let lines: Vec<String> = errors
            .iter()
            .take(5)
            .map(|e| format!("line {}: {}", e.line, e.message))
            .collect();
        format!(
            "{shown} is not a gauge table ({} problems): {}.",
            errors.len(),
            lines.join("; ")
        )
    })?;
    let mut out = Map::new();
    out.insert("materials".to_owned(), json!(read.tables.len()));
    out.insert(
        "gauges".to_owned(),
        json!(read.tables.iter().map(|t| t.entries.len()).sum::<usize>()),
    );
    *library = read;
    Ok(out)
}

/// Writes the tables as CSV.
pub(crate) fn export_materials(
    library: &MaterialLibrary,
    path: &Path,
) -> Result<Map<String, Value>, String> {
    let csv = library.to_csv();
    peet_platform::write_file(path, csv.as_bytes())?;
    let mut out = Map::new();
    out.insert("path".to_owned(), json!(path.to_string_lossy()));
    out.insert("bytes".to_owned(), json!(csv.len()));
    Ok(out)
}
