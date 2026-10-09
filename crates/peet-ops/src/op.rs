//! The operations, as a type: what [`crate::apply`] takes, and how a JSON operation is
//! read into one.

use std::path::PathBuf;
use std::sync::Arc;

use peet_document::Document;
use peet_io::step::StepSchema;
use peet_sketch::expr::LengthUnit;
use serde_json::{Map, Value};

use crate::args::{Args, boolean, integer, list, number, text};
use crate::assembly::{CompSel, ComponentChange, InsertSource, Placing, Point3};
use crate::fields::FeatureArgs;
use crate::host::{AppCommand, word_enum};
use crate::library::{CheckRule, Gauge};
use crate::mate::{MateChange, MateEndSel, MateSel, MateType};
use crate::session::SessionCommand;
use crate::sketch::{self, DrawItem};
use crate::value::{Configs, EdgeSel, FeatureSel, GeomSel, Input, PlaneSel};

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
        /// A flat sheet metal cover for the chassis, with holes for the housing.
        Cover = "cover",
        /// An M8 socket head screw, for the housing's counterbores.
        Bolt = "bolt",
        /// An M4 socket head screw, for the chassis's mounting holes.
        Screw = "screw",
        /// An assembly: the chassis, the cover, the housing and their screws, mated.
        Assembly = "assembly",
    }
}

impl Sample {
    /// The sample, built.
    pub fn model(self) -> peet_model::Model {
        match self {
            Self::Bracket => peet_model::samples::bracket().0,
            Self::Enclosure => peet_model::samples::enclosure().0,
            Self::Chassis => peet_model::samples::chassis().0,
            Self::Housing => peet_model::samples::housing().0,
            Self::Cover => peet_model::samples::cover().0,
            Self::Bolt => {
                use peet_model::samples::{enclosure_size, socket_screw};
                socket_screw("Bolt M8", enclosure_size::M8).0
            }
            Self::Screw => {
                use peet_model::samples::{enclosure_size, socket_screw};
                socket_screw("Screw M4", enclosure_size::M4).0
            }
            Self::Assembly => peet_model::samples::enclosure_assembly().0,
        }
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

/// A file to read: where it is, or what was already read from it (the application's file
/// dialogs, and the browser, hand over the contents).
#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    /// The file's name, for messages and for naming what comes out of it.
    pub name: String,
    /// Where it is, if it is on disk.
    pub path: Option<PathBuf>,
    /// Its contents, if they have been read already.
    pub bytes: Option<Arc<Vec<u8>>>,
}

impl Source {
    /// A file on disk, read when the operation is applied.
    pub fn path(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        Self {
            name: path.file_name().map_or_else(
                || path.to_string_lossy().into_owned(),
                |n| n.to_string_lossy().into_owned(),
            ),
            path: Some(path),
            bytes: None,
        }
    }

    /// A file whose contents have been read already.
    pub fn loaded(name: impl Into<String>, path: Option<PathBuf>, bytes: Vec<u8>) -> Self {
        Self {
            name: name.into(),
            path,
            bytes: Some(Arc::new(bytes)),
        }
    }

    /// The file for messages: its path, or its name.
    pub(crate) fn shown(&self) -> String {
        self.path
            .as_ref()
            .map_or_else(|| self.name.clone(), |p| p.display().to_string())
    }

    pub(crate) fn read(&self) -> Result<Arc<Vec<u8>>, String> {
        match (&self.bytes, &self.path) {
            (Some(bytes), _) => Ok(bytes.clone()),
            (None, Some(path)) => std::fs::read(path)
                .map(Arc::new)
                .map_err(|e| format!("Couldn't read {}: {e}", path.display())),
            (None, None) => Err(format!("There is nothing to read {} from.", self.name)),
        }
    }
}

impl From<PathBuf> for Source {
    fn from(path: PathBuf) -> Self {
        Self::path(path)
    }
}

/// Where the rollback bar goes.
#[derive(Clone, Debug, PartialEq)]
pub enum RollTo {
    /// Below every feature: everything is built.
    End,
    /// Above every feature: nothing is built.
    Start,
    /// Below this feature: it is the last one built.
    After(FeatureSel),
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
    /// The configurations, which one is active, and what differs between them.
    Configurations,
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
    /// Mass properties: volume, area, centre of gravity, principal moments of inertia
    /// (for a density of 1), and the mass if the part has a material. Of one body, or of
    /// all (each, and together).
    Mass {
        body: Option<usize>,
    },
    /// The exact measurements of a face, an edge or a vertex; with `b`, the distance and
    /// the angle between the two.
    Measure {
        a: Box<GeomSel>,
        b: Option<Box<GeomSel>>,
    },
    /// The components of an assembly, and its parts.
    Components,
    /// The mates of an assembly, and the freedom they leave.
    Mates,
    /// The steps of an assembly's exploded view.
    ExplodeSteps,
    /// Where the components of an assembly run into each other: every pair, or those
    /// one component is in.
    Interference {
        component: Option<CompSel>,
    },
    /// The bill of materials of an assembly: its parts and how many of each. With
    /// `top_level`, a sub-assembly is one line instead of its parts.
    Bom {
        top_level: bool,
    },
}

/// A file format to export.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Stl,
    /// A sheet metal body's flat pattern.
    Dxf,
    Step,
    /// An assembly's bill of materials.
    Csv,
}

/// One operation on a part. A change is one undo step; a query changes nothing.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// Add features (several as one step: an edge flange on each of several edges).
    Add(Vec<New>),
    /// Change fields of a feature. `fields` must be of the feature's kind.
    ///
    /// Its numeric values can differ between configurations: `configurations` says which
    /// ones the new values are for. Left out, a value that already differs changes in
    /// the active configuration and one that doesn't changes in all of them. The other
    /// fields are the same in every configuration.
    Edit {
        feature: FeatureSel,
        fields: FeatureArgs,
        configurations: Option<Configs>,
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
    /// Change a sketch dimension (`"d1"`), in the configurations given (as for
    /// [`Op::Edit`]).
    SetDimension {
        sketch: FeatureSel,
        name: String,
        value: Input,
        configurations: Option<Configs>,
    },
    Rename {
        feature: FeatureSel,
        name: String,
    },
    /// Suppress a feature (skip it when rebuilding, with everything built on it), or
    /// unsuppress it, in some configurations.
    Suppress {
        feature: FeatureSel,
        on: bool,
        configurations: Configs,
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
        to: RollTo,
    },
    /// Replace what is drawn in a sketch: how the sketch editor commits its work. A
    /// script draws with `Draw`.
    ///
    /// `configurations` says which ones the dimensions whose values changed are changed
    /// in (as for [`Op::Edit`]); what is drawn is the same in every configuration.
    SetSketch {
        sketch: FeatureSel,
        content: Box<peet_sketch::Sketch>,
        configurations: Option<Configs>,
    },
    /// Add or change a named value usable in every expression. A new one exists in
    /// every configuration; an existing one changes in the configurations given.
    SetParameter {
        name: String,
        value: Input,
        configurations: Configs,
    },
    /// Add a configuration: a copy of another (the active one if absent), which becomes
    /// the active one.
    AddConfiguration {
        name: String,
        copy: Option<String>,
        comment: Option<String>,
    },
    /// Rename a configuration, or change its comment.
    EditConfiguration {
        configuration: String,
        name: Option<String>,
        comment: Option<String>,
    },
    /// Delete a configuration. A part keeps at least one.
    DeleteConfiguration {
        configuration: String,
    },
    /// Make a configuration the active one: the one built, shown and exported. Like the
    /// flat pattern view, it is not an undo step.
    Configuration {
        configuration: String,
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
        file: Source,
    },
    /// Bring the lines, arcs, circles, polylines and fit-point splines of a DXF file into a sketch.
    ImportDxf {
        file: Source,
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
    /// Say what the part is made of: a material of the tables, or any name with a
    /// density in kg/m³. No material clears it.
    SetMaterial {
        material: Option<String>,
        /// The tables' density for the material if absent.
        density: Option<f64>,
    },
    /// The colour the part is drawn in (sRGB), or `None` for the usual one.
    SetColor {
        color: Option<[u8; 3]>,
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
        file: Source,
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
    /// discard them. With `keep`, the part that is open stays open beside the new one
    /// (see [`crate::apply_session`]), and so there is nothing to discard.
    New {
        discard: bool,
        keep: bool,
        /// An assembly, not a part.
        assembly: bool,
    },
    /// Open a part from a `.peet` file: in place of the one that is open, or with `keep`
    /// beside it.
    Open {
        file: Source,
        discard: bool,
        keep: bool,
    },
    OpenSample {
        sample: Sample,
        discard: bool,
        keep: bool,
    },
    /// Something about the documents that are open together: list them, switch, close.
    Session(SessionCommand),
    /// Add a component to an assembly: an instance of a part, which is copied into the
    /// assembly (once, however many instances it has).
    Insert {
        from: InsertSource,
        /// An automatic name (`Bracket-1`) if absent.
        name: Option<String>,
        /// At the assembly's origin, as the part is, if absent.
        placing: Option<Placing>,
        /// The first component of an assembly is fixed, the others are not, if absent.
        fixed: Option<bool>,
        /// Link the part to its file instead of copying it into the assembly: the
        /// assembly then follows the file. Needs a file on disk.
        link: bool,
        /// Keep the link as a full path, not relative to the assembly's folder.
        absolute: bool,
    },
    /// Read the linked parts of an assembly from their files again.
    UpdateLinks,
    /// Link a part of an assembly to a file: to the file if there is one, else the part
    /// is written there first.
    Link {
        part: crate::links::PartSel,
        path: PathBuf,
        /// Keep the link as a full path, not relative to the assembly's folder.
        absolute: bool,
    },
    /// Make a linked part the assembly's own again.
    Unlink {
        part: crate::links::PartSel,
    },
    /// Change a component of an assembly.
    Component {
        component: CompSel,
        change: ComponentChange,
    },
    /// Replace the model of one of an assembly's parts: how a part that was opened from
    /// an assembly and edited is stored back. A script saves that part instead.
    SetPart {
        part: peet_model::DefId,
        model: Arc<peet_model::Model>,
    },
    /// Pull a point of a component of an assembly towards a place, as with the mouse:
    /// it goes as far as its mates let it, and what it is mated to comes along.
    Drag {
        component: CompSel,
        /// In the component's own coordinates. Its origin if absent.
        point: Option<Point3>,
        /// In the assembly's coordinates.
        to: Point3,
    },
    /// Hold two components of an assembly together by geometry of their parts.
    Mate {
        kind: MateType,
        a: MateEndSel,
        b: MateEndSel,
        /// Two flat faces the same way round, instead of against each other.
        flip: Option<bool>,
        /// An automatic name (`Coincident1`) if absent.
        name: Option<String>,
    },
    /// Change a mate of an assembly.
    EditMate {
        mate: MateSel,
        change: MateChange,
    },
    /// Copy components of an assembly in rows or round an axis.
    ComponentPattern {
        /// The originals.
        components: Vec<CompSel>,
        kind: crate::pattern::PatternSpec,
        /// An automatic name (`LPattern1`, `CirPattern1`) if absent.
        name: Option<String>,
    },
    /// Change a component pattern of an assembly.
    EditComponentPattern {
        pattern: crate::pattern::PatternSel,
        change: crate::pattern::PatternChange,
    },
    /// Add a step to an assembly's exploded view: components shown moved by a distance.
    ExplodeStep {
        components: Vec<CompSel>,
        /// How far, in the assembly's directions.
        by: crate::assembly::Point3,
        /// An automatic name (`Explode1`) if absent.
        name: Option<String>,
    },
    /// Change a step of an assembly's exploded view.
    EditExplodeStep {
        step: crate::explode::ExplodeSel,
        change: crate::explode::ExplodeChange,
    },
    /// Show an assembly exploded, or as it is (`None`: the other way). A view.
    Explode {
        on: Option<bool>,
    },
    /// Show every hidden component of an assembly.
    ShowAll,
    /// Show these components of an assembly and hide every other.
    Isolate {
        components: Vec<CompSel>,
    },
    /// Open a component's part as a document of its own (see
    /// [`crate::apply_session`]): saving that document stores it back.
    OpenComponent {
        component: CompSel,
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
            Self::SetSketch { .. } => "set_sketch",
            Self::SetParameter { .. } => "set_parameter",
            Self::AddConfiguration { .. } => "add_configuration",
            Self::EditConfiguration { .. } => "edit_configuration",
            Self::DeleteConfiguration { .. } => "delete_configuration",
            Self::Configuration { .. } => "configuration",
            Self::DeleteParameter { .. } => "delete_parameter",
            Self::SetUnits { .. } => "set_units",
            Self::ImportStep { .. } => "import_step",
            Self::ImportDxf { .. } => "import_dxf",
            Self::ShowDatum { .. } => "show",
            Self::FlatPattern { .. } => "flat_pattern",
            Self::ApplyMaterial { .. } => "apply_material",
            Self::SetMaterial { .. } => "set_material",
            Self::SetColor { .. } => "set_color",
            Self::SetGauge(_) => "set_gauge",
            Self::DeleteGauge { .. } => "delete_gauge",
            Self::ImportMaterials { .. } => "import_materials",
            Self::ExportMaterials { .. } => "export_materials",
            Self::SetCheckRule { .. } => "set_check_rule",
            Self::New { .. } => "new",
            Self::Open { .. } => "open",
            Self::OpenSample { .. } => "open_sample",
            Self::App(command) => command.word(),
            Self::Session(command) => command.word(),
            Self::Insert { .. } => "insert",
            Self::UpdateLinks => "update_links",
            Self::Link { .. } => "link",
            Self::Unlink { .. } => "unlink",
            Self::Component { change, .. } => change.word(),
            Self::Drag { .. } => "drag",
            Self::Mate { .. } => "mate",
            Self::EditMate { change, .. } => change.word(),
            Self::ComponentPattern { .. } => "component_pattern",
            Self::EditComponentPattern { change, .. } => change.word(),
            Self::ExplodeStep { .. } => "explode_step",
            Self::EditExplodeStep { change, .. } => change.word(),
            Self::Explode { .. } => "explode",
            Self::ShowAll => "show_all",
            Self::Isolate { .. } => "isolate",
            Self::SetPart { .. } => "set_part",
            Self::OpenComponent { .. } => "open_component",
            Self::Undo => "undo",
            Self::Redo => "redo",
            Self::Query(q) => match q {
                Query::Help => "help",
                Query::Status => "status",
                Query::Features => "features",
                Query::Feature(_) => "feature",
                Query::Parameters => "parameters",
                Query::Configurations => "configurations",
                Query::Bodies => "bodies",
                Query::Faces { .. } => "faces",
                Query::Edges { .. } => "edges",
                Query::BendTable { .. } => "bend_table",
                Query::Checks { .. } => "checks",
                Query::Materials { .. } => "materials",
                Query::Mass { .. } => "mass",
                Query::Measure { .. } => "measure",
                Query::Components => "components",
                Query::Mates => "mates",
                Query::ExplodeSteps => "explode_steps",
                Query::Interference { .. } => "interference",
                Query::Bom { .. } => "bom",
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
    // Several features from one name are numbered: Wall1, Wall2, …
    let several = edges.len() > 1;
    Ok(edges
        .into_iter()
        .enumerate()
        .map(|(i, edge)| {
            let mut feature = args.clone();
            match &mut feature {
                FeatureArgs::EdgeFlange(f) => f.edge = Some(Some(edge)),
                FeatureArgs::Hem(h) => h.edge = Some(Some(edge)),
                _ => {}
            }
            New {
                name: name.as_ref().map(|n| {
                    if several {
                        format!("{n}{}", i + 1)
                    } else {
                        n.clone()
                    }
                }),
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
        "sketch, name, value, configurations (\"this\", \"all\", a name or a list of names; left out: this one if the dimension already differs between configurations, else all)",
        "Change a sketch dimension (\"d1\") to a number or an expression, in some configurations.",
    ),
    (
        "edit",
        "feature, then any of the feature's fields ('on' for a sketch: its plane), configurations (\"this\", \"all\", a name or a list of names: for numeric fields only; left out: this one for a value that already differs between configurations, else all)",
        "Change fields of a feature. Its numeric values can differ between configurations.",
    ),
    ("rename", "feature, name", "Rename a feature."),
    (
        "suppress",
        "feature, on (default true), configurations (\"this\" (default), \"all\", a name or a list of names)",
        "Suppress a feature (skip it when rebuilding, with everything built on it), or unsuppress it, in some configurations.",
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
        "to (a feature, \"end\" or \"start\")",
        "Put the rollback bar after a feature: later features are not built, and new ones are added there.",
    ),
    (
        "set_parameter",
        "name, value, configurations (\"this\" (default), \"all\", a name or a list of names)",
        "Add or change a named value usable in every expression. A new one exists in every configuration.",
    ),
    (
        "add_configuration",
        "name, copy (a configuration; default the active one), comment",
        "Add a configuration, which starts as a copy and becomes the active one.",
    ),
    (
        "edit_configuration",
        "configuration, name, comment",
        "Rename a configuration, or change its comment.",
    ),
    (
        "delete_configuration",
        "configuration",
        "Delete a configuration. A part keeps at least one.",
    ),
    (
        "configuration",
        "configuration",
        "Make a configuration the active one: the one built, shown and exported. Not an undo step.",
    ),
    (
        "configurations",
        "",
        "The configurations, which one is active, and what differs between them.",
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
        "Mass properties: volume, area, centre of gravity, principal moments of inertia (for a density of 1), and the mass if the part has a material. Of an assembly: every component and the whole, each part weighed with its own material.",
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
        "Bring the lines, arcs, circles, polylines and fit-point splines of a DXF file into a sketch.
",
    ),
    (
        "materials",
        "material",
        "The material and gauge tables: thickness, bend radius and bend model by material and gauge.",
    ),
    (
        "apply_material",
        "material, gauge or thickness (the nearest gauge), feature (a base flange; default the first)",
        "Set a sheet metal body's thickness, bend radius and bend model from the material tables, and the part's material.",
    ),
    (
        "set_material",
        "material (a name, or null for none), density (kg/m³; default the material tables')",
        "Say what the part is made of: its name in a bill of materials and the density its mass is worked out with.",
    ),
    (
        "set_color",
        "color (\"#rrggbb\" or [r, g, b], or null for the usual one)",
        "The colour the part is drawn in.",
    ),
    (
        "set_gauge",
        "material, gauge, thickness, radius, bend ({k_factor} | {allowance} | {deduction}), notes, density (kg/m³, of the material)",
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
        "discard (default false), keep (default false), assembly (default false)",
        "Start a new, empty part, or with assembly an empty assembly. Refused if there are unsaved changes, unless discard is true. With keep, the document that is open stays open beside the new one.",
    ),
    (
        "open",
        "path, discard (default false), keep (default false)",
        "Open a part from a .peet file: in place of the one that is open, or with keep beside it.",
    ),
    (
        "open_sample",
        "sample (bracket, enclosure, chassis, housing, cover, bolt, screw; assembly: an assembly of the last five), discard (default false), keep (default false)",
        "Open one of the sample parts: in place of the one that is open, or with keep beside it.",
    ),
    (
        "save",
        "path, caches (default true)",
        "Save the part (to the file it came from if no path is given).",
    ),
    (
        "export",
        "path, format (stl, dxf, step, csv), body, schema (ap214, ap242)",
        "Export the bodies as STL or STEP, a sheet metal body's flat pattern as DXF, or an assembly's bill of materials as CSV.",
    ),
];

fn read(op: &str, a: &mut Args, doc: &Document) -> Result<Op, String> {
    if let Some(found) = crate::pattern::parse(op, a) {
        return found;
    }
    if let Some(found) = crate::explode::parse(op, a) {
        return found;
    }
    if let Some(found) = crate::mate::parse(op, a) {
        return found;
    }
    if let Some(found) = crate::assembly::parse(op, a) {
        return found;
    }
    Ok(match op {
        "help" => Op::Query(Query::Help),
        "status" => Op::Query(Query::Status),
        "features" => Op::Query(Query::Features),
        "feature" => Op::Query(Query::Feature(feature(a, "feature")?)),
        "parameters" => Op::Query(Query::Parameters),
        "configurations" => Op::Query(Query::Configurations),
        "add_configuration" => Op::AddConfiguration {
            name: a.required("name", |v| text(v).map(str::to_owned))?,
            copy: a.string("copy")?,
            comment: a.string("comment")?,
        },
        "edit_configuration" => Op::EditConfiguration {
            configuration: a.required("configuration", |v| text(v).map(str::to_owned))?,
            name: a.string("name")?,
            comment: a.string("comment")?,
        },
        "delete_configuration" => Op::DeleteConfiguration {
            configuration: a.required("configuration", |v| text(v).map(str::to_owned))?,
        },
        "configuration" => Op::Configuration {
            configuration: a.required("configuration", |v| text(v).map(str::to_owned))?,
        },
        "bodies" => Op::Query(Query::Bodies),
        "faces" => Op::Query(Query::Faces { body: body(a)? }),
        "edges" => Op::Query(Query::Edges { body: body(a)? }),
        "bend_table" => Op::Query(Query::BendTable { body: body(a)? }),
        "checks" => Op::Query(Query::Checks { body: body(a)? }),
        "materials" => Op::Query(Query::Materials {
            material: a.string("material")?,
        }),
        "update_links" => Op::UpdateLinks,
        "link" => Op::Link {
            part: a.required("part", crate::links::PartSel::parse)?,
            path: path(a, "link")?,
            absolute: a.flag("absolute", false)?,
        },
        "unlink" => Op::Unlink {
            part: a.required("part", crate::links::PartSel::parse)?,
        },
        "new" => Op::New {
            discard: a.flag("discard", false)?,
            keep: a.flag("keep", false)?,
            assembly: a.flag("assembly", false)?,
        },
        "open" => Op::Open {
            file: Source::path(path(a, "open")?),
            discard: a.flag("discard", false)?,
            keep: a.flag("keep", false)?,
        },
        "open_sample" => Op::OpenSample {
            sample: a.required("sample", Sample::parse)?,
            discard: a.flag("discard", false)?,
            keep: a.flag("keep", false)?,
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
                file: Source::path(file),
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
        "set_material" => {
            let material = match a.take_nullable("material") {
                None => {
                    return Err(
                        "'set_material' needs a 'material' field (null for no material)."
                            .to_owned(),
                    );
                }
                Some(Value::Null) => None,
                Some(v) => Some(text(&v).map_err(|e| format!("material: {e}"))?.to_owned()),
            };
            Op::SetMaterial {
                material,
                density: a.parsed("density", number)?,
            }
        }
        "set_color" => Op::SetColor {
            color: match a.take_nullable("color") {
                None => {
                    return Err(
                        "'set_color' needs a 'color' field (null for the usual colour).".to_owned(),
                    );
                }
                Some(Value::Null) => None,
                Some(v) => Some(crate::library::color(&v).map_err(|e| format!("color: {e}"))?),
            },
        },
        "set_gauge" => Op::SetGauge(Gauge::parse(a)?),
        "delete_gauge" => Op::DeleteGauge {
            material: a.required("material", |v| text(v).map(str::to_owned))?,
            gauge: a.string("gauge")?,
        },
        "import_materials" => Op::ImportMaterials {
            file: Source::path(path(a, "import_materials")?),
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
            file: Source::path(path(a, "import_step")?),
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
                "csv" => Ok(Format::Csv),
                other => Err(format!("'{other}' is not stl, dxf, step or csv")),
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
            configurations: a.parsed("configurations", Configs::parse)?,
        },
        "edit" => {
            let target = feature(a, "feature")?;
            let configurations = a.parsed("configurations", Configs::parse)?;
            let id = target.resolve(doc)?;
            let kind = doc
                .feature(id)
                .map(|f| &f.kind)
                .ok_or_else(|| "The feature no longer exists.".to_owned())?;
            Op::Edit {
                feature: target,
                fields: FeatureArgs::parse_for(kind, a)?,
                configurations,
            }
        }
        "rename" => Op::Rename {
            feature: feature(a, "feature")?,
            name: a.required("name", |v| text(v).map(str::to_owned))?,
        },
        "suppress" => Op::Suppress {
            feature: feature(a, "feature")?,
            on: a.flag("on", true)?,
            configurations: a
                .parsed("configurations", Configs::parse)?
                .unwrap_or_default(),
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
            to: a.required("to", |v| match v.as_str() {
                Some("end") => Ok(RollTo::End),
                Some("start") => Ok(RollTo::Start),
                _ => FeatureSel::parse(v).map(RollTo::After),
            })?,
        },
        "set_parameter" => Op::SetParameter {
            name: a.required("name", |v| text(v).map(str::to_owned))?,
            value: a.required("value", Input::parse)?,
            configurations: a
                .parsed("configurations", Configs::parse)?
                .unwrap_or_default(),
        },
        "delete_parameter" => Op::DeleteParameter {
            name: a.required("name", |v| text(v).map(str::to_owned))?,
        },
        "set_units" => Op::SetUnits {
            length: a.required("length", units)?,
        },
        _ if FeatureArgs::adds(op) => Op::Add(new_features(op, a)?),
        _ if let Some(command) = AppCommand::parse(op, a) => Op::App(command?),
        _ if let Some(command) = SessionCommand::parse(op, a) => Op::Session(command?),
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
            all.extend(crate::session::SESSION_OPS.iter().map(|(n, ..)| *n));
            all.extend(crate::assembly::ASSEMBLY_OPS.iter().map(|(n, ..)| *n));
            all.extend(crate::mate::MATE_OPS.iter().map(|(n, ..)| *n));
            all.extend(crate::links::LINK_OPS.iter().map(|(n, ..)| *n));
            all.extend(crate::explode::EXPLODE_OPS.iter().map(|(n, ..)| *n));
            all.extend(crate::pattern::PATTERN_OPS.iter().map(|(n, ..)| *n));
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
