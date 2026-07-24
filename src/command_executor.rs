//! Execution of decoded protocol frames as transactional HID state changes.

use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::Timer;

use crate::commands::decode_command;
use crate::error::ErrorCode;
use crate::execution_core::{ExecutionBackend, execute_command, execute_command_once};
use crate::hid_report::{send_keyboard_state, send_mouse_state};
use crate::input_state::{KeyboardState, MouseState};
use crate::protocol::Frame;
use crate::safety::CancellationObservation;
use crate::usb_device::{KeyboardWriter, MouseWriter, caps_lock_enabled};

pub use crate::execution_core::DeviceResponse;
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
    let command = decode_command(frame).map_err(ErrorCode::from)?;
    let mut backend = HidExecutionBackend {
        keyboard,
        mouse,
        cancel,
    };
    execute_command(
        command,
        frame.version,
        caps_lock_enabled(),
        &mut backend,
        state,
    )
    .await
}

/// Executes one batch member without recovery; the batch loop owns its single release attempt.
pub(crate) async fn execute_frame_in_batch(
    frame: &Frame<'_>,
    keyboard: &mut KeyboardWriter,
    mouse: &mut MouseWriter,
    state: &mut InputState,
    cancel: &mut CancelWait<'_>,
) -> Result<DeviceResponse, ErrorCode> {
    let command = decode_command(frame).map_err(ErrorCode::from)?;
    let mut backend = HidExecutionBackend {
        keyboard,
        mouse,
        cancel,
    };
    execute_command_once(
        command,
        frame.version,
        caps_lock_enabled(),
        &mut backend,
        state,
    )
    .await
}

struct HidExecutionBackend<'writers, 'cancel, 'signal> {
    keyboard: &'writers mut KeyboardWriter,
    mouse: &'writers mut MouseWriter,
    cancel: &'cancel mut CancelWait<'signal>,
}

impl ExecutionBackend for HidExecutionBackend<'_, '_, '_> {
    fn check_cancelled(&mut self) -> Result<(), ErrorCode> {
        self.cancel.check()
    }

    async fn delay(&mut self, milliseconds: u64) -> Result<(), ErrorCode> {
        self.cancel.delay(milliseconds).await
    }

    async fn send_keyboard(&mut self, state: KeyboardState) -> Result<(), ErrorCode> {
        send_keyboard_state(self.keyboard, &state).await
    }

    async fn send_mouse(
        &mut self,
        state: MouseState,
        x: i8,
        y: i8,
        wheel: i8,
    ) -> Result<(), ErrorCode> {
        send_mouse_state(self.mouse, state, x, y, wheel).await
    }
}
