//! Line-art icons for the ribbon and the feature tree, drawn with egui shapes.
//!
//! Every icon is drawn on a 24 × 24 grid scaled to the target rectangle, with a neutral
//! stroke plus one accent colour for its key element. The accent colour comes from the
//! icon's category, so all sketch tools share one colour, all sheet metal tools another,
//! and so on. Drawing the icons keeps the app free of image files and image decoders.

use std::f32::consts::{FRAC_PI_2, PI};

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke};
use peet_model::{FeatureKind, Operation};

use crate::commands::CommandId;

/// What an icon belongs to, which sets its accent colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Neutral,
    View,
    Sketch,
    Solid,
    Cut,
    Reference,
    SheetMetal,
    Export,
    Confirm,
}

impl Category {
    pub fn color(self, dark: bool) -> Color32 {
        let (d, l) = match self {
            Self::Neutral => ((150, 160, 175), (95, 105, 120)),
            Self::View => ((120, 170, 235), (45, 110, 200)),
            Self::Sketch => ((90, 160, 255), (30, 105, 220)),
            Self::Solid => ((255, 160, 60), (215, 115, 10)),
            Self::Cut => ((245, 100, 90), (205, 55, 45)),
            Self::Reference => ((180, 140, 245), (125, 80, 205)),
            Self::SheetMetal => ((60, 200, 185), (10, 145, 130)),
            Self::Export => ((110, 200, 120), (40, 145, 60)),
            Self::Confirm => ((110, 210, 120), (35, 150, 55)),
        };
        let (r, g, b) = if dark { d } else { l };
        Color32::from_rgb(r, g, b)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Save,
    Undo,
    Redo,
    ViewIso,
    ViewFront,
    ViewTop,
    ViewRight,
    Fit,
    Perspective,
    Grid,
    Planes,
    NewSketch,
    Sketch,
    EditSketch,
    Parameters,
    Extrude,
    Cut,
    Reference,
    RefPlane,
    RefAxis,
    RefPoint,
    CoordSystem,
    BaseFlange,
    EdgeFlange,
    SheetCut,
    FlatPattern,
    BendTable,
    ExportDxf,
    ExportStl,
    Hem,
    SketchedBend,
    Jog,
    MiterFlange,
    Corner,
    Dimple,
    Emboss,
    Louver,
    LinearPattern,
    CircularPattern,
    MirrorFeature,
    Checks,
    Gauge,
    ExportStep,
    ImportDxf,
    Revolve,
    CutRevolve,
    FilletEdge,
    Chamfer,
    Shell,
    Draft,
    Hole,
    ImportStep,
    MassProperties,
    Sweep,
    CutSweep,
    Loft,
    CutLoft,
    ConvertToSheet,
    Search,
    Check,
    Select,
    Line,
    Rectangle,
    CenterRectangle,
    Circle,
    Arc,
    Spline,
    Slot,
    Polygon,
    Point,
    Dimension,
    Trim,
    Extend,
    Fillet,
    Offset,
    Mirror,
    Construction,
    Relations,
    NewDocument,
    Open,
    SaveAs,
    Sample,
    Settings,
    Quit,
    Delete,
    Suppress,
    RollToEnd,
    ViewCube,
    TreePanel,
    PropertiesPanel,
    Performance,
    Keyboard,
    About,
}

impl Icon {
    /// The icon for a command, if it has one.
    pub fn for_command(cmd: CommandId) -> Option<Self> {
        use CommandId as C;
        Some(match cmd {
            C::SaveDocument => Self::Save,
            C::Undo => Self::Undo,
            C::Redo => Self::Redo,
            C::ViewIsometric => Self::ViewIso,
            C::ViewFront | C::ViewBack => Self::ViewFront,
            C::ViewTop | C::ViewBottom => Self::ViewTop,
            C::ViewRight | C::ViewLeft => Self::ViewRight,
            C::ZoomToFit => Self::Fit,
            C::ToggleProjection => Self::Perspective,
            C::ToggleGrid => Self::Grid,
            C::ToggleReferencePlanes => Self::Planes,
            C::NewSketch => Self::NewSketch,
            C::EditSketch => Self::EditSketch,
            C::ExitSketch => Self::Check,
            C::Parameters | C::ConfigurationTable => Self::Parameters,
            C::Extrude => Self::Extrude,
            C::CutExtrude => Self::Cut,
            C::RefPlane => Self::RefPlane,
            C::RefAxis => Self::RefAxis,
            C::RefPoint => Self::RefPoint,
            C::RefCoordSystem => Self::CoordSystem,
            C::BaseFlange => Self::BaseFlange,
            C::EdgeFlange => Self::EdgeFlange,
            C::SheetCut => Self::SheetCut,
            C::FlatPattern => Self::FlatPattern,
            C::BendTable => Self::BendTable,
            C::ExportDxf => Self::ExportDxf,
            C::ExportStl => Self::ExportStl,
            C::Hem => Self::Hem,
            C::SketchedBend => Self::SketchedBend,
            C::Jog => Self::Jog,
            C::MiterFlange => Self::MiterFlange,
            C::CornerTreatment => Self::Corner,
            C::Dimple => Self::Dimple,
            C::Emboss => Self::Emboss,
            C::Louver => Self::Louver,
            C::LinearPattern => Self::LinearPattern,
            C::CircularPattern => Self::CircularPattern,
            C::MirrorFeature => Self::MirrorFeature,
            C::SheetChecks => Self::Checks,
            C::GaugeTables => Self::Gauge,
            C::ExportStep => Self::ExportStep,
            C::ImportDxf => Self::ImportDxf,
            C::Revolve => Self::Revolve,
            C::CutRevolve => Self::CutRevolve,
            C::Fillet => Self::FilletEdge,
            C::Chamfer => Self::Chamfer,
            C::Shell => Self::Shell,
            C::Draft => Self::Draft,
            C::Hole => Self::Hole,
            C::ImportStep => Self::ImportStep,
            C::MassProperties => Self::MassProperties,
            C::Sweep => Self::Sweep,
            C::CutSweep => Self::CutSweep,
            C::Loft => Self::Loft,
            C::CutLoft => Self::CutLoft,
            C::ConvertToSheet => Self::ConvertToSheet,
            // The assembly's commands borrow the icons of what they are most like.
            C::NewAssembly => Self::NewDocument,
            C::InsertComponent => Self::Open,
            C::EditComponent => Self::EditSketch,
            C::InsertLinkedComponent => Self::ImportStep,
            C::UpdateLinks => Self::RollToEnd,
            C::InterferenceCheck => Self::Checks,
            C::BillOfMaterials => Self::BendTable,
            C::MateCoincident => Self::RefPlane,
            C::MateConcentric => Self::Circle,
            C::MateParallel => Self::Offset,
            C::MateDistance => Self::Dimension,
            C::MateAngle => Self::Draft,
            C::MateFasten => Self::Relations,
            C::ShowAllComponents => Self::Planes,
            C::IsolateComponent => Self::Select,
            C::AddExplodeStep => Self::Offset,
            C::ExplodeView => Self::ViewIso,
            C::LinearComponentPattern => Self::LinearPattern,
            C::CircularComponentPattern => Self::CircularPattern,
            C::OpenSampleAssembly => Self::Sample,
            C::CommandPalette => Self::Search,
            C::SketchSelect => Self::Select,
            C::SketchLine => Self::Line,
            C::SketchRectangle => Self::Rectangle,
            C::SketchCenterRectangle => Self::CenterRectangle,
            C::SketchCircle => Self::Circle,
            C::SketchArc => Self::Arc,
            C::SketchSpline => Self::Spline,
            C::SketchSlot => Self::Slot,
            C::SketchPolygon => Self::Polygon,
            C::SketchPoint => Self::Point,
            C::SmartDimension => Self::Dimension,
            C::SketchTrim => Self::Trim,
            C::SketchExtend => Self::Extend,
            C::SketchFillet => Self::Fillet,
            C::SketchOffset => Self::Offset,
            C::SketchMirror => Self::Mirror,
            C::ToggleConstruction => Self::Construction,
            C::ToggleRelations => Self::Relations,
            C::NewDocument => Self::NewDocument,
            C::OpenDocument => Self::Open,
            C::SaveDocumentAs => Self::SaveAs,
            C::OpenSample
            | C::OpenSampleEnclosure
            | C::OpenSampleChassis
            | C::OpenSampleHousing => Self::Sample,
            C::Settings => Self::Settings,
            C::Quit => Self::Quit,
            C::DeleteSelection => Self::Delete,
            C::ToggleSuppress => Self::Suppress,
            C::RollToEnd => Self::RollToEnd,
            C::ToggleViewCube => Self::ViewCube,
            C::ToggleFeatureTree => Self::TreePanel,
            C::ToggleProperties => Self::PropertiesPanel,
            C::TogglePerfOverlay => Self::Performance,
            C::KeyboardShortcuts => Self::Keyboard,
            C::About => Self::About,
            _ => return None,
        })
    }

    /// The icon for a feature in the tree.
    pub fn for_feature(kind: &FeatureKind) -> Self {
        match kind {
            FeatureKind::Sketch(_) => Self::Sketch,
            FeatureKind::Extrude(e) if e.params.operation == Operation::Cut => Self::Cut,
            FeatureKind::Extrude(_) => Self::Extrude,
            FeatureKind::Plane(_) => Self::RefPlane,
            FeatureKind::Axis(_) => Self::RefAxis,
            FeatureKind::Point(_) => Self::RefPoint,
            FeatureKind::CoordSystem(_) => Self::CoordSystem,
            FeatureKind::BaseFlange(_) => Self::BaseFlange,
            FeatureKind::EdgeFlange(_) => Self::EdgeFlange,
            FeatureKind::SheetCut(_) => Self::SheetCut,
            FeatureKind::Hem(_) => Self::Hem,
            FeatureKind::SketchedBend(_) => Self::SketchedBend,
            FeatureKind::Jog(_) => Self::Jog,
            FeatureKind::MiterFlange(_) => Self::MiterFlange,
            FeatureKind::Corner(_) => Self::Corner,
            FeatureKind::Form(f) => match f.kind {
                peet_sheetmetal::FormKind::Dimple => Self::Dimple,
                peet_sheetmetal::FormKind::Emboss => Self::Emboss,
                peet_sheetmetal::FormKind::Louver => Self::Louver,
            },
            FeatureKind::Pattern(p) => match p.def {
                peet_model::PatternDef::Linear { .. } => Self::LinearPattern,
                peet_model::PatternDef::Circular { .. } => Self::CircularPattern,
            },
            FeatureKind::Mirror(_) => Self::MirrorFeature,
            FeatureKind::Revolve(r) if r.operation == Operation::Cut => Self::CutRevolve,
            FeatureKind::Revolve(_) => Self::Revolve,
            FeatureKind::Blend(b) => match b.kind {
                peet_model::BlendKind::Fillet => Self::FilletEdge,
                peet_model::BlendKind::Chamfer => Self::Chamfer,
            },
            FeatureKind::Shell(_) => Self::Shell,
            FeatureKind::Draft(_) => Self::Draft,
            FeatureKind::Hole(_) => Self::Hole,
            FeatureKind::Import(_) => Self::ImportStep,
            FeatureKind::Sweep(s) if s.operation == Operation::Cut => Self::CutSweep,
            FeatureKind::Sweep(_) => Self::Sweep,
            FeatureKind::Loft(l) if l.operation == Operation::Cut => Self::CutLoft,
            FeatureKind::Loft(_) => Self::Loft,
            FeatureKind::ConvertToSheet(_) => Self::ConvertToSheet,
        }
    }

    pub fn category(self) -> Category {
        use Icon as I;
        match self {
            I::Save
            | I::Undo
            | I::Redo
            | I::Search
            | I::Parameters
            | I::BendTable
            | I::Gauge
            | I::MassProperties => Category::Neutral,
            I::Checks => Category::Confirm,
            I::LinearPattern | I::CircularPattern | I::MirrorFeature => Category::Solid,
            I::ViewIso
            | I::ViewFront
            | I::ViewTop
            | I::ViewRight
            | I::Fit
            | I::Perspective
            | I::Grid
            | I::Planes => Category::View,
            I::NewSketch
            | I::Sketch
            | I::EditSketch
            | I::Select
            | I::Line
            | I::Rectangle
            | I::CenterRectangle
            | I::Circle
            | I::Arc
            | I::Spline
            | I::Slot
            | I::Polygon
            | I::Point
            | I::Dimension
            | I::Trim
            | I::Extend
            | I::Fillet
            | I::Offset
            | I::Mirror
            | I::Construction
            | I::Relations => Category::Sketch,
            I::Extrude
            | I::Revolve
            | I::Sweep
            | I::Loft
            | I::FilletEdge
            | I::Chamfer
            | I::Shell
            | I::Draft => Category::Solid,
            I::Cut | I::CutRevolve | I::CutSweep | I::CutLoft | I::Hole => Category::Cut,
            I::Reference | I::RefPlane | I::RefAxis | I::RefPoint | I::CoordSystem => {
                Category::Reference
            }
            I::BaseFlange
            | I::EdgeFlange
            | I::SheetCut
            | I::FlatPattern
            | I::Hem
            | I::SketchedBend
            | I::Jog
            | I::MiterFlange
            | I::Corner
            | I::ConvertToSheet
            | I::Dimple
            | I::Emboss
            | I::Louver => Category::SheetMetal,
            I::ExportDxf | I::ExportStl | I::ExportStep | I::ImportDxf | I::ImportStep => {
                Category::Export
            }
            I::Check => Category::Confirm,
            I::Delete => Category::Cut,
            I::NewDocument
            | I::Open
            | I::SaveAs
            | I::Sample
            | I::Settings
            | I::Quit
            | I::Suppress
            | I::RollToEnd
            | I::Keyboard
            | I::About => Category::Neutral,
            I::ViewCube | I::TreePanel | I::PropertiesPanel | I::Performance => Category::View,
        }
    }

    /// Draws the icon into `rect` (square). `fg` is the neutral line colour; `accent` is
    /// usually [`Category::color`] of the icon.
    pub fn paint(self, painter: &Painter, rect: Rect, fg: Color32, accent: Color32) {
        let pen = Pen::new(painter, rect, fg, accent);
        pen.draw(self);
    }
}

/// Draws on a 24 × 24 grid mapped onto a screen rectangle.
struct Pen<'a> {
    painter: &'a Painter,
    origin: Pos2,
    scale: f32,
    fg: Color32,
    accent: Color32,
}

impl<'a> Pen<'a> {
    fn new(painter: &'a Painter, rect: Rect, fg: Color32, accent: Color32) -> Self {
        let size = rect.width().min(rect.height());
        Self {
            painter,
            origin: rect.center() - egui::vec2(size, size) * 0.5,
            scale: size / 24.0,
            fg,
            accent,
        }
    }

    fn p(&self, (x, y): (f32, f32)) -> Pos2 {
        self.origin + egui::vec2(x, y) * self.scale
    }

    fn stroke(&self, color: Color32) -> Stroke {
        Stroke::new((1.6 * self.scale).max(1.0), color)
    }

    fn pts(&self, pts: &[(f32, f32)]) -> Vec<Pos2> {
        pts.iter().map(|&q| self.p(q)).collect()
    }

    fn line(&self, pts: &[(f32, f32)], color: Color32) {
        self.painter
            .add(Shape::line(self.pts(pts), self.stroke(color)));
    }

    fn closed(&self, pts: &[(f32, f32)], color: Color32) {
        self.painter
            .add(Shape::closed_line(self.pts(pts), self.stroke(color)));
    }

    /// A convex shape filled with a translucent `color` and outlined with it.
    fn face(&self, pts: &[(f32, f32)], color: Color32) {
        self.painter.add(Shape::convex_polygon(
            self.pts(pts),
            color.gamma_multiply(0.35),
            self.stroke(color),
        ));
    }

    fn solid(&self, pts: &[(f32, f32)], color: Color32) {
        self.painter
            .add(Shape::convex_polygon(self.pts(pts), color, Stroke::NONE));
    }

    fn dashed(&self, pts: &[(f32, f32)], color: Color32) {
        let s = self.scale;
        self.painter.extend(Shape::dashed_line(
            &self.pts(pts),
            self.stroke(color),
            2.5 * s,
            2.0 * s,
        ));
    }

    fn circle(&self, c: (f32, f32), r: f32, color: Color32) {
        self.painter
            .circle_stroke(self.p(c), r * self.scale, self.stroke(color));
    }

    fn disc(&self, c: (f32, f32), r: f32, color: Color32) {
        self.painter.circle_filled(self.p(c), r * self.scale, color);
    }

    /// An arc, angles in radians measured clockwise on screen from +x.
    fn arc(&self, c: (f32, f32), r: f32, from: f32, to: f32, color: Color32) {
        let pts: Vec<(f32, f32)> = (0..=16)
            .map(|i| {
                let a = from + (to - from) * i as f32 / 16.0;
                (c.0 + r * a.cos(), c.1 + r * a.sin())
            })
            .collect();
        self.line(&pts, color);
    }

    /// An arrow from `a` to `b` with a filled head.
    fn arrow(&self, a: (f32, f32), b: (f32, f32), color: Color32) {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = dx.hypot(dy).max(1e-3);
        let (ux, uy) = (dx / len, dy / len);
        let head = 5.0;
        let neck = (b.0 - ux * head, b.1 - uy * head);
        self.line(&[a, neck], color);
        let (sx, sy) = (-uy * head * 0.55, ux * head * 0.55);
        self.solid(
            &[b, (neck.0 + sx, neck.1 + sy), (neck.0 - sx, neck.1 - sy)],
            color,
        );
    }

    /// The outline of a cube seen from the iso view, with one face (if any) highlighted.
    fn cube(&self, face: Option<&[(f32, f32)]>) {
        let (fg, ac) = (self.fg, self.accent);
        if let Some(f) = face {
            self.face(f, ac);
        }
        self.closed(
            &[
                (12.0, 3.0),
                (20.0, 7.5),
                (20.0, 16.5),
                (12.0, 21.0),
                (4.0, 16.5),
                (4.0, 7.5),
            ],
            fg,
        );
        self.line(&[(4.0, 7.5), (12.0, 12.0), (20.0, 7.5)], fg);
        self.line(&[(12.0, 12.0), (12.0, 21.0)], fg);
    }

    /// A block for extrude and cut: front, top and side faces.
    fn block(&self) {
        let fg = self.fg;
        self.closed(&[(3.0, 13.0), (15.0, 13.0), (15.0, 22.0), (3.0, 22.0)], fg);
        self.closed(&[(3.0, 13.0), (7.0, 9.0), (19.0, 9.0), (15.0, 13.0)], fg);
        self.line(&[(15.0, 22.0), (19.0, 18.0), (19.0, 9.0)], fg);
    }

    fn pencil(&self, color: Color32) {
        self.closed(
            &[
                (7.0, 19.0),
                (8.0, 15.0),
                (17.0, 6.0),
                (20.0, 9.0),
                (11.0, 18.0),
            ],
            color,
        );
        self.line(&[(15.0, 8.0), (18.0, 11.0)], color);
    }

    fn document(&self) {
        let fg = self.fg;
        self.closed(
            &[
                (5.0, 2.0),
                (14.0, 2.0),
                (19.0, 7.0),
                (19.0, 22.0),
                (5.0, 22.0),
            ],
            fg,
        );
        self.line(&[(14.0, 2.0), (14.0, 7.0), (19.0, 7.0)], fg);
    }

    fn draw(&self, icon: Icon) {
        let (fg, ac) = (self.fg, self.accent);
        match icon {
            Icon::Save => {
                self.closed(
                    &[
                        (4.0, 4.0),
                        (17.0, 4.0),
                        (20.0, 7.0),
                        (20.0, 20.0),
                        (4.0, 20.0),
                    ],
                    fg,
                );
                self.line(&[(8.0, 4.0), (8.0, 9.0), (15.0, 9.0), (15.0, 4.0)], fg);
                self.face(&[(7.0, 13.0), (17.0, 13.0), (17.0, 20.0), (7.0, 20.0)], ac);
            }
            Icon::Undo | Icon::Redo => {
                let m = |(x, y): (f32, f32)| {
                    if icon == Icon::Redo {
                        (24.0 - x, y)
                    } else {
                        (x, y)
                    }
                };
                let arc: Vec<(f32, f32)> = (0..=12)
                    .map(|i| {
                        let a = -FRAC_PI_2 * i as f32 / 12.0;
                        m((12.0 + 8.0 * a.cos(), 16.0 + 8.0 * a.sin()))
                    })
                    .collect();
                self.line(&arc, fg);
                self.arrow(m((12.0, 8.0)), m((3.0, 8.0)), ac);
            }
            Icon::ViewIso => {
                self.cube(None);
                self.disc((12.0, 12.0), 1.8, ac);
            }
            Icon::ViewFront => {
                self.cube(Some(&[(4.0, 7.5), (12.0, 12.0), (12.0, 21.0), (4.0, 16.5)]))
            }
            Icon::ViewRight => self.cube(Some(&[
                (12.0, 12.0),
                (20.0, 7.5),
                (20.0, 16.5),
                (12.0, 21.0),
            ])),
            Icon::ViewTop => self.cube(Some(&[(12.0, 3.0), (20.0, 7.5), (12.0, 12.0), (4.0, 7.5)])),
            Icon::Fit => {
                for (sx, sy) in [(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] {
                    let c = |dx: f32, dy: f32| (12.0 + sx * dx, 12.0 + sy * dy);
                    self.line(&[c(-9.0, -4.0), c(-9.0, -9.0), c(-4.0, -9.0)], fg);
                }
                self.face(&[(8.0, 8.0), (16.0, 8.0), (16.0, 16.0), (8.0, 16.0)], ac);
            }
            Icon::Perspective => {
                self.closed(&[(3.0, 7.0), (21.0, 3.0), (21.0, 21.0), (3.0, 17.0)], fg);
                self.line(&[(12.0, 5.0), (12.0, 19.0)], ac);
                self.line(&[(3.0, 12.0), (21.0, 12.0)], ac);
            }
            Icon::Grid => {
                for i in 0..4 {
                    let t = 4.0 + 16.0 * i as f32 / 3.0;
                    let c = if i == 0 || i == 3 { fg } else { ac };
                    self.line(&[(t, 4.0), (t, 20.0)], c);
                    self.line(&[(4.0, t), (20.0, t)], c);
                }
            }
            Icon::Planes => {
                self.face(&[(2.0, 17.0), (8.0, 11.0), (22.0, 11.0), (16.0, 17.0)], ac);
                self.closed(&[(6.0, 3.0), (14.0, 3.0), (14.0, 14.0), (6.0, 14.0)], fg);
            }
            Icon::NewSketch | Icon::Sketch => {
                self.dashed(
                    &[
                        (2.0, 21.0),
                        (6.0, 16.0),
                        (22.0, 16.0),
                        (18.0, 21.0),
                        (2.0, 21.0),
                    ],
                    fg,
                );
                self.pencil(ac);
                if icon == Icon::NewSketch {
                    self.line(&[(2.0, 5.0), (9.0, 5.0)], fg);
                    self.line(&[(5.5, 1.5), (5.5, 8.5)], fg);
                }
            }
            Icon::EditSketch => {
                self.pencil(ac);
                self.line(&[(3.0, 21.0), (21.0, 21.0)], fg);
            }
            Icon::Parameters => {
                for (y, knob) in [(6.0, 15.0), (12.0, 8.0), (18.0, 13.0)] {
                    self.line(&[(3.0, y), (21.0, y)], fg);
                    self.disc((knob, y), 2.6, ac);
                }
            }
            Icon::Extrude => {
                self.face(&[(3.0, 13.0), (7.0, 9.0), (19.0, 9.0), (15.0, 13.0)], ac);
                self.block();
                self.arrow((11.0, 11.0), (11.0, 1.0), ac);
            }
            Icon::Cut => {
                self.block();
                self.face(&[(7.0, 12.0), (9.0, 10.0), (15.0, 10.0), (13.0, 12.0)], ac);
                self.arrow((11.0, 0.5), (11.0, 10.5), ac);
            }
            Icon::Reference | Icon::RefPlane => {
                self.face(&[(2.0, 17.0), (8.0, 9.0), (22.0, 9.0), (16.0, 17.0)], ac);
                if icon == Icon::Reference {
                    self.dashed(&[(12.0, 2.0), (12.0, 22.0)], fg);
                }
            }
            Icon::RefAxis => {
                self.dashed(&[(3.0, 21.0), (21.0, 3.0)], ac);
                self.circle((12.0, 12.0), 3.0, fg);
            }
            Icon::RefPoint => {
                self.line(&[(12.0, 3.0), (12.0, 21.0)], fg);
                self.line(&[(3.0, 12.0), (21.0, 12.0)], fg);
                self.disc((12.0, 12.0), 3.5, ac);
            }
            Icon::CoordSystem => {
                let o = (9.0, 15.0);
                self.arrow(o, (22.0, 15.0), Color32::from_rgb(226, 86, 86));
                self.arrow(o, (9.0, 2.0), Color32::from_rgb(112, 196, 92));
                self.arrow(o, (2.0, 22.0), Color32::from_rgb(84, 146, 240));
                self.disc(o, 2.0, ac);
            }
            Icon::BaseFlange => {
                self.face(&[(3.0, 14.0), (9.0, 8.0), (21.0, 8.0), (15.0, 14.0)], ac);
                self.line(
                    &[
                        (3.0, 14.0),
                        (3.0, 17.0),
                        (15.0, 17.0),
                        (21.0, 11.0),
                        (21.0, 8.0),
                    ],
                    fg,
                );
                self.line(&[(15.0, 14.0), (15.0, 17.0)], fg);
            }
            Icon::EdgeFlange => {
                self.closed(&[(2.0, 20.0), (7.0, 15.0), (19.0, 15.0), (14.0, 20.0)], fg);
                self.face(&[(14.0, 20.0), (19.0, 15.0), (19.0, 3.0), (14.0, 8.0)], ac);
            }
            Icon::SheetCut => {
                self.closed(&[(3.0, 16.0), (9.0, 8.0), (21.0, 8.0), (15.0, 16.0)], fg);
                self.face(&[(9.0, 13.5), (11.0, 10.5), (15.0, 10.5), (13.0, 13.5)], ac);
                self.line(&[(3.0, 16.0), (3.0, 18.0), (15.0, 18.0), (15.0, 16.0)], fg);
            }
            Icon::FlatPattern => {
                self.closed(&[(3.0, 7.0), (21.0, 7.0), (21.0, 17.0), (3.0, 17.0)], fg);
                self.dashed(&[(9.0, 7.0), (9.0, 17.0)], ac);
                self.dashed(&[(15.0, 7.0), (15.0, 17.0)], ac);
            }
            Icon::BendTable => {
                self.face(&[(3.0, 4.0), (21.0, 4.0), (21.0, 9.0), (3.0, 9.0)], ac);
                self.closed(&[(3.0, 4.0), (21.0, 4.0), (21.0, 20.0), (3.0, 20.0)], fg);
                self.line(&[(3.0, 14.5), (21.0, 14.5)], fg);
                self.line(&[(10.0, 9.0), (10.0, 20.0)], fg);
            }
            Icon::ExportDxf => {
                self.document();
                self.closed(&[(8.0, 11.0), (16.0, 11.0), (16.0, 18.0), (8.0, 18.0)], ac);
                self.dashed(&[(12.0, 11.0), (12.0, 18.0)], ac);
            }
            Icon::ExportStl => {
                self.document();
                self.face(&[(8.0, 18.0), (12.0, 10.0), (16.0, 18.0)], ac);
            }
            Icon::Hem => {
                // The sheet, folded back over itself.
                self.line(&[(3.0, 16.0), (17.0, 16.0)], fg);
                self.arc((17.0, 13.0), 3.0, -FRAC_PI_2, FRAC_PI_2, ac);
                self.line(&[(17.0, 10.0), (8.0, 10.0)], ac);
            }
            Icon::SketchedBend => {
                self.closed(&[(2.0, 19.0), (8.0, 13.0), (14.0, 13.0), (8.0, 19.0)], fg);
                self.face(&[(8.0, 19.0), (14.0, 13.0), (20.0, 5.0), (14.0, 11.0)], ac);
                self.dashed(&[(8.0, 19.0), (14.0, 13.0)], fg);
            }
            Icon::Jog => {
                self.line(&[(2.0, 17.0), (9.0, 17.0)], fg);
                self.line(&[(9.0, 17.0), (15.0, 8.0)], ac);
                self.line(&[(15.0, 8.0), (22.0, 8.0)], fg);
            }
            Icon::MiterFlange => {
                self.closed(&[(3.0, 19.0), (9.0, 13.0), (21.0, 13.0), (15.0, 19.0)], fg);
                self.face(&[(15.0, 19.0), (21.0, 13.0), (21.0, 5.0), (15.0, 11.0)], ac);
                self.face(&[(9.0, 13.0), (21.0, 13.0), (21.0, 5.0), (9.0, 5.0)], ac);
                self.line(&[(21.0, 13.0), (21.0, 5.0)], fg);
            }
            Icon::Corner => {
                self.line(&[(4.0, 20.0), (4.0, 6.0), (10.0, 6.0)], fg);
                self.line(&[(12.0, 4.0), (12.0, 12.0), (20.0, 12.0)], fg);
                self.face(&[(4.0, 6.0), (10.0, 6.0), (10.0, 12.0), (4.0, 12.0)], ac);
                self.dashed(&[(10.0, 6.0), (12.0, 4.0)], ac);
            }
            Icon::ConvertToSheet => {
                // A solid block on the left turning into a bent sheet on the right.
                self.face(&[(2.0, 9.0), (9.0, 9.0), (9.0, 19.0), (2.0, 19.0)], fg);
                self.arrow((10.5, 14.0), (15.0, 14.0), fg);
                self.line(&[(16.0, 19.0), (22.0, 19.0), (22.0, 6.0)], ac);
                self.line(&[(16.0, 16.5), (19.5, 16.5), (19.5, 6.0)], ac);
            }
            Icon::Dimple => {
                self.line(&[(2.0, 17.0), (7.0, 17.0)], fg);
                self.line(&[(17.0, 17.0), (22.0, 17.0)], fg);
                self.arc((12.0, 17.0), 5.0, PI, 2.0 * PI, ac);
                self.circle((12.0, 8.0), 1.2, ac);
            }
            Icon::Emboss => {
                self.line(&[(2.0, 17.0), (7.0, 17.0), (7.0, 10.0)], fg);
                self.line(&[(7.0, 10.0), (17.0, 10.0)], ac);
                self.line(&[(17.0, 10.0), (17.0, 17.0), (22.0, 17.0)], fg);
            }
            Icon::Louver => {
                self.closed(&[(3.0, 5.0), (21.0, 5.0), (21.0, 19.0), (3.0, 19.0)], fg);
                for y in [9.0, 12.5, 16.0] {
                    self.line(&[(6.0, y), (18.0, y)], ac);
                }
            }
            Icon::LinearPattern => {
                self.face(&[(3.0, 9.0), (8.0, 9.0), (8.0, 15.0), (3.0, 15.0)], ac);
                self.closed(&[(10.0, 9.0), (15.0, 9.0), (15.0, 15.0), (10.0, 15.0)], fg);
                self.closed(&[(17.0, 9.0), (22.0, 9.0), (22.0, 15.0), (17.0, 15.0)], fg);
                self.arrow((4.0, 20.0), (21.0, 20.0), fg);
            }
            Icon::CircularPattern => {
                self.disc((12.0, 4.5), 2.5, ac);
                for (x, y) in [(19.0, 9.5), (16.5, 18.0), (7.5, 18.0), (5.0, 9.5)] {
                    self.circle((x, y), 2.5, fg);
                }
                self.disc((12.0, 12.5), 1.2, fg);
            }
            Icon::MirrorFeature => {
                self.dashed(&[(12.0, 2.0), (12.0, 22.0)], fg);
                self.face(&[(4.0, 7.0), (9.0, 7.0), (9.0, 17.0), (4.0, 14.0)], ac);
                self.closed(&[(20.0, 7.0), (15.0, 7.0), (15.0, 17.0), (20.0, 14.0)], fg);
            }
            Icon::Checks => {
                self.closed(&[(4.0, 3.0), (20.0, 3.0), (20.0, 21.0), (4.0, 21.0)], fg);
                self.line(&[(7.0, 9.0), (9.0, 11.0), (12.0, 7.0)], ac);
                self.line(&[(7.0, 16.0), (9.0, 18.0), (12.0, 14.0)], ac);
                self.line(&[(14.0, 9.5), (18.0, 9.5)], fg);
                self.line(&[(14.0, 16.5), (18.0, 16.5)], fg);
            }
            Icon::Gauge => {
                for (y, w) in [(6.0, 1.0), (11.0, 2.0), (17.0, 3.0)] {
                    self.solid(
                        &[(4.0, y), (20.0, y), (20.0, y + w), (4.0, y + w)],
                        if w > 1.5 { ac } else { fg },
                    );
                }
            }
            Icon::ExportStep => {
                self.document();
                self.face(
                    &[
                        (8.0, 15.0),
                        (12.0, 11.0),
                        (16.0, 13.0),
                        (16.0, 17.0),
                        (12.0, 19.0),
                        (8.0, 18.0),
                    ],
                    ac,
                );
            }
            Icon::ImportDxf => {
                self.document();
                self.arrow((12.0, 9.0), (12.0, 18.0), ac);
            }
            Icon::Revolve | Icon::CutRevolve => {
                // A half profile beside its axis, its other half where it turns to.
                self.dashed(&[(12.0, 3.0), (12.0, 22.0)], fg);
                self.face(&[(14.5, 8.0), (21.0, 8.0), (21.0, 19.0), (14.5, 19.0)], ac);
                let other = [(9.5, 8.0), (3.0, 8.0), (3.0, 19.0), (9.5, 19.0)];
                if icon == Icon::CutRevolve {
                    self.dashed(&[other[0], other[1], other[2], other[3], other[0]], fg);
                } else {
                    self.closed(&other, fg);
                }
                self.arc((12.0, 7.0), 6.0, 1.9 * PI, 1.2 * PI, ac);
                self.solid(&[(5.5, 4.8), (9.2, 1.4), (9.6, 5.6)], ac);
            }
            Icon::Sweep | Icon::CutSweep => {
                // A profile carried round a bend: the two sides of the swept body.
                self.face(&[(3.0, 17.0), (11.0, 17.0), (11.0, 22.0), (3.0, 22.0)], ac);
                self.arc((21.0, 19.0), 18.0, PI, 1.5 * PI, fg);
                self.arc((21.0, 19.0), 10.0, PI, 1.5 * PI, fg);
                self.line(&[(21.0, 1.0), (21.0, 9.0)], fg);
                if icon == Icon::CutSweep {
                    self.dashed(&[(7.0, 17.0), (9.0, 11.0), (14.0, 6.5), (21.0, 5.0)], ac);
                } else {
                    self.arrow((9.0, 11.0), (20.0, 5.0), ac);
                }
            }
            Icon::Loft | Icon::CutLoft => {
                // A wide profile below and a narrow one above, joined by the loft's sides.
                let low = [(2.0, 18.0), (17.0, 18.0), (22.0, 22.0), (7.0, 22.0)];
                let high = [(8.0, 3.0), (15.0, 3.0), (17.5, 6.0), (10.5, 6.0)];
                self.face(&low, ac);
                self.face(&high, ac);
                let sides = [[(2.0, 18.0), (8.0, 3.0)], [(22.0, 22.0), (17.5, 6.0)]];
                for side in sides {
                    if icon == Icon::CutLoft {
                        self.dashed(&side, fg);
                    } else {
                        self.line(&side, fg);
                    }
                }
            }
            Icon::FilletEdge | Icon::Chamfer => {
                // A block seen from the side, its top left corner rounded or cut.
                self.line(
                    &[
                        (4.0, 12.0),
                        (4.0, 20.0),
                        (20.0, 20.0),
                        (20.0, 4.0),
                        (12.0, 4.0),
                    ],
                    fg,
                );
                if icon == Icon::FilletEdge {
                    self.arc((12.0, 12.0), 8.0, PI, 1.5 * PI, ac);
                } else {
                    self.line(&[(4.0, 12.0), (12.0, 4.0)], ac);
                }
            }
            Icon::Shell => {
                // A cup in section: the outside, and the hollow open at the top.
                self.line(
                    &[
                        (8.0, 4.0),
                        (4.0, 4.0),
                        (4.0, 20.0),
                        (20.0, 20.0),
                        (20.0, 4.0),
                        (16.0, 4.0),
                    ],
                    fg,
                );
                self.line(&[(8.0, 4.0), (8.0, 16.0), (16.0, 16.0), (16.0, 4.0)], ac);
            }
            Icon::Draft => {
                // A block with leaning sides, and the upright it leans from.
                self.closed(&[(8.0, 5.0), (16.0, 5.0), (20.0, 20.0), (4.0, 20.0)], fg);
                self.dashed(&[(20.0, 20.0), (20.0, 4.0)], ac);
                self.line(&[(20.0, 20.0), (16.0, 5.0)], ac);
            }
            Icon::Hole => {
                // A drilled hole in section, with the drill's point.
                self.line(&[(2.0, 7.0), (8.0, 7.0)], fg);
                self.line(&[(16.0, 7.0), (22.0, 7.0)], fg);
                self.line(
                    &[
                        (8.0, 7.0),
                        (8.0, 17.0),
                        (12.0, 20.5),
                        (16.0, 17.0),
                        (16.0, 7.0),
                    ],
                    ac,
                );
                self.dashed(&[(12.0, 3.0), (12.0, 22.0)], fg);
            }
            Icon::ImportStep => {
                self.document();
                self.closed(
                    &[
                        (8.0, 15.0),
                        (12.0, 11.0),
                        (16.0, 13.0),
                        (16.0, 17.0),
                        (12.0, 19.0),
                        (8.0, 18.0),
                    ],
                    ac,
                );
                self.arrow((12.0, 5.0), (12.0, 15.5), ac);
            }
            Icon::MassProperties => {
                // A balance.
                self.line(&[(12.0, 4.0), (12.0, 20.0)], fg);
                self.line(&[(7.0, 20.0), (17.0, 20.0)], fg);
                self.line(&[(5.0, 7.0), (19.0, 7.0)], fg);
                for x in [5.0, 19.0] {
                    self.line(&[(x - 3.0, 13.0), (x, 7.0), (x + 3.0, 13.0)], fg);
                    self.arc((x, 13.0), 3.0, 0.0, PI, ac);
                    self.line(&[(x - 3.0, 13.0), (x + 3.0, 13.0)], ac);
                }
            }
            Icon::Search => {
                self.circle((10.0, 10.0), 6.0, fg);
                self.line(&[(14.5, 14.5), (20.0, 20.0)], ac);
            }
            Icon::Check => {
                let s = Stroke::new(2.6 * self.scale, ac);
                self.painter.add(Shape::line(
                    self.pts(&[(4.0, 12.5), (10.0, 18.0), (20.0, 6.0)]),
                    s,
                ));
            }
            Icon::Select => {
                self.closed(
                    &[
                        (6.0, 3.0),
                        (6.0, 19.0),
                        (10.0, 15.0),
                        (13.0, 21.0),
                        (15.5, 20.0),
                        (12.5, 14.0),
                        (18.0, 14.0),
                    ],
                    fg,
                );
                self.solid(&[(6.0, 3.0), (6.0, 19.0), (10.0, 15.0), (18.0, 14.0)], ac);
            }
            Icon::Line => {
                self.line(&[(5.0, 19.0), (19.0, 5.0)], ac);
                self.disc((5.0, 19.0), 2.0, fg);
                self.disc((19.0, 5.0), 2.0, fg);
            }
            Icon::Rectangle | Icon::CenterRectangle => {
                let c = [(4.0, 6.0), (20.0, 6.0), (20.0, 18.0), (4.0, 18.0)];
                self.closed(&c, ac);
                if icon == Icon::Rectangle {
                    for p in c {
                        self.disc(p, 1.8, fg);
                    }
                } else {
                    self.dashed(&[(4.0, 6.0), (20.0, 18.0)], fg);
                    self.disc((12.0, 12.0), 2.0, fg);
                }
            }
            Icon::Circle => {
                self.circle((12.0, 12.0), 8.0, ac);
                self.disc((12.0, 12.0), 1.8, fg);
            }
            Icon::Arc => {
                self.arc((12.0, 17.0), 9.0, PI, 2.0 * PI, ac);
                self.disc((3.0, 17.0), 1.8, fg);
                self.disc((21.0, 17.0), 1.8, fg);
                self.disc((12.0, 17.0), 1.5, fg);
            }
            Icon::Spline => {
                // A wave through three fit points.
                let pts: Vec<(f32, f32)> = (0..=24)
                    .map(|i| {
                        let t = i as f32 / 24.0;
                        (3.0 + 18.0 * t, 12.0 - 6.5 * (2.0 * PI * t).sin())
                    })
                    .collect();
                self.line(&pts, ac);
                self.disc((3.0, 12.0), 1.8, fg);
                self.disc((12.0, 12.0), 1.8, fg);
                self.disc((21.0, 12.0), 1.8, fg);
            }
            Icon::Slot => {
                self.arc((8.0, 12.0), 5.0, FRAC_PI_2, 1.5 * PI, ac);
                self.arc((16.0, 12.0), 5.0, -FRAC_PI_2, FRAC_PI_2, ac);
                self.line(&[(8.0, 7.0), (16.0, 7.0)], ac);
                self.line(&[(8.0, 17.0), (16.0, 17.0)], ac);
                self.disc((8.0, 12.0), 1.4, fg);
                self.disc((16.0, 12.0), 1.4, fg);
            }
            Icon::Polygon => {
                let pts: Vec<(f32, f32)> = (0..6)
                    .map(|i| {
                        let a = PI / 6.0 + i as f32 * PI / 3.0;
                        (12.0 + 9.0 * a.cos(), 12.0 + 9.0 * a.sin())
                    })
                    .collect();
                self.closed(&pts, ac);
                self.disc((12.0, 12.0), 1.5, fg);
            }
            Icon::Point => {
                self.circle((12.0, 12.0), 7.0, fg);
                self.disc((12.0, 12.0), 3.5, ac);
            }
            Icon::Dimension => {
                self.line(&[(3.0, 7.0), (3.0, 21.0)], fg);
                self.line(&[(21.0, 7.0), (21.0, 21.0)], fg);
                self.arrow((12.0, 14.0), (3.5, 14.0), ac);
                self.arrow((12.0, 14.0), (20.5, 14.0), ac);
            }
            Icon::Trim => {
                self.circle((7.0, 18.0), 3.0, fg);
                self.circle((17.0, 18.0), 3.0, fg);
                self.line(&[(9.0, 15.5), (18.0, 3.0)], ac);
                self.line(&[(15.0, 15.5), (6.0, 3.0)], ac);
            }
            Icon::Extend => {
                self.line(&[(2.0, 12.0), (12.0, 12.0)], fg);
                self.dashed(&[(12.0, 12.0), (19.0, 12.0)], ac);
                self.line(&[(21.0, 4.0), (21.0, 20.0)], fg);
                self.arrow((15.0, 12.0), (20.0, 12.0), ac);
            }
            Icon::Fillet => {
                self.line(&[(4.0, 3.0), (4.0, 12.0)], fg);
                self.arc((12.0, 12.0), 8.0, PI, FRAC_PI_2, ac);
                self.line(&[(12.0, 20.0), (21.0, 20.0)], fg);
            }
            Icon::Offset => {
                self.line(&[(4.0, 21.0), (4.0, 10.0), (10.0, 4.0), (21.0, 4.0)], fg);
                self.line(&[(9.0, 21.0), (9.0, 12.0), (12.0, 9.0), (21.0, 9.0)], ac);
            }
            Icon::Mirror => {
                self.dashed(&[(12.0, 2.0), (12.0, 22.0)], fg);
                self.closed(&[(10.0, 6.0), (10.0, 18.0), (3.0, 18.0)], fg);
                self.face(&[(14.0, 6.0), (21.0, 18.0), (14.0, 18.0)], ac);
            }
            Icon::Construction => {
                self.dashed(&[(4.0, 20.0), (20.0, 4.0)], ac);
                self.disc((4.0, 20.0), 1.8, fg);
                self.disc((20.0, 4.0), 1.8, fg);
            }
            Icon::Relations => {
                self.line(&[(4.0, 20.0), (20.0, 20.0)], fg);
                self.line(&[(12.0, 20.0), (12.0, 5.0)], ac);
                self.closed(&[(12.0, 16.0), (16.0, 16.0), (16.0, 20.0)], fg);
            }
            Icon::NewDocument => {
                self.document();
                self.line(&[(12.0, 11.0), (12.0, 19.0)], ac);
                self.line(&[(8.0, 15.0), (16.0, 15.0)], ac);
            }
            Icon::Open => {
                self.closed(
                    &[
                        (2.0, 5.0),
                        (9.0, 5.0),
                        (11.0, 7.0),
                        (20.0, 7.0),
                        (20.0, 19.0),
                        (2.0, 19.0),
                    ],
                    fg,
                );
                self.face(&[(2.0, 19.0), (5.5, 11.0), (23.0, 11.0), (20.0, 19.0)], ac);
            }
            Icon::SaveAs => {
                self.closed(&[(3.0, 3.0), (15.0, 3.0), (18.0, 6.0), (18.0, 12.0)], fg);
                self.line(&[(3.0, 3.0), (3.0, 18.0), (11.0, 18.0)], fg);
                self.line(&[(7.0, 3.0), (7.0, 7.0), (13.0, 7.0), (13.0, 3.0)], fg);
                self.closed(
                    &[
                        (13.0, 22.0),
                        (14.0, 19.0),
                        (20.0, 13.0),
                        (22.0, 15.0),
                        (16.0, 21.0),
                    ],
                    ac,
                );
            }
            Icon::Sample => {
                self.closed(
                    &[
                        (3.0, 7.5),
                        (12.0, 3.0),
                        (21.0, 7.5),
                        (21.0, 17.5),
                        (12.0, 22.0),
                        (3.0, 17.5),
                    ],
                    fg,
                );
                self.line(&[(3.0, 7.5), (12.0, 12.0), (21.0, 7.5)], fg);
                self.line(&[(12.0, 12.0), (12.0, 22.0)], fg);
                self.line(&[(7.5, 5.25), (16.5, 9.75), (16.5, 13.5)], ac);
            }
            Icon::Settings => {
                for i in 0..8 {
                    let a = i as f32 * PI / 4.0;
                    let (c, s) = (a.cos(), a.sin());
                    self.line(
                        &[
                            (12.0 + 6.5 * c, 12.0 + 6.5 * s),
                            (12.0 + 9.5 * c, 12.0 + 9.5 * s),
                        ],
                        fg,
                    );
                }
                self.circle((12.0, 12.0), 6.5, fg);
                self.circle((12.0, 12.0), 2.8, ac);
            }
            Icon::Quit => {
                self.arc((12.0, 13.0), 8.0, -PI / 3.0, 4.0 * PI / 3.0, fg);
                self.line(&[(12.0, 2.5), (12.0, 12.0)], ac);
            }
            Icon::Delete => {
                self.line(&[(3.0, 6.0), (21.0, 6.0)], fg);
                self.line(&[(9.0, 6.0), (9.0, 3.0), (15.0, 3.0), (15.0, 6.0)], fg);
                self.closed(&[(5.5, 6.0), (18.5, 6.0), (17.0, 21.0), (7.0, 21.0)], fg);
                self.line(&[(10.0, 10.0), (10.0, 17.0)], ac);
                self.line(&[(14.0, 10.0), (14.0, 17.0)], ac);
            }
            Icon::Suppress => {
                self.circle((12.0, 12.0), 8.5, fg);
                self.line(&[(6.0, 18.0), (18.0, 6.0)], ac);
            }
            Icon::RollToEnd => {
                self.line(&[(4.0, 4.0), (20.0, 4.0)], fg);
                self.line(&[(4.0, 8.5), (20.0, 8.5)], fg);
                self.arrow((12.0, 9.5), (12.0, 18.5), ac);
                self.line(&[(3.0, 21.0), (21.0, 21.0)], ac);
            }
            Icon::ViewCube => {
                self.closed(&[(2.0, 3.0), (22.0, 3.0), (22.0, 21.0), (2.0, 21.0)], fg);
                self.face(&[(15.0, 5.0), (19.0, 7.0), (15.0, 9.0), (11.0, 7.0)], ac);
                self.closed(
                    &[
                        (15.0, 5.0),
                        (19.0, 7.0),
                        (19.0, 11.5),
                        (15.0, 13.5),
                        (11.0, 11.5),
                        (11.0, 7.0),
                    ],
                    ac,
                );
                self.line(&[(15.0, 9.0), (15.0, 13.5)], ac);
            }
            Icon::TreePanel | Icon::PropertiesPanel => {
                let (l, r) = if icon == Icon::TreePanel {
                    (2.0, 9.0)
                } else {
                    (15.0, 22.0)
                };
                self.face(&[(l, 4.0), (r, 4.0), (r, 20.0), (l, 20.0)], ac);
                self.closed(&[(2.0, 4.0), (22.0, 4.0), (22.0, 20.0), (2.0, 20.0)], fg);
            }
            Icon::Performance => {
                self.arc((12.0, 17.0), 9.0, PI, 2.0 * PI, fg);
                self.line(&[(3.0, 17.0), (21.0, 17.0)], fg);
                self.line(&[(12.0, 17.0), (17.0, 10.0)], ac);
                self.disc((12.0, 17.0), 1.8, ac);
            }
            Icon::Keyboard => {
                self.closed(&[(2.0, 6.0), (22.0, 6.0), (22.0, 18.0), (2.0, 18.0)], fg);
                for x in [6.0, 10.0, 14.0, 18.0] {
                    self.disc((x, 10.0), 1.1, fg);
                }
                self.line(&[(7.0, 14.5), (17.0, 14.5)], ac);
            }
            Icon::About => {
                self.circle((12.0, 12.0), 9.0, fg);
                self.disc((12.0, 7.5), 1.4, ac);
                self.line(&[(12.0, 11.0), (12.0, 17.5)], ac);
            }
        }
    }
}

/// A rounded tile tinted with the icon's category colour, with the icon on it: used in the
/// feature tree so feature types stand out.
pub fn tile(painter: &Painter, rect: Rect, icon: Icon, dark: bool, fg: Color32, faded: bool) {
    let mut accent = icon.category().color(dark);
    let mut fg = fg;
    if faded {
        accent = accent.gamma_multiply(0.45);
        fg = fg.gamma_multiply(0.45);
    }
    painter.rect_filled(
        rect,
        4.0,
        accent.gamma_multiply(if dark { 0.22 } else { 0.16 }),
    );
    let inner = rect.shrink(rect.width() * 0.12);
    icon.paint(painter, inner, fg, accent);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solid_modelling_commands_have_icons() {
        use CommandId as C;
        let commands = [
            C::Revolve,
            C::CutRevolve,
            C::Fillet,
            C::Chamfer,
            C::Shell,
            C::Draft,
            C::Hole,
            C::ImportStep,
            C::MassProperties,
            C::Sweep,
            C::CutSweep,
            C::Loft,
            C::CutLoft,
            C::OpenSampleHousing,
        ];
        let mut seen = std::collections::HashSet::new();
        for cmd in commands {
            let icon = Icon::for_command(cmd).unwrap_or_else(|| panic!("{cmd:?} has no icon"));
            if cmd != C::OpenSampleHousing {
                assert!(seen.insert(format!("{icon:?}")), "{cmd:?} shares its icon");
            }
        }
        // The edge fillet is not the sketch fillet.
        assert_ne!(
            Icon::for_command(C::Fillet),
            Icon::for_command(C::SketchFillet)
        );
    }

    #[test]
    fn every_ribbon_command_has_an_icon() {
        // Relations live in a menu and the sketch tools' Select has its own: every other
        // command is a ribbon button, which needs a picture.
        for cmd in CommandId::ALL {
            if cmd.info().category == "Relation" {
                continue;
            }
            assert!(Icon::for_command(cmd).is_some(), "{cmd:?} has no icon");
        }
    }

    #[test]
    fn every_feature_kind_has_a_tree_icon() {
        // The samples between them use nearly every kind of feature.
        let samples = [
            peet_model::samples::bracket().0,
            peet_model::samples::enclosure().0,
            peet_model::samples::chassis().0,
            peet_model::samples::housing().0,
        ];
        let mut icons = std::collections::HashSet::new();
        for model in &samples {
            for f in model.features() {
                icons.insert(format!("{:?}", Icon::for_feature(&f.kind)));
            }
        }
        for expected in ["Revolve", "FilletEdge", "Chamfer", "Hole"] {
            assert!(icons.contains(expected), "no sample shows {expected}");
        }
        // Cuts are told apart from additions.
        let sketch = peet_sketch::Sketch::new();
        let cut = FeatureKind::Revolve(Box::new(peet_model::RevolveFeature::new(
            peet_model::FeatureId(1),
            &sketch,
            Operation::Cut,
        )));
        assert_eq!(Icon::for_feature(&cut), Icon::CutRevolve);
        assert_eq!(Icon::CutRevolve.category(), Category::Cut);
        let import = FeatureKind::Import(Box::new(peet_model::ImportFeature {
            source: String::new(),
            solids: Vec::new(),
        }));
        assert_eq!(Icon::for_feature(&import), Icon::ImportStep);
    }

    /// Drawing every icon exercises every arm of `draw` (no GPU needed).
    #[test]
    fn every_icon_paints() {
        let mut harness = egui_kittest::Harness::new_ui(|ui| {
            let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(24.0, 24.0));
            for cmd in CommandId::ALL {
                if let Some(icon) = Icon::for_command(cmd) {
                    let accent = icon.category().color(true);
                    icon.paint(ui.painter(), rect, Color32::WHITE, accent);
                }
            }
        });
        harness.step();
    }
}
