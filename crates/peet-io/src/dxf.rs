//! DXF export of sheet metal flat patterns.
//!
//! The file is AutoCAD R12 ASCII DXF (`AC1009`), the version every laser, plasma and
//! punching program reads. Units are millimetres (`$INSUNITS` = 4). The pattern is seen
//! from the top side of the sheet, in the flat-pattern coordinates of the part (a base
//! flange keeps its sketch coordinates), and uses these layers:
//!
//! | Layer        | Contents                                                   |
//! |--------------|------------------------------------------------------------|
//! | `OUTLINE`    | outer profile of the blank                                 |
//! | `CUTOUTS`    | holes and inner cutouts (round holes as circles)           |
//! | `BEND`       | bend lines (the middle of each bend region), dashed        |
//! | `BEND_NOTES` | one note per bend: direction, angle and inner radius       |
//! | `FORMS`      | outlines and centre marks of dimples, embosses and louvers |
//! | `FORM_NOTES` | one note per form: kind, direction and height              |
//!
//! "UP" means the flange bends (or the form stands) towards the viewer (the top side of
//! the sheet). Forms are pressed, not cut, so their outlines are kept off the cutting
//! layers; the lance of a louver is cut, and is on `CUTOUTS`.

use std::collections::HashSet;
use std::fmt::Write as _;

use peet_math::DVec2;
use peet_sheetmetal::{Report, SheetBody};
use peet_sketch::Curve;

pub mod layer {
    pub const OUTLINE: &str = "OUTLINE";
    pub const CUTOUTS: &str = "CUTOUTS";
    pub const BEND: &str = "BEND";
    pub const BEND_NOTES: &str = "BEND_NOTES";
    pub const FORMS: &str = "FORMS";
    pub const FORM_NOTES: &str = "FORM_NOTES";
}

/// A 2D drawing entity.
#[derive(Clone, Debug, PartialEq)]
pub enum Entity {
    Line {
        layer: &'static str,
        a: DVec2,
        b: DVec2,
        dashed: bool,
    },
    /// Counter-clockwise from `start` to `end` (degrees).
    Arc {
        layer: &'static str,
        center: DVec2,
        radius: f64,
        start: f64,
        end: f64,
    },
    Circle {
        layer: &'static str,
        center: DVec2,
        radius: f64,
    },
    /// Text centred on `at`, turned by `rotation` degrees.
    Text {
        layer: &'static str,
        at: DVec2,
        height: f64,
        rotation: f64,
        text: String,
    },
}

/// The entities of a sheet metal body's flat pattern.
pub fn flat_pattern_entities(sheet: &SheetBody) -> Vec<Entity> {
    let mut out = Vec::new();
    let mut seen: HashSet<[i64; 6]> = HashSet::new();
    let q = |v: f64| (v * 1e5).round() as i64;
    for l in &sheet.outline {
        let layer = if l.outer {
            layer::OUTLINE
        } else {
            layer::CUTOUTS
        };
        // A loop of arcs of one circle is a round hole (or a disc).
        if let Some((center, radius)) = whole_circle(&l.edges) {
            if seen.insert([3, q(center.x), q(center.y), q(radius), 0, 0]) {
                out.push(Entity::Circle {
                    layer,
                    center,
                    radius,
                });
            }
            continue;
        }
        for (curve, _) in &l.edges {
            match *curve {
                Curve::Line { a, b } => {
                    // A slit (tear relief) gives the same segment twice: cut it once.
                    let (p, r) = if (a.x, a.y) <= (b.x, b.y) {
                        (a, b)
                    } else {
                        (b, a)
                    };
                    if seen.insert([1, q(p.x), q(p.y), q(r.x), q(r.y), 0]) {
                        out.push(Entity::Line {
                            layer,
                            a,
                            b,
                            dashed: false,
                        });
                    }
                }
                Curve::Arc {
                    center,
                    radius,
                    start_angle,
                    sweep,
                } => {
                    let start = start_angle.to_degrees().rem_euclid(360.0);
                    let end = (start_angle + sweep).to_degrees().rem_euclid(360.0);
                    if seen.insert([2, q(center.x), q(center.y), q(radius), q(start), q(end)]) {
                        out.push(Entity::Arc {
                            layer,
                            center,
                            radius,
                            start,
                            end,
                        });
                    }
                }
                Curve::Circle { center, radius } => out.push(Entity::Circle {
                    layer,
                    center,
                    radius,
                }),
            }
        }
    }

    let report: Report = sheet.report();
    let (lo, hi) = sheet.flat_bounds();
    let height = ((hi - lo).max_element() / 80.0).clamp(1.5, 10.0);
    for line in &sheet.bend_lines {
        for s in &line.segments {
            out.push(Entity::Line {
                layer: layer::BEND,
                a: s[0],
                b: s[1],
                dashed: true,
            });
        }
        let (Some(row), Some(longest)) = (
            report.bends.iter().find(|b| b.piece == line.piece),
            line.segments
                .iter()
                .max_by(|a, b| a[0].distance(a[1]).total_cmp(&b[0].distance(b[1]))),
        ) else {
            continue;
        };
        let dir = (longest[1] - longest[0]).normalize_or(DVec2::X);
        // Text reads left to right (or bottom to top), just beside the line.
        let (dir, mut rotation) = (dir, dir.y.atan2(dir.x).to_degrees());
        if rotation > 90.0 + 1e-9 {
            rotation -= 180.0;
        } else if rotation <= -90.0 + 1e-9 {
            rotation += 180.0;
        }
        let side = DVec2::new(-dir.y, dir.x);
        let side = if side.y < 0.0 || (side.y == 0.0 && side.x > 0.0) {
            -side
        } else {
            side
        };
        out.push(Entity::Text {
            layer: layer::BEND_NOTES,
            at: (longest[0] + longest[1]) / 2.0 + side * height,
            height,
            rotation,
            text: format!(
                "{} {}%%d R{}",
                if row.up { "UP" } else { "DOWN" },
                number(row.angle, 2),
                number(row.radius, 3)
            ),
        });
    }

    for form in &sheet.forms {
        // A louver's lance is cut; the other sides are only marked.
        let lance = form.lance;
        let is_lance = |a: DVec2, b: DVec2| {
            lance.is_some_and(|l| {
                (l[0].distance(a) < 1e-9 && l[1].distance(b) < 1e-9)
                    || (l[0].distance(b) < 1e-9 && l[1].distance(a) < 1e-9)
            })
        };
        let mut size = 0.0_f64;
        for (curve, _) in &form.outline {
            match *curve {
                Curve::Line { a, b } => {
                    size = size.max(a.distance(form.center));
                    out.push(Entity::Line {
                        layer: if is_lance(a, b) {
                            layer::CUTOUTS
                        } else {
                            layer::FORMS
                        },
                        a,
                        b,
                        dashed: false,
                    });
                }
                Curve::Circle { center, radius } => {
                    size = size.max(radius);
                    out.push(Entity::Circle {
                        layer: layer::FORMS,
                        center,
                        radius,
                    });
                }
                Curve::Arc {
                    center,
                    radius,
                    start_angle,
                    sweep,
                } => {
                    size = size.max(radius);
                    out.push(Entity::Arc {
                        layer: layer::FORMS,
                        center,
                        radius,
                        start: start_angle.to_degrees().rem_euclid(360.0),
                        end: (start_angle + sweep).to_degrees().rem_euclid(360.0),
                    });
                }
            }
        }
        // A centre mark for the punch, and a note.
        let arm = (size / 4.0).clamp(0.5, 5.0);
        for d in [DVec2::X, DVec2::Y] {
            out.push(Entity::Line {
                layer: layer::FORMS,
                a: form.center - d * arm,
                b: form.center + d * arm,
                dashed: false,
            });
        }
        out.push(Entity::Text {
            layer: layer::FORM_NOTES,
            at: form.center + DVec2::new(0.0, arm + height),
            height,
            rotation: 0.0,
            text: format!(
                "{} {} {}",
                form.kind.label().to_uppercase(),
                if form.up { "UP" } else { "DOWN" },
                number(form.height, 3)
            ),
        });
    }
    out
}

/// Centre and radius if every edge is an arc of the same circle.
fn whole_circle(edges: &[(Curve, bool)]) -> Option<(DVec2, f64)> {
    let mut circle: Option<(DVec2, f64)> = None;
    let mut sweep_total = 0.0;
    for (c, _) in edges {
        let (center, radius, sweep) = match *c {
            Curve::Arc {
                center,
                radius,
                sweep,
                ..
            } => (center, radius, sweep),
            Curve::Circle { center, radius } => (center, radius, std::f64::consts::TAU),
            Curve::Line { .. } => return None,
        };
        match circle {
            None => circle = Some((center, radius)),
            Some((c0, r0)) if c0.distance(center) <= 1e-6 && (r0 - radius).abs() <= 1e-6 => {}
            Some(_) => return None,
        }
        sweep_total += sweep;
    }
    circle.filter(|_| (sweep_total - std::f64::consts::TAU).abs() < 1e-6)
}

/// A number with up to `decimals` decimals and no trailing zeros.
fn number(v: f64, decimals: usize) -> String {
    let s = format!("{v:.decimals$}");
    let s = if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        s
    };
    if s == "-0" { "0".to_owned() } else { s }
}

/// A coordinate for the file.
fn coord(v: f64) -> String {
    number(v, 6)
}

/// Writes entities as an R12 ASCII DXF file.
pub fn write(entities: &[Entity]) -> String {
    let mut lo = DVec2::splat(f64::INFINITY);
    let mut hi = DVec2::splat(f64::NEG_INFINITY);
    let mut grow = |p: DVec2| {
        lo = lo.min(p);
        hi = hi.max(p);
    };
    for e in entities {
        match e {
            Entity::Line { a, b, .. } => {
                grow(*a);
                grow(*b);
            }
            Entity::Arc { center, radius, .. } | Entity::Circle { center, radius, .. } => {
                grow(*center - DVec2::splat(*radius));
                grow(*center + DVec2::splat(*radius));
            }
            Entity::Text { at, .. } => grow(*at),
        }
    }
    if lo.x > hi.x {
        lo = DVec2::ZERO;
        hi = DVec2::ZERO;
    }

    let mut s = String::new();
    let mut pair = |code: i32, value: &str| {
        let _ = writeln!(s, "{code}\n{value}");
    };
    pair(0, "SECTION");
    pair(2, "HEADER");
    pair(9, "$ACADVER");
    pair(1, "AC1009");
    pair(9, "$INSUNITS");
    pair(70, "4");
    pair(9, "$EXTMIN");
    pair(10, &coord(lo.x));
    pair(20, &coord(lo.y));
    pair(30, "0");
    pair(9, "$EXTMAX");
    pair(10, &coord(hi.x));
    pair(20, &coord(hi.y));
    pair(30, "0");
    pair(0, "ENDSEC");

    pair(0, "SECTION");
    pair(2, "TABLES");
    pair(0, "TABLE");
    pair(2, "LTYPE");
    pair(70, "2");
    for (name, description, pattern) in [
        ("CONTINUOUS", "Solid line", &[][..]),
        ("DASHED", "Dashed __ __ __", &[6.0, -3.0][..]),
    ] {
        pair(0, "LTYPE");
        pair(2, name);
        pair(70, "0");
        pair(3, description);
        pair(72, "65");
        pair(73, &pattern.len().to_string());
        pair(40, &coord(pattern.iter().map(|d: &f64| d.abs()).sum()));
        for d in pattern {
            pair(49, &coord(*d));
        }
    }
    pair(0, "ENDTAB");
    pair(0, "TABLE");
    pair(2, "LAYER");
    pair(70, "6");
    for (name, color, linetype) in [
        (layer::OUTLINE, "7", "CONTINUOUS"),
        (layer::CUTOUTS, "4", "CONTINUOUS"),
        (layer::BEND, "3", "DASHED"),
        (layer::BEND_NOTES, "2", "CONTINUOUS"),
        (layer::FORMS, "6", "CONTINUOUS"),
        (layer::FORM_NOTES, "2", "CONTINUOUS"),
    ] {
        pair(0, "LAYER");
        pair(2, name);
        pair(70, "0");
        pair(62, color);
        pair(6, linetype);
    }
    pair(0, "ENDTAB");
    pair(0, "ENDSEC");

    pair(0, "SECTION");
    pair(2, "ENTITIES");
    for e in entities {
        match e {
            Entity::Line {
                layer,
                a,
                b,
                dashed,
            } => {
                pair(0, "LINE");
                pair(8, layer);
                if *dashed {
                    pair(6, "DASHED");
                }
                pair(10, &coord(a.x));
                pair(20, &coord(a.y));
                pair(30, "0");
                pair(11, &coord(b.x));
                pair(21, &coord(b.y));
                pair(31, "0");
            }
            Entity::Arc {
                layer,
                center,
                radius,
                start,
                end,
            } => {
                pair(0, "ARC");
                pair(8, layer);
                pair(10, &coord(center.x));
                pair(20, &coord(center.y));
                pair(30, "0");
                pair(40, &coord(*radius));
                pair(50, &coord(*start));
                pair(51, &coord(*end));
            }
            Entity::Circle {
                layer,
                center,
                radius,
            } => {
                pair(0, "CIRCLE");
                pair(8, layer);
                pair(10, &coord(center.x));
                pair(20, &coord(center.y));
                pair(30, "0");
                pair(40, &coord(*radius));
            }
            Entity::Text {
                layer,
                at,
                height,
                rotation,
                text,
            } => {
                pair(0, "TEXT");
                pair(8, layer);
                pair(10, &coord(at.x));
                pair(20, &coord(at.y));
                pair(30, "0");
                pair(40, &coord(*height));
                pair(1, text);
                pair(50, &coord(*rotation));
                pair(72, "1");
                pair(73, "2");
                pair(11, &coord(at.x));
                pair(21, &coord(at.y));
                pair(31, "0");
            }
        }
    }
    pair(0, "ENDSEC");
    pair(0, "EOF");
    s
}

/// The flat pattern of a sheet metal body as a DXF file.
pub fn flat_pattern(sheet: &SheetBody) -> String {
    write(&flat_pattern_entities(sheet))
}

/// Reading DXF back, for tests: the entities of the ENTITIES section as `(type, layer,
/// group codes)`.
pub mod read {
    use std::collections::BTreeMap;

    /// One entity: its type, and every group code with its values in order.
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct Raw {
        pub kind: String,
        pub codes: BTreeMap<i32, Vec<String>>,
    }

    impl Raw {
        pub fn get(&self, code: i32) -> Option<&str> {
            self.codes
                .get(&code)
                .and_then(|v| v.first())
                .map(String::as_str)
        }

        pub fn num(&self, code: i32) -> f64 {
            self.get(code)
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(f64::NAN)
        }

        pub fn layer(&self) -> &str {
            self.get(8).unwrap_or("")
        }
    }

    /// Parses the entities of an ASCII DXF. `Err` if the group structure is broken.
    pub fn entities(text: &str) -> Result<Vec<Raw>, String> {
        let lines: Vec<&str> = text.lines().collect();
        if !lines.len().is_multiple_of(2) {
            return Err("odd number of lines".to_owned());
        }
        let mut out = Vec::new();
        let mut in_entities = false;
        let mut current: Option<Raw> = None;
        let mut last_section_name = false;
        for pair in lines.chunks(2) {
            let code: i32 = pair[0]
                .trim()
                .parse()
                .map_err(|_| format!("bad group code {:?}", pair[0]))?;
            let value = pair[1].trim_end();
            if code == 0 {
                if let Some(e) = current.take() {
                    out.push(e);
                }
                if value == "ENDSEC" {
                    in_entities = false;
                }
                last_section_name = value == "SECTION";
                if in_entities {
                    current = Some(Raw {
                        kind: value.to_owned(),
                        ..Raw::default()
                    });
                }
                continue;
            }
            if code == 2 && last_section_name {
                in_entities = value == "ENTITIES";
                last_section_name = false;
                continue;
            }
            if let Some(e) = &mut current {
                e.codes.entry(code).or_default().push(value.to_owned());
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_reads_back() {
        let entities = vec![
            Entity::Line {
                layer: layer::OUTLINE,
                a: DVec2::ZERO,
                b: DVec2::new(10.5, 0.0),
                dashed: false,
            },
            Entity::Arc {
                layer: layer::OUTLINE,
                center: DVec2::new(1.0, 2.0),
                radius: 3.0,
                start: 0.0,
                end: 90.0,
            },
            Entity::Circle {
                layer: layer::CUTOUTS,
                center: DVec2::new(5.0, 5.0),
                radius: 1.25,
            },
            Entity::Text {
                layer: layer::BEND_NOTES,
                at: DVec2::new(1.0, 1.0),
                height: 2.5,
                rotation: 90.0,
                text: "UP 90%%d R1".to_owned(),
            },
        ];
        let text = write(&entities);
        assert!(text.starts_with("0\nSECTION\n2\nHEADER\n"));
        assert!(text.ends_with("0\nEOF\n"));
        let raw = read::entities(&text).unwrap();
        assert_eq!(raw.len(), 4);
        assert_eq!(raw[0].kind, "LINE");
        assert_eq!(raw[0].num(11), 10.5);
        assert_eq!(raw[1].kind, "ARC");
        assert_eq!(raw[1].num(51), 90.0);
        assert_eq!(raw[2].layer(), layer::CUTOUTS);
        assert_eq!(raw[2].num(40), 1.25);
        assert_eq!(raw[3].get(1), Some("UP 90%%d R1"));
    }

    #[test]
    fn numbers_are_short() {
        assert_eq!(number(90.0, 2), "90");
        assert_eq!(number(1.50, 3), "1.5");
        assert_eq!(number(-0.0000001, 3), "0");
        assert_eq!(coord(12.3456789), "12.345679");
    }
}
