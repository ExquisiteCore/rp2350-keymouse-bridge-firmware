pub const MAGIC: [u8; 2] = [0xA5, 0x5A];
pub const LEGACY_PROTOCOL_VERSION: u8 = 1;
pub const PROTOCOL_VERSION: u8 = 2;
pub const FLAG_NO_RESPONSE: u8 = 0x01;
pub const MAX_WAIT_MS: u32 = 60_000;
pub const MAX_PAYLOAD_SIZE: usize = 240;
pub const FRAME_OVERHEAD: usize = 11;
pub const MAX_FRAME_SIZE: usize = FRAME_OVERHEAD + MAX_PAYLOAD_SIZE;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandType {
    Ping,
    GetInfo,
    GetCaps,
    Heartbeat,
    KeyDown,
    KeyUp,
    KeyTap,
    TypeAscii,
    MouseMoveRel,
    MouseButtonDown,
    MouseButtonUp,
    MouseClick,
    MouseWheel,
    WaitMs,
    BatchBegin,
    BatchEnd,
    StopAll,
    Ack,
    Nack,
    Status,
    Busy,
    Unknown(u8),
}

impl CommandType {
    pub const fn from_byte(value: u8) -> Self {
        match value {
            0x01 => Self::Ping,
            0x02 => Self::GetInfo,
            0x03 => Self::GetCaps,
            0x04 => Self::Heartbeat,
            0x10 => Self::KeyDown,
            0x11 => Self::KeyUp,
            0x12 => Self::KeyTap,
            0x13 => Self::TypeAscii,
            0x20 => Self::MouseMoveRel,
            0x21 => Self::MouseButtonDown,
            0x22 => Self::MouseButtonUp,
            0x23 => Self::MouseClick,
            0x24 => Self::MouseWheel,
            0x30 => Self::WaitMs,
            0x40 => Self::BatchBegin,
            0x41 => Self::BatchEnd,
            0x7F => Self::StopAll,
            0x80 => Self::Ack,
            0x81 => Self::Nack,
            0x82 => Self::Status,
            0x83 => Self::Busy,
            other => Self::Unknown(other),
        }
    }

    pub const fn as_byte(self) -> u8 {
        match self {
            Self::Ping => 0x01,
            Self::GetInfo => 0x02,
            Self::GetCaps => 0x03,
            Self::Heartbeat => 0x04,
            Self::KeyDown => 0x10,
            Self::KeyUp => 0x11,
            Self::KeyTap => 0x12,
            Self::TypeAscii => 0x13,
            Self::MouseMoveRel => 0x20,
            Self::MouseButtonDown => 0x21,
            Self::MouseButtonUp => 0x22,
            Self::MouseClick => 0x23,
            Self::MouseWheel => 0x24,
            Self::WaitMs => 0x30,
            Self::BatchBegin => 0x40,
            Self::BatchEnd => 0x41,
            Self::StopAll => 0x7F,
            Self::Ack => 0x80,
            Self::Nack => 0x81,
            Self::Status => 0x82,
            Self::Busy => 0x83,
            Self::Unknown(value) => value,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Frame<'a> {
    pub version: u8,
    pub flags: u8,
    pub sequence: u16,
    pub command_type: CommandType,
    pub payload: &'a [u8],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestKind {
    ResponseExpected,
    NoResponseHeartbeat,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestError {
    UnsupportedVersion,
    UnsupportedFlags,
    InvalidSequence,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeError {
    TooShort,
    BadMagic,
    LengthMismatch,
    PayloadTooLong,
    BadCrc,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EncodeError {
    PayloadTooLong,
    BufferTooSmall,
}

pub fn encode_frame(
    version: u8,
    sequence: u16,
    command_type: CommandType,
    payload: &[u8],
    out: &mut [u8],
) -> Result<usize, EncodeError> {
    encode_frame_with_flags(version, 0, sequence, command_type, payload, out)
}

pub fn encode_frame_with_flags(
    version: u8,
    flags: u8,
    sequence: u16,
    command_type: CommandType,
    payload: &[u8],
    out: &mut [u8],
) -> Result<usize, EncodeError> {
    if payload.len() > MAX_PAYLOAD_SIZE {
        return Err(EncodeError::PayloadTooLong);
    }

    let total_len = FRAME_OVERHEAD + payload.len();
    if out.len() < total_len {
        return Err(EncodeError::BufferTooSmall);
    }

    out[0..2].copy_from_slice(&MAGIC);
    out[2] = version;
    out[3] = flags;
    out[4..6].copy_from_slice(&sequence.to_be_bytes());
    out[6] = command_type.as_byte();
    out[7..9].copy_from_slice(&(payload.len() as u16).to_be_bytes());
    out[9..9 + payload.len()].copy_from_slice(payload);

    let crc_offset = 9 + payload.len();
    let crc = crc16_ccitt_false(&out[2..crc_offset]);
    out[crc_offset..crc_offset + 2].copy_from_slice(&crc.to_be_bytes());

    Ok(total_len)
}

pub fn validate_request(frame: &Frame<'_>) -> Result<RequestKind, RequestError> {
    match frame.version {
        LEGACY_PROTOCOL_VERSION if frame.flags != 0 => Err(RequestError::UnsupportedFlags),
        LEGACY_PROTOCOL_VERSION if frame.sequence == 0 => Err(RequestError::InvalidSequence),
        LEGACY_PROTOCOL_VERSION => Ok(RequestKind::ResponseExpected),
        PROTOCOL_VERSION
            if frame.command_type == CommandType::Heartbeat
                && frame.flags == FLAG_NO_RESPONSE
                && frame.sequence == 0 =>
        {
            Ok(RequestKind::NoResponseHeartbeat)
        }
        PROTOCOL_VERSION if frame.flags != 0 => Err(RequestError::UnsupportedFlags),
        PROTOCOL_VERSION if frame.sequence == 0 => Err(RequestError::InvalidSequence),
        PROTOCOL_VERSION => Ok(RequestKind::ResponseExpected),
        _ => Err(RequestError::UnsupportedVersion),
    }
}

pub fn decode_frame(input: &[u8]) -> Result<Frame<'_>, DecodeError> {
    if input.len() < FRAME_OVERHEAD {
        return Err(DecodeError::TooShort);
    }

    if input[0..2] != MAGIC {
        return Err(DecodeError::BadMagic);
    }

    let payload_len = u16::from_be_bytes([input[7], input[8]]) as usize;
    if payload_len > MAX_PAYLOAD_SIZE {
        return Err(DecodeError::PayloadTooLong);
    }

    let expected_len = FRAME_OVERHEAD + payload_len;
    if input.len() != expected_len {
        return Err(DecodeError::LengthMismatch);
    }

    let crc_offset = 9 + payload_len;
    let expected_crc = u16::from_be_bytes([input[crc_offset], input[crc_offset + 1]]);
    let actual_crc = crc16_ccitt_false(&input[2..crc_offset]);
    if expected_crc != actual_crc {
        return Err(DecodeError::BadCrc);
    }

    Ok(Frame {
        version: input[2],
        flags: input[3],
        sequence: u16::from_be_bytes([input[4], input[5]]),
        command_type: CommandType::from_byte(input[6]),
        payload: &input[9..crc_offset],
    })
}

pub fn crc16_ccitt_false(data: &[u8]) -> u16 {
    let mut crc = 0xFFFFu16;
    for byte in data {
        crc ^= (*byte as u16) << 8;
        for _ in 0..8 {
            if (crc & 0x8000) != 0 {
                crc = (crc << 1) ^ 0x1021;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_valid_ping_frame() {
        let mut buf = [0u8; MAX_FRAME_SIZE];
        let written = encode_frame(1, 0x1234, CommandType::Ping, &[], &mut buf).unwrap();

        let frame = decode_frame(&buf[..written]).unwrap();

        assert_eq!(frame.version, 1);
        assert_eq!(frame.flags, 0);
        assert_eq!(frame.sequence, 0x1234);
        assert_eq!(frame.command_type, CommandType::Ping);
        assert_eq!(frame.payload, &[]);
    }

    #[test]
    fn rejects_bad_crc() {
        let mut buf = [0u8; MAX_FRAME_SIZE];
        let written = encode_frame(1, 7, CommandType::Ping, &[], &mut buf).unwrap();
        buf[written - 1] ^= 0x55;

        assert_eq!(decode_frame(&buf[..written]), Err(DecodeError::BadCrc));
    }

    #[test]
    fn rejects_short_frame() {
        assert_eq!(decode_frame(&[0xA5, 0x5A]), Err(DecodeError::TooShort));
    }

    #[test]
    fn rejects_payload_length_mismatch() {
        let frame = [0xA5, 0x5A, 1, 0, 0, 1, 0x01, 0, 4, 0, 0];

        assert_eq!(decode_frame(&frame), Err(DecodeError::LengthMismatch));
    }

    #[test]
    fn v2_heartbeat_supports_no_response_flag() {
        let mut buf = [0u8; MAX_FRAME_SIZE];
        let len = encode_frame_with_flags(
            2,
            FLAG_NO_RESPONSE,
            0,
            CommandType::Heartbeat,
            &[],
            &mut buf,
        )
        .unwrap();
        let frame = decode_frame(&buf[..len]).unwrap();
        assert_eq!(
            validate_request(&frame),
            Ok(RequestKind::NoResponseHeartbeat)
        );
    }

    #[test]
    fn v1_rejects_flags_and_v2_rejects_zero_command_sequence() {
        let v1 = Frame {
            version: 1,
            flags: FLAG_NO_RESPONSE,
            sequence: 1,
            command_type: CommandType::Ping,
            payload: &[],
        };
        assert_eq!(validate_request(&v1), Err(RequestError::UnsupportedFlags));

        let v2 = Frame {
            version: 2,
            flags: 0,
            sequence: 0,
            command_type: CommandType::Ping,
            payload: &[],
        };
        assert_eq!(validate_request(&v2), Err(RequestError::InvalidSequence));
    }

    #[test]
    fn heartbeat_wire_id_is_stable() {
        assert_eq!(CommandType::Heartbeat.as_byte(), 0x04);
        assert_eq!(CommandType::from_byte(0x04), CommandType::Heartbeat);
    }

    #[test]
    fn encode_frame_remains_a_zero_flags_wrapper() {
        let mut legacy = [0u8; MAX_FRAME_SIZE];
        let mut explicit = [0u8; MAX_FRAME_SIZE];
        let legacy_len = encode_frame(2, 8, CommandType::Ping, b"ok", &mut legacy).unwrap();
        let explicit_len =
            encode_frame_with_flags(2, 0, 8, CommandType::Ping, b"ok", &mut explicit).unwrap();

        assert_eq!(&legacy[..legacy_len], &explicit[..explicit_len]);
    }

    #[test]
    fn crc_covers_version_and_flags() {
        let mut buf = [0u8; MAX_FRAME_SIZE];
        let len = encode_frame_with_flags(
            2,
            FLAG_NO_RESPONSE,
            0,
            CommandType::Heartbeat,
            &[],
            &mut buf,
        )
        .unwrap();

        let mut changed_version = buf;
        changed_version[2] = 1;
        assert_eq!(
            decode_frame(&changed_version[..len]),
            Err(DecodeError::BadCrc)
        );

        let mut changed_flags = buf;
        changed_flags[3] = 0;
        assert_eq!(
            decode_frame(&changed_flags[..len]),
            Err(DecodeError::BadCrc)
        );
    }

    #[test]
    fn validation_rejects_invalid_heartbeat_combinations_and_versions() {
        let flagged_with_sequence = Frame {
            version: PROTOCOL_VERSION,
            flags: FLAG_NO_RESPONSE,
            sequence: 1,
            command_type: CommandType::Heartbeat,
            payload: &[],
        };
        assert_eq!(
            validate_request(&flagged_with_sequence),
            Err(RequestError::UnsupportedFlags)
        );

        let unflagged_without_sequence = Frame {
            version: PROTOCOL_VERSION,
            flags: 0,
            sequence: 0,
            command_type: CommandType::Heartbeat,
            payload: &[],
        };
        assert_eq!(
            validate_request(&unflagged_without_sequence),
            Err(RequestError::InvalidSequence)
        );

        let legacy_without_sequence = Frame {
            version: LEGACY_PROTOCOL_VERSION,
            flags: 0,
            sequence: 0,
            command_type: CommandType::Ping,
            payload: &[],
        };
        assert_eq!(
            validate_request(&legacy_without_sequence),
            Err(RequestError::InvalidSequence)
        );

        let unknown_version = Frame {
            version: PROTOCOL_VERSION + 1,
            flags: 0,
            sequence: 1,
            command_type: CommandType::Ping,
            payload: &[],
        };
        assert_eq!(
            validate_request(&unknown_version),
            Err(RequestError::UnsupportedVersion)
        );
    }
}
