//! 固件协议、USB 和 HID 行为配置。

use crate::protocol::MAX_PAYLOAD_SIZE;
pub use crate::protocol::PROTOCOL_VERSION;

pub const USB_VENDOR_ID: u16 = 0xCAFE;
pub const USB_PRODUCT_ID: u16 = 0x2350;

pub const KEY_TAP_DELAY_MS: u64 = 8;
pub const MOUSE_CLICK_DELAY_MS: u64 = 20;

const CAP_KEYBOARD: u16 = 1 << 0;
const CAP_MOUSE: u16 = 1 << 1;
const CAP_ASCII: u16 = 1 << 2;
const CAP_BATCH: u16 = 1 << 3;
const CAP_RETRY_SAFE: u16 = 1 << 4;
const CAP_LEASE: u16 = 1 << 5;
const CAP_CANCELLATION: u16 = 1 << 6;

pub fn info_payload() -> [u8; 4] {
    [
        PROTOCOL_VERSION,
        (MAX_PAYLOAD_SIZE >> 8) as u8,
        MAX_PAYLOAD_SIZE as u8,
        0x03,
    ]
}

pub fn capability_payload(request_version: u8) -> [u8; 10] {
    let mut caps = CAP_KEYBOARD | CAP_MOUSE | CAP_ASCII | CAP_BATCH;
    if request_version == PROTOCOL_VERSION {
        caps |= CAP_RETRY_SAFE | CAP_LEASE | CAP_CANCELLATION;
    }
    [
        PROTOCOL_VERSION,
        (MAX_PAYLOAD_SIZE >> 8) as u8,
        MAX_PAYLOAD_SIZE as u8,
        (caps >> 8) as u8,
        caps as u8,
        1,
        1,
        0,
        KEY_TAP_DELAY_MS as u8,
        MOUSE_CLICK_DELAY_MS as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::LEGACY_PROTOCOL_VERSION;

    #[test]
    fn info_payload_reports_protocol_limits() {
        assert_eq!(info_payload(), [2, 0, 240, 0x03]);
    }

    #[test]
    fn capability_payload_gates_v2_features_by_request_version() {
        assert_eq!(
            capability_payload(LEGACY_PROTOCOL_VERSION),
            [2, 0, 240, 0, 0b0000_1111, 1, 1, 0, 8, 20]
        );
        assert_eq!(
            capability_payload(PROTOCOL_VERSION),
            [2, 0, 240, 0, 0b0111_1111, 1, 1, 0, 8, 20]
        );
    }
}
