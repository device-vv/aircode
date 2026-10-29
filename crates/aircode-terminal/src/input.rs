use crate::TerminalError;

/// Keyboard modifiers using terminal/xterm encoding semantics.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Modifiers {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

/// Keys understood by the prototype encoder.
///
/// Character keys should contain the already shifted character from the UI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Key {
    Char(char),
    Enter,
    Backspace,
    Tab,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    Insert,
    Delete,
    PageUp,
    PageDown,
    Escape,
    F(u8),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyEvent {
    pub key: Key,
    pub modifiers: Modifiers,
}

impl KeyEvent {
    pub const fn new(key: Key) -> Self {
        Self {
            key,
            modifiers: Modifiers {
                shift: false,
                alt: false,
                ctrl: false,
            },
        }
    }

    pub const fn with_modifiers(key: Key, modifiers: Modifiers) -> Self {
        Self { key, modifiers }
    }
}

/// Encode a key using xterm-style input sequences.
///
/// `application_cursor_keys` must reflect DECCKM from the terminal grid.
/// Returns `None` for unsupported Ctrl combinations and F-keys above F12.
pub fn encode_key(event: KeyEvent, application_cursor_keys: bool) -> Option<Vec<u8>> {
    let modifiers = event.modifiers;
    let mut encoded = match event.key {
        Key::Char(character) => encode_character(character, modifiers)?,
        Key::Enter => with_alt(vec![b'\r'], modifiers),
        Key::Backspace => with_alt(vec![0x7f], modifiers),
        Key::Tab if modifiers.shift => with_alt(b"\x1b[Z".to_vec(), modifiers),
        Key::Tab => with_alt(vec![b'\t'], modifiers),
        Key::Escape => with_alt(vec![0x1b], modifiers),
        Key::Up => encode_cursor(b'A', modifiers, application_cursor_keys),
        Key::Down => encode_cursor(b'B', modifiers, application_cursor_keys),
        Key::Right => encode_cursor(b'C', modifiers, application_cursor_keys),
        Key::Left => encode_cursor(b'D', modifiers, application_cursor_keys),
        Key::Home => encode_cursor(b'H', modifiers, application_cursor_keys),
        Key::End => encode_cursor(b'F', modifiers, application_cursor_keys),
        Key::Insert => encode_tilde_key(2, modifiers),
        Key::Delete => encode_tilde_key(3, modifiers),
        Key::PageUp => encode_tilde_key(5, modifiers),
        Key::PageDown => encode_tilde_key(6, modifiers),
        Key::F(number) => encode_function_key(number, modifiers)?,
    };

    // Character and Ctrl encoding handles Alt itself. Other paths use with_alt.
    if matches!(event.key, Key::Char(_)) {
        return Some(encoded);
    }

    // Ctrl does not add a separate prefix for non-character special keys; it is
    // represented by the xterm modifier parameter where applicable.
    if matches!(
        event.key,
        Key::Enter | Key::Backspace | Key::Tab | Key::Escape
    ) && modifiers.ctrl
    {
        // These keys have no portable Ctrl+key representation. Preserve the
        // base key instead of silently turning it into a different control byte.
    }

    Some(std::mem::take(&mut encoded))
}

fn encode_character(character: char, modifiers: Modifiers) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    if modifiers.alt {
        bytes.push(0x1b);
    }

    if modifiers.ctrl {
        let upper = character.to_ascii_uppercase();
        let control = match upper {
            '@'..='_' => (upper as u8) & 0x1f,
            '?' => 0x7f,
            ' ' => 0,
            _ => return None,
        };
        bytes.push(control);
    } else {
        let mut buffer = [0; 4];
        bytes.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
    }
    Some(bytes)
}

fn with_alt(mut bytes: Vec<u8>, modifiers: Modifiers) -> Vec<u8> {
    if modifiers.alt {
        bytes.insert(0, 0x1b);
    }
    bytes
}

fn modifier_parameter(modifiers: Modifiers) -> Option<u8> {
    let value = 1
        + u8::from(modifiers.shift)
        + if modifiers.alt { 2 } else { 0 }
        + if modifiers.ctrl { 4 } else { 0 };
    (value > 1).then_some(value)
}

fn encode_cursor(final_byte: u8, modifiers: Modifiers, application_cursor_keys: bool) -> Vec<u8> {
    if let Some(parameter) = modifier_parameter(modifiers) {
        return format!("\x1b[1;{parameter}{}", final_byte as char).into_bytes();
    }

    if application_cursor_keys {
        vec![0x1b, b'O', final_byte]
    } else {
        vec![0x1b, b'[', final_byte]
    }
}

fn encode_tilde_key(number: u8, modifiers: Modifiers) -> Vec<u8> {
    match modifier_parameter(modifiers) {
        Some(parameter) => format!("\x1b[{number};{parameter}~").into_bytes(),
        None => format!("\x1b[{number}~").into_bytes(),
    }
}

fn encode_function_key(number: u8, modifiers: Modifiers) -> Option<Vec<u8>> {
    let encoded = match number {
        1..=4 => {
            let final_byte = b'P' + number - 1;
            if let Some(parameter) = modifier_parameter(modifiers) {
                format!("\x1b[1;{parameter}{}", final_byte as char).into_bytes()
            } else {
                vec![0x1b, b'O', final_byte]
            }
        }
        5 => encode_tilde_key(15, modifiers),
        6 => encode_tilde_key(17, modifiers),
        7 => encode_tilde_key(18, modifiers),
        8 => encode_tilde_key(19, modifiers),
        9 => encode_tilde_key(20, modifiers),
        10 => encode_tilde_key(21, modifiers),
        11 => encode_tilde_key(23, modifiers),
        12 => encode_tilde_key(24, modifiers),
        _ => return None,
    };
    Some(encoded)
}

pub(crate) fn encode_or_error(
    event: KeyEvent,
    application_cursor_keys: bool,
) -> Result<Vec<u8>, TerminalError> {
    encode_key(event, application_cursor_keys).ok_or(TerminalError::UnsupportedKey)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CTRL: Modifiers = Modifiers {
        shift: false,
        alt: false,
        ctrl: true,
    };
    const ALT: Modifiers = Modifiers {
        shift: false,
        alt: true,
        ctrl: false,
    };

    #[test]
    fn encodes_ctrl_c_regardless_of_ascii_case() {
        assert_eq!(
            encode_key(KeyEvent::with_modifiers(Key::Char('c'), CTRL), false),
            Some(vec![3])
        );
        assert_eq!(
            encode_key(KeyEvent::with_modifiers(Key::Char('C'), CTRL), false),
            Some(vec![3])
        );
    }

    #[test]
    fn encodes_utf8_and_alt_without_losing_characters() {
        assert_eq!(
            encode_key(KeyEvent::new(Key::Char('Ж')), false),
            Some("Ж".as_bytes().to_vec())
        );
        assert_eq!(
            encode_key(KeyEvent::with_modifiers(Key::Char('🦀'), ALT), false),
            Some([vec![0x1b], "🦀".as_bytes().to_vec()].concat())
        );
    }

    #[test]
    fn rejects_unsupported_ctrl_unicode_instead_of_corrupting_it() {
        assert_eq!(
            encode_key(KeyEvent::with_modifiers(Key::Char('Ж'), CTRL), false),
            None
        );
    }

    #[test]
    fn arrows_follow_normal_application_and_modified_modes() {
        assert_eq!(
            encode_key(KeyEvent::new(Key::Up), false),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            encode_key(KeyEvent::new(Key::Up), true),
            Some(b"\x1bOA".to_vec())
        );

        let modifiers = Modifiers {
            shift: true,
            alt: false,
            ctrl: true,
        };
        assert_eq!(
            encode_key(KeyEvent::with_modifiers(Key::Left, modifiers), false),
            Some(b"\x1b[1;6D".to_vec())
        );
    }

    #[test]
    fn encodes_backtab_and_modified_delete() {
        let shift = Modifiers {
            shift: true,
            alt: false,
            ctrl: false,
        };
        assert_eq!(
            encode_key(KeyEvent::with_modifiers(Key::Tab, shift), false),
            Some(b"\x1b[Z".to_vec())
        );
        assert_eq!(
            encode_key(KeyEvent::with_modifiers(Key::Delete, CTRL), false),
            Some(b"\x1b[3;5~".to_vec())
        );
    }
}
