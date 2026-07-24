use std::io::{Read, Write};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use hid_protocol::protocol::{
    CommandType, DecodeError, FLAG_NO_RESPONSE, FRAME_OVERHEAD, MAGIC, MAX_FRAME_SIZE,
    MAX_PAYLOAD_SIZE, PROTOCOL_VERSION, decode_frame, encode_frame, encode_frame_with_flags,
};
use serialport::{SerialPort, SerialPortType};

const DEFAULT_VID: u16 = 0xCAFE;
const DEFAULT_PID: u16 = 0x2350;
const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Clone, Debug)]
pub struct ClientOptions {
    pub port: Option<String>,
    pub baud: u32,
    pub timeout: Duration,
    pub retries: u8,
    pub vid: u16,
    pub pid: u16,
    pub heartbeat_interval: Duration,
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
            heartbeat_interval: HEARTBEAT_INTERVAL,
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

trait ClientPort: Read + Write + Send {
    fn try_clone_port(&self) -> Result<Box<dyn ClientPort>>;
    fn set_timeout(&mut self, timeout: Duration) -> Result<()>;
    fn set_dtr(&mut self, asserted: bool) -> Result<()>;
}

struct SerialPortAdapter {
    inner: Box<dyn SerialPort>,
}

impl Read for SerialPortAdapter {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf)
    }
}

impl Write for SerialPortAdapter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

impl ClientPort for SerialPortAdapter {
    fn try_clone_port(&self) -> Result<Box<dyn ClientPort>> {
        Ok(Box::new(Self {
            inner: self.inner.try_clone()?,
        }))
    }

    fn set_timeout(&mut self, timeout: Duration) -> Result<()> {
        self.inner.set_timeout(timeout)?;
        Ok(())
    }

    fn set_dtr(&mut self, asserted: bool) -> Result<()> {
        self.inner.write_data_terminal_ready(asserted)?;
        Ok(())
    }
}

struct HeartbeatWorker {
    stop: Arc<(Mutex<bool>, Condvar)>,
    handle: JoinHandle<()>,
}

impl HeartbeatWorker {
    fn request_stop(&self) {
        let mut stopped = self.stop.0.lock().unwrap();
        *stopped = true;
        self.stop.1.notify_all();
    }

    fn join(self) {
        let _ = self.handle.join();
    }
}

enum ReadResponseError {
    Timeout,
    Other(anyhow::Error),
}

pub struct HidClient {
    port: Box<dyn ClientPort>,
    sequence: u16,
    timeout: Duration,
    retries: u8,
    receive_buffer: Vec<u8>,
    batch_duration_ms: Option<u64>,
    write_mutex: Arc<Mutex<()>>,
    heartbeat: Option<HeartbeatWorker>,
    retry_sleep: Arc<dyn Fn(Duration) + Send + Sync>,
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

        Self::from_port(
            Box::new(SerialPortAdapter { inner: port }),
            options,
            Arc::new(std::thread::sleep),
        )
    }

    pub fn send_command(&mut self, command_type: CommandType, payload: &[u8]) -> Result<Response> {
        let timeout = self.response_timeout(command_type, payload);
        let response = self.send_command_with_timeout(command_type, payload, timeout)?;
        self.record_completed_command(command_type, payload);
        Ok(response)
    }

    fn from_port(
        mut port: Box<dyn ClientPort>,
        options: &ClientOptions,
        retry_sleep: Arc<dyn Fn(Duration) + Send + Sync>,
    ) -> Result<Self> {
        if options.heartbeat_interval.is_zero() {
            bail!("heartbeat interval must be positive");
        }
        port.set_timeout(options.timeout)?;
        port.set_dtr(true).context("assert serial DTR")?;
        let heartbeat_port = match port.try_clone_port() {
            Ok(port) => port,
            Err(err) => {
                let _ = port.set_dtr(false);
                return Err(err).context("clone serial port for heartbeat");
            }
        };
        let write_mutex = Arc::new(Mutex::new(()));
        let heartbeat = match start_heartbeat(
            heartbeat_port,
            Arc::clone(&write_mutex),
            options.heartbeat_interval,
        ) {
            Ok(worker) => worker,
            Err(err) => {
                let _ = port.set_dtr(false);
                return Err(err).context("start heartbeat worker");
            }
        };

        Ok(Self {
            port,
            sequence: 1,
            timeout: options.timeout,
            retries: options.retries,
            receive_buffer: Vec::with_capacity(MAX_FRAME_SIZE),
            batch_duration_ms: None,
            write_mutex,
            heartbeat: Some(heartbeat),
            retry_sleep,
        })
    }

    fn send_command_with_timeout(
        &mut self,
        command_type: CommandType,
        payload: &[u8],
        response_timeout: Duration,
    ) -> Result<Response> {
        if payload.len() > MAX_PAYLOAD_SIZE {
            bail!(
                "payload is {} bytes, max is {}",
                payload.len(),
                MAX_PAYLOAD_SIZE
            );
        }

        let sequence = self.next_sequence();
        let mut frame = [0u8; MAX_FRAME_SIZE];
        let frame_len = encode_frame(
            PROTOCOL_VERSION,
            sequence,
            command_type,
            payload,
            &mut frame,
        )
        .map_err(|err| anyhow!("encode frame failed: {err:?}"))?;

        for attempt in 0..=self.retries {
            self.write_frame(&frame[..frame_len])?;

            match self.read_response(sequence, response_timeout) {
                Ok(Response {
                    command_type: CommandType::Busy,
                    payload,
                }) => {
                    let delay = busy_retry_delay(&payload)?;
                    if attempt >= self.retries {
                        bail!("device remained BUSY");
                    }
                    (self.retry_sleep)(delay);
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

                    bail!(
                        "unexpected response type {:?}, expected {:?}",
                        response.command_type,
                        expected
                    );
                }
                Err(ReadResponseError::Timeout) if attempt < self.retries => continue,
                Err(ReadResponseError::Timeout) => bail!("timed out waiting for response"),
                Err(ReadResponseError::Other(err)) => return Err(err),
            }
        }

        unreachable!("retry loop always returns")
    }

    fn next_sequence(&mut self) -> u16 {
        let sequence = self.sequence;
        self.sequence = self.sequence.wrapping_add(1).max(1);
        sequence
    }

    fn write_frame(&mut self, frame: &[u8]) -> Result<()> {
        let write_mutex = Arc::clone(&self.write_mutex);
        let _guard = write_mutex.lock().unwrap();
        self.port.write_all(frame)?;
        self.port.flush()?;
        Ok(())
    }

    fn read_response(
        &mut self,
        expected_sequence: u16,
        timeout: Duration,
    ) -> std::result::Result<Response, ReadResponseError> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| ReadResponseError::Other(anyhow!("response timeout is too large")))?;
        let mut chunk = [0u8; 64];

        loop {
            if let Some(response) = try_decode_response(&mut self.receive_buffer, expected_sequence)
                .map_err(ReadResponseError::Other)?
            {
                return Ok(response);
            }
            if Instant::now() >= deadline {
                return Err(ReadResponseError::Timeout);
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            self.port
                .set_timeout(remaining)
                .map_err(ReadResponseError::Other)?;
            match self.port.read(&mut chunk) {
                Ok(0) => continue,
                Ok(read) => {
                    self.receive_buffer.extend_from_slice(&chunk[..read]);
                }
                Err(err) if err.kind() == std::io::ErrorKind::TimedOut => continue,
                Err(err) => return Err(ReadResponseError::Other(err.into())),
            }
        }
    }

    fn response_timeout(&self, command_type: CommandType, payload: &[u8]) -> Duration {
        let configured_ms = self.timeout.as_millis().min(u64::MAX as u128) as u64;
        let required_ms = if command_type == CommandType::BatchEnd {
            self.batch_duration_ms
                .map(|duration| duration.saturating_add(1_000))
                .unwrap_or(1_000)
        } else if let Some(duration) = known_command_duration_ms(command_type, payload) {
            duration.saturating_add(500)
        } else {
            1_000
        };
        Duration::from_millis(configured_ms.max(required_ms))
    }

    fn record_completed_command(&mut self, command_type: CommandType, payload: &[u8]) {
        match command_type {
            CommandType::BatchBegin => self.batch_duration_ms = Some(0),
            CommandType::BatchEnd | CommandType::StopAll => self.batch_duration_ms = None,
            _ => {
                if let (Some(total), Some(duration)) = (
                    self.batch_duration_ms.as_mut(),
                    known_command_duration_ms(command_type, payload),
                ) {
                    *total = total.saturating_add(duration);
                }
            }
        }
    }
}

impl Drop for HidClient {
    fn drop(&mut self) {
        let heartbeat = self.heartbeat.take();
        if let Some(worker) = heartbeat.as_ref() {
            worker.request_stop();
        }

        let sequence = self.next_sequence();
        let mut frame = [0u8; MAX_FRAME_SIZE];
        if let Ok(frame_len) = encode_frame(
            PROTOCOL_VERSION,
            sequence,
            CommandType::StopAll,
            &[],
            &mut frame,
        ) {
            let _ = self.write_frame(&frame[..frame_len]);
        }
        if let Some(worker) = heartbeat {
            worker.join();
        }
        let _ = self.port.set_dtr(false);
    }
}

fn start_heartbeat(
    mut port: Box<dyn ClientPort>,
    write_mutex: Arc<Mutex<()>>,
    interval: Duration,
) -> std::io::Result<HeartbeatWorker> {
    let mut frame = [0u8; MAX_FRAME_SIZE];
    let frame_len = encode_frame_with_flags(
        PROTOCOL_VERSION,
        FLAG_NO_RESPONSE,
        0,
        CommandType::Heartbeat,
        &[],
        &mut frame,
    )
    .expect("heartbeat frame always fits");
    let frame = frame[..frame_len].to_vec();
    let stop = Arc::new((Mutex::new(false), Condvar::new()));
    let thread_stop = Arc::clone(&stop);
    let handle = std::thread::Builder::new()
        .name("hidctl-heartbeat".into())
        .spawn(move || {
            while wait_for_heartbeat_tick(&thread_stop, interval) {
                if !write_heartbeat_if_running(&thread_stop, &write_mutex, &mut *port, &frame) {
                    break;
                }
            }
        })?;
    Ok(HeartbeatWorker { stop, handle })
}

fn write_heartbeat_if_running(
    stop: &(Mutex<bool>, Condvar),
    write_mutex: &Mutex<()>,
    port: &mut dyn ClientPort,
    frame: &[u8],
) -> bool {
    let _guard = write_mutex.lock().unwrap();
    if *stop.0.lock().unwrap() {
        return false;
    }
    let _ = port.write_all(frame);
    let _ = port.flush();
    true
}

fn wait_for_heartbeat_tick(stop: &(Mutex<bool>, Condvar), interval: Duration) -> bool {
    let stopped = stop.0.lock().unwrap();
    if *stopped {
        return false;
    }

    let (stopped, wait) = stop
        .1
        .wait_timeout_while(stopped, interval, |stopped| !*stopped)
        .unwrap();
    !*stopped && wait.timed_out()
}

fn busy_retry_delay(payload: &[u8]) -> Result<Duration> {
    if payload.len() != 3 {
        bail!("malformed BUSY payload ({} bytes)", payload.len());
    }
    Ok(Duration::from_millis(
        u16::from_be_bytes([payload[1], payload[2]]) as u64,
    ))
}

fn known_command_duration_ms(command_type: CommandType, payload: &[u8]) -> Option<u64> {
    match command_type {
        CommandType::WaitMs if payload.len() == 4 => {
            Some(u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]) as u64)
        }
        CommandType::TypeAscii => Some((payload.len() as u64).saturating_mul(8)),
        CommandType::MouseMoveRel if payload.len() == 4 => {
            let dx = i16::from_be_bytes([payload[0], payload[1]]) as i32;
            let dy = i16::from_be_bytes([payload[2], payload[3]]) as i32;
            let extent = dx.unsigned_abs().max(dy.unsigned_abs()) as u64;
            Some(extent.saturating_add(126) / 127)
        }
        CommandType::KeyTap => Some(8),
        CommandType::MouseClick => Some(20),
        _ => None,
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
    Ok(list_ports()?
        .into_iter()
        .find_map(|port| match (port.vid, port.pid) {
            (Some(port_vid), Some(port_pid)) if port_vid == vid && port_pid == pid => {
                Some(port.name)
            }
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
    use std::collections::VecDeque;
    use std::io;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::sync::{Arc, Condvar, Mutex};

    #[derive(Default)]
    struct FakeState {
        rx: VecDeque<u8>,
        writes: Vec<Vec<u8>>,
        timeouts: Vec<Duration>,
        dtr: Vec<bool>,
        max_concurrent_writes: usize,
        gate_first_write: bool,
        first_write_entered: bool,
        release_first_write: bool,
        write_entries: usize,
    }

    #[derive(Clone)]
    struct FakePort {
        state: Arc<(Mutex<FakeState>, Condvar)>,
        active_writes: Arc<AtomicUsize>,
    }

    impl FakePort {
        fn new(responses: &[Vec<u8>]) -> (Self, Arc<(Mutex<FakeState>, Condvar)>) {
            let mut state = FakeState::default();
            for response in responses {
                state.rx.extend(response.iter().copied());
            }
            let state = Arc::new((Mutex::new(state), Condvar::new()));
            (
                Self {
                    state: Arc::clone(&state),
                    active_writes: Arc::new(AtomicUsize::new(0)),
                },
                state,
            )
        }

        fn new_gated(responses: &[Vec<u8>]) -> (Self, Arc<(Mutex<FakeState>, Condvar)>) {
            let (port, state) = Self::new(responses);
            state.0.lock().unwrap().gate_first_write = true;
            (port, state)
        }
    }

    impl Read for FakePort {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let mut state = self.state.0.lock().unwrap();
            if state.rx.is_empty() {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "fake timeout"));
            }
            let count = buf.len().min(state.rx.len());
            for slot in &mut buf[..count] {
                *slot = state.rx.pop_front().unwrap();
            }
            Ok(count)
        }
    }

    impl Write for FakePort {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let concurrent = self.active_writes.fetch_add(1, Ordering::SeqCst) + 1;
            let mut state = self.state.0.lock().unwrap();
            state.max_concurrent_writes = state.max_concurrent_writes.max(concurrent);
            state.write_entries += 1;
            self.state.1.notify_all();
            if state.gate_first_write && !state.first_write_entered {
                state.first_write_entered = true;
                self.state.1.notify_all();
                while !state.release_first_write {
                    state = self.state.1.wait(state).unwrap();
                }
            }
            state.writes.push(buf.to_vec());
            self.state.1.notify_all();
            self.active_writes.fetch_sub(1, Ordering::SeqCst);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl ClientPort for FakePort {
        fn try_clone_port(&self) -> Result<Box<dyn ClientPort>> {
            Ok(Box::new(self.clone()))
        }

        fn set_timeout(&mut self, timeout: Duration) -> Result<()> {
            self.state.0.lock().unwrap().timeouts.push(timeout);
            Ok(())
        }

        fn set_dtr(&mut self, asserted: bool) -> Result<()> {
            self.state.0.lock().unwrap().dtr.push(asserted);
            Ok(())
        }
    }

    fn response(sequence: u16, command_type: CommandType, payload: &[u8]) -> Vec<u8> {
        let mut frame = [0u8; MAX_FRAME_SIZE];
        let len = encode_frame(
            PROTOCOL_VERSION,
            sequence,
            command_type,
            payload,
            &mut frame,
        )
        .unwrap();
        frame[..len].to_vec()
    }

    fn options(retries: u8, heartbeat_interval: Duration) -> ClientOptions {
        ClientOptions {
            port: Some("FAKE".into()),
            timeout: Duration::from_millis(100),
            retries,
            heartbeat_interval,
            ..ClientOptions::default()
        }
    }

    fn fake_client(
        responses: &[Vec<u8>],
        retries: u8,
    ) -> (
        HidClient,
        Arc<(Mutex<FakeState>, Condvar)>,
        Arc<Mutex<Vec<Duration>>>,
    ) {
        let (port, state) = FakePort::new(responses);
        let sleeps = Arc::new(Mutex::new(Vec::new()));
        let recorded_sleeps = Arc::clone(&sleeps);
        let client = HidClient::from_port(
            Box::new(port),
            &options(retries, Duration::from_secs(60)),
            Arc::new(move |duration| recorded_sleeps.lock().unwrap().push(duration)),
        )
        .unwrap();
        (client, state, sleeps)
    }

    #[test]
    fn maps_commands_to_expected_response_types() {
        assert_eq!(expected_response_type(CommandType::Ping), CommandType::Ack);
        assert_eq!(
            expected_response_type(CommandType::KeyTap),
            CommandType::Ack
        );
        assert_eq!(
            expected_response_type(CommandType::GetInfo),
            CommandType::Status
        );
        assert_eq!(
            expected_response_type(CommandType::GetCaps),
            CommandType::Status
        );
    }

    #[test]
    fn nack_is_terminal_and_writes_once() {
        let (mut client, state, _) = fake_client(&[response(1, CommandType::Nack, &[15])], 3);

        let error = client
            .send_command_with_timeout(CommandType::KeyDown, &[0, 0x1a], Duration::from_secs(1))
            .unwrap_err();

        assert!(error.to_string().contains("15"));
        assert_eq!(state.0.lock().unwrap().writes.len(), 1);
    }

    #[test]
    fn timeout_retry_reuses_exact_frame_and_sequence() {
        let (mut client, state, _) = fake_client(&[], 1);

        let error = client
            .send_command_with_timeout(CommandType::Ping, &[], Duration::ZERO)
            .unwrap_err();

        assert!(error.to_string().contains("timed out"));
        let state = state.0.lock().unwrap();
        assert_eq!(state.writes.len(), 2);
        assert_eq!(state.writes[0], state.writes[1]);
        assert_eq!(decode_frame(&state.writes[0]).unwrap().sequence, 1);
    }

    #[test]
    fn busy_delay_is_big_endian_and_retry_reuses_exact_frame() {
        let (mut client, state, sleeps) = fake_client(
            &[
                response(1, CommandType::Busy, &[3, 0, 25]),
                response(1, CommandType::Ack, &[]),
            ],
            1,
        );

        client
            .send_command_with_timeout(CommandType::KeyDown, &[0, 0x1a], Duration::from_secs(1))
            .unwrap();

        let state = state.0.lock().unwrap();
        assert_eq!(state.writes.len(), 2);
        assert_eq!(state.writes[0], state.writes[1]);
        assert_eq!(*sleeps.lock().unwrap(), vec![Duration::from_millis(25)]);
    }

    #[test]
    fn stale_coalesced_response_is_ignored_without_clearing_rx() {
        let combined = [
            response(99, CommandType::Ack, &[]),
            response(1, CommandType::Ack, &[]),
        ]
        .concat();
        let (mut client, state, _) = fake_client(&[combined], 0);

        client
            .send_command_with_timeout(CommandType::Ping, &[], Duration::from_secs(1))
            .unwrap();

        assert_eq!(state.0.lock().unwrap().writes.len(), 1);
    }

    #[test]
    fn response_timeouts_include_command_and_batch_duration() {
        let (mut client, _, _) = fake_client(&[], 0);

        assert_eq!(
            client.response_timeout(CommandType::Ping, &[]),
            Duration::from_secs(1)
        );
        assert_eq!(
            client.response_timeout(CommandType::WaitMs, &2_500u32.to_be_bytes()),
            Duration::from_millis(3_000)
        );
        assert_eq!(
            client.response_timeout(CommandType::TypeAscii, b"abcdefghij"),
            Duration::from_millis(580)
        );
        assert_eq!(
            client.response_timeout(CommandType::MouseMoveRel, &[1, 44, 254, 212]),
            Duration::from_millis(503)
        );

        client.record_completed_command(CommandType::BatchBegin, &[]);
        client.record_completed_command(CommandType::TypeAscii, b"abcdefghij");
        client.record_completed_command(CommandType::MouseMoveRel, &[1, 44, 254, 212]);
        client.record_completed_command(CommandType::WaitMs, &2_500u32.to_be_bytes());
        assert_eq!(
            client.response_timeout(CommandType::BatchEnd, &[]),
            Duration::from_millis(3_583)
        );
    }

    #[test]
    fn heartbeat_wait_exits_immediately_if_stop_was_already_requested() {
        let stop = (Mutex::new(true), Condvar::new());
        let started = Instant::now();

        assert!(!wait_for_heartbeat_tick(&stop, Duration::from_secs(60)));
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn drop_sends_stop_all_stops_heartbeat_and_clears_dtr() {
        let (client, state, _) = fake_client(&[], 0);
        let started = Instant::now();

        drop(client);

        assert!(started.elapsed() < Duration::from_millis(100));
        let state = state.0.lock().unwrap();
        assert_eq!(state.dtr, vec![true, false]);
        let commands = state
            .writes
            .iter()
            .map(|bytes| decode_frame(bytes).unwrap().command_type)
            .collect::<Vec<_>>();
        assert_eq!(commands, vec![CommandType::StopAll]);
    }

    #[test]
    fn drop_stops_heartbeat_waiting_on_write_lock_before_stop_all() {
        let (port, state) = FakePort::new(&[]);
        let mut heartbeat_port = port.clone();
        let write_mutex = Arc::new(Mutex::new(()));
        let write_guard = write_mutex.lock().unwrap();
        let stop = Arc::new((Mutex::new(false), Condvar::new()));

        let mut frame = [0u8; MAX_FRAME_SIZE];
        let frame_len = encode_frame_with_flags(
            PROTOCOL_VERSION,
            FLAG_NO_RESPONSE,
            0,
            CommandType::Heartbeat,
            &[],
            &mut frame,
        )
        .unwrap();
        let heartbeat_frame = frame[..frame_len].to_vec();
        let thread_stop = Arc::clone(&stop);
        let thread_write_mutex = Arc::clone(&write_mutex);
        let (ready_tx, ready_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            ready_tx.send(()).unwrap();
            let wrote = write_heartbeat_if_running(
                &thread_stop,
                &thread_write_mutex,
                &mut heartbeat_port,
                &heartbeat_frame,
            );
            result_tx.send(wrote).unwrap();
        });
        ready_rx.recv().unwrap();

        let client = HidClient {
            port: Box::new(port),
            sequence: 1,
            timeout: Duration::from_millis(100),
            retries: 0,
            receive_buffer: Vec::new(),
            batch_duration_ms: None,
            write_mutex: Arc::clone(&write_mutex),
            heartbeat: Some(HeartbeatWorker {
                stop: Arc::clone(&stop),
                handle,
            }),
            retry_sleep: Arc::new(|_| {}),
        };
        let drop_handle = std::thread::spawn(move || drop(client));

        let stopped = stop.0.lock().unwrap();
        let (stopped, _) = stop
            .1
            .wait_timeout_while(stopped, Duration::from_secs(1), |stopped| !*stopped)
            .unwrap();
        assert!(
            *stopped,
            "Drop must request heartbeat stop before waiting for the write lock"
        );
        drop(stopped);
        drop(write_guard);

        drop_handle.join().unwrap();
        assert!(!result_rx.recv().unwrap());
        let state = state.0.lock().unwrap();
        let commands = state
            .writes
            .iter()
            .map(|bytes| decode_frame(bytes).unwrap().command_type)
            .collect::<Vec<_>>();
        assert_eq!(commands, vec![CommandType::StopAll]);
        assert_eq!(state.dtr, vec![false]);
        assert_eq!(state.max_concurrent_writes, 1);
    }

    #[test]
    fn command_and_heartbeat_writes_are_serialized() {
        let (port, state) = FakePort::new_gated(&[response(1, CommandType::Ack, &[])]);
        let mut heartbeat_port = port.clone();
        let write_mutex = Arc::new(Mutex::new(()));
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let mut frame = [0u8; MAX_FRAME_SIZE];
        let frame_len = encode_frame_with_flags(
            PROTOCOL_VERSION,
            FLAG_NO_RESPONSE,
            0,
            CommandType::Heartbeat,
            &[],
            &mut frame,
        )
        .unwrap();
        let heartbeat_frame = frame[..frame_len].to_vec();
        let thread_stop = Arc::clone(&stop);
        let thread_write_mutex = Arc::clone(&write_mutex);
        let (trigger_tx, trigger_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            trigger_rx.recv().unwrap();
            ready_tx.send(()).unwrap();
            let wrote = write_heartbeat_if_running(
                &thread_stop,
                &thread_write_mutex,
                &mut heartbeat_port,
                &heartbeat_frame,
            );
            result_tx.send(wrote).unwrap();
        });
        let heartbeat = HeartbeatWorker {
            stop: Arc::clone(&stop),
            handle,
        };
        let mut client = HidClient {
            port: Box::new(port),
            sequence: 1,
            timeout: Duration::from_millis(100),
            retries: 0,
            receive_buffer: Vec::new(),
            batch_duration_ms: None,
            write_mutex,
            heartbeat: Some(heartbeat),
            retry_sleep: Arc::new(|_| {}),
        };
        let command_handle = std::thread::spawn(move || {
            client
                .send_command_with_timeout(CommandType::Ping, &[], Duration::from_secs(1))
                .unwrap();
            client
        });

        let state_lock = state.0.lock().unwrap();
        let (state_lock, _) = state
            .1
            .wait_timeout_while(state_lock, Duration::from_secs(1), |state| {
                !state.first_write_entered
            })
            .unwrap();
        assert!(state_lock.first_write_entered);
        trigger_tx.send(()).unwrap();
        ready_rx.recv().unwrap();
        let (mut state_lock, _) = state
            .1
            .wait_timeout_while(state_lock, Duration::from_millis(200), |state| {
                state.write_entries < 2
            })
            .unwrap();
        assert_eq!(state_lock.write_entries, 1);
        state_lock.release_first_write = true;
        state.1.notify_all();
        drop(state_lock);

        let client = command_handle.join().unwrap();
        assert!(result_rx.recv().unwrap());
        {
            let state = state.0.lock().unwrap();
            let commands = state
                .writes
                .iter()
                .map(|bytes| decode_frame(bytes).unwrap().command_type)
                .collect::<Vec<_>>();
            assert_eq!(commands, vec![CommandType::Ping, CommandType::Heartbeat]);
            assert_eq!(state.max_concurrent_writes, 1);
        }
        drop(client);
    }

    #[test]
    fn open_asserts_dtr_and_heartbeat_is_sequence_zero_no_response() {
        let (port, state) = FakePort::new(&[]);
        let client = HidClient::from_port(
            Box::new(port),
            &options(0, Duration::from_millis(1)),
            Arc::new(std::thread::sleep),
        )
        .unwrap();

        let mut locked = state.0.lock().unwrap();
        let deadline = Instant::now() + Duration::from_millis(100);
        while locked.writes.is_empty() && Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            locked = state.1.wait_timeout(locked, remaining).unwrap().0;
        }
        assert_eq!(locked.dtr, vec![true]);
        let heartbeat = locked
            .writes
            .iter()
            .map(|bytes| decode_frame(bytes).unwrap())
            .find(|frame| frame.command_type == CommandType::Heartbeat)
            .expect("heartbeat frame");
        assert_eq!(heartbeat.sequence, 0);
        assert_eq!(heartbeat.flags, hid_protocol::protocol::FLAG_NO_RESPONSE);
        drop(locked);

        drop(client);
        assert_eq!(state.0.lock().unwrap().dtr, vec![true, false]);
    }
}
