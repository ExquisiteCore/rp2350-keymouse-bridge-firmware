use std::io::{Read, Write};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use hid_protocol::protocol::{
    decode_frame, encode_frame, CommandType, DecodeError, FRAME_OVERHEAD, MAGIC, MAX_FRAME_SIZE,
    MAX_PAYLOAD_SIZE,
};
use serialport::{ClearBuffer, SerialPort, SerialPortType};

const DEFAULT_VID: u16 = 0xCAFE;
const DEFAULT_PID: u16 = 0x2350;
const PROTOCOL_VERSION: u8 = 1;

#[derive(Clone, Debug)]
pub struct ClientOptions {
    pub port: Option<String>,
    pub baud: u32,
    pub timeout: Duration,
    pub retries: u8,
    pub vid: u16,
    pub pid: u16,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            port: None,
            baud: 115_200,
            timeout: Duration::from_millis(1_000),
            retries: 2,
            vid: DEFAULT_VID,
            pid: DEFAULT_PID,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PortInfo {
    pub name: String,
    pub vid: Option<u16>,
    pub pid: Option<u16>,
    pub product: Option<String>,
    pub serial_number: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Response {
    pub command_type: CommandType,
    pub payload: Vec<u8>,
}

pub struct HidClient {
    port: Box<dyn SerialPort>,
    sequence: u16,
    timeout: Duration,
    retries: u8,
}

impl HidClient {
    pub fn open(options: &ClientOptions) -> Result<Self> {
        let port_name = match &options.port {
            Some(port) => port.clone(),
            None => find_board_port(options.vid, options.pid)?
                .ok_or_else(|| anyhow!("no RP2350 HID bridge serial port found"))?,
        };

        let port = serialport::new(&port_name, options.baud)
            .timeout(options.timeout)
            .open()
            .with_context(|| format!("open serial port {port_name}"))?;

        Ok(Self {
            port,
            sequence: 1,
            timeout: options.timeout,
            retries: options.retries,
        })
    }

    pub fn send_command(&mut self, command_type: CommandType, payload: &[u8]) -> Result<Response> {
        if payload.len() > MAX_PAYLOAD_SIZE {
            bail!("payload is {} bytes, max is {}", payload.len(), MAX_PAYLOAD_SIZE);
        }

        let sequence = self.next_sequence();
        let mut frame = [0u8; MAX_FRAME_SIZE];
        let frame_len = encode_frame(PROTOCOL_VERSION, sequence, command_type, payload, &mut frame)
            .map_err(|err| anyhow!("encode frame failed: {err:?}"))?;

        let mut last_error = None;
        for attempt in 0..=self.retries {
            let _ = self.port.clear(ClearBuffer::Input);
            self.port.write_all(&frame[..frame_len])?;
            self.port.flush()?;

            match self.read_response(sequence) {
                Ok(Response {
                    command_type: CommandType::Busy,
                    ..
                }) if attempt < self.retries => {
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                }
                Ok(Response {
                    command_type: CommandType::Nack,
                    payload,
                }) => {
                    let code = payload.first().copied().unwrap_or(0);
                    bail!("device returned NACK error code {code}");
                }
                Ok(response) => {
                    let expected = expected_response_type(command_type);
                    if response.command_type == expected {
                        return Ok(response);
                    }

                    let err = anyhow!(
                        "unexpected response type {:?}, expected {:?}",
                        response.command_type,
                        expected
                    );
                    if attempt < self.retries {
                        last_error = Some(err);
                        continue;
                    }
                    return Err(err);
                }
                Err(err) if attempt < self.retries => {
                    last_error = Some(err);
                    continue;
                }
                Err(err) => return Err(err),
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow!("command timed out")))
    }

    fn next_sequence(&mut self) -> u16 {
        let sequence = self.sequence;
        self.sequence = self.sequence.wrapping_add(1).max(1);
        sequence
    }

    fn read_response(&mut self, expected_sequence: u16) -> Result<Response> {
        let deadline = Instant::now() + self.timeout;
        let mut buf = Vec::with_capacity(MAX_FRAME_SIZE);
        let mut chunk = [0u8; 64];

        loop {
            if Instant::now() >= deadline {
                bail!("timed out waiting for response");
            }

            match self.port.read(&mut chunk) {
                Ok(0) => continue,
                Ok(read) => {
                    buf.extend_from_slice(&chunk[..read]);
                    if let Some(response) = try_decode_response(&mut buf, expected_sequence)? {
                        return Ok(response);
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::TimedOut => continue,
                Err(err) => return Err(err.into()),
            }
        }
    }
}

pub fn list_ports() -> Result<Vec<PortInfo>> {
    let ports = serialport::available_ports()?;
    Ok(ports
        .into_iter()
        .map(|port| {
            let (vid, pid, product, serial_number) = match port.port_type {
                SerialPortType::UsbPort(info) => (
                    Some(info.vid),
                    Some(info.pid),
                    info.product,
                    info.serial_number,
                ),
                _ => (None, None, None, None),
            };
            PortInfo {
                name: port.port_name,
                vid,
                pid,
                product,
                serial_number,
            }
        })
        .collect())
}

pub fn find_board_port(vid: u16, pid: u16) -> Result<Option<String>> {
    Ok(list_ports()?.into_iter().find_map(|port| match (port.vid, port.pid) {
        (Some(port_vid), Some(port_pid)) if port_vid == vid && port_pid == pid => Some(port.name),
        _ => None,
    }))
}

fn try_decode_response(buf: &mut Vec<u8>, expected_sequence: u16) -> Result<Option<Response>> {
    loop {
        if buf.len() < 2 {
            return Ok(None);
        }
        if buf[0..2] != MAGIC {
            if let Some(pos) = buf.windows(2).position(|window| window == MAGIC) {
                buf.drain(0..pos);
            } else {
                let keep = buf.pop();
                buf.clear();
                if let Some(byte) = keep {
                    buf.push(byte);
                }
            }
            continue;
        }
        if buf.len() < 9 {
            return Ok(None);
        }

        let payload_len = u16::from_be_bytes([buf[7], buf[8]]) as usize;
        let frame_len = FRAME_OVERHEAD + payload_len;
        if frame_len > MAX_FRAME_SIZE {
            bail!("response frame too long");
        }
        if buf.len() < frame_len {
            return Ok(None);
        }

        let frame_bytes: Vec<u8> = buf.drain(0..frame_len).collect();
        let frame = match decode_frame(&frame_bytes) {
            Ok(frame) => frame,
            Err(DecodeError::BadCrc) => bail!("response CRC check failed"),
            Err(err) => bail!("invalid response frame: {err:?}"),
        };

        if frame.sequence != expected_sequence {
            continue;
        }

        return Ok(Some(Response {
            command_type: frame.command_type,
            payload: frame.payload.to_vec(),
        }));
    }
}

fn expected_response_type(command_type: CommandType) -> CommandType {
    match command_type {
        CommandType::GetInfo | CommandType::GetCaps => CommandType::Status,
        _ => CommandType::Ack,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_commands_to_expected_response_types() {
        assert_eq!(expected_response_type(CommandType::Ping), CommandType::Ack);
        assert_eq!(expected_response_type(CommandType::KeyTap), CommandType::Ack);
        assert_eq!(expected_response_type(CommandType::GetInfo), CommandType::Status);
        assert_eq!(expected_response_type(CommandType::GetCaps), CommandType::Status);
    }
}
