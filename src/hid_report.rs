//! Standard HID keyboard and mouse state report writers.

use usbd_hid::descriptor::{KeyboardReport, MouseReport};

use crate::error::ErrorCode;
use crate::input_state::{KeyboardState, MouseState};
use crate::usb_device::{KeyboardWriter, MouseWriter};

pub async fn send_keyboard_state(
    writer: &mut KeyboardWriter,
    state: &KeyboardState,
) -> Result<(), ErrorCode> {
    let mut keycodes = [0; 6];
    let active = state.keycodes();
    keycodes[..active.len()].copy_from_slice(active);
    let report = KeyboardReport {
        modifier: state.modifiers(),
        reserved: 0,
        leds: 0,
        keycodes,
    };

    writer
        .write_serialize(&report)
        .await
        .map_err(|_| ErrorCode::HidWrite)
}

pub async fn send_mouse_state(
    writer: &mut MouseWriter,
    state: MouseState,
    x: i8,
    y: i8,
    wheel: i8,
) -> Result<(), ErrorCode> {
    let report = MouseReport {
        buttons: state.buttons(),
        x,
        y,
        wheel,
        pan: 0,
    };

    writer
        .write_serialize(&report)
        .await
        .map_err(|_| ErrorCode::HidWrite)
}
