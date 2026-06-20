//! 固件协议、USB 和 HID 行为配置。

use crate::protocol::MAX_PAYLOAD_SIZE;

pub const PROTOCOL_VERSION: u8 = 1;
pub const USB_VENDOR_ID: u16 = 0xCAFE;
pub const USB_PRODUCT_ID: u16 = 0x2350;

pub const KEY_TAP_DELAY_MS: u64 = 8;
pub const MOUSE_CLICK_DELAY_MS: u64 = 20;

const CAP_KEYBOARD: u16 = 1 << 0;
const CAP_MOUSE: u16 = 1 << 1;
const CAP_ASCII: u16 = 1 << 2;
const CAP_BATCH: u16 = 1 << 3;
const CAP_RETRY_SAFE: u16 = 1 << 4;

pub fn info_payload() -> [u8; 4] {
    [
        PROTOCOL_VERSION,
        (MAX_PAYLOAD_SIZE >> 8) as u8,
        MAX_PAYLOAD_SIZE as u8,
        0x03,
    ]
}

pub fn capability_payload() -> [u8; 10] {
    let caps = CAP_KEYBOARD | CAP_MOUSE | CAP_ASCII | CAP_BATCH | CAP_RETRY_SAFE;
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

    #[test]
    fn info_payload_reports_protocol_limits() {
        assert_eq!(info_payload(), [1, 0, 240, 0x03]);
    }

    #[test]
    fn capability_payload_reports_supported_features_and_delays() {
        assert_eq!(
            capability_payload(),
            [1, 0, 240, 0, 0b0001_1111, 1, 1, 0, 8, 20]
        );
    }
}
