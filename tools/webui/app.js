import {
  CommandName,
  CommandType,
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
import { parseButton, parseScript } from "./script.js";

const $ = (selector) => document.querySelector(selector);
const $$ = (selector) => Array.from(document.querySelectorAll(selector));

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

class SerialHidClient {
  constructor() {
    this.port = null;
    this.reader = null;
    this.writer = null;
    this.rxBuffer = new Uint8Array();
    this.sequence = 1;
    this.pending = new Map();
    this.connected = false;
    this.commandQueue = Promise.resolve();
  }

  async connect() {
    if (!("serial" in navigator)) {
      throw new Error("当前浏览器不支持 Web Serial");
    }

    this.port = await navigator.serial.requestPort({
      filters: [{ usbVendorId: 0xcafe, usbProductId: 0x2350 }],
    });
    await this.port.open({ baudRate: 115200, bufferSize: MAX_FRAME_SIZE });
    this.writer = this.port.writable.getWriter();
    this.connected = true;
    this.readLoop();
  }

  async disconnect() {
    this.connected = false;
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(new Error("serial disconnected"));
    }
    this.pending.clear();

    if (this.reader) {
      await this.reader.cancel().catch(() => {});
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
    const operation = this.commandQueue.catch(() => {}).then(() => this.sendOnce(commandType, payload));
    this.commandQueue = operation.catch(() => {});
    return operation;
  }

  async sendOnce(commandType, payload) {
    if (!this.writer || !this.connected) {
      throw new Error("serial is not connected");
    }

    const sequence = this.nextSequence();
    const frame = encodeFrame(sequence, commandType, payload);
    const started = performance.now();
    addLog("tx", CommandName[commandType] ?? `0x${commandType.toString(16)}`, bytesToHex(frame));

    const responsePromise = new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(sequence);
        reject(new Error("response timeout"));
      }, 1200);
      this.pending.set(sequence, { commandType, resolve, reject, timer, started });
    });

    try {
      await this.writer.write(frame);
    } catch (error) {
      const pending = this.pending.get(sequence);
      if (pending) {
        clearTimeout(pending.timer);
        this.pending.delete(sequence);
      }
      throw error;
    }
    return responsePromise;
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
          addLog("error", "RX", error.message);
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
        addLog("error", "DECODE", error.message);
      }
    }
  }

  acceptFrame(frame, frameBytes) {
    addLog("rx", CommandName[frame.commandType] ?? `0x${frame.commandType.toString(16)}`, bytesToHex(frameBytes));

    const pending = this.pending.get(frame.sequence);
    if (!pending) {
      addLog("info", "STALE", `seq=${frame.sequence}`);
      return;
    }

    this.pending.delete(frame.sequence);
    clearTimeout(pending.timer);

    if (frame.commandType === CommandType.Nack) {
      const code = frame.payload[0] ?? 0;
      pending.reject(new Error(`NACK ${code}`));
      return;
    }

    const expected = expectedResponseType(pending.commandType);
    if (frame.commandType !== expected) {
      pending.reject(new Error(`unexpected ${CommandName[frame.commandType]}, expected ${CommandName[expected]}`));
      return;
    }

    frame.elapsedMs = Math.round(performance.now() - pending.started);
    pending.resolve(frame);
  }
}

const client = new SerialHidClient();

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
  setConnected(false);
  addLog("info", "DETACH", "serial device removed");
});

setConnected(false);
updateArmState();

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
  await client.send(CommandType.BatchBegin);
  try {
    for (const command of commands) {
      await executeScriptCommand(command);
    }
    await client.send(CommandType.BatchEnd);
    scriptSummary.textContent = `OK ${commands.length} commands`;
  } catch (error) {
    await client.send(CommandType.StopAll).catch(() => {});
    throw error;
  }
}

function executeScriptCommand(command) {
  if (command.kind === "type") {
    return client.send(CommandType.TypeAscii, asciiPayload(command.text));
  }
  if (command.kind === "key") {
    const commandType = {
      tap: CommandType.KeyTap,
      down: CommandType.KeyDown,
      up: CommandType.KeyUp,
    }[command.action];
    return client.send(commandType, keyPayload(command.combo));
  }
  if (command.kind === "mouse") {
    if (command.action === "move") {
      return client.send(CommandType.MouseMoveRel, i16PairPayload(command.dx, command.dy));
    }
    if (command.action === "click") {
      return client.send(CommandType.MouseClick, bytePayload(command.button.mask));
    }
    if (command.action === "down") {
      return client.send(CommandType.MouseButtonDown, bytePayload(command.button.mask));
    }
    if (command.action === "up") {
      return client.send(CommandType.MouseButtonUp, bytePayload(command.button.mask));
    }
    if (command.action === "wheel") {
      return client.send(CommandType.MouseWheel, bytePayload(command.delta));
    }
  }
  if (command.kind === "wait") {
    return client.send(CommandType.WaitMs, u32Payload(command.ms));
  }
  if (command.kind === "stop") {
    return client.send(CommandType.StopAll);
  }
  throw new Error(`unknown script command ${command.kind}`);
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
