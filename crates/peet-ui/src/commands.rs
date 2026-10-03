//! The command system.
//!
//! Every user action is a named command with a label, a category, an optional keyboard
//! shortcut and a description. Menus, toolbars, the command palette and keyboard
//! shortcuts all go through this one registry, so they can never disagree. Commands
//! that change the part go through `Document::change`, which makes them undoable.

use egui::{Key, KeyboardShortcut, Modifiers};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CommandId {
    Undo,
    Redo,
    CommandPalette,
    ViewIsometric,
    ViewFront,
    ViewBack,
    ViewLeft,
    ViewRight,
    ViewTop,
    ViewBottom,
    ZoomToFit,
    ToggleProjection,
    ToggleGrid,
    ToggleReferencePlanes,
    ToggleViewCube,
    ToggleFeatureTree,
    ToggleProperties,
    TogglePerfOverlay,
    NewDocument,
    OpenDocument,
    SaveDocumentAs,
    SaveDocument,
    OpenSample,
    OpenSampleEnclosure,
    OpenSampleChassis,
    RefPlane,
    RefAxis,
    RefPoint,
    RefCoordSystem,
    ToggleSuppress,
    RollToEnd,
    Settings,
    KeyboardShortcuts,
    About,
    Quit,
    NewSketch,
    EditSketch,
    ExitSketch,
    Parameters,
    DeleteSelection,
    SketchSelect,
    SketchLine,
    SketchRectangle,
    SketchCenterRectangle,
    SketchCircle,
    SketchArc,
    SketchSlot,
    SketchPolygon,
    SketchPoint,
    SketchTrim,
    SketchExtend,
    SketchFillet,
    SketchOffset,
    SketchMirror,
    SmartDimension,
    ToggleConstruction,
    ToggleRelations,
    RelCoincident,
    RelHorizontal,
    RelVertical,
    RelParallel,
    RelPerpendicular,
    RelTangent,
    RelEqual,
    RelConcentric,
    RelMidpoint,
    RelSymmetric,
    RelFix,
    Extrude,
    CutExtrude,
    ExportStl,
    BaseFlange,
    EdgeFlange,
    SheetCut,
    FlatPattern,
    BendTable,
    ExportDxf,
    Hem,
    SketchedBend,
    Jog,
    MiterFlange,
    CornerTreatment,
    Dimple,
    Emboss,
    Louver,
    LinearPattern,
    CircularPattern,
    MirrorFeature,
    SheetChecks,
    GaugeTables,
    ExportStep,
    ImportDxf,
}

/// Static description of a command.
#[derive(Clone, Copy, Debug)]
pub struct CommandInfo {
    pub label: &'static str,
    pub category: &'static str,
    pub description: &'static str,
    pub shortcut: Option<KeyboardShortcut>,
}

/// Dynamic state of a command at this moment.
#[derive(Clone, Copy, Debug, Default)]
pub struct CommandState {
    pub enabled: bool,
    /// `Some` for toggle commands: whether the toggle is currently on.
    pub checked: Option<bool>,
}

const fn key(key: Key) -> Option<KeyboardShortcut> {
    Some(KeyboardShortcut::new(Modifiers::NONE, key))
}

const fn ctrl(key: Key) -> Option<KeyboardShortcut> {
    Some(KeyboardShortcut::new(Modifiers::COMMAND, key))
}

impl CommandId {
    pub const ALL: [Self; 92] = [
        Self::Undo,
        Self::Redo,
        Self::CommandPalette,
        Self::ViewIsometric,
        Self::ViewFront,
        Self::ViewBack,
        Self::ViewLeft,
        Self::ViewRight,
        Self::ViewTop,
        Self::ViewBottom,
        Self::ZoomToFit,
        Self::ToggleProjection,
        Self::ToggleGrid,
        Self::ToggleReferencePlanes,
        Self::ToggleViewCube,
        Self::ToggleFeatureTree,
        Self::ToggleProperties,
        Self::TogglePerfOverlay,
        Self::NewDocument,
        Self::OpenDocument,
        Self::SaveDocumentAs,
        Self::SaveDocument,
        Self::OpenSample,
        Self::OpenSampleEnclosure,
        Self::OpenSampleChassis,
        Self::RefPlane,
        Self::RefAxis,
        Self::RefPoint,
        Self::RefCoordSystem,
        Self::ToggleSuppress,
        Self::RollToEnd,
        Self::Settings,
        Self::KeyboardShortcuts,
        Self::About,
        Self::Quit,
        Self::NewSketch,
        Self::EditSketch,
        Self::ExitSketch,
        Self::Parameters,
        Self::DeleteSelection,
        Self::SketchSelect,
        Self::SketchLine,
        Self::SketchRectangle,
        Self::SketchCenterRectangle,
        Self::SketchCircle,
        Self::SketchArc,
        Self::SketchSlot,
        Self::SketchPolygon,
        Self::SketchPoint,
        Self::SketchTrim,
        Self::SketchExtend,
        Self::SketchFillet,
        Self::SketchOffset,
        Self::SketchMirror,
        Self::SmartDimension,
        Self::ToggleConstruction,
        Self::ToggleRelations,
        Self::RelCoincident,
        Self::RelHorizontal,
        Self::RelVertical,
        Self::RelParallel,
        Self::RelPerpendicular,
        Self::RelTangent,
        Self::RelEqual,
        Self::RelConcentric,
        Self::RelMidpoint,
        Self::RelSymmetric,
        Self::RelFix,
        Self::Extrude,
        Self::CutExtrude,
        Self::ExportStl,
        Self::BaseFlange,
        Self::EdgeFlange,
        Self::SheetCut,
        Self::FlatPattern,
        Self::BendTable,
        Self::ExportDxf,
        Self::Hem,
        Self::SketchedBend,
        Self::Jog,
        Self::MiterFlange,
        Self::CornerTreatment,
        Self::Dimple,
        Self::Emboss,
        Self::Louver,
        Self::LinearPattern,
        Self::CircularPattern,
        Self::MirrorFeature,
        Self::SheetChecks,
        Self::GaugeTables,
        Self::ExportStep,
        Self::ImportDxf,
    ];

    pub fn info(self) -> CommandInfo {
        let (label, category, description, shortcut) = match self {
            Self::Undo => ("Undo", "Edit", "Undo the last change.", ctrl(Key::Z)),
            Self::Redo => ("Redo", "Edit", "Redo the last undone change.", ctrl(Key::Y)),
            Self::CommandPalette => (
                "Command Palette…",
                "Help",
                "Search for any command by name.",
                ctrl(Key::K),
            ),
            Self::ViewIsometric => (
                "Isometric",
                "View",
                "Isometric view from front-right-top.",
                key(Key::Num0),
            ),
            Self::ViewFront => (
                "Front",
                "View",
                "Look at the front (XZ) plane.",
                key(Key::Num1),
            ),
            Self::ViewTop => (
                "Top",
                "View",
                "Look down at the top (XY) plane.",
                key(Key::Num2),
            ),
            Self::ViewRight => (
                "Right",
                "View",
                "Look at the right (YZ) plane.",
                key(Key::Num3),
            ),
            Self::ViewBack => (
                "Back",
                "View",
                "Look at the model from behind.",
                key(Key::Num4),
            ),
            Self::ViewBottom => (
                "Bottom",
                "View",
                "Look up at the model from below.",
                key(Key::Num5),
            ),
            Self::ViewLeft => (
                "Left",
                "View",
                "Look at the model from the left.",
                key(Key::Num6),
            ),
            Self::ZoomToFit => (
                "Zoom to Fit",
                "View",
                "Zoom so everything visible fits the view.",
                key(Key::F),
            ),
            Self::ToggleProjection => (
                "Perspective",
                "View",
                "Switch between perspective and orthographic projection.",
                key(Key::P),
            ),
            Self::ToggleGrid => ("Grid", "View", "Show or hide the ground grid.", key(Key::G)),
            Self::ToggleReferencePlanes => (
                "Reference Planes",
                "View",
                "Show or hide the Front, Top and Right planes.",
                None,
            ),
            Self::ToggleViewCube => ("View Cube", "View", "Show or hide the view cube.", None),
            Self::ToggleFeatureTree => (
                "Feature Tree",
                "Window",
                "Show or hide the feature tree panel.",
                None,
            ),
            Self::ToggleProperties => (
                "Properties",
                "Window",
                "Show or hide the properties panel.",
                None,
            ),
            Self::TogglePerfOverlay => (
                "Performance Overlay",
                "Window",
                "Show frame timings and renderer statistics.",
                key(Key::F3),
            ),
            Self::NewDocument => ("New", "File", "Start a new, empty part.", ctrl(Key::N)),
            Self::OpenDocument => (
                "Open…",
                "File",
                "Open a PeetCAD part (.peet).",
                ctrl(Key::O),
            ),
            Self::SaveDocument => ("Save", "File", "Save the part.", ctrl(Key::S)),
            Self::SaveDocumentAs => (
                "Save As…",
                "File",
                "Save the part under a new name.",
                Some(KeyboardShortcut::new(
                    Modifiers::COMMAND.plus(Modifiers::SHIFT),
                    Key::S,
                )),
            ),
            Self::OpenSample => (
                "Open Sample Bracket",
                "File",
                "Open a 20-feature sample part to explore.",
                None,
            ),
            Self::OpenSampleEnclosure => (
                "Open Sample Enclosure Panel",
                "File",
                "Open a sheet metal enclosure panel with four flanges, reliefs and cutouts.",
                None,
            ),
            Self::OpenSampleChassis => (
                "Open Sample Chassis",
                "File",
                "Open a sheet metal chassis tray: a mitre flange rim, a hem, patterned holes and louvers, and mirrored dimples.",
                None,
            ),
            Self::RefPlane => (
                "Plane",
                "Reference",
                "A reference plane: offset from the selected face or plane.",
                None,
            ),
            Self::RefAxis => (
                "Axis",
                "Reference",
                "A reference axis: along the selected edge, through a round face, or where two selected planes meet.",
                None,
            ),
            Self::RefPoint => (
                "Point",
                "Reference",
                "A reference point: at the selected vertex, or at coordinates.",
                None,
            ),
            Self::RefCoordSystem => (
                "Coordinate System",
                "Reference",
                "A coordinate system at the selected vertex (or the origin).",
                None,
            ),
            Self::ToggleSuppress => (
                "Suppress",
                "Edit",
                "Skip the selected feature when building the part, or bring it back.",
                None,
            ),
            Self::RollToEnd => (
                "Roll to End",
                "Edit",
                "Move the rollback bar to the end of the feature tree.",
                None,
            ),
            Self::Settings => (
                "Settings…",
                "File",
                "Theme, mouse navigation and viewport options.",
                ctrl(Key::Comma),
            ),
            Self::KeyboardShortcuts => (
                "Keyboard & Mouse…",
                "Help",
                "List all shortcuts and mouse controls.",
                key(Key::F1),
            ),
            Self::About => (
                "About PeetCAD",
                "Help",
                "Version and build information.",
                None,
            ),
            Self::Quit => ("Quit", "File", "Close PeetCAD.", ctrl(Key::Q)),
            Self::NewSketch => (
                "New Sketch",
                "Sketch",
                "Start a sketch on the selected plane.",
                key(Key::S),
            ),
            Self::EditSketch => (
                "Edit Sketch",
                "Sketch",
                "Open the selected sketch for editing.",
                None,
            ),
            Self::ExitSketch => (
                "Exit Sketch",
                "Sketch",
                "Finish editing the sketch.",
                ctrl(Key::Enter),
            ),
            Self::Parameters => (
                "Parameters…",
                "Tools",
                "Named values you can use in dimension expressions (width = 2 * height).",
                None,
            ),
            Self::DeleteSelection => (
                "Delete",
                "Edit",
                "Delete the selected items.",
                key(Key::Delete),
            ),
            Self::SketchSelect => (
                "Select",
                "Sketch",
                "Select, drag and edit sketch geometry (Esc).",
                None,
            ),
            Self::SketchLine => ("Line", "Sketch", "Draw lines and polylines.", key(Key::L)),
            Self::SketchRectangle => (
                "Rectangle",
                "Sketch",
                "Draw a rectangle from two corners.",
                key(Key::R),
            ),
            Self::SketchCenterRectangle => (
                "Center Rectangle",
                "Sketch",
                "Draw a rectangle from its centre and a corner.",
                None,
            ),
            Self::SketchCircle => (
                "Circle",
                "Sketch",
                "Draw a circle from its centre and radius.",
                key(Key::C),
            ),
            Self::SketchArc => (
                "Arc",
                "Sketch",
                "Draw an arc from its centre, start and end.",
                key(Key::A),
            ),
            Self::SketchSlot => (
                "Slot",
                "Sketch",
                "Draw a straight slot from its two end centres and width.",
                None,
            ),
            Self::SketchPolygon => (
                "Polygon",
                "Sketch",
                "Draw a regular polygon from its centre and a vertex.",
                None,
            ),
            Self::SketchPoint => ("Point", "Sketch", "Place a sketch point.", None),
            Self::SketchTrim => (
                "Trim",
                "Sketch",
                "Remove the part of a curve up to its nearest intersections.",
                key(Key::T),
            ),
            Self::SketchExtend => (
                "Extend",
                "Sketch",
                "Extend a line or arc to the next curve.",
                None,
            ),
            Self::SketchFillet => (
                "Fillet",
                "Sketch",
                "Round a corner between two lines.",
                None,
            ),
            Self::SketchOffset => (
                "Offset",
                "Sketch",
                "Offset the selected curves by the distance set in the properties panel.",
                key(Key::O),
            ),
            Self::SketchMirror => (
                "Mirror",
                "Sketch",
                "Mirror the selection about the last selected line.",
                key(Key::M),
            ),
            Self::SmartDimension => (
                "Smart Dimension",
                "Sketch",
                "Add a length, distance, radius, diameter or angle dimension.",
                key(Key::D),
            ),
            Self::ToggleConstruction => (
                "Construction Geometry",
                "Sketch",
                "Toggle construction geometry for the selection, or for new geometry.",
                key(Key::X),
            ),
            Self::ToggleRelations => (
                "Show Relations",
                "Sketch",
                "Show or hide relation glyphs in the sketch.",
                None,
            ),
            Self::RelCoincident => (
                "Coincident",
                "Relation",
                "Make two points coincide, or put a point on a curve.",
                None,
            ),
            Self::RelHorizontal => (
                "Horizontal",
                "Relation",
                "Make lines horizontal, or two points level.",
                key(Key::H),
            ),
            Self::RelVertical => (
                "Vertical",
                "Relation",
                "Make lines vertical, or two points vertically aligned.",
                key(Key::V),
            ),
            Self::RelParallel => ("Parallel", "Relation", "Make lines parallel.", None),
            Self::RelPerpendicular => (
                "Perpendicular",
                "Relation",
                "Make two lines perpendicular.",
                None,
            ),
            Self::RelTangent => (
                "Tangent",
                "Relation",
                "Make a line and an arc/circle, or two arcs/circles, tangent.",
                None,
            ),
            Self::RelEqual => (
                "Equal",
                "Relation",
                "Make lines equal length, or arcs/circles equal radius.",
                key(Key::E),
            ),
            Self::RelConcentric => (
                "Concentric",
                "Relation",
                "Give arcs/circles the same centre.",
                None,
            ),
            Self::RelMidpoint => (
                "Midpoint",
                "Relation",
                "Put a point at the middle of a line.",
                None,
            ),
            Self::RelSymmetric => (
                "Symmetric",
                "Relation",
                "Make two points symmetric about a line.",
                None,
            ),
            Self::RelFix => ("Fix", "Relation", "Fix geometry where it is now.", None),
            Self::Extrude => (
                "Extrude",
                "Features",
                "Extrude the selected (or open) sketch into a solid.",
                None,
            ),
            Self::CutExtrude => (
                "Cut-Extrude",
                "Features",
                "Cut the selected (or open) sketch's regions out of the bodies.",
                None,
            ),
            Self::ExportStl => (
                "Export STL…",
                "File",
                "Save the bodies as an STL mesh for 3D printing or other tools.",
                None,
            ),
            Self::BaseFlange => (
                "Base Flange",
                "Sheet Metal",
                "Start a sheet metal part from the selected (or open) sketch: a closed shape makes a plate, connected lines make a profile with a bend at each corner.",
                None,
            ),
            Self::EdgeFlange => (
                "Edge Flange",
                "Sheet Metal",
                "Add a flange with a bend on the selected edges of a sheet metal part (or pick an edge).",
                None,
            ),
            Self::SheetCut => (
                "Sheet Metal Cut",
                "Sheet Metal",
                "Cut the selected (or open) sketch square through the sheet. The sketch must be on a flat face of the sheet; the cut is made in the flat pattern, so it can cross bends.",
                None,
            ),
            Self::FlatPattern => (
                "Flat Pattern",
                "Sheet Metal",
                "Show sheet metal parts flat (unfolded) or folded.",
                key(Key::U),
            ),
            Self::BendTable => (
                "Bend Table…",
                "Sheet Metal",
                "The flat pattern report: flat size and every bend's direction, angle, radius, K-factor, allowance and deduction.",
                None,
            ),
            Self::ExportDxf => (
                "Export Flat Pattern DXF…",
                "File",
                "Save the flat pattern of the sheet metal part as a DXF for laser, plasma or punch cutting: outline, cutouts, bend lines and bend notes on separate layers.",
                None,
            ),
            Self::Hem => (
                "Hem",
                "Sheet Metal",
                "Fold the selected edges of a sheet metal part back over the sheet (or pick an edge): closed, open, teardrop or rolled.",
                None,
            ),
            Self::SketchedBend => (
                "Sketched Bend",
                "Sheet Metal",
                "Bend a flat face along the lines of the selected (or open) sketch, drawn across that face.",
                None,
            ),
            Self::Jog => (
                "Jog",
                "Sheet Metal",
                "Step a flat face along the line of the selected (or open) sketch: two bends that offset the far side.",
                None,
            ),
            Self::MiterFlange => (
                "Miter Flange",
                "Sheet Metal",
                "Run the profile in the selected (or open) sketch along the selected edges of a face. Draw the profile square to one of the edges, starting at it; where the edges meet, the flanges are mitred.",
                None,
            ),
            Self::CornerTreatment => (
                "Corner",
                "Sheet Metal",
                "Set how flanges meet at corners: butt, overlap or open, the gap between them and the relief. Applies to the corners at the selected faces, or to every corner of the part.",
                None,
            ),
            Self::Dimple => (
                "Dimple",
                "Sheet Metal",
                "Press a round dimple at each circle of the selected (or open) sketch, drawn on a flat face of the sheet.",
                None,
            ),
            Self::Emboss => (
                "Emboss",
                "Sheet Metal",
                "Press a raised plateau for each closed polygon of the selected (or open) sketch, drawn on a flat face of the sheet.",
                None,
            ),
            Self::Louver => (
                "Louver",
                "Sheet Metal",
                "Lance and form a louver for each closed polygon (usually a rectangle) of the selected (or open) sketch: one side is cut open.",
                None,
            ),
            Self::LinearPattern => (
                "Linear Pattern",
                "Features",
                "Copy the selected feature (an extrusion, a cut, a sheet metal cut or a form) in a row or a grid.",
                None,
            ),
            Self::CircularPattern => (
                "Circular Pattern",
                "Features",
                "Copy the selected feature (an extrusion, a cut, a sheet metal cut or a form) around an axis.",
                None,
            ),
            Self::MirrorFeature => (
                "Mirror",
                "Features",
                "Make a mirror image of the selected feature (an extrusion, a cut, a sheet metal cut or a form) across a plane.",
                None,
            ),
            Self::SheetChecks => (
                "Check for Manufacture…",
                "Sheet Metal",
                "Check the sheet metal part against shop rules: flanges too short to bend, holes too close to a bend or an edge, holes too small, parts that collide when folded.",
                None,
            ),
            Self::GaugeTables => (
                "Materials and Gauges…",
                "Sheet Metal",
                "Tables of sheet stock: thickness, bend radius and K-factor by material and gauge. Apply one to the part, edit the tables, or share them as a CSV file.",
                None,
            ),
            Self::ExportStep => (
                "Export STEP…",
                "File",
                "Save the bodies as a STEP file (exact geometry) for other CAD systems and for fabricators.",
                None,
            ),
            Self::ImportDxf => (
                "Import DXF…",
                "File",
                "Bring the lines, arcs and circles of a DXF drawing into the open sketch, or into a new sketch on the top plane (or the selected plane or face).",
                None,
            ),
        };
        CommandInfo {
            label,
            category,
            description,
            shortcut,
        }
    }

    /// Whether the command exists on this platform at all (hidden from menus if not).
    pub fn available(self) -> bool {
        match self {
            // Closing the tab is the browser's job.
            Self::Quit => !peet_platform::is_web(),
            _ => true,
        }
    }
}

/// Fuzzy-matches `query` against `text`. Returns a score (higher is better), or `None`
/// if the query's characters don't all appear in order.
///
/// Consecutive matches and matches at word starts score higher, so "ztf" finds
/// "Zoom to Fit" and "grid" ranks "Grid" above "Keyboard shortcuts … right …".
pub fn fuzzy_score(query: &str, text: &str) -> Option<i32> {
    let query: Vec<char> = query
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if query.is_empty() {
        return Some(0);
    }
    let text: Vec<char> = text.to_lowercase().chars().collect();
    let mut score = 0;
    let mut qi = 0;
    let mut prev_match: Option<usize> = None;
    for (ti, &c) in text.iter().enumerate() {
        if qi < query.len() && c == query[qi] {
            score += 1;
            let word_start = ti == 0 || !text[ti - 1].is_alphanumeric();
            if word_start {
                score += 8;
            }
            if prev_match == Some(ti.wrapping_sub(1)) {
                score += 5;
            }
            prev_match = Some(ti);
            qi += 1;
        }
    }
    if qi < query.len() {
        return None;
    }
    // An exact substring beats any scattered match, and more so at the start of a word.
    let query_str: String = query.iter().collect();
    let text_str: String = text.iter().collect();
    if let Some(byte_pos) = text_str.find(&query_str) {
        score += 40;
        let at_word_start = text_str[..byte_pos]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        if at_word_start {
            score += 20;
        }
    }
    // Prefer shorter texts when scores tie.
    Some(score * 16 - text.len() as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_matching() {
        assert!(fuzzy_score("ztf", "Zoom to Fit").is_some());
        assert!(fuzzy_score("xyz", "Zoom to Fit").is_none());
        assert_eq!(fuzzy_score("", "anything"), Some(0));
        let grid = fuzzy_score("grid", "View: Grid").unwrap();
        let scattered = fuzzy_score("grid", "View: Go right in depth").unwrap();
        assert!(grid > scattered);
        let top = fuzzy_score("top", "View: Top").unwrap();
        let other = fuzzy_score("top", "Window: Toggle perf overlay").unwrap_or(i32::MIN);
        assert!(top > other);
    }

    #[test]
    fn shortcuts_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for cmd in CommandId::ALL {
            if let Some(s) = cmd.info().shortcut {
                let key = format!("{:?}{:?}", s.logical_key, s.modifiers);
                assert!(seen.insert(key), "duplicate shortcut on {cmd:?}");
            }
        }
    }

    #[test]
    fn all_commands_listed() {
        let unique: std::collections::HashSet<_> = CommandId::ALL.iter().collect();
        assert_eq!(unique.len(), CommandId::ALL.len());
    }
}
