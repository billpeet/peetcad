//! The operations, as a type: what [`crate::apply`] takes, and how a JSON operation is
//! read into one.

use std::path::PathBuf;

use peet_document::Document;
use peet_io::step::StepSchema;
use peet_sketch::expr::LengthUnit;
use serde_json::{Map, Value};

use crate::args::{Args, boolean, integer, list, number, text};
use crate::fields::FeatureArgs;
use crate::host::{AppCommand, word_enum};
use crate::library::{CheckRule, Gauge};
use crate::sketch::{self, DrawItem};
use crate::value::{EdgeSel, FeatureSel, GeomSel, Input, PlaneSel};

word_enum! {
    /// A sample part that comes with PeetCAD.
    Sample, "a sample" {
        /// A 20-feature bracket.
        Bracket = "bracket",
        /// A sheet metal enclosure panel with four flanges.
        Enclosure = "enclosure",
        /// A sheet metal chassis tray.
        Chassis = "chassis",
        /// A revolved housing with a bolt circle.
        Housing = "housing",
    }
}

word_enum! {
    /// The reference geometry every part has.
    DatumSel, "built-in reference geometry" {
        Origin = "origin",
        Front = "front",
        Top = "top",
        Right = "right",
        /// The three standard planes together.
        Planes = "planes",
    }
}

word_enum! {
    /// Where an imported drawing is put in its sketch.
    DxfPlacement, "a placement" {
        /// Where the file has it.
        Keep = "keep",
        /// Its middle at the sketch's origin.
        Centred = "centred",
        /// Its lower left corner at the sketch's origin.
        LowerLeft = "lower_left",
    }
}

/// The sketch an imported drawing goes into.
#[derive(Clone, Debug, PartialEq)]
pub enum DxfTarget {
    /// A sketch the part already has.
    Sketch(FeatureSel),
    /// A new sketch on a plane or a flat face.
    New { on: PlaneSel, name: Option<String> },
}

/// A feature to add: its kind and fields, and optionally its name.
#[derive(Clone, Debug, PartialEq)]
pub struct New {
    /// An automatic name (`Extrude1`) if absent.
    pub name: Option<String>,
    pub feature: FeatureArgs,
}

impl From<FeatureArgs> for New {
    fn from(feature: FeatureArgs) -> Self {
        Self {
            name: None,
            feature,
        }
    }
}

/// Where to move a feature in the tree.
#[derive(Clone, Debug, PartialEq)]
pub enum Place {
    Before(FeatureSel),
    After(FeatureSel),
    Index(usize),
}

/// Something to read from the part.
#[derive(Clone, Debug, PartialEq)]
pub enum Query {
    /// Every operation and its fields.
    Help,
    /// The document in a few lines: name, units, counts, failures.
    Status,
    /// The feature tree, with each feature's status.
    Features,
    /// One feature: its fields, and a sketch's contents.
    Feature(FeatureSel),
    Parameters,
    /// Bounds, volume, and flat size for sheet metal.
    Bodies,
    /// The faces of one body, or of all.
    Faces {
        body: Option<usize>,
    },
    Edges {
        body: Option<usize>,
    },
    /// A sheet metal body's flat size and bends.
    BendTable {
        body: Option<usize>,
    },
    /// Manufacturing checks of a sheet metal body.
    Checks {
        body: Option<usize>,
    },
    /// The material and gauge tables: every material, or one.
    Materials {
        material: Option<String>,
    },
    /// Mass properties for a density of 1: volume, area, centre of gravity, principal
    /// moments of inertia. Of one body, or of all (each, and together).
    Mass {
        body: Option<usize>,
    },
    /// The exact measurements of a face, an edge or a vertex; with `b`, the distance and
    /// the angle between the two.
    Measure {
        a: Box<GeomSel>,
        b: Option<Box<GeomSel>>,
    },
}

/// A file format to export.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Stl,
    /// A sheet metal body's flat pattern.
    Dxf,
    Step,
}

/// One operation on a part. A change is one undo step; a query changes nothing.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// Add features (several as one step: an edge flange on each of several edges).
    Add(Vec<New>),
    /// Change fields of a feature. `fields` must be of the feature's kind.
    Edit {
        feature: FeatureSel,
        fields: FeatureArgs,
    },
    /// Add a sketch on a plane or a flat face, and draw in it.
    Sketch {
        on: PlaneSel,
        name: Option<String>,
        draw: Vec<DrawItem>,
    },
    /// Draw in a sketch: geometry, relations, dimensions and edits.
    Draw {
        sketch: FeatureSel,
        draw: Vec<DrawItem>,
    },
    /// Change a sketch dimension (`"d1"`).
    SetDimension {
        sketch: FeatureSel,
        name: String,
        value: Input,
    },
    Rename {
        feature: FeatureSel,
        name: String,
    },
    /// Suppress a feature (skip it when rebuilding), or unsuppress it.
    Suppress {
        feature: FeatureSel,
        on: bool,
    },
    /// Show or hide a feature's own geometry (a sketch, a plane).
    Show {
        feature: FeatureSel,
        on: bool,
    },
    Delete {
        features: Vec<FeatureSel>,
    },
    Move {
        feature: FeatureSel,
        to: Place,
    },
    /// Put the rollback bar after a feature (`None`: at the end).
    Rollback {
        to: Option<FeatureSel>,
    },
    /// Add or change a named value usable in every expression.
    SetParameter {
        name: String,
        value: Input,
    },
    DeleteParameter {
        name: String,
    },
    SetUnits {
        length: LengthUnit,
    },
    /// Add the solids of a STEP file as bodies, in one feature named after the file.
    /// Always its own undo step.
    ImportStep {
        path: PathBuf,
    },
    /// Bring the lines, arcs, circles and polylines of a DXF file into a sketch.
    ImportDxf {
        path: PathBuf,
        into: DxfTarget,
        /// The file's unit, if it doesn't say (or says wrong).
        unit: Option<LengthUnit>,
        placement: DxfPlacement,
    },
    /// Show or hide the origin or the standard planes.
    ShowDatum {
        datum: DatumSel,
        on: bool,
    },
    /// Show sheet metal bodies as their flat patterns, or folded (`None`: the other way).
    /// A view: it is not an undo step.
    FlatPattern {
        on: Option<bool>,
    },
    /// Set a base flange's thickness, bend radius and bend model from the material
    /// tables: the gauge named, or the one nearest a thickness.
    ApplyMaterial {
        material: String,
        gauge: Option<String>,
        thickness: Option<f64>,
        /// The first base flange if absent.
        feature: Option<FeatureSel>,
    },
    /// Add a row to a material's gauge table, or change one.
    SetGauge(Gauge),
    /// Remove a gauge from a material's table, or (with no gauge) the whole table.
    DeleteGauge {
        material: String,
        gauge: Option<String>,
    },
    /// Replace the material tables with those of a CSV file.
    ImportMaterials {
        path: PathBuf,
    },
    ExportMaterials {
        path: PathBuf,
    },
    /// Change a limit of the manufacturing checks: multiples of the thickness and of the
    /// bend radius, and a constant in mm. The parts left out keep their values.
    SetCheckRule {
        rule: CheckRule,
        thickness: Option<f64>,
        radius: Option<f64>,
        constant: Option<f64>,
    },
    /// Start a new, empty part. Refused if the part has unsaved changes, unless told to
    /// discard them.
    New {
        discard: bool,
    },
    /// Open a part from a `.peet` file.
    Open {
        path: PathBuf,
        discard: bool,
    },
    OpenSample {
        sample: Sample,
        discard: bool,
    },
    /// Something for the application itself, not the part: it needs a running PeetCAD.
    App(AppCommand),
    Undo,
    Redo,
    Query(Query),
    /// Save the part: to `path`, or to the file it came from.
    Save {
        path: Option<PathBuf>,
        /// Include the geometry caches that make the file open instantly.
        caches: bool,
    },
    /// Export the bodies as STL or STEP, or a sheet metal body's flat pattern as DXF.
    Export {
        path: PathBuf,
        /// Taken from the path's extension if absent.
        format: Option<Format>,
        body: Option<usize>,
        schema: StepSchema,
    },
}

impl Op {
    /// One feature to add, with an automatic name.
    pub fn add(feature: FeatureArgs) -> Self {
        Self::Add(vec![feature.into()])
    }

    /// The operation's name, as in JSON.
    pub fn word(&self) -> &'static str {
        match self {
            Self::Add(new) => new
                .first()
                .and_then(|n| n.feature.word().split('.').next())
                .unwrap_or("add"),
            Self::Edit { .. } => "edit",
            Self::Sketch { .. } => "sketch",
            Self::Draw { .. } => "draw",
            Self::SetDimension { .. } => "set_dimension",
            Self::Rename { .. } => "rename",
            Self::Suppress { .. } => "suppress",
            Self::Show { .. } => "show",
            Self::Delete { .. } => "delete",
            Self::Move { .. } => "move",
            Self::Rollback { .. } => "rollback",
            Self::SetParameter { .. } => "set_parameter",
            Self::DeleteParameter { .. } => "delete_parameter",
            Self::SetUnits { .. } => "set_units",
            Self::ImportStep { .. } => "import_step",
            Self::ImportDxf { .. } => "import_dxf",
            Self::ShowDatum { .. } => "show",
            Self::FlatPattern { .. } => "flat_pattern",
            Self::ApplyMaterial { .. } => "apply_material",
            Self::SetGauge(_) => "set_gauge",
            Self::DeleteGauge { .. } => "delete_gauge",
            Self::ImportMaterials { .. } => "import_materials",
            Self::ExportMaterials { .. } => "export_materials",
            Self::SetCheckRule { .. } => "set_check_rule",
            Self::New { .. } => "new",
            Self::Open { .. } => "open",
            Self::OpenSample { .. } => "open_sample",
            Self::App(command) => command.word(),
            Self::Undo => "undo",
            Self::Redo => "redo",
            Self::Query(q) => match q {
                Query::Help => "help",
                Query::Status => "status",
                Query::Features => "features",
                Query::Feature(_) => "feature",
                Query::Parameters => "parameters",
                Query::Bodies => "bodies",
                Query::Faces { .. } => "faces",
                Query::Edges { .. } => "edges",
                Query::BendTable { .. } => "bend_table",
                Query::Checks { .. } => "checks",
                Query::Materials { .. } => "materials",
                Query::Mass { .. } => "mass",
                Query::Measure { .. } => "measure",
            },
            Self::Save { .. } => "save",
            Self::Export { .. } => "export",
        }
    }
}

fn units(v: &Value) -> Result<LengthUnit, String> {
    let word = text(v)?;
    LengthUnit::ALL
        .into_iter()
        .find(|u| u.suffix() == word)
        .ok_or_else(|| {
            format!(
                "'{word}' is not a unit: use {}.",
                LengthUnit::ALL.map(LengthUnit::suffix).join(", ")
            )
        })
}

fn body(a: &mut Args) -> Result<Option<usize>, String> {
    a.parsed("body", |v| integer(v).map(|b| b as usize))
}

fn path(a: &mut Args, op: &str) -> Result<PathBuf, String> {
    a.string("path")?
        .map(PathBuf::from)
        .ok_or_else(|| format!("'{op}' needs a 'path' field."))
}

fn feature(a: &mut Args, key: &str) -> Result<FeatureSel, String> {
    a.required(key, FeatureSel::parse)
}

/// Reads the features a JSON operation adds: one, or one per edge for an edge flange or
/// a hem given `edges`.
fn new_features(op: &str, a: &mut Args) -> Result<Vec<New>, String> {
    let name = a.string("name")?;
    let per_edge = matches!(op, "edge_flange" | "hem") && a.has("edges");
    let edges = if per_edge {
        if a.has("edge") {
            return Err("Give 'edge' or 'edges', not both.".to_owned());
        }
        let edges: Vec<EdgeSel> =
            a.required("edges", |v| list(v)?.iter().map(EdgeSel::parse).collect())?;
        if edges.is_empty() {
            return Err("'edges' is empty.".to_owned());
        }
        if name.is_some() && edges.len() > 1 {
            return Err(
                "'name' can't be used with several edges: rename them afterwards.".to_owned(),
            );
        }
        Some(edges)
    } else {
        None
    };
    let word = FeatureArgs::word_for(op, a);
    let args =
        FeatureArgs::parse(&word, a).ok_or_else(|| format!("'{op}' doesn't add a feature."))??;
    let Some(edges) = edges else {
        return Ok(vec![New {
            name,
            feature: args,
        }]);
    };
    Ok(edges
        .into_iter()
        .map(|edge| {
            let mut feature = args.clone();
            match &mut feature {
                FeatureArgs::EdgeFlange(f) => f.edge = Some(edge),
                FeatureArgs::Hem(h) => h.edge = Some(edge),
                _ => {}
            }
            New {
                name: name.clone(),
                feature,
            }
        })
        .collect())
}

/// The operations that aren't feature kinds, with their fields, for `help` and for the
/// message about an unknown operation.
pub(crate) const OTHER_OPS: &[(&str, &str, &str)] = &[
    (
        "sketch",
        "on (plane), name, draw (list)",
        "Add a sketch on a plane or a flat face, and optionally draw in it.",
    ),
    (
        "draw",
        "sketch, draw (list)",
        "Draw in a sketch: geometry, relations, dimensions and edits.",
    ),
    (
        "set_dimension",
        "sketch, name, value",
        "Change a sketch dimension (\"d1\") to a number or an expression.",
    ),
    (
        "edit",
        "feature, then any of the feature's fields ('on' for a sketch: its plane)",
        "Change fields of a feature.",
    ),
    ("rename", "feature, name", "Rename a feature."),
    (
        "suppress",
        "feature, on (default true)",
        "Suppress a feature (skip it when rebuilding), or unsuppress it.",
    ),
    (
        "show",
        "feature, on (default true)",
        "Show or hide a feature's own geometry (a sketch, a plane).",
    ),
    ("delete", "feature, or features (list)", "Delete features."),
    (
        "move",
        "feature, and before or after (a feature) or index",
        "Move a feature in the tree.",
    ),
    (
        "rollback",
        "to (a feature, or \"end\")",
        "Put the rollback bar after a feature: later features are not built, and new ones are added there.",
    ),
    (
        "set_parameter",
        "name, value",
        "Add or change a named value usable in every expression.",
    ),
    ("delete_parameter", "name", "Remove a named value."),
    (
        "set_units",
        "length (mm, cm, m, in, ft)",
        "Change the document's length unit.",
    ),
    ("undo", "", "Undo the last change."),
    ("redo", "", "Redo the last undone change."),
    (
        "status",
        "",
        "The document in a few lines: name, units, counts, failures.",
    ),
    (
        "features",
        "",
        "The feature tree, with each feature's status.",
    ),
    (
        "feature",
        "feature",
        "Everything about one feature: its fields, and a sketch's contents.",
    ),
    ("parameters", "", "The named values and the units."),
    (
        "bodies",
        "",
        "The bodies: bounds, volume, and flat size for sheet metal.",
    ),
    (
        "faces",
        "body",
        "The faces of a body (or all): what made each, its normal and centre.",
    ),
    (
        "edges",
        "body",
        "The edges of a body (or all): their ends and the faces they join.",
    ),
    (
        "bend_table",
        "body",
        "A sheet metal body's flat size and bends.",
    ),
    (
        "checks",
        "body",
        "Manufacturing checks of a sheet metal body.",
    ),
    (
        "mass",
        "body",
        "Mass properties for a density of 1: volume, area, centre of gravity, principal moments of inertia.",
    ),
    (
        "measure",
        "a, b (each {\"face\": selector}, {\"edge\": selector} or {\"vertex\": selector}; b is optional)",
        "The exact size of a face, an edge or a vertex, or the distance and angle between two.",
    ),
    (
        "import_step",
        "path",
        "Add the solids of a STEP file as bodies, in one feature named after the file.",
    ),
    (
        "show",
        "datum (origin, front, top, right, planes), on (default true)",
        "Show or hide the origin or the standard planes.",
    ),
    (
        "flat_pattern",
        "on (left out: the other way)",
        "Show sheet metal bodies as their flat patterns, or folded.",
    ),
    (
        "import_dxf",
        "path, and sketch (an existing one) or on (a plane: a new sketch; default top) with name; unit (mm, cm, m, in, ft), placement (keep, centred, lower_left)",
        "Bring the lines, arcs, circles and polylines of a DXF file into a sketch.",
    ),
    (
        "materials",
        "material",
        "The material and gauge tables: thickness, bend radius and bend model by material and gauge.",
    ),
    (
        "apply_material",
        "material, gauge or thickness (the nearest gauge), feature (a base flange; default the first)",
        "Set a sheet metal body's thickness, bend radius and bend model from the material tables.",
    ),
    (
        "set_gauge",
        "material, gauge, thickness, radius, bend ({k_factor} | {allowance} | {deduction}), notes",
        "Add a row to a material's gauge table, or change one.",
    ),
    (
        "delete_gauge",
        "material, gauge (left out: the whole table)",
        "Remove a gauge from a material's table.",
    ),
    (
        "import_materials",
        "path",
        "Replace the material tables with those of a CSV file.",
    ),
    (
        "export_materials",
        "path",
        "Write the material tables as CSV.",
    ),
    (
        "set_check_rule",
        "rule (min_flange, hole_to_bend, hole_to_edge, hole_to_hole, min_hole, collision), thickness, radius, constant",
        "Change a limit of the manufacturing checks: multiples of the thickness and of the bend radius, plus a constant in mm.",
    ),
    (
        "new",
        "discard (default false)",
        "Start a new, empty part. Refused if there are unsaved changes, unless discard is true.",
    ),
    (
        "open",
        "path, discard (default false)",
        "Open a part from a .peet file.",
    ),
    (
        "open_sample",
        "sample (bracket, enclosure, chassis, housing), discard (default false)",
        "Open one of the sample parts.",
    ),
    (
        "save",
        "path, caches (default true)",
        "Save the part (to the file it came from if no path is given).",
    ),
    (
        "export",
        "path, format (stl, dxf, step), body, schema (ap214, ap242)",
        "Export the bodies as STL or STEP, or a sheet metal body's flat pattern as DXF.",
    ),
];

fn read(op: &str, a: &mut Args, doc: &Document) -> Result<Op, String> {
    Ok(match op {
        "help" => Op::Query(Query::Help),
        "status" => Op::Query(Query::Status),
        "features" => Op::Query(Query::Features),
        "feature" => Op::Query(Query::Feature(feature(a, "feature")?)),
        "parameters" => Op::Query(Query::Parameters),
        "bodies" => Op::Query(Query::Bodies),
        "faces" => Op::Query(Query::Faces { body: body(a)? }),
        "edges" => Op::Query(Query::Edges { body: body(a)? }),
        "bend_table" => Op::Query(Query::BendTable { body: body(a)? }),
        "checks" => Op::Query(Query::Checks { body: body(a)? }),
        "materials" => Op::Query(Query::Materials {
            material: a.string("material")?,
        }),
        "new" => Op::New {
            discard: a.flag("discard", false)?,
        },
        "open" => Op::Open {
            path: path(a, "open")?,
            discard: a.flag("discard", false)?,
        },
        "open_sample" => Op::OpenSample {
            sample: a.required("sample", Sample::parse)?,
            discard: a.flag("discard", false)?,
        },
        "flat_pattern" => Op::FlatPattern {
            on: a.parsed("on", boolean)?,
        },
        "import_dxf" => {
            let file = path(a, "import_dxf")?;
            let into = match a.parsed("sketch", FeatureSel::parse)? {
                Some(sketch) => {
                    if a.has("on") || a.has("name") {
                        return Err(
                            "Give 'sketch' (an existing one), or 'on' and 'name' (a new one), not both."
                                .to_owned(),
                        );
                    }
                    DxfTarget::Sketch(sketch)
                }
                None => DxfTarget::New {
                    on: a
                        .parsed("on", PlaneSel::parse)?
                        .unwrap_or(PlaneSel::Standard(peet_model::StdPlane::Top)),
                    name: a.string("name")?,
                },
            };
            Op::ImportDxf {
                path: file,
                into,
                unit: a.parsed("unit", units)?,
                placement: a
                    .parsed("placement", DxfPlacement::parse)?
                    .unwrap_or(DxfPlacement::Keep),
            }
        }
        "apply_material" => Op::ApplyMaterial {
            material: a.required("material", |v| text(v).map(str::to_owned))?,
            gauge: a.string("gauge")?,
            thickness: a.parsed("thickness", number)?,
            feature: a.parsed("feature", FeatureSel::parse)?,
        },
        "set_gauge" => Op::SetGauge(Gauge::parse(a)?),
        "delete_gauge" => Op::DeleteGauge {
            material: a.required("material", |v| text(v).map(str::to_owned))?,
            gauge: a.string("gauge")?,
        },
        "import_materials" => Op::ImportMaterials {
            path: path(a, "import_materials")?,
        },
        "export_materials" => Op::ExportMaterials {
            path: path(a, "export_materials")?,
        },
        "set_check_rule" => Op::SetCheckRule {
            rule: a.required("rule", CheckRule::parse)?,
            thickness: a.parsed("thickness", number)?,
            radius: a.parsed("radius", number)?,
            constant: a.parsed("constant", number)?,
        },
        "mass" => Op::Query(Query::Mass { body: body(a)? }),
        "measure" => Op::Query(Query::Measure {
            a: Box::new(a.required("a", GeomSel::parse)?),
            b: a.parsed("b", GeomSel::parse)?.map(Box::new),
        }),
        "import_step" => Op::ImportStep {
            path: a
                .string("path")?
                .map(PathBuf::from)
                .ok_or_else(|| "'import_step' needs a 'path' field.".to_owned())?,
        },
        "undo" => Op::Undo,
        "redo" => Op::Redo,
        "save" => Op::Save {
            path: a.string("path")?.map(PathBuf::from),
            caches: a.flag("caches", true)?,
        },
        "export" => Op::Export {
            path: a
                .string("path")?
                .map(PathBuf::from)
                .ok_or_else(|| "'export' needs a 'path' field.".to_owned())?,
            format: a.parsed("format", |v| match text(v)?.to_lowercase().as_str() {
                "stl" => Ok(Format::Stl),
                "dxf" => Ok(Format::Dxf),
                "step" | "stp" => Ok(Format::Step),
                other => Err(format!("'{other}' is not stl, dxf or step")),
            })?,
            body: body(a)?,
            schema: a
                .parsed("schema", |v| match text(v)? {
                    "ap214" => Ok(StepSchema::Ap214),
                    "ap242" => Ok(StepSchema::Ap242),
                    other => Err(format!("'{other}' is not ap214 or ap242")),
                })?
                .unwrap_or_default(),
        },
        "sketch" => Op::Sketch {
            on: a.required("on", PlaneSel::parse)?,
            name: a.string("name")?,
            draw: a.parsed("draw", sketch::parse)?.unwrap_or_default(),
        },
        "draw" => Op::Draw {
            sketch: feature(a, "sketch")?,
            draw: a.required("draw", sketch::parse)?,
        },
        "set_dimension" => Op::SetDimension {
            sketch: feature(a, "sketch")?,
            name: a.required("name", |v| text(v).map(str::to_owned))?,
            value: a.required("value", Input::parse)?,
        },
        "edit" => {
            let target = feature(a, "feature")?;
            let id = target.resolve(doc)?;
            let kind = doc
                .feature(id)
                .map(|f| &f.kind)
                .ok_or_else(|| "The feature no longer exists.".to_owned())?;
            Op::Edit {
                feature: target,
                fields: FeatureArgs::parse_for(kind, a)?,
            }
        }
        "rename" => Op::Rename {
            feature: feature(a, "feature")?,
            name: a.required("name", |v| text(v).map(str::to_owned))?,
        },
        "suppress" => Op::Suppress {
            feature: feature(a, "feature")?,
            on: a.flag("on", true)?,
        },
        "show" if a.has("datum") => Op::ShowDatum {
            datum: a.required("datum", DatumSel::parse)?,
            on: a.flag("on", true)?,
        },
        "show" => Op::Show {
            feature: feature(a, "feature")?,
            on: a.flag("on", true)?,
        },
        "delete" => {
            let mut features = Vec::new();
            features.extend(a.parsed("feature", FeatureSel::parse)?);
            features.extend(
                a.parsed("features", |v| {
                    list(v)?
                        .iter()
                        .map(FeatureSel::parse)
                        .collect::<Result<Vec<_>, _>>()
                })?
                .unwrap_or_default(),
            );
            if features.is_empty() {
                return Err("'delete' needs a 'feature' or 'features' field.".to_owned());
            }
            Op::Delete { features }
        }
        "move" => {
            let target = feature(a, "feature")?;
            let to = if let Some(t) = a.parsed("before", FeatureSel::parse)? {
                Place::Before(t)
            } else if let Some(t) = a.parsed("after", FeatureSel::parse)? {
                Place::After(t)
            } else if let Some(i) = a.parsed("index", integer)? {
                Place::Index(i as usize)
            } else {
                return Err("'move' needs a 'before', 'after' or 'index' field.".to_owned());
            };
            Op::Move {
                feature: target,
                to,
            }
        }
        "rollback" => Op::Rollback {
            to: a.required("to", |v| {
                if v.as_str() == Some("end") {
                    Ok(None)
                } else {
                    FeatureSel::parse(v).map(Some)
                }
            })?,
        },
        "set_parameter" => Op::SetParameter {
            name: a.required("name", |v| text(v).map(str::to_owned))?,
            value: a.required("value", Input::parse)?,
        },
        "delete_parameter" => Op::DeleteParameter {
            name: a.required("name", |v| text(v).map(str::to_owned))?,
        },
        "set_units" => Op::SetUnits {
            length: a.required("length", units)?,
        },
        _ if FeatureArgs::adds(op) => Op::Add(new_features(op, a)?),
        _ if let Some(command) = AppCommand::parse(op, a) => Op::App(command?),
        other => {
            let mut all: Vec<&str> = OTHER_OPS
                .iter()
                .map(|(n, ..)| *n)
                .chain(
                    FeatureArgs::OPS
                        .iter()
                        .filter_map(|(w, ..)| w.split('.').next())
                        .filter(|w| FeatureArgs::adds(w)),
                )
                .collect();
            all.push("help");
            all.extend([
                "view",
                "zoom_to_fit",
                "toggle",
                "window",
                "edit_sketch",
                "exit_sketch",
            ]);
            all.extend(["tool", "quit"]);
            all.sort_unstable();
            all.dedup();
            return Err(format!(
                "There is no operation '{other}'. Operations are: {}.",
                all.join(", ")
            ));
        }
    })
}

/// Reads a JSON operation. Returns its name (empty if it has none) with the operation, or
/// why it can't be read. The document is needed to know what kind of feature an `edit`
/// is about.
pub(crate) fn from_json(op: &Value, doc: &Document) -> (String, Result<Op, String>) {
    let Value::Object(map) = op else {
        return (
            String::new(),
            Err(format!(
                "An operation is an object with an 'op' field, not {op}."
            )),
        );
    };
    let mut map: Map<String, Value> = map.clone();
    let name = match map.remove("op") {
        Some(Value::String(n)) => n,
        _ => {
            return (
                String::new(),
                Err("The operation has no 'op' field saying what to do.".to_owned()),
            );
        }
    };
    let mut a = Args::new(&name, map);
    let result = read(&name, &mut a, doc).and_then(|op| a.check().map(|()| op));
    (name, result)
}
