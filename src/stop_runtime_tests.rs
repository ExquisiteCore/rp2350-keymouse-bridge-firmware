use core::future::Future;
use core::pin::pin;
use core::task::{Context, Poll, Waker};

use std::vec::Vec as StdVec;

use crate::batch::BatchCollector;
use crate::commands::{Command, KeyStroke, MouseButton, decode_command};
use crate::coordinator::{
    Admission, BatchMode, CachedResponse, CompletionToken, Coordinator, OwnedRequest,
    OwnedRequestBody,
};
use crate::error::ErrorCode;
use crate::execution_core::{
    BatchExecutionBackend, DeviceResponse, ExecutionBackend, execute_batch, execute_command,
    execute_command_once, reset_inputs,
};
use crate::input_state::{InputState, KeyboardState, MouseState};
use crate::owned_command::OwnedCommand;
use crate::protocol::{CommandType, Frame, PROTOCOL_VERSION};
use crate::safety::CancellationObservation;
use crate::stop_core::{EmergencyAction, StopCore};

#[derive(Clone, Copy)]
struct StartedExecution {
    token: CompletionToken,
    baseline_generation: u32,
}

struct FakeEmbassyAdapter {
    coordinator: Coordinator,
    stop_core: StopCore,
    published_generation: u32,
    queued_emergencies: usize,
    responses: StdVec<CachedResponse>,
}

impl FakeEmbassyAdapter {
    fn new() -> Self {
        let stop_core = StopCore::new();
        Self {
            published_generation: stop_core.current_generation(),
            coordinator: Coordinator::new(),
            stop_core,
            queued_emergencies: 0,
            responses: StdVec::new(),
        }
    }

    fn start(&mut self, frame: &Frame<'_>) -> StartedExecution {
        let owned = OwnedRequest::from_frame(frame).unwrap();
        let Admission::Execute(request) = self.coordinator.admit_prepared(owned) else {
            panic!("ordinary command must start exclusive execution");
        };
        StartedExecution {
            token: request.completion_token().unwrap(),
            baseline_generation: self.stop_core.current_generation(),
        }
    }

    fn issue_stop(&mut self, sequence: u16) {
        let frame = frame(sequence, CommandType::StopAll, &[]);
        let owned = OwnedRequest::from_frame(&frame).unwrap();
        let Admission::Stop(request) = self.coordinator.admit_prepared(owned) else {
            panic!("STOP_ALL must bypass active execution as Admission::Stop");
        };
        let action = self.stop_core.handle_stop(request);
        self.apply_emergency_action(action);
    }

    fn apply_emergency_action(&mut self, action: EmergencyAction) {
        self.published_generation = action.cancel_generation();
        if action.enqueue_emergency() {
            self.queued_emergencies += 1;
        }
    }

    fn complete_cancelled(&mut self, token: CompletionToken) {
        let response = token.nack(ErrorCode::Cancelled);
        self.coordinator.complete(token, response.clone()).unwrap();
        self.responses.push(response);
    }

    fn complete_emergency_reset(&mut self) {
        assert_eq!(self.queued_emergencies, 1);
        self.queued_emergencies = 0;
        for token in self.stop_core.complete_emergency() {
            let response = token.ack();
            self.coordinator.complete(token, response.clone()).unwrap();
            self.responses.push(response);
        }
    }
}

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

#[derive(Clone, Copy)]
enum StopTrigger {
    Delay(usize),
    Report(usize),
}

struct FakeExecutionBackend<'a> {
    adapter: &'a mut FakeEmbassyAdapter,
    cancellation: CancellationObservation,
    stop_sequence: u16,
    trigger: StopTrigger,
    stop_issued: bool,
    delay_count: usize,
    report_count: usize,
    keyboard_attempts: usize,
    fail_keyboard_attempt: Option<usize>,
    reports: StdVec<Report>,
}

impl<'a> FakeExecutionBackend<'a> {
    fn new(
        adapter: &'a mut FakeEmbassyAdapter,
        baseline_generation: u32,
        stop_sequence: u16,
        trigger: StopTrigger,
    ) -> Self {
        Self {
            adapter,
            cancellation: CancellationObservation::new(baseline_generation),
            stop_sequence,
            trigger,
            stop_issued: false,
            delay_count: 0,
            report_count: 0,
            keyboard_attempts: 0,
            fail_keyboard_attempt: None,
            reports: StdVec::new(),
        }
    }

    fn maybe_issue_stop_after_delay(&mut self) {
        if !self.stop_issued
            && matches!(self.trigger, StopTrigger::Delay(n) if n == self.delay_count)
        {
            self.adapter.issue_stop(self.stop_sequence);
            self.stop_issued = true;
        }
    }

    fn record_report(&mut self, report: Report) {
        self.reports.push(report);
        self.report_count += 1;
        if !self.stop_issued
            && matches!(self.trigger, StopTrigger::Report(n) if n == self.report_count)
        {
            self.adapter.issue_stop(self.stop_sequence);
            self.stop_issued = true;
        }
    }
}

impl ExecutionBackend for FakeExecutionBackend<'_> {
    fn check_cancelled(&mut self) -> Result<(), ErrorCode> {
        if self.cancellation.observe(self.adapter.published_generation) {
            Err(ErrorCode::Cancelled)
        } else {
            Ok(())
        }
    }

    async fn delay(&mut self, _milliseconds: u64) -> Result<(), ErrorCode> {
        self.delay_count += 1;
        self.maybe_issue_stop_after_delay();
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

struct CommandScenario {
    adapter: FakeEmbassyAdapter,
    result: Result<DeviceResponse, ErrorCode>,
    state: InputState,
    delay_count: usize,
    reports: StdVec<Report>,
}

fn run_stopped_command(
    command_frame: Frame<'_>,
    stop_sequence: u16,
    trigger: StopTrigger,
    fail_keyboard_attempt: Option<usize>,
) -> CommandScenario {
    let mut adapter = FakeEmbassyAdapter::new();
    let started = adapter.start(&command_frame);
    let command = decode_command(&command_frame).unwrap();
    let mut state = InputState::new();
    let (result, delay_count, reports) = {
        let mut backend = FakeExecutionBackend::new(
            &mut adapter,
            started.baseline_generation,
            stop_sequence,
            trigger,
        );
        backend.fail_keyboard_attempt = fail_keyboard_attempt;
        let result = run_ready(execute_command(
            command,
            PROTOCOL_VERSION,
            false,
            &mut backend,
            &mut state,
        ));
        (result, backend.delay_count, backend.reports)
    };

    assert_eq!(result, Err(ErrorCode::Cancelled));
    assert!(state.is_idle());
    assert_eq!(adapter.queued_emergencies, 1);
    assert_eq!(adapter.stop_core.pending_stop_count(), 1);
    adapter.complete_cancelled(started.token);
    assert_eq!(
        adapter.responses,
        [CachedResponse::nack(
            PROTOCOL_VERSION,
            command_frame.sequence,
            ErrorCode::Cancelled,
        )]
    );

    CommandScenario {
        adapter,
        result,
        state,
        delay_count,
        reports,
    }
}

#[test]
fn wait_stop_cancels_in_progress_and_defers_stop_ack_until_emergency_completion() {
    let mut scenario = run_stopped_command(
        frame(710, CommandType::WaitMs, &5_000_u32.to_be_bytes()),
        711,
        StopTrigger::Delay(1),
        Some(1),
    );

    assert_eq!(scenario.result, Err(ErrorCode::Cancelled));
    assert!(scenario.state.is_idle());
    assert_eq!(scenario.delay_count, 1);
    assert_eq!(
        scenario.reports,
        [
            Report::Keyboard(KeyboardState::new()),
            Report::Mouse {
                state: MouseState::new(),
                x: 0,
                y: 0,
                wheel: 0,
            },
        ]
    );
    assert_eq!(scenario.adapter.responses.len(), 1);

    scenario.adapter.complete_emergency_reset();

    assert_eq!(
        scenario.adapter.responses,
        [
            CachedResponse::nack(PROTOCOL_VERSION, 710, ErrorCode::Cancelled),
            CachedResponse::ack(PROTOCOL_VERSION, 711),
        ]
    );
}

#[test]
fn type_ascii_stop_after_first_stroke_skips_later_strokes_and_releases() {
    let mut scenario = run_stopped_command(
        frame(720, CommandType::TypeAscii, b"ab"),
        721,
        StopTrigger::Report(2),
        None,
    );

    assert_eq!(
        scenario.reports,
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
    scenario.adapter.complete_emergency_reset();
    assert_eq!(
        scenario.adapter.responses.last(),
        Some(&CachedResponse::ack(PROTOCOL_VERSION, 721))
    );
}

#[test]
fn mouse_click_stop_after_press_skips_restore_and_releases() {
    let mut scenario = run_stopped_command(
        frame(730, CommandType::MouseClick, &[MouseButton::Left.mask()]),
        731,
        StopTrigger::Report(1),
        None,
    );

    assert_eq!(
        scenario.reports,
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
    scenario.adapter.complete_emergency_reset();
    assert_eq!(
        scenario.adapter.responses.last(),
        Some(&CachedResponse::ack(PROTOCOL_VERSION, 731))
    );
}

#[test]
fn split_mouse_move_stop_after_first_chunk_skips_remaining_chunks_and_releases() {
    let payload = [0x01, 0x2c, 0xfe, 0xd4];
    let mut scenario = run_stopped_command(
        frame(740, CommandType::MouseMoveRel, &payload),
        741,
        StopTrigger::Report(1),
        Some(1),
    );

    assert_eq!(
        scenario.reports,
        [
            Report::Mouse {
                state: MouseState::new(),
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
    scenario.adapter.complete_emergency_reset();
    assert_eq!(
        scenario.adapter.responses.last(),
        Some(&CachedResponse::ack(PROTOCOL_VERSION, 741))
    );
}

struct FakeBatchBackend<'a> {
    io: FakeExecutionBackend<'a>,
    state: InputState,
    calls: StdVec<OwnedCommand>,
    reset_count: usize,
}

impl BatchExecutionBackend for FakeBatchBackend<'_> {
    async fn execute(&mut self, command: &OwnedCommand) -> Result<DeviceResponse, ErrorCode> {
        self.calls.push(command.clone());
        execute_command_once(
            borrowed_command(command),
            PROTOCOL_VERSION,
            false,
            &mut self.io,
            &mut self.state,
        )
        .await
    }

    async fn reset_inputs(&mut self) {
        self.reset_count += 1;
        let _ = reset_inputs(&mut self.io, &mut self.state).await;
    }
}

#[test]
fn batch_stop_fails_end_once_skips_sentinels_and_defers_stop_ack() {
    let mut adapter = FakeEmbassyAdapter::new();
    let begin = frame(750, CommandType::BatchBegin, &[]);
    assert!(matches!(
        adapter
            .coordinator
            .admit_prepared(OwnedRequest::from_frame(&begin).unwrap()),
        Admission::Immediate(_)
    ));
    let wait_payload = 5_000_u32.to_be_bytes();
    let wheel_payload = [7_u8];
    let move_payload = [0, 1, 0, 2];
    let batch_frames = [
        frame(751, CommandType::WaitMs, &wait_payload),
        frame(752, CommandType::MouseWheel, &wheel_payload),
        frame(753, CommandType::MouseMoveRel, &move_payload),
    ];
    let mut collected = BatchCollector::begin(InputState::new());
    for command_frame in &batch_frames {
        let Admission::Collect(request) = adapter
            .coordinator
            .admit_prepared(OwnedRequest::from_frame(command_frame).unwrap())
        else {
            panic!("batch command must be collected before BATCH_END");
        };
        let token = request.completion_token().unwrap();
        let OwnedRequestBody::Command(command) = request.body() else {
            panic!("collected request must own a command");
        };
        collected.push(command.clone()).unwrap();
        adapter.coordinator.complete(token, token.ack()).unwrap();
    }
    assert_eq!(
        collected.commands(),
        [
            OwnedCommand::WaitMs(5_000),
            OwnedCommand::MouseWheel(7),
            OwnedCommand::MouseMoveRel { dx: 1, dy: 2 },
        ]
    );
    let end = frame(754, CommandType::BatchEnd, &[]);
    let Admission::Execute(end_request) = adapter
        .coordinator
        .admit_prepared(OwnedRequest::from_frame(&end).unwrap())
    else {
        panic!("BATCH_END must own exclusive execution");
    };
    let end_token = end_request.completion_token().unwrap();
    let baseline_generation = adapter.stop_core.current_generation();

    let (result, calls, reset_count, reports, state) = {
        let mut backend = FakeBatchBackend {
            io: FakeExecutionBackend::new(
                &mut adapter,
                baseline_generation,
                755,
                StopTrigger::Delay(1),
            ),
            state: InputState::new(),
            calls: StdVec::new(),
            reset_count: 0,
        };
        backend.io.fail_keyboard_attempt = Some(1);
        let result = run_ready(execute_batch(collected.commands(), &mut backend));
        (
            result,
            backend.calls,
            backend.reset_count,
            backend.io.reports,
            backend.state,
        )
    };

    assert_eq!(result, Err(ErrorCode::Cancelled));
    assert_eq!(calls, collected.commands()[..1]);
    assert_eq!(reset_count, 1);
    assert!(state.is_idle());
    assert_eq!(
        reports,
        [
            Report::Keyboard(KeyboardState::new()),
            Report::Mouse {
                state: MouseState::new(),
                x: 0,
                y: 0,
                wheel: 0,
            },
        ]
    );
    assert_eq!(adapter.coordinator.batch_mode(), BatchMode::Executing);
    adapter.complete_cancelled(end_token);
    assert_eq!(adapter.coordinator.batch_mode(), BatchMode::Idle);
    assert_eq!(
        adapter.responses,
        [CachedResponse::nack(
            PROTOCOL_VERSION,
            754,
            ErrorCode::Cancelled,
        )]
    );
    assert_eq!(
        adapter
            .coordinator
            .admit_prepared(OwnedRequest::from_frame(&end).unwrap()),
        Admission::Replay(CachedResponse::nack(
            PROTOCOL_VERSION,
            754,
            ErrorCode::Cancelled,
        ))
    );

    adapter.complete_emergency_reset();

    assert_eq!(
        adapter.responses,
        [
            CachedResponse::nack(PROTOCOL_VERSION, 754, ErrorCode::Cancelled),
            CachedResponse::ack(PROTOCOL_VERSION, 755),
        ]
    );
}

fn borrowed_command(command: &OwnedCommand) -> Command<'_> {
    match command {
        OwnedCommand::Ping => Command::Ping,
        OwnedCommand::GetInfo => Command::GetInfo,
        OwnedCommand::GetCaps => Command::GetCaps,
        OwnedCommand::Heartbeat => Command::Heartbeat,
        OwnedCommand::KeyDown(stroke) => Command::KeyDown(*stroke),
        OwnedCommand::KeyUp(stroke) => Command::KeyUp(*stroke),
        OwnedCommand::KeyTap(stroke) => Command::KeyTap(*stroke),
        OwnedCommand::TypeAscii(bytes) => Command::TypeAscii(bytes.as_slice()),
        OwnedCommand::MouseMoveRel { dx, dy } => Command::MouseMoveRel { dx: *dx, dy: *dy },
        OwnedCommand::MouseButtonDown(button) => Command::MouseButtonDown(*button),
        OwnedCommand::MouseButtonUp(button) => Command::MouseButtonUp(*button),
        OwnedCommand::MouseClick(button) => Command::MouseClick(*button),
        OwnedCommand::MouseWheel(wheel) => Command::MouseWheel(*wheel),
        OwnedCommand::WaitMs(wait_ms) => Command::WaitMs(*wait_ms),
        OwnedCommand::StopAll => Command::StopAll,
    }
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

fn frame(sequence: u16, command_type: CommandType, payload: &[u8]) -> Frame<'_> {
    Frame {
        version: PROTOCOL_VERSION,
        flags: 0,
        sequence,
        command_type,
        payload,
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
