//! What an operation needs besides the part: the material tables and check limits (which
//! belong to the user's settings, not to a file), and a running application for the
//! operations that are about the application itself.

use peet_sheetmetal::{CheckRules, MaterialLibrary};
use serde_json::{Map, Value};

use crate::args::{Args, boolean, text};
use crate::value::FeatureSel;

/// Where operations run: the application, or nothing but a document.
pub trait Host {
    /// The material and gauge tables.
    fn materials(&mut self) -> &mut MaterialLibrary;

    /// The limits of the manufacturing checks.
    fn check_rules(&mut self) -> &mut CheckRules;

    /// Does something to the application itself: turns the view, opens a window. The data
    /// returned goes in the reply.
    fn app(&mut self, command: &AppCommand) -> Result<Map<String, Value>, String> {
        Err(format!(
            "'{}' is about the application's window, so it needs a running PeetCAD: there is none here.",
            command.word()
        ))
    }
}

/// A host with no application: the built-in material tables and check limits, kept for as
/// long as it lives.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Headless {
    pub materials: MaterialLibrary,
    pub check_rules: CheckRules,
}

impl Host for Headless {
    fn materials(&mut self) -> &mut MaterialLibrary {
        &mut self.materials
    }

    fn check_rules(&mut self) -> &mut CheckRules {
        &mut self.check_rules
    }
}

/// Declares an enum whose values are written as words in JSON.
macro_rules! word_enum {
    ($(#[$doc:meta])* $name:ident, $what:literal { $( $(#[$vdoc:meta])* $variant:ident = $word:literal ),* $(,)? }) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $name {
            $( $(#[$vdoc])* $variant, )*
        }

        impl $name {
            pub const ALL: &'static [Self] = &[ $( Self::$variant, )* ];

            /// The value as written in JSON.
            pub fn word(self) -> &'static str {
                match self {
                    $( Self::$variant => $word, )*
                }
            }

            pub(crate) fn words() -> String {
                let all: Vec<&str> = Self::ALL.iter().map(|v| v.word()).collect();
                all.join(", ")
            }

            pub(crate) fn parse(v: &Value) -> Result<Self, String> {
                let word = text(v)?;
                Self::ALL
                    .iter()
                    .copied()
                    .find(|x| x.word() == word)
                    .ok_or_else(|| format!("'{word}' is not {}: use {}", $what, Self::words()))
            }
        }
    };
}

pub(crate) use word_enum;

word_enum! {
    /// A standard view.
    View, "a view" {
        Isometric = "isometric",
        Front = "front",
        Back = "back",
        Left = "left",
        Right = "right",
        Top = "top",
        Bottom = "bottom",
    }
}

word_enum! {
    /// Something in the application that is on or off.
    Toggle, "something to toggle" {
        /// Perspective projection (off: orthographic).
        Perspective = "perspective",
        Grid = "grid",
        ViewCube = "view_cube",
        FeatureTree = "feature_tree",
        Properties = "properties",
        PerformanceOverlay = "performance_overlay",
        /// In a sketch: the relation markers.
        Relations = "relations",
        /// In a sketch: new geometry is construction geometry (or, with geometry
        /// selected, whether it is).
        Construction = "construction",
    }
}

word_enum! {
    /// A window or panel of the application to open.
    Window, "a window" {
        CommandPalette = "command_palette",
        Settings = "settings",
        KeyboardShortcuts = "keyboard_shortcuts",
        About = "about",
        Parameters = "parameters",
        BendTable = "bend_table",
        Checks = "checks",
        Materials = "materials",
        MassProperties = "mass",
    }
}

word_enum! {
    /// An interactive tool of the sketch editor.
    SketchTool, "a sketch tool" {
        Select = "select",
        Line = "line",
        Rectangle = "rectangle",
        CenterRectangle = "center_rectangle",
        Circle = "circle",
        Arc = "arc",
        Slot = "slot",
        Polygon = "polygon",
        Point = "point",
        Trim = "trim",
        Extend = "extend",
        Fillet = "fillet",
        Offset = "offset",
        Mirror = "mirror",
        Dimension = "dimension",
    }
}

/// An operation on the application itself, not on the part. It needs a running
/// application; headless, it is refused.
#[derive(Clone, Debug, PartialEq)]
pub enum AppCommand {
    /// Turn the view to a standard view.
    View(View),
    ZoomToFit,
    /// Turn something on or off (`None`: the other way).
    Toggle {
        what: Toggle,
        on: Option<bool>,
    },
    /// Open a window or a panel.
    Window(Window),
    /// Open a sketch for editing in the application.
    EditSketch(FeatureSel),
    /// Finish editing the open sketch, writing it to the part.
    ExitSketch,
    /// Pick a tool of the sketch editor.
    Tool(SketchTool),
    Quit,
}

impl AppCommand {
    /// The command's operation name, as in JSON.
    pub fn word(&self) -> &'static str {
        match self {
            Self::View(_) => "view",
            Self::ZoomToFit => "zoom_to_fit",
            Self::Toggle { .. } => "toggle",
            Self::Window(_) => "window",
            Self::EditSketch(_) => "edit_sketch",
            Self::ExitSketch => "exit_sketch",
            Self::Tool(_) => "tool",
            Self::Quit => "quit",
        }
    }

    /// Reads the application command called `op`, if `op` is one.
    pub(crate) fn parse(op: &str, a: &mut Args) -> Option<Result<Self, String>> {
        Some(match op {
            "view" => a.required("to", View::parse).map(Self::View),
            "zoom_to_fit" => Ok(Self::ZoomToFit),
            "toggle" => (|| {
                Ok(Self::Toggle {
                    what: a.required("what", Toggle::parse)?,
                    on: a.parsed("on", boolean)?,
                })
            })(),
            "window" => a.required("open", Window::parse).map(Self::Window),
            "edit_sketch" => a
                .required("sketch", FeatureSel::parse)
                .map(Self::EditSketch),
            "exit_sketch" => Ok(Self::ExitSketch),
            "tool" => a.required("tool", SketchTool::parse).map(Self::Tool),
            "quit" => Ok(Self::Quit),
            _ => return None,
        })
    }
}

/// The application commands, with their fields, for `help`.
pub(crate) fn app_ops() -> Vec<(&'static str, String, &'static str)> {
    vec![
        (
            "view",
            format!("to ({})", View::words()),
            "Turn the view to a standard view.",
        ),
        (
            "zoom_to_fit",
            String::new(),
            "Zoom so everything visible fits the view.",
        ),
        (
            "toggle",
            format!("what ({}), on (left out: the other way)", Toggle::words()),
            "Turn something in the application on or off.",
        ),
        (
            "window",
            format!("open ({})", Window::words()),
            "Open a window or a panel.",
        ),
        (
            "edit_sketch",
            "sketch".to_owned(),
            "Open a sketch for editing in the application.",
        ),
        (
            "exit_sketch",
            String::new(),
            "Finish editing the open sketch, writing it to the part.",
        ),
        (
            "tool",
            format!("tool ({})", SketchTool::words()),
            "Pick a tool of the sketch editor.",
        ),
        ("quit", String::new(), "Close the application."),
    ]
}
