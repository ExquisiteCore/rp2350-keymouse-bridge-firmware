import { parseCombo } from "./keys.js";

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
