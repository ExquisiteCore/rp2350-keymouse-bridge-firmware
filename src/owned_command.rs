use crate::commands::{Command, CommandError, KeyStroke, MouseButton, decode_command};
use crate::input_state::MAX_ASCII_STROKES;
use crate::protocol::Frame;
use heapless::Vec;

// The largest wire payload must remain inline so commands stay allocation-free under `no_std`.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OwnedCommand {
    Ping,
    GetInfo,
    GetCaps,
    Heartbeat,
    KeyDown(KeyStroke),
    KeyUp(KeyStroke),
    KeyTap(KeyStroke),
    TypeAscii(Vec<u8, MAX_ASCII_STROKES>),
    MouseMoveRel { dx: i16, dy: i16 },
    MouseButtonDown(MouseButton),
    MouseButtonUp(MouseButton),
    MouseClick(MouseButton),
    MouseWheel(i8),
    WaitMs(u32),
    StopAll,
}

impl OwnedCommand {
    pub fn from_frame(frame: &Frame<'_>) -> Result<Self, CommandError> {
        match decode_command(frame)? {
            Command::Ping => Ok(Self::Ping),
            Command::GetInfo => Ok(Self::GetInfo),
            Command::GetCaps => Ok(Self::GetCaps),
            Command::Heartbeat => Ok(Self::Heartbeat),
            Command::KeyDown(stroke) => Ok(Self::KeyDown(stroke)),
            Command::KeyUp(stroke) => Ok(Self::KeyUp(stroke)),
            Command::KeyTap(stroke) => Ok(Self::KeyTap(stroke)),
            Command::TypeAscii(bytes) => Self::type_ascii(bytes),
            Command::MouseMoveRel { dx, dy } => Ok(Self::MouseMoveRel { dx, dy }),
            Command::MouseButtonDown(button) => Ok(Self::MouseButtonDown(button)),
            Command::MouseButtonUp(button) => Ok(Self::MouseButtonUp(button)),
            Command::MouseClick(button) => Ok(Self::MouseClick(button)),
            Command::MouseWheel(wheel) => Ok(Self::MouseWheel(wheel)),
            Command::WaitMs(wait_ms) => Ok(Self::WaitMs(wait_ms)),
            Command::StopAll => Ok(Self::StopAll),
            Command::BatchBegin => Err(CommandError::UnsupportedCommand),
            Command::BatchEnd => Err(CommandError::UnsupportedCommand),
        }
    }

    pub fn type_ascii(bytes: &[u8]) -> Result<Self, CommandError> {
        let mut owned = Vec::new();
        owned
            .extend_from_slice(bytes)
            .map_err(|_| CommandError::InvalidPayloadLength)?;
        Ok(Self::TypeAscii(owned))
    }

    pub fn payload_len(&self) -> usize {
        match self {
            Self::Ping | Self::GetInfo | Self::GetCaps | Self::Heartbeat | Self::StopAll => 0,
            Self::KeyDown(_) | Self::KeyUp(_) | Self::KeyTap(_) => 2,
            Self::TypeAscii(bytes) => bytes.len(),
            Self::MouseMoveRel { .. } | Self::WaitMs(_) => 4,
            Self::MouseButtonDown(_)
            | Self::MouseButtonUp(_)
            | Self::MouseClick(_)
            | Self::MouseWheel(_) => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{CommandError, KeyStroke, MouseButton};
    use crate::input_state::MAX_ASCII_STROKES;
    use crate::protocol::{CommandType, Frame, MAX_WAIT_MS};

    fn frame<'a>(command_type: CommandType, payload: &'a [u8]) -> Frame<'a> {
        Frame {
            version: 2,
            flags: 0,
            sequence: 7,
            command_type,
            payload,
        }
    }

    #[test]
    fn owned_type_ascii_is_independent_and_enforces_wire_capacity() {
        let mut source = [b'a'; MAX_ASCII_STROKES];
        let owned = OwnedCommand::from_frame(&frame(CommandType::TypeAscii, &source)).unwrap();

        source[0] = b'z';
        assert_eq!(source[0], b'z');

        let OwnedCommand::TypeAscii(bytes) = owned else {
            panic!("expected owned TypeAscii command");
        };
        assert_eq!(bytes.len(), MAX_ASCII_STROKES);
        assert_eq!(bytes[0], b'a');
        assert_eq!(
            OwnedCommand::type_ascii(&[b'a'; MAX_ASCII_STROKES + 1]),
            Err(CommandError::InvalidPayloadLength)
        );
    }

    #[test]
    fn every_storable_decoded_command_maps_exactly() {
        let cases = [
            (frame(CommandType::Ping, &[]), OwnedCommand::Ping),
            (frame(CommandType::GetInfo, &[]), OwnedCommand::GetInfo),
            (frame(CommandType::GetCaps, &[]), OwnedCommand::GetCaps),
            (frame(CommandType::Heartbeat, &[]), OwnedCommand::Heartbeat),
            (
                frame(CommandType::KeyDown, &[0x02, 0x04]),
                OwnedCommand::KeyDown(KeyStroke {
                    modifier: 0x02,
                    keycode: 0x04,
                }),
            ),
            (
                frame(CommandType::KeyUp, &[0x01, 0x05]),
                OwnedCommand::KeyUp(KeyStroke {
                    modifier: 0x01,
                    keycode: 0x05,
                }),
            ),
            (
                frame(CommandType::KeyTap, &[0x04, 0x06]),
                OwnedCommand::KeyTap(KeyStroke {
                    modifier: 0x04,
                    keycode: 0x06,
                }),
            ),
            (
                frame(CommandType::MouseMoveRel, &[0x01, 0x2C, 0xFF, 0x38]),
                OwnedCommand::MouseMoveRel { dx: 300, dy: -200 },
            ),
            (
                frame(CommandType::MouseButtonDown, &[0x01]),
                OwnedCommand::MouseButtonDown(MouseButton::Left),
            ),
            (
                frame(CommandType::MouseButtonUp, &[0x02]),
                OwnedCommand::MouseButtonUp(MouseButton::Right),
            ),
            (
                frame(CommandType::MouseClick, &[0x04]),
                OwnedCommand::MouseClick(MouseButton::Middle),
            ),
            (
                frame(CommandType::MouseWheel, &[0xFE]),
                OwnedCommand::MouseWheel(-2),
            ),
            (
                frame(CommandType::WaitMs, &[0, 0, 0x03, 0xE8]),
                OwnedCommand::WaitMs(1_000),
            ),
            (frame(CommandType::StopAll, &[]), OwnedCommand::StopAll),
        ];

        for (input, expected) in cases {
            assert_eq!(OwnedCommand::from_frame(&input), Ok(expected));
        }

        assert_eq!(
            OwnedCommand::from_frame(&frame(CommandType::TypeAscii, b"Hi!")),
            OwnedCommand::type_ascii(b"Hi!")
        );
    }

    #[test]
    fn batch_markers_are_not_storable_owned_commands() {
        assert_eq!(
            OwnedCommand::from_frame(&frame(CommandType::BatchBegin, &[])),
            Err(CommandError::UnsupportedCommand)
        );
        assert_eq!(
            OwnedCommand::from_frame(&frame(CommandType::BatchEnd, &[])),
            Err(CommandError::UnsupportedCommand)
        );
    }

    #[test]
    fn heartbeat_requires_an_empty_payload() {
        assert_eq!(
            OwnedCommand::from_frame(&frame(CommandType::Heartbeat, &[])),
            Ok(OwnedCommand::Heartbeat)
        );
        assert_eq!(
            OwnedCommand::from_frame(&frame(CommandType::Heartbeat, &[1])),
            Err(CommandError::InvalidPayloadLength)
        );
    }

    #[test]
    fn wait_decoding_preserves_the_sixty_second_cap() {
        assert_eq!(
            OwnedCommand::from_frame(&frame(CommandType::WaitMs, &MAX_WAIT_MS.to_be_bytes())),
            Ok(OwnedCommand::WaitMs(MAX_WAIT_MS))
        );
        assert_eq!(
            OwnedCommand::from_frame(&frame(
                CommandType::WaitMs,
                &(MAX_WAIT_MS + 1).to_be_bytes()
            )),
            Err(CommandError::WaitTooLong)
        );
    }

    #[test]
    fn payload_len_matches_each_wire_command_payload() {
        let cases = [
            (OwnedCommand::Ping, 0),
            (OwnedCommand::GetInfo, 0),
            (OwnedCommand::GetCaps, 0),
            (OwnedCommand::Heartbeat, 0),
            (
                OwnedCommand::KeyDown(KeyStroke {
                    modifier: 1,
                    keycode: 2,
                }),
                2,
            ),
            (
                OwnedCommand::KeyUp(KeyStroke {
                    modifier: 1,
                    keycode: 2,
                }),
                2,
            ),
            (
                OwnedCommand::KeyTap(KeyStroke {
                    modifier: 1,
                    keycode: 2,
                }),
                2,
            ),
            (OwnedCommand::type_ascii(b"hello").unwrap(), 5),
            (OwnedCommand::MouseMoveRel { dx: -1, dy: 2 }, 4),
            (OwnedCommand::MouseButtonDown(MouseButton::Left), 1),
            (OwnedCommand::MouseButtonUp(MouseButton::Right), 1),
            (OwnedCommand::MouseClick(MouseButton::Middle), 1),
            (OwnedCommand::MouseWheel(-1), 1),
            (OwnedCommand::WaitMs(42), 4),
            (OwnedCommand::StopAll, 0),
        ];

        for (command, expected) in cases {
            assert_eq!(command.payload_len(), expected);
        }
    }
}
