export const MOD_LEFT_CTRL = 0x01;
export const MOD_LEFT_SHIFT = 0x02;
export const MOD_LEFT_ALT = 0x04;
export const MOD_LEFT_GUI = 0x08;

const KEYCODES = new Map([
  ["0", 0x27],
  ["ENTER", 0x28],
  ["RETURN", 0x28],
  ["ESC", 0x29],
  ["ESCAPE", 0x29],
  ["BACKSPACE", 0x2a],
  ["BKSP", 0x2a],
  ["TAB", 0x2b],
  ["SPACE", 0x2c],
  ["MINUS", 0x2d],
  ["-", 0x2d],
  ["EQUAL", 0x2e],
  ["=", 0x2e],
  ["LBRACKET", 0x2f],
  ["[", 0x2f],
  ["RBRACKET", 0x30],
  ["]", 0x30],
  ["BACKSLASH", 0x31],
  ["\\", 0x31],
  ["SEMICOLON", 0x33],
  [";", 0x33],
  ["QUOTE", 0x34],
  ["'", 0x34],
  ["GRAVE", 0x35],
  ["`", 0x35],
  ["COMMA", 0x36],
  [",", 0x36],
  ["DOT", 0x37],
  ["PERIOD", 0x37],
  [".", 0x37],
  ["SLASH", 0x38],
  ["/", 0x38],
  ["CAPSLOCK", 0x39],
  ["F1", 0x3a],
  ["F2", 0x3b],
  ["F3", 0x3c],
  ["F4", 0x3d],
  ["F5", 0x3e],
  ["F6", 0x3f],
  ["F7", 0x40],
  ["F8", 0x41],
  ["F9", 0x42],
  ["F10", 0x43],
  ["F11", 0x44],
  ["F12", 0x45],
  ["PRINTSCREEN", 0x46],
  ["PRTSCR", 0x46],
  ["SCROLLLOCK", 0x47],
  ["PAUSE", 0x48],
  ["INSERT", 0x49],
  ["HOME", 0x4a],
  ["PAGEUP", 0x4b],
  ["PGUP", 0x4b],
  ["DELETE", 0x4c],
  ["DEL", 0x4c],
  ["END", 0x4d],
  ["PAGEDOWN", 0x4e],
  ["PGDN", 0x4e],
  ["RIGHT", 0x4f],
  ["LEFT", 0x50],
  ["DOWN", 0x51],
  ["UP", 0x52],
]);

export function parseCombo(input) {
  let modifier = 0;
  let keycode = null;

  for (const rawPart of input.split("+")) {
    const token = rawPart.trim().toUpperCase();
    if (!token) {
      throw new Error(`empty key token in combo ${input}`);
    }

    if (token === "CTRL" || token === "CONTROL") {
      modifier |= MOD_LEFT_CTRL;
    } else if (token === "SHIFT") {
      modifier |= MOD_LEFT_SHIFT;
    } else if (token === "ALT") {
      modifier |= MOD_LEFT_ALT;
    } else if (token === "GUI" || token === "WIN" || token === "META") {
      modifier |= MOD_LEFT_GUI;
    } else {
      if (keycode !== null) {
        throw new Error(`combo ${input} contains more than one non-modifier key`);
      }
      keycode = parseKeycode(token);
    }
  }

  if (keycode === null) {
    throw new Error(`combo ${input} has no key`);
  }

  return { modifier, keycode };
}

function parseKeycode(key) {
  if (key.length === 1) {
    const code = key.charCodeAt(0);
    if (code >= 0x41 && code <= 0x5a) {
      return 0x04 + (code - 0x41);
    }
    if (code >= 0x31 && code <= 0x39) {
      return 0x1e + (code - 0x31);
    }
  }

  const keycode = KEYCODES.get(key);
  if (keycode === undefined) {
    throw new Error(`unknown key ${key}`);
  }
  return keycode;
}
