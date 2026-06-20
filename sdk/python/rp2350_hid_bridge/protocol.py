from __future__ import annotations

from dataclasses import dataclass
from enum import IntEnum

MAGIC = b"\xA5\x5A"
PROTOCOL_VERSION = 1
MAX_PAYLOAD_SIZE = 240
FRAME_OVERHEAD = 11
MAX_FRAME_SIZE = FRAME_OVERHEAD + MAX_PAYLOAD_SIZE


class CommandType(IntEnum):
    PING = 0x01
    GET_INFO = 0x02
    GET_CAPS = 0x03
    KEY_DOWN = 0x10
    KEY_UP = 0x11
    KEY_TAP = 0x12
    TYPE_ASCII = 0x13
    MOUSE_MOVE_REL = 0x20
    MOUSE_BUTTON_DOWN = 0x21
    MOUSE_BUTTON_UP = 0x22
    MOUSE_CLICK = 0x23
    MOUSE_WHEEL = 0x24
    WAIT_MS = 0x30
    BATCH_BEGIN = 0x40
    BATCH_END = 0x41
    STOP_ALL = 0x7F
    ACK = 0x80
    NACK = 0x81
    STATUS = 0x82
    BUSY = 0x83


class DecodeError(ValueError):
    pass


@dataclass(frozen=True)
class Frame:
    version: int
    flags: int
    sequence: int
    command_type: CommandType | int
    payload: bytes


@dataclass(frozen=True)
class Response:
    command_type: CommandType | int
    payload: bytes
    sequence: int


def encode_frame(
    sequence: int,
    command_type: CommandType | int,
    payload: bytes = b"",
    version: int = PROTOCOL_VERSION,
) -> bytes:
    if len(payload) > MAX_PAYLOAD_SIZE:
        raise ValueError(f"payload is {len(payload)} bytes, max is {MAX_PAYLOAD_SIZE}")

    header = bytearray(FRAME_OVERHEAD + len(payload))
    header[0:2] = MAGIC
    header[2] = version & 0xFF
    header[3] = 0
    header[4:6] = int(sequence).to_bytes(2, "big")
    header[6] = int(command_type) & 0xFF
    header[7:9] = len(payload).to_bytes(2, "big")
    header[9 : 9 + len(payload)] = payload

    crc = crc16_ccitt_false(header[2 : 9 + len(payload)])
    header[9 + len(payload) : 11 + len(payload)] = crc.to_bytes(2, "big")
    return bytes(header)


def decode_frame(data: bytes | bytearray) -> Frame:
    frame = bytes(data)
    if len(frame) < FRAME_OVERHEAD:
        raise DecodeError("frame is too short")
    if frame[0:2] != MAGIC:
        raise DecodeError("bad magic")

    payload_len = int.from_bytes(frame[7:9], "big")
    if payload_len > MAX_PAYLOAD_SIZE:
        raise DecodeError("payload too long")

    expected_len = FRAME_OVERHEAD + payload_len
    if len(frame) != expected_len:
        raise DecodeError("length mismatch")

    crc_offset = 9 + payload_len
    expected_crc = int.from_bytes(frame[crc_offset : crc_offset + 2], "big")
    actual_crc = crc16_ccitt_false(frame[2:crc_offset])
    if expected_crc != actual_crc:
        raise DecodeError("bad crc")

    command_raw = frame[6]
    try:
        command_type: CommandType | int = CommandType(command_raw)
    except ValueError:
        command_type = command_raw

    return Frame(
        version=frame[2],
        flags=frame[3],
        sequence=int.from_bytes(frame[4:6], "big"),
        command_type=command_type,
        payload=frame[9:crc_offset],
    )


def expected_response_type(command_type: CommandType | int) -> CommandType:
    if command_type in (CommandType.GET_INFO, CommandType.GET_CAPS):
        return CommandType.STATUS
    return CommandType.ACK


def crc16_ccitt_false(data: bytes | bytearray) -> int:
    crc = 0xFFFF
    for byte in data:
        crc ^= byte << 8
        for _ in range(8):
            if crc & 0x8000:
                crc = ((crc << 1) ^ 0x1021) & 0xFFFF
            else:
                crc = (crc << 1) & 0xFFFF
    return crc


def ascii_payload(text: str) -> bytes:
    try:
        return text.encode("ascii")
    except UnicodeEncodeError as exc:
        raise ValueError("TYPE_ASCII only accepts ASCII text") from exc


def i16_pair_payload(dx: int, dy: int) -> bytes:
    return int(dx).to_bytes(2, "big", signed=True) + int(dy).to_bytes(2, "big", signed=True)


def u32_payload(value: int) -> bytes:
    return int(value).to_bytes(4, "big", signed=False)


def byte_payload(value: int) -> bytes:
    return bytes([int(value) & 0xFF])
