//! Target-independent HID command and batch execution control flow.

use heapless::Vec;

use crate::commands::Command;
use crate::error::ErrorCode;
use crate::firmware_config::{
    KEY_TAP_DELAY_MS, MOUSE_CLICK_DELAY_MS, capability_payload, info_payload,
};
use crate::input_state::{
    InputState, KeyboardPulse, KeyboardState, MAX_ASCII_STROKES, MousePulse, MouseState,
    RelativeMovementSteps, ascii_strokes,
};
use crate::owned_command::OwnedCommand;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceResponse {
    Ack,
    Info([u8; 4]),
    Caps([u8; 10]),
}

/// Replaceable boundary around HID writes, delays, and cancellation observations.
#[allow(async_fn_in_trait)]
pub trait ExecutionBackend {
    fn check_cancelled(&mut self) -> Result<(), ErrorCode>;

    async fn delay(&mut self, milliseconds: u64) -> Result<(), ErrorCode>;

    async fn send_keyboard(&mut self, state: KeyboardState) -> Result<(), ErrorCode>;

    async fn send_mouse(
        &mut self,
        state: MouseState,
        x: i8,
        y: i8,
        wheel: i8,
    ) -> Result<(), ErrorCode>;
}

/// Executes one decoded command and best-effort releases interrupted HID state.
pub async fn execute_command<B: ExecutionBackend>(
    command: Command<'_>,
    version: u8,
    caps_lock: bool,
    backend: &mut B,
    state: &mut InputState,
) -> Result<DeviceResponse, ErrorCode> {
    match execute_command_once(command, version, caps_lock, backend, state).await {
        Err(error) if error.requires_best_effort_release() => {
            let _ = reset_inputs(backend, state).await;
            Err(error)
        }
        result => result,
    }
}

/// Executes one command without recovery so a surrounding batch can release exactly once.
pub(crate) async fn execute_command_once<B: ExecutionBackend>(
    command: Command<'_>,
    version: u8,
    caps_lock: bool,
    backend: &mut B,
    state: &mut InputState,
) -> Result<DeviceResponse, ErrorCode> {
    backend.check_cancelled()?;

    let response = match command {
        Command::Ping | Command::Heartbeat => DeviceResponse::Ack,
        Command::GetInfo => DeviceResponse::Info(info_payload()),
        Command::GetCaps => DeviceResponse::Caps(capability_payload(version)),
        Command::KeyDown(stroke) => {
            let mut next = state.keyboard;
            next.key_down(stroke).map_err(ErrorCode::from)?;
            commit_keyboard_state(backend, state, next).await?;
            DeviceResponse::Ack
        }
        Command::KeyUp(stroke) => {
            let mut next = state.keyboard;
            next.key_up(stroke);
            commit_keyboard_state(backend, state, next).await?;
            DeviceResponse::Ack
        }
        Command::KeyTap(stroke) => {
            let pulse = state.keyboard.tap_plan(stroke).map_err(ErrorCode::from)?;
            execute_keyboard_pulse(backend, state, pulse).await?;
            DeviceResponse::Ack
        }
        Command::TypeAscii(bytes) => {
            if !state.keyboard.is_idle() {
                return Err(ErrorCode::KeyboardBusy);
            }

            let mut strokes = Vec::<_, MAX_ASCII_STROKES>::new();
            ascii_strokes(bytes, caps_lock, &mut strokes).map_err(ErrorCode::from)?;
            backend.check_cancelled()?;

            for stroke in strokes.iter().copied() {
                backend.check_cancelled()?;
                let pulse = state.keyboard.tap_plan(stroke).map_err(ErrorCode::from)?;
                execute_keyboard_pulse(backend, state, pulse).await?;
            }
            DeviceResponse::Ack
        }
        Command::MouseMoveRel { dx, dy } => {
            for (x, y) in RelativeMovementSteps::new(dx, dy) {
                backend.check_cancelled()?;
                backend.send_mouse(state.mouse, x, y, 0).await?;
                backend.check_cancelled()?;
            }
            DeviceResponse::Ack
        }
        Command::MouseButtonDown(button) => {
            let mut next = state.mouse;
            next.button_down(button);
            commit_mouse_state(backend, state, next).await?;
            DeviceResponse::Ack
        }
        Command::MouseButtonUp(button) => {
            let mut next = state.mouse;
            next.button_up(button);
            commit_mouse_state(backend, state, next).await?;
            DeviceResponse::Ack
        }
        Command::MouseClick(button) => {
            let pulse = state.mouse.click_plan(button);
            execute_mouse_pulse(backend, state, pulse).await?;
            DeviceResponse::Ack
        }
        Command::MouseWheel(wheel) => {
            backend.send_mouse(state.mouse, 0, 0, wheel).await?;
            DeviceResponse::Ack
        }
        Command::WaitMs(milliseconds) => {
            backend.delay(u64::from(milliseconds)).await?;
            DeviceResponse::Ack
        }
        Command::BatchBegin | Command::BatchEnd => DeviceResponse::Ack,
        Command::StopAll => {
            reset_inputs(backend, state).await?;
            DeviceResponse::Ack
        }
    };

    backend.check_cancelled()?;
    Ok(response)
}

pub(crate) async fn reset_inputs<B: ExecutionBackend>(
    backend: &mut B,
    state: &mut InputState,
) -> Result<(), ErrorCode> {
    state.clear();
    let keyboard_result = backend.send_keyboard(state.keyboard).await;
    let mouse_result = backend.send_mouse(state.mouse, 0, 0, 0).await;

    match (keyboard_result, mouse_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}

async fn commit_keyboard_state<B: ExecutionBackend>(
    backend: &mut B,
    state: &mut InputState,
    next: KeyboardState,
) -> Result<(), ErrorCode> {
    backend.send_keyboard(next).await?;
    state.keyboard = next;
    Ok(())
}

async fn commit_mouse_state<B: ExecutionBackend>(
    backend: &mut B,
    state: &mut InputState,
    next: MouseState,
) -> Result<(), ErrorCode> {
    backend.send_mouse(next, 0, 0, 0).await?;
    state.mouse = next;
    Ok(())
}

async fn execute_keyboard_pulse<B: ExecutionBackend>(
    backend: &mut B,
    state: &mut InputState,
    pulse: KeyboardPulse,
) -> Result<(), ErrorCode> {
    if let Some(released) = pulse.released().copied() {
        commit_keyboard_state(backend, state, released).await?;
        backend.delay(KEY_TAP_DELAY_MS).await?;
    }

    commit_keyboard_state(backend, state, *pulse.pressed()).await?;
    backend.delay(KEY_TAP_DELAY_MS).await?;
    commit_keyboard_state(backend, state, *pulse.restore()).await
}

async fn execute_mouse_pulse<B: ExecutionBackend>(
    backend: &mut B,
    state: &mut InputState,
    pulse: MousePulse,
) -> Result<(), ErrorCode> {
    if let Some(released) = pulse.released() {
        commit_mouse_state(backend, state, released).await?;
        backend.delay(MOUSE_CLICK_DELAY_MS).await?;
    }

    commit_mouse_state(backend, state, pulse.pressed()).await?;
    backend.delay(MOUSE_CLICK_DELAY_MS).await?;
    commit_mouse_state(backend, state, pulse.restore()).await
}

/// Production batch-loop boundary used by the runtime and deterministic host tests.
#[allow(async_fn_in_trait)]
pub trait BatchExecutionBackend {
    async fn execute(&mut self, command: &OwnedCommand) -> Result<DeviceResponse, ErrorCode>;

    async fn reset_inputs(&mut self);
}

pub async fn execute_batch<B: BatchExecutionBackend>(
    commands: &[OwnedCommand],
    backend: &mut B,
) -> Result<DeviceResponse, ErrorCode> {
    let mut response = DeviceResponse::Ack;
    for command in commands {
        match backend.execute(command).await {
            Ok(next_response) => response = next_response,
            Err(error) => {
                backend.reset_inputs().await;
                return Err(error);
            }
        }
    }
    Ok(response)
}
