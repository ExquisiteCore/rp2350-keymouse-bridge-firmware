# RP2350 HID Bridge Python SDK

Python SDK for the ExquisiteCore RP2350 KeyMouse Bridge. It talks to the board through the CDC serial command endpoint and the board emits standard USB HID keyboard and mouse reports.

## Install

From the repository root:

```powershell
pip install -e sdk/python
```

The package depends on `pyserial>=3.5`.

## Find The Device

The firmware uses VID/PID `CAFE:2350`. Passing `port=None` enables automatic discovery:

```python
from rp2350_hid_bridge import HidBridge, HidBridgeOptions

with HidBridge(HidBridgeOptions(port=None)) as hid:
    hid.ping()
```

List all serial ports:

```powershell
$env:PYTHONPATH='sdk\python'
python sdk\python\examples\list_ports.py
```

## Direct Control API

```python
from rp2350_hid_bridge import HidBridge, HidBridgeOptions

with HidBridge(HidBridgeOptions(port="COM3")) as hid:
    hid.ping()
    print(hid.info().hex(" "))
    print(hid.caps().hex(" "))

    hid.type_text("hello")
    hid.key_tap("ENTER")
    hid.key_down("CTRL")
    hid.key_up("CTRL")

    hid.mouse_move(10, -5)
    hid.mouse_click("left")
    hid.mouse_down("right")
    hid.mouse_up("right")
    hid.mouse_wheel(-1)

    hid.wait_ms(100)
    hid.stop_all()
```

Common key names include letters, digits, `ENTER`, `ESC`, `TAB`, `SPACE`, `F1`-`F12`, arrows, `HOME`, `END`, `PAGEUP`, `PAGEDOWN`, `DELETE`, `INSERT`, and punctuation names such as `SLASH`, `DOT`, `COMMA`, `BACKSLASH`.

Modifiers are combined with `+`: `CTRL+C`, `SHIFT+F5`, `ALT+TAB`, `WIN+R`.

## Script API

Scripts are useful for short batches:

```python
script = '''
type "hello from script"
key tap ENTER
mouse move 20 0
mouse click left
wait 100
stop
'''

with HidBridge(HidBridgeOptions(port="COM3")) as hid:
    hid.run_script(script)
```

Supported commands:

```text
type "ASCII text"
key tap|down|up COMBO
mouse move DX DY
mouse click|down|up left|right|middle
mouse wheel DELTA
wait MILLISECONDS
stop
```

Preview the bundled script without sending input:

```powershell
$env:PYTHONPATH='sdk\python'
python sdk\python\examples\script_demo.py
```

Send it intentionally:

```powershell
$env:PYTHONPATH='sdk\python'
python sdk\python\examples\script_demo.py --run --port COM3
```

## Error Handling

The client retries `BUSY` responses, raises `RuntimeError` for `NACK`, and raises `TimeoutError` if no matching response frame arrives before the configured timeout.

```python
from rp2350_hid_bridge import HidBridge, HidBridgeOptions

try:
    with HidBridge(HidBridgeOptions(port=None, timeout=1.0, retries=2)) as hid:
        hid.type_text("safe text")
except TimeoutError:
    print("device did not respond")
except RuntimeError as exc:
    print(f"device/client error: {exc}")
```

The examples produce real keyboard and mouse input only when explicitly run against a device. Make sure the active window is safe before sending commands.
