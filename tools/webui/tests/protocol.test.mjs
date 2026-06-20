import test from "node:test";
import assert from "node:assert/strict";

import {
  CommandType,
  DecodeError,
  decodeFrame,
  encodeFrame,
} from "../protocol.js";
import { parseCombo } from "../keys.js";
import { parseScript } from "../script.js";

test("encodes and decodes a ping frame", () => {
  const frame = encodeFrame(0x1234, CommandType.Ping, new Uint8Array());
  const decoded = decodeFrame(frame);

  assert.equal(decoded.version, 1);
  assert.equal(decoded.sequence, 0x1234);
  assert.equal(decoded.commandType, CommandType.Ping);
  assert.deepEqual(Array.from(decoded.payload), []);
});

test("rejects bad crc", () => {
  const frame = encodeFrame(7, CommandType.Ping, new Uint8Array());
  frame[frame.length - 1] ^= 0x55;

  assert.throws(() => decodeFrame(frame), DecodeError);
});

test("parses key combos", () => {
  assert.deepEqual(parseCombo("CTRL+C"), { modifier: 0x01, keycode: 0x06 });
  assert.deepEqual(parseCombo("SHIFT+R"), { modifier: 0x02, keycode: 0x15 });
  assert.deepEqual(parseCombo("ENTER"), { modifier: 0, keycode: 0x28 });
  assert.deepEqual(parseCombo("F5"), { modifier: 0, keycode: 0x3e });
});

test("parses script commands", () => {
  const commands = parseScript(`
type "abc"
key tap ENTER
mouse move 10 -5
wait 100
stop
`);

  assert.equal(commands.length, 5);
  assert.equal(commands[0].kind, "type");
  assert.equal(commands[0].text, "abc");
  assert.equal(commands[1].kind, "key");
  assert.equal(commands[1].action, "tap");
  assert.equal(commands[2].kind, "mouse");
  assert.equal(commands[2].action, "move");
  assert.equal(commands[2].dx, 10);
  assert.equal(commands[2].dy, -5);
  assert.deepEqual(commands[3], { kind: "wait", ms: 100 });
  assert.deepEqual(commands[4], { kind: "stop" });
});
