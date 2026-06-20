from __future__ import annotations

from dataclasses import dataclass

from .keys import parse_combo


@dataclass(frozen=True)
class ScriptCommand:
    kind: str
    action: str | None = None
    text: str | None = None
    combo: tuple[int, int] | None = None
    dx: int | None = None
    dy: int | None = None
    button: int | None = None
    delta: int | None = None
    ms: int | None = None


def parse_script(input_text: str) -> list[ScriptCommand]:
    commands: list[ScriptCommand] = []
    for line_no, line in enumerate(input_text.splitlines(), start=1):
        command = parse_line(line, line_no)
        if command is not None:
            commands.append(command)
    return commands


def parse_line(line: str, line_no: int = 1) -> ScriptCommand | None:
    try:
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            return None

        parts = _split_words(stripped)
        head = parts.pop(0).lower()
        if head in ("type", "text"):
            _expect_count(parts, 1, "type expects one string")
            return ScriptCommand(kind="type", text=parts[0])
        if head == "key":
            _expect_count(parts, 2, "key expects: key tap|down|up COMBO")
            action = parts[0].lower()
            if action not in ("tap", "down", "up"):
                raise ValueError(f"unknown key action {parts[0]}")
            return ScriptCommand(kind="key", action=action, combo=parse_combo(parts[1]))
        if head == "mouse":
            return _parse_mouse(parts)
        if head == "wait":
            _expect_count(parts, 1, "wait expects milliseconds")
            return ScriptCommand(kind="wait", ms=int(parts[0]))
        if head == "stop":
            _expect_count(parts, 0, "stop takes no arguments")
            return ScriptCommand(kind="stop")
        raise ValueError(f"unknown script command {head}")
    except Exception as exc:
        raise ValueError(f"line {line_no}: {exc}") from exc


def mouse_button_mask(name: str) -> int:
    lowered = name.lower()
    if lowered in ("left", "l"):
        return 0x01
    if lowered in ("right", "r"):
        return 0x02
    if lowered in ("middle", "m"):
        return 0x04
    raise ValueError(f"unknown mouse button {name!r}")


def _parse_mouse(parts: list[str]) -> ScriptCommand:
    if not parts:
        raise ValueError("mouse expects an action")
    action = parts[0].lower()
    if action == "move":
        _expect_count(parts, 3, "mouse move expects dx dy")
        return ScriptCommand(kind="mouse", action=action, dx=int(parts[1]), dy=int(parts[2]))
    if action in ("click", "down", "up"):
        _expect_count(parts, 2, f"mouse {action} expects button")
        return ScriptCommand(kind="mouse", action=action, button=mouse_button_mask(parts[1]))
    if action == "wheel":
        _expect_count(parts, 2, "mouse wheel expects delta")
        return ScriptCommand(kind="mouse", action=action, delta=int(parts[1]))
    raise ValueError(f"unknown mouse action {parts[0]}")


def _split_words(line: str) -> list[str]:
    out: list[str] = []
    current: list[str] = []
    in_quote = False
    i = 0
    while i < len(line):
        ch = line[i]
        if ch == '"':
            if in_quote:
                out.append("".join(current))
                current = []
                in_quote = False
                while i + 1 < len(line) and line[i + 1] in (" ", "\t"):
                    i += 1
            else:
                if current:
                    raise ValueError("quote must start a new token")
                in_quote = True
        elif ch == "\\" and in_quote:
            i += 1
            if i >= len(line):
                raise ValueError("trailing escape in quoted string")
            current.append({"n": "\n", "r": "\r", "t": "\t", '"': '"', "\\": "\\"}.get(line[i], line[i]))
        elif ch in (" ", "\t") and not in_quote:
            if current:
                out.append("".join(current))
                current = []
        elif ch == "#" and not in_quote and not current:
            break
        else:
            current.append(ch)
        i += 1

    if in_quote:
        raise ValueError("unterminated quoted string")
    if current:
        out.append("".join(current))
    return out


def _expect_count(parts: list[str], count: int, message: str) -> None:
    if len(parts) != count:
        raise ValueError(message)
