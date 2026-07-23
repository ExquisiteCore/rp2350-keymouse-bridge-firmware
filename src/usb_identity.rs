//! USB 设备身份字符串。

pub const USB_MANUFACTURER: &str = "ExquisiteCore";
pub const USB_PRODUCT: &str = "ExquisiteCore KeyMouse Bridge";
pub const USB_SERIAL_CAPACITY: usize = 30;

const USB_SERIAL_PREFIX: &[u8] = b"EXQC-KMOUSE-";
const USB_SERIAL_HEX_DIGITS: usize = 16;
const USB_SERIAL_LENGTH: usize = USB_SERIAL_PREFIX.len() + USB_SERIAL_HEX_DIGITS;
const UPPER_HEX: &[u8; 16] = b"0123456789ABCDEF";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbIdentityError {
    InvalidEncoding,
}

pub fn format_usb_serial(
    chip_id: u64,
    out: &mut [u8; USB_SERIAL_CAPACITY],
) -> Result<&str, UsbIdentityError> {
    out[..USB_SERIAL_PREFIX.len()].copy_from_slice(USB_SERIAL_PREFIX);
    for digit in 0..USB_SERIAL_HEX_DIGITS {
        let shift = (USB_SERIAL_HEX_DIGITS - digit - 1) * 4;
        let nibble = ((chip_id >> shift) & 0x0F) as usize;
        out[USB_SERIAL_PREFIX.len() + digit] = UPPER_HEX[nibble];
    }

    core::str::from_utf8(&out[..USB_SERIAL_LENGTH]).map_err(|_| UsbIdentityError::InvalidEncoding)
}

pub const fn caps_lock_from_led_report(leds: u8) -> bool {
    (leds & 0x02) != 0
}
