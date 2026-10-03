//! DXF import: the 2D geometry of a DXF file as sketch lines, arcs and circles.
//!
//! Used for "new sketch from DXF" ([`import`]) and "import DXF into the open sketch"
//! ([`import_into`]), so existing profiles can become sketches and base flanges.
//!
//! **Reading.** ASCII DXF of any version (R12 `AC1009` up to current AutoCAD releases) is
//! a list of group code / value pairs, one per line. Lines may end in CRLF or LF and codes
//! may have leading spaces. The file is split into sections; only three matter here:
//! `HEADER` for `$INSUNITS`, `BLOCKS` for block definitions and `ENTITIES` for the
//! drawing. Everything else (classes, tables, objects) is skipped, so the reader does not
//! depend on handles, subclass markers or the version. Binary DXF is detected and refused
//! with a message saying how to get an ASCII file. Malformed input never panics: a broken
//! pair structure or a number that doesn't parse is an [`ImportError`] naming the line,
//! and geometry that makes no sense (zero size, coordinates that aren't finite or are
//! absurdly far away) is skipped and counted.
//!
//! **Entities.** `LINE`, `ARC`, `CIRCLE`, `LWPOLYLINE` and 2D `POLYLINE`s (segments with
//! bulges become arcs) are imported. An `ELLIPSE` is imported when it is a circle or a
//! circular arc. A `SPLINE` saved with fit points becomes a sketch spline through those
//! points: the sketch draws its own curve through them, which can differ a little from
//! the original between the points (the report says so). A spline with control points
//! only is skipped (approximating it by many short lines would leave a profile that is
//! hard to edit), as are ellipses that aren't circles and 3D polylines and meshes. Text,
//! dimensions, hatches and every other
//! kind are ignored. Every skipped entity is counted by kind in the [`ImportReport`], with
//! a plain warning for each reason.
//!
//! **Blocks.** An `INSERT` is expanded with its translation, rotation and scale (and
//! `MINSERT`-style column and row arrays). The X and Y scales must be equal in size; a
//! negative one mirrors. Inserts with different X and Y scales are skipped, since arcs
//! would turn into ellipses. Blocks nest up to [`MAX_BLOCK_DEPTH`] levels, and a block
//! that inserts itself is skipped. Entities on layer `0` inside a block take the layer of
//! the insert, as in AutoCAD.
//!
//! **Coordinates.** Arcs, circles, polylines and inserts are given in their object
//! coordinate system (OCS), set by the extrusion direction (group codes 210/220/230). An
//! extrusion of (0, 0, 1) is the XY plane itself; (0, 0, −1), which mirrored geometry
//! often has, is the XY plane seen from below, so X is mirrored. Entities in any other
//! plane are skipped. Z coordinates are dropped: the drawing is seen from the top.
//!
//! **Units.** `$INSUNITS` gives the drawing unit; coordinates are scaled to millimetres.
//! When the file doesn't say (no `$INSUNITS`, or "unitless"), millimetres are assumed and
//! the report says so. The caller can override the unit.
//!
//! **Layers.** The report lists every layer with its entity count. The caller can skip
//! layers, and import others as construction geometry: for a flat pattern exported by
//! PeetCAD ([`crate::dxf`]), [`ImportOptions::flat_pattern`] skips the bend notes and makes
//! the bend lines construction, so the outline and cutouts still form one region.
//!
//! **Building the sketch.** Curve ends closer than a tolerance (10⁻⁶ of the drawing's
//! size, at least 10⁻⁹ mm) are merged: they are moved onto one position and joined by
//! coincident relations, which is how the sketcher connects curves. Arc centres are
//! nudged so both ends stay on the arc's circle after merging. No other relations or
//! dimensions are added: the import is plain geometry, free to drag, for the user to
//! constrain. A closed outline therefore forms regions for a base flange
//! ([`peet_sketch::region::find_regions`]), and an open chain stays open. Imported
//! curves don't connect to geometry already in the sketch.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::f64::consts::{PI, TAU};

use peet_math::DVec2;
use peet_sketch::{ConstraintKind, Curve, EntityId, Sketch};

use crate::dxf::layer;

/// How deep block inserts may nest.
pub const MAX_BLOCK_DEPTH: usize = 32;

/// Most curves an import may produce: more is not a usable sketch.
pub const MAX_CURVES: usize = 100_000;

/// Most entities the reader will visit while expanding blocks (a guard against block
/// definitions that multiply out to astronomical counts).
const MAX_WORK: usize = 2_000_000;

/// Coordinates further than this from the origin (mm) are treated as broken.
const MAX_COORD: f64 = 1e9;

/// A length unit from `$INSUNITS`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Unit {
    Millimetres,
    Centimetres,
    Decimetres,
    Metres,
    Kilometres,
    Micrometres,
    Nanometres,
    Angstroms,
    Inches,
    Feet,
    UsSurveyFeet,
    Yards,
    Miles,
    Mils,
    Microinches,
}

impl Unit {
    /// The unit for an `$INSUNITS` code; `None` for unitless (0) and codes with no
    /// sensible length (astronomical units and the like).
    pub fn from_insunits(code: i64) -> Option<Self> {
        Some(match code {
            1 => Self::Inches,
            2 => Self::Feet,
            3 => Self::Miles,
            4 => Self::Millimetres,
            5 => Self::Centimetres,
            6 => Self::Metres,
            7 => Self::Kilometres,
            8 => Self::Microinches,
            9 => Self::Mils,
            10 => Self::Yards,
            11 => Self::Angstroms,
            12 => Self::Nanometres,
            13 => Self::Micrometres,
            14 => Self::Decimetres,
            21 => Self::UsSurveyFeet,
            _ => return None,
        })
    }

    /// Millimetres per unit.
    pub fn to_mm(self) -> f64 {
        match self {
            Self::Millimetres => 1.0,
            Self::Centimetres => 10.0,
            Self::Decimetres => 100.0,
            Self::Metres => 1000.0,
            Self::Kilometres => 1e6,
            Self::Micrometres => 1e-3,
            Self::Nanometres => 1e-6,
            Self::Angstroms => 1e-7,
            Self::Inches => 25.4,
            Self::Feet => 304.8,
            Self::UsSurveyFeet => 1_200_000.0 / 3937.0,
            Self::Yards => 914.4,
            Self::Miles => 1_609_344.0,
            Self::Mils => 0.0254,
            Self::Microinches => 25.4e-6,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Millimetres => "millimetres",
            Self::Centimetres => "centimetres",
            Self::Decimetres => "decimetres",
            Self::Metres => "metres",
            Self::Kilometres => "kilometres",
            Self::Micrometres => "micrometres",
            Self::Nanometres => "nanometres",
            Self::Angstroms => "ångströms",
            Self::Inches => "inches",
            Self::Feet => "feet",
            Self::UsSurveyFeet => "US survey feet",
            Self::Yards => "yards",
            Self::Miles => "miles",
            Self::Mils => "mils",
            Self::Microinches => "microinches",
        }
    }
}

/// Where the unit used for an import came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitSource {
    /// `$INSUNITS` in the file.
    File,
    /// The caller's override.
    Caller,
    /// The file doesn't say; millimetres were assumed.
    Assumed,
}

/// Where the imported geometry lands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Placement {
    /// The drawing's own coordinates.
    #[default]
    Keep,
    /// The centre of the drawing's bounding box at the sketch origin.
    Centred,
    /// The lower-left corner of the drawing's bounding box at the sketch origin.
    LowerLeftAtOrigin,
}

/// Choices for an import.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImportOptions {
    /// The drawing unit, overriding `$INSUNITS`.
    pub unit: Option<Unit>,
    /// Layers to leave out (names compare without regard to case, as in AutoCAD).
    pub skip_layers: Vec<String>,
    /// Layers whose curves become construction geometry.
    pub layers_as_construction: Vec<String>,
    pub placement: Placement,
}

impl ImportOptions {
    /// For flat patterns exported by PeetCAD: the bend notes are left out and the bend
    /// lines become construction geometry, so the outline and cutouts form one region.
    pub fn flat_pattern() -> Self {
        Self {
            skip_layers: vec![layer::BEND_NOTES.to_owned(), layer::FORM_NOTES.to_owned()],
            layers_as_construction: vec![layer::BEND.to_owned(), layer::FORMS.to_owned()],
            ..Self::default()
        }
    }
}

/// A layer of the file and how many entities are on it (a block insert counts as one,
/// and its contents count again on their own layers).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayerInfo {
    pub name: String,
    pub entities: usize,
}

/// What an import did.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportReport {
    /// Lines, arcs and circles added to the sketch.
    pub entities_imported: usize,
    /// The ids of those curves, in the order they were added.
    pub added: Vec<EntityId>,
    /// Entities left out, by kind (an entity type such as `TEXT`, or a reason such as
    /// "on a skipped layer"), sorted by kind.
    pub skipped: Vec<(String, usize)>,
    /// Plain-English notes for the user: assumptions and what was left out, and why.
    pub warnings: Vec<String>,
    pub unit_used: Unit,
    pub unit_source: UnitSource,
    /// Every layer with entities on it, sorted by name.
    pub layers: Vec<LayerInfo>,
    /// The translation applied by [`ImportOptions::placement`] (mm).
    pub offset: DVec2,
}

/// A new sketch made from a DXF file.
#[derive(Clone, Debug, PartialEq)]
pub struct Import {
    pub sketch: Sketch,
    pub report: ImportReport,
}

/// Why a file could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportError {
    pub message: String,
}

impl ImportError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ImportError {}

/// Reads a DXF file into a new sketch.
pub fn import(bytes: &[u8], options: &ImportOptions) -> Result<Import, ImportError> {
    let mut sketch = Sketch::new();
    let report = import_into(&mut sketch, bytes, options)?;
    Ok(Import { sketch, report })
}

/// Reads a DXF file and adds its geometry to `sketch`. The sketch is left unchanged when
/// the file can't be read.
pub fn import_into(
    sketch: &mut Sketch,
    bytes: &[u8],
    options: &ImportOptions,
) -> Result<ImportReport, ImportError> {
    let text = decode(bytes)?;
    let pairs = pairs(&text)?;
    let file = sections(&pairs)?;

    let mut warnings = file.warnings.clone();
    let (unit, unit_source) = match (options.unit, file.insunits) {
        (Some(u), _) => (u, UnitSource::Caller),
        (None, Some(code)) => match Unit::from_insunits(code) {
            Some(u) => (u, UnitSource::File),
            None => {
                warnings.push(if code == 0 {
                    "The file says its drawing is unitless ($INSUNITS = 0); millimetres were \
                     assumed."
                        .to_owned()
                } else {
                    format!(
                        "The file's unit ($INSUNITS = {code}) is not a length PeetCAD knows; \
                         millimetres were assumed."
                    )
                });
                (Unit::Millimetres, UnitSource::Assumed)
            }
        },
        (None, None) => {
            warnings.push(
                "The file doesn't say what unit it uses ($INSUNITS); millimetres were assumed."
                    .to_owned(),
            );
            (Unit::Millimetres, UnitSource::Assumed)
        }
    };

    let mut reader = Reader {
        file: &file,
        options,
        items: Vec::new(),
        skipped: BTreeMap::new(),
        layers: BTreeMap::new(),
        work: 0,
        stack: Vec::new(),
    };
    reader.expand(
        &file.entities,
        &Xform::scale(unit.to_mm(), unit.to_mm()),
        None,
        true,
    )?;
    let Reader {
        items,
        mut skipped,
        layers,
        ..
    } = reader;

    let (added, offset, collapsed) = build(sketch, items, options.placement);
    if collapsed > 0 {
        *skipped.entry(DEGENERATE).or_default() += collapsed;
    }
    if let Some(n) = skipped.remove(SPLINE_REDRAWN) {
        warnings.push(format!(
            "{n} spline{} drawn again through {} fit points. Between the points the curve \
             can differ a little from the original: check it against the drawing where that \
             matters.",
            if n == 1 { " was" } else { "s were" },
            if n == 1 { "its" } else { "their" }
        ));
    }
    warnings.extend(skip_warnings(&skipped));
    if added.is_empty() {
        warnings.push("The file has no lines, arcs or circles that could be imported.".to_owned());
    }
    Ok(ImportReport {
        entities_imported: added.len(),
        added,
        skipped: skipped
            .into_iter()
            .map(|(k, n)| (k.to_owned(), n))
            .collect(),
        warnings,
        unit_used: unit,
        unit_source,
        layers: layers.into_values().collect(),
        offset,
    })
}

// ---- Reading the file ----

/// Text of the file: UTF-8 when it is valid UTF-8 (R2007 and later), else each byte as
/// the Latin-1 character (older files are in the Windows code page, which only matters
/// for layer names here).
fn decode(bytes: &[u8]) -> Result<Cow<'_, str>, ImportError> {
    if bytes.starts_with(b"AutoCAD Binary DXF") {
        return Err(ImportError::new(
            "This is a binary DXF file, which PeetCAD can't read. Save it from your CAD \
             program as an ASCII DXF and import that.",
        ));
    }
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    Ok(match std::str::from_utf8(bytes) {
        Ok(s) => Cow::Borrowed(s),
        Err(_) => Cow::Owned(bytes.iter().map(|&b| char::from(b)).collect()),
    })
}

/// A group code and its value. `line` is the value's line number (from 1).
#[derive(Clone, Copy, Debug)]
struct Pair<'a> {
    code: i32,
    value: &'a str,
    line: usize,
}

fn pairs(text: &str) -> Result<Vec<Pair<'_>>, ImportError> {
    let mut lines: Vec<&str> = if text.contains('\n') {
        text.split('\n')
            .map(|l| l.strip_suffix('\r').unwrap_or(l))
            .collect()
    } else {
        // Classic Mac line ends.
        text.split('\r').collect()
    };
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    if lines.is_empty() {
        return Err(ImportError::new("The file is empty."));
    }
    let mut out = Vec::with_capacity(lines.len() / 2);
    let mut i = 0;
    while i < lines.len() {
        let code_line = lines[i];
        let Ok(code) = code_line.trim().parse::<i32>() else {
            return Err(ImportError::new(if i == 0 {
                "This doesn't look like a DXF file: it should start with a group code (a \
                 number) on its first line."
                    .to_owned()
            } else {
                format!(
                    "Line {} should hold a group code (a whole number) but reads {:?}. The \
                     file may be damaged.",
                    i + 1,
                    shorten(code_line)
                )
            }));
        };
        let Some(&value) = lines.get(i + 1) else {
            return Err(ImportError::new(format!(
                "The file ends after group code {code} on line {} without its value. The \
                 file may be cut short.",
                i + 1
            )));
        };
        out.push(Pair {
            code,
            value,
            line: i + 2,
        });
        if code == 0 && value.trim() == "EOF" {
            break;
        }
        i += 2;
    }
    Ok(out)
}

/// A text for a message: trimmed and at most 40 characters.
fn shorten(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() > 40 {
        let mut t: String = s.chars().take(40).collect();
        t.push('…');
        t
    } else {
        s.to_owned()
    }
}

/// One record: an entity (or block start, vertex…) and the pairs that follow its type.
#[derive(Clone, Copy, Debug)]
struct Rec<'a> {
    kind: &'a str,
    pairs: &'a [Pair<'a>],
}

impl<'a> Rec<'a> {
    fn get(&self, code: i32) -> Option<&Pair<'a>> {
        self.pairs.iter().find(|p| p.code == code)
    }

    fn text(&self, code: i32) -> Option<&'a str> {
        self.get(code).map(|p| p.value.trim())
    }

    fn layer(&self) -> &'a str {
        self.text(8).filter(|l| !l.is_empty()).unwrap_or("0")
    }

    /// A real number, or `default` if the code is absent.
    fn num(&self, code: i32, default: f64) -> Result<f64, ImportError> {
        self.get(code).map_or(Ok(default), number)
    }

    /// A whole number (flags, counts), or `default` if the code is absent.
    fn int(&self, code: i32, default: i64) -> Result<i64, ImportError> {
        let v = self.num(code, default as f64)?;
        Ok(v.clamp(i64::MIN as f64, i64::MAX as f64) as i64)
    }

    /// A 2D point from codes `x` and `x + 10`.
    fn point(&self, x: i32) -> Result<DVec2, ImportError> {
        Ok(DVec2::new(self.num(x, 0.0)?, self.num(x + 10, 0.0)?))
    }

    /// The extrusion direction (210/220/230).
    fn extrusion(&self) -> Result<[f64; 3], ImportError> {
        Ok([
            self.num(210, 0.0)?,
            self.num(220, 0.0)?,
            self.num(230, 1.0)?,
        ])
    }
}

fn number(p: &Pair) -> Result<f64, ImportError> {
    match p.value.trim().parse::<f64>() {
        Ok(v) if v.is_finite() => Ok(v),
        _ => Err(ImportError::new(format!(
            "Line {} should hold a number (for group code {}) but reads {:?}. The file may \
             be damaged.",
            p.line,
            p.code,
            shorten(p.value)
        ))),
    }
}

/// Splits pairs into records at each code 0.
fn records<'a>(pairs: &'a [Pair<'a>]) -> Vec<Rec<'a>> {
    let starts: Vec<usize> = (0..pairs.len()).filter(|&i| pairs[i].code == 0).collect();
    starts
        .iter()
        .enumerate()
        .map(|(n, &s)| {
            let end = starts.get(n + 1).copied().unwrap_or(pairs.len());
            Rec {
                kind: pairs[s].value.trim(),
                pairs: &pairs[s + 1..end],
            }
        })
        .collect()
}

struct Block<'a> {
    base: DVec2,
    xref: bool,
    records: Vec<Rec<'a>>,
}

/// The parts of the file the import uses.
struct File<'a> {
    insunits: Option<i64>,
    /// Blocks by upper-case name.
    blocks: HashMap<String, Block<'a>>,
    entities: Vec<Rec<'a>>,
    warnings: Vec<String>,
}

fn sections<'a>(pairs: &'a [Pair<'a>]) -> Result<File<'a>, ImportError> {
    let mut file = File {
        insunits: None,
        blocks: HashMap::new(),
        entities: Vec::new(),
        warnings: Vec::new(),
    };
    let is = |p: &Pair, code: i32, value: &str| p.code == code && p.value.trim() == value;
    let mut found = false;
    let mut cut_short = false;
    let mut i = 0;
    while i < pairs.len() {
        if is(&pairs[i], 0, "EOF") {
            break;
        }
        if !is(&pairs[i], 0, "SECTION") {
            i += 1;
            continue;
        }
        found = true;
        let Some(name) = pairs.get(i + 1).filter(|p| p.code == 2) else {
            return Err(ImportError::new(format!(
                "The section starting on line {} has no name. The file may be damaged.",
                pairs[i].line
            )));
        };
        let start = i + 2;
        let mut end = start;
        while end < pairs.len()
            && !(pairs[end].code == 0
                && matches!(pairs[end].value.trim(), "ENDSEC" | "SECTION" | "EOF"))
        {
            end += 1;
        }
        if !pairs.get(end).is_some_and(|p| is(p, 0, "ENDSEC")) {
            cut_short = true;
        }
        let body = &pairs[start..end];
        match name.value.trim() {
            "HEADER" => file.insunits = header_units(body, &mut file.warnings),
            "BLOCKS" => blocks(body, &mut file)?,
            "ENTITIES" => file.entities.extend(records(body)),
            _ => {}
        }
        i = if pairs.get(end).is_some_and(|p| is(p, 0, "ENDSEC")) {
            end + 1
        } else {
            end
        };
    }
    if !found {
        return Err(ImportError::new(
            "This doesn't look like a DXF file: it has no SECTION.",
        ));
    }
    if cut_short {
        file.warnings.push(
            "A section of the file has no end (ENDSEC): the file may be cut short. What was \
             there was imported."
                .to_owned(),
        );
    }
    Ok(file)
}

/// `$INSUNITS` from the header.
fn header_units(body: &[Pair], warnings: &mut Vec<String>) -> Option<i64> {
    let at = body
        .iter()
        .position(|p| p.code == 9 && p.value.trim() == "$INSUNITS")?;
    let p = body[at + 1..]
        .iter()
        .take_while(|p| p.code != 9)
        .find(|p| p.code == 70)?;
    match number(p) {
        Ok(v) if v.fract() == 0.0 => Some(v as i64),
        _ => {
            warnings.push(format!(
                "The unit in the header (line {}) is not a valid $INSUNITS code; it was \
                 ignored.",
                p.line
            ));
            None
        }
    }
}

fn blocks<'a>(body: &'a [Pair<'a>], file: &mut File<'a>) -> Result<(), ImportError> {
    let recs = records(body);
    let mut i = 0;
    while i < recs.len() {
        let r = recs[i];
        i += 1;
        if r.kind != "BLOCK" {
            continue;
        }
        let mut contents = Vec::new();
        while i < recs.len() && recs[i].kind != "ENDBLK" {
            if recs[i].kind == "BLOCK" {
                break;
            }
            contents.push(recs[i]);
            i += 1;
        }
        let name = r.text(2).or(r.text(3)).unwrap_or("");
        if name.is_empty() {
            continue;
        }
        file.blocks.entry(name.to_uppercase()).or_insert(Block {
            base: r.point(10)?,
            xref: r.int(70, 0)? & 4 != 0,
            records: contents,
        });
    }
    Ok(())
}

// ---- Geometry ----

/// A 2D affine map `p ↦ x·p.x + y·p.y + t`. Only similarities (rotation, uniform scale,
/// mirror, translation) are built, so circles stay circles.
#[derive(Clone, Copy, Debug)]
struct Xform {
    x: DVec2,
    y: DVec2,
    t: DVec2,
}

impl Xform {
    fn scale(sx: f64, sy: f64) -> Self {
        Self {
            x: DVec2::new(sx, 0.0),
            y: DVec2::new(0.0, sy),
            t: DVec2::ZERO,
        }
    }

    fn translate(t: DVec2) -> Self {
        Self {
            t,
            ..Self::scale(1.0, 1.0)
        }
    }

    fn rotate(radians: f64) -> Self {
        let (s, c) = radians.sin_cos();
        Self {
            x: DVec2::new(c, s),
            y: DVec2::new(-s, c),
            t: DVec2::ZERO,
        }
    }

    fn apply(&self, p: DVec2) -> DVec2 {
        self.x * p.x + self.y * p.y + self.t
    }

    /// `self` after `inner`.
    fn then(&self, inner: &Xform) -> Xform {
        Xform {
            x: self.x * inner.x.x + self.y * inner.x.y,
            y: self.x * inner.y.x + self.y * inner.y.y,
            t: self.apply(inner.t),
        }
    }

    fn det(&self) -> f64 {
        self.x.perp_dot(self.y)
    }

    fn scale_factor(&self) -> f64 {
        self.det().abs().sqrt()
    }
}

/// The object coordinate system of an entity with extrusion `n`, as a map to world XY:
/// the identity for +Z, a mirror of X for −Z, `None` for any other plane.
fn ocs(n: [f64; 3]) -> Option<Xform> {
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if len == 0.0 || !len.is_finite() {
        return Some(Xform::scale(1.0, 1.0));
    }
    let (x, y, z) = (n[0] / len, n[1] / len, n[2] / len);
    if x.abs() > 1e-9 || y.abs() > 1e-9 {
        return None;
    }
    // The arbitrary axis algorithm gives OCS X = (−1, 0, 0), Y = (0, 1, 0) for −Z.
    Some(if z > 0.0 {
        Xform::scale(1.0, 1.0)
    } else {
        Xform::scale(-1.0, 1.0)
    })
}

/// A curve read from the file, in world millimetres.
#[derive(Clone, Debug)]
enum Shape {
    /// A spline through fit points (a closed one doesn't repeat its first point).
    Spline {
        through: Vec<DVec2>,
        closed: bool,
    },
    Line(DVec2, DVec2),
    /// Counter-clockwise from `start` to `end` around `center`. `exact` when the ends
    /// were given in the file (polyline vertices) rather than computed from angles.
    Arc {
        center: DVec2,
        start: DVec2,
        end: DVec2,
        sweep: f64,
        exact: bool,
    },
    Circle(DVec2, f64),
}

impl Shape {
    /// The counter-clockwise arc of a circle between two angles (radians).
    fn arc(center: DVec2, radius: f64, start: f64, sweep: f64) -> Self {
        let at = |a: f64| center + DVec2::from_angle(a) * radius;
        if sweep >= TAU - 1e-12 {
            return Self::Circle(center, radius);
        }
        Self::Arc {
            center,
            start: at(start),
            end: at(start + sweep),
            sweep,
            exact: false,
        }
    }

    /// The segment of a polyline from `a` to `b` with bulge `bulge` (the tangent of a
    /// quarter of the included angle, positive counter-clockwise).
    fn bulge(a: DVec2, b: DVec2, bulge: f64) -> Self {
        let chord = b - a;
        let c = chord.length();
        if bulge.abs() < 1e-12 || c == 0.0 {
            return Self::Line(a, b);
        }
        // The centre lies on the chord's bisector, c(1 − b²)/(4b) to the left.
        let center = (a + b) / 2.0 + chord.perp() / c * (c * (1.0 - bulge * bulge) / (4.0 * bulge));
        let (start, end) = if bulge > 0.0 { (a, b) } else { (b, a) };
        Self::Arc {
            center,
            start,
            end,
            sweep: 4.0 * bulge.abs().atan(),
            exact: true,
        }
    }

    fn mapped(self, xf: &Xform) -> Self {
        match self {
            Self::Spline { through, closed } => Self::Spline {
                through: through.into_iter().map(|p| xf.apply(p)).collect(),
                closed,
            },
            Self::Line(a, b) => Self::Line(xf.apply(a), xf.apply(b)),
            Self::Arc {
                center,
                start,
                end,
                sweep,
                exact,
            } => {
                // A mirror turns counter-clockwise into clockwise: swap the ends.
                let (start, end) = if xf.det() < 0.0 {
                    (end, start)
                } else {
                    (start, end)
                };
                Self::Arc {
                    center: xf.apply(center),
                    start: xf.apply(start),
                    end: xf.apply(end),
                    sweep,
                    exact,
                }
            }
            Self::Circle(c, r) => Self::Circle(xf.apply(c), r * xf.scale_factor()),
        }
    }

    /// Whether the numbers are usable: finite, within reach and not of zero size.
    fn check(&self) -> Result<(), &'static str> {
        let ok = |p: DVec2| p.is_finite() && p.abs().max_element() < MAX_COORD;
        match *self {
            Self::Spline {
                ref through,
                closed,
            } => {
                if !through.iter().all(|p| ok(*p)) {
                    return Err(INVALID);
                }
                let mut different: Vec<DVec2> = Vec::new();
                for p in through {
                    if !different.iter().any(|q| q.distance(*p) <= 1e-9) {
                        different.push(*p);
                    }
                }
                if different.len() < if closed { 3 } else { 2 } {
                    return Err(DEGENERATE);
                }
            }
            Self::Line(a, b) => {
                if !(ok(a) && ok(b)) {
                    return Err(INVALID);
                }
            }
            Self::Arc {
                center,
                start,
                end,
                sweep,
                ..
            } => {
                if !(ok(center) && ok(start) && ok(end) && sweep.is_finite()) {
                    return Err(INVALID);
                }
                if start.distance(center) * sweep == 0.0 {
                    return Err(DEGENERATE);
                }
            }
            Self::Circle(c, r) => {
                if !(ok(c) && r.is_finite() && r < MAX_COORD) {
                    return Err(INVALID);
                }
                if r <= 0.0 {
                    return Err(DEGENERATE);
                }
            }
        }
        Ok(())
    }

    fn curve(&self) -> Curve {
        match *self {
            Self::Spline {
                ref through,
                closed,
            } => Curve::spline_through(through, closed),
            Self::Line(a, b) => Curve::Line { a, b },
            Self::Arc {
                center, start, end, ..
            } => Curve::arc_from_points(center, start, end),
            Self::Circle(center, radius) => Curve::Circle { center, radius },
        }
    }
}

// ---- Skip reasons (keys of `ImportReport::skipped`) ----

const SPLINE: &str = "SPLINE";
/// Not a reason to skip: counts the splines that were drawn again through fit points.
const SPLINE_REDRAWN: &str = "SPLINE (drawn again through its fit points)";
const ELLIPSE: &str = "ELLIPSE (not a circle)";
const POLY3D: &str = "POLYLINE (3D or mesh)";
const NOT_XY: &str = "not in the XY plane";
const NON_UNIFORM: &str = "INSERT (different X and Y scales)";
const TOO_DEEP: &str = "INSERT (nested too deep)";
const RECURSIVE: &str = "INSERT (block inserts itself)";
const MISSING_BLOCK: &str = "INSERT (missing block)";
const XREF: &str = "INSERT (external reference)";
const PAPER: &str = "paper space";
const SKIPPED_LAYER: &str = "on a skipped layer";
const DEGENERATE: &str = "zero size";
const INVALID: &str = "invalid coordinates";

/// One warning per reason, and one for everything ignored by kind.
fn skip_warnings(skipped: &BTreeMap<&str, usize>) -> Vec<String> {
    let mut out = Vec::new();
    let mut ignored = Vec::new();
    for (&kind, &n) in skipped {
        let s = if n == 1 { "" } else { "s" };
        let line = match kind {
            SPLINE => format!(
                "{n} spline{s} skipped: only splines saved with fit points can be imported, \
                 and {} only control points. Save the drawing with fit-point splines, or \
                 redraw {} in the sketch.",
                if n == 1 { "this one has" } else { "these have" },
                if n == 1 { "it" } else { "them" }
            ),
            SPLINE_REDRAWN => continue,
            ELLIPSE => format!("{n} ellipse{s} skipped: sketches have no ellipses yet."),
            POLY3D => format!(
                "{n} 3D polyline{s} or mesh{} skipped.",
                if n == 1 { "" } else { "es" }
            ),
            NOT_XY => format!(
                "{n} entit{} skipped because {} not lie in the XY plane.",
                if n == 1 { "y" } else { "ies" },
                if n == 1 { "it does" } else { "they do" }
            ),
            NON_UNIFORM => format!(
                "{n} block insert{s} skipped: different X and Y scales would turn arcs into \
                 ellipses."
            ),
            TOO_DEEP => format!(
                "{n} block insert{s} skipped: blocks nest more than {MAX_BLOCK_DEPTH} levels \
                 deep."
            ),
            RECURSIVE => format!("{n} block insert{s} skipped: the block inserts itself."),
            MISSING_BLOCK => {
                format!("{n} block insert{s} skipped: the block is not defined in the file.")
            }
            XREF => format!(
                "{n} external reference{s} (xref) not loaded: only the file itself is read."
            ),
            PAPER => format!(
                "{n} entit{} in paper space (layouts) skipped: only model space is imported.",
                if n == 1 { "y" } else { "ies" }
            ),
            SKIPPED_LAYER => continue,
            DEGENERATE => format!(
                "{n} entit{} of zero size skipped.",
                if n == 1 { "y" } else { "ies" }
            ),
            INVALID => format!(
                "{n} entit{} with unusable coordinates (not finite, or more than 1000 km \
                 away) skipped.",
                if n == 1 { "y" } else { "ies" }
            ),
            other => {
                ignored.push(format!("{n} {other}"));
                continue;
            }
        };
        out.push(line);
    }
    if !ignored.is_empty() {
        out.push(format!(
            "Not imported: {}. Only lines, arcs, circles, polylines and blocks of them become \
             sketch geometry.",
            ignored.join(", ")
        ));
    }
    out
}

// ---- Walking the entities ----

/// An imported curve and what it becomes.
struct Item {
    shape: Shape,
    construction: bool,
}

struct Reader<'a, 'o> {
    file: &'a File<'a>,
    options: &'o ImportOptions,
    items: Vec<Item>,
    skipped: BTreeMap<&'a str, usize>,
    /// By upper-case name.
    layers: BTreeMap<String, LayerInfo>,
    work: usize,
    /// Upper-case names of the blocks being expanded.
    stack: Vec<String>,
}

fn has_layer(list: &[String], name: &str) -> bool {
    list.iter().any(|l| l.trim().eq_ignore_ascii_case(name))
}

impl<'a> Reader<'a, '_> {
    fn skip(&mut self, kind: &'a str) {
        *self.skipped.entry(kind).or_default() += 1;
    }

    fn count_layer(&mut self, name: &str) {
        self.layers
            .entry(name.to_uppercase())
            .or_insert_with(|| LayerInfo {
                name: name.to_owned(),
                entities: 0,
            })
            .entities += 1;
    }

    fn charge(&mut self) -> Result<(), ImportError> {
        self.work += 1;
        if self.work > MAX_WORK {
            return Err(ImportError::new(format!(
                "The drawing is too large to import: its blocks expand to more than \
                 {MAX_WORK} entities."
            )));
        }
        Ok(())
    }

    fn emit(&mut self, shape: Shape, xf: &Xform, construction: bool) -> Result<(), ImportError> {
        let shape = shape.mapped(xf);
        if let Err(reason) = shape.check() {
            self.skip(reason);
            return Ok(());
        }
        if self.items.len() >= MAX_CURVES {
            return Err(ImportError::new(format!(
                "The drawing has more than {MAX_CURVES} curves, too many for a sketch."
            )));
        }
        self.items.push(Item {
            shape,
            construction,
        });
        Ok(())
    }

    /// Imports `recs` through `xf`. `inherit` is the layer that layer-0 entities take (in
    /// a block); `top` is set for the ENTITIES section itself.
    fn expand(
        &mut self,
        recs: &[Rec<'a>],
        xf: &Xform,
        inherit: Option<&str>,
        top: bool,
    ) -> Result<(), ImportError> {
        let mut i = 0;
        while i < recs.len() {
            let r = recs[i];
            i += 1;
            // A polyline's vertices follow it, up to SEQEND.
            let mut vertices: &[Rec<'a>] = &[];
            if r.kind == "POLYLINE" {
                let from = i;
                while i < recs.len() && recs[i].kind == "VERTEX" {
                    i += 1;
                }
                vertices = &recs[from..i];
                if i < recs.len() && recs[i].kind == "SEQEND" {
                    i += 1;
                }
            }
            if matches!(r.kind, "SEQEND" | "VERTEX" | "ENDBLK" | "") {
                continue;
            }
            self.charge()?;
            let own = r.layer();
            let layer = match inherit {
                Some(l) if own == "0" => l,
                _ => own,
            };
            self.count_layer(layer);
            if top && r.int(67, 0)? == 1 {
                self.skip(PAPER);
                continue;
            }
            if has_layer(&self.options.skip_layers, layer) {
                self.skip(SKIPPED_LAYER);
                continue;
            }
            let construction = has_layer(&self.options.layers_as_construction, layer);
            self.entity(r, vertices, xf, layer, construction)?;
        }
        Ok(())
    }

    fn entity(
        &mut self,
        r: Rec<'a>,
        vertices: &[Rec<'a>],
        xf: &Xform,
        layer: &str,
        construction: bool,
    ) -> Result<(), ImportError> {
        // Entities given in object coordinates: their plane must be XY.
        let in_ocs = matches!(
            r.kind,
            "ARC" | "CIRCLE" | "LWPOLYLINE" | "POLYLINE" | "INSERT"
        );
        let xf = if in_ocs {
            let Some(o) = ocs(r.extrusion()?) else {
                self.skip(NOT_XY);
                return Ok(());
            };
            xf.then(&o)
        } else {
            *xf
        };
        match r.kind {
            "LINE" => {
                let shape = Shape::Line(r.point(10)?, r.point(11)?);
                self.emit(shape, &xf, construction)
            }
            "CIRCLE" => {
                let shape = Shape::Circle(r.point(10)?, r.num(40, 0.0)?);
                self.emit(shape, &xf, construction)
            }
            "ARC" => {
                let (a0, a1) = (r.num(50, 0.0)?, r.num(51, 0.0)?);
                let mut sweep = (a1 - a0).rem_euclid(360.0);
                if sweep < 1e-12 {
                    sweep = 360.0;
                }
                let shape = Shape::arc(
                    r.point(10)?,
                    r.num(40, 0.0)?,
                    a0.to_radians(),
                    sweep.to_radians(),
                );
                self.emit(shape, &xf, construction)
            }
            "ELLIPSE" => self.ellipse(r, &xf, construction),
            "LWPOLYLINE" => {
                let mut points: Vec<(DVec2, f64)> = Vec::new();
                for p in r.pairs {
                    match p.code {
                        10 => points.push((DVec2::new(number(p)?, 0.0), 0.0)),
                        20 => {
                            if let Some(last) = points.last_mut() {
                                last.0.y = number(p)?;
                            }
                        }
                        42 => {
                            if let Some(last) = points.last_mut() {
                                last.1 = number(p)?;
                            }
                        }
                        _ => {}
                    }
                }
                self.polyline(&points, r.int(70, 0)? & 1 != 0, &xf, construction)
            }
            "POLYLINE" => {
                let flags = r.int(70, 0)?;
                // 8: 3D polyline, 16: polygon mesh, 64: polyface mesh.
                if flags & (8 | 16 | 64) != 0 {
                    self.skip(POLY3D);
                    return Ok(());
                }
                let mut points = Vec::new();
                for v in vertices {
                    // 16: a spline frame control point, not on the curve.
                    if v.int(70, 0)? & 16 != 0 {
                        continue;
                    }
                    points.push((v.point(10)?, v.num(42, 0.0)?));
                }
                self.polyline(&points, flags & 1 != 0, &xf, construction)
            }
            "INSERT" => self.insert(r, &xf, layer),
            "SPLINE" => {
                // Fit points are world coordinates: 11 and 21, in order. Flag 1: closed.
                let mut through: Vec<DVec2> = Vec::new();
                let mut x = None;
                for p in r.pairs {
                    match p.code {
                        11 => x = Some(number(p)?),
                        21 => {
                            if let Some(x) = x.take() {
                                through.push(DVec2::new(x, number(p)?));
                            }
                        }
                        _ => {}
                    }
                }
                if through.len() < 2 {
                    self.skip(SPLINE);
                    return Ok(());
                }
                if ocs(r.extrusion()?).is_none() {
                    self.skip(NOT_XY);
                    return Ok(());
                }
                let mut closed = r.int(70, 0)? & 1 != 0;
                if through.len() > 2 && through[0].distance(through[through.len() - 1]) <= 1e-9 {
                    through.pop();
                    closed = true;
                }
                let before = self.items.len();
                self.emit(Shape::Spline { through, closed }, &xf, construction)?;
                if self.items.len() > before {
                    self.skip(SPLINE_REDRAWN);
                }
                Ok(())
            }
            other => {
                self.skip(other);
                Ok(())
            }
        }
    }

    fn polyline(
        &mut self,
        points: &[(DVec2, f64)],
        closed: bool,
        xf: &Xform,
        construction: bool,
    ) -> Result<(), ImportError> {
        let n = points.len();
        let segments = if closed && n > 2 {
            n
        } else {
            n.saturating_sub(1)
        };
        if segments == 0 {
            self.skip(DEGENERATE);
        }
        for i in 0..segments {
            let (a, bulge) = points[i];
            let b = points[(i + 1) % n].0;
            if a == b {
                // A repeated vertex (often the closing one): nothing to draw.
                continue;
            }
            self.emit(Shape::bulge(a, b, bulge), xf, construction)?;
        }
        Ok(())
    }

    fn ellipse(&mut self, r: Rec<'a>, xf: &Xform, construction: bool) -> Result<(), ImportError> {
        let n = r.extrusion()?;
        let major = r.point(11)?;
        let major_z = r.num(31, 0.0)?;
        let Some(o) = ocs(n) else {
            self.skip(NOT_XY);
            return Ok(());
        };
        let radius = major.length();
        if major_z.abs() > 1e-9 * radius.max(1e-300) {
            self.skip(NOT_XY);
            return Ok(());
        }
        let ratio = r.num(40, 1.0)?;
        if (ratio - 1.0).abs() > 1e-9 {
            self.skip(ELLIPSE);
            return Ok(());
        }
        let (p0, p1) = (r.num(41, 0.0)?, r.num(42, TAU)?);
        let mut sweep = (p1 - p0).rem_euclid(TAU);
        if sweep < 1e-12 {
            sweep = TAU;
        }
        // Parameters run counter-clockwise about the extrusion: clockwise in world XY
        // when it points down.
        let base = major.y.atan2(major.x);
        let start = if o.det() > 0.0 { base + p0 } else { base - p1 };
        let shape = Shape::arc(r.point(10)?, radius, start, sweep);
        self.emit(shape, xf, construction)
    }

    fn insert(&mut self, r: Rec<'a>, xf: &Xform, layer: &str) -> Result<(), ImportError> {
        let file = self.file;
        let name = r.text(2).unwrap_or("");
        let key = name.to_uppercase();
        let Some(block) = file.blocks.get(&key) else {
            self.skip(MISSING_BLOCK);
            return Ok(());
        };
        if block.xref {
            self.skip(XREF);
            return Ok(());
        }
        if self.stack.contains(&key) {
            self.skip(RECURSIVE);
            return Ok(());
        }
        if self.stack.len() >= MAX_BLOCK_DEPTH {
            self.skip(TOO_DEEP);
            return Ok(());
        }
        let (sx, sy) = (r.num(41, 1.0)?, r.num(42, 1.0)?);
        if sx == 0.0 || sy == 0.0 {
            self.skip(DEGENERATE);
            return Ok(());
        }
        if (sx.abs() - sy.abs()).abs() > 1e-9 * sx.abs().max(sy.abs()) {
            self.skip(NON_UNIFORM);
            return Ok(());
        }
        let at = r.point(10)?;
        let rotation = r.num(50, 0.0)?.to_radians();
        let (cols, rows) = (r.int(70, 1)?.max(1), r.int(71, 1)?.max(1));
        let (dx, dy) = (r.num(44, 0.0)?, r.num(45, 0.0)?);
        let placed = xf
            .then(&Xform::translate(at))
            .then(&Xform::rotate(rotation));
        let local = Xform::scale(sx, sy).then(&Xform::translate(-block.base));
        self.stack.push(key);
        for row in 0..rows {
            for col in 0..cols {
                self.charge()?;
                let cell = Xform::translate(DVec2::new(col as f64 * dx, row as f64 * dy));
                let full = placed.then(&cell).then(&local);
                self.expand(&block.records, &full, Some(layer), false)?;
            }
        }
        self.stack.pop();
        Ok(())
    }
}

// ---- Building the sketch ----

/// Adds the items to the sketch, merging coincident ends. Returns the new curves, the
/// placement offset and how many curves shrank to nothing when their ends merged.
fn build(
    sketch: &mut Sketch,
    items: Vec<Item>,
    placement: Placement,
) -> (Vec<EntityId>, DVec2, usize) {
    if items.is_empty() {
        return (Vec::new(), DVec2::ZERO, 0);
    }
    let mut lo = DVec2::splat(f64::INFINITY);
    let mut hi = DVec2::splat(f64::NEG_INFINITY);
    for it in &items {
        let (a, b) = it.shape.curve().bounds();
        lo = lo.min(a);
        hi = hi.max(b);
    }
    let offset = match placement {
        Placement::Keep => DVec2::ZERO,
        Placement::Centred => -(lo + hi) / 2.0,
        Placement::LowerLeftAtOrigin => -lo,
    };
    let tol = (1e-6 * (hi - lo).length()).max(1e-9);
    let shift = Xform::translate(offset);
    let items: Vec<Item> = items
        .into_iter()
        .map(|it| Item {
            shape: it.shape.mapped(&shift),
            ..it
        })
        .collect();

    // The ends of lines and arcs, and whether each was given exactly in the file.
    let mut ends: Vec<(DVec2, bool)> = Vec::new();
    for it in &items {
        match it.shape {
            Shape::Spline {
                ref through,
                closed,
            } => {
                if !closed {
                    ends.extend([(through[0], true), (through[through.len() - 1], true)]);
                }
            }
            Shape::Line(a, b) => ends.extend([(a, true), (b, true)]),
            Shape::Arc {
                start, end, exact, ..
            } => ends.extend([(start, exact), (end, exact)]),
            Shape::Circle(..) => {}
        }
    }
    let (cluster, position) = merge(&ends, tol);

    let mut added = Vec::with_capacity(items.len());
    let mut collapsed = 0;
    // Sketch points at each merged position, by cluster.
    let mut joined: Vec<Vec<EntityId>> = vec![Vec::new(); position.len()];
    let mut k = 0;
    for it in &items {
        let id = match it.shape {
            Shape::Spline {
                ref through,
                closed,
            } => {
                let mut through = through.clone();
                let mut closed = closed;
                let mut joins = None;
                if !closed {
                    let (ca, cb) = (cluster[k], cluster[k + 1]);
                    k += 2;
                    let last = through.len() - 1;
                    if ca == cb {
                        // The ends merged: the spline closes on itself.
                        through.pop();
                        closed = true;
                    } else {
                        through[0] = position[ca];
                        through[last] = position[cb];
                        joins = Some((ca, cb));
                    }
                }
                let Ok(id) = sketch.add_spline(&through, closed) else {
                    collapsed += 1;
                    continue;
                };
                if let (Some((ca, cb)), Some((s, e))) = (joins, sketch.endpoints(id)) {
                    joined[ca].push(s);
                    joined[cb].push(e);
                }
                id
            }
            Shape::Line(..) => {
                let (ca, cb) = (cluster[k], cluster[k + 1]);
                k += 2;
                if ca == cb {
                    collapsed += 1;
                    continue;
                }
                let id = sketch.add_line(position[ca], position[cb]);
                if let Some((s, e)) = sketch.endpoints(id) {
                    joined[ca].push(s);
                    joined[cb].push(e);
                }
                id
            }
            Shape::Arc { center, sweep, .. } => {
                let (cs, ce) = (cluster[k], cluster[k + 1]);
                k += 2;
                let (s, e) = (position[cs], position[ce]);
                if cs == ce {
                    // The ends merged: a whole circle if the arc nearly was one.
                    if sweep > PI {
                        sketch.add_circle(center, s.distance(center))
                    } else {
                        collapsed += 1;
                        continue;
                    }
                } else {
                    // Keep both (moved) ends on the circle: move the centre onto their
                    // bisector.
                    let mid = (s + e) / 2.0;
                    let normal = (e - s).perp().normalize_or_zero();
                    let center = mid + normal * normal.dot(center - mid);
                    let id = sketch.add_arc(center, s, e);
                    if let Some((sp, ep)) = sketch.endpoints(id) {
                        joined[cs].push(sp);
                        joined[ce].push(ep);
                    }
                    id
                }
            }
            Shape::Circle(c, r) => sketch.add_circle(c, r),
        };
        if it.construction {
            sketch.set_construction(id, true);
        }
        added.push(id);
    }
    for points in &joined {
        for &p in points.iter().skip(1) {
            let _ = sketch.add_constraint(ConstraintKind::Coincident(points[0], p));
        }
    }
    (added, offset, collapsed)
}

/// Groups points closer than `tol` (transitively). Returns each point's group and each
/// group's position: its first exact point, else its first point.
fn merge(points: &[(DVec2, bool)], tol: f64) -> (Vec<usize>, Vec<DVec2>) {
    let mut parent: Vec<usize> = (0..points.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let cell = |p: DVec2| ((p.x / tol).floor() as i64, (p.y / tol).floor() as i64);
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, &(p, _)) in points.iter().enumerate() {
        let (cx, cy) = cell(p);
        for nx in cx.saturating_sub(1)..=cx.saturating_add(1) {
            for ny in cy.saturating_sub(1)..=cy.saturating_add(1) {
                let Some(near) = grid.get(&(nx, ny)) else {
                    continue;
                };
                for &j in near {
                    if points[j].0.distance(p) <= tol {
                        let (a, b) = (root(&mut parent, i), root(&mut parent, j));
                        if a != b {
                            parent[a.max(b)] = a.min(b);
                        }
                    }
                }
            }
        }
        grid.entry((cx, cy)).or_default().push(i);
    }
    let mut group = vec![usize::MAX; points.len()];
    let mut position: Vec<DVec2> = Vec::new();
    let mut exact: Vec<bool> = Vec::new();
    let mut out = Vec::with_capacity(points.len());
    for (i, &(p, is_exact)) in points.iter().enumerate() {
        let r = root(&mut parent, i);
        if group[r] == usize::MAX {
            group[r] = position.len();
            position.push(p);
            exact.push(is_exact);
        } else if is_exact && !exact[group[r]] {
            position[group[r]] = p;
            exact[group[r]] = true;
        }
        out.push(group[r]);
    }
    (out, position)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bulge_arcs() {
        // A half circle above the chord from (0, 0) to (2, 0) when running clockwise.
        let Shape::Arc {
            center,
            start,
            end,
            sweep,
            ..
        } = Shape::bulge(DVec2::ZERO, DVec2::new(2.0, 0.0), 1.0)
        else {
            panic!()
        };
        assert!(center.abs_diff_eq(DVec2::new(1.0, 0.0), 1e-12));
        assert_eq!((start, end), (DVec2::ZERO, DVec2::new(2.0, 0.0)));
        assert!((sweep - PI).abs() < 1e-12);
        // A quarter circle counter-clockwise from (1, 0) to (0, 1) around the origin.
        let b = (PI / 8.0).tan();
        let Shape::Arc { center, .. } = Shape::bulge(DVec2::X, DVec2::Y, b) else {
            panic!()
        };
        assert!(center.abs_diff_eq(DVec2::ZERO, 1e-12), "{center}");
        // Clockwise: the ends swap, the centre is on the other side.
        let Shape::Arc {
            center, start, end, ..
        } = Shape::bulge(DVec2::Y, DVec2::X, -b)
        else {
            panic!()
        };
        assert!(center.abs_diff_eq(DVec2::ZERO, 1e-12), "{center}");
        assert_eq!((start, end), (DVec2::X, DVec2::Y));
    }

    #[test]
    fn mirror_swaps_arc_ends() {
        let arc = Shape::arc(DVec2::new(10.0, 0.0), 10.0, 0.0, PI / 2.0);
        let Shape::Arc {
            center, start, end, ..
        } = arc.mapped(&ocs([0.0, 0.0, -1.0]).unwrap())
        else {
            panic!()
        };
        assert!(center.abs_diff_eq(DVec2::new(-10.0, 0.0), 1e-12));
        assert!(start.abs_diff_eq(DVec2::new(-10.0, 10.0), 1e-12));
        assert!(end.abs_diff_eq(DVec2::new(-20.0, 0.0), 1e-12));
        assert!(ocs([0.0, 1.0, 0.0]).is_none());
    }

    #[test]
    fn merge_prefers_exact_points() {
        let pts = [
            (DVec2::new(1e-10, 0.0), false),
            (DVec2::ZERO, true),
            (DVec2::new(5.0, 0.0), true),
        ];
        let (group, pos) = merge(&pts, 1e-9);
        assert_eq!(group, vec![0, 0, 1]);
        assert_eq!(pos, vec![DVec2::ZERO, DVec2::new(5.0, 0.0)]);
    }
}
