import test from "node:test";
import assert from "node:assert/strict";

import {
  CommandType,
  DecodeError,
  FLAG_NO_RESPONSE,
  decodeFrame,
  encodeFrame,
  asciiPayload,
  i16PairPayload,
  u32Payload,
} from "../protocol.js";
import { parseCombo } from "../keys.js";
import { parseScript } from "../script.js";
import { SerialHidClient } from "../app.js";

function fakeTimers(events = []) {
  let nextId = 1;
  const timeouts = new Map();
  const intervals = new Map();
  return {
    api: {
      setTimeout(callback, delay) {
        const id = nextId++;
        timeouts.set(id, { callback, delay });
        return id;
      },
      clearTimeout(id) {
        timeouts.delete(id);
      },
      setInterval(callback, delay) {
        const id = nextId++;
        intervals.set(id, { callback, delay });
        return id;
      },
      clearInterval(id) {
        if (intervals.delete(id)) {
          events.push("heartbeat:stopped");
        }
      },
    },
    delays() {
      return Array.from(timeouts.values(), ({ delay }) => delay);
    },
    runTimeout(delay) {
      const found = Array.from(timeouts.entries()).find(([, timer]) => timer.delay === delay);
      assert.ok(found, `missing timeout scheduled for ${delay} ms`);
      const [id, timer] = found;
      timeouts.delete(id);
      timer.callback();
    },
    runInterval(delay) {
      const timer = Array.from(intervals.values()).find((candidate) => candidate.delay === delay);
      assert.ok(timer, `missing interval scheduled for ${delay} ms`);
      timer.callback();
    },
  };
}

function fakeSerial({ readable = false } = {}) {
  const events = [];
  const writes = [];
  let finishRead = null;
  const reader = {
    read() {
      return new Promise((resolve) => {
        finishRead = resolve;
      });
    },
    async cancel() {
      events.push("reader:cancel");
      finishRead?.({ value: undefined, done: true });
    },
    releaseLock() {
      events.push("reader:release");
    },
  };
  const writer = {
    async write(frame) {
      const copy = frame.slice();
      writes.push(copy);
      events.push(`write:${decodeFrame(copy).commandType}`);
    },
    releaseLock() {
      events.push("writer:release");
    },
  };
  const port = {
    readable: readable ? { getReader: () => reader } : null,
    writable: { getWriter: () => writer },
    async open() {
      events.push("open");
    },
    async setSignals({ dataTerminalReady }) {
      events.push(`dtr:${dataTerminalReady}`);
    },
    async close() {
      events.push("close");
    },
  };
  return {
    serial: { requestPort: async () => port },
    port,
    writes,
    events,
  };
}

async function flushMicrotasks() {
  for (let index = 0; index < 8; index += 1) {
    await Promise.resolve();
  }
}

function responseFrame(sequence, commandType, payload = new Uint8Array()) {
  return encodeFrame(sequence, commandType, payload);
}

function makeClient(serialState, timers, options = {}) {
  return new SerialHidClient({
    serial: serialState.serial,
    timers: timers.api,
    now: () => 0,
    log: () => {},
    responseTimeoutMs: 100,
    retries: 1,
    heartbeatIntervalMs: 500,
    ...options,
  });
}

test("encodes and decodes a ping frame", () => {
  const frame = encodeFrame(0x1234, CommandType.Ping, new Uint8Array());
  const decoded = decodeFrame(frame);

  assert.equal(decoded.version, 2);
  assert.equal(decoded.sequence, 0x1234);
  assert.equal(decoded.commandType, CommandType.Ping);
  assert.deepEqual(Array.from(decoded.payload), []);
});

test("encodes protocol v2 heartbeat flags", () => {
  const frame = encodeFrame(0, CommandType.Heartbeat, new Uint8Array(), FLAG_NO_RESPONSE);
  const decoded = decodeFrame(frame);

  assert.equal(decoded.version, 2);
  assert.equal(decoded.flags, FLAG_NO_RESPONSE);
  assert.equal(decoded.sequence, 0);
  assert.equal(decoded.commandType, CommandType.Heartbeat);
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
  assert.deepEqual(parseCombo("SHIFT"), { modifier: 0x02, keycode: 0 });
  assert.deepEqual(parseCombo("CTRL+SHIFT"), { modifier: 0x03, keycode: 0 });
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

test("connect asserts DTR and heartbeat writes no-response without pending", async () => {
  const serialState = fakeSerial();
  const timers = fakeTimers(serialState.events);
  const client = makeClient(serialState, timers);

  await client.connect();
  assert.ok(serialState.events.includes("dtr:true"));

  timers.runInterval(500);
  await flushMicrotasks();

  const heartbeat = decodeFrame(serialState.writes.at(-1));
  assert.equal(heartbeat.sequence, 0);
  assert.equal(heartbeat.commandType, CommandType.Heartbeat);
  assert.equal(heartbeat.flags, FLAG_NO_RESPONSE);
  assert.equal(client.pending.size, 0);

  await client.disconnect();
});

test("NACK is terminal and writes the request only once", async () => {
  const serialState = fakeSerial();
  const timers = fakeTimers(serialState.events);
  const client = makeClient(serialState, timers, { retries: 3 });
  await client.connect();

  const request = client.send(CommandType.KeyDown, new Uint8Array([0, 0x1a]));
  await flushMicrotasks();
  const nack = responseFrame(1, CommandType.Nack, new Uint8Array([15]));
  client.acceptFrame(decodeFrame(nack), nack);

  await assert.rejects(request, /NACK 15/);
  assert.equal(serialState.writes.length, 1);
  await client.disconnect();
});

test("BUSY keeps pending, honors big-endian delay, and retries identical bytes", async () => {
  const serialState = fakeSerial();
  const timers = fakeTimers(serialState.events);
  const client = makeClient(serialState, timers);
  await client.connect();

  const request = client.send(CommandType.KeyDown, new Uint8Array([0, 0x1a]));
  await flushMicrotasks();
  const busy = responseFrame(1, CommandType.Busy, new Uint8Array([3, 0, 25]));
  client.acceptFrame(decodeFrame(busy), busy);

  assert.equal(client.pending.size, 1);
  assert.ok(timers.delays().includes(25));
  timers.runTimeout(25);
  await flushMicrotasks();
  assert.equal(serialState.writes.length, 2);
  assert.deepEqual(serialState.writes[1], serialState.writes[0]);

  const ack = responseFrame(1, CommandType.Ack);
  client.acceptFrame(decodeFrame(ack), ack);
  await request;
  await client.disconnect();
});

test("response timeout retries the identical frame and sequence", async () => {
  const serialState = fakeSerial();
  const timers = fakeTimers(serialState.events);
  const client = makeClient(serialState, timers);
  await client.connect();

  const request = client.send(CommandType.Ping);
  await flushMicrotasks();
  assert.ok(timers.delays().includes(1000));
  timers.runTimeout(1000);
  await flushMicrotasks();

  assert.equal(serialState.writes.length, 2);
  assert.deepEqual(serialState.writes[1], serialState.writes[0]);
  assert.equal(decodeFrame(serialState.writes[1]).sequence, 1);
  const ack = responseFrame(1, CommandType.Ack);
  client.acceptFrame(decodeFrame(ack), ack);
  await request;
  await client.disconnect();
});

test("response deadlines include wait, text, movement, and accumulated batch duration", () => {
  const serialState = fakeSerial();
  const timers = fakeTimers(serialState.events);
  const client = makeClient(serialState, timers);

  assert.equal(client.responseTimeoutFor(CommandType.Ping, new Uint8Array()), 1000);
  assert.equal(client.responseTimeoutFor(CommandType.WaitMs, u32Payload(2500)), 3000);
  assert.equal(client.responseTimeoutFor(CommandType.TypeAscii, asciiPayload("abcdefghij")), 580);
  assert.equal(client.responseTimeoutFor(CommandType.MouseMoveRel, i16PairPayload(300, -300)), 503);

  client.recordCompletedCommand(CommandType.BatchBegin, new Uint8Array());
  client.recordCompletedCommand(CommandType.TypeAscii, asciiPayload("abcdefghij"));
  client.recordCompletedCommand(CommandType.MouseMoveRel, i16PairPayload(300, -300));
  client.recordCompletedCommand(CommandType.WaitMs, u32Payload(2500));
  assert.equal(client.responseTimeoutFor(CommandType.BatchEnd, new Uint8Array()), 3583);
});

test("disconnect orders STOP_ALL before heartbeat stop, DTR low, and close", async () => {
  const serialState = fakeSerial({ readable: true });
  const timers = fakeTimers(serialState.events);
  const client = makeClient(serialState, timers);
  await client.connect();
  client.recordCompletedCommand(CommandType.BatchBegin, new Uint8Array());
  client.recordCompletedCommand(CommandType.WaitMs, u32Payload(2500));

  const request = client.send(CommandType.Ping);
  await flushMicrotasks();
  const start = serialState.events.length;

  const disconnect = client.disconnect();
  await assert.rejects(request, /serial disconnected/);
  await disconnect;

  const events = serialState.events.slice(start);
  const stopIndex = events.indexOf(`write:${CommandType.StopAll}`);
  const heartbeatStopIndex = events.indexOf("heartbeat:stopped");
  const readerCancelIndex = events.indexOf("reader:cancel");
  const dtrLowIndex = events.indexOf("dtr:false");
  const writerReleaseIndex = events.indexOf("writer:release");
  const closeIndex = events.indexOf("close");
  assert.ok(stopIndex >= 0);
  assert.ok(stopIndex < heartbeatStopIndex);
  assert.ok(heartbeatStopIndex < readerCancelIndex);
  assert.ok(readerCancelIndex < dtrLowIndex);
  assert.ok(dtrLowIndex < writerReleaseIndex);
  assert.ok(writerReleaseIndex < closeIndex);
  assert.equal(client.pending.size, 0);
  assert.equal(client.responseTimeoutFor(CommandType.BatchEnd, new Uint8Array()), 1000);
  await assert.rejects(client.send(CommandType.Ping), /serial is not connected/);
});
