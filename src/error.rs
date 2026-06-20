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
}
