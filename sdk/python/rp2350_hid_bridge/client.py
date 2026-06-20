from __future__ import annotations

import time
from dataclasses import dataclass

from .keys import parse_combo
from .protocol import (
    MAGIC,
    CommandType,
    DecodeError,
    Response,
    ascii_payload,
    byte_payload,
    decode_frame,
    encode_frame,
    expected_response_type,
    i16_pair_payload,
    u32_payload,
)
from .script import ScriptCommand, mouse_button_mask, parse_script

DEFAULT_VID = 0xCAFE
DEFAULT_PID = 0x2350


@dataclass
class HidBridgeOptions:
    port: str | None = None
    baudrate: int = 115200
    timeout: float = 1.0
    retries: int = 2
    vid: int = DEFAULT_VID
    pid: int = DEFAULT_PID


class HidBridge:
    def __init__(self, options: HidBridgeOptions | None = None):
        self.options = options or HidBridgeOptions()
        self._serial = None
        self._sequence = 1

    def __enter__(self) -> "HidBridge":
        self.open()
        return self

    def __exit__(self, *_exc) -> None:
        self.close()

    def open(self) -> None:
        serial_mod = _serial_module()
        port = self.options.port or find_port(self.options.vid, self.options.pid)
        if not port:
            raise RuntimeError("RP2350 HID bridge serial port not found")
        self._serial = serial_mod.Serial(
            port=port,
            baudrate=self.options.baudrate,
            timeout=self.options.timeout,
            write_timeout=self.options.timeout,
        )

    def close(self) -> None:
        if self._serial is not None:
            self._serial.close()
            self._serial = None

    def send_command(self, command_type: CommandType, payload: bytes = b"") -> Response:
        serial_obj = self._require_open()
        sequence = self._next_sequence()
        frame = encode_frame(sequence, command_type, payload)
        last_error: Exception | None = None

        for attempt in range(self.options.retries + 1):
            try:
                serial_obj.reset_input_buffer()
                serial_obj.write(frame)
                serial_obj.flush()
                response = self._read_response(sequence)
                if response.command_type == CommandType.BUSY and attempt < self.options.retries:
                    time.sleep(0.05)
                    continue
                if response.command_type == CommandType.NACK:
                    code = response.payload[0] if response.payload else 0
                    raise RuntimeError(f"device returned NACK error code {code}")
                expected = expected_response_type(command_type)
                if response.command_type != expected:
                    raise RuntimeError(f"unexpected response {response.command_type}, expected {expected}")
                return response
            except Exception as exc:
                last_error = exc
                if attempt >= self.options.retries:
                    raise
        raise RuntimeError("command failed") from last_error

    def ping(self) -> None:
        self.send_command(CommandType.PING)

    def info(self) -> bytes:
        return self.send_command(CommandType.GET_INFO).payload

    def caps(self) -> bytes:
        return self.send_command(CommandType.GET_CAPS).payload

    def type_text(self, text: str) -> None:
        self.send_command(CommandType.TYPE_ASCII, ascii_payload(text))

    def key_tap(self, combo: str) -> None:
        self._send_key(CommandType.KEY_TAP, combo)

    def key_down(self, combo: str) -> None:
        self._send_key(CommandType.KEY_DOWN, combo)

    def key_up(self, combo: str) -> None:
        self._send_key(CommandType.KEY_UP, combo)

    def mouse_move(self, dx: int, dy: int) -> None:
        self.send_command(CommandType.MOUSE_MOVE_REL, i16_pair_payload(dx, dy))

    def mouse_click(self, button: str = "left") -> None:
        self.send_command(CommandType.MOUSE_CLICK, byte_payload(mouse_button_mask(button)))

    def mouse_down(self, button: str = "left") -> None:
        self.send_command(CommandType.MOUSE_BUTTON_DOWN, byte_payload(mouse_button_mask(button)))

    def mouse_up(self, button: str = "left") -> None:
        self.send_command(CommandType.MOUSE_BUTTON_UP, byte_payload(mouse_button_mask(button)))

    def mouse_wheel(self, delta: int) -> None:
        self.send_command(CommandType.MOUSE_WHEEL, byte_payload(delta))

    def wait_ms(self, ms: int) -> None:
        self.send_command(CommandType.WAIT_MS, u32_payload(ms))

    def stop_all(self) -> None:
        self.send_command(CommandType.STOP_ALL)

    def run_script(self, text: str) -> None:
        commands = parse_script(text)
        self.send_command(CommandType.BATCH_BEGIN)
        try:
            for command in commands:
                self._execute_script_command(command)
            self.send_command(CommandType.BATCH_END)
        except Exception:
            try:
                self.stop_all()
            finally:
                raise

    def _send_key(self, command_type: CommandType, combo: str) -> None:
        modifier, keycode = parse_combo(combo)
        self.send_command(command_type, bytes([modifier, keycode]))

    def _execute_script_command(self, command: ScriptCommand) -> None:
        if command.kind == "type" and command.text is not None:
            self.type_text(command.text)
        elif command.kind == "key" and command.action and command.combo:
            combo = bytes(command.combo)
            command_type = {
                "tap": CommandType.KEY_TAP,
                "down": CommandType.KEY_DOWN,
                "up": CommandType.KEY_UP,
            }[command.action]
            self.send_command(command_type, combo)
        elif command.kind == "mouse" and command.action == "move":
            self.mouse_move(command.dx or 0, command.dy or 0)
        elif command.kind == "mouse" and command.action == "click":
            self.send_command(CommandType.MOUSE_CLICK, byte_payload(command.button or 0))
        elif command.kind == "mouse" and command.action == "down":
            self.send_command(CommandType.MOUSE_BUTTON_DOWN, byte_payload(command.button or 0))
        elif command.kind == "mouse" and command.action == "up":
            self.send_command(CommandType.MOUSE_BUTTON_UP, byte_payload(command.button or 0))
        elif command.kind == "mouse" and command.action == "wheel":
            self.mouse_wheel(command.delta or 0)
        elif command.kind == "wait" and command.ms is not None:
            self.wait_ms(command.ms)
        elif command.kind == "stop":
            self.stop_all()
        else:
            raise ValueError(f"unsupported script command {command}")

    def _read_response(self, expected_sequence: int) -> Response:
        serial_obj = self._require_open()
        deadline = time.monotonic() + self.options.timeout
        buffer = bytearray()
        while time.monotonic() < deadline:
            chunk = serial_obj.read(64)
            if chunk:
                buffer.extend(chunk)
                response = _try_decode_response(buffer, expected_sequence)
                if response is not None:
                    return response
        raise TimeoutError("timed out waiting for response")

    def _next_sequence(self) -> int:
        sequence = self._sequence
        self._sequence = (self._sequence + 1) & 0xFFFF
        if self._sequence == 0:
            self._sequence = 1
        return sequence

    def _require_open(self):
        if self._serial is None:
            raise RuntimeError("serial port is not open")
        return self._serial


def list_ports():
    ports_mod = _list_ports_module()
    return list(ports_mod.comports())


def find_port(vid: int = DEFAULT_VID, pid: int = DEFAULT_PID) -> str | None:
    for port in list_ports():
        if getattr(port, "vid", None) == vid and getattr(port, "pid", None) == pid:
            return port.device
    return None


def _try_decode_response(buffer: bytearray, expected_sequence: int) -> Response | None:
    while len(buffer) >= 2:
        if bytes(buffer[:2]) != MAGIC:
            del buffer[0]
            continue
        if len(buffer) < 9:
            return None
        payload_len = int.from_bytes(buffer[7:9], "big")
        frame_len = 11 + payload_len
        if len(buffer) < frame_len:
            return None
        frame_bytes = bytes(buffer[:frame_len])
        del buffer[:frame_len]
        try:
            frame = decode_frame(frame_bytes)
        except DecodeError:
            continue
        if frame.sequence != expected_sequence:
            continue
        return Response(frame.command_type, frame.payload, frame.sequence)
    return None


def _serial_module():
    try:
        import serial
    except ImportError as exc:
        raise RuntimeError("install pyserial to use the serial client: pip install pyserial") from exc
    return serial


def _list_ports_module():
    try:
        from serial.tools import list_ports
    except ImportError as exc:
        raise RuntimeError("install pyserial to enumerate serial ports: pip install pyserial") from exc
    return list_ports
