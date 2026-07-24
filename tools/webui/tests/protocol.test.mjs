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
import { parseScript, planScriptCommands, runScriptCommands } from "../script.js";
import { SerialHidClient, handlePhysicalDisconnect } from "../app.js";

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
    callbackFor(delay) {
      const timer = Array.from(timeouts.values()).find((candidate) => candidate.delay === delay);
      assert.ok(timer, `missing timeout scheduled for ${delay} ms`);
      return timer.callback;
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
    intervalCallbackFor(delay) {
      const timer = Array.from(intervals.values()).find((candidate) => candidate.delay === delay);
      assert.ok(timer, `missing interval scheduled for ${delay} ms`);
      return timer.callback;
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

function serialWithInitializationFailure(stage) {
  const failedEvents = [];
  const failedWriter = {
    releaseLock() {
      failedEvents.push("writer:release");
    },
  };
  const failedPort = {
    readable: null,
    writable: {
      getWriter() {
        failedEvents.push("writer:get");
        if (stage === "getWriter") {
          throw new Error("getWriter failed");
        }
        return failedWriter;
      },
    },
    async open() {
      failedEvents.push("open");
      if (stage === "open") {
        throw new Error("open failed");
      }
    },
    async setSignals({ dataTerminalReady }) {
      failedEvents.push(`dtr:${dataTerminalReady}`);
      if (stage === "setSignals" && dataTerminalReady) {
        throw new Error("setSignals failed after asserting DTR");
      }
    },
    async close() {
      failedEvents.push("close");
    },
  };
  const healthy = fakeSerial();
  const ports = [failedPort, healthy.port];

  return {
    serial: {
      async requestPort() {
        return ports.shift();
      },
    },
    failedEvents,
    healthy,
  };
}

function gatedSerial() {
  const events = [];
  const writes = [];
  let releaseFirstWrite;
  let markFirstWriteStarted;
  const firstWriteStarted = new Promise((resolve) => {
    markFirstWriteStarted = resolve;
  });
  const firstWriteGate = new Promise((resolve) => {
    releaseFirstWrite = resolve;
  });
  let writeCount = 0;
  let activeWrites = 0;
  let maxConcurrentWrites = 0;
  const writer = {
    async write(frame) {
      const index = writeCount;
      writeCount += 1;
      activeWrites += 1;
      maxConcurrentWrites = Math.max(maxConcurrentWrites, activeWrites);
      try {
        if (index === 0) {
          markFirstWriteStarted();
          await firstWriteGate;
        }
        const copy = frame.slice();
        writes.push(copy);
        events.push(`write:${decodeFrame(copy).commandType}`);
      } finally {
        activeWrites -= 1;
      }
    },
    releaseLock() {
      events.push("writer:release");
    },
  };
  const port = {
    readable: null,
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
    writes,
    events,
    firstWriteStarted,
    releaseFirstWrite,
    get maxConcurrentWrites() {
      return maxConcurrentWrites;
    },
  };
}

function hungSerial() {
  const events = [];
  const writes = [];
  let markFirstWriteStarted;
  const firstWriteStarted = new Promise((resolve) => {
    markFirstWriteStarted = resolve;
  });
  let releaseFirstWrite;
  const firstWriteGate = new Promise((resolve) => {
    releaseFirstWrite = resolve;
  });
  const neverRead = new Promise(() => {});
  let writeCount = 0;
  const reader = {
    read() {
      events.push("reader:read");
      return neverRead;
    },
    async cancel() {
      events.push("reader:cancel");
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
      writeCount += 1;
      if (writeCount === 1) {
        markFirstWriteStarted();
        await firstWriteGate;
      }
    },
    releaseLock() {
      events.push("writer:release");
    },
  };
  const port = {
    readable: { getReader: () => reader },
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
    firstWriteStarted,
    releaseFirstWrite,
  };
}

function serialWithHungCleanup(stage) {
  const events = [];
  const never = new Promise(() => {});
  let finishRead = null;
  const reader = {
    read() {
      return new Promise((resolve) => {
        finishRead = resolve;
      });
    },
    cancel() {
      events.push("reader:cancel");
      finishRead?.({ value: undefined, done: true });
      return stage === "cancel" ? never : Promise.resolve();
    },
    releaseLock() {
      events.push("reader:release");
    },
  };
  const writer = {
    async write(frame) {
      events.push(`write:${decodeFrame(frame).commandType}`);
    },
    releaseLock() {
      events.push("writer:release");
    },
  };
  const port = {
    readable: { getReader: () => reader },
    writable: { getWriter: () => writer },
    async open() {
      events.push("open");
    },
    setSignals({ dataTerminalReady }) {
      events.push(`dtr:${dataTerminalReady}`);
      return stage === "dtr" && !dataTerminalReady ? never : Promise.resolve();
    },
    close() {
      events.push("close");
      return stage === "close" ? never : Promise.resolve();
    },
  };
  return {
    serial: { requestPort: async () => port },
    port,
    events,
  };
}

async function flushMicrotasks() {
  for (let index = 0; index < 40; index += 1) {
    await Promise.resolve();
  }
}

async function flushUntil(predicate) {
  for (let index = 0; index < 40; index += 1) {
    if (predicate()) {
      return;
    }
    await Promise.resolve();
  }
  assert.fail("condition did not become true while flushing microtasks");
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

test("plans STOP as a boundary between non-empty script batches", () => {
  const plan = planScriptCommands(parseScript(`
wait 10
stop
wait 20
`));

  assert.deepEqual(
    plan.map((packet) => packet.commandType),
    [
      CommandType.BatchBegin,
      CommandType.WaitMs,
      CommandType.BatchEnd,
      CommandType.StopAll,
      CommandType.BatchBegin,
      CommandType.WaitMs,
      CommandType.BatchEnd,
    ],
  );
  assert.deepEqual(plan[1].payload, u32Payload(10));
  assert.deepEqual(plan[5].payload, u32Payload(20));
});

test("script planner does not emit empty batches around STOP", () => {
  const plan = planScriptCommands(parseScript(`
stop
stop
wait 10
stop
`));

  assert.deepEqual(
    plan.map((packet) => packet.commandType),
    [
      CommandType.StopAll,
      CommandType.StopAll,
      CommandType.BatchBegin,
      CommandType.WaitMs,
      CommandType.BatchEnd,
      CommandType.StopAll,
    ],
  );
  assert.deepEqual(planScriptCommands([]), []);
});

test("script runner stops without continuing after command error", async () => {
  const sent = [];
  const original = new Error("original command error");

  await assert.rejects(
    runScriptCommands(parseScript("wait 10\nwait 20"), async (commandType) => {
      sent.push(commandType);
      if (sent.length === 2) {
        throw original;
      }
    }),
    (error) => error === original,
  );
  assert.deepEqual(sent, [CommandType.BatchBegin, CommandType.WaitMs, CommandType.StopAll]);
});

test("script runner stops and preserves BatchEnd error", async () => {
  const sent = [];
  const original = new Error("original batch error");

  await assert.rejects(
    runScriptCommands(parseScript("wait 10"), async (commandType) => {
      sent.push(commandType);
      if (commandType === CommandType.BatchEnd) {
        throw original;
      }
    }),
    (error) => error === original,
  );
  assert.deepEqual(sent, [
    CommandType.BatchBegin,
    CommandType.WaitMs,
    CommandType.BatchEnd,
    CommandType.StopAll,
  ]);
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

for (const failure of [
  {
    stage: "open",
    error: /open failed/,
    expectedEvents: ["open"],
  },
  {
    stage: "getWriter",
    error: /getWriter failed/,
    expectedEvents: ["open", "writer:get", "close"],
  },
  {
    stage: "setSignals",
    error: /setSignals failed after asserting DTR/,
    expectedEvents: ["open", "writer:get", "dtr:true", "dtr:false", "writer:release", "close"],
  },
]) {
  test(`connect rolls back when ${failure.stage} fails and permits a later connection`, async () => {
    const serialState = serialWithInitializationFailure(failure.stage);
    const timers = fakeTimers();
    const client = makeClient(serialState, timers);

    await assert.rejects(client.connect(), failure.error);
    assert.equal(client.port, null);
    assert.equal(client.writer, null);
    assert.equal(client.reader, null);
    assert.equal(client.connected, false);
    assert.equal(client.heartbeatTimer, null);
    assert.equal(client.readTask, null);
    assert.equal(client.pending.size, 0);
    assert.deepEqual(serialState.failedEvents, failure.expectedEvents);

    await client.connect();
    assert.equal(client.connected, true);
    assert.equal(client.port, serialState.healthy.port);
    await client.disconnect();
    assert.equal(client.connected, false);
    assert.equal(client.port, null);
    assert.ok(serialState.healthy.events.includes("close"));
  });
}

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

test("disconnect immediately invalidates pending work and bounds hung write and read cleanup", async () => {
  const serialState = hungSerial();
  const timers = fakeTimers(serialState.events);
  const client = makeClient(serialState, timers, { cleanupTimeoutMs: 25 });
  await client.connect();

  const request = client.send(CommandType.Ping);
  const rejectedRequest = assert.rejects(request, /serial disconnected/);
  await serialState.firstWriteStarted;

  const disconnect = client.disconnect();
  assert.equal(client.connected, false);
  assert.equal(client.pending.size, 0);
  await rejectedRequest;

  timers.runTimeout(25);
  await flushMicrotasks();
  assert.ok(serialState.events.includes("reader:cancel"));
  timers.runTimeout(25);
  await disconnect;

  assert.ok(serialState.events.includes("dtr:false"));
  assert.ok(serialState.events.includes("writer:release"));
  assert.ok(serialState.events.includes("close"));
  assert.equal(serialState.events.filter((event) => event === "reader:cancel").length, 1);
  assert.equal(serialState.events.filter((event) => event === "writer:release").length, 1);
  assert.equal(serialState.events.filter((event) => event === "close").length, 1);
});

test("concurrent manual and physical disconnects share one cleanup", async () => {
  const serialState = hungSerial();
  const timers = fakeTimers(serialState.events);
  const client = makeClient(serialState, timers, { cleanupTimeoutMs: 25 });
  const ui = [];
  await client.connect();

  const request = client.send(CommandType.Ping);
  const rejectedRequest = assert.rejects(request, /serial disconnected/);
  await serialState.firstWriteStarted;
  const expectedSession = client.sessionSnapshot();

  let firstSettled = false;
  let secondSettled = false;
  let physicalSettled = false;
  const physical = handlePhysicalDisconnect(
    client,
    expectedSession,
    (connected) => ui.push(`connected:${connected}`),
    (level, label, message) => ui.push(`${level}:${label}:${message}`),
  ).then(() => { physicalSettled = true; });
  const first = client.disconnect().then(() => { firstSettled = true; });
  const second = client.disconnect().then(() => { secondSettled = true; });
  await rejectedRequest;
  await flushMicrotasks();

  assert.equal(firstSettled, false);
  assert.equal(secondSettled, false);
  assert.equal(physicalSettled, false);

  timers.runTimeout(25);
  await flushMicrotasks();
  timers.runTimeout(25);
  await Promise.all([first, second, physical]);

  assert.equal(serialState.events.filter((event) => event === "reader:cancel").length, 1);
  assert.equal(serialState.events.filter((event) => event === "reader:release").length, 0);
  assert.equal(serialState.events.filter((event) => event === "writer:release").length, 1);
  assert.equal(serialState.events.filter((event) => event === "close").length, 1);
  assert.deepEqual(ui.slice(-2), ["connected:false", "info:DETACH:serial device removed"]);
});

test("reconnect waits for old cleanup and stale callbacks cannot affect the new session", async () => {
  const first = hungSerial();
  const second = fakeSerial();
  const ports = [first.port, second.port];
  const timers = fakeTimers();
  const client = makeClient({
    serial: { requestPort: async () => ports.shift() },
  }, timers, { cleanupTimeoutMs: 25 });
  await client.connect();

  const staleHeartbeat = timers.intervalCallbackFor(500);
  const oldRequest = client.send(CommandType.Ping);
  const rejectedOldRequest = assert.rejects(oldRequest, /serial disconnected/);
  await first.firstWriteStarted;
  const oldPending = client.pending.get(1);
  client.armResponseTimeout(1, oldPending);
  const staleTimeout = timers.callbackFor(1000);
  const busy = responseFrame(1, CommandType.Busy, new Uint8Array([3, 0, 40]));
  client.acceptFrame(decodeFrame(busy), busy);
  const staleRetry = timers.callbackFor(40);
  staleHeartbeat();
  await flushMicrotasks();

  const disconnect = client.disconnect();
  client.serial = { requestPort: async () => ports.shift() };
  const reconnect = client.connect();
  await rejectedOldRequest;
  await flushMicrotasks();
  assert.equal(second.events.length, 0);

  timers.runTimeout(25);
  await flushMicrotasks();
  timers.runTimeout(25);
  await Promise.all([disconnect, reconnect]);
  assert.equal(client.connected, true);
  assert.ok(first.events.includes("close"));
  assert.ok(second.events.includes("dtr:true"));

  first.releaseFirstWrite();
  staleTimeout();
  staleRetry();
  staleHeartbeat();
  await flushMicrotasks();
  assert.equal(second.writes.length, 0);
  assert.equal(second.events.filter((event) => event === "close").length, 0);
  assert.equal(client.connected, true);

  const newRequest = client.send(CommandType.Ping);
  await flushMicrotasks();
  assert.equal(second.writes.length, 1);
  const newFrame = decodeFrame(second.writes[0]);
  const ack = responseFrame(newFrame.sequence, CommandType.Ack);
  client.acceptFrame(decodeFrame(ack), ack);
  await newRequest;

  await client.disconnect();
  assert.equal(second.events.filter((event) => event === "writer:release").length, 1);
  assert.equal(second.events.filter((event) => event === "close").length, 1);
});

test("physical disconnect ignores stale and unrelated port session tokens", async () => {
  const first = fakeSerial();
  const second = fakeSerial();
  const ports = [first.port, second.port];
  const timers = fakeTimers();
  const client = makeClient({
    serial: { requestPort: async () => ports.shift() },
  }, timers);
  const ui = [];

  await client.connect();
  const firstSession = client.sessionSnapshot();
  await client.disconnect();
  await client.connect();
  const secondSession = client.sessionSnapshot();
  const unrelatedSession = { port: {}, generation: secondSession.generation };

  assert.equal(await handlePhysicalDisconnect(
    client,
    firstSession,
    (connected) => ui.push(`connected:${connected}`),
    (level, label, message) => ui.push(`${level}:${label}:${message}`),
  ), false);
  assert.equal(await handlePhysicalDisconnect(
    client,
    unrelatedSession,
    (connected) => ui.push(`connected:${connected}`),
    (level, label, message) => ui.push(`${level}:${label}:${message}`),
  ), false);

  assert.equal(client.connected, true);
  assert.equal(client.port, second.port);
  assert.equal(second.events.filter((event) => event === "writer:release").length, 0);
  assert.equal(second.events.filter((event) => event === "close").length, 0);
  assert.deepEqual(ui, []);

  assert.equal(await handlePhysicalDisconnect(
    client,
    secondSession,
    (connected) => ui.push(`connected:${connected}`),
    (level, label, message) => ui.push(`${level}:${label}:${message}`),
  ), true);
  assert.equal(second.events.filter((event) => event === "writer:release").length, 1);
  assert.equal(second.events.filter((event) => event === "close").length, 1);
  assert.deepEqual(ui.slice(-2), ["connected:false", "info:DETACH:serial device removed"]);
});

test("physical disconnect rejects an old generation for the same SerialPort", async () => {
  const shared = fakeSerial();
  const timers = fakeTimers();
  const client = makeClient(shared, timers);
  const ui = [];

  await client.connect();
  const oldSession = client.sessionSnapshot();
  await client.disconnect();
  await client.connect();
  const currentSession = client.sessionSnapshot();
  assert.equal(oldSession.port, currentSession.port);
  assert.notEqual(oldSession.generation, currentSession.generation);
  const closeCount = shared.events.filter((event) => event === "close").length;

  assert.equal(await handlePhysicalDisconnect(
    client,
    oldSession,
    (connected) => ui.push(`connected:${connected}`),
    (level, label, message) => ui.push(`${level}:${label}:${message}`),
  ), false);
  assert.equal(client.connected, true);
  assert.equal(shared.events.filter((event) => event === "close").length, closeCount);
  assert.deepEqual(ui, []);

  assert.equal(await handlePhysicalDisconnect(
    client,
    currentSession,
    (connected) => ui.push(`connected:${connected}`),
    (level, label, message) => ui.push(`${level}:${label}:${message}`),
  ), true);
  assert.equal(shared.events.filter((event) => event === "close").length, closeCount + 1);
  assert.deepEqual(ui.slice(-2), ["connected:false", "info:DETACH:serial device removed"]);
});

for (const stage of ["cancel", "dtr", "close"]) {
  test(`disconnect bounds a hung ${stage} cleanup step and continues`, async () => {
    const serialState = serialWithHungCleanup(stage);
    const timers = fakeTimers(serialState.events);
    const client = makeClient(serialState, timers, { cleanupTimeoutMs: 25 });
    await client.connect();

    let settled = false;
    const disconnect = client.disconnect().then(() => { settled = true; });
    const hungEvent = {
      cancel: "reader:cancel",
      dtr: "dtr:false",
      close: "close",
    }[stage];
    await flushUntil(() => serialState.events.includes(hungEvent));
    assert.equal(settled, false);
    assert.ok(timers.delays().includes(25));

    timers.runTimeout(25);
    await disconnect;
    assert.equal(settled, true);
    assert.ok(serialState.events.includes("reader:cancel"));
    assert.ok(serialState.events.includes("dtr:false"));
    assert.ok(serialState.events.includes("writer:release"));
    assert.ok(serialState.events.includes("close"));
  });
}

test("physical disconnect fully closes the session and stale timeout cannot retry after reconnect", async () => {
  const first = fakeSerial({ readable: true });
  const second = fakeSerial();
  const timers = fakeTimers(first.events);
  const client = makeClient(first, timers);
  const ui = [];
  await client.connect();

  const request = client.send(CommandType.Ping);
  await flushMicrotasks();
  const staleTimeout = timers.callbackFor(1000);
  const expectedSession = client.sessionSnapshot();
  const detach = handlePhysicalDisconnect(
    client,
    expectedSession,
    (connected) => ui.push(`connected:${connected}`),
    (level, label, message) => ui.push(`${level}:${label}:${message}`),
  );

  await assert.rejects(request, /serial disconnected/);
  await detach;
  assert.ok(first.events.includes("heartbeat:stopped"));
  assert.ok(first.events.includes("reader:cancel"));
  assert.ok(first.events.includes("dtr:false"));
  assert.ok(first.events.includes("writer:release"));
  assert.ok(first.events.includes("close"));
  assert.equal(client.pending.size, 0);
  assert.deepEqual(ui.slice(-2), ["connected:false", "info:DETACH:serial device removed"]);

  client.serial = second.serial;
  await client.connect();
  staleTimeout();
  await flushMicrotasks();
  assert.equal(second.writes.length, 0);
  await client.disconnect();
});

test("physical disconnect reports cleanup failure without falsely closing the UI", async () => {
  const ui = [];

  assert.equal(await handlePhysicalDisconnect(
    { disconnect: async () => { throw new Error("cleanup failed"); } },
    { port: {}, generation: 1 },
    (connected) => ui.push(`connected:${connected}`),
    (level, label, message) => ui.push(`${level}:${label}:${message}`),
  ), false);

  assert.deepEqual(ui, [
    "error:DETACH:cleanup failed",
  ]);
});

test("command heartbeat and STOP writes share one serialized writer queue", async () => {
  const serialState = gatedSerial();
  const timers = fakeTimers(serialState.events);
  const client = makeClient(serialState, timers);
  await client.connect();

  const request = client.send(CommandType.Ping);
  const rejectedRequest = assert.rejects(request, /serial disconnected/);
  await serialState.firstWriteStarted;
  timers.runInterval(500);
  const disconnect = client.disconnect();
  await flushMicrotasks();
  serialState.releaseFirstWrite();

  await rejectedRequest;
  await disconnect;
  assert.deepEqual(
    serialState.writes.map((frame) => decodeFrame(frame).commandType),
    [CommandType.Ping, CommandType.Heartbeat, CommandType.StopAll],
  );
  assert.equal(serialState.maxConcurrentWrites, 1);
});
