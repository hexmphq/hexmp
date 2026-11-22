//! Nomes das teclas que scripts podem pedir ↔ virtual-key code do Windows.
//!
//! Esc e F8 ficam fora de propósito: são do jogo e do cliente (recuperar o input e sair).

const NAMED: &[(&str, u32)] = &[
    ("BACKSPACE", 0x08),
    ("TAB", 0x09),
    ("ENTER", 0x0D),
    ("SHIFT", 0x10),
    ("CTRL", 0x11),
    ("ALT", 0x12),
    ("CAPSLOCK", 0x14),
    ("SPACE", 0x20),
    ("PAGEUP", 0x21),
    ("PAGEDOWN", 0x22),
    ("END", 0x23),
    ("HOME", 0x24),
    ("LEFT", 0x25),
    ("UP", 0x26),
    ("RIGHT", 0x27),
    ("DOWN", 0x28),
    ("INSERT", 0x2D),
    ("DELETE", 0x2E),
    ("F1", 0x70),
    ("F2", 0x71),
    ("F3", 0x72),
    ("F4", 0x73),
    ("F5", 0x74),
    ("F6", 0x75),
    ("F7", 0x76),
    ("F9", 0x78),
    ("F10", 0x79),
    ("F11", 0x7A),
    ("F12", 0x7B),
];

/// Maiúsculas ou minúsculas tanto faz.
pub fn code(name: &str) -> Option<u32> {
    let name = name.to_ascii_uppercase();
    if let [c] = name.as_bytes()
        && c.is_ascii_alphanumeric()
    {
        return Some(u32::from(*c));
    }
    NAMED.iter().find(|(n, _)| *n == name).map(|(_, c)| *c)
}

pub fn name(code: u32) -> Option<String> {
    match u8::try_from(code) {
        Ok(c) if c.is_ascii_digit() || c.is_ascii_uppercase() => Some(char::from(c).to_string()),
        _ => NAMED
            .iter()
            .find(|(_, c)| *c == code)
            .map(|(n, _)| (*n).to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_codes() {
        for n in ["A", "7", "F1", "F12", "SPACE"] {
            assert_eq!(code(n).and_then(name).as_deref(), Some(n));
        }
        assert_eq!(code("f3"), Some(0x72));
        for n in ["", "ESC", "F8", "F13", "AB", "é"] {
            assert_eq!(code(n), None, "{n}");
        }
        // 'p' minúsculo é o código de F1.
        assert_eq!(name(0x70).as_deref(), Some("F1"));
    }
}
