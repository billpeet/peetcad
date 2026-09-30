//! The command system.
//!
//! Every user action is a named command with a label, a category, an optional keyboard
//! shortcut and a description. Menus, toolbars, the command palette and keyboard
//! shortcuts all go through this one registry, so they can never disagree. Undo/redo
//! will hook in here once the document model exists (Phase 3).

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
    ToggleDemoPart,
    Settings,
    KeyboardShortcuts,
    About,
    Quit,
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
    pub const ALL: [Self; 23] = [
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
        Self::ToggleDemoPart,
        Self::Settings,
        Self::KeyboardShortcuts,
        Self::About,
        Self::Quit,
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
            Self::ToggleDemoPart => (
                "Demo Part",
                "View",
                "Show the placeholder sheet metal part used to test the viewport.",
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
