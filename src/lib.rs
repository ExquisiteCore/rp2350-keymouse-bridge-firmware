#![cfg_attr(not(test), no_std)]

pub mod commands;
pub mod error;
pub mod firmware_config;
pub mod frame_stream;
pub mod input_state;
pub mod led;
pub mod protocol;
pub mod usb_identity;

pub use protocol::{
    FLAG_NO_RESPONSE, LEGACY_PROTOCOL_VERSION, MAX_WAIT_MS, PROTOCOL_VERSION, RequestError,
    RequestKind, encode_frame_with_flags, validate_request,
};

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

#[cfg(test)]
mod protocol_v2_exports_tests {
    use crate::{
        FLAG_NO_RESPONSE, LEGACY_PROTOCOL_VERSION, MAX_WAIT_MS, PROTOCOL_VERSION, RequestError,
        RequestKind, encode_frame_with_flags, validate_request,
    };

    #[test]
    fn protocol_v2_primitives_are_available_at_the_crate_root() {
        let _encode = encode_frame_with_flags;
        let _validate = validate_request;
        let _request_error = RequestError::UnsupportedVersion;
        let _request_kind = RequestKind::ResponseExpected;

        assert_eq!(LEGACY_PROTOCOL_VERSION, 1);
        assert_eq!(PROTOCOL_VERSION, 2);
        assert_eq!(FLAG_NO_RESPONSE, 0x01);
        assert_eq!(MAX_WAIT_MS, 60_000);
    }
}
