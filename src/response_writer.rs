//! CDC 响应帧写入。

use embassy_usb::driver::EndpointError;

use crate::error::ErrorCode;
use crate::firmware_config::PROTOCOL_VERSION;
use crate::protocol::{CommandType, MAX_FRAME_SIZE, encode_frame};
use crate::usb_device::CdcClass;

pub async fn send_ack(cdc: &mut CdcClass, sequence: u16) -> Result<(), EndpointError> {
    send_response(cdc, sequence, CommandType::Ack, &[]).await
}

pub async fn send_nack(
    cdc: &mut CdcClass,
    sequence: u16,
    error: ErrorCode,
) -> Result<(), EndpointError> {
    send_response(cdc, sequence, CommandType::Nack, &[error as u8]).await
}

pub async fn send_status(
    cdc: &mut CdcClass,
    sequence: u16,
    payload: &[u8],
) -> Result<(), EndpointError> {
    send_response(cdc, sequence, CommandType::Status, payload).await
}

async fn send_response(
    cdc: &mut CdcClass,
    sequence: u16,
    command_type: CommandType,
    payload: &[u8],
) -> Result<(), EndpointError> {
    let mut out = [0u8; MAX_FRAME_SIZE];
    let len = encode_frame(PROTOCOL_VERSION, sequence, command_type, payload, &mut out)
        .map_err(|_| EndpointError::BufferOverflow)?;
    cdc.write_packet(&out[..len]).await
}
