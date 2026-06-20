#![cfg_attr(not(test), no_std)]

pub mod commands;
pub mod error;
pub mod firmware_config;
pub mod frame_stream;
pub mod led;
pub mod protocol;
pub mod usb_identity;

#[cfg(test)]
mod usb_identity_tests {
    #[test]
    fn usb_identity_uses_exquisitecore_name() {
        assert_eq!(crate::usb_identity::USB_MANUFACTURER, "ExquisiteCore");
        assert_eq!(
            crate::usb_identity::USB_PRODUCT,
            "ExquisiteCore KeyMouse Bridge"
        );
        assert_eq!(crate::usb_identity::USB_SERIAL_NUMBER, "EXQC-KMOUSE-0001");
    }
}
