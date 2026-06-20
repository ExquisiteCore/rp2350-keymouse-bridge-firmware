MOD_LEFT_CTRL = 0x01
MOD_LEFT_SHIFT = 0x02
MOD_LEFT_ALT = 0x04
MOD_LEFT_GUI = 0x08

_KEYCODES = {
    "0": 0x27,
    "ENTER": 0x28,
    "RETURN": 0x28,
    "ESC": 0x29,
    "ESCAPE": 0x29,
    "BACKSPACE": 0x2A,
    "BKSP": 0x2A,
    "TAB": 0x2B,
    "SPACE": 0x2C,
    "MINUS": 0x2D,
    "-": 0x2D,
    "EQUAL": 0x2E,
    "=": 0x2E,
    "LBRACKET": 0x2F,
    "[": 0x2F,
    "RBRACKET": 0x30,
    "]": 0x30,
    "BACKSLASH": 0x31,
    "\\": 0x31,
    "SEMICOLON": 0x33,
    ";": 0x33,
    "QUOTE": 0x34,
    "'": 0x34,
    "GRAVE": 0x35,
    "`": 0x35,
    "COMMA": 0x36,
    ",": 0x36,
    "DOT": 0x37,
    "PERIOD": 0x37,
    ".": 0x37,
    "SLASH": 0x38,
    "/": 0x38,
    "CAPSLOCK": 0x39,
    "F1": 0x3A,
    "F2": 0x3B,
    "F3": 0x3C,
    "F4": 0x3D,
    "F5": 0x3E,
    "F6": 0x3F,
    "F7": 0x40,
    "F8": 0x41,
    "F9": 0x42,
    "F10": 0x43,
    "F11": 0x44,
    "F12": 0x45,
    "PRINTSCREEN": 0x46,
    "PRTSCR": 0x46,
    "SCROLLLOCK": 0x47,
    "PAUSE": 0x48,
    "INSERT": 0x49,
    "HOME": 0x4A,
    "PAGEUP": 0x4B,
    "PGUP": 0x4B,
    "DELETE": 0x4C,
    "DEL": 0x4C,
    "END": 0x4D,
    "PAGEDOWN": 0x4E,
    "PGDN": 0x4E,
    "RIGHT": 0x4F,
    "LEFT": 0x50,
    "DOWN": 0x51,
    "UP": 0x52,
}


def parse_combo(input_text: str) -> tuple[int, int]:
    modifier = 0
    keycode: int | None = None

    for part in input_text.split("+"):
        token = part.strip().upper()
        if not token:
            raise ValueError(f"empty key token in combo {input_text!r}")
        if token in ("CTRL", "CONTROL"):
            modifier |= MOD_LEFT_CTRL
        elif token == "SHIFT":
            modifier |= MOD_LEFT_SHIFT
        elif token == "ALT":
            modifier |= MOD_LEFT_ALT
        elif token in ("GUI", "WIN", "META"):
            modifier |= MOD_LEFT_GUI
        else:
            if keycode is not None:
                raise ValueError(f"combo {input_text!r} contains more than one non-modifier key")
            keycode = _parse_keycode(token)

    if keycode is None:
        raise ValueError(f"combo {input_text!r} has no key")
    return modifier, keycode


def _parse_keycode(key: str) -> int:
    if len(key) == 1:
        code = ord(key)
        if ord("A") <= code <= ord("Z"):
            return 0x04 + (code - ord("A"))
        if ord("1") <= code <= ord("9"):
            return 0x1E + (code - ord("1"))

    try:
        return _KEYCODES[key]
    except KeyError as exc:
        raise ValueError(f"unknown key {key!r}") from exc
