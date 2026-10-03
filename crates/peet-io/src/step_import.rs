//! STEP import (ISO 10303-21 clear text; AP203, AP214 and AP242) of solids as exact
//! B-reps: the inverse of [`crate::step`].
//!
//! **Scope.** The reader takes what the kernel has: `MANIFOLD_SOLID_BREP` and
//! `BREP_WITH_VOIDS` whose faces lie on a `PLANE`, `CYLINDRICAL_SURFACE`,
//! `CONICAL_SURFACE`, `SPHERICAL_SURFACE`, `TOROIDAL_SURFACE`,
//! `DEGENERATE_TOROIDAL_SURFACE`, a B-spline surface or a swept surface, and whose edges
//! are a `LINE`, `CIRCLE`, `ELLIPSE` or B-spline curve (also inside a `SURFACE_CURVE`,
//! `SEAM_CURVE`, `INTERSECTION_CURVE` or `TRIMMED_CURVE`). Anything else on a face or an
//! edge (offset and trimmed surfaces, edges given only as a `PCURVE`) is an error that
//! names the entity. Surface bodies (`SHELL_BASED_SURFACE_MODEL`) and faceted bodies are
//! skipped with a warning. Colours, layers, annotations and everything else in the file
//! are ignored.
//!
//! **Freeform geometry.** B-splines come as `B_SPLINE_CURVE_WITH_KNOTS` and
//! `B_SPLINE_SURFACE_WITH_KNOTS` (knots with their multiplicities), `BEZIER_`,
//! `QUASI_UNIFORM_` and `UNIFORM_CURVE` / `_SURFACE` (knots implied), and as complex
//! instances that add weights (`RATIONAL_B_SPLINE_CURVE` / `_SURFACE`). They become
//! [`peet_kernel::nurbs`] curves and surfaces, control point for control point (a
//! surface's are listed a row along `v` for each step in `u`, the kernel's order). What
//! the kernel does not have is converted, exactly, or refused:
//!
//! - *Knots that are not clamped* (uniform ones, the "periodic" way of writing a closed
//!   curve): knots are inserted at the ends of the curve's range until it is clamped.
//! - *Closed curves and surfaces.* The kernel's are not periodic: an edge is a stretch
//!   `t0 < t1` of its curve, and a face a simple region of its surface's parameter
//!   rectangle. Whether a B-spline closes on itself is seen from its shape (its ends
//!   meet), not from the file's `closed` flags. An edge that runs across the start of a
//!   closed curve, or all the way round it from a vertex elsewhere, gets the curve put
//!   together anew from where the edge starts. A face on a closed surface gets the
//!   surface cut down to the stretch the face covers (put together across the surface's
//!   seam, where the face lies across it). A face that goes all the way round, with a
//!   seam edge used twice, is first cut in two along a line of constant parameter half
//!   way round from its seam, which splits the edges the line meets (and so adds
//!   vertices to the neighbouring faces). A face that goes all the way round *without* a
//!   seam (a band between two closed edges) is refused.
//! - *Pinched surfaces.* A B-spline surface with an edge squeezed to a point (the tip of
//!   a curve turned about an axis it ends on, a three-sided patch) has no parameters
//!   there, which the kernel's freeform faces can't cope with yet: a face that comes to
//!   such a point is refused. (A sphere or cone written as such is fine: those have
//!   their poles.)
//!
//! `SURFACE_OF_LINEAR_EXTRUSION` and `SURFACE_OF_REVOLUTION` of a line, circle, ellipse or
//! B-spline become the plane, cylinder, cone, sphere or torus they are, where they are
//! one (a line swept sideways, a circle turned about a line in its plane), with the face
//! turned over where that surface faces the other way. Otherwise they become an exact
//! B-spline surface, as long as the face needs: ruled along the extrusion, or a rational
//! quadratic round the axis (which closes on itself, and is treated as above).
//!
//! **Reading.** The file is tokenised and parsed into a table of entity instances
//! (`#12=NAME(..);`, or a complex instance `#12=(A(..) B(..));`). Strings understand `''`
//! and the `\X2\`, `\X4\`, `\X\` and `\S\` escapes; typed parameters such as
//! `LENGTH_MEASURE(1.0)`, enumerations, `$` and `*` are kept as they are. The header is
//! skipped. Nothing here trusts the file: every reference is looked up, every parameter is
//! checked for its kind, nesting and reference chains are bounded, and numbers must be
//! finite and within [`MAX_COORD`]. A file that is damaged, cut off or hostile gives a
//! [`StepImportError`], never a panic.
//!
//! **Units.** Each brep is an item of a representation whose context
//! (`GLOBAL_UNIT_ASSIGNED_CONTEXT`) names the length unit (an `SI_UNIT` with its prefix,
//! or a `CONVERSION_BASED_UNIT` such as the inch) and the plane angle unit (radians, or
//! degrees as a conversion based unit). Lengths (a B-spline's control points among them)
//! are converted to millimetres and angles (only a cone's semi-angle is one) to radians.
//! A file that names no unit is taken to be in millimetres and radians, with a warning.
//!
//! **Orientation.** STEP's conventions are the kernel's (see [`crate::step`] and
//! [`peet_kernel::topo`]), so most of the mapping is direct: `Face.reversed` is
//! `!same_sense`, a coedge is reversed when its `ORIENTED_EDGE` says `.F.`, a bound with
//! orientation `.F.` is walked backwards. Where the two differ, the file is brought into
//! the kernel's form:
//!
//! - *Edges.* A kernel edge runs along its curve in the direction of increasing parameter.
//!   An `EDGE_CURVE` with `same_sense = .F.` (or a line whose vertices are given against
//!   its direction) gets its curve turned round: a line's direction is negated, a circle's
//!   or ellipse's frame is turned half a turn about its X axis, a B-spline's control
//!   points are listed backwards. The parameter range comes from the vertices; a closed
//!   edge spans one period, or the whole of a closed B-spline.
//! - *Outer loops.* The kernel wants a face's outer loop first. `FACE_OUTER_BOUND` is not
//!   reliable (many writers use plain `FACE_BOUND` throughout), so the outer loop is the
//!   one that runs counter-clockwise seen from outside: the one with a positive area in the
//!   surface's parameter space.
//! - *Seams.* A kernel face is simply connected in its surface's `(u, v)`: a cylinder wall
//!   that goes all the way round has a seam edge, used twice by its loop. Files that carry
//!   seams (OCCT's, PeetCAD's own) or split such faces in halves are taken as they are.
//!   Where a face is bounded by loops that go round the surface (a cylinder wall between
//!   two circles), a seam is added: a meridian (or, on a torus whose loops go round the
//!   tube, a parallel) from one loop to the other, at an angle where it crosses each of
//!   them once and stays clear of the face's holes. The loops' edges are split where the
//!   seam meets them, which also updates the neighbouring faces.
//! - *Poles.* A sphere's poles and a cone's apex are vertices of every kernel face that
//!   touches them. A face that surrounds a pole (a cone bounded by its base circle, with
//!   or without a `VERTEX_LOOP` at the apex; a spherical cap) gets a seam from its loop to
//!   a vertex at the pole. A whole sphere (no bounds, or a `VERTEX_LOOP`) gets one seam
//!   from pole to pole, and a whole torus two seams that cross. An edge that passes
//!   through a pole of its face's surface is split there.
//! - *Shells.* Outer shells must enclose a positive volume and voids a negative one. STEP
//!   writers disagree on which way a void's faces point and whether its
//!   `ORIENTED_CLOSED_SHELL` flips them, so the importer measures each shell and turns it
//!   inside out when its volume has the wrong sign.
//!
//! Vertices and edges are shared by STEP identity (two faces share an edge when they name
//! the same `EDGE_CURVE`). Files that repeat a vertex as several `VERTEX_POINT`s in the
//! same place get those merged where a loop would otherwise not close, with a warning.
//!
//! **Validity.** Every solid is checked with [`peet_kernel::validate::validate`] before it
//! is returned. A body that fails (edges not shared between faces, vertices off their
//! curves, a hole across a seam) makes the import fail with the reasons, rather than
//! handing the rest of the program a broken solid.
//!
//! **Assemblies.** Parts placed in assemblies (`ITEM_DEFINED_TRANSFORMATION` under a
//! `REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION`, or `MAPPED_ITEM`) are imported in
//! place: one body per occurrence, moved by the chain of placements above it. A placement
//! that is not a rigid motion is ignored with a warning.

use std::collections::{HashMap, HashSet};
use std::f64::consts::{FRAC_PI_2, PI, TAU};
use std::sync::Arc;

use peet_kernel::geom::{
    Circle3, Cone, Curve3, Cylinder, Ellipse3, Line3, Sphere, Surface, Torus, pole_exit,
};
use peet_kernel::nurbs::{NurbsCurve, NurbsError, NurbsSurface};
use peet_kernel::topo::{EdgeId, ShellId, Solid, VertexId};
use peet_kernel::validate::{measure, validate};
use peet_math::{DQuat, DVec2, DVec3, Frame, Plane, tolerance};

/// Deepest nesting of parentheses in an entity's parameters. Real files use two or three
/// levels (the control points of a B-spline surface); the limit keeps the parser's
/// recursion bounded on hostile input.
const MAX_NESTING: usize = 64;

/// Longest chain of entities that wrap one another (a `SURFACE_CURVE` around a
/// `TRIMMED_CURVE` around a `CIRCLE`; a unit defined by another unit).
const MAX_WRAPPERS: usize = 16;

/// Deepest assembly structure followed when placing parts.
const MAX_ASSEMBLY_DEPTH: usize = 64;

/// Most bodies an import may produce (occurrences in assemblies multiply out).
const MAX_BODIES: usize = 10_000;

/// Coordinates and radii beyond this (mm) are treated as broken: far outside what the
/// tolerance model in [`peet_math::tolerance`] can work with.
pub const MAX_COORD: f64 = 1e9;

/// `VERTEX_POINT`s this close together (mm) are taken to be one vertex written twice,
/// where a loop would otherwise not close. The same distance the kernel allows between a
/// vertex and its edges' curves.
const VERTEX_MERGE: f64 = 100.0 * tolerance::LINEAR;

/// A seam that meets a loop this close (mm) to one of its vertices ends at that vertex
/// instead of splitting the edge beside it.
const SEAM_SNAP: f64 = 100.0 * tolerance::LINEAR;

/// An edge passes through a pole when its curve comes this close to it (mm).
const POLE_ON_EDGE: f64 = tolerance::LINEAR;

/// Curved edges are sampled in steps of at most this much of their parameter when a loop
/// is followed through a surface's parameter space; lines in [`LINE_STEPS`] steps.
const SAMPLE_STEP: f64 = PI / 32.0;
const LINE_STEPS: usize = 4;

/// A loop's lifted parameters must come back to where they started, or a whole number of
/// turns away, to within this (radians).
const WINDING_SLACK: f64 = 1e-6;

/// A new seam stays at least this far (radians) from the face's holes: more than the
/// slack the kernel's validation allows a hole at a seam.
const SEAM_CLEARANCE: f64 = 1e-5;

/// Two neighbouring samples of an edge this close (radians) to a seam's angle mean the
/// edge runs along the seam; another angle is tried.
const ALONG_SEAM: f64 = 1e-9;

/// How many evenly spaced angles are tried for a seam after the loops' own vertices.
const SEAM_TRIES: usize = 16;

/// The highest degree of a B-spline the kernel works with.
const MAX_DEGREE: usize = 15;

/// A B-spline curve or surface is closed when its ends are this close together (mm).
const CLOSED: f64 = VERTEX_MERGE;

/// A face covers a closed freeform surface all the way round when it leaves less than
/// this fraction of the surface's period free.
const FULL_TURN: f64 = 1e-6;

/// A closed freeform surface is cut down to its face with at most this fraction of its
/// period to spare on either side.
const TRIM_MARGIN: f64 = 1.0 / 16.0;

/// Where a face that goes all the way round a closed freeform surface is tried for a
/// cut, as fractions of the period from the face's seam.
const CUT_TRIES: [f64; 7] = [0.5, 0.375, 0.625, 0.25, 0.75, 0.4375, 0.5625];

/// How often a face on a closed freeform surface is cut or trimmed before giving up.
const UNWRAP_PASSES: usize = 16;

/// One solid of the file, in millimetres.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportedBody {
    /// The brep's name; the name of its product when the brep has none.
    pub name: String,
    pub solid: Solid,
}

/// What a STEP file held.
#[derive(Clone, Debug, PartialEq)]
pub struct StepImport {
    /// Every solid, in file order. A part used several times in an assembly appears once
    /// per occurrence, in place.
    pub bodies: Vec<ImportedBody>,
    /// Plain-English notes for the user: assumptions made and what was left out.
    pub warnings: Vec<String>,
    /// The name of the file's (first) product; empty when it has none.
    pub product_name: String,
}

/// Why a file could not be imported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepImportError {
    pub message: String,
}

impl std::fmt::Display for StepImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for StepImportError {}

type Result<T> = std::result::Result<T, StepImportError>;

fn error(message: impl Into<String>) -> StepImportError {
    StepImportError {
        message: message.into(),
    }
}

/// An error for a file whose contents contradict themselves.
fn damaged(what: impl std::fmt::Display) -> StepImportError {
    error(format!(
        "The STEP file is damaged: {what}. Try exporting it again from the program that made it."
    ))
}

/// Reads a STEP file's solids. See the module docs for what is supported.
pub fn read(text: &str) -> Result<StepImport> {
    let file = parse(text)?;
    let structure = Structure::read(&file);
    let mut warnings = Vec::new();
    let breps = file.ids_named(&["MANIFOLD_SOLID_BREP", "BREP_WITH_VOIDS"]);
    let surface_models = file
        .ids_named(&["SHELL_BASED_SURFACE_MODEL", "FACE_BASED_SURFACE_MODEL"])
        .len();
    let faceted = file.ids_named(&["FACETED_BREP"]).len();
    if breps.is_empty() {
        return Err(error(if surface_models > 0 {
            "This STEP file has only surface bodies (open or unstitched shells), which PeetCAD \
             can't import: it needs solids. Export the model as a solid (a manifold solid B-rep)."
        } else if faceted > 0 {
            "This STEP file has only faceted bodies, which PeetCAD can't import. Export the \
             model as a solid with exact faces (a manifold solid B-rep)."
        } else {
            "This STEP file has no solids in it (no MANIFOLD_SOLID_BREP), so there is nothing \
             to import. Check that the model was exported as a solid."
        }));
    }
    if surface_models > 0 {
        warnings.push(format!(
            "{surface_models} surface bod{} skipped: PeetCAD imports solids only.",
            if surface_models == 1 {
                "y was"
            } else {
                "ies were"
            }
        ));
    }
    if faceted > 0 {
        warnings.push(format!(
            "{faceted} faceted bod{} skipped: PeetCAD imports solids with exact faces only.",
            if faceted == 1 { "y was" } else { "ies were" }
        ));
    }

    let mut bodies = Vec::new();
    let mut assumed_units = false;
    let mut merged_vertices = 0;
    let mut turned_shells = 0;
    let mut bad_placements = 0;
    for id in breps {
        let reps = structure.item_reps.get(&id).cloned().unwrap_or_default();
        let units = match reps.first() {
            Some(&rep) => structure.units(&file, rep),
            None => structure.any_units(&file),
        };
        let units = units.unwrap_or_else(|| {
            assumed_units = true;
            Units {
                length: 1.0,
                angle: 1.0,
            }
        });
        let brep = file.expect(id, &["MANIFOLD_SOLID_BREP", "BREP_WITH_VOIDS"], None)?;
        let mut builder = Builder::new(&file, units);
        let built = builder.brep(&brep)?;
        merged_vertices += builder.merged;
        turned_shells += usize::from(built.outer_was_inside_out);

        let own_name = brep.text(0).trim();
        let name = if !own_name.is_empty() {
            own_name.to_owned()
        } else if let Some(n) = reps.iter().find_map(|&r| structure.name_of(r)) {
            n.to_owned()
        } else if !structure.product_name.is_empty() {
            structure.product_name.clone()
        } else {
            "Body".to_owned()
        };
        if let Err(problems) = validate(&built.solid) {
            let list: Vec<&str> = problems
                .iter()
                .take(3)
                .map(|p| p.message.as_str())
                .collect();
            let more = problems.len().saturating_sub(list.len());
            return Err(error(format!(
                "The body \"{name}\" (entity #{id}) in this STEP file is not a closed, valid \
                 solid, so PeetCAD can't import it: {}{}. Try exporting it again as a solid, \
                 with a finer tolerance if the other program offers one.",
                list.join("; "),
                if more > 0 {
                    format!(
                        "; and {more} more problem{}",
                        if more == 1 { "" } else { "s" }
                    )
                } else {
                    String::new()
                }
            )));
        }

        // Where the body goes: once per occurrence in the assemblies above it.
        let mut frames = Vec::new();
        for &rep in &reps {
            let placed = structure.placements(&file, rep);
            bad_placements += placed.unsupported;
            frames.extend(placed.frames);
        }
        if frames.is_empty() {
            frames.push(Frame::WORLD);
        }
        for frame in frames {
            if bodies.len() >= MAX_BODIES {
                return Err(error(format!(
                    "This STEP file has more than {MAX_BODIES} bodies once its assemblies are \
                     expanded, which is more than PeetCAD can import. Export a smaller part of \
                     the assembly."
                )));
            }
            let solid = if frame == Frame::WORLD {
                built.solid.clone()
            } else {
                peet_kernel::transform::solid(&built.solid, &frame)
            };
            bodies.push(ImportedBody {
                name: name.clone(),
                solid,
            });
        }
    }
    if assumed_units {
        warnings.push(
            "The file doesn't say what units it is in; millimetres and radians were assumed."
                .to_owned(),
        );
    }
    if merged_vertices > 0 {
        warnings.push(format!(
            "The file writes some vertices more than once; {merged_vertices} pair{} closer \
             than {VERTEX_MERGE} mm {} merged.",
            if merged_vertices == 1 { "" } else { "s" },
            if merged_vertices == 1 { "was" } else { "were" }
        ));
    }
    if turned_shells > 0 {
        warnings.push(format!(
            "{turned_shells} bod{} written inside out (faces pointing inwards) and {} turned \
             the right way round.",
            if turned_shells == 1 {
                "y was"
            } else {
                "ies were"
            },
            if turned_shells == 1 { "was" } else { "were" }
        ));
    }
    if bad_placements > 0 {
        warnings.push(
            "Some placements in the file's assembly structure are not plain moves and turns \
             (or could not be read); the parts they place were left where their own \
             coordinates put them."
                .to_owned(),
        );
    }
    Ok(StepImport {
        bodies,
        warnings,
        product_name: structure.product_name,
    })
}

// ---- Part 21 syntax ----

#[derive(Clone, Debug, PartialEq)]
enum Value {
    Ref(u32),
    Int(i64),
    Real(f64),
    Str(String),
    Enum(String),
    List(Vec<Value>),
    /// A typed parameter such as `LENGTH_MEASURE(25.4)`.
    Typed(String, Vec<Value>),
    /// `$`.
    Unset,
    /// `*`.
    Derived,
}

/// One `NAME(parameters)`. A simple instance is one record; a complex one has several.
#[derive(Clone, Debug, PartialEq)]
struct Record {
    name: String,
    params: Vec<Value>,
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Open,
    Close,
    Comma,
    Semi,
    Equals,
    Ref(u32),
    Int(i64),
    Real(f64),
    Str(String),
    Enum(String),
    Keyword(String),
    Dollar,
    Star,
    End,
}

struct Lexer<'a> {
    bytes: &'a [u8],
    at: usize,
    /// Where the token last returned starts.
    start: usize,
}

impl Lexer<'_> {
    fn line(&self, at: usize) -> usize {
        1 + self.bytes[..at.min(self.bytes.len())]
            .iter()
            .filter(|&&b| b == b'\n')
            .count()
    }

    fn syntax<T>(&self, what: &str) -> Result<T> {
        // Whatever breaks off at the very end of the text is a file cut short.
        if self.at >= self.bytes.len() {
            return self.cut_off();
        }
        Err(error(format!(
            "The STEP file can't be read: {what} on line {}. The file may be damaged; try \
             exporting it again.",
            self.line(self.start)
        )))
    }

    fn cut_off<T>(&self) -> Result<T> {
        Err(error(format!(
            "The STEP file is cut off: it ends in the middle of its data (line {}). It may not \
             have been saved or copied completely; export or copy it again.",
            self.line(self.bytes.len())
        )))
    }

    fn next(&mut self) -> Result<Token> {
        let b = self.bytes;
        loop {
            while b.get(self.at).is_some_and(u8::is_ascii_whitespace) {
                self.at += 1;
            }
            if b.get(self.at) == Some(&b'/') && b.get(self.at + 1) == Some(&b'*') {
                let rest = &b[self.at + 2..];
                match rest.windows(2).position(|w| w == b"*/") {
                    Some(end) => self.at += end + 4,
                    None => return self.cut_off(),
                }
            } else {
                break;
            }
        }
        self.start = self.at;
        let Some(&c) = b.get(self.at) else {
            return Ok(Token::End);
        };
        let simple = match c {
            b'(' => Some(Token::Open),
            b')' => Some(Token::Close),
            b',' => Some(Token::Comma),
            b';' => Some(Token::Semi),
            b'=' => Some(Token::Equals),
            b'$' => Some(Token::Dollar),
            b'*' => Some(Token::Star),
            _ => None,
        };
        if let Some(t) = simple {
            self.at += 1;
            return Ok(t);
        }
        match c {
            b'#' => {
                let from = self.at + 1;
                self.at = from;
                while b.get(self.at).is_some_and(u8::is_ascii_digit) {
                    self.at += 1;
                }
                match std::str::from_utf8(&b[from..self.at])
                    .ok()
                    .and_then(|t| t.parse().ok())
                {
                    Some(id) => Ok(Token::Ref(id)),
                    None => self.syntax("an entity number (#...) that isn't a number"),
                }
            }
            b'\'' => {
                let mut raw = Vec::new();
                self.at += 1;
                loop {
                    match b.get(self.at) {
                        None => return self.cut_off(),
                        Some(b'\'') if b.get(self.at + 1) == Some(&b'\'') => {
                            raw.push(b'\'');
                            self.at += 2;
                        }
                        Some(b'\'') => {
                            self.at += 1;
                            break;
                        }
                        Some(&ch) => {
                            raw.push(ch);
                            self.at += 1;
                        }
                    }
                }
                Ok(Token::Str(decode_string(&String::from_utf8_lossy(&raw))))
            }
            b'"' => {
                // A binary literal: nothing the importer uses.
                self.at += 1;
                while b.get(self.at).is_some_and(|&ch| ch != b'"') {
                    self.at += 1;
                }
                if self.at >= b.len() {
                    return self.cut_off();
                }
                self.at += 1;
                Ok(Token::Str(String::new()))
            }
            b'.' if !b.get(self.at + 1).is_some_and(u8::is_ascii_digit) => {
                let from = self.at + 1;
                self.at = from;
                while b
                    .get(self.at)
                    .is_some_and(|ch| ch.is_ascii_alphanumeric() || *ch == b'_')
                {
                    self.at += 1;
                }
                if b.get(self.at) != Some(&b'.') || self.at == from {
                    return self.syntax("an enumeration value that isn't written .NAME.");
                }
                let name = String::from_utf8_lossy(&b[from..self.at]).to_ascii_uppercase();
                self.at += 1;
                Ok(Token::Enum(name))
            }
            b'0'..=b'9' | b'+' | b'-' | b'.' => {
                let from = self.at;
                self.at += 1;
                while let Some(&ch) = b.get(self.at) {
                    let after_exponent = matches!(b[self.at - 1], b'e' | b'E');
                    let part = ch.is_ascii_digit()
                        || matches!(ch, b'.' | b'e' | b'E')
                        || (after_exponent && matches!(ch, b'+' | b'-'));
                    if !part {
                        break;
                    }
                    self.at += 1;
                }
                let text = std::str::from_utf8(&b[from..self.at]).unwrap_or("");
                if !text.contains(['.', 'e', 'E'])
                    && let Ok(v) = text.parse::<i64>()
                {
                    return Ok(Token::Int(v));
                }
                match text.parse::<f64>() {
                    Ok(v) if v.is_finite() => Ok(Token::Real(v)),
                    _ => self.syntax("a number that can't be read"),
                }
            }
            _ if c.is_ascii_alphabetic() || c == b'!' => {
                let from = self.at;
                self.at += 1;
                while b
                    .get(self.at)
                    .is_some_and(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'_' | b'-'))
                {
                    self.at += 1;
                }
                Ok(Token::Keyword(
                    String::from_utf8_lossy(&b[from..self.at]).to_ascii_uppercase(),
                ))
            }
            _ => self.syntax("an unexpected character"),
        }
    }
}

/// Decodes the escapes of a Part 21 string (`''` is already undone): `\\`, `\X2\hhhh..\X0\`
/// (UTF-16), `\X4\hhhhhhhh..\X0\` (UTF-32), `\X\hh` and `\S\c` (ISO 8859-1). Anything that
/// isn't a well-formed escape is kept as written.
fn decode_string(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(i) = rest.find('\\') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        if let Some(r) = rest.strip_prefix("\\\\") {
            out.push('\\');
            rest = r;
            continue;
        }
        let wide = rest
            .strip_prefix("\\X2\\")
            .map(|r| (r, 4))
            .or_else(|| rest.strip_prefix("\\X4\\").map(|r| (r, 8)));
        if let Some((r, width)) = wide
            && let Some(end) = r.find("\\X0\\")
            && r[..end].len() % width == 0
            && r[..end].bytes().all(|b| b.is_ascii_hexdigit())
        {
            let codes = r.as_bytes()[..end].chunks(width).map(|chunk| {
                std::str::from_utf8(chunk)
                    .ok()
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                    .unwrap_or(0xFFFD)
            });
            if width == 4 {
                let units: Vec<u16> = codes.map(|c| c as u16).collect();
                out.push_str(&String::from_utf16_lossy(&units));
            } else {
                out.extend(codes.map(|c| char::from_u32(c).unwrap_or('\u{FFFD}')));
            }
            rest = &r[end + 4..];
            continue;
        }
        if let Some(r) = rest.strip_prefix("\\X\\")
            && let Some(hex) = r.get(..2)
            && let Ok(code) = u8::from_str_radix(hex, 16)
        {
            out.push(char::from(code));
            rest = &r[2..];
            continue;
        }
        if let Some(r) = rest.strip_prefix("\\S\\")
            && let Some(c) = r.chars().next().filter(char::is_ascii)
        {
            out.push(char::from(c as u8 | 0x80));
            rest = &r[1..];
            continue;
        }
        out.push('\\');
        rest = &rest[1..];
    }
    out.push_str(rest);
    out
}

struct Parser<'a> {
    lexer: Lexer<'a>,
}

impl Parser<'_> {
    fn expect(&mut self, want: Token, what: &str) -> Result<()> {
        match self.lexer.next()? {
            t if t == want => Ok(()),
            Token::End => self.lexer.cut_off(),
            _ => self.lexer.syntax(what),
        }
    }

    /// The values up to the closing parenthesis (the opening one has been read).
    fn values(&mut self, depth: usize) -> Result<Vec<Value>> {
        if depth > MAX_NESTING {
            return self.lexer.syntax("parentheses nested too deeply");
        }
        let mut out = Vec::new();
        let mut token = self.lexer.next()?;
        if token == Token::Close {
            return Ok(out);
        }
        loop {
            out.push(match token {
                Token::Ref(r) => Value::Ref(r),
                Token::Int(v) => Value::Int(v),
                Token::Real(v) => Value::Real(v),
                Token::Str(s) => Value::Str(s),
                Token::Enum(e) => Value::Enum(e),
                Token::Dollar => Value::Unset,
                Token::Star => Value::Derived,
                Token::Open => Value::List(self.values(depth + 1)?),
                Token::Keyword(k) => {
                    self.expect(Token::Open, "a typed value without its parentheses")?;
                    Value::Typed(k, self.values(depth + 1)?)
                }
                Token::End => return self.lexer.cut_off(),
                _ => {
                    return self
                        .lexer
                        .syntax("a misplaced character in a parameter list");
                }
            });
            match self.lexer.next()? {
                Token::Comma => token = self.lexer.next()?,
                Token::Close => return Ok(out),
                Token::End => return self.lexer.cut_off(),
                _ => {
                    return self
                        .lexer
                        .syntax("a parameter list without a comma or a closing )");
                }
            }
        }
    }

    fn record(&mut self, name: String) -> Result<Record> {
        self.expect(Token::Open, "an entity without its parameter list")?;
        Ok(Record {
            name,
            params: self.values(0)?,
        })
    }
}

/// The DATA section: entity instances by number.
struct File {
    data: HashMap<u32, Vec<Record>>,
}

fn parse(text: &str) -> Result<File> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if text.trim().is_empty() {
        return Err(error(
            "The file is empty, so there is nothing to import. Check that the export finished.",
        ));
    }
    let mut p = Parser {
        lexer: Lexer {
            bytes: text.as_bytes(),
            at: 0,
            start: 0,
        },
    };
    let not_step = || {
        error(
            "This is not a STEP file: it doesn't start with ISO-10303-21. Pick a .step or .stp \
             file (text STEP; compressed and binary variants can't be read).",
        )
    };
    match p.lexer.next() {
        Ok(Token::Keyword(k)) if k == "ISO-10303-21" => {}
        _ => return Err(not_step()),
    }
    // The header: skipped, up to the DATA keyword between statements.
    let mut depth = 0usize;
    loop {
        match p.lexer.next()? {
            Token::Open => depth += 1,
            Token::Close => depth = depth.saturating_sub(1),
            Token::Keyword(k) if k == "DATA" && depth == 0 => break,
            Token::End => {
                return Err(error(
                    "The STEP file has no DATA section: it is cut off before the model starts, \
                     or holds no model. Export it again.",
                ));
            }
            _ => {}
        }
    }
    let mut data: HashMap<u32, Vec<Record>> = HashMap::new();
    'sections: loop {
        // The rest of `DATA;` (a later edition may give it parameters).
        loop {
            match p.lexer.next()? {
                Token::Semi => break,
                Token::End => return p.lexer.cut_off(),
                _ => {}
            }
        }
        loop {
            let id = match p.lexer.next()? {
                Token::Ref(id) => id,
                Token::Keyword(k) if k == "ENDSEC" => break,
                Token::End => return p.lexer.cut_off(),
                _ => {
                    return p
                        .lexer
                        .syntax("something that isn't an entity (#number = ...)");
                }
            };
            p.expect(Token::Equals, "an entity number without an = after it")?;
            let records = match p.lexer.next()? {
                Token::Keyword(name) => vec![p.record(name)?],
                Token::Open => {
                    let mut parts = Vec::new();
                    loop {
                        match p.lexer.next()? {
                            Token::Close => break,
                            Token::Keyword(name) => parts.push(p.record(name)?),
                            Token::End => return p.lexer.cut_off(),
                            _ => return p.lexer.syntax("a broken complex entity"),
                        }
                    }
                    if parts.is_empty() {
                        return p.lexer.syntax("an entity with nothing in it");
                    }
                    parts
                }
                Token::End => return p.lexer.cut_off(),
                _ => return p.lexer.syntax("an entity without a name"),
            };
            p.expect(Token::Semi, "an entity that doesn't end with a semicolon")?;
            if data.insert(id, records).is_some() {
                return Err(damaged(format!("it defines entity #{id} twice")));
            }
        }
        // After ENDSEC: another DATA section, or the end.
        loop {
            match p.lexer.next() {
                Ok(Token::Keyword(k)) if k == "DATA" => continue 'sections,
                Ok(Token::Semi) => {}
                _ => break 'sections,
            }
        }
    }
    Ok(File { data })
}

/// One record of an instance, with the accessors that check parameters.
#[derive(Clone, Copy)]
struct Ent<'a> {
    file: &'a File,
    id: u32,
    rec: &'a Record,
}

impl<'a> Ent<'a> {
    fn name(&self) -> &'a str {
        &self.rec.name
    }

    fn param(&self, i: usize) -> Result<&'a Value> {
        self.rec.params.get(i).ok_or_else(|| {
            damaged(format!(
                "#{} {} has {} parameter{} where at least {} are needed",
                self.id,
                self.rec.name,
                self.rec.params.len(),
                if self.rec.params.len() == 1 { "" } else { "s" },
                i + 1
            ))
        })
    }

    fn bad<T>(&self, i: usize, want: &str) -> Result<T> {
        Err(damaged(format!(
            "parameter {} of #{} {} should be {want}",
            i + 1,
            self.id,
            self.rec.name
        )))
    }

    fn reference(&self, i: usize) -> Result<u32> {
        match self.param(i)? {
            Value::Ref(r) => Ok(*r),
            _ => self.bad(i, "a reference to another entity"),
        }
    }

    /// A reference that may be left out (`$`, or missing at the end).
    fn optional(&self, i: usize) -> Result<Option<u32>> {
        match self.rec.params.get(i) {
            None | Some(Value::Unset | Value::Derived) => Ok(None),
            Some(Value::Ref(r)) => Ok(Some(*r)),
            Some(_) => self.bad(i, "a reference to another entity, or $"),
        }
    }

    fn real(&self, i: usize) -> Result<f64> {
        match number(self.param(i)?) {
            Some(v) => Ok(v),
            None => self.bad(i, "a number"),
        }
    }

    /// A whole number that is not negative (a degree).
    fn count(&self, i: usize) -> Result<usize> {
        match self.param(i)? {
            Value::Int(v) => match usize::try_from(*v) {
                Ok(v) => Ok(v),
                Err(_) => self.bad(i, "a whole number that is not negative"),
            },
            _ => self.bad(i, "a whole number"),
        }
    }

    /// The numbers of a list.
    fn reals_in(&self, i: usize, list: &[Value]) -> Result<Vec<f64>> {
        list.iter()
            .map(|v| match number(v) {
                Some(x) => Ok(x),
                None => self.bad(i, "a list of numbers"),
            })
            .collect()
    }

    fn logical(&self, i: usize) -> Result<bool> {
        match self.param(i)? {
            Value::Enum(e) if e == "T" => Ok(true),
            Value::Enum(e) if e == "F" => Ok(false),
            _ => self.bad(i, ".T. or .F."),
        }
    }

    fn list(&self, i: usize) -> Result<&'a [Value]> {
        match self.param(i)? {
            Value::List(l) => Ok(l),
            _ => self.bad(i, "a list in parentheses"),
        }
    }

    fn refs(&self, i: usize) -> Result<Vec<u32>> {
        self.list(i)?
            .iter()
            .map(|v| match v {
                Value::Ref(r) => Ok(*r),
                _ => self.bad(i, "a list of references to other entities"),
            })
            .collect()
    }

    /// A string parameter; empty when it is anything else.
    fn text(&self, i: usize) -> &'a str {
        match self.rec.params.get(i) {
            Some(Value::Str(s)) => s,
            _ => "",
        }
    }

    /// The entity parameter `i` refers to, which must be one of `kinds`.
    fn child(&self, i: usize, kinds: &[&str]) -> Result<Ent<'a>> {
        self.file.expect(self.reference(i)?, kinds, Some(self))
    }
}

/// A number, also inside a typed value (`LENGTH_MEASURE(1.0)`).
fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Real(x) => Some(*x),
        Value::Int(i) => Some(*i as f64),
        Value::Typed(_, l) => match &l[..] {
            [Value::Real(x)] => Some(*x),
            [Value::Int(i)] => Some(*i as f64),
            _ => None,
        },
        _ => None,
    }
}

/// An instance's name for messages: `A`, or `A+B+C` for a complex one.
fn describe(records: &[Record]) -> String {
    let names: Vec<&str> = records.iter().map(|r| r.name.as_str()).collect();
    names.join("+")
}

impl File {
    fn instance(&self, id: u32, from: Option<&Ent>) -> Result<&[Record]> {
        match self.data.get(&id) {
            Some(records) => Ok(records),
            None => Err(damaged(match from {
                Some(e) => format!(
                    "#{} {} refers to #{id}, which the file doesn't define",
                    e.id, e.rec.name
                ),
                None => format!("it refers to #{id}, which it doesn't define"),
            })),
        }
    }

    /// The record of `id` named one of `kinds` (the instance may be complex).
    fn expect<'a>(&'a self, id: u32, kinds: &[&str], from: Option<&Ent>) -> Result<Ent<'a>> {
        let records = self.instance(id, from)?;
        match records.iter().find(|r| kinds.contains(&r.name.as_str())) {
            Some(rec) => Ok(Ent {
                file: self,
                id,
                rec,
            }),
            None => Err(error(format!(
                "The STEP file has something PeetCAD doesn't expect: #{id} is a {} where a {} \
                 should be{}. The file may be damaged or use a feature PeetCAD can't read.",
                describe(records),
                kinds.join(" or "),
                match from {
                    Some(e) => format!(" (referred to by #{} {})", e.id, e.rec.name),
                    None => String::new(),
                }
            ))),
        }
    }

    /// Like [`File::expect`], for optional structure: `None` instead of an error.
    fn find<'a>(&'a self, id: u32, kinds: &[&str]) -> Option<Ent<'a>> {
        let rec = self
            .data
            .get(&id)?
            .iter()
            .find(|r| kinds.contains(&r.name.as_str()))?;
        Some(Ent {
            file: self,
            id,
            rec,
        })
    }

    /// The instances with a record named one of `kinds`, in order of their numbers.
    fn ids_named(&self, kinds: &[&str]) -> Vec<u32> {
        let mut ids: Vec<u32> = self
            .data
            .iter()
            .filter(|(_, records)| records.iter().any(|r| kinds.contains(&r.name.as_str())))
            .map(|(&id, _)| id)
            .collect();
        ids.sort_unstable();
        ids
    }
}

// ---- Units, names and placements ----

/// What one of a file's units is in the importer's: millimetres per length unit and
/// radians per plane angle unit.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Units {
    length: f64,
    angle: f64,
}

#[derive(Clone, Copy, PartialEq)]
enum UnitKind {
    Length,
    Angle,
}

/// The size of unit `id`: its kind and what one of it is in millimetres or radians.
fn unit_size(file: &File, id: u32, depth: usize) -> Option<(UnitKind, f64)> {
    if depth > MAX_WRAPPERS {
        return None;
    }
    let records = file.data.get(&id)?;
    let has = |name: &str| records.iter().find(|r| r.name == name);
    let kind = if has("LENGTH_UNIT").is_some() {
        UnitKind::Length
    } else if has("PLANE_ANGLE_UNIT").is_some() {
        UnitKind::Angle
    } else {
        return None;
    };
    let size = if let Some(si) = has("SI_UNIT") {
        let prefix = match si.params.first() {
            Some(Value::Enum(p)) => match p.as_str() {
                "EXA" => 1e18,
                "PETA" => 1e15,
                "TERA" => 1e12,
                "GIGA" => 1e9,
                "MEGA" => 1e6,
                "KILO" => 1e3,
                "HECTO" => 1e2,
                "DECA" => 1e1,
                "DECI" => 1e-1,
                "CENTI" => 1e-2,
                "MILLI" => 1e-3,
                "MICRO" => 1e-6,
                "NANO" => 1e-9,
                "PICO" => 1e-12,
                "FEMTO" => 1e-15,
                "ATTO" => 1e-18,
                _ => return None,
            },
            _ => 1.0,
        };
        match (kind, si.params.get(1)) {
            // Metres to millimetres.
            (UnitKind::Length, Some(Value::Enum(n))) if n == "METRE" => prefix * 1000.0,
            (UnitKind::Angle, Some(Value::Enum(n))) if n == "RADIAN" => prefix,
            _ => return None,
        }
    } else {
        let conversion = has("CONVERSION_BASED_UNIT")?;
        let Some(Value::Ref(measure)) = conversion.params.get(1) else {
            return None;
        };
        // A (LENGTH_|PLANE_ANGLE_)MEASURE_WITH_UNIT: a value and the unit it is in.
        let (value, base) = file.data.get(measure)?.iter().find_map(|r| {
            match (r.params.first().and_then(number), r.params.get(1)) {
                (Some(v), Some(Value::Ref(base))) => Some((v, *base)),
                _ => None,
            }
        })?;
        let (base_kind, base_size) = unit_size(file, base, depth + 1)?;
        if base_kind != kind {
            return None;
        }
        value * base_size
    };
    (size.is_finite() && size > 0.0).then_some((kind, size))
}

/// The units a representation context assigns.
fn context_units(file: &File, context: u32) -> Option<Units> {
    let assigned = file.find(context, &["GLOBAL_UNIT_ASSIGNED_CONTEXT"])?;
    let mut length = None;
    let mut angle = None;
    for unit in assigned.refs(0).ok()? {
        match unit_size(file, unit, 0) {
            Some((UnitKind::Length, s)) => length = length.or(Some(s)),
            Some((UnitKind::Angle, s)) => angle = angle.or(Some(s)),
            None => {}
        }
    }
    let mut angle = angle.unwrap_or(1.0);
    // Degrees are usually written with ten digits or so; use the exact value.
    let degree = PI / 180.0;
    if (angle - degree).abs() < 1e-6 * degree {
        angle = degree;
    }
    Some(Units {
        length: length?,
        angle,
    })
}

struct Rep {
    items: Vec<u32>,
    context: u32,
}

/// One way a representation is placed in another: the child's geometry is moved so that
/// its `child_item` placement lands on the parent's `parent_item`.
#[derive(Clone, Copy)]
struct Link {
    child: u32,
    parent: u32,
    /// `None` when the transformation is not given by two axis placements.
    items: Option<(u32, u32)>,
}

struct Placed {
    frames: Vec<Frame>,
    /// Placements that could not be applied.
    unsupported: usize,
}

/// The file's product structure, as far as importing solids needs it: which
/// representation each item is in, which representations are the same shape, how they are
/// placed in one another, and what their products are called.
struct Structure {
    reps: HashMap<u32, Rep>,
    item_reps: HashMap<u32, Vec<u32>>,
    /// Representations tied by a relationship without a transformation are one shape:
    /// each maps to its group's representative.
    group: HashMap<u32, u32>,
    /// Placements of a group (by representative) in parent representations.
    links: HashMap<u32, Vec<Link>>,
    names: HashMap<u32, String>,
    product_name: String,
}

impl Structure {
    fn read(file: &File) -> Self {
        let mut ids: Vec<u32> = file.data.keys().copied().collect();
        ids.sort_unstable();
        let mut s = Self {
            reps: HashMap::new(),
            item_reps: HashMap::new(),
            group: HashMap::new(),
            links: HashMap::new(),
            names: HashMap::new(),
            product_name: String::new(),
        };
        // Representations: any `..REPRESENTATION(name, (items), #context)`.
        for &id in &ids {
            for r in &file.data[&id] {
                if let (true, [_, Value::List(items), Value::Ref(context)]) =
                    (r.name.ends_with("REPRESENTATION"), &r.params[..])
                {
                    let items: Vec<u32> = items
                        .iter()
                        .filter_map(|v| match v {
                            Value::Ref(r) => Some(*r),
                            _ => None,
                        })
                        .collect();
                    for &item in &items {
                        let list = s.item_reps.entry(item).or_default();
                        if !list.contains(&id) {
                            list.push(id);
                        }
                    }
                    s.reps.insert(
                        id,
                        Rep {
                            items,
                            context: *context,
                        },
                    );
                    break;
                }
            }
        }
        // Products, and which product definition each representation belongs to.
        let mut definition_rep: HashMap<u32, u32> = HashMap::new();
        let mut rep_names: Vec<(u32, String)> = Vec::new();
        for &id in &ids {
            if let Some(product) = file.find(id, &["PRODUCT"])
                && s.product_name.is_empty()
            {
                s.product_name = product_name(&product);
            }
            let Some(sdr) = file.find(id, &["SHAPE_DEFINITION_REPRESENTATION"]) else {
                continue;
            };
            let (Ok(shape), Ok(rep)) = (sdr.reference(0), sdr.reference(1)) else {
                continue;
            };
            let Some(definition) = file
                .find(shape, &["PRODUCT_DEFINITION_SHAPE"])
                .and_then(|e| e.reference(2).ok())
            else {
                continue;
            };
            definition_rep.entry(definition).or_insert(rep);
            let name = file
                .find(
                    definition,
                    &[
                        "PRODUCT_DEFINITION",
                        "PRODUCT_DEFINITION_WITH_ASSOCIATED_DOCUMENTS",
                    ],
                )
                .and_then(|e| e.reference(2).ok())
                .and_then(|formation| {
                    file.find(
                        formation,
                        &[
                            "PRODUCT_DEFINITION_FORMATION",
                            "PRODUCT_DEFINITION_FORMATION_WITH_SPECIFIED_SOURCE",
                        ],
                    )
                })
                .and_then(|e| e.reference(2).ok())
                .and_then(|product| file.find(product, &["PRODUCT"]))
                .map(|p| product_name(&p));
            if let Some(name) = name.filter(|n| !n.is_empty()) {
                rep_names.push((rep, name));
            }
        }
        // Which side of a placed relationship is the assembly, where the file says: a
        // CONTEXT_DEPENDENT_SHAPE_REPRESENTATION ties it to an assembly usage.
        let mut usage: HashMap<u32, (u32, u32)> = HashMap::new();
        for &id in &ids {
            let Some(cdsr) = file.find(id, &["CONTEXT_DEPENDENT_SHAPE_REPRESENTATION"]) else {
                continue;
            };
            let found = (|| {
                let relationship = cdsr.reference(0).ok()?;
                let shape = file.find(cdsr.reference(1).ok()?, &["PRODUCT_DEFINITION_SHAPE"])?;
                let occurrence = &file.data.get(&shape.reference(2).ok()?)?[0];
                if !occurrence.name.ends_with("USAGE_OCCURRENCE")
                    && !occurrence.name.ends_with("COMPONENT_USAGE")
                {
                    return None;
                }
                match (occurrence.params.get(3), occurrence.params.get(4)) {
                    (Some(Value::Ref(parent)), Some(Value::Ref(child))) => Some((
                        relationship,
                        (*definition_rep.get(parent)?, *definition_rep.get(child)?),
                    )),
                    _ => None,
                }
            })();
            if let Some((relationship, sides)) = found {
                usage.insert(relationship, sides);
            }
        }
        // Relationships between representations.
        let mut placed: Vec<(u32, u32, u32, u32)> = Vec::new();
        for &id in &ids {
            let records = &file.data[&id];
            let relationship = records.iter().find_map(|r| {
                let known = matches!(
                    r.name.as_str(),
                    "REPRESENTATION_RELATIONSHIP"
                        | "SHAPE_REPRESENTATION_RELATIONSHIP"
                        | "REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION"
                );
                match (known, r.params.get(2), r.params.get(3)) {
                    (true, Some(Value::Ref(a)), Some(Value::Ref(b))) => Some((*a, *b, r)),
                    _ => None,
                }
            });
            let Some((a, b, record)) = relationship else {
                continue;
            };
            if !s.reps.contains_key(&a) || !s.reps.contains_key(&b) || a == b {
                continue;
            }
            let transformation = records
                .iter()
                .find(|r| r.name == "REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION")
                .and_then(|r| {
                    let at = if std::ptr::eq(r, record) { 4 } else { 0 };
                    match r.params.get(at) {
                        Some(Value::Ref(t)) => Some(*t),
                        _ => None,
                    }
                });
            match transformation {
                Some(t) => placed.push((id, a, b, t)),
                None => s.join(a, b),
            }
        }
        for (rep, name) in rep_names {
            let g = s.group_of(rep);
            s.names.entry(g).or_insert(name);
        }
        for (id, a, b, transformation) in placed {
            // By convention the first representation is the part and the second the
            // assembly, and the transformation takes its first item onto its second.
            let (mut child, mut parent) = (a, b);
            if let Some(&(up, down)) = usage.get(&id)
                && s.group_of(up) == s.group_of(a)
                && s.group_of(down) == s.group_of(b)
                && s.group_of(a) != s.group_of(b)
            {
                (child, parent) = (b, a);
            }
            let items = file
                .find(transformation, &["ITEM_DEFINED_TRANSFORMATION"])
                .and_then(|t| Some((t.reference(2).ok()?, t.reference(3).ok()?)))
                .map(|(first, second)| {
                    let (mut from, mut to) = if child == a {
                        (first, second)
                    } else {
                        (second, first)
                    };
                    // The other way round when the items plainly belong the other way.
                    let (in_child, in_parent) = (&s.reps[&child].items, &s.reps[&parent].items);
                    if in_parent.contains(&from)
                        && !in_child.contains(&from)
                        && in_child.contains(&to)
                        && !in_parent.contains(&to)
                    {
                        (from, to) = (to, from);
                    }
                    (from, to)
                });
            let g = s.group_of(child);
            s.links.entry(g).or_default().push(Link {
                child,
                parent,
                items,
            });
        }
        // Mapped items: a representation used as an item of another.
        for &id in &ids {
            let Some(mapped) = file.find(id, &["MAPPED_ITEM"]) else {
                continue;
            };
            let Some(parents) = s.item_reps.get(&id).cloned() else {
                continue;
            };
            let source = mapped
                .reference(1)
                .ok()
                .and_then(|m| file.find(m, &["REPRESENTATION_MAP"]));
            let Some((origin, child)) =
                source.and_then(|m| Some((m.reference(0).ok()?, m.reference(1).ok()?)))
            else {
                continue;
            };
            if !s.reps.contains_key(&child) {
                continue;
            }
            let target = mapped.reference(2).ok();
            let g = s.group_of(child);
            for parent in parents {
                s.links.entry(g).or_default().push(Link {
                    child,
                    parent,
                    items: target.map(|t| (origin, t)),
                });
            }
        }
        s
    }

    fn group_of(&self, rep: u32) -> u32 {
        let mut at = rep;
        // The chains are short; the bound only guards against a corrupt table.
        for _ in 0..self.group.len() + 1 {
            match self.group.get(&at) {
                Some(&next) if next != at => at = next,
                _ => break,
            }
        }
        at
    }

    fn join(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.group_of(a), self.group_of(b));
        let root = ra.min(rb);
        // Point everything on the way at the representative, so chains stay short.
        for mut at in [a, b] {
            while at != root {
                let next = self.group.insert(at, root).unwrap_or(at);
                at = if next == at { root } else { next };
            }
        }
    }

    fn units(&self, file: &File, rep: u32) -> Option<Units> {
        context_units(file, self.reps.get(&rep)?.context)
    }

    /// The units of the file's first context that assigns any: for breps that are in no
    /// representation.
    fn any_units(&self, file: &File) -> Option<Units> {
        file.ids_named(&["GLOBAL_UNIT_ASSIGNED_CONTEXT"])
            .into_iter()
            .find_map(|id| context_units(file, id))
    }

    fn name_of(&self, rep: u32) -> Option<&str> {
        self.names.get(&self.group_of(rep)).map(String::as_str)
    }

    /// Where the geometry of `rep` goes: one frame per occurrence in the assemblies above
    /// it (the identity for a representation that is placed nowhere).
    fn placements(&self, file: &File, rep: u32) -> Placed {
        let mut unsupported = 0;
        let mut memo = HashMap::new();
        let frames = self.placements_of(
            file,
            self.group_of(rep),
            &mut Vec::new(),
            &mut memo,
            &mut unsupported,
        );
        Placed {
            frames,
            unsupported,
        }
    }

    fn placements_of(
        &self,
        file: &File,
        group: u32,
        stack: &mut Vec<u32>,
        memo: &mut HashMap<u32, Vec<Frame>>,
        unsupported: &mut usize,
    ) -> Vec<Frame> {
        if let Some(known) = memo.get(&group) {
            return known.clone();
        }
        let links = match self.links.get(&group) {
            Some(links) if !stack.contains(&group) && stack.len() < MAX_ASSEMBLY_DEPTH => links,
            Some(_) => {
                // An assembly that contains itself, or one nested absurdly deep.
                *unsupported += 1;
                return vec![Frame::WORLD];
            }
            None => return vec![Frame::WORLD],
        };
        stack.push(group);
        let mut out = Vec::new();
        for link in links {
            let own = match self.link_frame(file, link) {
                Some(f) => f,
                None => {
                    *unsupported += 1;
                    Frame::WORLD
                }
            };
            let above =
                self.placements_of(file, self.group_of(link.parent), stack, memo, unsupported);
            for parent in above {
                if out.len() > MAX_BODIES {
                    break;
                }
                out.push(parent.compose(&own));
            }
        }
        stack.pop();
        memo.insert(group, out.clone());
        out
    }

    /// The motion a link applies to the child's geometry (already in millimetres).
    fn link_frame(&self, file: &File, link: &Link) -> Option<Frame> {
        let (from, to) = link.items?;
        let frame = |item: u32, rep: u32| {
            let units = self.units(file, rep).unwrap_or(Units {
                length: 1.0,
                angle: 1.0,
            });
            Builder::new(file, units).placement(item, None).ok()
        };
        let (from, to) = (frame(from, link.child)?, frame(to, link.parent)?);
        let placed = to.compose(&from.inverse());
        (placed.origin.is_finite() && placed.rotation.is_finite()).then_some(placed)
    }
}

/// A `PRODUCT`'s name; its id when the name is empty.
fn product_name(product: &Ent) -> String {
    let name = product.text(1).trim();
    if name.is_empty() {
        product.text(0).trim().to_owned()
    } else {
        name.to_owned()
    }
}

// ---- Building solids ----

/// An edge use in a loop: the edge, and whether the loop runs against it.
type Use = (usize, bool);

#[derive(Clone)]
struct RawEdge {
    curve: Curve3,
    start: usize,
    end: usize,
    t0: f64,
    t1: f64,
}

struct RawFace {
    /// The face's entity, for messages.
    id: u32,
    surface: Surface,
    reversed: bool,
    loops: Vec<Vec<Use>>,
}

/// A loop followed through its face's parameter space.
struct Lift {
    samples: Vec<Sample>,
    /// Whole turns the loop makes around the surface in `u` and in `v`.
    turns: [i32; 2],
}

/// A point of a lifted loop. `coedge` indexes the loop's uses; the sample that closes a
/// loop ending at a pole has `coedge == uses.len()`.
struct Sample {
    coedge: usize,
    t: f64,
    uv: DVec2,
}

/// A loop followed across a freeform surface that closes on itself.
struct FreeLift {
    /// The loop's samples; `coedge` indexes its uses.
    samples: Vec<Sample>,
    /// Whole turns the loop makes round the surface in `u` and in `v`.
    turns: [i32; 2],
    /// The lowest and the highest parameters the loop reaches.
    low: DVec2,
    high: DVec2,
}

/// What a step of bringing a face on a closed freeform surface into the kernel's form
/// did.
enum Unwrapped {
    /// Nothing: the surface does not close on itself (any more).
    Open,
    /// The surface was cut down to the face.
    Trimmed,
    /// The face was cut in two; the other half is the shell's face with this index.
    Cut(usize),
}

/// Where a seam meets a loop: on raw edge `edge` at parameter `t`.
#[derive(Clone, Copy)]
struct Hit {
    edge: usize,
    t: f64,
}

struct Built {
    solid: Solid,
    outer_was_inside_out: bool,
}

/// Reads one brep into raw vertices, edges and faces, brings them into the kernel's form
/// (see the module docs) and emits a [`Solid`].
struct Builder<'a> {
    file: &'a File,
    units: Units,
    vertices: Vec<DVec3>,
    /// Union-find over vertices: merged duplicates point at their representative.
    alias: Vec<usize>,
    vertex_ids: HashMap<u32, usize>,
    edges: Vec<RawEdge>,
    edge_ids: HashMap<u32, usize>,
    shells: Vec<Vec<RawFace>>,
    /// Pairs of duplicate vertices merged.
    merged: usize,
}

impl<'a> Builder<'a> {
    fn new(file: &'a File, units: Units) -> Self {
        Self {
            file,
            units,
            vertices: Vec::new(),
            alias: Vec::new(),
            vertex_ids: HashMap::new(),
            edges: Vec::new(),
            edge_ids: HashMap::new(),
            shells: Vec::new(),
            merged: 0,
        }
    }

    // -- Geometry --

    fn point(&self, id: u32, from: Option<&Ent>) -> Result<DVec3> {
        let e = self.file.expect(id, &["CARTESIAN_POINT"], from)?;
        let p = self.triple(&e)? * self.units.length;
        if !p.is_finite() || p.abs().max_element() > MAX_COORD {
            return Err(error(format!(
                "The STEP file has a point (#{id}) more than {MAX_COORD:e} mm from the origin, \
                 which PeetCAD can't work with. Check the file's units and export it again."
            )));
        }
        Ok(p)
    }

    /// The three coordinates of a point or a direction.
    fn triple(&self, e: &Ent) -> Result<DVec3> {
        match e.list(1)? {
            [x, y, z] => match (number(x), number(y), number(z)) {
                (Some(x), Some(y), Some(z)) => Ok(DVec3::new(x, y, z)),
                _ => e.bad(1, "three numbers"),
            },
            _ => e.bad(1, "three coordinates (PeetCAD reads 3D geometry only)"),
        }
    }

    fn direction(&self, id: u32, from: Option<&Ent>) -> Result<DVec3> {
        let e = self.file.expect(id, &["DIRECTION"], from)?;
        match self.triple(&e)?.try_normalize() {
            Some(d) => Ok(d),
            None => Err(damaged(format!("the direction #{id} has no length"))),
        }
    }

    /// An `AXIS2_PLACEMENT_3D` as a frame. The axis defaults to +Z and the reference
    /// direction to +X (or +Y when the axis is along X), projected square to the axis, as
    /// the standard's `build_axes` has it.
    fn placement(&self, id: u32, from: Option<&Ent>) -> Result<Frame> {
        let e = self.file.expect(id, &["AXIS2_PLACEMENT_3D"], from)?;
        let origin = self.point(e.reference(1)?, Some(&e))?;
        let z = match e.optional(2)? {
            Some(d) => self.direction(d, Some(&e))?,
            None => DVec3::Z,
        };
        let x = match e.optional(3)? {
            Some(d) => Some(self.direction(d, Some(&e))?),
            None => None,
        };
        x.and_then(|x| Frame::from_origin_z_x(origin, z, x))
            .or_else(|| Frame::from_origin_z_x(origin, z, DVec3::X))
            .or_else(|| Frame::from_origin_z_x(origin, z, DVec3::Y))
            .ok_or_else(|| damaged(format!("the placement #{id} has no usable axes")))
    }

    /// A radius or semi-axis in millimetres.
    fn length(&self, e: &Ent, i: usize, least: f64) -> Result<f64> {
        let v = e.real(i)? * self.units.length;
        if v.is_finite() && v >= least && v <= MAX_COORD {
            Ok(v)
        } else {
            Err(error(format!(
                "The STEP file has a {} (#{}) with a size of {v} mm, which is not a usable \
                 shape. The file may be damaged; try exporting it again.",
                e.name().to_lowercase().replace('_', " "),
                e.id
            )))
        }
    }

    fn surface(&self, id: u32, from: &Ent) -> Result<Surface> {
        let records = self.file.instance(id, Some(from))?;
        let e = Ent {
            file: self.file,
            id,
            rec: &records[0],
        };
        if records.iter().any(|r| is_spline(&r.name, "SURFACE")) {
            return self.spline_surface(id, records);
        }
        if records.len() != 1 {
            return Err(unsupported(id, records, "surface"));
        }
        // Radii must be more than the kernel's linear tolerance.
        let least = 2.0 * tolerance::LINEAR;
        let frame = |b: &Self| b.placement(e.reference(1)?, Some(&e));
        Ok(match e.name() {
            "PLANE" => Surface::Plane(Plane {
                frame: frame(self)?,
            }),
            "CYLINDRICAL_SURFACE" => Surface::Cylinder(Cylinder {
                frame: frame(self)?,
                radius: self.length(&e, 2, least)?,
            }),
            "CONICAL_SURFACE" => {
                let half_angle = e.real(3)? * self.units.angle;
                let limit = FRAC_PI_2 - 2.0 * tolerance::ANGULAR;
                if !(half_angle.abs() > 2.0 * tolerance::ANGULAR && half_angle.abs() < limit) {
                    return Err(error(format!(
                        "The STEP file has a cone (#{id}) with a half angle of {half_angle} \
                         radians, which is not a cone. The file may be damaged or name the \
                         wrong angle unit; try exporting it again."
                    )));
                }
                Surface::Cone(Cone {
                    frame: frame(self)?,
                    radius: self.length(&e, 2, 0.0)?,
                    half_angle,
                })
            }
            "SPHERICAL_SURFACE" => Surface::Sphere(Sphere {
                frame: frame(self)?,
                radius: self.length(&e, 2, least)?,
            }),
            "TOROIDAL_SURFACE" | "DEGENERATE_TOROIDAL_SURFACE" => {
                // The kernel's torus with `major < minor` is the outer part of the
                // self-intersecting surface: `select_outer = .T.`.
                if e.name() == "DEGENERATE_TOROIDAL_SURFACE" && !e.logical(4)? {
                    return Err(error(format!(
                        "This STEP file has a face on the inner part of a self-intersecting \
                         torus (entity #{id} DEGENERATE_TOROIDAL_SURFACE), which PeetCAD can't \
                         import yet."
                    )));
                }
                Surface::Torus(Torus {
                    frame: frame(self)?,
                    major: self.length(&e, 2, least)?,
                    minor: self.length(&e, 3, least)?,
                })
            }
            _ => return Err(unsupported(id, records, "surface")),
        })
    }

    /// A curve, and whether it runs the way its entity says (a `TRIMMED_CURVE` may turn
    /// its basis curve round).
    fn curve(&self, id: u32, from: &Ent, depth: usize) -> Result<(Curve3, bool)> {
        if depth > MAX_WRAPPERS {
            return Err(damaged(format!(
                "the curve #{id} is defined in terms of itself"
            )));
        }
        let records = self.file.instance(id, Some(from))?;
        let wrappers = [
            "SURFACE_CURVE",
            "SEAM_CURVE",
            "INTERSECTION_CURVE",
            "BOUNDED_SURFACE_CURVE",
        ];
        if records.iter().any(|r| is_spline(&r.name, "CURVE")) {
            return Ok((self.spline_curve(id, records)?, true));
        }
        let rec = match records {
            [only] => only,
            _ => records
                .iter()
                .find(|r| wrappers.contains(&r.name.as_str()))
                .ok_or_else(|| unsupported(id, records, "curve"))?,
        };
        let e = Ent {
            file: self.file,
            id,
            rec,
        };
        let least = 2.0 * tolerance::LINEAR;
        Ok(match e.name() {
            "LINE" => {
                let origin = self.point(e.reference(1)?, Some(&e))?;
                let vector = e.child(2, &["VECTOR"])?;
                let dir = self.direction(vector.reference(1)?, Some(&vector))?;
                (Curve3::Line(Line3 { origin, dir }), true)
            }
            "CIRCLE" => (
                Curve3::Circle(Circle3 {
                    frame: self.placement(e.reference(1)?, Some(&e))?,
                    radius: self.length(&e, 2, least)?,
                }),
                true,
            ),
            "ELLIPSE" => {
                let frame = self.placement(e.reference(1)?, Some(&e))?;
                let (a, b) = (self.length(&e, 2, least)?, self.length(&e, 3, least)?);
                let curve = if (a - b).abs() <= tolerance::LINEAR {
                    Curve3::Circle(Circle3 { frame, radius: a })
                } else if a > b {
                    Curve3::Ellipse(Ellipse3 {
                        frame,
                        major: a,
                        minor: b,
                    })
                } else {
                    // The kernel's major axis is along X: turn the frame a quarter turn
                    // about Z. The direction of travel stays the same.
                    Curve3::Ellipse(Ellipse3 {
                        frame: Frame {
                            origin: frame.origin,
                            rotation: (frame.rotation * DQuat::from_rotation_z(FRAC_PI_2))
                                .normalize(),
                        },
                        major: b,
                        minor: a,
                    })
                };
                (curve, true)
            }
            name if wrappers.contains(&name) => self.curve(e.reference(1)?, &e, depth + 1)?,
            "TRIMMED_CURVE" => {
                let (curve, forward) = self.curve(e.reference(1)?, &e, depth + 1)?;
                let agrees = !matches!(e.rec.params.get(4), Some(Value::Enum(s)) if s == "F");
                (curve, forward == agrees)
            }
            "PCURVE" => {
                return Err(error(format!(
                    "This STEP file has an edge that is given only as a curve in its surface's \
                     parameters (entity #{id} PCURVE), which PeetCAD can't import yet. Export \
                     the model with its edges as curves in space."
                )));
            }
            _ => return Err(unsupported(id, records, "curve")),
        })
    }

    /// A `SURFACE_OF_LINEAR_EXTRUSION` or `SURFACE_OF_REVOLUTION` as a surface of the
    /// kernel's: the plane, cylinder, cone, sphere or torus it is, where it is one, or
    /// else an exact B-spline surface, long enough for the face bounded by `loops`. Also
    /// returns whether the kernel's surface faces the other way than the file's.
    fn swept_surface(&self, e: &Ent, loops: &[Vec<Use>]) -> Result<(Surface, bool)> {
        let (curve, forward) = self.curve(e.reference(1)?, e, 0)?;
        let curve = if forward { curve } else { turned_round(&curve) };
        // Points of the face, to see how far the surface has to reach.
        let mut points = Vec::new();
        for &(edge, _) in loops.iter().flatten() {
            let edge = &self.edges[edge];
            points.extend((0..=8).map(|k| {
                edge.curve
                    .point(edge.t0 + (edge.t1 - edge.t0) * f64::from(k) / 8.0)
            }));
        }
        let made = if e.name() == "SURFACE_OF_LINEAR_EXTRUSION" {
            let vector = e.child(2, &["VECTOR"])?;
            let dir = self.direction(vector.reference(1)?, Some(&vector))?;
            extruded(&curve, dir, &points)
        } else {
            let axis = e.child(2, &["AXIS1_PLACEMENT"])?;
            let origin = self.point(axis.reference(1)?, Some(&axis))?;
            let dir = match axis.optional(2)? {
                Some(d) => self.direction(d, Some(&axis))?,
                None => DVec3::Z,
            };
            revolved(&curve, origin, dir, &points)
        };
        match made {
            Some((surface, at, normal)) => {
                let flipped = surface.normal_at(at).dot(normal) < 0.0;
                Ok((surface, flipped))
            }
            None => Err(error(format!(
                "This STEP file has a swept surface PeetCAD can't import (entity #{} {}): its \
                 curve is swept along itself, or the face on it has no edges to say how far \
                 it reaches. Try exporting the model again, or from a different program.",
                e.id,
                e.name()
            ))),
        }
    }

    /// A B-spline curve: `B_SPLINE_CURVE_WITH_KNOTS`, `BEZIER_CURVE`,
    /// `QUASI_UNIFORM_CURVE` or `UNIFORM_CURVE`, or the complex instance that adds
    /// weights (`RATIONAL_B_SPLINE_CURVE`) to one of them.
    fn spline_curve(&self, id: u32, records: &'a [Record]) -> Result<Curve3> {
        let file = self.file;
        let part = |name: &str| {
            records
                .iter()
                .find(|r| r.name == name)
                .map(|rec| Ent { file, id, rec })
        };
        // The record with the degree and the control points, and where they start in it:
        // after the name in a simple instance.
        let simple = records.len() == 1;
        let (base, at) = match (simple, part("B_SPLINE_CURVE")) {
            (true, _) => (
                Ent {
                    file,
                    id,
                    rec: &records[0],
                },
                1,
            ),
            (false, Some(e)) => (e, 0),
            (false, None) => return Err(unsupported(id, records, "curve")),
        };
        let degree = base.count(at)?;
        let points = base
            .refs(at + 1)?
            .into_iter()
            .map(|p| self.point(p, Some(&base)))
            .collect::<Result<Vec<_>>>()?;
        check_spline_size(&base, degree, points.len(), "")?;
        let knots = if let Some(k) = part("B_SPLINE_CURVE_WITH_KNOTS") {
            let first = if simple { at + 5 } else { 0 };
            given_knots(&k, first, first + 1, degree, points.len(), "")?
        } else if part("BEZIER_CURVE").is_some() {
            regular_knots(&base, KnotKind::Bezier, degree, points.len())?
        } else if part("QUASI_UNIFORM_CURVE").is_some() {
            regular_knots(&base, KnotKind::QuasiUniform, degree, points.len())?
        } else if part("UNIFORM_CURVE").is_some() {
            regular_knots(&base, KnotKind::Uniform, degree, points.len())?
        } else {
            return Err(no_knots(id, records, "curve"));
        };
        let weights = match part("RATIONAL_B_SPLINE_CURVE") {
            Some(r) if !simple => {
                let weights = r.reals_in(0, r.list(0)?)?;
                if weights.len() != points.len() {
                    return Err(damaged(format!(
                        "#{id} RATIONAL_B_SPLINE_CURVE has {} weights for {} control points",
                        weights.len(),
                        points.len()
                    )));
                }
                Some(weights)
            }
            _ => None,
        };
        match NurbsCurve::from_unclamped(degree, knots, points, weights) {
            Ok(curve) => Ok(Curve3::Nurbs(Arc::new(curve))),
            Err(why) => Err(bad_spline(id, records, "curve", &why)),
        }
    }

    /// A B-spline surface: `B_SPLINE_SURFACE_WITH_KNOTS`, `BEZIER_SURFACE`,
    /// `QUASI_UNIFORM_SURFACE` or `UNIFORM_SURFACE`, or the complex instance that adds
    /// weights (`RATIONAL_B_SPLINE_SURFACE`) to one of them. The control points are a list
    /// of rows, one per step in `u`, each running along `v`: the kernel's order.
    fn spline_surface(&self, id: u32, records: &'a [Record]) -> Result<Surface> {
        let file = self.file;
        let part = |name: &str| {
            records
                .iter()
                .find(|r| r.name == name)
                .map(|rec| Ent { file, id, rec })
        };
        let simple = records.len() == 1;
        let (base, at) = match (simple, part("B_SPLINE_SURFACE")) {
            (true, _) => (
                Ent {
                    file,
                    id,
                    rec: &records[0],
                },
                1,
            ),
            (false, Some(e)) => (e, 0),
            (false, None) => return Err(unsupported(id, records, "surface")),
        };
        let (degree_u, degree_v) = (base.count(at)?, base.count(at + 1)?);
        let rows = base.list(at + 2)?;
        let mut points = Vec::new();
        let mut count_v = None;
        for row in rows {
            let Value::List(row) = row else {
                return base.bad(at + 2, "a list of rows of control points");
            };
            if *count_v.get_or_insert(row.len()) != row.len() {
                return Err(damaged(format!(
                    "the rows of control points of #{id} {} are not all the same length",
                    base.name()
                )));
            }
            for v in row {
                let Value::Ref(p) = v else {
                    return base.bad(at + 2, "a list of rows of references to points");
                };
                points.push(self.point(*p, Some(&base))?);
            }
        }
        let (count_u, count_v) = (rows.len(), count_v.unwrap_or(0));
        check_spline_size(&base, degree_u, count_u, " in u")?;
        check_spline_size(&base, degree_v, count_v, " in v")?;
        let (knots_u, knots_v) = if let Some(k) = part("B_SPLINE_SURFACE_WITH_KNOTS") {
            let first = if simple { at + 7 } else { 0 };
            (
                given_knots(&k, first, first + 2, degree_u, count_u, " in u")?,
                given_knots(&k, first + 1, first + 3, degree_v, count_v, " in v")?,
            )
        } else {
            let kind = if part("BEZIER_SURFACE").is_some() {
                KnotKind::Bezier
            } else if part("QUASI_UNIFORM_SURFACE").is_some() {
                KnotKind::QuasiUniform
            } else if part("UNIFORM_SURFACE").is_some() {
                KnotKind::Uniform
            } else {
                return Err(no_knots(id, records, "surface"));
            };
            (
                regular_knots(&base, kind, degree_u, count_u)?,
                regular_knots(&base, kind, degree_v, count_v)?,
            )
        };
        let weights = match part("RATIONAL_B_SPLINE_SURFACE") {
            Some(r) if !simple => {
                let mut weights = Vec::with_capacity(points.len());
                let rows = r.list(0)?;
                for row in rows {
                    let Value::List(row) = row else {
                        return r.bad(0, "a list of rows of weights");
                    };
                    if row.len() != count_v {
                        break;
                    }
                    weights.extend(r.reals_in(0, row)?);
                }
                if rows.len() != count_u || weights.len() != points.len() {
                    return Err(damaged(format!(
                        "the weights of #{id} RATIONAL_B_SPLINE_SURFACE are not one for each of \
                         its {count_u} by {count_v} control points"
                    )));
                }
                Some(weights)
            }
            _ => None,
        };
        match NurbsSurface::from_unclamped(degree_u, degree_v, knots_u, knots_v, points, weights) {
            Ok(surface) => Ok(Surface::Nurbs(Arc::new(surface))),
            Err(why) => Err(bad_spline(id, records, "surface", &why)),
        }
    }

    // -- Topology --

    fn rep(&self, mut v: usize) -> usize {
        while self.alias[v] != v {
            v = self.alias[v];
        }
        v
    }

    fn add_vertex(&mut self, point: DVec3) -> usize {
        self.vertices.push(point);
        self.alias.push(self.vertices.len() - 1);
        self.vertices.len() - 1
    }

    /// Makes two vertices in the same place one.
    fn merge(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.rep(a), self.rep(b));
        if ra == rb {
            return;
        }
        self.merged += 1;
        let root = ra.min(rb);
        // Point everything on the way at the representative, so chains stay short.
        for mut at in [a, b] {
            while at != root {
                at = std::mem::replace(&mut self.alias[at], root);
            }
        }
    }

    fn vertex(&mut self, id: u32, from: &Ent) -> Result<usize> {
        if let Some(&v) = self.vertex_ids.get(&id) {
            return Ok(v);
        }
        let e = self.file.expect(id, &["VERTEX_POINT"], Some(from))?;
        let point = self.point(e.reference(1)?, Some(&e))?;
        let v = self.add_vertex(point);
        self.vertex_ids.insert(id, v);
        Ok(v)
    }

    fn edge(&mut self, id: u32, from: &Ent) -> Result<usize> {
        if let Some(&e) = self.edge_ids.get(&id) {
            return Ok(e);
        }
        let e = self.file.expect(id, &["EDGE_CURVE"], Some(from))?;
        let start = self.vertex(e.reference(1)?, &e)?;
        let end = self.vertex(e.reference(2)?, &e)?;
        let (mut curve, forward) = self.curve(e.reference(3)?, &e, 0)?;
        if e.logical(4)? != forward {
            curve = turned_round(&curve);
        }
        let (a, b) = (self.vertices[self.rep(start)], self.vertices[self.rep(end)]);
        let coincide = a.distance(b) <= VERTEX_MERGE;
        let (t0, t1) = match curve {
            Curve3::Line(_) => {
                if coincide {
                    return Err(error(format!(
                        "The STEP file has a straight edge with no length (#{id}), which \
                         PeetCAD can't import. Try exporting the model again."
                    )));
                }
                // A line edge is the stretch between its vertices, whichever way the
                // file says the line runs.
                if curve.param(b) < curve.param(a) {
                    curve = turned_round(&curve);
                }
                (curve.param(a), curve.param(b))
            }
            Curve3::Circle(_) | Curve3::Ellipse(_) => {
                let t0 = curve.param(a);
                if coincide {
                    // A closed edge; two vertices in one place are one written twice.
                    self.merge(start, end);
                    (t0, t0 + TAU)
                } else {
                    let t1 = curve.param(b);
                    (t0, if t1 <= t0 { t1 + TAU } else { t1 })
                }
            }
            Curve3::Nurbs(ref spline) => {
                if coincide {
                    self.merge(start, end);
                }
                let (spline, t0, t1) = spline_edge(id, spline, a, b, coincide)?;
                curve = Curve3::Nurbs(spline);
                (t0, t1)
            }
        };
        self.edges.push(RawEdge {
            curve,
            start,
            end,
            t0,
            t1,
        });
        self.edge_ids.insert(id, self.edges.len() - 1);
        Ok(self.edges.len() - 1)
    }

    fn use_start(&self, (edge, reversed): Use) -> usize {
        let e = &self.edges[edge];
        self.rep(if reversed { e.end } else { e.start })
    }

    fn use_end(&self, (edge, reversed): Use) -> usize {
        self.use_start((edge, !reversed))
    }

    fn face(&mut self, id: u32, from: &Ent) -> Result<RawFace> {
        let e = self
            .file
            .expect(id, &["ADVANCED_FACE", "FACE_SURFACE"], Some(from))?;
        let bounds = e.refs(1)?;
        // A swept surface is made to fit the face: it waits for the face's edges.
        let swept = self.file.find(
            e.reference(2)?,
            &["SURFACE_OF_LINEAR_EXTRUSION", "SURFACE_OF_REVOLUTION"],
        );
        let surface = match swept {
            Some(_) => None,
            None => Some(self.surface(e.reference(2)?, &e)?),
        };
        let mut reversed = !e.logical(3)?;
        let mut loops = Vec::new();
        for bound in bounds {
            let bound = self
                .file
                .expect(bound, &["FACE_BOUND", "FACE_OUTER_BOUND"], Some(&e))?;
            let forward = bound.logical(2)?;
            let l = bound.child(1, &["EDGE_LOOP", "VERTEX_LOOP", "POLY_LOOP"])?;
            match l.name() {
                // A vertex loop marks a point the face closes onto (a cone's apex) or
                // stands for "no boundary" (a whole sphere); see `close_periodic`.
                "VERTEX_LOOP" => continue,
                "POLY_LOOP" => {
                    return Err(error(format!(
                        "This STEP file has faceted faces (entity #{} POLY_LOOP), which \
                         PeetCAD can't import. Export the model as a solid with exact faces.",
                        l.id
                    )));
                }
                _ => {}
            }
            let mut uses = Vec::new();
            for oriented in l.refs(1)? {
                let oriented = self.file.expect(oriented, &["ORIENTED_EDGE"], Some(&l))?;
                let against = !oriented.logical(4)?;
                uses.push((self.edge(oriented.reference(3)?, &oriented)?, against));
            }
            if uses.is_empty() {
                return Err(damaged(format!("the loop #{} has no edges", l.id)));
            }
            if !forward {
                uses.reverse();
                for u in &mut uses {
                    u.1 = !u.1;
                }
            }
            // Each edge must start where the one before it ended.
            for i in 0..uses.len() {
                let (a, b) = (
                    self.use_end(uses[i]),
                    self.use_start(uses[(i + 1) % uses.len()]),
                );
                if a == b {
                    continue;
                }
                let gap = self.vertices[a].distance(self.vertices[b]);
                if gap > VERTEX_MERGE {
                    return Err(error(format!(
                        "The STEP file has a face boundary that isn't closed (loop #{}: its \
                         edges leave a gap of {gap:.3e} mm), so the body is not a solid. Try \
                         exporting it again.",
                        l.id
                    )));
                }
                self.merge(a, b);
            }
            loops.push(uses);
        }
        let surface = match (surface, swept) {
            (Some(surface), _) => surface,
            (None, Some(swept)) => {
                let (surface, flipped) = self.swept_surface(&swept, &loops)?;
                reversed ^= flipped;
                surface
            }
            (None, None) => self.surface(e.reference(2)?, &e)?,
        };
        Ok(RawFace {
            id,
            surface,
            reversed,
            loops,
        })
    }

    fn shell(&mut self, id: u32, from: &Ent) -> Result<()> {
        let mut e = self.file.expect(
            id,
            &["CLOSED_SHELL", "ORIENTED_CLOSED_SHELL", "OPEN_SHELL"],
            Some(from),
        )?;
        if e.name() == "ORIENTED_CLOSED_SHELL" {
            // Which way round it is used is settled by measuring the shell.
            e = e.child(2, &["CLOSED_SHELL"])?;
        }
        if e.name() == "OPEN_SHELL" {
            return Err(error(format!(
                "This STEP file has a body made of an open shell (entity #{id}), which is a \
                 surface body. PeetCAD imports solids only; export the model as a solid."
            )));
        }
        let mut faces = Vec::new();
        for face in e.refs(1)? {
            faces.push(self.face(face, &e)?);
        }
        if faces.is_empty() {
            return Err(damaged(format!("the shell #{} has no faces", e.id)));
        }
        self.shells.push(faces);
        Ok(())
    }

    fn brep(&mut self, brep: &Ent) -> Result<Built> {
        self.shell(brep.reference(1)?, brep)?;
        if brep.name() == "BREP_WITH_VOIDS" {
            for void in brep.refs(2)? {
                self.shell(void, brep)?;
            }
        }
        self.split_at_poles()?;
        for s in 0..self.shells.len() {
            // Cutting a face in two adds one to the shell: the count is not fixed.
            let mut f = 0;
            while f < self.shells[s].len() {
                if self.shells[s][f].surface.is_periodic_u() {
                    self.close_periodic(s, f)?;
                } else if matches!(self.shells[s][f].surface, Surface::Nurbs(_)) {
                    self.refuse_pinched(s, f)?;
                    self.unwrap_freeform(s, f)?;
                }
                self.outer_loop_first(s, f)?;
                f += 1;
            }
        }
        let mut emitter = Emitter::default();
        for faces in &self.shells {
            let shell = emitter.solid.add_shell();
            for face in faces {
                let f = emitter
                    .solid
                    .add_face(shell, face.surface.clone(), face.reversed);
                for uses in &face.loops {
                    let uses = emitter.uses(self, uses);
                    emitter.solid.add_loop(f, &uses);
                }
            }
        }
        // Outer shell positive, voids negative.
        let mut solid = emitter.solid;
        let mut outer_was_inside_out = false;
        for k in 0..solid.shells.len() {
            let shell = ShellId(k as u32);
            let volume = measure::shell_volume(&solid, shell);
            if (volume < 0.0) == (k == 0) && volume != 0.0 {
                turn_inside_out(&mut solid, shell);
                outer_was_inside_out |= k == 0;
            }
        }
        Ok(Built {
            solid,
            outer_was_inside_out,
        })
    }

    // -- Bringing faces into the kernel's form --

    /// Splits `edge` at parameter `t` (strictly inside its range) with a new vertex, in
    /// every loop that uses it. Returns the vertex.
    fn split_edge(&mut self, edge: usize, t: f64) -> Result<usize> {
        let old = self.edges[edge].clone();
        if !(t > old.t0 && t < old.t1) {
            return Err(error(
                "PeetCAD could not split an edge of this STEP file where a seam meets it. \
                 Try exporting the model again, or from a different program.",
            ));
        }
        let vertex = self.add_vertex(old.curve.point(t));
        self.edges[edge].end = vertex;
        self.edges[edge].t1 = t;
        let second = self.edges.len();
        self.edges.push(RawEdge {
            start: vertex,
            t0: t,
            ..old
        });
        for face in self.shells.iter_mut().flatten() {
            for uses in &mut face.loops {
                if !uses.iter().any(|u| u.0 == edge) {
                    continue;
                }
                let mut out = Vec::with_capacity(uses.len() + 2);
                for &(e, reversed) in uses.iter() {
                    match (e == edge, reversed) {
                        (false, _) => out.push((e, reversed)),
                        (true, false) => out.extend([(edge, false), (second, false)]),
                        (true, true) => out.extend([(second, true), (edge, true)]),
                    }
                }
                *uses = out;
            }
        }
        Ok(vertex)
    }

    /// An edge must not pass through a pole of its face's surface: splits those that do.
    fn split_at_poles(&mut self) -> Result<()> {
        // An edge can pass through both poles of a sphere: the second pass finds the other.
        for _ in 0..4 {
            let mut todo: Vec<(usize, f64)> = Vec::new();
            let mut seen: HashSet<usize> = HashSet::new();
            for face in self.shells.iter().flatten() {
                let poles: Vec<DVec3> = face
                    .surface
                    .poles()
                    .into_iter()
                    .flatten()
                    .map(|p| face.surface.point(DVec2::new(0.0, p.v)))
                    .collect();
                if poles.is_empty() {
                    continue;
                }
                for &(e, _) in face.loops.iter().flatten() {
                    if seen.contains(&e) {
                        continue;
                    }
                    let edge = &self.edges[e];
                    let (a, b) = (
                        self.vertices[self.rep(edge.start)],
                        self.vertices[self.rep(edge.end)],
                    );
                    for &pole in &poles {
                        let mut t = edge.curve.param(pole);
                        if edge.curve.period().is_some() {
                            t += ((edge.t0 - t) / TAU).ceil() * TAU;
                        }
                        if t > edge.t0
                            && t < edge.t1
                            && edge.curve.point(t).distance(pole) <= POLE_ON_EDGE
                            && pole.distance(a) > SEAM_SNAP
                            && pole.distance(b) > SEAM_SNAP
                        {
                            seen.insert(e);
                            todo.push((e, t));
                            break;
                        }
                    }
                }
            }
            if todo.is_empty() {
                break;
            }
            for (e, t) in todo {
                self.split_edge(e, t)?;
            }
        }
        Ok(())
    }

    /// Follows a loop through its surface's parameter space, the way the kernel does (see
    /// "Lifting loops into parameter space" in [`peet_kernel::geom`]).
    fn lift(&self, surface: &Surface, reversed: bool, uses: &[Use]) -> Result<Lift> {
        let ccw = !reversed;
        let mut samples = Vec::new();
        let mut at: Option<DVec2> = None;
        let mut start = DVec2::ZERO;
        let mut end_pole = None;
        for (i, &(e, against)) in uses.iter().enumerate() {
            let edge = &self.edges[e];
            let (ta, tb) = if against {
                (edge.t1, edge.t0)
            } else {
                (edge.t0, edge.t1)
            };
            let (raw, pole) = surface.param_toward(&edge.curve, ta, tb);
            let mut uv = match (at, pole) {
                (None, _) => {
                    start = raw;
                    raw
                }
                (Some(prev), Some(pole)) => {
                    DVec2::new(pole_exit(prev.x, raw.x, pole.top, ccw), pole.v)
                }
                (Some(prev), None) => prev,
            };
            samples.push(Sample {
                coedge: i,
                t: ta,
                uv,
            });
            let steps = match edge.curve {
                Curve3::Line(_) => LINE_STEPS,
                // A B-spline's parameter says nothing about how far it goes.
                Curve3::Nurbs(_) => freeform_steps(&edge.curve, ta, tb),
                _ => ((tb - ta).abs() / SAMPLE_STEP).ceil().clamp(1.0, 256.0) as usize,
            };
            for k in 1..steps {
                let t = ta + (tb - ta) * k as f64 / steps as f64;
                uv = surface.param_near(surface.param(edge.curve.point(t)), uv);
                samples.push(Sample { coedge: i, t, uv });
            }
            let (raw, pole) = surface.param_toward(&edge.curve, tb, ta);
            uv = surface.param_near(raw, uv);
            samples.push(Sample {
                coedge: i,
                t: tb,
                uv,
            });
            at = Some(uv);
            end_pole = pole;
        }
        let mut end = at.unwrap_or(start);
        if let Some(pole) = end_pole {
            end.x = pole_exit(end.x, start.x, pole.top, ccw);
            samples.push(Sample {
                coedge: uses.len(),
                t: samples[0].t,
                uv: end,
            });
        }
        let mut turns = [0; 2];
        for (axis, turns) in turns.iter_mut().enumerate() {
            let winding = (end - start)[axis] / TAU;
            let periodic = axis == 0 || surface.is_periodic_v();
            let whole = if periodic { winding.round() } else { 0.0 };
            if !((winding - whole).abs() * TAU <= WINDING_SLACK && whole.abs() <= 8.0) {
                return Err(error(
                    "The STEP file has a face whose boundary doesn't close up on its surface, \
                     so the body is not a solid PeetCAD can import. Try exporting it again.",
                ));
            }
            *turns = whole as i32;
        }
        Ok(Lift { samples, turns })
    }

    /// Where a lifted loop crosses the line `uv[axis] = target` (in whole turns of it),
    /// and how close it otherwise comes to it. `None` when an edge runs along the line.
    fn crossings(
        &self,
        surface: &Surface,
        uses: &[Use],
        lift: &Lift,
        axis: usize,
        target: f64,
    ) -> Option<(Vec<Hit>, f64)> {
        let samples = &lift.samples;
        let offsets: Vec<f64> = samples.iter().map(|s| wrap(s.uv[axis] - target)).collect();
        let mut above: Vec<bool> = offsets.iter().map(|&f| f >= 0.0).collect();
        // The loop ends where it started: one point, one side.
        if let Some(&first) = above.first()
            && let Some(last) = above.last_mut()
        {
            *last = first;
        }
        let at_start = |coedge: usize| {
            let (edge, against) = uses[coedge % uses.len()];
            let e = &self.edges[edge];
            Hit {
                edge,
                t: if against { e.t1 } else { e.t0 },
            }
        };
        let mut hits = Vec::new();
        for j in 0..samples.len().saturating_sub(1) {
            let (a, b) = (&samples[j], &samples[j + 1]);
            if a.coedge != b.coedge {
                // A vertex. The loop only moves there at a pole, along the pole's line.
                let (lo, hi) = (a.uv[axis].min(b.uv[axis]), a.uv[axis].max(b.uv[axis]));
                if hi - lo > ALONG_SEAM {
                    let mut x = target + ((lo - target) / TAU).ceil() * TAU;
                    while x <= hi && hits.len() < 64 {
                        hits.push(at_start(b.coedge));
                        x += TAU;
                    }
                }
                continue;
            }
            if offsets[j].abs() < ALONG_SEAM && offsets[j + 1].abs() < ALONG_SEAM {
                return None;
            }
            // A change of side far from the line is the jump half a turn away from it.
            if above[j] != above[j + 1]
                && offsets[j].abs() < FRAC_PI_2
                && offsets[j + 1].abs() < FRAC_PI_2
            {
                let (edge, _) = uses[a.coedge];
                let curve = &self.edges[edge].curve;
                let side = |t: f64| wrap(surface.param(curve.point(t))[axis] - target) >= 0.0;
                let (mut lo, mut hi) = (a.t, b.t);
                for _ in 0..60 {
                    let mid = 0.5 * (lo + hi);
                    if side(mid) == above[j] {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                hits.push(Hit {
                    edge,
                    t: 0.5 * (lo + hi),
                });
            }
        }
        let clearance = offsets.iter().fold(f64::INFINITY, |m, f| m.min(f.abs()));
        Some((hits, clearance))
    }

    /// A value of `uv[axis]` at which a seam crosses each of the loops `through` exactly
    /// once and stays clear of the loops `clear`; the crossings come with it.
    fn seam_position(
        &self,
        face: &RawFace,
        lifts: &[Lift],
        through: &[usize],
        axis: usize,
    ) -> Result<(f64, Vec<Hit>)> {
        // The loops' own vertices first, so that the seam splits as few edges as possible:
        // those where every loop has one before the others.
        let corners: Vec<Vec<f64>> = through
            .iter()
            .map(|&l| {
                let samples = &lifts[l].samples;
                (0..samples.len())
                    .filter(|&j| j == 0 || samples[j - 1].coedge != samples[j].coedge)
                    .map(|j| samples[j].uv[axis])
                    .collect()
            })
            .collect();
        let shared = |x: f64| {
            corners
                .iter()
                .all(|list| list.iter().any(|&y| wrap(x - y).abs() < ALONG_SEAM))
        };
        let mut candidates: Vec<f64> = corners.iter().flatten().copied().collect();
        candidates.sort_by_key(|&x| !shared(x));
        let base = candidates.first().copied().unwrap_or(0.0);
        if candidates.is_empty() {
            candidates.push(0.0);
        }
        candidates
            .extend((0..SEAM_TRIES).map(|k| base + (k as f64 + 0.37) * TAU / SEAM_TRIES as f64));
        'candidates: for target in candidates {
            let mut hits = Vec::new();
            for (l, lift) in lifts.iter().enumerate() {
                let Some((found, clearance)) =
                    self.crossings(&face.surface, &face.loops[l], lift, axis, target)
                else {
                    continue 'candidates;
                };
                if through.contains(&l) {
                    if found.len() != 1 {
                        continue 'candidates;
                    }
                    hits.push(found[0]);
                } else if !found.is_empty() || clearance < SEAM_CLEARANCE {
                    continue 'candidates;
                }
            }
            if hits.len() == 2 && hits[0].edge == hits[1].edge {
                continue;
            }
            return Ok((target, hits));
        }
        Err(error(format!(
            "The STEP file has a face that goes all the way round its surface without a seam \
             (entity #{}), and PeetCAD could not find a place to put one. Try exporting the \
             model from a different program, or with periodic faces split.",
            face.id
        )))
    }

    /// The vertex where a seam meets a loop: an existing one when it is close enough,
    /// otherwise a new one that splits the edge.
    fn vertex_at(&mut self, hit: Hit) -> Result<usize> {
        let edge = &self.edges[hit.edge];
        let p = edge.curve.point(hit.t);
        let (start, end) = (self.rep(edge.start), self.rep(edge.end));
        if p.distance(self.vertices[start]) <= SEAM_SNAP {
            Ok(start)
        } else if p.distance(self.vertices[end]) <= SEAM_SNAP {
            Ok(end)
        } else {
            self.split_edge(hit.edge, hit.t)
        }
    }

    /// A loop's uses, starting from `vertex`.
    fn starting_at(&self, face: u32, uses: &[Use], vertex: usize) -> Result<Vec<Use>> {
        match uses.iter().position(|&u| self.use_start(u) == vertex) {
            Some(i) => {
                let mut out = uses.to_vec();
                out.rotate_left(i);
                Ok(out)
            }
            None => Err(seam_failed(face)),
        }
    }

    /// Adds an arc edge from vertex `a` to vertex `b`: the circle `frame`/`radius` from
    /// the angle `from` over `sweep` (negative: clockwise).
    fn arc(
        &mut self,
        frame: Frame,
        radius: f64,
        a: usize,
        b: usize,
        from: f64,
        sweep: f64,
    ) -> usize {
        let (frame, t0) = if sweep < 0.0 {
            (half_turn(&frame), -from)
        } else {
            (frame, from)
        };
        self.edges.push(RawEdge {
            curve: Curve3::Circle(Circle3 { frame, radius }),
            start: a,
            end: b,
            t0,
            t1: t0 + sweep.abs(),
        });
        self.edges.len() - 1
    }

    /// Adds the seam edge from vertex `a` to vertex `b` of `face`: along the meridian
    /// (`axis == 0`) or the parallel (`axis == 1`) through `a`. On a surface closed in
    /// that direction it goes the way `direction` says.
    fn seam(
        &mut self,
        surface: &Surface,
        face: u32,
        axis: usize,
        a: usize,
        b: usize,
        direction: f64,
    ) -> Result<usize> {
        let (pa, pb) = (self.vertices[a], self.vertices[b]);
        if matches!(surface, Surface::Cylinder(_) | Surface::Cone(_)) {
            let curve = Curve3::line_through(pa, pb).ok_or_else(|| seam_failed(face))?;
            self.edges.push(RawEdge {
                curve,
                start: a,
                end: b,
                t0: 0.0,
                t1: pa.distance(pb),
            });
            return Ok(self.edges.len() - 1);
        }
        // The end at a pole has no angle of its own: take the other end's.
        let (mut ua, mut ub) = (surface.param(pa), surface.param(pb));
        if surface.pole_at(pa).is_some() {
            ua.x = ub.x;
        }
        if surface.pole_at(pb).is_some() {
            ub.x = ua.x;
        }
        let along = 1 - axis;
        let closed = along == 0 || surface.is_periodic_v();
        let sweep = if closed {
            let s = (direction * (ub[along] - ua[along])).rem_euclid(TAU);
            direction * if s < WINDING_SLACK { TAU } else { s }
        } else {
            ub[along] - ua[along]
        };
        let circle = if axis == 0 {
            meridian_circle(surface, ua.x)
        } else {
            parallel_circle(surface, ua.y)
        };
        match circle {
            Some((frame, radius)) if sweep.abs() > WINDING_SLACK => {
                Ok(self.arc(frame, radius, a, b, ua[along], sweep))
            }
            _ => Err(seam_failed(face)),
        }
    }

    /// Makes a face on a surface of revolution simply connected in `(u, v)`: joins loops
    /// that go round the surface with seams, and gives a whole sphere or torus its seams
    /// (see the module docs).
    fn close_periodic(&mut self, s: usize, f: usize) -> Result<()> {
        let (surface, reversed) = {
            let face = &self.shells[s][f];
            (face.surface.clone(), face.reversed)
        };
        let mut lifts = Vec::new();
        for uses in &self.shells[s][f].loops {
            lifts.push(self.lift(&surface, reversed, uses)?);
        }
        let round: Vec<usize> = (0..lifts.len())
            .filter(|&l| lifts[l].turns != [0, 0])
            .collect();
        // The face is on the left of its loops, seen from outside: that says which way a
        // seam leaves a loop that goes round the surface.
        let side = if reversed { -1.0 } else { 1.0 };
        let face = &self.shells[s][f];
        let id = face.id;
        let unsupported_shape = || {
            error(format!(
                "The STEP file has a face (entity #{id}) that wraps around its surface in a way \
                 PeetCAD can't import yet. Try exporting the model from a different program, \
                 or with periodic faces split."
            ))
        };
        match round[..] {
            [] => {
                if face
                    .loops
                    .iter()
                    .any(|uses| self.loop_area(face, uses) > 0.0)
                {
                    return Ok(());
                }
                // No outer loop: the whole surface, less any holes.
                match surface {
                    Surface::Sphere(_) => {
                        let (u, _) = self.seam_position(face, &lifts, &[], 0)?;
                        let south = self.add_vertex(surface.point(DVec2::new(u, -FRAC_PI_2)));
                        let north = self.add_vertex(surface.point(DVec2::new(u, FRAC_PI_2)));
                        let (frame, radius) =
                            meridian_circle(&surface, u).ok_or_else(unsupported_shape)?;
                        let seam = self.arc(frame, radius, south, north, -FRAC_PI_2, PI);
                        self.shells[s][f]
                            .loops
                            .push(vec![(seam, false), (seam, true)]);
                    }
                    Surface::Torus(_) => {
                        let (u, _) = self.seam_position(face, &lifts, &[], 0)?;
                        let (v, _) = self.seam_position(face, &lifts, &[], 1)?;
                        let (around, tube) = meridian_circle(&surface, u)
                            .zip(parallel_circle(&surface, v))
                            .map(|(m, p)| (p, m))
                            .ok_or_else(unsupported_shape)?;
                        let corner = self.add_vertex(surface.point(DVec2::new(u, v)));
                        let around = self.arc(around.0, around.1, corner, corner, u, TAU);
                        let tube = self.arc(tube.0, tube.1, corner, corner, v, TAU);
                        // Counter-clockwise in (u, v) for a face along the natural normal.
                        let mut uses =
                            vec![(around, false), (tube, false), (around, true), (tube, true)];
                        if reversed {
                            uses =
                                vec![(tube, false), (around, false), (tube, true), (around, true)];
                        }
                        self.shells[s][f].loops.push(uses);
                    }
                    _ => {
                        return Err(error(format!(
                            "The STEP file has a face without an outer boundary (entity #{id}), \
                             so the body is not a solid PeetCAD can import. Try exporting it \
                             again."
                        )));
                    }
                }
            }
            [l] => {
                // One loop round the axis: the face closes onto a pole on its left.
                let turns = lifts[l].turns;
                if turns[1] != 0 || turns[0].abs() != 1 {
                    return Err(unsupported_shape());
                }
                let up = f64::from(turns[0]) * side > 0.0;
                let pole = match surface {
                    Surface::Sphere(_) => {
                        if up {
                            FRAC_PI_2
                        } else {
                            -FRAC_PI_2
                        }
                    }
                    Surface::Cone(c) => c.apex_v(),
                    _ => return Err(unsupported_shape()),
                };
                let (u, hits) = self.seam_position(face, &lifts, &[l], 0)?;
                let apex = surface.point(DVec2::new(u, pole));
                let from = self.vertex_at(hits[0])?;
                let apex = self.add_vertex(apex);
                let seam = self.seam(&surface, id, 0, from, apex, 1.0)?;
                let mut uses = self.starting_at(id, &self.shells[s][f].loops[l], from)?;
                uses.extend([(seam, false), (seam, true)]);
                self.shells[s][f].loops[l] = uses;
            }
            [la, lb] => {
                // Two loops round the surface the same way: a band between them.
                let (ta, tb) = (lifts[la].turns, lifts[lb].turns);
                let axis = if ta[1] == 0 && tb[1] == 0 {
                    0
                } else if ta[0] == 0 && tb[0] == 0 {
                    1
                } else {
                    return Err(unsupported_shape());
                };
                if ta[axis].abs() != 1 || tb[axis].abs() != 1 {
                    return Err(unsupported_shape());
                }
                // Travelling along +u the left is +v; along +v it is −u.
                let direction = f64::from(ta[axis]) * side * if axis == 0 { 1.0 } else { -1.0 };
                let (_, hits) = self.seam_position(face, &lifts, &[la, lb], axis)?;
                let a = self.vertex_at(hits[0])?;
                let b = self.vertex_at(hits[1])?;
                let seam = self.seam(&surface, id, axis, a, b, direction)?;
                let loops = &self.shells[s][f].loops;
                let mut uses = self.starting_at(id, &loops[la], a)?;
                uses.push((seam, false));
                uses.extend(self.starting_at(id, &loops[lb], b)?);
                uses.push((seam, true));
                let loops = &mut self.shells[s][f].loops;
                loops[la] = uses;
                loops.remove(lb);
            }
            _ => return Err(unsupported_shape()),
        }
        Ok(())
    }

    /// Follows a loop across a freeform surface that closes on itself: every sample takes
    /// the parameters closest to those of the one before.
    fn lift_freeform(&self, closed: &Closed, uses: &[Use]) -> FreeLift {
        let mut samples: Vec<Sample> = Vec::new();
        let mut near = None;
        for (i, &(e, against)) in uses.iter().enumerate() {
            let edge = &self.edges[e];
            let (ta, tb) = if against {
                (edge.t1, edge.t0)
            } else {
                (edge.t0, edge.t1)
            };
            let steps = freeform_steps(&edge.curve, ta, tb);
            for k in 0..=steps {
                let t = if k == steps {
                    tb
                } else {
                    ta + (tb - ta) * k as f64 / steps as f64
                };
                let uv = closed.lifted(edge.curve.point(t), near);
                samples.push(Sample { coedge: i, t, uv });
                near = Some(uv);
            }
        }
        let mut turns = [0; 2];
        let (mut low, mut high) = (DVec2::INFINITY, DVec2::NEG_INFINITY);
        for sample in &samples {
            low = low.min(sample.uv);
            high = high.max(sample.uv);
        }
        if let (Some(first), Some(last)) = (samples.first(), samples.last()) {
            for (axis, turns) in turns.iter_mut().enumerate() {
                if let Some(period) = closed.periods[axis] {
                    *turns = ((last.uv[axis] - first.uv[axis]) / period).round() as i32;
                }
            }
        }
        FreeLift {
            samples,
            turns,
            low,
            high,
        }
    }

    /// Where a loop followed across a closed freeform surface crosses the line
    /// `uv[axis] = cut`. `None` when an edge runs along the line.
    fn freeform_crossings(
        &self,
        closed: &Closed,
        uses: &[Use],
        lift: &FreeLift,
        axis: usize,
        cut: f64,
    ) -> Option<Vec<Hit>> {
        let along = 1e-9 * closed.periods[axis]?;
        let above = |x: f64| x >= cut;
        let mut hits = Vec::new();
        for pair in lift.samples.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            if a.coedge != b.coedge {
                // The same vertex, seen from both its edges.
                continue;
            }
            let (xa, xb) = (a.uv[axis], b.uv[axis]);
            if (xa - cut).abs() < along && (xb - cut).abs() < along {
                return None;
            }
            if above(xa) == above(xb) {
                continue;
            }
            let (edge, _) = uses[a.coedge];
            let curve = &self.edges[edge].curve;
            let (mut ta, mut tb) = (a.t, b.t);
            for _ in 0..48 {
                let mid = 0.5 * (ta + tb);
                let uv = closed.lifted(curve.point(mid), Some(a.uv));
                if above(uv[axis]) == above(xa) {
                    ta = mid;
                } else {
                    tb = mid;
                }
            }
            hits.push(Hit {
                edge,
                t: 0.5 * (ta + tb),
            });
            if hits.len() > 8 {
                return None;
            }
        }
        Some(hits)
    }

    /// Refuses a face that comes to a point where its B-spline surface is pinched
    /// together (one of the surface's edges is a single point: the tip of a curve turned
    /// about an axis it ends on, a three-sided patch). The kernel's freeform faces can't
    /// have such a corner yet: their parameters mean nothing there.
    fn refuse_pinched(&self, s: usize, f: usize) -> Result<()> {
        let face = &self.shells[s][f];
        let Surface::Nurbs(surface) = &face.surface else {
            return Ok(());
        };
        let (lo, hi) = surface.domain();
        let ends = [(true, lo.y), (true, hi.y), (false, lo.x), (false, hi.x)];
        for (along_u, at) in ends {
            let edge = surface.iso_curve(along_u, at);
            let bounds = edge.bounds();
            if (bounds.max - bounds.min).length() > CLOSED {
                continue;
            }
            let pole = edge.point(edge.domain().0);
            let touches = face.loops.iter().flatten().any(|&(e, _)| {
                let e = &self.edges[e];
                let t = e.curve.param(pole).clamp(e.t0, e.t1);
                [t, e.t0, e.t1]
                    .iter()
                    .any(|&t| e.curve.point(t).distance(pole) <= CLOSED)
            });
            if touches {
                return Err(error(format!(
                    "This STEP file has a freeform face that comes to a point where its \
                     surface is pinched together (entity #{}: the tip of a turned curve, or a \
                     three-sided patch), which PeetCAD can't import yet. Try exporting the \
                     model from a different program.",
                    face.id
                )));
            }
        }
        Ok(())
    }

    /// Makes a face on a B-spline surface that closes on itself (a tube, a ring) a simple
    /// region of a surface that does not, as the kernel wants its freeform faces: the
    /// surface is cut down to the stretch the face covers, and a face that goes all the
    /// way round (one with a seam) is first cut in two. See the module docs.
    fn unwrap_freeform(&mut self, s: usize, f: usize) -> Result<()> {
        let id = self.shells[s][f].id;
        let mut work = vec![f];
        while let Some(f) = work.pop() {
            let mut passes = 0;
            for axis in 0..2 {
                loop {
                    passes += 1;
                    if passes > UNWRAP_PASSES {
                        return Err(wraps_freeform(id));
                    }
                    match self.unwrap_axis(s, f, axis)? {
                        Unwrapped::Open => break,
                        Unwrapped::Trimmed => {}
                        Unwrapped::Cut(other) => work.push(other),
                    }
                }
            }
        }
        Ok(())
    }

    /// One step of [`Builder::unwrap_freeform`] in `u` (`axis == 0`) or `v`.
    fn unwrap_axis(&mut self, s: usize, f: usize, axis: usize) -> Result<Unwrapped> {
        let Surface::Nurbs(surface) = self.shells[s][f].surface.clone() else {
            return Ok(Unwrapped::Open);
        };
        let closed = Closed::of(&surface);
        let Some(period) = closed.periods[axis] else {
            return Ok(Unwrapped::Open);
        };
        let id = self.shells[s][f].id;
        let (lo, hi) = surface.domain();
        let lifts: Vec<FreeLift> = self.shells[s][f]
            .loops
            .iter()
            .map(|uses| self.lift_freeform(&closed, uses))
            .collect();
        // A loop that goes round the surface: a band between two of them, with no seam
        // to cut along.
        if lifts.iter().any(|l| l.turns[axis] != 0) {
            return Err(wraps_freeform(id));
        }
        // The outer loop reaches furthest.
        let width = |l: &FreeLift| l.high[axis] - l.low[axis];
        let Some(outer) =
            (0..lifts.len()).max_by(|&a, &b| width(&lifts[a]).total_cmp(&width(&lifts[b])))
        else {
            return Err(wraps_freeform(id));
        };
        let (a, b) = (lifts[outer].low[axis], lifts[outer].high[axis]);
        if !(a.is_finite() && b.is_finite()) {
            return Err(wraps_freeform(id));
        }
        if b - a > period * (1.0 - FULL_TURN) {
            return self
                .cut_freeform(s, f, &closed, axis, &lifts, outer)
                .map(Unwrapped::Cut);
        }
        // The stretch the face covers, with some to spare but never the whole turn, and
        // counted from the surface's own range.
        let margin = (0.25 * (period - (b - a))).min(TRIM_MARGIN * period);
        let shift = ((a - lo[axis]) / period + 1e-9).floor() * period;
        let (a, b) = (a - shift, b - shift);
        let from = (a - margin).max(lo[axis]);
        let to = if b <= hi[axis] + 1e-9 * period {
            (b + margin).min(hi[axis])
        } else {
            b + margin
        };
        let band =
            band_of_closed(&surface, axis, from, to).map_err(|why| freeform_failed(id, &why))?;
        self.shells[s][f].surface = Surface::Nurbs(Arc::new(band));
        Ok(Unwrapped::Trimmed)
    }

    /// Cuts a face that goes all the way round a closed freeform surface in two, along a
    /// line of constant `uv[axis]` from one side of its outer loop to the other. The new
    /// face is added to the shell; its index is returned.
    fn cut_freeform(
        &mut self,
        s: usize,
        f: usize,
        closed: &Closed,
        axis: usize,
        lifts: &[FreeLift],
        outer: usize,
    ) -> Result<usize> {
        let id = self.shells[s][f].id;
        let other = 1 - axis;
        let surface = closed.surface;
        let (lo, hi) = surface.domain();
        let period = hi[axis] - lo[axis];
        let seam = lifts[outer].low[axis];
        let clearance = 1e-6 * period;
        // A line that crosses the outer loop twice and no hole.
        let mut found = None;
        for fraction in CUT_TRIES {
            let cut = seam + period * fraction;
            let through_a_hole = lifts.iter().enumerate().any(|(l, lift)| {
                let (low, high) = (lift.low[axis] - clearance, lift.high[axis] + clearance);
                l != outer && cut + ((low - cut) / period).ceil() * period <= high
            });
            if through_a_hole {
                continue;
            }
            let uses = &self.shells[s][f].loops[outer];
            if let Some(hits) = self.freeform_crossings(closed, uses, &lifts[outer], axis, cut)
                && hits.len() == 2
            {
                found = Some((fraction, cut, hits));
                break;
            }
        }
        let Some((fraction, cut, mut hits)) = found else {
            return Err(wraps_freeform(id));
        };
        // On one edge, the later crossing first: splitting there leaves the earlier one
        // where it was.
        if hits[0].edge == hits[1].edge && hits[0].t < hits[1].t {
            hits.swap(0, 1);
        }
        self.vertex_at(hits[0])?;
        self.vertex_at(hits[1])?;

        // The loop's edges on either side of the line: two runs.
        let mut uses = self.shells[s][f].loops[outer].clone();
        let lift = self.lift_freeform(closed, &uses);
        let line = lift.low[axis] + period * fraction;
        let mut beyond = Vec::with_capacity(uses.len());
        let mut at = 0;
        for i in 0..uses.len() {
            let count = lift.samples[at..]
                .iter()
                .take_while(|sample| sample.coedge == i)
                .count();
            let Some(middle) = lift.samples.get(at + count / 2) else {
                return Err(wraps_freeform(id));
            };
            beyond.push(middle.uv[axis] > line);
            at += count;
        }
        let n = uses.len();
        let starts: Vec<usize> = (0..n)
            .filter(|&i| beyond[i] && !beyond[(i + n - 1) % n])
            .collect();
        let [first] = starts[..] else {
            return Err(wraps_freeform(id));
        };
        uses.rotate_left(first);
        beyond.rotate_left(first);
        let run = beyond.iter().take_while(|b| **b).count();

        // The edge along the line, from the run's one end to its other, in the direction
        // the line's own parameter grows.
        let lift = self.lift_freeform(closed, &uses);
        let end = lift.samples.iter().rev().find(|x| x.coedge + 1 == run);
        let (Some(start), Some(end)) = (lift.samples.first(), end) else {
            return Err(wraps_freeform(id));
        };
        let (x, y) = (self.use_start(uses[0]), self.use_end(uses[run - 1]));
        let (from, to, a, b, back) = if end.uv[other] > start.uv[other] {
            (start.uv[other], end.uv[other], x, y, true)
        } else {
            (end.uv[other], start.uv[other], y, x, false)
        };
        // (Not so for a NaN either.)
        let has_length = to - from > 1e-9 * (hi[other] - lo[other]);
        if !has_length {
            return Err(wraps_freeform(id));
        }
        let iso = surface.iso_curve(axis == 1, lo[axis] + (cut - lo[axis]).rem_euclid(period));
        let (curve, t0, t1) = if closed.periods[other].is_some() {
            arc_of_closed(&iso, from, to).map_err(|why| freeform_failed(id, &why))?
        } else {
            (iso, from.max(lo[other]), to.min(hi[other]))
        };
        self.edges.push(RawEdge {
            curve: Curve3::Nurbs(Arc::new(curve)),
            start: a,
            end: b,
            t0,
            t1,
        });
        let cut_edge = self.edges.len() - 1;
        let mut kept = vec![uses[..run].to_vec()];
        let mut moved = vec![uses[run..].to_vec()];
        kept[0].push((cut_edge, back));
        moved[0].push((cut_edge, !back));
        // Each hole goes with the side it is on.
        let face = &mut self.shells[s][f];
        for (l, hole) in face.loops.iter().enumerate() {
            let Some(sample) = lifts[l].samples.first().filter(|_| l != outer) else {
                continue;
            };
            if (sample.uv[axis] - cut).rem_euclid(period) < period * (1.0 - fraction) {
                kept.push(hole.clone());
            } else {
                moved.push(hole.clone());
            }
        }
        face.loops = kept;
        let half = RawFace {
            id,
            surface: face.surface.clone(),
            reversed: face.reversed,
            loops: moved,
        };
        self.shells[s].push(half);
        Ok(self.shells[s].len() - 1)
    }

    /// The area a loop encloses in its face's parameter space, signed: positive when it
    /// runs counter-clockwise seen from outside, as an outer loop does.
    fn loop_area(&self, face: &RawFace, uses: &[Use]) -> f64 {
        let mut emitter = Emitter::default();
        let shell = emitter.solid.add_shell();
        let f = emitter
            .solid
            .add_face(shell, face.surface.clone(), face.reversed);
        let uses = emitter.uses(self, uses);
        emitter.solid.add_loop(f, &uses);
        measure::face_area(&emitter.solid, f)
    }

    /// Puts the face's outer loop first: the one that runs counter-clockwise seen from
    /// outside.
    fn outer_loop_first(&mut self, s: usize, f: usize) -> Result<()> {
        let face = &self.shells[s][f];
        let outer: Vec<usize> = match face.loops.len() {
            0 => Vec::new(),
            // A single loop is the outer loop; validation says so if it runs backwards.
            1 => vec![0],
            n => (0..n)
                .filter(|&l| self.loop_area(face, &face.loops[l]) > 0.0)
                .collect(),
        };
        match outer[..] {
            [l] => {
                let loops = &mut self.shells[s][f].loops;
                let outer = loops.remove(l);
                loops.insert(0, outer);
                Ok(())
            }
            [] => Err(error(format!(
                "The STEP file has a face without an outer boundary (entity #{}), so the body \
                 is not a solid PeetCAD can import. Try exporting it again.",
                face.id
            ))),
            _ => Err(error(format!(
                "The STEP file has a face with more than one outer boundary (entity #{}), \
                 which PeetCAD can't import. Try exporting the model again, or from a \
                 different program.",
                face.id
            ))),
        }
    }
}

/// Turns raw vertices and edges into a solid's, as loops use them.
#[derive(Default)]
struct Emitter {
    solid: Solid,
    vertices: HashMap<usize, VertexId>,
    edges: HashMap<usize, EdgeId>,
}

impl Emitter {
    fn vertex(&mut self, b: &Builder, v: usize) -> VertexId {
        let v = b.rep(v);
        match self.vertices.get(&v) {
            Some(&id) => id,
            None => {
                let id = self.solid.add_vertex(b.vertices[v]);
                self.vertices.insert(v, id);
                id
            }
        }
    }

    fn uses(&mut self, b: &Builder, uses: &[Use]) -> Vec<(EdgeId, bool)> {
        uses.iter()
            .map(|&(e, reversed)| {
                let id = match self.edges.get(&e) {
                    Some(&id) => id,
                    None => {
                        let edge = &b.edges[e];
                        let (start, end) = (self.vertex(b, edge.start), self.vertex(b, edge.end));
                        let id =
                            self.solid
                                .add_edge(edge.curve.clone(), start, end, edge.t0, edge.t1);
                        self.edges.insert(e, id);
                        id
                    }
                };
                (id, reversed)
            })
            .collect()
    }
}

/// Turns a shell inside out: every face flipped and every loop run backwards.
fn turn_inside_out(solid: &mut Solid, shell: ShellId) {
    for face in solid.shell(shell).faces.clone() {
        let loops = solid.face(face).loops.clone();
        solid.faces[face.index()].reversed ^= true;
        for l in loops {
            for c in solid.loop_coedges(l) {
                let coedge = &mut solid.coedges[c.index()];
                coedge.reversed = !coedge.reversed;
                std::mem::swap(&mut coedge.next, &mut coedge.prev);
            }
        }
    }
}

/// The same curve run the other way: `point(t)` becomes `point(−t)`.
fn turned_round(curve: &Curve3) -> Curve3 {
    match curve {
        Curve3::Line(l) => Curve3::Line(Line3 {
            origin: l.origin,
            dir: -l.dir,
        }),
        Curve3::Circle(c) => Curve3::Circle(Circle3 {
            frame: half_turn(&c.frame),
            radius: c.radius,
        }),
        Curve3::Ellipse(e) => Curve3::Ellipse(Ellipse3 {
            frame: half_turn(&e.frame),
            ..*e
        }),
        Curve3::Nurbs(c) => Curve3::Nurbs(std::sync::Arc::new(c.reversed())),
    }
}

/// A frame turned half a turn about its X axis, which mirrors angles about its Z.
fn half_turn(frame: &Frame) -> Frame {
    Frame {
        origin: frame.origin,
        rotation: (frame.rotation * DQuat::from_rotation_x(PI)).normalize(),
    }
}

/// The circle of a sphere's or torus's meridian at the angle `u`, with the surface's `v`
/// as its parameter.
fn meridian_circle(surface: &Surface, u: f64) -> Option<(Frame, f64)> {
    let frame = surface.revolution_frame()?;
    let (sin, cos) = u.sin_cos();
    let radial = frame.x_axis() * cos + frame.y_axis() * sin;
    let (centre, radius) = match surface {
        Surface::Sphere(s) => (frame.origin, s.radius),
        Surface::Torus(t) => (frame.origin + radial * t.major, t.minor),
        _ => return None,
    };
    Some((
        Frame::from_origin_z_x(centre, radial.cross(frame.z_axis()), radial)?,
        radius,
    ))
}

/// The circle about a surface of revolution's axis at `v`, with `u` as its parameter.
fn parallel_circle(surface: &Surface, v: f64) -> Option<(Frame, f64)> {
    let frame = surface.revolution_frame()?;
    let m = surface.meridian(v)?;
    (m.rho > 2.0 * tolerance::LINEAR).then_some((
        Frame {
            origin: frame.origin + frame.z_axis() * m.z,
            rotation: frame.rotation,
        },
        m.rho,
    ))
}

/// An angle brought into `[−π, π)`.
fn wrap(a: f64) -> f64 {
    (a + PI).rem_euclid(TAU) - PI
}

fn seam_failed(face: u32) -> StepImportError {
    error(format!(
        "The STEP file has a face that goes all the way round its surface without a seam \
         (entity #{face}), and PeetCAD could not add one. Try exporting the model from a \
         different program, or with periodic faces split."
    ))
}

/// The error for a surface or curve the kernel has no counterpart for.
fn unsupported(id: u32, records: &[Record], what: &str) -> StepImportError {
    error(format!(
        "This STEP file has a {what} of a kind PeetCAD can't import yet (entity #{id} {}). \
         PeetCAD reads solids whose faces are planes, cylinders, cones, spheres, tori and \
         B-spline surfaces, with straight, circular, elliptical and B-spline edges.",
        describe(records)
    ))
}

// ---- Swept surfaces ----

/// A curve as a B-spline curve: a whole circle or ellipse, or the stretch of a line
/// between the parameters `range`.
fn as_spline(curve: &Curve3, range: Option<(f64, f64)>) -> Option<NurbsCurve> {
    match curve {
        Curve3::Line(l) => {
            let (a, b) = range?;
            Some(NurbsCurve::line(
                l.origin + l.dir * a,
                l.origin + l.dir * b,
                1,
            ))
        }
        Curve3::Circle(c) => Some(NurbsCurve::arc(&c.frame, c.radius, 0.0, TAU)),
        Curve3::Ellipse(e) => {
            // A circle stretched: stretching maps control points to control points.
            let unit = NurbsCurve::arc(&Frame::WORLD, 1.0, 0.0, TAU);
            Some(unit.mapped(|p| {
                e.frame
                    .to_world(DVec3::new(p.x * e.major, p.y * e.minor, 0.0))
            }))
        }
        Curve3::Nurbs(c) => Some(NurbsCurve::clone(c)),
    }
}

/// Parameters of a curve to try for a point where a surface swept from it has a normal.
fn sweep_samples(curve: &Curve3, range: Option<(f64, f64)>) -> Vec<f64> {
    let fractions = [0.5, 0.25, 0.75, 0.1, 0.9, 0.0, 1.0];
    match (curve.domain().or(range), curve) {
        (Some((lo, hi)), _) => fractions.iter().map(|f| lo + (hi - lo) * f).collect(),
        (None, Curve3::Line(_)) => vec![0.0, 1.0, -1.0],
        // A circle or an ellipse: from where its parameter starts.
        (None, _) => (0..8).map(|k| TAU * f64::from(k) / 8.0).collect(),
    }
}

/// The sample with the longest normal: the first of them, where several are as long.
fn steadiest(samples: impl Iterator<Item = (DVec3, DVec3)>) -> Option<(DVec3, DVec3)> {
    let mut best: Option<(DVec3, DVec3)> = None;
    for sample in samples {
        let longer = |b: &(DVec3, DVec3)| sample.1.length() > b.1.length() * (1.0 + 1e-9);
        if best.as_ref().is_none_or(longer) {
            best = Some(sample);
        }
    }
    let (at, normal) = best?;
    Some((at, normal.try_normalize()?))
}

/// The range `values` cover, widened a little; `None` when there are none.
fn padded_range(values: impl Iterator<Item = f64>) -> Option<(f64, f64)> {
    let (lo, hi) = values.fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
        (lo.min(v), hi.max(v))
    });
    let pad = 0.05 * (hi - lo) + 1e-3;
    (lo.is_finite() && hi.is_finite()).then_some((lo - pad, hi + pad))
}

/// The surface `curve` sweeps along `dir`: `curve(u) + v dir`. With it come a point of the
/// surface and the surface's normal there as STEP has it (`∂/∂u × ∂/∂v`), from which to
/// tell whether the surface made faces the same way. `points` are points of the face.
fn extruded(curve: &Curve3, dir: DVec3, points: &[DVec3]) -> Option<(Surface, DVec3, DVec3)> {
    let (at, normal) = steadiest(
        sweep_samples(curve, None)
            .into_iter()
            .map(|t| (curve.point(t), curve.derivative(t).cross(dir))),
    )?;
    let surface = match curve {
        Curve3::Line(l) => Surface::Plane(Plane::from_origin_normal_x(l.origin, normal, l.dir)?),
        Curve3::Circle(c) if c.frame.z_axis().cross(dir).length() <= tolerance::ANGULAR => {
            Surface::Cylinder(Cylinder {
                frame: c.frame,
                radius: c.radius,
            })
        }
        _ => {
            // Ruled between the curve at the face's two ends along the direction. The
            // curve lies within its control points, which bounds how far along it is.
            let base = as_spline(curve, None)?;
            let (low, high) = padded_range(points.iter().map(|p| p.dot(dir)))?;
            let (first, last) = padded_range(base.control_points().iter().map(|p| p.dot(dir)))?;
            let (from, to) = (low - last, high - first);
            let ends = [
                base.mapped(|p| p + dir * from),
                base.mapped(|p| p + dir * to),
            ];
            Surface::Nurbs(Arc::new(NurbsSurface::skin(&ends).ok()?))
        }
    };
    Some((surface, at, normal))
}

/// The surface `curve` sweeps when it is turned about the axis through `origin` along
/// `axis`: `u` is the angle, `v` the curve's parameter. See [`extruded`] for the rest.
fn revolved(
    curve: &Curve3,
    origin: DVec3,
    axis: DVec3,
    points: &[DVec3],
) -> Option<(Surface, DVec3, DVec3)> {
    let frame = Frame::from_origin_z_x(origin, axis, DVec3::X)
        .or_else(|| Frame::from_origin_z_x(origin, axis, DVec3::Y))?;
    let height = |p: DVec3| (p - origin).dot(axis);
    let radial = |p: DVec3| (p - origin) - axis * height(p);
    // A line is turned as far along it as the face reaches.
    let range = match curve {
        Curve3::Line(l) if l.dir.dot(axis).abs() > tolerance::ANGULAR => padded_range(
            points
                .iter()
                .map(|p| (height(*p) - height(l.origin)) / l.dir.dot(axis)),
        ),
        _ => None,
    };
    let (at, normal) = steadiest(sweep_samples(curve, range).into_iter().map(|t| {
        let p = curve.point(t);
        (p, axis.cross(radial(p)).cross(curve.derivative(t)))
    }))?;
    let analytic = match curve {
        Curve3::Line(l) => {
            let along = l.dir.dot(axis);
            let across = l.dir.cross(axis);
            // How close the line comes to the axis.
            let gap = match across.try_normalize() {
                Some(n) => (l.origin - origin).dot(n).abs(),
                None => radial(l.origin).length(),
            };
            if across.length() <= tolerance::ANGULAR {
                (gap > 2.0 * tolerance::LINEAR)
                    .then_some(Surface::Cylinder(Cylinder { frame, radius: gap }))
            } else if along.abs() <= tolerance::ANGULAR {
                // Square to the axis: a plane across it.
                Plane::from_origin_normal_x(l.origin, normal, l.dir).map(Surface::Plane)
            } else if gap <= tolerance::LINEAR {
                // Through the axis: a cone, of which the kernel has the half the face is
                // on. The line meets the axis where its distance from it is nothing.
                let sideways = l.dir - axis * along;
                let t = -radial(l.origin).dot(sideways) / sideways.length_squared();
                let apex = origin + axis * height(l.origin + l.dir * t);
                let side: f64 = points.iter().map(|p| (*p - apex).dot(axis)).sum();
                let up = if side < 0.0 { -axis } else { axis };
                Frame::from_origin_z_x(apex, up, frame.x_axis()).map(|frame| {
                    Surface::Cone(Cone {
                        frame,
                        radius: 0.0,
                        half_angle: along.abs().min(1.0).acos(),
                    })
                })
            } else {
                None
            }
        }
        Curve3::Circle(c) => {
            let centre = c.frame.origin;
            // The circle's plane holds the axis.
            let in_plane = c.frame.z_axis().dot(axis).abs() <= tolerance::ANGULAR
                && (centre - origin).dot(c.frame.z_axis()).abs() <= tolerance::LINEAR;
            let major = radial(centre).length();
            let frame = Frame {
                origin: origin + axis * height(centre),
                rotation: frame.rotation,
            };
            if in_plane && major <= tolerance::LINEAR {
                Some(Surface::Sphere(Sphere {
                    frame,
                    radius: c.radius,
                }))
            } else if in_plane && major > c.radius + tolerance::LINEAR {
                Some(Surface::Torus(Torus {
                    frame,
                    major,
                    minor: c.radius,
                }))
            } else {
                None
            }
        }
        _ => None,
    };
    let surface = match analytic {
        Some(surface) => surface,
        None => {
            // The curve's control points turned about the axis: a circle is a rational
            // quadratic, and turning is linear in the cosine and sine of the angle.
            let profile = as_spline(curve, range)?;
            let round = NurbsCurve::arc(&Frame::WORLD, 1.0, 0.0, TAU);
            let round_weights = round.weights()?;
            let mut net = Vec::new();
            let mut weights = Vec::new();
            for (corner, weight) in round.control_points().iter().zip(round_weights) {
                for (j, p) in profile.control_points().iter().enumerate() {
                    let l = frame.to_local(*p);
                    net.push(frame.to_world(DVec3::new(
                        l.x * corner.x - l.y * corner.y,
                        l.x * corner.y + l.y * corner.x,
                        l.z,
                    )));
                    weights.push(weight * profile.weights().map_or(1.0, |w| w[j]));
                }
            }
            let surface = NurbsSurface::new(
                2,
                profile.degree(),
                round.knots().to_vec(),
                profile.knots().to_vec(),
                net,
                Some(weights),
            );
            Surface::Nurbs(Arc::new(surface.ok()?))
        }
    };
    Some((surface, at, normal))
}

// ---- B-splines ----

/// Whether `name` is one of the B-spline entities of `kind` (`CURVE` or `SURFACE`).
fn is_spline(name: &str, kind: &str) -> bool {
    ["B_SPLINE_", "BEZIER_", "QUASI_UNIFORM_", "UNIFORM_"]
        .iter()
        .any(|prefix| name.strip_prefix(prefix) == Some(kind))
        || name
            .strip_prefix("B_SPLINE_")
            .and_then(|n| n.strip_suffix("_WITH_KNOTS"))
            == Some(kind)
        || name.strip_prefix("RATIONAL_B_SPLINE_") == Some(kind)
}

/// The knot vectors STEP gives no numbers for.
#[derive(Clone, Copy)]
enum KnotKind {
    /// Bézier pieces end to end: whole numbers, each as often as the degree.
    Bezier,
    /// Whole numbers, clamped at both ends.
    QuasiUniform,
    /// Whole numbers, none repeated: not clamped.
    Uniform,
}

/// Checks a B-spline's degree and its number of control points (`direction` is empty for
/// a curve, " in u" or " in v" for a surface), so that what follows is bounded.
fn check_spline_size(e: &Ent, degree: usize, count: usize, direction: &str) -> Result<()> {
    if degree == 0 || degree > MAX_DEGREE {
        return Err(error(format!(
            "This STEP file has a B-spline of degree {degree}{direction} (entity #{} {}), which \
             PeetCAD can't import: it reads degrees 1 to {MAX_DEGREE}. The file may be damaged; \
             try exporting it again.",
            e.id,
            e.name()
        )));
    }
    if count <= degree {
        return Err(damaged(format!(
            "#{} {} has {count} control point{}{direction}, too few for its degree of {degree}",
            e.id,
            e.name(),
            if count == 1 { "" } else { "s" }
        )));
    }
    Ok(())
}

/// A knot vector written as multiplicities (parameter `multiplicities` of `e`) and
/// distinct knots (parameter `values`), for `count` control points of `degree`.
fn given_knots(
    e: &Ent,
    multiplicities: usize,
    values: usize,
    degree: usize,
    count: usize,
    direction: &str,
) -> Result<Vec<f64>> {
    let repeats = e.list(multiplicities)?;
    let values_at = values;
    let values = e.reals_in(values_at, e.list(values_at)?)?;
    if repeats.len() != values.len() {
        return Err(damaged(format!(
            "#{} {} lists {} knot multiplicities for {} knots{direction}",
            e.id,
            e.name(),
            repeats.len(),
            values.len()
        )));
    }
    let need = count + degree + 1;
    let mut total = 0usize;
    let mut knots = Vec::with_capacity(need);
    for (repeat, &knot) in repeats.iter().zip(&values) {
        let repeat = match repeat {
            Value::Int(n) if *n >= 1 => usize::try_from(*n).unwrap_or(usize::MAX),
            _ => return e.bad(multiplicities, "a list of whole numbers, 1 or more"),
        };
        total = total.saturating_add(repeat);
        // Only as many as are needed: a wrong count must not fill the memory.
        knots.extend(std::iter::repeat_n(
            knot,
            repeat.min(need + 1 - knots.len()),
        ));
    }
    if total != need {
        return Err(damaged(format!(
            "#{} {} has {total} knots{direction} (counting how often each is repeated) where \
             its {count} control points of degree {degree} need {need}",
            e.id,
            e.name()
        )));
    }
    Ok(knots)
}

/// The knot vector of a Bézier, quasi-uniform or uniform B-spline.
fn regular_knots(e: &Ent, kind: KnotKind, degree: usize, count: usize) -> Result<Vec<f64>> {
    let ends = |pieces: usize, inner: usize| {
        let mut knots = vec![0.0; degree + 1];
        for k in 1..pieces {
            knots.extend(std::iter::repeat_n(k as f64, inner));
        }
        knots.extend(std::iter::repeat_n(pieces as f64, degree + 1));
        knots
    };
    Ok(match kind {
        KnotKind::Bezier => {
            if !(count - 1).is_multiple_of(degree) {
                return Err(damaged(format!(
                    "#{} {} has {count} control points, which don't make whole Bézier pieces \
                     of degree {degree}",
                    e.id,
                    e.name()
                )));
            }
            ends((count - 1) / degree, degree)
        }
        KnotKind::QuasiUniform => ends(count - degree, 1),
        KnotKind::Uniform => (0..count + degree + 1)
            .map(|k| k as f64 - degree as f64)
            .collect(),
    })
}

fn no_knots(id: u32, records: &[Record], what: &str) -> StepImportError {
    error(format!(
        "This STEP file has a B-spline {what} without a knot vector (entity #{id} {}), which \
         PeetCAD can't import. Try exporting the model again, or from a different program.",
        describe(records)
    ))
}

/// The error for a B-spline the kernel refuses.
fn bad_spline(id: u32, records: &[Record], what: &str, why: &NurbsError) -> StepImportError {
    // The kernel's messages start with what they are about ("curve: ...").
    let why = why.0.split_once(": ").map_or(why.0.as_str(), |(_, w)| w);
    error(format!(
        "This STEP file has a B-spline {what} PeetCAD can't use (entity #{id} {}): {why}. The \
         file may be damaged; try exporting it again.",
        describe(records)
    ))
}

/// The stretch of a B-spline curve that the edge #`id` from `a` to `b` is: the curve to
/// give the edge and its parameter range there. An edge runs the way its curve does, so
/// on a closed curve it may start anywhere and run across the curve's own start: the
/// curve is then put together anew, starting where the edge does. An open curve whose
/// vertices are given against its direction is turned round, like a line.
fn spline_edge(
    id: u32,
    curve: &Arc<NurbsCurve>,
    a: DVec3,
    b: DVec3,
    closed_edge: bool,
) -> Result<(Arc<NurbsCurve>, f64, f64)> {
    let (lo, hi) = curve.domain();
    let closed = curve.is_closed(CLOSED);
    let seam = curve.point(lo);
    // A vertex where a closed curve starts and ends is at whichever end suits.
    let at_seam = |p: DVec3| closed && p.distance(seam) <= CLOSED;
    let failed = |why: NurbsError| {
        error(format!(
            "PeetCAD could not put the closed B-spline curve of an edge in this STEP file \
             together again from where the edge starts (entity #{id}): {}. Try exporting the \
             model again.",
            why.0
        ))
    };
    if closed_edge {
        if !closed {
            return Err(error(format!(
                "The STEP file has an edge that starts and ends at the same point, on a curve \
                 that isn't closed (#{id}), which PeetCAD can't import. Try exporting the \
                 model again."
            )));
        }
        // All the way round, from the edge's vertex.
        let t = curve.param(a);
        let near_end = 1e-9 * (hi - lo);
        if at_seam(a) || t - lo <= near_end || hi - t <= near_end {
            return Ok((curve.clone(), lo, hi));
        }
        let (turned, t0, t1) = arc_of_closed(curve, t, t + (hi - lo)).map_err(failed)?;
        return Ok((Arc::new(turned), t0, t1));
    }
    let ta = if at_seam(a) { lo } else { curve.param(a) };
    let tb = if at_seam(b) { hi } else { curve.param(b) };
    if tb > ta {
        return Ok((curve.clone(), ta, tb));
    }
    if closed {
        // Across the curve's start.
        let (turned, t0, t1) = arc_of_closed(curve, ta, tb + (hi - lo)).map_err(failed)?;
        return Ok((Arc::new(turned), t0, t1));
    }
    let reversed = curve.reversed();
    let (ta, tb) = (reversed.param(a), reversed.param(b));
    if tb > ta {
        Ok((Arc::new(reversed), ta, tb))
    } else {
        Err(error(format!(
            "The STEP file has a freeform edge with no length (#{id}), which PeetCAD can't \
             import. Try exporting the model again."
        )))
    }
}

/// The part of a closed curve from the parameter `from` on to `to` (at most one period
/// further), which may run across the curve's ends: then the curve is put together anew
/// from its tail and its head. The curve comes with the part's parameter range in it.
fn arc_of_closed(
    curve: &NurbsCurve,
    from: f64,
    to: f64,
) -> std::result::Result<(NurbsCurve, f64, f64), NurbsError> {
    let (lo, hi) = curve.domain();
    let period = hi - lo;
    let slack = 1e-9 * period;
    let shift = ((from + slack - lo) / period).floor() * period;
    let (from, to) = ((from - shift).max(lo), to - shift);
    if to <= hi + slack {
        return Ok((curve.clone(), from, to.min(hi)));
    }
    let rest = (to - period).min(from);
    if rest - lo <= slack {
        return Ok((curve.clone(), from, hi));
    }
    let joined = curve
        .sub_curve(from, hi)?
        .joined(&curve.sub_curve(lo, rest)?)?;
    Ok((joined, from, hi + (rest - lo)))
}

/// The part of a surface that is closed in `u` (`axis == 0`) or `v` between `from` and
/// `to` there (at most one period further), which may run across the surface's seam:
/// then the part is put together from the stretch up to the seam and the stretch after
/// it, and its parameter runs on past the seam.
fn band_of_closed(
    surface: &NurbsSurface,
    axis: usize,
    from: f64,
    to: f64,
) -> std::result::Result<NurbsSurface, NurbsError> {
    let (lo, hi) = surface.domain();
    let (lo, hi) = (lo[axis], hi[axis]);
    let period = hi - lo;
    let slack = 1e-9 * period;
    let shift = ((from + slack - lo) / period).floor() * period;
    let (from, to) = ((from - shift).max(lo), to - shift);
    let along_u = axis == 0;
    if to <= hi + slack {
        return surface.sub_surface(along_u, from, to.min(hi));
    }
    let rest = (to - period).min(from);
    if rest - lo <= slack {
        return surface.sub_surface(along_u, from, hi);
    }
    surface
        .sub_surface(along_u, from, hi)?
        .joined(&surface.sub_surface(along_u, lo, rest)?, along_u)
}

/// A freeform surface that closes on itself, for following a face's loops across it.
struct Closed<'s> {
    surface: &'s NurbsSurface,
    /// The surface's period in `u` and in `v`; `None` for a direction it is open in.
    periods: [Option<f64>; 2],
}

impl<'s> Closed<'s> {
    fn of(surface: &'s NurbsSurface) -> Self {
        let (lo, hi) = surface.domain();
        let size = hi - lo;
        Self {
            surface,
            periods: [
                surface.is_closed(true, CLOSED).then_some(size.x),
                surface.is_closed(false, CLOSED).then_some(size.y),
            ],
        }
    }

    /// The parameters of the point `p` of the surface: of the values that differ by whole
    /// periods, the ones closest to `near`.
    fn lifted(&self, p: DVec3, near: Option<DVec2>) -> DVec2 {
        let mut uv = self.surface.param(p);
        if let Some(near) = near {
            for axis in 0..2 {
                if let Some(period) = self.periods[axis] {
                    uv[axis] += ((near[axis] - uv[axis]) / period).round() * period;
                }
            }
        }
        uv
    }
}

/// How many steps an edge is sampled in when it is followed across a freeform surface.
fn freeform_steps(curve: &Curve3, ta: f64, tb: f64) -> usize {
    match curve {
        Curve3::Line(_) => 8,
        Curve3::Nurbs(c) => {
            let (lo, hi) = (ta.min(tb), ta.max(tb));
            let spans = c
                .knots()
                .windows(2)
                .filter(|w| w[1] > w[0] && w[1] > lo && w[0] < hi)
                .count();
            (4 * spans).clamp(8, 128)
        }
        _ => ((tb - ta).abs() / (PI / 16.0)).ceil().clamp(4.0, 128.0) as usize,
    }
}

fn wraps_freeform(face: u32) -> StepImportError {
    error(format!(
        "This STEP file has a freeform face that wraps all the way round its surface (entity \
         #{face}) without a seam edge PeetCAD could split it at, which it can't import yet. \
         Try exporting the model from a different program, or with periodic faces split."
    ))
}

fn freeform_failed(face: u32, why: &NurbsError) -> StepImportError {
    error(format!(
        "PeetCAD could not cut the closed B-spline surface of a face in this STEP file down \
         to the face (entity #{face}): {}. Try exporting the model with periodic faces split.",
        why.0
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_decode() {
        assert_eq!(decode_string("plain"), "plain");
        assert_eq!(decode_string("a\\\\b"), "a\\b");
        assert_eq!(decode_string("Gr\\X2\\00F600DF\\X0\\e"), "Größe");
        assert_eq!(decode_string("x\\X4\\0001F600\\X0\\"), "x\u{1F600}");
        assert_eq!(decode_string("\\X2\\D83DDE00\\X0\\"), "\u{1F600}");
        assert_eq!(decode_string("\\X\\E9t\\S\\i"), "ét\u{e9}");
        // Broken escapes are kept as written.
        assert_eq!(decode_string("\\X2\\00F"), "\\X2\\00F");
        assert_eq!(decode_string("\\X2\\zzzz\\X0\\"), "\\X2\\zzzz\\X0\\");
        assert_eq!(decode_string("end\\"), "end\\");
    }

    #[test]
    fn tokens() {
        let mut lexer = Lexer {
            bytes: b"#12 = A_B ( 'it''s', .T., $, *, -1.5E-3, 7, 1., /* c */ (#3) ) ;",
            at: 0,
            start: 0,
        };
        let mut out = Vec::new();
        loop {
            match lexer.next().unwrap() {
                Token::End => break,
                t => out.push(t),
            }
        }
        assert_eq!(
            out,
            vec![
                Token::Ref(12),
                Token::Equals,
                Token::Keyword("A_B".into()),
                Token::Open,
                Token::Str("it's".into()),
                Token::Comma,
                Token::Enum("T".into()),
                Token::Comma,
                Token::Dollar,
                Token::Comma,
                Token::Star,
                Token::Comma,
                Token::Real(-1.5e-3),
                Token::Comma,
                Token::Int(7),
                Token::Comma,
                Token::Real(1.0),
                Token::Comma,
                Token::Open,
                Token::Ref(3),
                Token::Close,
                Token::Close,
                Token::Semi,
            ]
        );
    }

    #[test]
    fn units_convert() {
        let file = parse(
            "ISO-10303-21;HEADER;ENDSEC;DATA;\
             #1=(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT(.CENTI.,.METRE.));\
             #2=(NAMED_UNIT(*)PLANE_ANGLE_UNIT()SI_UNIT($,.RADIAN.));\
             #3=(CONVERSION_BASED_UNIT('INCH',#4)LENGTH_UNIT()NAMED_UNIT(#9));\
             #4=LENGTH_MEASURE_WITH_UNIT(LENGTH_MEASURE(2.54),#1);\
             #5=(CONVERSION_BASED_UNIT('DEGREE',#6)NAMED_UNIT(*)PLANE_ANGLE_UNIT());\
             #6=PLANE_ANGLE_MEASURE_WITH_UNIT(PLANE_ANGLE_MEASURE(0.0174532925),#2);\
             #7=(CONVERSION_BASED_UNIT('LOOP',#8)LENGTH_UNIT()NAMED_UNIT(*));\
             #8=LENGTH_MEASURE_WITH_UNIT(LENGTH_MEASURE(2.0),#7);\
             ENDSEC;END-ISO-10303-21;",
        )
        .unwrap();
        let size = |id| unit_size(&file, id, 0).map(|(_, s)| s);
        assert_eq!(size(1), Some(10.0));
        assert_eq!(size(2), Some(1.0));
        assert!((size(3).unwrap() - 25.4).abs() < 1e-12);
        assert!((size(5).unwrap() - PI / 180.0).abs() < 1e-10);
        assert_eq!(size(7), None, "a unit defined by itself");
        assert_eq!(size(99), None);
    }
}
