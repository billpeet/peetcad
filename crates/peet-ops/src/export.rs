//! Writing files: saving the part and exporting STL, DXF and STEP.

use std::path::{Path, PathBuf};

use peet_document::{Document, FileLocation};
use peet_io::step::StepSchema;
use serde_json::{Map, Value, json};

use crate::args::point2_out;
use crate::op::Format;
use crate::select;

fn stem(doc: &Document) -> String {
    let title = doc.title();
    title.strip_suffix(".peet").unwrap_or(&title).to_owned()
}

fn written(path: &Path, bytes: usize) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("path".to_owned(), json!(path.to_string_lossy()));
    m.insert("bytes".to_owned(), json!(bytes));
    m
}

/// Writes the part to `path`, or to the file it was opened from.
pub(crate) fn save(
    doc: &mut Document,
    path: Option<&PathBuf>,
    caches: bool,
) -> Result<Map<String, Value>, String> {
    let path = match path {
        Some(p) => p.clone(),
        None => doc
            .file
            .as_ref()
            .and_then(|f| f.path.clone())
            .ok_or_else(|| "The part has no file yet: give 'save' a 'path'.".to_owned())?,
    };
    // In the folder it is going to: an assembly's relative links are written from there.
    let bytes = doc.save_bytes_in(caches, path.parent())?;
    peet_platform::write_file(&path, &bytes)?;
    let name = path
        .file_name()
        .map_or_else(|| doc.title(), |n| n.to_string_lossy().into_owned());
    let out = written(&path, bytes.len());
    doc.mark_saved(Some(FileLocation {
        name,
        path: Some(path),
    }));
    Ok(out)
}

/// The format a file name asks for, by its extension.
pub(crate) fn format_of(path: &Path) -> Result<Format, String> {
    let extension = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "stl" => Ok(Format::Stl),
        "dxf" => Ok(Format::Dxf),
        "step" | "stp" => Ok(Format::Step),
        "csv" => Ok(Format::Csv),
        other => Err(format!(
            "'{other}' is not a format to export: use stl, dxf, step or csv (or a path ending in one)."
        )),
    }
}

/// Writes the bodies as STL or STEP, or a sheet metal body's flat pattern as DXF.
pub(crate) fn export(
    doc: &Document,
    path: &Path,
    format: Option<Format>,
    body: Option<usize>,
    schema: StepSchema,
) -> Result<Map<String, Value>, String> {
    let format = match format {
        Some(f) => f,
        None => format_of(path)?,
    };
    let (bytes, mut out) = export_bytes(doc, format, body, schema)?;
    peet_platform::write_file(path, &bytes)?;
    out.extend(written(path, bytes.len()));
    Ok(out)
}

/// An assembly's export: every component's bodies where they are, as STL or STEP.
fn export_assembly(
    doc: &Document,
    format: Format,
    body: Option<usize>,
    schema: StepSchema,
) -> Result<(Vec<u8>, Map<String, Value>), String> {
    if body.is_some() {
        return Err(
            "An assembly is exported whole: leave 'body' out (to export one part, open it with 'open_component')."
                .to_owned(),
        );
    }
    if format == Format::Csv {
        let rows = doc.bill_of_materials(true);
        if rows.is_empty() {
            return Err("There is nothing to list: the assembly has no components.".to_owned());
        }
        let mut out = Map::new();
        out.insert("rows".to_owned(), json!(rows.len()));
        out.insert("format".to_owned(), json!("csv"));
        return Ok((crate::analysis::bom_csv(doc, &rows).into_bytes(), out));
    }
    let solids = crate::assembly::placed_solids(doc);
    if solids.is_empty() {
        return Err(
            "There are no bodies to export: the assembly has no components that show any."
                .to_owned(),
        );
    }
    let mut out = Map::new();
    let (bytes, word) = match format {
        Format::Stl => {
            let mut triangles = Vec::new();
            for (view, placed) in doc.bodies.iter().zip(&doc.placed) {
                triangles.extend(
                    view.tess()
                        .triangles()
                        .into_iter()
                        .map(|t| t.map(|p| placed.frame.to_world(p))),
                );
            }
            out.insert("triangles".to_owned(), json!(triangles.len()));
            (peet_io::stl::write_binary(&stem(doc), &triangles), "stl")
        }
        Format::Dxf => {
            return Err(
                "A flat pattern is a sheet metal part's: open the part with 'open_component' and export it there."
                    .to_owned(),
            );
        }
        Format::Csv => unreachable!("handled above"),
        Format::Step => {
            let named: Vec<(&str, &peet_kernel::Solid)> =
                solids.iter().map(|(n, s)| (n.as_str(), s)).collect();
            let options = peet_io::step::StepOptions {
                schema,
                product_name: stem(doc),
                author: String::new(),
                organization: String::new(),
                timestamp: peet_platform::timestamp_iso(),
            };
            out.insert("bodies".to_owned(), json!(named.len()));
            (peet_io::step::write(&named, &options).into_bytes(), "step")
        }
    };
    out.insert("format".to_owned(), json!(word));
    Ok((bytes, out))
}

/// The contents of an export, with what to say about it: the bodies as STL or STEP, or a
/// sheet metal body's flat pattern as DXF. For a host that writes the file itself (a
/// save dialog, a download).
pub fn export_bytes(
    doc: &Document,
    format: Format,
    body: Option<usize>,
    schema: StepSchema,
) -> Result<(Vec<u8>, Map<String, Value>), String> {
    if doc.is_assembly() {
        return export_assembly(doc, format, body, schema);
    }
    let bodies = &doc.evaluation().bodies;
    if let Some(b) = body
        && b >= bodies.len()
    {
        return Err(format!(
            "There is no body {b}: the part has {} bodies.",
            bodies.len()
        ));
    }
    let chosen: Vec<usize> = match body {
        Some(b) => vec![b],
        None => (0..bodies.len()).collect(),
    };
    let units = &doc.model.parameters.units;
    let mut out = Map::new();
    let (bytes, word) = match format {
        Format::Stl => {
            let triangles: Vec<_> = chosen
                .iter()
                .flat_map(|b| select::mesh(doc, *b).triangles())
                .collect();
            if triangles.is_empty() {
                return Err("There are no bodies to export.".to_owned());
            }
            out.insert("triangles".to_owned(), json!(triangles.len()));
            (peet_io::stl::write_binary(&stem(doc), &triangles), "stl")
        }
        Format::Csv => {
            return Err(
                "A bill of materials is an assembly's: a part has no parts to list.".to_owned(),
            );
        }
        Format::Dxf => {
            let sheet_body = doc.sheet_body(body).ok_or_else(|| {
                "There is no sheet metal body to export: start one with base_flange.".to_owned()
            })?;
            let Some(sheet) = &sheet_body.sheet else {
                return Err("That body is not sheet metal.".to_owned());
            };
            out.insert(
                "flat_size".to_owned(),
                point2_out(sheet.report().flat_size, units),
            );
            (peet_io::dxf::flat_pattern(sheet).into_bytes(), "dxf")
        }
        Format::Step => {
            if chosen.is_empty() {
                return Err("There are no bodies to export.".to_owned());
            }
            let names: Vec<String> = chosen
                .iter()
                .map(|b| doc.model.name_of(bodies[*b].origin).to_owned())
                .collect();
            let solids: Vec<(&str, &peet_kernel::Solid)> = names
                .iter()
                .zip(&chosen)
                .map(|(n, b)| (n.as_str(), &bodies[*b].solid))
                .collect();
            let options = peet_io::step::StepOptions {
                schema,
                product_name: stem(doc),
                author: String::new(),
                organization: String::new(),
                timestamp: peet_platform::timestamp_iso(),
            };
            out.insert("bodies".to_owned(), json!(solids.len()));
            (peet_io::step::write(&solids, &options).into_bytes(), "step")
        }
    };
    out.insert("format".to_owned(), json!(word));
    Ok((bytes, out))
}
