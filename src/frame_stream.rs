//! CDC 字节流到协议帧的边界判断。

use crate::protocol::{DecodeError, FRAME_OVERHEAD, MAGIC, MAX_FRAME_SIZE, MAX_PAYLOAD_SIZE};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameAction {
    NeedMore,
    DropPrefix(usize),
    Reject {
        len: usize,
        sequence: u16,
        error: DecodeError,
    },
    Process(usize),
}

pub fn shift_left(buf: &mut [u8; MAX_FRAME_SIZE], len: &mut usize, count: usize) {
    if count >= *len {
        *len = 0;
        return;
    }

    let remaining = *len - count;
    buf.copy_within(count..*len, 0);
    *len = remaining;
}

pub fn sequence_from_partial(data: &[u8]) -> u16 {
    if data.len() >= 6 {
        u16::from_be_bytes([data[4], data[5]])
    } else {
        0
    }
}

pub fn next_frame_action(data: &[u8]) -> Option<FrameAction> {
    if data.is_empty() {
        return None;
    }

    if data.len() < 2 {
        return Some(FrameAction::NeedMore);
    }

    if data[0..2] != MAGIC {
        let count = data
            .windows(2)
            .position(|window| window == MAGIC)
            .unwrap_or(data.len().saturating_sub(1));
        return Some(FrameAction::DropPrefix(count.max(1)));
    }

    if data.len() < 9 {
        return Some(FrameAction::NeedMore);
    }

    let payload_len = u16::from_be_bytes([data[7], data[8]]) as usize;
    if payload_len > MAX_PAYLOAD_SIZE {
        return Some(FrameAction::Reject {
            len: data.len().min(FRAME_OVERHEAD),
            sequence: sequence_from_partial(data),
            error: DecodeError::PayloadTooLong,
        });
    }

    let expected_len = FRAME_OVERHEAD + payload_len;
    if data.len() < expected_len {
        return Some(FrameAction::NeedMore);
    }

    Some(FrameAction::Process(expected_len))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{CommandType, encode_frame};

    #[test]
    fn keeps_partial_magic_until_more_bytes_arrive() {
        assert_eq!(next_frame_action(&[MAGIC[0]]), Some(FrameAction::NeedMore));
    }

    #[test]
    fn drops_noise_before_next_magic() {
        assert_eq!(
            next_frame_action(&[0x00, 0x11, MAGIC[0], MAGIC[1], 0x01]),
            Some(FrameAction::DropPrefix(2))
        );
    }

    #[test]
    fn rejects_payloads_that_exceed_protocol_limit() {
        let data = [MAGIC[0], MAGIC[1], 1, 0, 0x12, 0x34, 0x01, 0x00, 0xF1];

        assert_eq!(
            next_frame_action(&data),
            Some(FrameAction::Reject {
                len: FRAME_OVERHEAD.min(data.len()),
                sequence: 0x1234,
                error: DecodeError::PayloadTooLong
            })
        );
    }

    #[test]
    fn reports_complete_frame_length() {
        let mut buf = [0u8; MAX_FRAME_SIZE];
        let len = encode_frame(1, 7, CommandType::Ping, &[], &mut buf).unwrap();

        assert_eq!(
            next_frame_action(&buf[..len]),
            Some(FrameAction::Process(len))
        );
    }

    #[test]
    fn shifts_consumed_bytes_left() {
        let mut buf = [0u8; MAX_FRAME_SIZE];
        buf[..4].copy_from_slice(&[1, 2, 3, 4]);
        let mut len = 4usize;

        shift_left(&mut buf, &mut len, 2);

        assert_eq!(len, 2);
        assert_eq!(&buf[..len], &[3, 4]);
    }
}
