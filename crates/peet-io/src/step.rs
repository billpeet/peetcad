//! STEP export (ISO 10303-21 clear text) of solids as exact B-reps, in AP214
//! (`AUTOMOTIVE_DESIGN`) or AP242 (`AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF`).
//!
//! **File layout.** The entities follow what OCCT (FreeCAD) and the big commercial
//! exporters write for a single part, so the files import cleanly almost everywhere:
//!
//! - product structure: `APPLICATION_CONTEXT`, `APPLICATION_PROTOCOL_DEFINITION`,
//!   `PRODUCT_CONTEXT`, `PRODUCT` (with a `PRODUCT_RELATED_PRODUCT_CATEGORY` of `'part'`),
//!   `PRODUCT_DEFINITION_FORMATION`, `PRODUCT_DEFINITION_CONTEXT`, `PRODUCT_DEFINITION`,
//!   `PRODUCT_DEFINITION_SHAPE` and a `SHAPE_DEFINITION_REPRESENTATION` linking it to one
//!   `ADVANCED_BREP_SHAPE_REPRESENTATION`;
//! - the representation's context: millimetres, radians and steradians
//!   (`GLOBAL_UNIT_ASSIGNED_CONTEXT`) and the kernel's linear tolerance as the distance
//!   uncertainty (`GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT`). Units and the context are complex
//!   entity instances, written `(A(..) B(..) ..)` with the partial entities in alphabetical
//!   order, as Part 21 requires;
//! - the representation's items: a world `AXIS2_PLACEMENT_3D` and one
//!   `MANIFOLD_SOLID_BREP` per lump (or a `BREP_WITH_VOIDS` when the lump has internal
//!   voids), named after the body it came from.
//!
//! Several bodies go into the one product and representation, one brep each, the way a
//! multi-body part is usually exported; they are not made into an assembly.
//!
//! **Topology.** Kernel elements map one to one: shell → `CLOSED_SHELL`, face →
//! `ADVANCED_FACE` on a `PLANE`, `CYLINDRICAL_SURFACE`, `CONICAL_SURFACE`, `SPHERICAL_SURFACE` or
//! `TOROIDAL_SURFACE`, loop → `FACE_OUTER_BOUND` (the
//! first loop) or `FACE_BOUND` (holes) around an `EDGE_LOOP`, coedge → `ORIENTED_EDGE`,
//! edge → `EDGE_CURVE` on a `LINE` (with a unit `VECTOR`), `CIRCLE` or `ELLIPSE`, vertex →
//! `VERTEX_POINT`. Edges and vertices are shared between faces exactly as in the kernel; a
//! closed edge (a full circle) has the same vertex at both ends, and a cylinder's seam is one
//! `EDGE_CURVE` used twice by the face's loop.
//!
//! **Orientation.** The kernel's conventions (see [`peet_kernel::topo`]) line up with
//! STEP's, so the mapping is direct:
//!
//! - STEP surfaces have the same parameterisation and natural normal as the kernel's: a
//!   `PLANE` points along its placement's axis (the frame's Z), a `CYLINDRICAL_SURFACE`
//!   radially outwards. `ADVANCED_FACE.same_sense` is therefore `!face.reversed`, and the
//!   face's oriented normal is the kernel's outward normal.
//! - Kernel edges run from `start` at `t0` to `end` at `t1 > t0`, i.e. in the direction of
//!   increasing curve parameter, and the STEP curves are parameterised the same way (`LINE`
//!   as origin + t · direction, `CIRCLE` and `ELLIPSE` counter-clockwise about the placement
//!   axis from its reference direction). So every `EDGE_CURVE` goes from the start vertex
//!   to the end vertex with `same_sense = .T.`; a STEP reader trims the curve between the
//!   two vertices in the positive direction, a full turn for a closed edge.
//! - `ORIENTED_EDGE.orientation` is `!coedge.reversed`.
//! - Kernel loops run counter-clockwise seen from outside, with the face on their left;
//!   STEP wants the same of a bound with `orientation = .T.` relative to the face's oriented
//!   normal. Loops are written in coedge order with every bound's orientation `.T.`.
//!
//! **Voids.** A kernel solid may hold several shells: outer shells (positive volume), each a
//! separate lump, and inside-out shells (negative volume) that bound internal voids. Each
//! void shell is assigned to the smallest outer shell whose bounding box contains it. STEP
//! writes a void as an `ORIENTED_CLOSED_SHELL` with orientation `.F.` over a closed shell
//! that faces *away* from the cavity (as if the cavity were a solid), which is how OCCT
//! writes and reads them; so a void shell's faces are written flipped (`same_sense`
//! negated, each loop reversed with its oriented edges flipped).
//!
//! **Numbers and text.** Reals are written with the shortest representation that reads back
//! to the same `f64`, always with a decimal point and with a Part 21 exponent (`1.0`,
//! `-0.25`, `1.5E-07`). Strings double apostrophes and backslashes and encode non-ASCII
//! characters as `\X2\hhhh\X0\` (or `\X4\` beyond the Basic Multilingual Plane).
//!
//! The writer does not validate its input: give it valid solids (see
//! [`peet_kernel::validate`]). It reads no clock (the time stamp comes from the caller), so
//! it also runs on `wasm32`.

use std::fmt::Write as _;

use peet_kernel::geom::{Curve3, Surface};
use peet_kernel::topo::{ShellId, Solid};
use peet_kernel::validate::measure;
use peet_math::{Aabb, DVec3, Frame, tolerance};

/// The application protocol (schema) a file is written in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StepSchema {
    /// AP214 (`AUTOMOTIVE_DESIGN`): the one every importer reads.
    #[default]
    Ap214,
    /// AP242 (`AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF`), its successor.
    Ap242,
}

impl StepSchema {
    /// The schema name in `FILE_SCHEMA`.
    pub fn file_schema(self) -> &'static str {
        match self {
            Self::Ap214 => "AUTOMOTIVE_DESIGN { 1 0 10303 214 1 1 1 1 }",
            Self::Ap242 => {
                "AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF { 1 0 10303 442 1 1 4 }"
            }
        }
    }

    /// `APPLICATION_CONTEXT` description, `APPLICATION_PROTOCOL_DEFINITION` schema name and
    /// year, as OCCT writes them.
    fn application(self) -> (&'static str, &'static str, u32) {
        match self {
            Self::Ap214 => (
                "core data for automotive mechanical design processes",
                "automotive_design",
                2000,
            ),
            Self::Ap242 => (
                "managed model based 3d engineering",
                "ap242_managed_model_based_3d_engineering",
                2014,
            ),
        }
    }
}

/// What goes into a STEP file besides the geometry.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StepOptions {
    pub schema: StepSchema,
    /// The part's name (`PRODUCT` id and name, and `FILE_NAME`).
    pub product_name: String,
    /// The file's author; empty for none.
    pub author: String,
    /// The author's organization; empty for none.
    pub organization: String,
    /// When the file was written, ISO 8601 (`2026-10-03T14:05:00`). Supplied by the caller,
    /// since this crate reads no clock.
    pub timestamp: String,
}

/// Writes `bodies` (each a name and a solid) as one part in a STEP file.
pub fn write(bodies: &[(&str, &Solid)], options: &StepOptions) -> String {
    let mut w = Writer::default();
    let (context_text, protocol, year) = options.schema.application();

    // Product structure.
    let app = w.add(format!("APPLICATION_CONTEXT({})", string(context_text)));
    w.add(format!(
        "APPLICATION_PROTOCOL_DEFINITION('international standard',{},{year},#{app})",
        string(protocol)
    ));
    let product_context = w.add(format!("PRODUCT_CONTEXT('',#{app},'mechanical')"));
    let name = string(&options.product_name);
    let product = w.add(format!("PRODUCT({name},{name},'',(#{product_context}))"));
    w.add(format!(
        "PRODUCT_RELATED_PRODUCT_CATEGORY('part',$,(#{product}))"
    ));
    let formation = w.add(format!("PRODUCT_DEFINITION_FORMATION('','',#{product})"));
    let definition_context = w.add(format!(
        "PRODUCT_DEFINITION_CONTEXT('part definition',#{app},'design')"
    ));
    let definition = w.add(format!(
        "PRODUCT_DEFINITION('design','',#{formation},#{definition_context})"
    ));
    let definition_shape = w.add(format!("PRODUCT_DEFINITION_SHAPE('','',#{definition})"));

    // Units and the representation context.
    let mm = w.add("( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.) )".to_owned());
    let rad = w.add("( NAMED_UNIT(*) PLANE_ANGLE_UNIT() SI_UNIT($,.RADIAN.) )".to_owned());
    let sr = w.add("( NAMED_UNIT(*) SI_UNIT($,.STERADIAN.) SOLID_ANGLE_UNIT() )".to_owned());
    let uncertainty = w.add(format!(
        "UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE({}),#{mm},'distance_accuracy_value',\
         'confusion accuracy')",
        real(tolerance::LINEAR)
    ));
    let context = w.add(format!(
        "( GEOMETRIC_REPRESENTATION_CONTEXT(3) \
         GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#{uncertainty})) \
         GLOBAL_UNIT_ASSIGNED_CONTEXT((#{mm},#{rad},#{sr})) \
         REPRESENTATION_CONTEXT('Context #1','3D Context with UNIT and UNCERTAINTY') )"
    ));

    // Geometry.
    let mut items = vec![w.axis(&Frame::WORLD)];
    for &(body_name, solid) in bodies {
        items.extend(w.solid(body_name, solid));
    }
    let representation = w.add(format!(
        "ADVANCED_BREP_SHAPE_REPRESENTATION({name},{},#{context})",
        refs(&items)
    ));
    w.add(format!(
        "SHAPE_DEFINITION_REPRESENTATION(#{definition_shape},#{representation})"
    ));

    let mut out = String::with_capacity(w.data.len() + 1024);
    out.push_str("ISO-10303-21;\nHEADER;\n");
    out.push_str("FILE_DESCRIPTION(('PeetCAD model'),'2;1');\n");
    let list = |s: &str| format!("({})", string(s));
    let _ = writeln!(
        out,
        "FILE_NAME({name},{},{},{},{},'PeetCAD','');",
        string(&options.timestamp),
        list(&options.author),
        list(&options.organization),
        string(concat!("PeetCAD ", env!("CARGO_PKG_VERSION"))),
    );
    let _ = writeln!(
        out,
        "FILE_SCHEMA(({}));",
        string(options.schema.file_schema())
    );
    out.push_str("ENDSEC;\nDATA;\n");
    out.push_str(&w.data);
    out.push_str("ENDSEC;\nEND-ISO-10303-21;\n");
    out
}

/// Appends numbered entity instances to the DATA section.
#[derive(Default)]
struct Writer {
    data: String,
    count: u32,
}

impl Writer {
    /// Writes `#n=entity;` and returns `n`.
    fn add(&mut self, entity: String) -> u32 {
        self.count += 1;
        let _ = writeln!(self.data, "#{}={entity};", self.count);
        self.count
    }

    fn point(&mut self, p: DVec3) -> u32 {
        self.add(format!(
            "CARTESIAN_POINT('',({},{},{}))",
            real(p.x),
            real(p.y),
            real(p.z)
        ))
    }

    fn direction(&mut self, d: DVec3) -> u32 {
        self.add(format!(
            "DIRECTION('',({},{},{}))",
            real(d.x),
            real(d.y),
            real(d.z)
        ))
    }

    /// A placement: origin, axis (local Z) and reference direction (local X).
    fn axis(&mut self, f: &Frame) -> u32 {
        let origin = self.point(f.origin);
        let z = self.direction(f.z_axis());
        let x = self.direction(f.x_axis());
        self.add(format!("AXIS2_PLACEMENT_3D('',#{origin},#{z},#{x})"))
    }

    fn curve(&mut self, c: &Curve3) -> u32 {
        match c {
            Curve3::Line(l) => {
                let origin = self.point(l.origin);
                let dir = self.direction(l.dir);
                let vector = self.add(format!("VECTOR('',#{dir},{})", real(1.0)));
                self.add(format!("LINE('',#{origin},#{vector})"))
            }
            Curve3::Circle(c) => {
                let axis = self.axis(&c.frame);
                self.add(format!("CIRCLE('',#{axis},{})", real(c.radius)))
            }
            Curve3::Ellipse(e) => {
                let axis = self.axis(&e.frame);
                self.add(format!(
                    "ELLIPSE('',#{axis},{},{})",
                    real(e.major),
                    real(e.minor)
                ))
            }
        }
    }

    fn surface(&mut self, s: &Surface) -> u32 {
        match s {
            Surface::Plane(p) => {
                let axis = self.axis(&p.frame);
                self.add(format!("PLANE('',#{axis})"))
            }
            Surface::Cylinder(c) => {
                let axis = self.axis(&c.frame);
                self.add(format!(
                    "CYLINDRICAL_SURFACE('',#{axis},{})",
                    real(c.radius)
                ))
            }
            Surface::Cone(c) => {
                // STEP wants a positive semi-angle: a cone that narrows along its axis is
                // written about the opposite axis. Turning the frame half a turn about X
                // negates both parameters, which keeps the natural normal.
                let frame = if c.half_angle < 0.0 {
                    Frame {
                        origin: c.frame.origin,
                        rotation: c.frame.rotation
                            * peet_math::DQuat::from_rotation_x(std::f64::consts::PI),
                    }
                } else {
                    c.frame
                };
                let axis = self.axis(&frame);
                self.add(format!(
                    "CONICAL_SURFACE('',#{axis},{},{})",
                    real(c.radius),
                    real(c.half_angle.abs())
                ))
            }
            Surface::Sphere(s) => {
                let axis = self.axis(&s.frame);
                self.add(format!("SPHERICAL_SURFACE('',#{axis},{})", real(s.radius)))
            }
            Surface::Torus(t) => {
                let axis = self.axis(&t.frame);
                if t.major > t.minor {
                    self.add(format!(
                        "TOROIDAL_SURFACE('',#{axis},{},{})",
                        real(t.major),
                        real(t.minor)
                    ))
                } else {
                    // The tube overlaps itself around the axis; the kernel uses its outer part.
                    self.add(format!(
                        "DEGENERATE_TOROIDAL_SURFACE('',#{axis},{},{},.T.)",
                        real(t.major),
                        real(t.minor)
                    ))
                }
            }
        }
    }

    /// Writes a solid's vertices, edges and shells; returns its breps (one per lump).
    fn solid(&mut self, name: &str, s: &Solid) -> Vec<u32> {
        let vertices: Vec<u32> = s
            .vertices
            .iter()
            .map(|v| {
                let p = self.point(v.point);
                self.add(format!("VERTEX_POINT('',#{p})"))
            })
            .collect();
        let edges: Vec<u32> = s
            .edges
            .iter()
            .map(|e| {
                let curve = self.curve(&e.curve);
                let (a, b) = (vertices[e.start.index()], vertices[e.end.index()]);
                self.add(format!("EDGE_CURVE('',#{a},#{b},#{curve},.T.)"))
            })
            .collect();
        let name = string(name);
        lumps(s)
            .into_iter()
            .map(|(outer, voids)| {
                let shell = self.shell(s, outer, &edges, false);
                if voids.is_empty() {
                    return self.add(format!("MANIFOLD_SOLID_BREP({name},#{shell})"));
                }
                let voids: Vec<u32> = voids
                    .into_iter()
                    .map(|v| {
                        let shell = self.shell(s, v, &edges, true);
                        self.add(format!("ORIENTED_CLOSED_SHELL('',*,#{shell},.F.)"))
                    })
                    .collect();
                self.add(format!("BREP_WITH_VOIDS({name},#{shell},{})", refs(&voids)))
            })
            .collect()
    }

    /// Writes a shell's faces and the `CLOSED_SHELL`. `flip` turns every face over (for
    /// voids, see the module docs).
    fn shell(&mut self, s: &Solid, shell: ShellId, edges: &[u32], flip: bool) -> u32 {
        let mut faces = Vec::with_capacity(s.shell(shell).faces.len());
        for &f in &s.shell(shell).faces {
            let face = s.face(f);
            let surface = self.surface(&face.surface);
            let mut bounds = Vec::with_capacity(face.loops.len());
            for (k, &l) in face.loops.iter().enumerate() {
                let mut coedges = s.loop_coedges(l);
                if flip {
                    coedges.reverse();
                }
                let oriented: Vec<u32> = coedges
                    .into_iter()
                    .map(|c| {
                        let c = s.coedge(c);
                        let edge = edges[c.edge.index()];
                        self.add(format!(
                            "ORIENTED_EDGE('',*,*,#{edge},{})",
                            logical(c.reversed == flip)
                        ))
                    })
                    .collect();
                let edge_loop = self.add(format!("EDGE_LOOP('',{})", refs(&oriented)));
                let kind = if k == 0 {
                    "FACE_OUTER_BOUND"
                } else {
                    "FACE_BOUND"
                };
                bounds.push(self.add(format!("{kind}('',#{edge_loop},.T.)")));
            }
            faces.push(self.add(format!(
                "ADVANCED_FACE('',{},#{surface},{})",
                refs(&bounds),
                logical(face.reversed == flip)
            )));
        }
        self.add(format!("CLOSED_SHELL('',{})", refs(&faces)))
    }
}

/// Groups a solid's shells into lumps: each outer shell (positive volume) with the void
/// shells (negative volume) inside it. A void that fits in no outer shell (an inside-out
/// solid, which a valid solid never has) is written as a lump of its own.
fn lumps(s: &Solid) -> Vec<(ShellId, Vec<ShellId>)> {
    let shells: Vec<(ShellId, f64, Aabb)> = (0..s.shells.len() as u32)
        .map(ShellId)
        .filter(|&id| !s.shell(id).faces.is_empty())
        .map(|id| (id, measure::shell_volume(s, id), shell_bounds(s, id)))
        .collect();
    let mut out: Vec<(ShellId, Vec<ShellId>)> = shells
        .iter()
        .filter(|(_, volume, _)| *volume >= 0.0)
        .map(|&(id, _, _)| (id, Vec::new()))
        .collect();
    let slack = DVec3::splat(tolerance::LINEAR);
    for &(id, _, inner) in shells.iter().filter(|(_, volume, _)| *volume < 0.0) {
        let host = shells
            .iter()
            .filter(|(_, volume, outer)| {
                *volume >= 0.0
                    && (outer.min - slack).cmple(inner.min).all()
                    && inner.max.cmple(outer.max + slack).all()
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|&(host, _, _)| host);
        match host.and_then(|h| out.iter_mut().find(|(o, _)| *o == h)) {
            Some((_, voids)) => voids.push(id),
            None => out.push((id, Vec::new())),
        }
    }
    out
}

/// Bounds of a shell's edges (sampled, so arcs count with their bulge).
fn shell_bounds(s: &Solid, shell: ShellId) -> Aabb {
    let mut b = Aabb::EMPTY;
    for &f in &s.shell(shell).faces {
        for &l in &s.face(f).loops {
            for c in s.loop_coedges(l) {
                let e = s.edge(s.coedge(c).edge);
                for i in 0..=8 {
                    b.extend(e.point_at_fraction(f64::from(i) / 8.0));
                }
            }
        }
    }
    b
}

/// `(#a,#b,...)`.
fn refs(ids: &[u32]) -> String {
    let mut out = String::from("(");
    for (i, id) in ids.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let _ = write!(out, "#{id}");
    }
    out.push(')');
    out
}

fn logical(b: bool) -> &'static str {
    if b { ".T." } else { ".F." }
}

/// A Part 21 REAL that reads back as exactly `v`: Rust's shortest round-trip form, with a
/// decimal point always present and the exponent as `E-07` / `E+20`. Non-finite values
/// (which valid geometry never has) are written as `0.0` to keep the file readable.
pub fn real(v: f64) -> String {
    if !v.is_finite() {
        return "0.0".to_owned();
    }
    // `{:?}` gives the shortest representation that round-trips: `1.0`, `0.1`, `1e-7`,
    // `1.5e20`. Negative zero is written as zero.
    let v = if v == 0.0 { 0.0 } else { v };
    let text = format!("{v:?}");
    let (mantissa, exponent) = match text.split_once('e') {
        Some((m, e)) => (m, Some(e)),
        None => (text.as_str(), None),
    };
    let mut out = mantissa.to_owned();
    if !out.contains('.') {
        out.push_str(".0");
    }
    if let Some(e) = exponent {
        let (sign, digits) = match e.strip_prefix('-') {
            Some(d) => ('-', d),
            None => ('+', e),
        };
        let _ = write!(out, "E{sign}{digits:0>2}");
    }
    out
}

/// A Part 21 string literal, quotes included. Apostrophes and backslashes are doubled,
/// characters outside printable ASCII are encoded with `\X2\` (16-bit) or `\X4\` (32-bit)
/// runs ended by `\X0\`, and control characters become spaces.
pub fn string(s: &str) -> String {
    #[derive(PartialEq)]
    enum Run {
        Ascii,
        X2,
        X4,
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    let mut run = Run::Ascii;
    for c in s.chars() {
        let c = if c.is_control() { ' ' } else { c };
        let code = c as u32;
        let want = if c.is_ascii() {
            Run::Ascii
        } else if code <= 0xFFFF {
            Run::X2
        } else {
            Run::X4
        };
        if want != run {
            if run != Run::Ascii {
                out.push_str("\\X0\\");
            }
            match want {
                Run::Ascii => {}
                Run::X2 => out.push_str("\\X2\\"),
                Run::X4 => out.push_str("\\X4\\"),
            }
            run = want;
        }
        match run {
            Run::Ascii => match c {
                '\'' => out.push_str("''"),
                '\\' => out.push_str("\\\\"),
                _ => out.push(c),
            },
            Run::X2 => {
                let _ = write!(out, "{code:04X}");
            }
            Run::X4 => {
                let _ = write!(out, "{code:08X}");
            }
        }
    }
    if run != Run::Ascii {
        out.push_str("\\X0\\");
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reals_have_a_point_and_round_trip() {
        for (v, text) in [
            (1.0, "1.0"),
            (-0.25, "-0.25"),
            (0.0, "0.0"),
            (-0.0, "0.0"),
            (1e-7, "1.0E-07"),
            (1.5e-7, "1.5E-07"),
            (1e20, "1.0E+20"),
            (-2.5e-300, "-2.5E-300"),
            (123456.789, "123456.789"),
            (f64::NAN, "0.0"),
        ] {
            assert_eq!(real(v), text, "{v}");
        }
        for v in [
            0.1,
            1.0 / 3.0,
            std::f64::consts::PI,
            6.123233995736766e-17,
            -1e-5,
            1e23,
            0.1 + 0.2,
            f64::MAX,
            f64::MIN_POSITIVE,
        ] {
            let text = real(v);
            assert!(text.contains('.'), "{text}");
            assert_eq!(text.parse::<f64>().unwrap(), v, "{text}");
        }
    }

    #[test]
    fn strings_are_escaped() {
        assert_eq!(string("Bracket"), "'Bracket'");
        assert_eq!(string("Bill's part"), "'Bill''s part'");
        assert_eq!(string("a\\b"), "'a\\\\b'");
        assert_eq!(string("Größe"), "'Gr\\X2\\00F600DF\\X0\\e'");
        assert_eq!(string("x\u{1F600}"), "'x\\X4\\0001F600\\X0\\'");
        assert_eq!(
            string("é\u{1F600}"),
            "'\\X2\\00E9\\X0\\\\X4\\0001F600\\X0\\'"
        );
        assert_eq!(string("two\nlines"), "'two lines'");
        assert_eq!(string(""), "''");
    }
}
