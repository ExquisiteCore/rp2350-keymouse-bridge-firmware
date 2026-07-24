import { parseCombo } from "./keys.js";
import {
  CommandType,
  asciiPayload,
  bytePayload,
  i16PairPayload,
  keyPayload,
  u32Payload,
} from "./protocol.js";

export function parseScript(input) {
  const commands = [];
  const lines = input.split(/\r?\n/);
  for (let index = 0; index < lines.length; index += 1) {
    const command = parseLine(lines[index], index + 1);
    if (command) {
      commands.push(command);
    }
  }
  return commands;
}

export function planScriptCommands(commands) {
  const plan = [];
  let segment = [];

  const appendSegment = () => {
    if (segment.length === 0) {
      return;
    }
    plan.push({ commandType: CommandType.BatchBegin, payload: new Uint8Array() });
    plan.push(...segment.map(scriptCommandPacket));
    plan.push({ commandType: CommandType.BatchEnd, payload: new Uint8Array() });
    segment = [];
  };

  for (const command of commands) {
    if (command.kind === "stop") {
      appendSegment();
      plan.push({ commandType: CommandType.StopAll, payload: new Uint8Array() });
    } else {
      segment.push(command);
    }
  }
  appendSegment();
  return plan;
}

export async function runScriptCommands(commands, send) {
  try {
    for (const packet of planScriptCommands(commands)) {
      await send(packet.commandType, packet.payload);
    }
  } catch (error) {
    try {
      await send(CommandType.StopAll, new Uint8Array());
    } catch {
      // Preserve the original script error.
    }
    throw error;
  }
}

function scriptCommandPacket(command) {
  if (command.kind === "type") {
    return { commandType: CommandType.TypeAscii, payload: asciiPayload(command.text) };
  }
  if (command.kind === "key") {
    const commandType = {
      tap: CommandType.KeyTap,
      down: CommandType.KeyDown,
      up: CommandType.KeyUp,
    }[command.action];
    return { commandType, payload: keyPayload(command.combo) };
  }
  if (command.kind === "mouse") {
    if (command.action === "move") {
      return {
        commandType: CommandType.MouseMoveRel,
        payload: i16PairPayload(command.dx, command.dy),
      };
    }
    const commandType = {
      click: CommandType.MouseClick,
      down: CommandType.MouseButtonDown,
      up: CommandType.MouseButtonUp,
      wheel: CommandType.MouseWheel,
    }[command.action];
    const value = command.action === "wheel" ? command.delta : command.button.mask;
    return { commandType, payload: bytePayload(value) };
  }
  if (command.kind === "wait") {
    return { commandType: CommandType.WaitMs, payload: u32Payload(command.ms) };
  }
  throw new Error(`unknown script command ${command.kind}`);
}

export function parseLine(line, lineNumber = 1) {
  try {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) {
      return null;
    }

    const parts = splitWords(trimmed);
    const head = parts.shift()?.toLowerCase();
    if (!head) {
      return null;
    }

    if (head === "type" || head === "text") {
      expectCount(parts, 1, "type expects one string");
      return { kind: "type", text: parts[0] };
    }
    if (head === "key") {
      expectCount(parts, 2, "key expects: key tap|down|up COMBO");
      return parseKeyCommand(parts);
    }
    if (head === "mouse") {
      return parseMouseCommand(parts);
    }
    if (head === "wait") {
      expectCount(parts, 1, "wait expects milliseconds");
      return { kind: "wait", ms: parseInteger(parts[0], "milliseconds") };
    }
    if (head === "stop") {
      expectCount(parts, 0, "stop takes no arguments");
      return { kind: "stop" };
    }

    throw new Error(`unknown script command ${head}`);
  } catch (error) {
    throw new Error(`line ${lineNumber}: ${error.message}`);
  }
}

export function parseButton(input) {
  const button = input.toLowerCase();
  if (button === "left" || button === "l") {
    return { name: "left", mask: 0x01 };
  }
  if (button === "right" || button === "r") {
    return { name: "right", mask: 0x02 };
  }
  if (button === "middle" || button === "m") {
    return { name: "middle", mask: 0x04 };
  }
  throw new Error(`unknown mouse button ${input}`);
}

function parseKeyCommand(parts) {
  const action = parts[0].toLowerCase();
  if (!["tap", "down", "up"].includes(action)) {
    throw new Error(`unknown key action ${parts[0]}`);
  }
  return {
    kind: "key",
    action,
    combo: parseCombo(parts[1]),
    label: parts[1],
  };
}

function parseMouseCommand(parts) {
  if (parts.length === 0) {
    throw new Error("mouse expects an action");
  }

  const action = parts[0].toLowerCase();
  if (action === "move") {
    expectCount(parts, 3, "mouse move expects dx dy");
    return {
      kind: "mouse",
      action,
      dx: parseInteger(parts[1], "dx"),
      dy: parseInteger(parts[2], "dy"),
    };
  }
  if (action === "click" || action === "down" || action === "up") {
    expectCount(parts, 2, `mouse ${action} expects button`);
    return {
      kind: "mouse",
      action,
      button: parseButton(parts[1]),
    };
  }
  if (action === "wheel") {
    expectCount(parts, 2, "mouse wheel expects delta");
    return {
      kind: "mouse",
      action,
      delta: parseInteger(parts[1], "delta"),
    };
  }

  throw new Error(`unknown mouse action ${parts[0]}`);
}

function splitWords(line) {
  const out = [];
  let current = "";
  let inQuote = false;

  for (let i = 0; i < line.length; i += 1) {
    const ch = line[i];
    if (ch === '"') {
      if (inQuote) {
        out.push(current);
        current = "";
        inQuote = false;
        while (line[i + 1] === " " || line[i + 1] === "\t") {
          i += 1;
        }
      } else {
        if (current) {
          throw new Error("quote must start a new token");
        }
        inQuote = true;
      }
    } else if (ch === "\\" && inQuote) {
      i += 1;
      if (i >= line.length) {
        throw new Error("trailing escape in quoted string");
      }
      const escaped = line[i];
      current += { n: "\n", r: "\r", t: "\t", '"': '"', "\\": "\\" }[escaped] ?? escaped;
    } else if ((ch === " " || ch === "\t") && !inQuote) {
      if (current) {
        out.push(current);
        current = "";
      }
    } else if (ch === "#" && !inQuote && !current) {
      break;
    } else {
      current += ch;
    }
  }

  if (inQuote) {
    throw new Error("unterminated quoted string");
  }
  if (current) {
    out.push(current);
  }
  return out;
}

function expectCount(parts, count, message) {
  if (parts.length !== count) {
    throw new Error(message);
  }
}

function parseInteger(input, label) {
  const value = Number(input);
  if (!Number.isInteger(value)) {
    throw new Error(`${label} must be an integer`);
  }
  return value;
}
