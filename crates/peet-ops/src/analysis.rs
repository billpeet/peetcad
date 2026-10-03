//! What is asked of an assembly as a whole: interference between its components, its
//! bill of materials, and its mass. The working out is the document's
//! (`peet_document`); this turns it into replies, and the bill into CSV.

use peet_document::{BomRow, Document};
use serde_json::{Map, Value, json};

use crate::args::{length_out, point2_out, point3_out, round};
use crate::assembly::CompSel;

/// Where components run into each other.
pub(crate) fn interference(
    doc: &Document,
    component: Option<&CompSel>,
) -> Result<Map<String, Value>, String> {
    let only = component.map(|c| c.resolve(doc)).transpose()?;
    let units = &doc.model.parameters.units;
    let found = doc.interferences(only);
    let list: Vec<Value> = found
        .found
        .iter()
        .map(|i| {
            json!({
                "a": i.a,
                "b": i.b,
                "volume_mm3": round(i.volume),
                "min": point3_out(i.bounds.min, units),
                "max": point3_out(i.bounds.max, units),
            })
        })
        .collect();
    let mut out = Map::new();
    out.insert(
        "clear".to_owned(),
        json!(found.found.is_empty() && found.unchecked.is_empty()),
    );
    out.insert("interferences".to_owned(), json!(list));
    out.insert("compared".to_owned(), json!(found.compared));
    if !found.unchecked.is_empty() {
        let unchecked: Vec<Value> = found
            .unchecked
            .iter()
            .map(|(a, b, why)| json!({ "a": a, "b": b, "reason": why }))
            .collect();
        out.insert("unchecked".to_owned(), json!(unchecked));
    }
    Ok(out)
}

fn row_out(doc: &Document, row: &BomRow) -> Value {
    let units = &doc.model.parameters.units;
    let mut m = Map::new();
    m.insert("item".to_owned(), json!(row.item));
    m.insert("part".to_owned(), json!(row.part));
    if row.is_assembly {
        m.insert("kind".to_owned(), json!("assembly"));
    }
    m.insert("quantity".to_owned(), json!(row.quantity));
    if let Some(material) = &row.material {
        m.insert("material".to_owned(), json!(material));
    }
    m.insert("volume_mm3".to_owned(), json!(round(row.volume)));
    if let Some(mass) = row.mass {
        m.insert("mass_kg".to_owned(), json!(round(mass)));
        m.insert(
            "total_mass_kg".to_owned(),
            json!(round(mass * row.quantity as f64)),
        );
    }
    if let Some(sheet) = &row.sheet {
        m.insert("thickness".to_owned(), length_out(sheet.thickness, units));
        m.insert("flat_size".to_owned(), point2_out(sheet.flat_size, units));
        m.insert("bends".to_owned(), json!(sheet.bends));
    }
    m.insert("components".to_owned(), json!(row.components));
    Value::Object(m)
}

/// The bill of materials.
pub(crate) fn bom(doc: &Document, top_level: bool) -> Map<String, Value> {
    let rows = doc.bill_of_materials(!top_level);
    let mut out = Map::new();
    out.insert(
        "level".to_owned(),
        json!(if top_level { "top" } else { "parts" }),
    );
    out.insert(
        "rows".to_owned(),
        json!(rows.iter().map(|r| row_out(doc, r)).collect::<Vec<_>>()),
    );
    out.insert(
        "quantity".to_owned(),
        json!(rows.iter().map(|r| r.quantity).sum::<usize>()),
    );
    // The whole, if every line could be weighed.
    let mass: Option<f64> = rows
        .iter()
        .map(|r| r.mass.map(|m| m * r.quantity as f64))
        .sum();
    match mass {
        Some(mass) if !rows.is_empty() => {
            out.insert("mass_kg".to_owned(), json!(round(mass)));
        }
        _ => {
            let bare: Vec<&str> = rows
                .iter()
                .filter(|r| r.mass.is_none())
                .map(|r| r.part.as_str())
                .collect();
            if !bare.is_empty() {
                out.insert("without_mass".to_owned(), json!(bare));
            }
        }
    }
    out
}

/// A CSV field, quoted if it has to be.
fn field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) || s.trim() != s {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}

/// The bill of materials as CSV: a header line, then a line per part. Lengths are in
/// the document's unit, which the header names.
pub(crate) fn bom_csv(doc: &Document, rows: &[BomRow]) -> String {
    let units = &doc.model.parameters.units;
    let unit = units.length.suffix();
    let number = |v: f64| round(v).to_string();
    let length = |mm: f64| number(units.from_mm(mm));
    let mut out = format!(
        "item,part,quantity,material,mass_kg,total_mass_kg,thickness_{unit},flat_width_{unit},flat_height_{unit},bends,components\n"
    );
    for row in rows {
        let (thickness, width, height, bends) = row.sheet.as_ref().map_or_else(
            || (String::new(), String::new(), String::new(), String::new()),
            |s| {
                (
                    length(s.thickness),
                    length(s.flat_size.x),
                    length(s.flat_size.y),
                    s.bends.to_string(),
                )
            },
        );
        let fields = [
            row.item.to_string(),
            field(&row.part),
            row.quantity.to_string(),
            field(row.material.as_deref().unwrap_or("")),
            row.mass.map_or_else(String::new, number),
            row.mass
                .map_or_else(String::new, |m| number(m * row.quantity as f64)),
            thickness,
            width,
            height,
            bends,
            field(&row.components.join(" ")),
        ];
        out.push_str(&fields.join(","));
        out.push('\n');
    }
    out
}

/// The mass properties of an assembly: every component, and the whole.
pub(crate) fn mass(doc: &Document, body: Option<usize>) -> Result<Map<String, Value>, String> {
    if body.is_some() {
        return Err(
            "An assembly is weighed as a whole, component by component: leave 'body' out (to weigh one part, open it with 'open_component')."
                .to_owned(),
        );
    }
    let units = &doc.model.parameters.units;
    let mut out = Map::new();
    let Some(mass) = doc.assembly_mass() else {
        return Err(
            "There is nothing to weigh: no component of the assembly shows a body.".to_owned(),
        );
    };
    let components: Vec<Value> = mass
        .components
        .iter()
        .map(|c| {
            let mut m = Map::new();
            m.insert("component".to_owned(), json!(c.name));
            m.insert("volume_mm3".to_owned(), json!(round(c.volume)));
            if let Some(kg) = c.mass {
                m.insert("mass_kg".to_owned(), json!(round(kg)));
            }
            m.insert(
                "center_of_gravity".to_owned(),
                point3_out(c.centroid, units),
            );
            Value::Object(m)
        })
        .collect();
    out.insert("components".to_owned(), json!(components));
    let mut total = Map::new();
    total.insert("volume_mm3".to_owned(), json!(round(mass.volume)));
    total.insert("area_mm2".to_owned(), json!(round(mass.area)));
    // Weighed if every part has a material; else measured as if all were one material.
    match mass.mass {
        Some(kg) => {
            total.insert("mass_kg".to_owned(), json!(round(kg)));
            total.insert("center_of_gravity_of".to_owned(), json!("mass"));
            total.insert(
                "principal_moments_kg_mm2".to_owned(),
                json!(mass.principal_moments.map(round)),
            );
        }
        None => {
            total.insert("center_of_gravity_of".to_owned(), json!("volume"));
            total.insert(
                "principal_moments_mm5".to_owned(),
                json!(mass.principal_moments.map(round)),
            );
        }
    }
    total.insert(
        "center_of_gravity".to_owned(),
        point3_out(mass.centroid, units),
    );
    total.insert("min".to_owned(), point3_out(mass.bounds.min, units));
    total.insert("max".to_owned(), point3_out(mass.bounds.max, units));
    out.insert("total".to_owned(), Value::Object(total));
    if !mass.without_material.is_empty() {
        out.insert("without_material".to_owned(), json!(mass.without_material));
    }
    if !mass.problems.is_empty() {
        out.insert("warnings".to_owned(), json!(mass.problems));
    }
    Ok(out)
}
