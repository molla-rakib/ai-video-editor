//! Keyboard shortcuts: every action that has keys, its default keys, and the keys the user
//! chose instead. A key combination is written the way it is shown: "Ctrl+Shift+Z", "Del", "←".

use std::collections::BTreeMap;

use iced::keyboard::key::Named;
use iced::keyboard::{Key, Modifiers};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Action {
    NewProject,
    Open,
    Save,
    SaveAs,
    Import,
    Export,
    Settings,
    Shortcuts,
    Quit,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    PasteEffects,
    Duplicate,
    Delete,
    RippleDelete,
    SelectAll,
    SelectForward,
    Group,
    Ungroup,
    BringFront,
    SendBack,
    PlayPause,
    GoStart,
    GoEnd,
    StepBack,
    StepForward,
    StepBack10,
    StepForward10,
    MutePreview,
    AddText,
    Split,
    RangeTool,
    RangeDone,
    AddZoom,
    AddPress,
    AddMarker,
    DefaultTransition,
    FreezeFrame,
    ToggleDisabled,
    CloseGaps,
    ToggleSnapping,
    ZoomIn,
    ZoomOut,
    ZoomFit,
    PreviewFit,
    Preview100,
    ToggleAssistant,
}

pub struct Info {
    pub action: Action,
    pub group: &'static str,
    pub label: &'static str,
    pub keys: &'static [&'static str],
}

const fn info(action: Action, group: &'static str, label: &'static str, keys: &'static [&'static str]) -> Info {
    Info { action, group, label, keys }
}

/// Every action with its default keys, in the order the shortcuts dialog lists them.
pub const ACTIONS: &[Info] = &[
    info(Action::NewProject, "File", "New project", &["Ctrl+N"]),
    info(Action::Open, "File", "Open project", &["Ctrl+O"]),
    info(Action::Save, "File", "Save", &["Ctrl+S"]),
    info(Action::SaveAs, "File", "Save as", &["Ctrl+Shift+S"]),
    info(Action::Import, "File", "Import media", &["Ctrl+I"]),
    info(Action::Export, "File", "Export video", &["Ctrl+E"]),
    info(Action::Settings, "File", "Settings", &["Ctrl+,"]),
    info(Action::Shortcuts, "File", "Keyboard shortcuts", &["Ctrl+/"]),
    info(Action::Quit, "File", "Quit", &["Ctrl+Q"]),
    info(Action::Undo, "Edit", "Undo", &["Ctrl+Z"]),
    info(Action::Redo, "Edit", "Redo", &["Ctrl+Shift+Z", "Ctrl+Y"]),
    info(Action::Cut, "Edit", "Cut", &["Ctrl+X"]),
    info(Action::Copy, "Edit", "Copy", &["Ctrl+C"]),
    info(Action::Paste, "Edit", "Paste at playhead", &["Ctrl+V"]),
    info(Action::PasteEffects, "Edit", "Paste effects", &["Ctrl+Shift+V"]),
    info(Action::Duplicate, "Edit", "Duplicate", &["Ctrl+D"]),
    info(Action::Delete, "Edit", "Delete", &["Del", "Backspace"]),
    info(Action::RippleDelete, "Edit", "Ripple delete (close the space)", &["Shift+Del", "Ctrl+Backspace"]),
    info(Action::SelectAll, "Edit", "Select all clips", &["Ctrl+A"]),
    info(Action::SelectForward, "Edit", "Select everything after the playhead", &["Shift+A"]),
    info(Action::Group, "Edit", "Group", &["Ctrl+G"]),
    info(Action::Ungroup, "Edit", "Ungroup", &["Ctrl+Shift+G"]),
    info(Action::BringFront, "Edit", "Bring to front", &["Ctrl+]"]),
    info(Action::SendBack, "Edit", "Send to back", &["Ctrl+["]),
    info(Action::PlayPause, "Playback", "Play / Pause", &["Space"]),
    info(Action::GoStart, "Playback", "Go to start", &["Home"]),
    info(Action::GoEnd, "Playback", "Go to end", &["End"]),
    info(Action::StepBack, "Playback", "Back one frame", &["←"]),
    info(Action::StepForward, "Playback", "Forward one frame", &["→"]),
    info(Action::StepBack10, "Playback", "Back 10 frames", &["Shift+←"]),
    info(Action::StepForward10, "Playback", "Forward 10 frames", &["Shift+→"]),
    info(Action::MutePreview, "Playback", "Mute preview", &["M"]),
    info(Action::Split, "Timeline & clips", "Split at playhead", &["S", "B"]),
    info(Action::AddText, "Timeline & clips", "Add text at playhead", &["T"]),
    info(Action::DefaultTransition, "Timeline & clips", "Add transition at the cut", &["Ctrl+T"]),
    info(Action::FreezeFrame, "Timeline & clips", "Freeze frame at playhead", &["Shift+F"]),
    info(Action::ToggleDisabled, "Timeline & clips", "Disable / enable clip", &["Shift+E"]),
    info(Action::CloseGaps, "Timeline & clips", "Close gaps", &["G"]),
    info(Action::RangeTool, "Timeline & clips", "Select a range", &["R"]),
    info(Action::RangeDone, "Timeline & clips", "Done with range", &["Enter"]),
    info(Action::AddZoom, "Timeline & clips", "Add zoom at playhead", &["Z"]),
    info(Action::AddPress, "Timeline & clips", "Add press pulse", &["P"]),
    info(Action::AddMarker, "Timeline & clips", "Add marker", &["Shift+M"]),
    info(Action::ToggleSnapping, "Timeline & clips", "Toggle snapping", &["N"]),
    info(Action::ZoomIn, "Timeline & clips", "Zoom timeline in", &["+", "="]),
    info(Action::ZoomOut, "Timeline & clips", "Zoom timeline out", &["-"]),
    info(Action::ZoomFit, "Timeline & clips", "Fit timeline to window", &["Shift+Z"]),
    info(Action::PreviewFit, "View", "Fit preview to window", &["Ctrl+0"]),
    info(Action::Preview100, "View", "Preview at 100%", &["Ctrl+1"]),
    info(Action::ToggleAssistant, "View", "Toggle assistant", &["Ctrl+K"]),
];

pub const GROUPS: [&str; 5] = ["File", "Edit", "Playback", "Timeline & clips", "View"];

/// The key combination of a key press, written as it is shown ("Ctrl+Shift+Z"), or `None`
/// for keys that can't be shortcuts (modifiers alone, Esc, Tab, dead keys).
pub fn chord(key: &Key, modifiers: Modifiers) -> Option<String> {
    let (name, shift_counts): (String, bool) = match key.as_ref() {
        Key::Character(c) => {
            let mut chars = c.chars();
            let ch = chars.next()?;
            if chars.next().is_some() || ch.is_control() || ch.is_whitespace() {
                return None;
            }
            // Shift is already part of a symbol ("+", "?"); it only counts for letters.
            if ch.is_alphabetic() { (ch.to_uppercase().collect(), true) } else { (ch.to_string(), false) }
        }
        Key::Named(n) => {
            let name = match n {
                Named::Space => "Space",
                Named::Delete => "Del",
                Named::Backspace => "Backspace",
                Named::Home => "Home",
                Named::End => "End",
                Named::PageUp => "PageUp",
                Named::PageDown => "PageDown",
                Named::ArrowLeft => "←",
                Named::ArrowRight => "→",
                Named::ArrowUp => "↑",
                Named::ArrowDown => "↓",
                Named::Enter => "Enter",
                Named::Insert => "Insert",
                Named::F1 => "F1",
                Named::F2 => "F2",
                Named::F3 => "F3",
                Named::F4 => "F4",
                Named::F5 => "F5",
                Named::F6 => "F6",
                Named::F7 => "F7",
                Named::F8 => "F8",
                Named::F9 => "F9",
                Named::F10 => "F10",
                Named::F11 => "F11",
                Named::F12 => "F12",
                _ => return None,
            };
            (name.to_string(), true)
        }
        _ => return None,
    };
    let mut s = String::new();
    if modifiers.command() {
        s.push_str("Ctrl+");
    }
    if modifiers.alt() {
        s.push_str("Alt+");
    }
    if modifiers.shift() && shift_counts {
        s.push_str("Shift+");
    }
    s.push_str(&name);
    Some(s)
}

/// The user's own keys, for the actions they changed (everything else uses its defaults).
/// Saved by action name; names this version doesn't know are skipped, so a settings file from a
/// newer version still loads.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "BTreeMap<String, Vec<String>>", into = "BTreeMap<String, Vec<String>>")]
pub struct Keymap(BTreeMap<Action, Vec<String>>);

impl From<BTreeMap<String, Vec<String>>> for Keymap {
    fn from(saved: BTreeMap<String, Vec<String>>) -> Keymap {
        let mut map = Keymap(saved.into_iter().filter_map(|(name, keys)| Some((ACTIONS.iter().find(|i| format!("{:?}", i.action) == name)?.action, keys))).collect());
        map.tidy();
        map
    }
}

impl From<Keymap> for BTreeMap<String, Vec<String>> {
    fn from(map: Keymap) -> Self {
        map.0.into_iter().map(|(action, keys)| (format!("{action:?}"), keys)).collect()
    }
}

impl Keymap {
    pub fn keys(&self, action: Action) -> Vec<String> {
        match self.0.get(&action) {
            Some(keys) => keys.clone(),
            None => ACTIONS.iter().find(|i| i.action == action).map(|i| i.keys.iter().map(|k| k.to_string()).collect()).unwrap_or_default(),
        }
    }

    /// The main key of an action, for tooltips and menus.
    pub fn key(&self, action: Action) -> Option<String> {
        self.keys(action).into_iter().next()
    }

    pub fn lookup(&self, chord: &str) -> Option<Action> {
        ACTIONS.iter().map(|i| i.action).find(|a| self.keys(*a).iter().any(|k| k == chord))
    }

    pub fn is_custom(&self, action: Action) -> bool {
        self.0.contains_key(&action)
    }

    pub fn any_custom(&self) -> bool {
        !self.0.is_empty()
    }

    /// Gives `action` this key instead of its current ones. Whatever had the key loses it.
    pub fn assign(&mut self, action: Action, chord: String) {
        for other in ACTIONS.iter().map(|i| i.action).filter(|a| *a != action) {
            let keys = self.keys(other);
            if keys.contains(&chord) {
                self.0.insert(other, keys.into_iter().filter(|k| *k != chord).collect());
            }
        }
        self.0.insert(action, vec![chord]);
        self.tidy();
    }

    /// Back to the default keys of `action` (taking them back from whatever was given them).
    pub fn reset(&mut self, action: Action) {
        self.0.remove(&action);
        let defaults = self.keys(action);
        for other in ACTIONS.iter().map(|i| i.action).filter(|a| *a != action) {
            let keys = self.keys(other);
            if keys.iter().any(|k| defaults.contains(k)) {
                self.0.insert(other, keys.into_iter().filter(|k| !defaults.contains(k)).collect());
            }
        }
        self.tidy();
    }

    pub fn reset_all(&mut self) {
        self.0.clear();
    }

    /// Entries that say the same as the defaults aren't custom.
    fn tidy(&mut self) {
        self.0.retain(|action, keys| ACTIONS.iter().find(|i| i.action == *action).is_some_and(|i| !i.keys.iter().copied().eq(keys.iter().map(String::as_str))));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(c: &str, m: Modifiers) -> Option<String> {
        chord(&Key::Character(c.into()), m)
    }

    #[test]
    fn key_presses_read_like_the_labels() {
        assert_eq!(press("z", Modifiers::CTRL | Modifiers::SHIFT).as_deref(), Some("Ctrl+Shift+Z"));
        assert_eq!(press("Z", Modifiers::CTRL | Modifiers::SHIFT).as_deref(), Some("Ctrl+Shift+Z"));
        assert_eq!(press("m", Modifiers::empty()).as_deref(), Some("M"));
        assert_eq!(press("+", Modifiers::SHIFT).as_deref(), Some("+"), "shift is part of the symbol");
        assert_eq!(press("]", Modifiers::CTRL).as_deref(), Some("Ctrl+]"));
        assert_eq!(chord(&Key::Named(Named::Delete), Modifiers::SHIFT).as_deref(), Some("Shift+Del"));
        assert_eq!(chord(&Key::Named(Named::ArrowLeft), Modifiers::empty()).as_deref(), Some("←"));
        assert_eq!(chord(&Key::Named(Named::Shift), Modifiers::SHIFT), None);
        assert_eq!(chord(&Key::Named(Named::Escape), Modifiers::empty()), None);
    }

    #[test]
    fn default_keys_are_unique_and_every_action_is_listed() {
        let mut seen = std::collections::BTreeMap::new();
        for i in ACTIONS {
            assert!(GROUPS.contains(&i.group), "{}", i.group);
            for k in i.keys {
                assert!(seen.insert(*k, i.label).is_none(), "{k} is used twice");
            }
        }
        let map = Keymap::default();
        assert_eq!(map.lookup("Ctrl+Shift+Z"), Some(Action::Redo));
        assert_eq!(map.lookup("Ctrl+Y"), Some(Action::Redo));
        assert_eq!(map.lookup("Del"), Some(Action::Delete));
        assert_eq!(map.lookup("Q"), None);
    }

    #[test]
    fn assigning_a_key_takes_it_from_its_old_action_and_reset_gives_it_back() {
        let mut map = Keymap::default();
        // S splits by default; give it to Save.
        map.assign(Action::Save, "S".into());
        assert_eq!(map.lookup("S"), Some(Action::Save));
        assert_eq!(map.keys(Action::Split), vec!["B".to_string()]);
        assert_eq!(map.lookup("Ctrl+S"), None, "the old key no longer saves");
        assert!(map.is_custom(Action::Save) && map.is_custom(Action::Split));
        // Resetting Split takes S back, leaving Save without a key.
        map.reset(Action::Split);
        assert_eq!(map.lookup("S"), Some(Action::Split));
        assert!(map.keys(Action::Save).is_empty());
        map.reset(Action::Save);
        assert_eq!(map, Keymap::default(), "back to the defaults, nothing custom left");
        // It survives being saved and loaded.
        map.assign(Action::PlayPause, "K".into());
        let json = serde_json::to_string(&map).unwrap();
        assert_eq!(serde_json::from_str::<Keymap>(&json).unwrap(), map);
        let newer = r#"{"PlayPause":["K"],"SomethingNew":["F9"]}"#;
        assert_eq!(serde_json::from_str::<Keymap>(newer).unwrap(), map, "unknown actions are skipped");
    }
}
