//! Concurrent Embassy runtime for CDC admission, cancellable HID execution, and safety release.

use core::sync::atomic::{AtomicBool, Ordering};

use embassy_futures::select::{Either, Either3, select, select3};
use embassy_time::Timer;
use embassy_usb::driver::EndpointError;

use crate::batch::{BatchCollector, BatchError};
use crate::command_executor::{
    CancelWait, DeviceResponse, InputState, execute_frame_with_cancel, reset_inputs,
};
use crate::coordinator::{
    Admission, CachedResponse, CompletionToken, Coordinator, OwnedRequest, OwnedRequestBody,
    SafetyEvent,
};
use crate::error::ErrorCode;
use crate::firmware_config::{capability_payload, info_payload};
use crate::frame_stream::{FrameAction, next_frame_action, sequence_from_partial, shift_left};
use crate::led::{LedMode, LedSignal};
use crate::owned_command::OwnedCommand;
use crate::protocol::{
    CommandType, Frame, LEGACY_PROTOCOL_VERSION, MAX_FRAME_SIZE, MAX_PAYLOAD_SIZE,
    PROTOCOL_VERSION, decode_frame,
};
use crate::response_writer::send_cached_response;
use crate::safety::{CONTROL_LEASE_MS, CancellationGeneration, PARTIAL_FRAME_TIMEOUT_MS};
use crate::static_resources::{
    CANCEL, JOBS, LEASE_REFRESH, REQUESTS, RESPONSES, RESULTS, SAFETY_EVENTS,
};
use crate::usb_device::{
    CdcControl, CdcReceiver, CdcSender, KeyboardReader, KeyboardWriter, MouseWriter,
};

const BUSY_RETRY_MS: u16 = 10;

static GUARDED_WORK: AtomicBool = AtomicBool::new(false);

/// One exclusive executor action. The single-slot channel prevents ordinary work queueing.
// The batch stays inline intentionally: firmware has no allocator and the channel has capacity one.
#[allow(clippy::large_enum_variant)]
pub enum ExecutionJob {
    Single {
        request: OwnedRequest,
        baseline_generation: u32,
    },
    Batch {
        request: OwnedRequest,
        batch: BatchCollector,
        baseline_generation: u32,
    },
    Emergency {
        stop: Option<CompletionToken>,
    },
}

/// Executor completion returned to the dispatcher for token-bound cache mutation.
pub enum ExecutionResult {
    Command {
        token: CompletionToken,
        response: CachedResponse,
        input_state: InputState,
    },
    Emergency {
        stop: Option<CompletionToken>,
        input_state: InputState,
    },
}

#[embassy_executor::task]
pub async fn cdc_receive_task(mut receiver: CdcReceiver) -> ! {
    let mut packet = [0u8; 64];
    let mut frame_buf = [0u8; MAX_FRAME_SIZE];

    loop {
        crate::set_led_mode(LedMode::Disconnected);
        receiver.wait_connection().await;
        crate::set_led_mode(LedMode::Connected);
        let mut frame_len = 0usize;

        loop {
            let read_result = if frame_len == 0 {
                receiver.read_packet(&mut packet).await
            } else {
                match select(
                    receiver.read_packet(&mut packet),
                    Timer::after_millis(PARTIAL_FRAME_TIMEOUT_MS),
                )
                .await
                {
                    Either::First(result) => result,
                    Either::Second(()) => {
                        frame_len = 0;
                        continue;
                    }
                }
            };

            let read_len = match read_result {
                Ok(read_len) => read_len,
                Err(EndpointError::Disabled) => {
                    crate::set_led_mode(LedMode::Disconnected);
                    SAFETY_EVENTS.send(SafetyEvent::UsbDisabled).await;
                    break;
                }
                Err(EndpointError::BufferOverflow) => {
                    crate::signal_led(LedSignal::Error);
                    frame_len = 0;
                    RESPONSES
                        .send(CachedResponse::nack(
                            PROTOCOL_VERSION,
                            0,
                            ErrorCode::Transport,
                        ))
                        .await;
                    continue;
                }
            };

            if read_len == 0 {
                continue;
            }
            if frame_len + read_len > frame_buf.len() {
                crate::signal_led(LedSignal::Error);
                let sequence = sequence_from_partial(&frame_buf[..frame_len]);
                let version = response_version(&frame_buf[..frame_len]);
                frame_len = 0;
                RESPONSES
                    .send(CachedResponse::nack(
                        version,
                        sequence,
                        ErrorCode::FrameTooLong,
                    ))
                    .await;
                continue;
            }

            frame_buf[frame_len..frame_len + read_len].copy_from_slice(&packet[..read_len]);
            frame_len += read_len;
            drain_frames(&mut frame_buf, &mut frame_len).await;
        }
    }
}

async fn drain_frames(frame_buf: &mut [u8; MAX_FRAME_SIZE], frame_len: &mut usize) {
    while let Some(action) = next_frame_action(&frame_buf[..*frame_len]) {
        match action {
            FrameAction::NeedMore => break,
            FrameAction::DropPrefix(count) => shift_left(frame_buf, frame_len, count),
            FrameAction::Reject {
                len,
                sequence,
                error,
            } => {
                crate::signal_led(LedSignal::Error);
                let version = response_version(&frame_buf[..*frame_len]);
                RESPONSES
                    .send(CachedResponse::nack(
                        version,
                        sequence,
                        ErrorCode::from_decode(error),
                    ))
                    .await;
                shift_left(frame_buf, frame_len, len);
            }
            FrameAction::Process(len) => {
                match decode_frame(&frame_buf[..len]) {
                    Ok(frame) => match OwnedRequest::from_frame(&frame) {
                        Ok(request) => {
                            REQUESTS.send(request).await;
                            crate::signal_led(LedSignal::Activity);
                        }
                        Err(error) => {
                            crate::signal_led(LedSignal::Error);
                            RESPONSES
                                .send(CachedResponse::nack(frame.version, frame.sequence, error))
                                .await;
                        }
                    },
                    Err(error) => {
                        crate::signal_led(LedSignal::Error);
                        let version = response_version(&frame_buf[..len]);
                        let sequence = sequence_from_partial(&frame_buf[..len]);
                        RESPONSES
                            .send(CachedResponse::nack(
                                version,
                                sequence,
                                ErrorCode::from_decode(error),
                            ))
                            .await;
                    }
                }
                shift_left(frame_buf, frame_len, len);
            }
        }
    }
}

#[embassy_executor::task]
pub async fn dispatcher_task() -> ! {
    let mut coordinator = Coordinator::new();
    let mut batch: Option<BatchCollector> = None;
    let mut input_state = InputState::new();
    let mut cancellation = CancellationGeneration::new();

    loop {
        match select3(
            SAFETY_EVENTS.receive(),
            RESULTS.receive(),
            REQUESTS.receive(),
        )
        .await
        {
            Either3::Third(request) => {
                let version = request.version();
                let sequence = request.sequence();
                let command_type = request.command_type();
                let admission = coordinator.admit_prepared(request);
                if version == PROTOCOL_VERSION
                    && !matches!(
                        &admission,
                        Admission::Reject(
                            ErrorCode::BadCommand
                                | ErrorCode::FrameTooLong
                                | ErrorCode::UnsupportedVersion
                                | ErrorCode::UnsupportedFlags
                                | ErrorCode::InvalidSequence
                                | ErrorCode::WaitTooLong
                        )
                    )
                {
                    LEASE_REFRESH.signal(());
                }

                match admission {
                    Admission::Execute(request) => {
                        let baseline_generation = cancellation.current();
                        if command_type == CommandType::BatchEnd {
                            let Some(collected) = batch.take() else {
                                let token = request
                                    .completion_token()
                                    .expect("batch end must carry a completion token");
                                let response = token.nack(ErrorCode::BatchState);
                                let _ = coordinator.complete(token, response.clone());
                                RESPONSES.send(response).await;
                                update_guarded(&coordinator, batch.as_ref(), input_state);
                                continue;
                            };
                            JOBS.send(ExecutionJob::Batch {
                                request,
                                batch: collected,
                                baseline_generation,
                            })
                            .await;
                        } else {
                            JOBS.send(ExecutionJob::Single {
                                request,
                                baseline_generation,
                            })
                            .await;
                        }
                    }
                    Admission::Collect(request) => {
                        let token = request
                            .completion_token()
                            .expect("collected request must carry a completion token");
                        let result = match request.body() {
                            OwnedRequestBody::Command(command) => batch
                                .as_mut()
                                .ok_or(BatchError::NotBatchable)
                                .and_then(|collector| collector.push(command.clone())),
                            _ => Err(BatchError::NotBatchable),
                        };
                        let response = match result {
                            Ok(()) => token.ack(),
                            Err(error) => {
                                batch = None;
                                coordinator.abort_batch();
                                token.nack(batch_error_code(error))
                            }
                        };
                        let _ = coordinator.complete(token, response.clone());
                        RESPONSES.send(response).await;
                        if result.is_err() {
                            let generation = cancellation.cancel();
                            CANCEL.signal(generation);
                            JOBS.send(ExecutionJob::Emergency { stop: None }).await;
                        }
                    }
                    Admission::Bypass(request) => {
                        complete_bypass(&mut coordinator, request).await;
                    }
                    Admission::NoResponse(_) => {}
                    Admission::Stop(request) => {
                        batch = None;
                        let stop = request
                            .completion_token()
                            .expect("STOP_ALL must carry a completion token");
                        let generation = cancellation.cancel();
                        CANCEL.signal(generation);
                        JOBS.send(ExecutionJob::Emergency { stop: Some(stop) })
                            .await;
                    }
                    Admission::Replay(response) => {
                        RESPONSES.send(response).await;
                    }
                    Admission::Immediate(response) => {
                        debug_assert_eq!(command_type, CommandType::BatchBegin);
                        batch = Some(BatchCollector::begin(input_state));
                        RESPONSES.send(response).await;
                    }
                    Admission::Busy(reason) => {
                        RESPONSES
                            .send(CachedResponse::busy(
                                version,
                                sequence,
                                reason,
                                BUSY_RETRY_MS,
                            ))
                            .await;
                    }
                    Admission::Reject(error) => {
                        RESPONSES
                            .send(CachedResponse::nack(version, sequence, error))
                            .await;
                    }
                }
            }
            Either3::Second(result) => match result {
                ExecutionResult::Command {
                    token,
                    response,
                    input_state: next_state,
                } => {
                    input_state = next_state;
                    if coordinator.complete(token, response.clone()).is_ok() {
                        RESPONSES.send(response).await;
                    }
                }
                ExecutionResult::Emergency {
                    stop,
                    input_state: next_state,
                } => {
                    input_state = next_state;
                    if let Some(token) = stop {
                        let response = token.ack();
                        if coordinator.complete(token, response.clone()).is_ok() {
                            RESPONSES.send(response).await;
                        }
                    }
                }
            },
            Either3::First(_event) => {
                batch = None;
                coordinator.clear_session();
                let generation = cancellation.cancel();
                CANCEL.signal(generation);
                JOBS.send(ExecutionJob::Emergency { stop: None }).await;
            }
        }

        update_guarded(&coordinator, batch.as_ref(), input_state);
    }
}

async fn complete_bypass(coordinator: &mut Coordinator, request: OwnedRequest) {
    let token = request
        .completion_token()
        .expect("response-bearing bypass must carry a completion token");
    let response = match request.command_type() {
        CommandType::GetInfo => {
            CachedResponse::status(token.version(), token.sequence(), &info_payload())
                .expect("info response fits cache")
        }
        CommandType::GetCaps => CachedResponse::status(
            token.version(),
            token.sequence(),
            &capability_payload(request.version()),
        )
        .expect("capability response fits cache"),
        _ => token.ack(),
    };
    let completed = coordinator.complete(token, response.clone());
    debug_assert!(completed.is_ok());
    RESPONSES.send(response).await;
}

#[embassy_executor::task]
pub async fn executor_task(mut keyboard: KeyboardWriter, mut mouse: MouseWriter) -> ! {
    let mut input_state = InputState::new();

    loop {
        match JOBS.receive().await {
            ExecutionJob::Single {
                request,
                baseline_generation,
            } => {
                let token = request
                    .completion_token()
                    .expect("executor request must carry a completion token");
                let mut cancel = CancelWait::new(&CANCEL, baseline_generation);
                let result = execute_request(
                    &request,
                    &mut keyboard,
                    &mut mouse,
                    &mut input_state,
                    &mut cancel,
                )
                .await;
                RESULTS
                    .send(ExecutionResult::Command {
                        token,
                        response: execution_response(token, result),
                        input_state,
                    })
                    .await;
            }
            ExecutionJob::Batch {
                request,
                batch,
                baseline_generation,
            } => {
                let token = request
                    .completion_token()
                    .expect("batch end must carry a completion token");
                let mut cancel = CancelWait::new(&CANCEL, baseline_generation);
                let mut result = Ok(DeviceResponse::Ack);
                for command in batch.commands() {
                    result = execute_owned_command(
                        request.version(),
                        request.sequence(),
                        command,
                        &mut keyboard,
                        &mut mouse,
                        &mut input_state,
                        &mut cancel,
                    )
                    .await;
                    if result.is_err() {
                        break;
                    }
                }
                if result.is_err() {
                    let _ = reset_inputs(&mut keyboard, &mut mouse, &mut input_state).await;
                }
                RESULTS
                    .send(ExecutionResult::Command {
                        token,
                        response: execution_response(token, result),
                        input_state,
                    })
                    .await;
            }
            ExecutionJob::Emergency { stop } => {
                let _ = reset_inputs(&mut keyboard, &mut mouse, &mut input_state).await;
                RESULTS
                    .send(ExecutionResult::Emergency { stop, input_state })
                    .await;
            }
        }
    }
}

async fn execute_request(
    request: &OwnedRequest,
    keyboard: &mut KeyboardWriter,
    mouse: &mut MouseWriter,
    input_state: &mut InputState,
    cancel: &mut CancelWait<'_>,
) -> Result<DeviceResponse, ErrorCode> {
    let OwnedRequestBody::Command(command) = request.body() else {
        return Err(ErrorCode::BadCommand);
    };
    execute_owned_command(
        request.version(),
        request.sequence(),
        command,
        keyboard,
        mouse,
        input_state,
        cancel,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn execute_owned_command(
    version: u8,
    sequence: u16,
    command: &OwnedCommand,
    keyboard: &mut KeyboardWriter,
    mouse: &mut MouseWriter,
    input_state: &mut InputState,
    cancel: &mut CancelWait<'_>,
) -> Result<DeviceResponse, ErrorCode> {
    let mut payload = [0u8; MAX_PAYLOAD_SIZE];
    let (command_type, payload_len) = encode_owned_command(command, &mut payload);
    let frame = Frame {
        version,
        flags: 0,
        sequence,
        command_type,
        payload: &payload[..payload_len],
    };
    execute_frame_with_cancel(&frame, keyboard, mouse, input_state, cancel).await
}

fn encode_owned_command(
    command: &OwnedCommand,
    payload: &mut [u8; MAX_PAYLOAD_SIZE],
) -> (CommandType, usize) {
    match command {
        OwnedCommand::Ping => (CommandType::Ping, 0),
        OwnedCommand::GetInfo => (CommandType::GetInfo, 0),
        OwnedCommand::GetCaps => (CommandType::GetCaps, 0),
        OwnedCommand::Heartbeat => (CommandType::Heartbeat, 0),
        OwnedCommand::KeyDown(stroke) => {
            payload[..2].copy_from_slice(&[stroke.modifier, stroke.keycode]);
            (CommandType::KeyDown, 2)
        }
        OwnedCommand::KeyUp(stroke) => {
            payload[..2].copy_from_slice(&[stroke.modifier, stroke.keycode]);
            (CommandType::KeyUp, 2)
        }
        OwnedCommand::KeyTap(stroke) => {
            payload[..2].copy_from_slice(&[stroke.modifier, stroke.keycode]);
            (CommandType::KeyTap, 2)
        }
        OwnedCommand::TypeAscii(bytes) => {
            payload[..bytes.len()].copy_from_slice(bytes.as_slice());
            (CommandType::TypeAscii, bytes.len())
        }
        OwnedCommand::MouseMoveRel { dx, dy } => {
            payload[..2].copy_from_slice(&dx.to_be_bytes());
            payload[2..4].copy_from_slice(&dy.to_be_bytes());
            (CommandType::MouseMoveRel, 4)
        }
        OwnedCommand::MouseButtonDown(button) => {
            payload[0] = button.mask();
            (CommandType::MouseButtonDown, 1)
        }
        OwnedCommand::MouseButtonUp(button) => {
            payload[0] = button.mask();
            (CommandType::MouseButtonUp, 1)
        }
        OwnedCommand::MouseClick(button) => {
            payload[0] = button.mask();
            (CommandType::MouseClick, 1)
        }
        OwnedCommand::MouseWheel(wheel) => {
            payload[0] = *wheel as u8;
            (CommandType::MouseWheel, 1)
        }
        OwnedCommand::WaitMs(wait_ms) => {
            payload[..4].copy_from_slice(&wait_ms.to_be_bytes());
            (CommandType::WaitMs, 4)
        }
        OwnedCommand::StopAll => (CommandType::StopAll, 0),
    }
}

fn execution_response(
    token: CompletionToken,
    result: Result<DeviceResponse, ErrorCode>,
) -> CachedResponse {
    match result {
        Ok(DeviceResponse::Ack) => token.ack(),
        Ok(DeviceResponse::Info(payload)) => {
            CachedResponse::status(token.version(), token.sequence(), &payload)
                .expect("info response fits cache")
        }
        Ok(DeviceResponse::Caps(payload)) => {
            CachedResponse::status(token.version(), token.sequence(), &payload)
                .expect("capability response fits cache")
        }
        Err(error) => token.nack(error),
    }
}

#[embassy_executor::task]
pub async fn response_task(mut sender: CdcSender) -> ! {
    loop {
        sender.wait_connection().await;
        loop {
            let response = RESPONSES.receive().await;
            match send_cached_response(&mut sender, &response).await {
                Ok(()) => {}
                Err(EndpointError::Disabled) => {
                    SAFETY_EVENTS.send(SafetyEvent::UsbDisabled).await;
                    break;
                }
                Err(EndpointError::BufferOverflow) => {}
            }
        }
    }
}

#[embassy_executor::task]
pub async fn cdc_control_task(control: CdcControl) -> ! {
    let mut previous_dtr = control.dtr();
    loop {
        control.control_changed().await;
        let dtr = control.dtr();
        if previous_dtr && !dtr {
            SAFETY_EVENTS.send(SafetyEvent::DtrLost).await;
        }
        previous_dtr = dtr;
    }
}

#[embassy_executor::task]
pub async fn lease_task() -> ! {
    loop {
        LEASE_REFRESH.wait().await;
        loop {
            match select(Timer::after_millis(CONTROL_LEASE_MS), LEASE_REFRESH.wait()).await {
                Either::First(()) => {
                    if GUARDED_WORK.load(Ordering::Acquire) {
                        SAFETY_EVENTS.send(SafetyEvent::LeaseExpired).await;
                    }
                    break;
                }
                Either::Second(()) => {}
            }
        }
    }
}

/// Task 8 will wire the keyboard OUT endpoint and implement LED report parsing.
#[embassy_executor::task]
pub async fn keyboard_led_task(reader: KeyboardReader) -> ! {
    let _reader = reader;
    loop {
        Timer::after_millis(60_000).await;
    }
}

fn update_guarded(
    coordinator: &Coordinator,
    batch: Option<&BatchCollector>,
    input_state: InputState,
) {
    let guarded = !input_state.is_idle() || batch.is_some() || coordinator.is_executor_occupied();
    GUARDED_WORK.store(guarded, Ordering::Release);
}

fn response_version(data: &[u8]) -> u8 {
    match data.get(2).copied() {
        Some(LEGACY_PROTOCOL_VERSION) => LEGACY_PROTOCOL_VERSION,
        Some(PROTOCOL_VERSION) => PROTOCOL_VERSION,
        _ => PROTOCOL_VERSION,
    }
}

fn batch_error_code(error: BatchError) -> ErrorCode {
    match error {
        BatchError::Capacity => ErrorCode::BatchCapacity,
        BatchError::TooManyKeys => ErrorCode::TooManyKeys,
        BatchError::UnsupportedAscii(_) => ErrorCode::UnsupportedAscii,
        BatchError::KeyboardBusy => ErrorCode::KeyboardBusy,
        BatchError::WaitTooLong => ErrorCode::WaitTooLong,
        BatchError::NotBatchable => ErrorCode::BadCommand,
    }
}
