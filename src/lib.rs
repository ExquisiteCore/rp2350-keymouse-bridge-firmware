#![cfg_attr(not(test), no_std)]

pub mod batch;
pub mod commands;
pub mod coordinator;
pub mod error;
pub mod execution_core;
pub mod firmware_config;
pub mod frame_stream;
pub mod input_state;
pub mod led;
pub mod owned_command;
pub mod protocol;
pub mod runtime_safety;
pub mod safety;
pub mod stop_core;
pub mod usb_identity;

pub use protocol::{
    FLAG_NO_RESPONSE, LEGACY_PROTOCOL_VERSION, MAX_WAIT_MS, PROTOCOL_VERSION, RequestError,
    RequestKind, encode_frame_with_flags, validate_request,
};

#[cfg(test)]
mod stop_runtime_tests;

#[cfg(test)]
mod usb_identity_tests {
    use crate::usb_identity::{
        USB_PRODUCT, USB_SERIAL_CAPACITY, caps_lock_from_led_report, format_usb_serial,
    };

    #[test]
    fn usb_identity_uses_exquisitecore_name() {
        assert_eq!(crate::usb_identity::USB_MANUFACTURER, "ExquisiteCore");
        assert_eq!(USB_PRODUCT, "ExquisiteCore KeyMouse Bridge");
    }

    #[test]
    fn formats_chip_id_as_stable_usb_serial() {
        let mut out = [0u8; USB_SERIAL_CAPACITY];
        let serial = format_usb_serial(0x0123_4567_89AB_CDEF, &mut out).unwrap();
        assert_eq!(serial, "EXQC-KMOUSE-0123456789ABCDEF");
    }

    #[test]
    fn caps_lock_output_bit_is_detected() {
        assert!(!caps_lock_from_led_report(0b0000_0001));
        assert!(caps_lock_from_led_report(0b0000_0010));
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

#[cfg(test)]
mod stop_runtime_glue_tests {
    use crate::coordinator::{Admission, Coordinator, OwnedRequest};
    use crate::protocol::{CommandType, Frame};
    use crate::stop_core::StopCore;

    #[test]
    fn stop_frame_admission_drives_the_production_cancellation_generation() {
        let mut coordinator = Coordinator::new();
        let mut stop_core = StopCore::new();
        let baseline_generation = stop_core.current_generation();
        let frame = Frame {
            version: 2,
            flags: 0,
            sequence: 700,
            command_type: CommandType::StopAll,
            payload: &[],
        };
        let owned = OwnedRequest::from_frame(&frame).unwrap();
        let Admission::Stop(request) = coordinator.admit_prepared(owned) else {
            panic!("STOP_ALL must use urgent stop admission");
        };

        let action = stop_core.handle_stop(request);

        assert_ne!(action.cancel_generation(), baseline_generation);
        assert!(action.enqueue_emergency());
        assert!(stop_core.emergency_in_flight());
        assert_eq!(stop_core.pending_stop_count(), 1);
    }

    #[test]
    fn non_stop_safety_emergencies_coalesce_and_session_reset_discards_stop_tokens() {
        let mut stop_core = StopCore::new();
        let first = stop_core.schedule_emergency();
        let second = stop_core.schedule_emergency();

        assert!(first.enqueue_emergency());
        assert!(!second.enqueue_emergency());
        assert_ne!(first.cancel_generation(), second.cancel_generation());
        assert!(stop_core.complete_emergency().is_empty());
        assert!(!stop_core.emergency_in_flight());

        let mut coordinator = Coordinator::new();
        let frame = Frame {
            version: 2,
            flags: 0,
            sequence: 701,
            command_type: CommandType::StopAll,
            payload: &[],
        };
        let Admission::Stop(request) =
            coordinator.admit_prepared(OwnedRequest::from_frame(&frame).unwrap())
        else {
            panic!("STOP_ALL must use urgent stop admission");
        };
        let _ = stop_core.handle_stop(request);
        stop_core.clear_pending_stops();

        assert!(stop_core.complete_emergency().is_empty());
    }
}

#[cfg(test)]
mod execution_core_tests {
    use core::future::Future;
    use core::pin::pin;
    use core::task::{Context, Poll, Waker};

    use std::vec::Vec;

    use crate::commands::{Command, KeyStroke, MouseButton};
    use crate::error::ErrorCode;
    use crate::execution_core::{
        BatchExecutionBackend, DeviceResponse, ExecutionBackend, execute_batch, execute_command,
    };
    use crate::input_state::{InputState, KeyboardState, MouseState};
    use crate::owned_command::OwnedCommand;
    use crate::safety::CancellationGeneration;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Report {
        Keyboard(KeyboardState),
        Mouse {
            state: MouseState,
            x: i8,
            y: i8,
            wheel: i8,
        },
    }

    struct FakeExecutionBackend {
        generation: CancellationGeneration,
        baseline_generation: u32,
        cancel_after_report: usize,
        report_count: usize,
        keyboard_attempts: usize,
        fail_keyboard_attempt: Option<usize>,
        reports: Vec<Report>,
    }

    impl FakeExecutionBackend {
        fn cancelling_after_report(report: usize) -> Self {
            let generation = CancellationGeneration::new();
            Self {
                baseline_generation: generation.current(),
                generation,
                cancel_after_report: report,
                report_count: 0,
                keyboard_attempts: 0,
                fail_keyboard_attempt: None,
                reports: Vec::new(),
            }
        }

        fn record_report(&mut self, report: Report) {
            self.reports.push(report);
            self.report_count += 1;
            if self.report_count == self.cancel_after_report {
                self.generation.cancel();
            }
        }
    }

    impl ExecutionBackend for FakeExecutionBackend {
        fn check_cancelled(&mut self) -> Result<(), ErrorCode> {
            if self.generation.changed_since(self.baseline_generation) {
                Err(ErrorCode::Cancelled)
            } else {
                Ok(())
            }
        }

        async fn delay(&mut self, _milliseconds: u64) -> Result<(), ErrorCode> {
            self.check_cancelled()
        }

        async fn send_keyboard(&mut self, state: KeyboardState) -> Result<(), ErrorCode> {
            self.keyboard_attempts += 1;
            self.record_report(Report::Keyboard(state));
            if self.fail_keyboard_attempt == Some(self.keyboard_attempts) {
                Err(ErrorCode::HidWrite)
            } else {
                Ok(())
            }
        }

        async fn send_mouse(
            &mut self,
            state: MouseState,
            x: i8,
            y: i8,
            wheel: i8,
        ) -> Result<(), ErrorCode> {
            self.record_report(Report::Mouse { state, x, y, wheel });
            Ok(())
        }
    }

    fn run_ready<F: Future>(future: F) -> F::Output {
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        let mut future = pin!(future);
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("host fake must not sleep or pend"),
        }
    }

    fn assert_direct_batch_marker_is_rejected_without_hid_effect(command: Command<'_>) {
        let mut backend = FakeExecutionBackend::cancelling_after_report(usize::MAX);
        let mut state = InputState::new();
        state
            .keyboard
            .key_down(KeyStroke {
                modifier: 0x02,
                keycode: 0x04,
            })
            .unwrap();
        state.mouse.button_down(MouseButton::Right);
        let state_before = state;

        let result = run_ready(execute_command(command, 2, false, &mut backend, &mut state));

        assert_eq!(result, Err(ErrorCode::BadCommand));
        assert!(backend.reports.is_empty());
        assert_eq!(state, state_before);
    }

    #[test]
    fn direct_batch_begin_marker_is_rejected_without_hid_effect() {
        assert_direct_batch_marker_is_rejected_without_hid_effect(Command::BatchBegin);
    }

    #[test]
    fn direct_batch_end_marker_is_rejected_without_hid_effect() {
        assert_direct_batch_marker_is_rejected_without_hid_effect(Command::BatchEnd);
    }

    #[test]
    fn type_ascii_generation_change_between_strokes_stops_next_stroke_and_resets() {
        let mut backend = FakeExecutionBackend::cancelling_after_report(2);
        let mut state = InputState::new();

        let result = run_ready(execute_command(
            Command::TypeAscii(b"ab"),
            2,
            false,
            &mut backend,
            &mut state,
        ));

        assert!(matches!(result, Err(ErrorCode::Cancelled)));
        assert!(state.is_idle());
        assert_eq!(
            backend.reports,
            [
                Report::Keyboard(keyboard_with_key(0x04)),
                Report::Keyboard(KeyboardState::new()),
                Report::Keyboard(KeyboardState::new()),
                Report::Mouse {
                    state: MouseState::new(),
                    x: 0,
                    y: 0,
                    wheel: 0,
                },
            ]
        );
    }

    #[test]
    fn mouse_click_generation_change_after_press_skips_restore_and_resets() {
        let mut backend = FakeExecutionBackend::cancelling_after_report(1);
        let mut state = InputState::new();

        let result = run_ready(execute_command(
            Command::MouseClick(MouseButton::Left),
            2,
            false,
            &mut backend,
            &mut state,
        ));

        assert!(matches!(result, Err(ErrorCode::Cancelled)));
        assert!(state.is_idle());
        assert_eq!(
            backend.reports,
            [
                Report::Mouse {
                    state: MouseState::from_buttons(MouseButton::Left.mask()),
                    x: 0,
                    y: 0,
                    wheel: 0,
                },
                Report::Keyboard(KeyboardState::new()),
                Report::Mouse {
                    state: MouseState::new(),
                    x: 0,
                    y: 0,
                    wheel: 0,
                },
            ]
        );
    }

    #[test]
    fn split_mouse_move_cancellation_stops_chunks_and_attempts_both_resets() {
        let mut backend = FakeExecutionBackend::cancelling_after_report(1);
        backend.fail_keyboard_attempt = Some(1);
        let mut state = InputState::new();
        state
            .keyboard
            .key_down(KeyStroke {
                modifier: 0,
                keycode: 0x1a,
            })
            .unwrap();
        state.mouse.button_down(MouseButton::Right);

        let result = run_ready(execute_command(
            Command::MouseMoveRel { dx: 300, dy: -300 },
            2,
            false,
            &mut backend,
            &mut state,
        ));

        assert!(matches!(result, Err(ErrorCode::Cancelled)));
        assert!(state.is_idle());
        assert_eq!(
            backend.reports,
            [
                Report::Mouse {
                    state: MouseState::from_buttons(MouseButton::Right.mask()),
                    x: 127,
                    y: -127,
                    wheel: 0,
                },
                Report::Keyboard(KeyboardState::new()),
                Report::Mouse {
                    state: MouseState::new(),
                    x: 0,
                    y: 0,
                    wheel: 0,
                },
            ]
        );
    }

    fn keyboard_with_key(keycode: u8) -> KeyboardState {
        let mut state = KeyboardState::new();
        state
            .key_down(KeyStroke {
                modifier: 0,
                keycode,
            })
            .unwrap();
        state
    }

    struct FakeBatchBackend {
        fail_at: usize,
        calls: Vec<OwnedCommand>,
        resets: usize,
    }

    impl BatchExecutionBackend for FakeBatchBackend {
        async fn execute(&mut self, command: &OwnedCommand) -> Result<DeviceResponse, ErrorCode> {
            self.calls.push(command.clone());
            if self.calls.len() == self.fail_at {
                Err(ErrorCode::KeyboardBusy)
            } else {
                Ok(DeviceResponse::Ack)
            }
        }

        async fn reset_inputs(&mut self) {
            self.resets += 1;
        }
    }

    #[test]
    fn batch_error_stops_before_sentinel_preserves_error_and_resets_once() {
        let commands = [
            OwnedCommand::WaitMs(1),
            OwnedCommand::MouseMoveRel { dx: 1, dy: 2 },
            OwnedCommand::MouseWheel(7),
        ];
        let mut backend = FakeBatchBackend {
            fail_at: 2,
            calls: Vec::new(),
            resets: 0,
        };

        let result = run_ready(execute_batch(&commands, &mut backend));

        assert!(matches!(result, Err(ErrorCode::KeyboardBusy)));
        assert_eq!(backend.calls, commands[..2]);
        assert_eq!(backend.resets, 1);
    }
}
