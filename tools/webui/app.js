import {
  CommandName,
  CommandType,
  FLAG_NO_RESPONSE,
  MAX_FRAME_SIZE,
  asciiPayload,
  bytePayload,
  bytesToHex,
  decodeFrame,
  encodeFrame,
  expectedResponseType,
  extractFrames,
  i16PairPayload,
  keyPayload,
  u32Payload,
} from "./protocol.js";
import { parseCombo } from "./keys.js";
import { parseButton, parseScript, runScriptCommands } from "./script.js";

const $ = (selector) => globalThis.document?.querySelector(selector) ?? null;
const $$ = (selector) => Array.from(globalThis.document?.querySelectorAll(selector) ?? []);

const statusPill = $("#statusPill");
const portLabel = $("#portLabel");
const lastStatus = $("#lastStatus");
const scriptSummary = $("#scriptSummary");
const logList = $("#logList");
const connectBtn = $("#connectBtn");
const disconnectBtn = $("#disconnectBtn");
const armToggle = $("#armToggle");

const demoScript = `type "abc 123"
key tap ENTER
mouse move 10 -5
wait 100
stop`;

export class SerialHidClient {
  constructor({
    serial = globalThis.navigator?.serial,
    timers = globalThis,
    now = () => globalThis.performance.now(),
    log = addLog,
    responseTimeoutMs = 1200,
    retries = 2,
    heartbeatIntervalMs = 500,
  } = {}) {
    this.serial = serial;
    this.timers = timers;
    this.now = now;
    this.log = log;
    this.responseTimeoutMs = responseTimeoutMs;
    this.retries = retries;
    this.heartbeatIntervalMs = heartbeatIntervalMs;
    this.port = null;
    this.reader = null;
    this.readTask = null;
    this.writer = null;
    this.rxBuffer = new Uint8Array();
    this.sequence = 1;
    this.pending = new Map();
    this.connected = false;
    this.commandQueue = Promise.resolve();
    this.writeQueue = Promise.resolve();
    this.heartbeatTimer = null;
    this.batchDurationMs = null;
  }

  async connect() {
    if (!this.serial) {
      throw new Error("当前浏览器不支持 Web Serial");
    }

    let port = null;
    let writer = null;
    let opened = false;
    let dtrAttempted = false;

    try {
      port = await this.serial.requestPort({
        filters: [{ usbVendorId: 0xcafe, usbProductId: 0x2350 }],
      });
      await port.open({ baudRate: 115200, bufferSize: MAX_FRAME_SIZE });
      opened = true;
      writer = port.writable.getWriter();
      dtrAttempted = true;
      await port.setSignals({ dataTerminalReady: true });

      this.port = port;
      this.writer = writer;
      this.connected = true;
      this.startHeartbeat();
      this.readTask = this.readLoop();
    } catch (error) {
      if (dtrAttempted) {
        try {
          await port.setSignals({ dataTerminalReady: false });
        } catch {}
      }
      if (writer) {
        try {
          writer.releaseLock();
        } catch {}
      }
      if (opened) {
        try {
          await port.close();
        } catch {}
      }

      this.port = null;
      this.writer = null;
      this.connected = false;
      this.stopHeartbeat();
      this.readTask = null;
      throw error;
    }
  }

  startHeartbeat() {
    const frame = encodeFrame(0, CommandType.Heartbeat, new Uint8Array(), FLAG_NO_RESPONSE);
    this.heartbeatTimer = this.timers.setInterval(() => {
      if (!this.connected || !this.writer) {
        return;
      }
      this.queueWrite(frame).catch((error) => this.log("error", "HEARTBEAT", error.message));
    }, this.heartbeatIntervalMs);
  }

  stopHeartbeat() {
    if (this.heartbeatTimer !== null) {
      this.timers.clearInterval(this.heartbeatTimer);
      this.heartbeatTimer = null;
    }
  }

  queueWrite(frame) {
    const operation = this.writeQueue.catch(() => {}).then(async () => {
      if (!this.writer) {
        throw new Error("serial is not connected");
      }
      await this.writer.write(frame);
    });
    this.writeQueue = operation.catch(() => {});
    return operation;
  }

  responseTimeoutFor(commandType, payload) {
    let requiredMs = 1000;
    if (commandType === CommandType.BatchEnd) {
      requiredMs = (this.batchDurationMs ?? 0) + 1000;
    } else {
      const durationMs = this.commandDurationMs(commandType, payload);
      if (durationMs !== null) {
        requiredMs = durationMs + 500;
      }
    }
    return Math.max(this.responseTimeoutMs, requiredMs);
  }

  recordCompletedCommand(commandType, payload) {
    if (commandType === CommandType.BatchBegin) {
      this.batchDurationMs = 0;
      return;
    }
    if (commandType === CommandType.BatchEnd || commandType === CommandType.StopAll) {
      this.batchDurationMs = null;
      return;
    }

    const durationMs = this.commandDurationMs(commandType, payload);
    if (this.batchDurationMs !== null && durationMs !== null) {
      this.batchDurationMs += durationMs;
    }
  }

  commandDurationMs(commandType, payload) {
    if (commandType === CommandType.WaitMs && payload.length === 4) {
      return (((payload[0] << 24) >>> 0) + (payload[1] << 16) + (payload[2] << 8) + payload[3]) >>> 0;
    }
    if (commandType === CommandType.TypeAscii) {
      return payload.length * 8;
    }
    if (commandType === CommandType.MouseMoveRel && payload.length === 4) {
      const dxRaw = (payload[0] << 8) | payload[1];
      const dyRaw = (payload[2] << 8) | payload[3];
      const dx = dxRaw & 0x8000 ? dxRaw - 0x10000 : dxRaw;
      const dy = dyRaw & 0x8000 ? dyRaw - 0x10000 : dyRaw;
      return Math.ceil(Math.max(Math.abs(dx), Math.abs(dy)) / 127);
    }
    if (commandType === CommandType.KeyTap) {
      return 8;
    }
    if (commandType === CommandType.MouseClick) {
      return 20;
    }
    return null;
  }

  async disconnect() {
    this.connected = false;

    if (this.writer) {
      const stopFrame = encodeFrame(this.nextSequence(), CommandType.StopAll);
      await this.queueWrite(stopFrame).catch(() => {});
    }
    this.stopHeartbeat();
    this.batchDurationMs = null;

    for (const [sequence, pending] of Array.from(this.pending.entries())) {
      this.rejectPending(sequence, pending, new Error("serial disconnected"));
    }

    if (this.reader) {
      await this.reader.cancel().catch(() => {});
    }
    if (this.readTask) {
      await this.readTask.catch(() => {});
      this.readTask = null;
    }
    if (this.port) {
      await this.port.setSignals({ dataTerminalReady: false }).catch(() => {});
    }
    if (this.writer) {
      this.writer.releaseLock();
      this.writer = null;
    }
    if (this.port) {
      await this.port.close().catch(() => {});
      this.port = null;
    }
  }

  send(commandType, payload = new Uint8Array()) {
    if (!this.writer || !this.connected) {
      return Promise.reject(new Error("serial is not connected"));
    }

    const operation = this.commandQueue
      .catch(() => {})
      .then(() => this.sendOnce(commandType, payload))
      .then((response) => {
        this.recordCompletedCommand(commandType, payload);
        return response;
      });
    this.commandQueue = operation.catch(() => {});
    return operation;
  }

  async sendOnce(commandType, payload) {
    if (!this.writer || !this.connected) {
      throw new Error("serial is not connected");
    }

    const sequence = this.nextSequence();
    const frame = encodeFrame(sequence, commandType, payload);
    const started = this.now();
    this.log("tx", CommandName[commandType] ?? `0x${commandType.toString(16)}`, bytesToHex(frame));

    return new Promise((resolve, reject) => {
      const pending = {
        commandType,
        frame,
        resolve,
        reject,
        timer: null,
        started,
        retriesRemaining: this.retries,
        responseTimeoutMs: this.responseTimeoutFor(commandType, payload),
      };
      this.pending.set(sequence, pending);
      this.writePending(sequence, pending);
    });
  }

  writePending(sequence, pending) {
    this.queueWrite(pending.frame).then(
      () => this.armResponseTimeout(sequence, pending),
      (error) => this.rejectPending(sequence, pending, error),
    );
  }

  armResponseTimeout(sequence, pending) {
    if (this.pending.get(sequence) !== pending) {
      return;
    }
    pending.timer = this.timers.setTimeout(() => {
      pending.timer = null;
      this.retryPending(sequence, pending, 0, new Error("response timeout"));
    }, pending.responseTimeoutMs);
  }

  retryPending(sequence, pending, delayMs, terminalError) {
    if (this.pending.get(sequence) !== pending) {
      return;
    }
    if (pending.retriesRemaining === 0) {
      this.rejectPending(sequence, pending, terminalError);
      return;
    }

    pending.retriesRemaining -= 1;
    if (delayMs === 0) {
      this.writePending(sequence, pending);
      return;
    }
    pending.timer = this.timers.setTimeout(() => {
      pending.timer = null;
      if (this.pending.get(sequence) === pending) {
        this.writePending(sequence, pending);
      }
    }, delayMs);
  }

  rejectPending(sequence, pending, error) {
    if (this.pending.get(sequence) !== pending) {
      return;
    }
    if (pending.timer !== null) {
      this.timers.clearTimeout(pending.timer);
      pending.timer = null;
    }
    this.pending.delete(sequence);
    pending.reject(error);
  }

  nextSequence() {
    const sequence = this.sequence;
    this.sequence = (this.sequence + 1) & 0xffff;
    if (this.sequence === 0) {
      this.sequence = 1;
    }
    return sequence;
  }

  async readLoop() {
    while (this.port?.readable && this.connected) {
      this.reader = this.port.readable.getReader();
      try {
        while (this.connected) {
          const { value, done } = await this.reader.read();
          if (done) {
            break;
          }
          if (value) {
            this.acceptBytes(value);
          }
        }
      } catch (error) {
        if (this.connected) {
          this.log("error", "RX", error.message);
        }
      } finally {
        this.reader.releaseLock();
        this.reader = null;
      }
    }
  }

  acceptBytes(chunk) {
    const merged = new Uint8Array(this.rxBuffer.length + chunk.length);
    merged.set(this.rxBuffer);
    merged.set(chunk, this.rxBuffer.length);
    const { frames, remaining } = extractFrames(merged);
    this.rxBuffer = remaining;

    for (const frameBytes of frames) {
      try {
        this.acceptFrame(decodeFrame(frameBytes), frameBytes);
      } catch (error) {
        this.log("error", "DECODE", error.message);
      }
    }
  }

  acceptFrame(frame, frameBytes) {
    this.log("rx", CommandName[frame.commandType] ?? `0x${frame.commandType.toString(16)}`, bytesToHex(frameBytes));

    const pending = this.pending.get(frame.sequence);
    if (!pending) {
      this.log("info", "STALE", `seq=${frame.sequence}`);
      return;
    }

    if (frame.commandType === CommandType.Nack) {
      const code = frame.payload[0] ?? 0;
      this.rejectPending(frame.sequence, pending, new Error(`NACK ${code}`));
      return;
    }

    if (frame.commandType === CommandType.Busy) {
      if (pending.timer !== null) {
        this.timers.clearTimeout(pending.timer);
        pending.timer = null;
      }
      if (frame.payload.length !== 3) {
        this.rejectPending(frame.sequence, pending, new Error(`malformed BUSY payload (${frame.payload.length} bytes)`));
        return;
      }
      const reason = frame.payload[0];
      const delayMs = (frame.payload[1] << 8) | frame.payload[2];
      this.retryPending(
        frame.sequence,
        pending,
        delayMs,
        new Error(`device remained BUSY (reason ${reason})`),
      );
      return;
    }

    const expected = expectedResponseType(pending.commandType);
    if (frame.commandType !== expected) {
      this.rejectPending(
        frame.sequence,
        pending,
        new Error(`unexpected ${CommandName[frame.commandType]}, expected ${CommandName[expected]}`),
      );
      return;
    }

    if (pending.timer !== null) {
      this.timers.clearTimeout(pending.timer);
      pending.timer = null;
    }
    this.pending.delete(frame.sequence);
    frame.elapsedMs = Math.round(this.now() - pending.started);
    pending.resolve(frame);
  }
}

export async function handlePhysicalDisconnect(client, setConnectedCallback, log) {
  try {
    await client.disconnect();
  } catch (error) {
    log("error", "DETACH", error.message);
  } finally {
    setConnectedCallback(false);
    log("info", "DETACH", "serial device removed");
  }
}

const client = new SerialHidClient();

if (globalThis.document) {
connectBtn.addEventListener("click", async () => {
  await runUiAction(async () => {
    await client.connect();
    setConnected(true);
    portLabel.textContent = "CAFE:2350 已授权";
    addLog("ok", "CONNECT", "serial opened");
  });
});

disconnectBtn.addEventListener("click", async () => {
  await runUiAction(async () => {
    await client.disconnect();
    setConnected(false);
    addLog("info", "DISCONNECT", "serial closed");
  });
});

armToggle.addEventListener("change", updateArmState);
$("#clearLogBtn").addEventListener("click", () => {
  logList.replaceChildren();
});

$$("[data-action]").forEach((button) => {
  button.addEventListener("click", () => handleAction(button.dataset.action));
});

$$("[data-key-action]").forEach((button) => {
  button.addEventListener("click", () => sendKey(button.dataset.keyAction));
});

$$("[data-move]").forEach((button) => {
  button.addEventListener("click", () => {
    const [dx, dy] = button.dataset.move.split(",").map(Number);
    sendMouseMove(dx, dy);
  });
});

$$("[data-click]").forEach((button) => {
  button.addEventListener("click", () => sendMouseClick(button.dataset.click));
});

navigator.serial?.addEventListener("disconnect", () => {
  void handlePhysicalDisconnect(client, setConnected, addLog);
});

setConnected(false);
updateArmState();
}

async function handleAction(action) {
  const handlers = {
    ping: () => sendSimple(CommandType.Ping, "Ping OK"),
    info: () => sendStatus(CommandType.GetInfo, "INFO"),
    caps: () => sendStatus(CommandType.GetCaps, "CAPS"),
    stop: () => sendSimple(CommandType.StopAll, "Stop OK"),
    wait: () => sendWait(),
    type: () => sendTypeText(),
    mouseMove: () => sendMouseMove(readInteger("#mouseDx"), readInteger("#mouseDy")),
    wheel: () => sendWheel(),
    loadDemo: () => {
      $("#scriptText").value = demoScript;
      scriptSummary.textContent = "Demo loaded";
    },
    parseScript: () => parseScriptFromEditor(),
    runScript: () => runScriptFromEditor(),
  };

  await runUiAction(handlers[action]);
}

async function sendSimple(commandType, label) {
  const response = await client.send(commandType);
  lastStatus.textContent = `${label} (${response.elapsedMs} ms)`;
}

async function sendStatus(commandType, label) {
  const response = await client.send(commandType);
  lastStatus.textContent = `${label} ${bytesToHex(response.payload)} (${response.elapsedMs} ms)`;
}

async function sendWait() {
  const ms = readInteger("#waitMs");
  const response = await client.send(CommandType.WaitMs, u32Payload(ms));
  lastStatus.textContent = `Wait OK (${response.elapsedMs} ms)`;
}

async function sendTypeText() {
  requireArmed();
  const text = $("#typeText").value;
  const response = await client.send(CommandType.TypeAscii, asciiPayload(text));
  lastStatus.textContent = `Type OK (${response.elapsedMs} ms)`;
}

async function sendKey(action) {
  await runUiAction(async () => {
    requireArmed();
    const combo = parseCombo($("#keyCombo").value);
    const commandType = {
      tap: CommandType.KeyTap,
      down: CommandType.KeyDown,
      up: CommandType.KeyUp,
    }[action];
    const response = await client.send(commandType, keyPayload(combo));
    lastStatus.textContent = `Key ${action} OK (${response.elapsedMs} ms)`;
  });
}

async function sendMouseMove(dx, dy) {
  await runUiAction(async () => {
    requireArmed();
    const response = await client.send(CommandType.MouseMoveRel, i16PairPayload(dx, dy));
    lastStatus.textContent = `Move ${dx},${dy} OK (${response.elapsedMs} ms)`;
  });
}

async function sendMouseClick(buttonName) {
  await runUiAction(async () => {
    requireArmed();
    const button = parseButton(buttonName);
    const response = await client.send(CommandType.MouseClick, bytePayload(button.mask));
    lastStatus.textContent = `Mouse ${button.name} OK (${response.elapsedMs} ms)`;
  });
}

async function sendWheel() {
  requireArmed();
  const delta = readInteger("#wheelDelta");
  const response = await client.send(CommandType.MouseWheel, bytePayload(delta));
  lastStatus.textContent = `Wheel ${delta} OK (${response.elapsedMs} ms)`;
}

function parseScriptFromEditor() {
  const commands = parseScript($("#scriptText").value);
  scriptSummary.textContent = `${commands.length} commands`;
  return commands;
}

async function runScriptFromEditor() {
  requireArmed();
  const commands = parseScriptFromEditor();
  await runScriptCommands(commands, (commandType, payload) => client.send(commandType, payload));
  scriptSummary.textContent = `OK ${commands.length} commands`;
}

async function runUiAction(action) {
  try {
    setBusy(true);
    await action();
  } catch (error) {
    addLog("error", "ERROR", error.message);
    lastStatus.textContent = error.message;
  } finally {
    setBusy(false);
  }
}

function setConnected(connected) {
  client.connected = connected;
  statusPill.textContent = connected ? "已连接" : "未连接";
  statusPill.classList.toggle("connected", connected);
  statusPill.classList.toggle("disconnected", !connected);
  connectBtn.disabled = connected;
  disconnectBtn.disabled = !connected;
  portLabel.textContent = connected ? portLabel.textContent : "等待串口授权";
  updateControlState(false);
}

function updateArmState() {
  updateControlState(false);
}

function updateControlState(busy) {
  const connected = client.connected;
  const armed = armToggle.checked;

  connectBtn.disabled = busy || connected;
  disconnectBtn.disabled = busy || !connected;
  $$("button").forEach((button) => {
    if (button.id === "connectBtn" || button.id === "disconnectBtn") {
      return;
    }
    const localOnly = button.dataset.action === "loadDemo" || button.dataset.action === "parseScript" || button.id === "clearLogBtn";
    const needsHid = button.classList.contains("hid-action");
    const needsSerial = button.dataset.action || button.dataset.keyAction || button.dataset.move || button.dataset.click;

    if (localOnly) {
      button.disabled = busy;
    } else if (needsHid) {
      button.disabled = busy || !connected || !armed;
    } else if (needsSerial) {
      button.disabled = busy || !connected;
    }
  });
}

function setBusy(busy) {
  updateControlState(busy);
}

function requireArmed() {
  if (!armToggle.checked) {
    throw new Error("HID actions are locked");
  }
}

function readInteger(selector) {
  const value = Number($(selector).value);
  if (!Number.isInteger(value)) {
    throw new Error(`${selector} must be an integer`);
  }
  return value;
}

function addLog(level, label, message) {
  const row = document.createElement("div");
  row.className = `log-entry ${level}`;
  const time = new Date().toLocaleTimeString("zh-CN", { hour12: false });
  row.innerHTML = `<span>${time}</span><span class="level">${escapeHtml(label)}</span><span>${escapeHtml(message)}</span>`;
  logList.prepend(row);
  while (logList.childElementCount > 160) {
    logList.lastElementChild?.remove();
  }
}

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}
