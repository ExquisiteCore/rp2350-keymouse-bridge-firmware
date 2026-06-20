//! 已解码协议帧到 HID 行为的执行层。

use embassy_time::Timer;

use crate::commands::{Command, ascii_to_keystroke, decode_command};
use crate::error::ErrorCode;
use crate::firmware_config::{capability_payload, info_payload};
use crate::hid_report::{
    click_mouse, move_mouse, release_keyboard, send_keystroke, send_mouse_report, tap_keystroke,
};
use crate::protocol::Frame;
use crate::usb_device::{KeyboardWriter, MouseWriter};

pub enum DeviceResponse {
    Ack,
    Info([u8; 4]),
    Caps([u8; 10]),
}

#[derive(Default)]
pub struct InputState {
    pub mouse_buttons: u8,
}

impl InputState {
    pub const fn new() -> Self {
        Self { mouse_buttons: 0 }
    }
}

pub async fn reset_inputs(
    keyboard: &mut KeyboardWriter,
    mouse: &mut MouseWriter,
    state: &mut InputState,
) -> Result<(), ErrorCode> {
    state.mouse_buttons = 0;
    release_keyboard(keyboard).await?;
    send_mouse_report(mouse, 0, 0, 0, 0).await
}

pub async fn execute_frame(
    frame: &Frame<'_>,
    keyboard: &mut KeyboardWriter,
    mouse: &mut MouseWriter,
    state: &mut InputState,
) -> Result<DeviceResponse, ErrorCode> {
    match decode_command(frame).map_err(|_| ErrorCode::BadCommand)? {
        Command::Ping => Ok(DeviceResponse::Ack),
        Command::GetInfo => Ok(DeviceResponse::Info(info_payload())),
        Command::GetCaps => Ok(DeviceResponse::Caps(capability_payload())),
        Command::KeyDown(stroke) => {
            send_keystroke(keyboard, stroke).await?;
            Ok(DeviceResponse::Ack)
        }
        Command::KeyUp(_) => {
            release_keyboard(keyboard).await?;
            Ok(DeviceResponse::Ack)
        }
        Command::KeyTap(stroke) => {
            tap_keystroke(keyboard, stroke).await?;
            Ok(DeviceResponse::Ack)
        }
        Command::TypeAscii(bytes) => {
            for byte in bytes {
                let stroke = ascii_to_keystroke(*byte).ok_or(ErrorCode::UnsupportedAscii)?;
                tap_keystroke(keyboard, stroke).await?;
            }
            Ok(DeviceResponse::Ack)
        }
        Command::MouseMoveRel { dx, dy } => {
            move_mouse(mouse, state.mouse_buttons, dx, dy).await?;
            Ok(DeviceResponse::Ack)
        }
        Command::MouseButtonDown(button) => {
            state.mouse_buttons |= button.mask();
            send_mouse_report(mouse, state.mouse_buttons, 0, 0, 0).await?;
            Ok(DeviceResponse::Ack)
        }
        Command::MouseButtonUp(button) => {
            state.mouse_buttons &= !button.mask();
            send_mouse_report(mouse, state.mouse_buttons, 0, 0, 0).await?;
            Ok(DeviceResponse::Ack)
        }
        Command::MouseClick(button) => {
            click_mouse(mouse, &mut state.mouse_buttons, button).await?;
            Ok(DeviceResponse::Ack)
        }
        Command::MouseWheel(wheel) => {
            send_mouse_report(mouse, state.mouse_buttons, 0, 0, wheel).await?;
            Ok(DeviceResponse::Ack)
        }
        Command::WaitMs(ms) => {
            Timer::after_millis(ms as u64).await;
            Ok(DeviceResponse::Ack)
        }
        Command::BatchBegin | Command::BatchEnd => Ok(DeviceResponse::Ack),
        Command::StopAll => {
            reset_inputs(keyboard, mouse, state).await?;
            Ok(DeviceResponse::Ack)
        }
    }
}
