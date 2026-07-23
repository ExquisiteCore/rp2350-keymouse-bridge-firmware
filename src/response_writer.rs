//! CDC response frame serialization for the dedicated sender task.

use embassy_usb::driver::EndpointError;

use crate::coordinator::CachedResponse;
use crate::protocol::{MAX_FRAME_SIZE, encode_frame};
use crate::usb_device::CdcSender;

pub async fn send_cached_response(
    sender: &mut CdcSender,
    response: &CachedResponse,
) -> Result<(), EndpointError> {
    let mut out = [0u8; MAX_FRAME_SIZE];
    let len = encode_frame(
        response.version(),
        response.sequence(),
        response.command_type(),
        response.payload(),
        &mut out,
    )
    .map_err(|_| EndpointError::BufferOverflow)?;
    sender.write_packet(&out[..len]).await
}
