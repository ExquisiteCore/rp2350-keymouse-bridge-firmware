//! CDC 字节流到协议帧的边界判断。

use crate::protocol::{DecodeError, FRAME_OVERHEAD, MAGIC, MAX_FRAME_SIZE, MAX_PAYLOAD_SIZE};
use crate::safety::PartialFrameDeadline;

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

/// Appends the next bounded portion of one CDC packet into the frame buffer.
///
/// The caller drains complete/rejected frames after each chunk, then calls this
/// again until `packet_offset == packet.len()`.
pub fn append_packet_chunk(
    buf: &mut [u8; MAX_FRAME_SIZE],
    len: &mut usize,
    packet: &[u8],
    packet_offset: &mut usize,
) -> usize {
    if *len >= buf.len() || *packet_offset >= packet.len() {
        return 0;
    }

    let copied = (buf.len() - *len).min(packet.len() - *packet_offset);
    buf[*len..*len + copied].copy_from_slice(&packet[*packet_offset..*packet_offset + copied]);
    *len += copied;
    *packet_offset += copied;
    copied
}

/// Drops a stalled partial frame without inventing a protocol response.
///
/// On expiration, this authoritatively resets the buffered length to zero and
/// disarms the deadline. The caller remains responsible for response policy,
/// including emitting no response when the sequence is not reliable.
pub fn clear_expired_partial(
    len: &mut usize,
    deadline: &mut PartialFrameDeadline,
    now_ms: u64,
) -> bool {
    if !deadline.expired(now_ms) {
        return false;
    }

    *len = 0;
    deadline.clear();
    true
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
    use crate::safety::{PARTIAL_FRAME_TIMEOUT_MS, PartialFrameDeadline};

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

    #[test]
    fn partial_stall_clears_buffer_and_allows_valid_frame_recovery() {
        let mut buf = [0u8; MAX_FRAME_SIZE];
        buf[..4].copy_from_slice(&[MAGIC[0], MAGIC[1], 1, 0]);
        let mut len = 4;
        let mut deadline = PartialFrameDeadline::new(PARTIAL_FRAME_TIMEOUT_MS);
        deadline.note_bytes(1_000, len);

        assert_eq!(next_frame_action(&buf[..len]), Some(FrameAction::NeedMore));
        assert!(!clear_expired_partial(&mut len, &mut deadline, 1_249));
        assert_eq!(len, 4);
        assert!(clear_expired_partial(&mut len, &mut deadline, 1_250));
        assert_eq!(len, 0);
        assert!(!deadline.is_active());

        len = encode_frame(1, 7, CommandType::Ping, &[], &mut buf).unwrap();
        deadline.note_bytes(1_300, len);
        assert_eq!(
            next_frame_action(&buf[..len]),
            Some(FrameAction::Process(len))
        );

        let consumed = len;
        shift_left(&mut buf, &mut len, consumed);
        deadline.note_bytes(1_300, len);
        assert_eq!(len, 0);
        assert!(!deadline.is_active());
    }

    #[test]
    fn fragmented_progress_refreshes_the_partial_deadline() {
        let mut frame = [0u8; MAX_FRAME_SIZE];
        let frame_len = encode_frame(1, 8, CommandType::Ping, &[], &mut frame).unwrap();
        let mut buf = [0u8; MAX_FRAME_SIZE];
        buf[..4].copy_from_slice(&frame[..4]);
        let mut len = 4;
        let mut deadline = PartialFrameDeadline::new(PARTIAL_FRAME_TIMEOUT_MS);
        deadline.note_bytes(1_000, len);

        assert!(!clear_expired_partial(&mut len, &mut deadline, 1_249));

        buf[len..frame_len].copy_from_slice(&frame[len..frame_len]);
        len = frame_len;
        deadline.note_bytes(1_249, len);

        assert!(!clear_expired_partial(&mut len, &mut deadline, 1_250));
        assert_eq!(
            next_frame_action(&buf[..len]),
            Some(FrameAction::Process(frame_len))
        );

        shift_left(&mut buf, &mut len, frame_len);
        deadline.clear();
        assert_eq!(len, 0);
        assert!(!deadline.is_active());
    }

    #[test]
    fn noise_resynchronization_finishes_with_a_disarmed_deadline() {
        let mut frame = [0u8; MAX_FRAME_SIZE];
        let frame_len = encode_frame(1, 9, CommandType::Ping, &[], &mut frame).unwrap();
        let mut buf = [0u8; MAX_FRAME_SIZE];
        buf[..2].copy_from_slice(&[0x00, 0x11]);
        buf[2..2 + frame_len].copy_from_slice(&frame[..frame_len]);
        let mut len = frame_len + 2;
        let mut deadline = PartialFrameDeadline::new(PARTIAL_FRAME_TIMEOUT_MS);
        deadline.note_bytes(1_000, len);

        assert_eq!(
            next_frame_action(&buf[..len]),
            Some(FrameAction::DropPrefix(2))
        );
        shift_left(&mut buf, &mut len, 2);
        deadline.note_bytes(1_001, len);
        assert_eq!(
            next_frame_action(&buf[..len]),
            Some(FrameAction::Process(frame_len))
        );

        shift_left(&mut buf, &mut len, frame_len);
        deadline.note_bytes(1_002, len);
        assert_eq!(len, 0);
        assert!(!deadline.is_active());
    }

    #[test]
    fn oversized_rejection_with_empty_buffer_disarms_deadline() {
        let mut buf = [0u8; MAX_FRAME_SIZE];
        let oversized = [MAGIC[0], MAGIC[1], 1, 0, 0x12, 0x34, 0x01, 0x00, 0xF1];
        buf[..oversized.len()].copy_from_slice(&oversized);
        let mut len = oversized.len();
        let mut deadline = PartialFrameDeadline::new(PARTIAL_FRAME_TIMEOUT_MS);
        deadline.note_bytes(1_000, len);

        let Some(FrameAction::Reject { len: rejected, .. }) = next_frame_action(&buf[..len]) else {
            panic!("oversized frame was not rejected");
        };
        shift_left(&mut buf, &mut len, rejected);
        deadline.note_bytes(1_001, len);

        assert_eq!(len, 0);
        assert!(!deadline.is_active());
        assert!(!clear_expired_partial(&mut len, &mut deadline, 9_999));
    }

    #[test]
    fn rejected_prefix_preserves_deadline_for_coalesced_trailing_partial() {
        let mut valid = [0u8; MAX_FRAME_SIZE];
        let _valid_len = encode_frame(1, 10, CommandType::Ping, &[], &mut valid).unwrap();
        let oversized_prefix = [
            MAGIC[0], MAGIC[1], 1, 0, 0x12, 0x34, 0x01, 0x00, 0xF1, 0x00, 0x00,
        ];
        let mut buf = [0u8; MAX_FRAME_SIZE];
        buf[..FRAME_OVERHEAD].copy_from_slice(&oversized_prefix);
        buf[FRAME_OVERHEAD..FRAME_OVERHEAD + 4].copy_from_slice(&valid[..4]);
        let mut len = FRAME_OVERHEAD + 4;
        let mut deadline = PartialFrameDeadline::new(PARTIAL_FRAME_TIMEOUT_MS);
        deadline.note_bytes(1_000, len);

        let Some(FrameAction::Reject { len: rejected, .. }) = next_frame_action(&buf[..len]) else {
            panic!("oversized prefix was not rejected");
        };
        assert_eq!(rejected, FRAME_OVERHEAD);

        shift_left(&mut buf, &mut len, rejected);
        deadline.note_bytes(1_100, len);

        assert_eq!(len, 4);
        assert_eq!(next_frame_action(&buf[..len]), Some(FrameAction::NeedMore));
        assert!(deadline.is_active());
        assert_eq!(deadline.deadline_ms(), Some(1_350));
        assert!(!clear_expired_partial(&mut len, &mut deadline, 1_349));
        assert!(clear_expired_partial(&mut len, &mut deadline, 1_350));
        assert_eq!(len, 0);
        assert!(!deadline.is_active());
    }

    #[test]
    fn maximum_frame_tail_and_next_prefix_in_one_packet_are_preserved() {
        let mut first = [0u8; MAX_FRAME_SIZE];
        let first_len = encode_frame(
            2,
            100,
            CommandType::TypeAscii,
            &[b'a'; MAX_PAYLOAD_SIZE],
            &mut first,
        )
        .unwrap();
        assert_eq!(first_len, MAX_FRAME_SIZE);
        let mut second = [0u8; MAX_FRAME_SIZE];
        let second_len = encode_frame(2, 101, CommandType::Ping, &[], &mut second).unwrap();

        let mut stream = [0u8; MAX_FRAME_SIZE + FRAME_OVERHEAD];
        stream[..first_len].copy_from_slice(&first[..first_len]);
        stream[first_len..first_len + second_len].copy_from_slice(&second[..second_len]);

        let mut frame_buf = [0u8; MAX_FRAME_SIZE];
        let mut frame_len = 0usize;
        let mut observed = heapless::Vec::<u16, 2>::new();
        for packet in stream[..first_len + second_len].chunks(64) {
            let mut packet_offset = 0usize;
            while packet_offset < packet.len() {
                let copied =
                    append_packet_chunk(&mut frame_buf, &mut frame_len, packet, &mut packet_offset);
                assert!(copied > 0);

                loop {
                    match next_frame_action(&frame_buf[..frame_len]) {
                        Some(FrameAction::Process(len)) => {
                            let frame = crate::protocol::decode_frame(&frame_buf[..len]).unwrap();
                            observed.push(frame.sequence).unwrap();
                            shift_left(&mut frame_buf, &mut frame_len, len);
                        }
                        Some(FrameAction::NeedMore) | None => break,
                        other => panic!("valid coalesced stream was damaged: {other:?}"),
                    }
                }
            }
        }

        assert_eq!(observed.as_slice(), &[100, 101]);
        assert_eq!(frame_len, 0);
    }
}
