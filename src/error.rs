//! 固件响应给主机的错误码。

use crate::protocol::DecodeError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ErrorCode {
    BadFrame = 1,
    BadCommand = 2,
    UnsupportedAscii = 3,
    HidWrite = 4,
    Transport = 5,
    FrameTooLong = 6,
    UnsupportedVersion = 7,
    UnsupportedFlags = 8,
    InvalidSequence = 9,
    SequenceConflict = 10,
    BatchState = 11,
    BatchCapacity = 12,
    TooManyKeys = 13,
    WaitTooLong = 14,
    KeyboardBusy = 15,
    Cancelled = 16,
}

impl ErrorCode {
    pub const fn from_decode(error: DecodeError) -> Self {
        match error {
            DecodeError::TooShort
            | DecodeError::BadMagic
            | DecodeError::LengthMismatch
            | DecodeError::PayloadTooLong
            | DecodeError::BadCrc => Self::BadFrame,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_decode_errors_map_to_bad_frame() {
        let errors = [
            DecodeError::TooShort,
            DecodeError::BadMagic,
            DecodeError::LengthMismatch,
            DecodeError::PayloadTooLong,
            DecodeError::BadCrc,
        ];

        for error in errors {
            assert_eq!(ErrorCode::from_decode(error), ErrorCode::BadFrame);
        }
    }

    #[test]
    fn wire_error_ids_are_stable() {
        assert_eq!(ErrorCode::BadFrame as u8, 1);
        assert_eq!(ErrorCode::BadCommand as u8, 2);
        assert_eq!(ErrorCode::UnsupportedAscii as u8, 3);
        assert_eq!(ErrorCode::HidWrite as u8, 4);
        assert_eq!(ErrorCode::Transport as u8, 5);
        assert_eq!(ErrorCode::FrameTooLong as u8, 6);
        assert_eq!(ErrorCode::UnsupportedVersion as u8, 7);
        assert_eq!(ErrorCode::UnsupportedFlags as u8, 8);
        assert_eq!(ErrorCode::InvalidSequence as u8, 9);
        assert_eq!(ErrorCode::SequenceConflict as u8, 10);
        assert_eq!(ErrorCode::BatchState as u8, 11);
        assert_eq!(ErrorCode::BatchCapacity as u8, 12);
        assert_eq!(ErrorCode::TooManyKeys as u8, 13);
        assert_eq!(ErrorCode::WaitTooLong as u8, 14);
        assert_eq!(ErrorCode::KeyboardBusy as u8, 15);
        assert_eq!(ErrorCode::Cancelled as u8, 16);
    }
}
