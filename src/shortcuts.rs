use std::{collections::BTreeMap, fmt};

use gpui::{Global, Keystroke};
use serde::{Deserialize, Deserializer, Serialize};

use crate::formatting;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Switcher,
    WorkspaceSearch,
    Find,
    Save,
    NewNote,
    Split,
    Move,
    NextTab,
    PreviousTab,
    CloseTab,
    Rename,
    Back,
    Forward,
    NextMatch,
    PreviousMatch,
    Bold,
    Italic,
    Heading1,
    Heading2,
    Heading3,
    Heading4,
    Heading5,
    Heading6,
    InlineCode,
    FencedCode,
}

pub const DEFAULTS: [(Action, &str, &str); 25] = [
    (Action::Switcher, "Open note", "Ctrl+P"),
    (Action::WorkspaceSearch, "Search workspace", "Ctrl+Shift+F"),
    (Action::Find, "Find in note", "Ctrl+F"),
    (Action::Save, "Save note", "Ctrl+S"),
    (Action::NewNote, "New note", "Ctrl+N"),
    (Action::Split, "Toggle split", "Ctrl+Backslash"),
    (Action::Move, "Move note", "Ctrl+Shift+M"),
    (Action::NextTab, "Next tab", "Ctrl+Tab"),
    (Action::PreviousTab, "Previous tab", "Ctrl+Shift+Tab"),
    (Action::CloseTab, "Close tab", "Ctrl+W"),
    (Action::Rename, "Rename note", "F2"),
    (Action::Back, "History back", "Alt+Left"),
    (Action::Forward, "History forward", "Alt+Right"),
    (Action::NextMatch, "Next find match", "F3"),
    (Action::PreviousMatch, "Previous find match", "Shift+F3"),
    (Action::Bold, "Bold", "Ctrl+B"),
    (Action::Italic, "Italic", "Ctrl+I"),
    (Action::Heading1, "Heading 1", "Ctrl+1"),
    (Action::Heading2, "Heading 2", "Ctrl+2"),
    (Action::Heading3, "Heading 3", "Ctrl+3"),
    (Action::Heading4, "Heading 4", "Ctrl+4"),
    (Action::Heading5, "Heading 5", "Ctrl+5"),
    (Action::Heading6, "Heading 6", "Ctrl+6"),
    (Action::InlineCode, "Inline code", "Ctrl+E"),
    (Action::FencedCode, "Code block", "Ctrl+Shift+E"),
];

impl Action {
    pub fn label(self) -> &'static str {
        DEFAULTS.iter().find(|entry| entry.0 == self).unwrap().1
    }

    pub fn formatting(self) -> Option<formatting::Command> {
        use formatting::Command;
        Some(match self {
            Self::Bold => Command::Bold,
            Self::Italic => Command::Italic,
            Self::Heading1 => Command::Heading(1),
            Self::Heading2 => Command::Heading(2),
            Self::Heading3 => Command::Heading(3),
            Self::Heading4 => Command::Heading(4),
            Self::Heading5 => Command::Heading(5),
            Self::Heading6 => Command::Heading(6),
            Self::InlineCode => Command::InlineCode,
            Self::FencedCode => Command::FencedCode,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Chord {
    key: String,
    control: bool,
    alt: bool,
    shift: bool,
}

fn normalize(key: &str) -> String {
    match key.to_ascii_lowercase().as_str() {
        "\\" => "backslash".to_owned(),
        "esc" => "escape".to_owned(),
        "return" => "enter".to_owned(),
        key if key
            .strip_prefix('f')
            .and_then(|key| key.parse::<u8>().ok())
            .is_some() =>
        {
            format!("f{}", key[1..].parse::<u8>().unwrap())
        }
        key => key.to_owned(),
    }
}

impl TryFrom<String> for Chord {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let mut parts = value.split('+');
        let key = parts.next_back().ok_or("Choose a key.")?;
        let mut chord = Self {
            key: normalize(key.trim()),
            control: false,
            alt: false,
            shift: false,
        };
        for modifier in parts {
            let flag = match modifier.trim().to_ascii_lowercase().as_str() {
                "ctrl" | "control" => &mut chord.control,
                "alt" => &mut chord.alt,
                "shift" => &mut chord.shift,
                _ => return Err("Use Ctrl, Alt, or Shift modifiers.".to_owned()),
            };
            if *flag {
                return Err("Repeated modifier.".to_owned());
            }
            *flag = true;
        }
        chord.validate()?;
        Ok(chord)
    }
}

impl From<Chord> for String {
    fn from(chord: Chord) -> Self {
        chord.to_string()
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (enabled, label) in [
            (self.control, "Ctrl+"),
            (self.alt, "Alt+"),
            (self.shift, "Shift+"),
        ] {
            if enabled {
                f.write_str(label)?;
            }
        }
        let mut key = self.key.clone();
        key[..1].make_ascii_uppercase();
        f.write_str(&key)
    }
}

impl Chord {
    pub fn capture(stroke: &Keystroke) -> Result<Self, String> {
        let modifiers = stroke.modifiers;
        if modifiers.platform || modifiers.function {
            return Err("Windows/Super and Fn keys are reserved by the system.".to_owned());
        }
        let chord = Self {
            key: normalize(&stroke.key),
            control: modifiers.control,
            alt: modifiers.alt,
            shift: modifiers.shift,
        };
        chord.validate()?;
        Ok(chord)
    }

    fn validate(&self) -> Result<(), String> {
        let function = self
            .key
            .strip_prefix('f')
            .and_then(|key| key.parse::<u8>().ok());
        let letter = self.key.len() == 1 && self.key.as_bytes()[0].is_ascii_alphanumeric();
        let special = matches!(self.key.as_str(), "backslash" | "tab" | "left" | "right");
        if !letter && !special && !function.is_some_and(|number| (1..=12).contains(&number)) {
            return Err("Use a letter, digit, F1–F12, or a supported navigation key.".to_owned());
        }
        if self.control && self.alt {
            return Err("Ctrl+Alt is reserved for AltGr typing.".to_owned());
        }
        if function.is_some() {
            if self.key == "f4" && self.alt || self.key == "f10" && !self.control && !self.alt {
                return Err("This shortcut is reserved by the system.".to_owned());
            }
            return Ok(());
        }
        if !self.control && !self.alt {
            return Err("Include Ctrl or Alt so typing and navigation stay available.".to_owned());
        }
        if self.control && matches!(self.key.as_str(), "a" | "c" | "x" | "v" | "z" | "y") {
            return Err("This shortcut is reserved for editing or the clipboard.".to_owned());
        }
        if matches!(self.key.as_str(), "left" | "right") && (!self.alt || self.shift) {
            return Err("This shortcut is reserved for editor navigation.".to_owned());
        }
        if self.key == "tab" && !self.control {
            return Err("This shortcut is reserved for navigation.".to_owned());
        }
        Ok(())
    }

    pub fn matches(&self, stroke: &Keystroke) -> bool {
        let modifiers = stroke.modifiers;
        !modifiers.platform
            && !modifiers.function
            && self.control == modifiers.control
            && self.alt == modifiers.alt
            && self.shift == modifiers.shift
            && self.key == normalize(&stroke.key)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Bindings(BTreeMap<Action, Chord>);

impl Global for Bindings {}

impl Default for Bindings {
    fn default() -> Self {
        Self(
            DEFAULTS
                .iter()
                .map(|&(action, _, chord)| (action, Chord::try_from(chord.to_owned()).unwrap()))
                .collect(),
        )
    }
}

impl<'de> Deserialize<'de> for Bindings {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let Ok(custom) = serde_json::from_value::<BTreeMap<Action, Chord>>(value) else {
            return Ok(Self::default());
        };
        let mut bindings = Self::default();
        bindings.0.extend(custom);
        if bindings
            .0
            .values()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != bindings.0.len()
        {
            return Ok(Self::default());
        }
        Ok(bindings)
    }
}

impl Bindings {
    pub fn chord(&self, action: Action) -> &Chord {
        &self.0[&action]
    }

    pub fn action(&self, stroke: &Keystroke) -> Option<Action> {
        self.0
            .iter()
            .find_map(|(&action, chord)| chord.matches(stroke).then_some(action))
    }

    pub fn changed(&self, action: Action, chord: Chord) -> Result<Self, String> {
        chord.validate()?;
        if let Some((&other, _)) = self
            .0
            .iter()
            .find(|(other, bound)| **other != action && **bound == chord)
        {
            return Err(format!("{chord} is already used by {}.", other.label()));
        }
        let mut bindings = self.clone();
        bindings.0.insert(action, chord);
        Ok(bindings)
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, Bindings, Chord, DEFAULTS};
    use gpui::{Keystroke, Modifiers};

    fn chord(value: &str) -> Chord {
        Chord::try_from(value.to_owned()).unwrap()
    }

    fn stroke(key: &str, control: bool, alt: bool, shift: bool) -> Keystroke {
        Keystroke {
            key: key.to_owned(),
            modifiers: Modifiers {
                control,
                alt,
                shift,
                ..Modifiers::default()
            },
            key_char: None,
        }
    }

    #[test]
    fn defaults_are_unique_and_match_every_registered_action() {
        let bindings = Bindings::default();
        assert_eq!(bindings.0.len(), DEFAULTS.len());
        let mut seen = std::collections::BTreeSet::new();
        for &(action, _, value) in &DEFAULTS {
            let chord = chord(value);
            assert!(seen.insert(chord.clone()), "duplicate binding {value}");
            assert_eq!(
                bindings.action(&stroke(&chord.key, chord.control, chord.alt, chord.shift)),
                Some(action)
            );
        }
        assert_eq!(
            Action::Heading6.formatting(),
            Some(crate::formatting::Command::Heading(6))
        );
        assert_eq!(Action::Switcher.formatting(), None);
    }

    #[test]
    fn chords_normalize_aliases_and_require_exact_modifiers() {
        assert_eq!(chord("control+b"), chord("Ctrl+B"));
        assert_eq!(chord("Ctrl+\\"), chord("Ctrl+Backslash"));
        assert_eq!(chord("Shift+F01"), chord("Shift+F1"));
        assert_eq!(chord("Shift+F01").to_string(), "Shift+F1");
        let bold = chord("Ctrl+B");
        assert!(bold.matches(&stroke("B", true, false, false)));
        assert!(!bold.matches(&stroke("b", true, false, true)));
        assert!(!bold.matches(&stroke("b", true, true, false)));
        assert!(!bold.matches(&stroke("b", false, false, false)));
        let mut extra = stroke("b", true, false, false);
        extra.modifiers.platform = true;
        assert!(!bold.matches(&extra));
        extra.modifiers.platform = false;
        extra.modifiers.function = true;
        assert!(!bold.matches(&extra));
        assert_eq!(
            Chord::capture(&stroke("\\", true, false, false)).unwrap(),
            chord("Ctrl+Backslash")
        );
    }

    #[test]
    fn typing_altgr_editor_and_system_keys_are_reserved() {
        for value in [
            "B",
            "Shift+B",
            "1",
            "Tab",
            "Escape",
            "Enter",
            "Ctrl+Enter",
            "Ctrl+Left",
            "Alt+Shift+Left",
            "Ctrl+Home",
            "Ctrl+Backspace",
            "Ctrl+Z",
            "Ctrl+Shift+Z",
            "Ctrl+C",
            "Ctrl+Shift+V",
            "Ctrl+Alt+E",
            "Alt+Tab",
            "Alt+F4",
            "F10",
            "Shift+F10",
            "Ctrl+Ctrl+B",
            "Super+B",
            "F0",
            "F13",
            "",
        ] {
            assert!(
                Chord::try_from(value.to_owned()).is_err(),
                "accepted reserved {value}"
            );
        }
        assert!(Chord::capture(&stroke("e", true, true, false)).is_err());
        assert!(Chord::capture(&stroke("enter", true, false, false)).is_err());
        assert!(Chord::capture(&stroke("b", false, false, false)).is_err());
        for value in ["Ctrl+J", "Alt+J", "Shift+F8", "Alt+Left", "Ctrl+Shift+Tab"] {
            assert!(
                Chord::try_from(value.to_owned()).is_ok(),
                "rejected {value}"
            );
        }
    }

    #[test]
    fn conflicts_cover_app_and_formatting_scopes_and_aliases() {
        let bindings = Bindings::default();
        for (action, taken) in [
            (Action::Save, "Ctrl+N"),
            (Action::Save, "Ctrl+B"),
            (Action::Bold, "Ctrl+I"),
            (Action::Bold, "Ctrl+S"),
            (Action::Save, "F02"),
        ] {
            assert!(bindings.changed(action, chord(taken)).is_err());
        }
        assert_eq!(
            bindings.changed(Action::Bold, chord("Ctrl+B")).unwrap(),
            bindings
        );
        assert_eq!(bindings.chord(Action::Save), &chord("Ctrl+S"));
    }

    #[test]
    fn remapped_input_and_note_actions_stop_matching_old_keys() {
        let bindings = Bindings::default()
            .changed(Action::Switcher, chord("Ctrl+J"))
            .unwrap()
            .changed(Action::NextMatch, chord("Ctrl+K"))
            .unwrap()
            .changed(Action::PreviousMatch, chord("Ctrl+Shift+K"))
            .unwrap()
            .changed(Action::Bold, chord("F8"))
            .unwrap();
        for (key, ctrl, shift, expected) in [
            ("j", true, false, Some(Action::Switcher)),
            ("p", true, false, None),
            ("k", true, false, Some(Action::NextMatch)),
            ("k", true, true, Some(Action::PreviousMatch)),
            ("f3", false, false, None),
            ("f8", false, false, Some(Action::Bold)),
            ("b", true, false, None),
            ("b", false, false, None),
        ] {
            assert_eq!(bindings.action(&stroke(key, ctrl, false, shift)), expected);
        }
        assert_eq!(bindings.action(&stroke("j", true, true, false)), None);
    }
}
