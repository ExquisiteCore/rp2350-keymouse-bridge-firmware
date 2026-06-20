//! 标准 HID 键盘和鼠标 report 发送工具。

use embassy_time::Timer;
use usbd_hid::descriptor::{KeyboardReport, MouseReport};

use crate::commands::{KeyStroke, MouseButton};
use crate::error::ErrorCode;
use crate::firmware_config::{KEY_TAP_DELAY_MS, MOUSE_CLICK_DELAY_MS};
use crate::usb_device::{KeyboardWriter, MouseWriter};

pub async fn send_keystroke(
    writer: &mut KeyboardWriter,
    stroke: KeyStroke,
) -> Result<(), ErrorCode> {
    let report = KeyboardReport {
        modifier: stroke.modifier,
        reserved: 0,
        leds: 0,
        keycodes: [stroke.keycode, 0, 0, 0, 0, 0],
    };

    writer
        .write_serialize(&report)
        .await
        .map_err(|_| ErrorCode::HidWrite)
}

pub async fn release_keyboard(writer: &mut KeyboardWriter) -> Result<(), ErrorCode> {
    send_keystroke(
        writer,
        KeyStroke {
            modifier: 0,
            keycode: 0,
        },
    )
    .await
}

pub async fn tap_keystroke(
    writer: &mut KeyboardWriter,
    stroke: KeyStroke,
) -> Result<(), ErrorCode> {
    send_keystroke(writer, stroke).await?;
    Timer::after_millis(KEY_TAP_DELAY_MS).await;
    release_keyboard(writer).await
}

pub async fn move_mouse(
    writer: &mut MouseWriter,
    buttons: u8,
    dx: i16,
    dy: i16,
) -> Result<(), ErrorCode> {
    let mut remaining_x = dx;
    let mut remaining_y = dy;

    while remaining_x != 0 || remaining_y != 0 {
        let step_x = clamp_i16_to_i8(remaining_x);
        let step_y = clamp_i16_to_i8(remaining_y);
        send_mouse_report(writer, buttons, step_x, step_y, 0).await?;
        remaining_x -= step_x as i16;
        remaining_y -= step_y as i16;
    }

    Ok(())
}

pub async fn click_mouse(
    writer: &mut MouseWriter,
    buttons: &mut u8,
    button: MouseButton,
) -> Result<(), ErrorCode> {
    let mask = button.mask();
    *buttons |= mask;
    send_mouse_report(writer, *buttons, 0, 0, 0).await?;
    Timer::after_millis(MOUSE_CLICK_DELAY_MS).await;
    *buttons &= !mask;
    send_mouse_report(writer, *buttons, 0, 0, 0).await
}

pub async fn send_mouse_report(
    writer: &mut MouseWriter,
    buttons: u8,
    x: i8,
    y: i8,
    wheel: i8,
) -> Result<(), ErrorCode> {
    let report = MouseReport {
        buttons,
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

fn clamp_i16_to_i8(value: i16) -> i8 {
    value.clamp(-127, 127) as i8
}
