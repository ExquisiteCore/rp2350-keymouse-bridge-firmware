//! Execution of decoded protocol frames as transactional HID state changes.

use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::Timer;
use heapless::Vec;

use crate::commands::{Command, decode_command};
use crate::error::ErrorCode;
use crate::firmware_config::{
    KEY_TAP_DELAY_MS, MOUSE_CLICK_DELAY_MS, capability_payload, info_payload,
};
use crate::hid_report::{send_keyboard_state, send_mouse_state};
use crate::input_state::{
    KeyboardPulse, KeyboardState, MAX_ASCII_STROKES, MousePulse, MouseState, RelativeMovementSteps,
    ascii_strokes,
};
use crate::protocol::Frame;
use crate::safety::CancellationObservation;
use crate::usb_device::{KeyboardWriter, MouseWriter, caps_lock_enabled};

pub use crate::input_state::InputState;

pub type CancelSignal = Signal<CriticalSectionRawMutex, u32>;

/// Per-command view of the shared cancellation generation.
///
/// A changed generation is sticky for the command lifetime. Values equal to
/// the baseline are stale publications and are consumed without cancelling.
pub struct CancelWait<'a> {
    signal: Option<&'a CancelSignal>,
    observation: CancellationObservation,
}

impl<'a> CancelWait<'a> {
    /// Creates the Signal-backed adapter used by the concurrent Task 7 runtime.
    #[allow(dead_code)]
    pub const fn new(signal: &'a CancelSignal, baseline_generation: u32) -> Self {
        Self {
            signal: Some(signal),
            observation: CancellationObservation::new(baseline_generation),
        }
    }

    /// Temporary mode for the legacy serial loop, which cannot preempt work.
    const fn disabled() -> Self {
        Self {
            signal: None,
            observation: CancellationObservation::new(0),
        }
    }

    fn observe(&mut self, generation: u32) -> Result<(), ErrorCode> {
        if self.observation.observe(generation) {
            Err(ErrorCode::Cancelled)
        } else {
            Ok(())
        }
    }

    /// Nonblocking cancellation point that consumes the latest publication.
    pub fn check(&mut self) -> Result<(), ErrorCode> {
        if self.observation.is_cancelled() {
            return Err(ErrorCode::Cancelled);
        }

        if let Some(generation) = self.signal.and_then(Signal::try_take) {
            self.observe(generation)?;
        }

        Ok(())
    }

    /// Waits for the duration or a changed cancellation generation.
    pub async fn delay(&mut self, milliseconds: u64) -> Result<(), ErrorCode> {
        self.check()?;
        let Some(signal) = self.signal else {
            Timer::after_millis(milliseconds).await;
            return Ok(());
        };

        match select(
            Timer::after_millis(milliseconds),
            wait_for_generation_change(signal, self.observation.baseline()),
        )
        .await
        {
            Either::First(()) => self.check(),
            Either::Second(generation) => self.observe(generation),
        }
    }
}

async fn wait_for_generation_change(signal: &CancelSignal, baseline_generation: u32) -> u32 {
    loop {
        let generation = signal.wait().await;
        if generation != baseline_generation {
            return generation;
        }
    }
}

pub enum DeviceResponse {
    Ack,
    Info([u8; 4]),
    Caps([u8; 10]),
}

/// Clears logical state before independently attempting both zero reports.
pub async fn reset_inputs(
    keyboard: &mut KeyboardWriter,
    mouse: &mut MouseWriter,
    state: &mut InputState,
) -> Result<(), ErrorCode> {
    state.clear();
    let keyboard_result = send_keyboard_state(keyboard, &state.keyboard).await;
    let mouse_result = send_mouse_state(mouse, state.mouse, 0, 0, 0).await;

    match (keyboard_result, mouse_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}

/// Compatibility wrapper for the temporary single-task serial runtime.
pub async fn execute_frame(
    frame: &Frame<'_>,
    keyboard: &mut KeyboardWriter,
    mouse: &mut MouseWriter,
    state: &mut InputState,
) -> Result<DeviceResponse, ErrorCode> {
    let mut cancel = CancelWait::disabled();
    execute_frame_with_cancel(frame, keyboard, mouse, state, &mut cancel).await
}

/// Executes a frame with Signal-backed cancellation points for Task 7.
pub async fn execute_frame_with_cancel(
    frame: &Frame<'_>,
    keyboard: &mut KeyboardWriter,
    mouse: &mut MouseWriter,
    state: &mut InputState,
    cancel: &mut CancelWait<'_>,
) -> Result<DeviceResponse, ErrorCode> {
    match execute_frame_inner(frame, keyboard, mouse, state, cancel).await {
        Err(error) if error.requires_best_effort_release() => {
            let _ = reset_inputs(keyboard, mouse, state).await;
            Err(error)
        }
        result => result,
    }
}

async fn execute_frame_inner(
    frame: &Frame<'_>,
    keyboard: &mut KeyboardWriter,
    mouse: &mut MouseWriter,
    state: &mut InputState,
    cancel: &mut CancelWait<'_>,
) -> Result<DeviceResponse, ErrorCode> {
    let command = decode_command(frame).map_err(ErrorCode::from)?;
    cancel.check()?;

    let response = match command {
        Command::Ping | Command::Heartbeat => DeviceResponse::Ack,
        Command::GetInfo => DeviceResponse::Info(info_payload()),
        Command::GetCaps => DeviceResponse::Caps(capability_payload(frame.version)),
        Command::KeyDown(stroke) => {
            let mut next = state.keyboard;
            next.key_down(stroke).map_err(ErrorCode::from)?;
            commit_keyboard_state(keyboard, state, next).await?;
            DeviceResponse::Ack
        }
        Command::KeyUp(stroke) => {
            let mut next = state.keyboard;
            next.key_up(stroke);
            commit_keyboard_state(keyboard, state, next).await?;
            DeviceResponse::Ack
        }
        Command::KeyTap(stroke) => {
            let pulse = state.keyboard.tap_plan(stroke).map_err(ErrorCode::from)?;
            execute_keyboard_pulse(keyboard, state, pulse, cancel).await?;
            DeviceResponse::Ack
        }
        Command::TypeAscii(bytes) => {
            if !state.keyboard.is_idle() {
                return Err(ErrorCode::KeyboardBusy);
            }

            let caps_lock = caps_lock_enabled();
            let mut strokes = Vec::<_, MAX_ASCII_STROKES>::new();
            ascii_strokes(bytes, caps_lock, &mut strokes).map_err(ErrorCode::from)?;
            cancel.check()?;

            for stroke in strokes.iter().copied() {
                cancel.check()?;
                let pulse = state.keyboard.tap_plan(stroke).map_err(ErrorCode::from)?;
                execute_keyboard_pulse(keyboard, state, pulse, cancel).await?;
            }
            DeviceResponse::Ack
        }
        Command::MouseMoveRel { dx, dy } => {
            for (x, y) in RelativeMovementSteps::new(dx, dy) {
                cancel.check()?;
                send_mouse_state(mouse, state.mouse, x, y, 0).await?;
                cancel.check()?;
            }
            DeviceResponse::Ack
        }
        Command::MouseButtonDown(button) => {
            let mut next = state.mouse;
            next.button_down(button);
            commit_mouse_state(mouse, state, next).await?;
            DeviceResponse::Ack
        }
        Command::MouseButtonUp(button) => {
            let mut next = state.mouse;
            next.button_up(button);
            commit_mouse_state(mouse, state, next).await?;
            DeviceResponse::Ack
        }
        Command::MouseClick(button) => {
            let pulse = state.mouse.click_plan(button);
            execute_mouse_pulse(mouse, state, pulse, cancel).await?;
            DeviceResponse::Ack
        }
        Command::MouseWheel(wheel) => {
            send_mouse_state(mouse, state.mouse, 0, 0, wheel).await?;
            DeviceResponse::Ack
        }
        Command::WaitMs(milliseconds) => {
            cancel.delay(u64::from(milliseconds)).await?;
            DeviceResponse::Ack
        }
        Command::BatchBegin | Command::BatchEnd => DeviceResponse::Ack,
        Command::StopAll => {
            reset_inputs(keyboard, mouse, state).await?;
            DeviceResponse::Ack
        }
    };

    cancel.check()?;
    Ok(response)
}

async fn commit_keyboard_state(
    writer: &mut KeyboardWriter,
    state: &mut InputState,
    next: KeyboardState,
) -> Result<(), ErrorCode> {
    send_keyboard_state(writer, &next).await?;
    state.keyboard = next;
    Ok(())
}

async fn commit_mouse_state(
    writer: &mut MouseWriter,
    state: &mut InputState,
    next: MouseState,
) -> Result<(), ErrorCode> {
    send_mouse_state(writer, next, 0, 0, 0).await?;
    state.mouse = next;
    Ok(())
}

async fn execute_keyboard_pulse(
    writer: &mut KeyboardWriter,
    state: &mut InputState,
    pulse: KeyboardPulse,
    cancel: &mut CancelWait<'_>,
) -> Result<(), ErrorCode> {
    if let Some(released) = pulse.released().copied() {
        commit_keyboard_state(writer, state, released).await?;
        cancel.delay(KEY_TAP_DELAY_MS).await?;
    }

    commit_keyboard_state(writer, state, *pulse.pressed()).await?;
    cancel.delay(KEY_TAP_DELAY_MS).await?;
    commit_keyboard_state(writer, state, *pulse.restore()).await
}

async fn execute_mouse_pulse(
    writer: &mut MouseWriter,
    state: &mut InputState,
    pulse: MousePulse,
    cancel: &mut CancelWait<'_>,
) -> Result<(), ErrorCode> {
    if let Some(released) = pulse.released() {
        commit_mouse_state(writer, state, released).await?;
        cancel.delay(MOUSE_CLICK_DELAY_MS).await?;
    }

    commit_mouse_state(writer, state, pulse.pressed()).await?;
    cancel.delay(MOUSE_CLICK_DELAY_MS).await?;
    commit_mouse_state(writer, state, pulse.restore()).await
}
