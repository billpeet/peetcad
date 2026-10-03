//! DXF import: hand-written files with known geometry (R12 and AC1027 style), a round
//! trip through our own flat-pattern export, and a check that no input makes it panic.

use std::f64::consts::{FRAC_PI_2, PI};

use peet_io::dxf;
use peet_io::dxf_import::{ImportOptions, Placement, Unit, UnitSource, import, import_into};
use peet_kernel::validate::measure;
use peet_math::{DVec2, Plane};
use peet_sheetmetal::{
    Area, BendModel, EdgeFlangeSpec, EdgeSite, FlangePosition, Layout, ReliefType, SheetSettings,
    build,
};
use peet_sketch::region::find_regions;
use peet_sketch::{ConstraintKind, Curve, Sketch, shapes};
use proptest::prelude::*;

fn v(x: f64, y: f64) -> DVec2 {
    DVec2::new(x, y)
}

/// A DXF from `(code, value)` pairs, one per line.
fn dxf(pairs: &[(i32, &str)], line_end: &str, indent: &str) -> String {
    pairs
        .iter()
        .map(|(c, v)| format!("{indent}{c}{line_end}{v}{line_end}"))
        .collect()
}

/// Every curve of the sketch (not points), with its construction flag.
fn curves(s: &Sketch) -> Vec<(Curve, bool)> {
    s.entities()
        .filter(|(_, e)| e.kind().is_curve())
        .map(|(id, e)| (s.curve(id).unwrap(), e.construction))
        .collect()
}

fn coincidences(s: &Sketch) -> usize {
    s.constraints()
        .filter(|(_, c)| matches!(c.kind, ConstraintKind::Coincident(..)))
        .count()
}

#[track_caller]
fn has_line(s: &Sketch, a: DVec2, b: DVec2) {
    let found = curves(s).iter().any(|(c, _)| match *c {
        Curve::Line { a: p, b: q } => {
            (p.abs_diff_eq(a, 1e-9) && q.abs_diff_eq(b, 1e-9))
                || (p.abs_diff_eq(b, 1e-9) && q.abs_diff_eq(a, 1e-9))
        }
        _ => false,
    });
    assert!(found, "no line {a} - {b} in {:#?}", curves(s));
}

#[track_caller]
fn has_arc(s: &Sketch, center: DVec2, start: DVec2, end: DVec2) {
    let found = curves(s).iter().any(|(c, _)| {
        matches!(*c, Curve::Arc { .. })
            && c.center().unwrap().abs_diff_eq(center, 1e-9)
            && c.start().abs_diff_eq(start, 1e-9)
            && c.end().abs_diff_eq(end, 1e-9)
    });
    assert!(
        found,
        "no arc around {center} from {start} to {end} in {:#?}",
        curves(s)
    );
}

#[track_caller]
fn has_circle(s: &Sketch, center: DVec2, radius: f64) {
    let found = curves(s).iter().any(|(c, _)| {
        matches!(*c, Curve::Circle { .. })
            && c.center().unwrap().abs_diff_eq(center, 1e-9)
            && (c.radius().unwrap() - radius).abs() < 1e-9
    });
    assert!(found, "no circle {center} r{radius} in {:#?}", curves(s));
}

#[track_caller]
fn close(a: f64, b: f64, tol: f64) {
    assert!((a - b).abs() <= tol, "{a} != {b} (±{tol})");
}

/// R12: LINE, ARC, CIRCLE, a closed POLYLINE with a bulge, and TEXT, in inches.
fn r12_inches() -> String {
    let mut p: Vec<(i32, &str)> = vec![
        (0, "SECTION"),
        (2, "HEADER"),
        (9, "$ACADVER"),
        (1, "AC1009"),
        (9, "$INSUNITS"),
        (70, "1"),
        (0, "ENDSEC"),
        (0, "SECTION"),
        (2, "ENTITIES"),
    ];
    // A 4 × 2 plate with a round right-hand end and a hole.
    p.extend([
        (0, "LINE"),
        (8, "0"),
        (10, "0.0"),
        (20, "0.0"),
        (30, "0.0"),
        (11, "4.0"),
        (21, "0.0"),
        (31, "0.0"),
        (0, "ARC"),
        (8, "0"),
        (10, "4.0"),
        (20, "1.0"),
        (30, "0.0"),
        (40, "1.0"),
        (50, "270.0"),
        (51, "90.0"),
        (0, "LINE"),
        (8, "0"),
        (10, "4.0"),
        (20, "2.0"),
        (11, "0.0"),
        (21, "2.0"),
        (0, "LINE"),
        (8, "0"),
        (10, "0.0"),
        (20, "2.0"),
        (11, "0.0"),
        (21, "0.0"),
        (0, "CIRCLE"),
        (8, "HOLES"),
        (10, "2.0"),
        (20, "1.0"),
        (40, "0.5"),
    ]);
    // A 2 × 2 square with a bulged right side, as a closed POLYLINE.
    p.extend([
        (0, "POLYLINE"),
        (8, "0"),
        (66, "1"),
        (10, "0.0"),
        (20, "0.0"),
        (30, "0.0"),
        (70, "1"),
        (0, "VERTEX"),
        (8, "0"),
        (10, "10.0"),
        (20, "0.0"),
        (0, "VERTEX"),
        (8, "0"),
        (10, "12.0"),
        (20, "0.0"),
        (42, "1.0"),
        (0, "VERTEX"),
        (8, "0"),
        (10, "12.0"),
        (20, "2.0"),
        (0, "VERTEX"),
        (8, "0"),
        (10, "10.0"),
        (20, "2.0"),
        (0, "SEQEND"),
        (8, "0"),
        (0, "TEXT"),
        (8, "NOTES"),
        (10, "0"),
        (20, "5"),
        (40, "0.2"),
        (1, "PLATE"),
        (0, "ENDSEC"),
        (0, "EOF"),
    ]);
    dxf(&p, "\n", "")
}

#[test]
fn r12_file_in_inches() {
    let out = import(r12_inches().as_bytes(), &ImportOptions::default()).unwrap();
    let (s, r) = (&out.sketch, &out.report);
    let i = 25.4;
    assert_eq!(r.unit_used, Unit::Inches);
    assert_eq!(r.unit_source, UnitSource::File);
    assert_eq!(r.entities_imported, 9, "{r:#?}");
    assert_eq!(r.added.len(), 9);
    has_line(s, v(0.0, 0.0), v(4.0 * i, 0.0));
    has_arc(s, v(4.0 * i, i), v(4.0 * i, 0.0), v(4.0 * i, 2.0 * i));
    has_circle(s, v(2.0 * i, i), 0.5 * i);
    has_line(s, v(10.0 * i, 0.0), v(12.0 * i, 0.0));
    has_arc(s, v(12.0 * i, i), v(12.0 * i, 0.0), v(12.0 * i, 2.0 * i));
    has_line(s, v(10.0 * i, 2.0 * i), v(10.0 * i, 0.0));
    assert_eq!(r.skipped, vec![("TEXT".to_owned(), 1)]);
    assert!(
        r.warnings.iter().any(|w| w.contains("1 TEXT")),
        "{:?}",
        r.warnings
    );
    // Four corners in each profile, joined by coincident relations.
    assert_eq!(coincidences(s), 8);
    let names: Vec<(&str, usize)> = r
        .layers
        .iter()
        .map(|l| (l.name.as_str(), l.entities))
        .collect();
    assert_eq!(names, vec![("0", 5), ("HOLES", 1), ("NOTES", 1)]);

    // Two closed profiles: the plate with its hole, and the square with a round side
    // (and the hole's inside, which region detection also reports as a face).
    let profile = find_regions(s);
    assert_eq!(profile.regions.len(), 3);
    let plate = &profile.regions[profile.region_at(v(0.5 * i, 0.5 * i)).unwrap()];
    assert_eq!(plate.holes.len(), 1);
    close(plate.area(), (8.0 + FRAC_PI_2 - PI * 0.25) * i * i, 1e-6);
    let square = &profile.regions[profile.region_at(v(11.0 * i, i)).unwrap()];
    close(square.area(), (4.0 + FRAC_PI_2) * i * i, 1e-6);
}

#[test]
fn unit_override_and_placement() {
    let options = ImportOptions {
        unit: Some(Unit::Millimetres),
        placement: Placement::LowerLeftAtOrigin,
        ..ImportOptions::default()
    };
    let out = import(r12_inches().as_bytes(), &options).unwrap();
    assert_eq!(out.report.unit_used, Unit::Millimetres);
    assert_eq!(out.report.unit_source, UnitSource::Caller);
    // The drawing already starts at the origin, so nothing moves.
    assert_eq!(out.report.offset, DVec2::ZERO);
    has_circle(&out.sketch, v(2.0, 1.0), 0.5);

    let options = ImportOptions {
        unit: Some(Unit::Millimetres),
        placement: Placement::Centred,
        ..ImportOptions::default()
    };
    let out = import(r12_inches().as_bytes(), &options).unwrap();
    // Bounds 0..13 × 0..2: centred, the hole moves by (−6.5, −1).
    assert!(out.report.offset.abs_diff_eq(v(-6.5, -1.0), 1e-12));
    has_circle(&out.sketch, v(-4.5, 0.0), 0.5);
}

/// AC1027 (AutoCAD 2013) style: CRLF, indented codes, handles and subclass markers,
/// LWPOLYLINE with bulges, a block inserted with rotation and scale, a mirrored arc, a
/// circular ELLIPSE, and things to skip.
fn ac1027() -> String {
    let mut p: Vec<(i32, &str)> = vec![
        (999, "written by hand"),
        (0, "SECTION"),
        (2, "HEADER"),
        (9, "$ACADVER"),
        (1, "AC1027"),
        (9, "$INSBASE"),
        (10, "0.0"),
        (20, "0.0"),
        (30, "0.0"),
        (9, "$INSUNITS"),
        (70, "4"),
        (0, "ENDSEC"),
        (0, "SECTION"),
        (2, "TABLES"),
        (0, "TABLE"),
        (2, "LAYER"),
        (5, "2"),
        (100, "AcDbSymbolTable"),
        (70, "1"),
        (0, "LAYER"),
        (5, "10"),
        (100, "AcDbSymbolTableRecord"),
        (100, "AcDbLayerTableRecord"),
        (2, "0"),
        (70, "0"),
        (62, "7"),
        (6, "Continuous"),
        (0, "ENDTAB"),
        (0, "ENDSEC"),
        (0, "SECTION"),
        (2, "BLOCKS"),
        (0, "BLOCK"),
        (5, "20"),
        (100, "AcDbEntity"),
        (8, "0"),
        (100, "AcDbBlockBegin"),
        (2, "PEG"),
        (70, "0"),
        (10, "1.0"),
        (20, "0.0"),
        (30, "0.0"),
        (3, "PEG"),
        (1, ""),
        (0, "CIRCLE"),
        (5, "21"),
        (100, "AcDbEntity"),
        (8, "0"),
        (100, "AcDbCircle"),
        (10, "6.0"),
        (20, "0.0"),
        (30, "0.0"),
        (40, "2.0"),
        (0, "LINE"),
        (5, "22"),
        (100, "AcDbEntity"),
        (8, "MARKS"),
        (100, "AcDbLine"),
        (10, "1.0"),
        (20, "0.0"),
        (30, "0.0"),
        (11, "3.0"),
        (21, "0.0"),
        (31, "0.0"),
        (0, "ENDBLK"),
        (5, "23"),
        (100, "AcDbEntity"),
        (8, "0"),
        (100, "AcDbBlockEnd"),
        (0, "ENDSEC"),
        (0, "SECTION"),
        (2, "ENTITIES"),
    ];
    // A 20 × 10 slot: straight sides, half-circle ends from bulges of 1.
    p.extend([
        (0, "LWPOLYLINE"),
        (5, "30"),
        (330, "1F"),
        (100, "AcDbEntity"),
        (8, "PROFILE"),
        (100, "AcDbPolyline"),
        (90, "4"),
        (70, "1"),
        (43, "0.0"),
        (10, "0.0"),
        (20, "0.0"),
        (10, "20.0"),
        (20, "0.0"),
        (42, "1.0"),
        (10, "20.0"),
        (20, "10.0"),
        (10, "0.0"),
        (20, "10.0"),
        (42, "1.0"),
    ]);
    // The block, at (100, 50), turned 90° and twice the size.
    p.extend([
        (0, "INSERT"),
        (5, "31"),
        (100, "AcDbEntity"),
        (8, "PEGS"),
        (100, "AcDbBlockReference"),
        (2, "PEG"),
        (10, "100.0"),
        (20, "50.0"),
        (30, "0.0"),
        (41, "2.0"),
        (42, "2.0"),
        (43, "2.0"),
        (50, "90.0"),
    ]);
    // The same block stretched: skipped.
    p.extend([
        (0, "INSERT"),
        (8, "PEGS"),
        (2, "PEG"),
        (10, "0.0"),
        (20, "0.0"),
        (41, "2.0"),
        (42, "3.0"),
    ]);
    // An arc seen from below: OCS centre (40, 0), 0° to 90°.
    p.extend([
        (0, "ARC"),
        (5, "32"),
        (100, "AcDbEntity"),
        (8, "PROFILE"),
        (100, "AcDbCircle"),
        (10, "40.0"),
        (20, "0.0"),
        (30, "0.0"),
        (40, "10.0"),
        (210, "0.0"),
        (220, "0.0"),
        (230, "-1.0"),
        (100, "AcDbArc"),
        (50, "0.0"),
        (51, "90.0"),
    ]);
    // A circular ellipse: the upper half of a circle of radius 5 around (0, −30).
    p.extend([
        (0, "ELLIPSE"),
        (100, "AcDbEntity"),
        (8, "PROFILE"),
        (100, "AcDbEllipse"),
        (10, "0.0"),
        (20, "-30.0"),
        (30, "0.0"),
        (11, "5.0"),
        (21, "0.0"),
        (31, "0.0"),
        (210, "0.0"),
        (220, "0.0"),
        (230, "1.0"),
        (40, "1.0"),
        (41, "0.0"),
        (42, "3.141592653589793"),
    ]);
    // Skipped: a real ellipse, a spline, text, a dimension, a tilted circle, paper space.
    p.extend([
        (0, "ELLIPSE"),
        (8, "PROFILE"),
        (10, "0.0"),
        (20, "0.0"),
        (11, "5.0"),
        (21, "0.0"),
        (40, "0.5"),
        (0, "SPLINE"),
        (8, "PROFILE"),
        (70, "8"),
        (71, "3"),
        (0, "MTEXT"),
        (8, "TEXT"),
        (1, "Hello"),
        (0, "DIMENSION"),
        (8, "DIMS"),
        (2, "*D1"),
        (0, "CIRCLE"),
        (8, "PROFILE"),
        (10, "0.0"),
        (20, "0.0"),
        (40, "1.0"),
        (210, "1.0"),
        (220, "0.0"),
        (230, "0.0"),
        (0, "LINE"),
        (67, "1"),
        (8, "PROFILE"),
        (10, "0.0"),
        (20, "0.0"),
        (11, "1.0"),
        (21, "1.0"),
        (0, "ENDSEC"),
        (0, "SECTION"),
        (2, "OBJECTS"),
        (0, "DICTIONARY"),
        (5, "C"),
        (0, "ENDSEC"),
        (0, "EOF"),
    ]);
    dxf(&p, "\r\n", "  ")
}

#[test]
fn ac1027_file_with_blocks_and_bulges() {
    let out = import(ac1027().as_bytes(), &ImportOptions::default()).unwrap();
    let (s, r) = (&out.sketch, &out.report);
    assert_eq!(r.unit_used, Unit::Millimetres);
    assert_eq!(r.unit_source, UnitSource::File);

    // The slot.
    has_line(s, v(0.0, 0.0), v(20.0, 0.0));
    has_arc(s, v(20.0, 5.0), v(20.0, 0.0), v(20.0, 10.0));
    has_line(s, v(20.0, 10.0), v(0.0, 10.0));
    has_arc(s, v(0.0, 5.0), v(0.0, 10.0), v(0.0, 0.0));
    // The block: (6, 0) − base (1, 0) = (5, 0), doubled to (10, 0), turned to (0, 10).
    has_circle(s, v(100.0, 60.0), 4.0);
    has_line(s, v(100.0, 50.0), v(100.0, 54.0));
    // The mirrored arc runs counter-clockwise from (−40, 10) to (−50, 0).
    has_arc(s, v(-40.0, 0.0), v(-40.0, 10.0), v(-50.0, 0.0));
    // The ellipse's upper half.
    has_arc(s, v(0.0, -30.0), v(5.0, -30.0), v(-5.0, -30.0));
    assert_eq!(r.entities_imported, 8, "{r:#?}");

    let skipped: Vec<(&str, usize)> = r.skipped.iter().map(|(k, n)| (k.as_str(), *n)).collect();
    assert_eq!(
        skipped,
        vec![
            ("DIMENSION", 1),
            ("ELLIPSE (not a circle)", 1),
            ("INSERT (different X and Y scales)", 1),
            ("MTEXT", 1),
            ("SPLINE", 1),
            ("not in the XY plane", 1),
            ("paper space", 1),
        ]
    );
    let all = r.warnings.join("\n");
    for needle in [
        "1 spline skipped",
        "1 ellipse skipped",
        "different X and Y scales",
        "XY plane",
        "paper space",
        "1 DIMENSION, 1 MTEXT",
    ] {
        assert!(all.contains(needle), "{needle:?} not in {all}");
    }
    // Block contents on layer 0 take the insert's layer; others keep theirs.
    let layer = |n: &str| r.layers.iter().find(|l| l.name == n).map(|l| l.entities);
    assert_eq!(layer("PEGS"), Some(3), "two inserts and one circle");
    assert_eq!(layer("MARKS"), Some(1));
    assert_eq!(layer("PROFILE"), Some(7));

    // The slot is closed: one region of 20 × 10 plus a circle of radius 5.
    let profile = find_regions(s);
    let slot = profile
        .regions
        .iter()
        .find(|g| g.contains(v(10.0, 5.0)))
        .expect("the slot is a region");
    close(slot.area(), 200.0 + 25.0 * PI, 1e-9);
}

#[test]
fn layers_can_be_skipped_or_made_construction() {
    let options = ImportOptions {
        skip_layers: vec!["holes".to_owned()],
        layers_as_construction: vec!["0".to_owned()],
        ..ImportOptions::default()
    };
    let out = import(r12_inches().as_bytes(), &options).unwrap();
    let c = curves(&out.sketch);
    assert_eq!(c.len(), 8);
    assert!(c.iter().all(|(_, construction)| *construction));
    assert!(
        out.report
            .skipped
            .contains(&("on a skipped layer".to_owned(), 1))
    );
    // Construction geometry makes no regions.
    assert!(find_regions(&out.sketch).regions.is_empty());
}

#[test]
fn open_chain_stays_open() {
    let text = dxf(
        &[
            (0, "SECTION"),
            (2, "ENTITIES"),
            (0, "LINE"),
            (10, "0"),
            (20, "0"),
            (11, "50"),
            (21, "0"),
            (0, "LINE"),
            (10, "50"),
            (20, "0"),
            (11, "50"),
            (21, "30"),
            (0, "LINE"),
            (10, "50.0000000001"),
            (20, "30"),
            (11, "80"),
            (21, "30"),
            (0, "ENDSEC"),
            (0, "EOF"),
        ],
        "\n",
        "",
    );
    let out = import(text.as_bytes(), &ImportOptions::default()).unwrap();
    assert_eq!(out.report.unit_source, UnitSource::Assumed);
    assert!(out.report.warnings[0].contains("millimetres were assumed"));
    let profile = find_regions(&out.sketch);
    assert!(profile.regions.is_empty());
    assert_eq!(profile.open_ends.len(), 2);
    // Two joints; the tiny gap was closed onto the exact end of the second line.
    assert_eq!(coincidences(&out.sketch), 2);
    has_line(&out.sketch, v(50.0, 30.0), v(80.0, 30.0));
}

#[test]
fn import_into_appends_to_a_sketch() {
    let mut sketch = Sketch::new();
    let rect = shapes::rectangle(&mut sketch, v(-10.0, -10.0), v(-5.0, -5.0));
    let before = sketch.entities().count();
    let report = import_into(
        &mut sketch,
        r12_inches().as_bytes(),
        &ImportOptions::default(),
    )
    .unwrap();
    assert_eq!(report.entities_imported, 9);
    assert!(sketch.entity(rect.curves[0]).is_some());
    assert!(sketch.entities().count() > before);
    assert!(report.added.iter().all(|id| sketch.entity(*id).is_some()));
    // The file can't be read: the sketch is left alone.
    let snapshot = sketch.clone();
    assert!(import_into(&mut sketch, b"0\nSECTION\n2", &ImportOptions::default()).is_err());
    assert_eq!(sketch, snapshot);
}

#[test]
fn clear_errors() {
    let err = |bytes: &[u8]| {
        import(bytes, &ImportOptions::default())
            .unwrap_err()
            .message
    };
    assert!(err(b"AutoCAD Binary DXF\r\n\x1a\0\0\0").contains("binary DXF"));
    assert!(err(b"").contains("empty"));
    assert!(err(b"hello world\n").contains("doesn't look like a DXF"));
    assert!(err(b"0\nLINE\n").contains("no SECTION"));
    let m = err(b"0\nSECTION\n2\nENTITIES\n0\nLINE\n10\nabc\n0\nENDSEC\n0\nEOF\n");
    assert!(m.contains("Line 8") && m.contains("abc"), "{m}");
    let m = err(b"0\nSECTION\n2\nENTITIES\nx0\nLINE\n");
    assert!(m.contains("Line 5"), "{m}");
    let m = err(b"0\nSECTION\n2\nENTITIES\n0\n");
    assert!(m.contains("cut short"), "{m}");
}

#[test]
fn block_bombs_are_refused_or_skipped() {
    let self_insert = dxf(
        &[
            (0, "SECTION"),
            (2, "BLOCKS"),
            (0, "BLOCK"),
            (2, "A"),
            (0, "LINE"),
            (11, "1"),
            (0, "INSERT"),
            (2, "a"),
            (10, "5"),
            (0, "ENDBLK"),
            (0, "ENDSEC"),
            (0, "SECTION"),
            (2, "ENTITIES"),
            (0, "INSERT"),
            (2, "A"),
            (0, "ENDSEC"),
            (0, "EOF"),
        ],
        "\n",
        "",
    );
    let out = import(self_insert.as_bytes(), &ImportOptions::default()).unwrap();
    assert_eq!(out.report.entities_imported, 1);
    assert!(
        out.report
            .skipped
            .contains(&("INSERT (block inserts itself)".to_owned(), 1))
    );

    let array = dxf(
        &[
            (0, "SECTION"),
            (2, "BLOCKS"),
            (0, "BLOCK"),
            (2, "E"),
            (0, "ENDBLK"),
            (0, "ENDSEC"),
            (0, "SECTION"),
            (2, "ENTITIES"),
            (0, "INSERT"),
            (2, "E"),
            (70, "32767"),
            (71, "32767"),
            (0, "ENDSEC"),
            (0, "EOF"),
        ],
        "\n",
        "",
    );
    let e = import(array.as_bytes(), &ImportOptions::default()).unwrap_err();
    assert!(e.message.contains("too large"), "{e}");
}

// ---- Round trip through our own flat-pattern export ----

const T: f64 = 1.5;

/// A 100 × 60 plate with an edge flange (set back, so with reliefs) on the top edge, a
/// round hole and a slot.
fn sheet() -> peet_sheetmetal::SheetBody {
    let settings = SheetSettings {
        thickness: T,
        radius: 2.0,
        model: BendModel::KFactor(0.44),
        relief: ReliefType::Rectangular,
        relief_ratio: 0.5,
    };
    let mut plate = Sketch::new();
    shapes::rectangle(&mut plate, DVec2::ZERO, v(100.0, 60.0));
    let regions = find_regions(&plate).regions;
    let mut layout = Layout::plate(settings, 1, &Plane::TOP, &regions, false).unwrap();
    layout
        .add_edge_flange(
            2,
            &EdgeSite {
                piece: 0,
                a: v(100.0, 60.0),
                b: v(0.0, 60.0),
                top: true,
            },
            &EdgeFlangeSpec {
                length: 20.0,
                angle: 90.0,
                position: FlangePosition::MaterialInside,
                offsets: [10.0, 10.0],
                flip: false,
                radius: None,
            },
        )
        .unwrap();
    let mut cuts = Sketch::new();
    cuts.add_circle(v(30.0, 20.0), 4.0);
    shapes::slot(&mut cuts, v(50.0, 20.0), v(70.0, 20.0), 3.0);
    layout.add_cut(Area::from_regions(&find_regions(&cuts).regions, 3));
    let (_, body) = build(layout).unwrap_or_else(|e| panic!("{}", e.message(|o| o.to_string())));
    body
}

#[test]
fn flat_pattern_round_trip() {
    let body = sheet();
    let flat_area = measure::volume(&body.flat) / T;
    let holes = body.outline.iter().filter(|l| !l.outer).count();
    assert_eq!(holes, 2);
    let text = dxf::flat_pattern(&body);

    let out = import(text.as_bytes(), &ImportOptions::flat_pattern()).unwrap();
    let (s, r) = (&out.sketch, &out.report);
    assert_eq!(r.unit_used, Unit::Millimetres);
    assert_eq!(r.unit_source, UnitSource::File);
    assert!(
        r.skipped.contains(&("on a skipped layer".to_owned(), 1)),
        "{r:#?}"
    );
    // The bend line comes back as construction.
    let bend_segments: usize = body.bend_lines.iter().map(|b| b.segments.len()).sum();
    let construction = curves(s).iter().filter(|(_, c)| *c).count();
    assert_eq!(construction, bend_segments);

    // One blank with two holes (region detection also reports each hole's inside).
    let profile = find_regions(s);
    assert!(profile.open_ends.is_empty(), "{:?}", profile.open_ends);
    assert_eq!(profile.regions.len(), 3);
    let region = &profile.regions[profile.region_at(v(30.0, 40.0)).unwrap()];
    assert_eq!(region.holes.len(), 2);
    close(region.area(), flat_area, 1e-4);
    // The holes are where they were cut.
    assert!(!region.contains(v(30.0, 20.0)));
    assert!(!region.contains(v(60.0, 20.0)));
    assert!(region.contains(v(30.0, 40.0)));

    // Without the option the bend line splits the blank in two.
    let plain = import(text.as_bytes(), &ImportOptions::default()).unwrap();
    let parts = find_regions(&plain.sketch);
    let plate = parts.region_at(v(30.0, 40.0)).unwrap();
    let flange = parts.region_at(v(50.0, 70.0)).unwrap();
    assert_ne!(plate, flange);
    close(
        parts.regions[plate].area() + parts.regions[flange].area(),
        flat_area,
        1e-4,
    );
}

// ---- Nothing panics ----

/// Tokens that make the reader take interesting paths.
const TOKENS: &[&str] = &[
    "0",
    "2",
    "8",
    "10",
    "20",
    "11",
    "40",
    "42",
    "50",
    "70",
    "71",
    "210",
    "230",
    "SECTION",
    "ENDSEC",
    "ENTITIES",
    "BLOCKS",
    "HEADER",
    "BLOCK",
    "ENDBLK",
    "INSERT",
    "POLYLINE",
    "VERTEX",
    "SEQEND",
    "LWPOLYLINE",
    "ARC",
    "CIRCLE",
    "ELLIPSE",
    "LINE",
    "EOF",
    "$INSUNITS",
    "PEG",
    "1e308",
    "-1e308",
    "nan",
    "inf",
    "-0",
    "0.0",
    "1",
    "-1",
    "32767",
    "",
    "  ",
    "\u{1a}",
    "é",
];

fn mutated() -> impl Strategy<Value = Vec<u8>> {
    let base = prop_oneof![Just(r12_inches()), Just(ac1027())];
    let edit = (0usize..4, any::<prop::sample::Index>(), 0..TOKENS.len());
    (base, prop::collection::vec(edit, 1..12)).prop_map(|(text, edits)| {
        let mut lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
        for (op, at, token) in edits {
            if lines.is_empty() {
                break;
            }
            let i = at.index(lines.len());
            match op {
                0 => lines[i] = TOKENS[token].to_owned(),
                1 => {
                    lines.remove(i);
                }
                2 => lines.insert(i, TOKENS[token].to_owned()),
                _ => {
                    let j = (i + token) % lines.len();
                    lines.swap(i, j);
                }
            }
        }
        lines.join("\n").into_bytes()
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn random_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..400)) {
        let _ = import(&bytes, &ImportOptions::default());
    }

    #[test]
    fn mutated_files_never_panic(bytes in mutated()) {
        if let Ok(out) = import(&bytes, &ImportOptions::default()) {
            // Whatever comes in, the sketch is usable for region detection.
            let _ = find_regions(&out.sketch);
        }
    }
}
