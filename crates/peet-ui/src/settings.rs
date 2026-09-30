//! User preferences, persisted between sessions (a file natively, localStorage on the web).

use egui::PointerButton;
use peet_render::OrbitStyle;
use serde::{Deserialize, Serialize};

pub const STORAGE_KEY: &str = "peetcad.settings";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeChoice {
    System,
    Dark,
    Light,
}

impl ThemeChoice {
    pub const ALL: [Self; 3] = [Self::System, Self::Dark, Self::Light];

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "Follow system",
            Self::Dark => "Dark",
            Self::Light => "Light",
        }
    }

    pub fn preference(self) -> egui::ThemePreference {
        match self {
            Self::System => egui::ThemePreference::System,
            Self::Dark => egui::ThemePreference::Dark,
            Self::Light => egui::ThemePreference::Light,
        }
    }
}

/// What a mouse drag in the viewport does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavAction {
    Orbit,
    Pan,
    Zoom,
}

/// Modifier keys that must be held for a mouse binding (exactly these, no others).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavModifier {
    None,
    Ctrl,
    Shift,
    Alt,
}

impl NavModifier {
    fn matches(self, m: egui::Modifiers) -> bool {
        let ctrl = m.ctrl || m.command;
        match self {
            Self::None => !ctrl && !m.shift && !m.alt,
            Self::Ctrl => ctrl && !m.shift && !m.alt,
            Self::Shift => m.shift && !ctrl && !m.alt,
            Self::Alt => m.alt && !ctrl && !m.shift,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Ctrl => "Ctrl + ",
            Self::Shift => "Shift + ",
            Self::Alt => "Alt + ",
        }
    }
}

/// Mouse navigation presets, so people coming from other CAD tools feel at home.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MousePreset {
    SolidWorks,
    Blender,
    Onshape,
}

type Binding = (PointerButton, NavModifier, NavAction);

impl MousePreset {
    pub const ALL: [Self; 3] = [Self::SolidWorks, Self::Blender, Self::Onshape];

    pub fn label(self) -> &'static str {
        match self {
            Self::SolidWorks => "SolidWorks / FreeCAD",
            Self::Blender => "Blender",
            Self::Onshape => "Onshape",
        }
    }

    /// The drag bindings of this preset. Every preset also supports Alt + left drag to
    /// orbit and Alt + Shift + left drag to pan, for laptops without a middle button.
    pub fn bindings(self) -> &'static [Binding] {
        use NavAction::*;
        use NavModifier as M;
        use PointerButton::*;
        match self {
            Self::SolidWorks => &[
                (Middle, M::None, Orbit),
                (Middle, M::Ctrl, Pan),
                (Middle, M::Shift, Zoom),
                (Primary, M::Alt, Orbit),
            ],
            Self::Blender => &[
                (Middle, M::None, Orbit),
                (Middle, M::Shift, Pan),
                (Middle, M::Ctrl, Zoom),
                (Primary, M::Alt, Orbit),
            ],
            Self::Onshape => &[
                (Secondary, M::None, Orbit),
                (Middle, M::None, Pan),
                (Secondary, M::Ctrl, Pan),
                (Primary, M::Alt, Orbit),
            ],
        }
    }

    /// Which navigation action (if any) a drag with this button and these modifiers performs.
    pub fn action_for(
        self,
        button: PointerButton,
        modifiers: egui::Modifiers,
    ) -> Option<NavAction> {
        if button == PointerButton::Primary && modifiers.alt && modifiers.shift {
            return Some(NavAction::Pan);
        }
        self.bindings()
            .iter()
            .find(|(b, m, _)| *b == button && m.matches(modifiers))
            .map(|(_, _, a)| *a)
    }

    /// Human readable description of the bindings, for help text.
    pub fn describe(self) -> Vec<(String, &'static str)> {
        let mut out: Vec<(String, &'static str)> = self
            .bindings()
            .iter()
            .map(|(b, m, a)| {
                let button = match b {
                    PointerButton::Primary => "left",
                    PointerButton::Secondary => "right",
                    PointerButton::Middle => "middle",
                    _ => "extra",
                };
                let action = match a {
                    NavAction::Orbit => "Rotate",
                    NavAction::Pan => "Pan",
                    NavAction::Zoom => "Zoom",
                };
                (format!("{}{button} drag", m.label()), action)
            })
            .collect();
        out.push(("Alt + Shift + left drag".to_owned(), "Pan"));
        out.push(("Mouse wheel / pinch".to_owned(), "Zoom at cursor"));
        out
    }

    /// One-line hint for the status bar.
    pub fn status_hint(self) -> &'static str {
        match self {
            Self::SolidWorks => "Middle drag: rotate · Ctrl+middle: pan · Wheel: zoom",
            Self::Blender => "Middle drag: rotate · Shift+middle: pan · Wheel: zoom",
            Self::Onshape => "Right drag: rotate · Middle drag: pan · Wheel: zoom",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrbitChoice {
    Turntable,
    Trackball,
}

impl OrbitChoice {
    pub fn style(self) -> OrbitStyle {
        match self {
            Self::Turntable => OrbitStyle::Turntable,
            Self::Trackball => OrbitStyle::Trackball,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeChoice,
    pub mouse_preset: MousePreset,
    pub orbit: OrbitChoice,
    /// Reverse the mouse wheel zoom direction.
    pub invert_zoom: bool,
    /// Animate transitions to standard views and zoom to fit.
    pub animate_views: bool,
    pub perspective: bool,
    pub show_grid: bool,
    pub show_view_cube: bool,
    pub show_perf_overlay: bool,
    pub show_feature_tree: bool,
    pub show_properties: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::System,
            mouse_preset: MousePreset::SolidWorks,
            orbit: OrbitChoice::Turntable,
            invert_zoom: false,
            animate_views: true,
            perspective: false,
            show_grid: true,
            show_view_cube: true,
            show_perf_overlay: false,
            show_feature_tree: true,
            show_properties: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Modifiers;

    #[test]
    fn solidworks_bindings() {
        let p = MousePreset::SolidWorks;
        assert_eq!(
            p.action_for(PointerButton::Middle, Modifiers::NONE),
            Some(NavAction::Orbit)
        );
        assert_eq!(
            p.action_for(PointerButton::Middle, Modifiers::CTRL),
            Some(NavAction::Pan)
        );
        assert_eq!(
            p.action_for(PointerButton::Middle, Modifiers::SHIFT),
            Some(NavAction::Zoom)
        );
        assert_eq!(p.action_for(PointerButton::Primary, Modifiers::NONE), None);
        assert_eq!(
            p.action_for(PointerButton::Primary, Modifiers::ALT),
            Some(NavAction::Orbit)
        );
        let alt_shift = Modifiers {
            alt: true,
            shift: true,
            ..Modifiers::NONE
        };
        assert_eq!(
            p.action_for(PointerButton::Primary, alt_shift),
            Some(NavAction::Pan)
        );
    }

    #[test]
    fn left_click_is_never_navigation_without_alt() {
        for preset in MousePreset::ALL {
            assert_eq!(
                preset.action_for(PointerButton::Primary, Modifiers::NONE),
                None
            );
        }
    }
}
