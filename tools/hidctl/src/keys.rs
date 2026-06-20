use anyhow::{anyhow, bail, Result};
use hid_protocol::commands::MOD_LEFT_SHIFT;

pub const MOD_LEFT_CTRL: u8 = 0x01;
pub const MOD_LEFT_ALT: u8 = 0x04;
pub const MOD_LEFT_GUI: u8 = 0x08;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyCombo {
    pub modifier: u8,
    pub keycode: u8,
}

pub fn parse_combo(input: &str) -> Result<KeyCombo> {
    let mut modifier = 0u8;
    let mut keycode = None;

    for part in input.split('+') {
        let token = part.trim().to_ascii_uppercase();
        match token.as_str() {
            "" => bail!("empty key token in combo `{input}`"),
            "CTRL" | "CONTROL" => modifier |= MOD_LEFT_CTRL,
            "SHIFT" => modifier |= MOD_LEFT_SHIFT,
            "ALT" => modifier |= MOD_LEFT_ALT,
            "GUI" | "WIN" | "META" => modifier |= MOD_LEFT_GUI,
            key => {
                if keycode.replace(parse_keycode(key)?).is_some() {
                    bail!("combo `{input}` contains more than one non-modifier key");
                }
            }
        }
    }

    Ok(KeyCombo {
        modifier,
        keycode: keycode.ok_or_else(|| anyhow!("combo `{input}` has no key"))?,
    })
}

fn parse_keycode(key: &str) -> Result<u8> {
    if key.len() == 1 {
        let byte = key.as_bytes()[0];
        if byte.is_ascii_uppercase() {
            return Ok(0x04 + (byte - b'A'));
        }
        if (b'1'..=b'9').contains(&byte) {
            return Ok(0x1E + (byte - b'1'));
        }
    }

    match key {
        "0" => Ok(0x27),
        "ENTER" | "RETURN" => Ok(0x28),
        "ESC" | "ESCAPE" => Ok(0x29),
        "BACKSPACE" | "BKSP" => Ok(0x2A),
        "TAB" => Ok(0x2B),
        "SPACE" => Ok(0x2C),
        "MINUS" | "-" => Ok(0x2D),
        "EQUAL" | "=" => Ok(0x2E),
        "LBRACKET" | "[" => Ok(0x2F),
        "RBRACKET" | "]" => Ok(0x30),
        "BACKSLASH" | "\\" => Ok(0x31),
        "SEMICOLON" | ";" => Ok(0x33),
        "QUOTE" | "'" => Ok(0x34),
        "GRAVE" | "`" => Ok(0x35),
        "COMMA" | "," => Ok(0x36),
        "DOT" | "PERIOD" | "." => Ok(0x37),
        "SLASH" | "/" => Ok(0x38),
        "CAPSLOCK" => Ok(0x39),
        "F1" => Ok(0x3A),
        "F2" => Ok(0x3B),
        "F3" => Ok(0x3C),
        "F4" => Ok(0x3D),
        "F5" => Ok(0x3E),
        "F6" => Ok(0x3F),
        "F7" => Ok(0x40),
        "F8" => Ok(0x41),
        "F9" => Ok(0x42),
        "F10" => Ok(0x43),
        "F11" => Ok(0x44),
        "F12" => Ok(0x45),
        "PRINTSCREEN" | "PRTSCR" => Ok(0x46),
        "SCROLLLOCK" => Ok(0x47),
        "PAUSE" => Ok(0x48),
        "INSERT" => Ok(0x49),
        "HOME" => Ok(0x4A),
        "PAGEUP" | "PGUP" => Ok(0x4B),
        "DELETE" | "DEL" => Ok(0x4C),
        "END" => Ok(0x4D),
        "PAGEDOWN" | "PGDN" => Ok(0x4E),
        "RIGHT" => Ok(0x4F),
        "LEFT" => Ok(0x50),
        "DOWN" => Ok(0x51),
        "UP" => Ok(0x52),
        _ => bail!("unknown key `{key}`"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_keys() {
        assert_eq!(
            parse_combo("ENTER").unwrap(),
            KeyCombo {
                modifier: 0,
                keycode: 0x28
            }
        );
        assert_eq!(
            parse_combo("F5").unwrap(),
            KeyCombo {
                modifier: 0,
                keycode: 0x3E
            }
        );
    }

    #[test]
    fn parses_modified_keys() {
        assert_eq!(
            parse_combo("CTRL+C").unwrap(),
            KeyCombo {
                modifier: MOD_LEFT_CTRL,
                keycode: 0x06
            }
        );
        assert_eq!(
            parse_combo("SHIFT+R").unwrap(),
            KeyCombo {
                modifier: MOD_LEFT_SHIFT,
                keycode: 0x15
            }
        );
    }
}
