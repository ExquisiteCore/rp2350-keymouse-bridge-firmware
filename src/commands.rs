use crate::protocol::{CommandType, Frame};

pub const MOD_LEFT_SHIFT: u8 = 0x02;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyStroke {
    pub modifier: u8,
    pub keycode: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

impl MouseButton {
    pub const fn from_mask(mask: u8) -> Option<Self> {
        match mask {
            0x01 => Some(Self::Left),
            0x02 => Some(Self::Right),
            0x04 => Some(Self::Middle),
            _ => None,
        }
    }

    pub const fn mask(self) -> u8 {
        match self {
            Self::Left => 0x01,
            Self::Right => 0x02,
            Self::Middle => 0x04,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command<'a> {
    Ping,
    GetInfo,
    GetCaps,
    KeyDown(KeyStroke),
    KeyUp(KeyStroke),
    KeyTap(KeyStroke),
    TypeAscii(&'a [u8]),
    MouseMoveRel { dx: i16, dy: i16 },
    MouseButtonDown(MouseButton),
    MouseButtonUp(MouseButton),
    MouseClick(MouseButton),
    MouseWheel(i8),
    WaitMs(u32),
    BatchBegin,
    BatchEnd,
    StopAll,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandError {
    InvalidPayloadLength,
    InvalidMouseButton,
    UnsupportedCommand,
}

pub fn decode_command<'a>(frame: &Frame<'a>) -> Result<Command<'a>, CommandError> {
    match frame.command_type {
        CommandType::Ping => expect_empty(frame.payload, Command::Ping),
        CommandType::GetInfo => expect_empty(frame.payload, Command::GetInfo),
        CommandType::GetCaps => expect_empty(frame.payload, Command::GetCaps),
        CommandType::KeyDown => decode_keystroke(frame.payload).map(Command::KeyDown),
        CommandType::KeyUp => decode_keystroke(frame.payload).map(Command::KeyUp),
        CommandType::KeyTap => decode_keystroke(frame.payload).map(Command::KeyTap),
        CommandType::TypeAscii => Ok(Command::TypeAscii(frame.payload)),
        CommandType::MouseMoveRel => decode_mouse_move(frame.payload),
        CommandType::MouseButtonDown => {
            decode_mouse_button(frame.payload).map(Command::MouseButtonDown)
        }
        CommandType::MouseButtonUp => {
            decode_mouse_button(frame.payload).map(Command::MouseButtonUp)
        }
        CommandType::MouseClick => decode_mouse_button(frame.payload).map(Command::MouseClick),
        CommandType::MouseWheel => decode_mouse_wheel(frame.payload),
        CommandType::WaitMs => decode_wait_ms(frame.payload),
        CommandType::BatchBegin => expect_empty(frame.payload, Command::BatchBegin),
        CommandType::BatchEnd => expect_empty(frame.payload, Command::BatchEnd),
        CommandType::StopAll => expect_empty(frame.payload, Command::StopAll),
        _ => Err(CommandError::UnsupportedCommand),
    }
}

fn expect_empty<'a>(payload: &[u8], command: Command<'a>) -> Result<Command<'a>, CommandError> {
    if payload.is_empty() {
        Ok(command)
    } else {
        Err(CommandError::InvalidPayloadLength)
    }
}

fn decode_keystroke(payload: &[u8]) -> Result<KeyStroke, CommandError> {
    if payload.len() != 2 {
        return Err(CommandError::InvalidPayloadLength);
    }

    Ok(KeyStroke {
        modifier: payload[0],
        keycode: payload[1],
    })
}

fn decode_mouse_move(payload: &[u8]) -> Result<Command<'_>, CommandError> {
    if payload.len() != 4 {
        return Err(CommandError::InvalidPayloadLength);
    }

    Ok(Command::MouseMoveRel {
        dx: i16::from_be_bytes([payload[0], payload[1]]),
        dy: i16::from_be_bytes([payload[2], payload[3]]),
    })
}

fn decode_mouse_button(payload: &[u8]) -> Result<MouseButton, CommandError> {
    if payload.len() != 1 {
        return Err(CommandError::InvalidPayloadLength);
    }

    MouseButton::from_mask(payload[0]).ok_or(CommandError::InvalidMouseButton)
}

fn decode_mouse_wheel(payload: &[u8]) -> Result<Command<'_>, CommandError> {
    if payload.len() != 1 {
        return Err(CommandError::InvalidPayloadLength);
    }

    Ok(Command::MouseWheel(payload[0] as i8))
}

fn decode_wait_ms(payload: &[u8]) -> Result<Command<'_>, CommandError> {
    if payload.len() != 4 {
        return Err(CommandError::InvalidPayloadLength);
    }

    Ok(Command::WaitMs(u32::from_be_bytes([
        payload[0], payload[1], payload[2], payload[3],
    ])))
}

pub fn ascii_to_keystroke(byte: u8) -> Option<KeyStroke> {
    let (modifier, keycode) = match byte {
        b'a'..=b'z' => (0, 0x04 + (byte - b'a')),
        b'A'..=b'Z' => (MOD_LEFT_SHIFT, 0x04 + (byte - b'A')),
        b'1'..=b'9' => (0, 0x1E + (byte - b'1')),
        b'0' => (0, 0x27),
        b'\n' | b'\r' => (0, 0x28),
        b'\x08' => (0, 0x2A),
        b'\t' => (0, 0x2B),
        b' ' => (0, 0x2C),
        b'-' => (0, 0x2D),
        b'_' => (MOD_LEFT_SHIFT, 0x2D),
        b'=' => (0, 0x2E),
        b'+' => (MOD_LEFT_SHIFT, 0x2E),
        b'[' => (0, 0x2F),
        b'{' => (MOD_LEFT_SHIFT, 0x2F),
        b']' => (0, 0x30),
        b'}' => (MOD_LEFT_SHIFT, 0x30),
        b'\\' => (0, 0x31),
        b'|' => (MOD_LEFT_SHIFT, 0x31),
        b';' => (0, 0x33),
        b':' => (MOD_LEFT_SHIFT, 0x33),
        b'\'' => (0, 0x34),
        b'"' => (MOD_LEFT_SHIFT, 0x34),
        b'`' => (0, 0x35),
        b'~' => (MOD_LEFT_SHIFT, 0x35),
        b',' => (0, 0x36),
        b'<' => (MOD_LEFT_SHIFT, 0x36),
        b'.' => (0, 0x37),
        b'>' => (MOD_LEFT_SHIFT, 0x37),
        b'/' => (0, 0x38),
        b'?' => (MOD_LEFT_SHIFT, 0x38),
        b'!' => (MOD_LEFT_SHIFT, 0x1E),
        b'@' => (MOD_LEFT_SHIFT, 0x1F),
        b'#' => (MOD_LEFT_SHIFT, 0x20),
        b'$' => (MOD_LEFT_SHIFT, 0x21),
        b'%' => (MOD_LEFT_SHIFT, 0x22),
        b'^' => (MOD_LEFT_SHIFT, 0x23),
        b'&' => (MOD_LEFT_SHIFT, 0x24),
        b'*' => (MOD_LEFT_SHIFT, 0x25),
        b'(' => (MOD_LEFT_SHIFT, 0x26),
        b')' => (MOD_LEFT_SHIFT, 0x27),
        _ => return None,
    };

    Some(KeyStroke { modifier, keycode })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{CommandType, Frame};

    fn frame<'a>(command_type: CommandType, payload: &'a [u8]) -> Frame<'a> {
        Frame {
            version: 1,
            flags: 0,
            sequence: 42,
            command_type,
            payload,
        }
    }

    #[test]
    fn decodes_keyboard_tap_payload() {
        let command = decode_command(&frame(CommandType::KeyTap, &[0x02, 0x04])).unwrap();

        assert_eq!(
            command,
            Command::KeyTap(KeyStroke {
                modifier: 0x02,
                keycode: 0x04
            })
        );
    }

    #[test]
    fn decodes_type_ascii_payload() {
        let command = decode_command(&frame(CommandType::TypeAscii, b"Hi!")).unwrap();

        assert_eq!(command, Command::TypeAscii(b"Hi!"));
    }

    #[test]
    fn maps_ascii_to_hid_keystrokes() {
        assert_eq!(
            ascii_to_keystroke(b'A'),
            Some(KeyStroke {
                modifier: MOD_LEFT_SHIFT,
                keycode: 0x04
            })
        );
        assert_eq!(
            ascii_to_keystroke(b'!'),
            Some(KeyStroke {
                modifier: MOD_LEFT_SHIFT,
                keycode: 0x1E
            })
        );
        assert_eq!(
            ascii_to_keystroke(b' '),
            Some(KeyStroke {
                modifier: 0,
                keycode: 0x2C
            })
        );
    }

    #[test]
    fn decodes_mouse_move_payload() {
        let command =
            decode_command(&frame(CommandType::MouseMoveRel, &[0x01, 0x2C, 0xFF, 0x38])).unwrap();

        assert_eq!(command, Command::MouseMoveRel { dx: 300, dy: -200 });
    }

    #[test]
    fn decodes_mouse_click_payload() {
        let command = decode_command(&frame(CommandType::MouseClick, &[0x01])).unwrap();

        assert_eq!(command, Command::MouseClick(MouseButton::Left));
    }

    #[test]
    fn decodes_wait_and_stop_all() {
        let wait = decode_command(&frame(CommandType::WaitMs, &[0, 0, 0x03, 0xE8])).unwrap();
        let stop = decode_command(&frame(CommandType::StopAll, &[])).unwrap();

        assert_eq!(wait, Command::WaitMs(1000));
        assert_eq!(stop, Command::StopAll);
    }

    #[test]
    fn decodes_capabilities_and_batch_commands() {
        let caps = decode_command(&frame(CommandType::GetCaps, &[])).unwrap();
        let batch_begin = decode_command(&frame(CommandType::BatchBegin, &[])).unwrap();
        let batch_end = decode_command(&frame(CommandType::BatchEnd, &[])).unwrap();

        assert_eq!(caps, Command::GetCaps);
        assert_eq!(batch_begin, Command::BatchBegin);
        assert_eq!(batch_end, Command::BatchEnd);
    }

    #[test]
    fn rejects_wrong_payload_length() {
        let error = decode_command(&frame(CommandType::MouseMoveRel, &[1, 2, 3])).unwrap_err();

        assert_eq!(error, CommandError::InvalidPayloadLength);
    }
}
