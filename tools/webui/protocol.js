export const PROTOCOL_VERSION = 1;
export const MAGIC = [0xa5, 0x5a];
export const MAX_PAYLOAD_SIZE = 240;
export const FRAME_OVERHEAD = 11;
export const MAX_FRAME_SIZE = FRAME_OVERHEAD + MAX_PAYLOAD_SIZE;

export const CommandType = Object.freeze({
  Ping: 0x01,
  GetInfo: 0x02,
  GetCaps: 0x03,
  KeyDown: 0x10,
  KeyUp: 0x11,
  KeyTap: 0x12,
  TypeAscii: 0x13,
  MouseMoveRel: 0x20,
  MouseButtonDown: 0x21,
  MouseButtonUp: 0x22,
  MouseClick: 0x23,
  MouseWheel: 0x24,
  WaitMs: 0x30,
  BatchBegin: 0x40,
  BatchEnd: 0x41,
  StopAll: 0x7f,
  Ack: 0x80,
  Nack: 0x81,
  Status: 0x82,
  Busy: 0x83,
});

export const CommandName = Object.freeze(
  Object.fromEntries(Object.entries(CommandType).map(([name, value]) => [value, name])),
);

export class DecodeError extends Error {
  constructor(code, message) {
    super(message);
    this.name = "DecodeError";
    this.code = code;
  }
}

export function encodeFrame(sequence, commandType, payload = new Uint8Array()) {
  const body = toBytes(payload);
  if (body.length > MAX_PAYLOAD_SIZE) {
    throw new RangeError(`payload is ${body.length} bytes, max is ${MAX_PAYLOAD_SIZE}`);
  }

  const frame = new Uint8Array(FRAME_OVERHEAD + body.length);
  frame[0] = MAGIC[0];
  frame[1] = MAGIC[1];
  frame[2] = PROTOCOL_VERSION;
  frame[3] = 0;
  writeU16(frame, 4, sequence);
  frame[6] = commandType;
  writeU16(frame, 7, body.length);
  frame.set(body, 9);

  const crc = crc16CcittFalse(frame.subarray(2, 9 + body.length));
  writeU16(frame, 9 + body.length, crc);
  return frame;
}

export function decodeFrame(input) {
  const frame = toBytes(input);
  if (frame.length < FRAME_OVERHEAD) {
    throw new DecodeError("too_short", "frame is too short");
  }
  if (frame[0] !== MAGIC[0] || frame[1] !== MAGIC[1]) {
    throw new DecodeError("bad_magic", "frame magic does not match");
  }

  const payloadLength = readU16(frame, 7);
  if (payloadLength > MAX_PAYLOAD_SIZE) {
    throw new DecodeError("payload_too_long", "payload is too long");
  }

  const expectedLength = FRAME_OVERHEAD + payloadLength;
  if (frame.length !== expectedLength) {
    throw new DecodeError("length_mismatch", "frame length does not match payload length");
  }

  const crcOffset = 9 + payloadLength;
  const expectedCrc = readU16(frame, crcOffset);
  const actualCrc = crc16CcittFalse(frame.subarray(2, crcOffset));
  if (expectedCrc !== actualCrc) {
    throw new DecodeError("bad_crc", "frame CRC check failed");
  }

  return {
    version: frame[2],
    flags: frame[3],
    sequence: readU16(frame, 4),
    commandType: frame[6],
    payload: frame.slice(9, crcOffset),
  };
}

export function extractFrames(buffer) {
  const frames = [];
  let offset = 0;

  while (buffer.length - offset >= 2) {
    if (buffer[offset] !== MAGIC[0] || buffer[offset + 1] !== MAGIC[1]) {
      offset += 1;
      continue;
    }

    if (buffer.length - offset < 9) {
      break;
    }

    const payloadLength = readU16(buffer, offset + 7);
    const frameLength = FRAME_OVERHEAD + payloadLength;
    if (frameLength > MAX_FRAME_SIZE) {
      offset += 2;
      continue;
    }
    if (buffer.length - offset < frameLength) {
      break;
    }

    frames.push(buffer.slice(offset, offset + frameLength));
    offset += frameLength;
  }

  return {
    frames,
    remaining: buffer.slice(offset),
  };
}

export function expectedResponseType(commandType) {
  if (commandType === CommandType.GetInfo || commandType === CommandType.GetCaps) {
    return CommandType.Status;
  }
  return CommandType.Ack;
}

export function crc16CcittFalse(data) {
  let crc = 0xffff;
  for (const byte of data) {
    crc ^= byte << 8;
    for (let i = 0; i < 8; i += 1) {
      if ((crc & 0x8000) !== 0) {
        crc = ((crc << 1) ^ 0x1021) & 0xffff;
      } else {
        crc = (crc << 1) & 0xffff;
      }
    }
  }
  return crc;
}

export function bytesToHex(bytes) {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0").toUpperCase()).join(" ");
}

export function asciiPayload(text) {
  const out = new Uint8Array(text.length);
  for (let i = 0; i < text.length; i += 1) {
    const code = text.charCodeAt(i);
    if (code > 0x7f) {
      throw new Error("TYPE_ASCII only accepts ASCII text");
    }
    out[i] = code;
  }
  return out;
}

export function i16PairPayload(dx, dy) {
  const payload = new Uint8Array(4);
  writeI16(payload, 0, dx);
  writeI16(payload, 2, dy);
  return payload;
}

export function u32Payload(value) {
  const payload = new Uint8Array(4);
  payload[0] = (value >>> 24) & 0xff;
  payload[1] = (value >>> 16) & 0xff;
  payload[2] = (value >>> 8) & 0xff;
  payload[3] = value & 0xff;
  return payload;
}

export function keyPayload(combo) {
  return new Uint8Array([combo.modifier, combo.keycode]);
}

export function bytePayload(value) {
  return new Uint8Array([value & 0xff]);
}

function toBytes(input) {
  if (input instanceof Uint8Array) {
    return input;
  }
  if (input instanceof ArrayBuffer) {
    return new Uint8Array(input);
  }
  if (Array.isArray(input)) {
    return new Uint8Array(input);
  }
  throw new TypeError("expected bytes");
}

function readU16(bytes, offset) {
  return (bytes[offset] << 8) | bytes[offset + 1];
}

function writeU16(bytes, offset, value) {
  bytes[offset] = (value >>> 8) & 0xff;
  bytes[offset + 1] = value & 0xff;
}

function writeI16(bytes, offset, value) {
  const signed = value < 0 ? 0x10000 + value : value;
  writeU16(bytes, offset, signed & 0xffff);
}
