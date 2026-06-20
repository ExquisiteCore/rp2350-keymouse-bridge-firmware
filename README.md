# RP2350 KeyMouse Bridge Firmware

Rust firmware for the ExquisiteCore RP2350 KeyMouse Bridge.

The board enumerates as a USB composite device:

```text
CDC serial endpoint     receives framed control commands
USB HID keyboard        emits standard keyboard reports
USB HID mouse           emits standard relative mouse reports
```

Host applications can control the board through the serial protocol by using
`tools/hidctl`, the C++ SDK, or the Python SDK.

## Repository Layout

```text
src/                  RP2350 firmware source
tools/hidctl/         Windows host CLI for protocol checks
tools/webui/          Web Serial protocol/debug UI
sdk/cpp/              C++17 header-only SDK submodule
sdk/python/           Python SDK submodule
.cargo/config.toml    RP2350 target and picotool runner config
rp2350.x              linker script
```

Nested SDK repositories:

```text
sdk/cpp    -> https://github.com/ExquisiteCore/rp2350-hid-bridge-cpp
sdk/python -> https://github.com/ExquisiteCore/rp2350-hid-bridge-python
```

## Requirements

```text
Rust stable with edition 2024 support
rustup target thumbv8m.main-none-eabihf
picotool for USB flashing
Visual Studio 2022 Build Tools for Windows host tools
```

Install the embedded target:

```powershell
rustup target add thumbv8m.main-none-eabihf
```

## Build Firmware

From the firmware repository root:

```powershell
cargo build --release
```

Output:

```text
target\thumbv8m.main-none-eabihf\release\rp2350-keymouse-bridge-firmware
```

The root package is a firmware binary. `Cargo.lock` is intentionally committed
for reproducible firmware builds.

## Run Host-Side Tests

Pure protocol and parser tests can run on the Windows host:

```powershell
cargo test --target x86_64-pc-windows-msvc --lib
```

## Flash Firmware

`.cargo\config.toml` configures the RP2350 target and a picotool runner:

```text
runner = "${PICOTOOL_PATH} load -u -v -x -t elf"
```

Set `PICOTOOL_PATH`, put the board into BOOTSEL mode, then run:

```powershell
$env:PICOTOOL_PATH = "D:\Tool\picotool\picotool.exe"
cargo run --release
```

Manual picotool command:

```powershell
& $env:PICOTOOL_PATH load -u -v -x -t elf target\thumbv8m.main-none-eabihf\release\rp2350-keymouse-bridge-firmware
```

After flashing, the device should expose a CDC serial COM port and USB HID
keyboard/mouse interfaces.

## Build hidctl

`tools/hidctl` is a Windows host command-line tool for checking the serial
protocol and sending controlled commands.

Build:

```powershell
cargo build --manifest-path tools\hidctl\Cargo.toml --release --target x86_64-pc-windows-msvc
```

Run:

```powershell
.\tools\hidctl\target\x86_64-pc-windows-msvc\release\hidctl.exe --help
```

List ports:

```powershell
.\tools\hidctl\target\x86_64-pc-windows-msvc\release\hidctl.exe list
```

Ping a board:

```powershell
.\tools\hidctl\target\x86_64-pc-windows-msvc\release\hidctl.exe --port COM3 ping
```

Read device info and capabilities:

```powershell
.\tools\hidctl\target\x86_64-pc-windows-msvc\release\hidctl.exe --port COM3 info
.\tools\hidctl\target\x86_64-pc-windows-msvc\release\hidctl.exe --port COM3 caps
```

Mouse movement check:

```powershell
.\tools\hidctl\target\x86_64-pc-windows-msvc\release\hidctl.exe --port COM3 mouse move 100 0
```

Run a script:

```powershell
.\tools\hidctl\target\x86_64-pc-windows-msvc\release\hidctl.exe --port COM3 run examples\hidctl-demo.txt
```

## SDK Usage

C++ SDK:

```powershell
cd sdk\cpp
cmake -S . -B build -G "Visual Studio 17 2022" -A x64
cmake --build build --config Release
.\build\Release\test_protocol.exe
```

Python SDK:

```powershell
cd sdk\python
python -m venv .venv
.\.venv\Scripts\python -m pip install -U pip
.\.venv\Scripts\python -m pip install -e .
.\.venv\Scripts\python -m unittest discover -s tests
```

## Protocol Summary

The serial endpoint uses framed binary commands with CRC checking. Supported
high-level actions include:

```text
ping
get info / get caps
key tap / key down / key up
type ASCII text
mouse relative move
mouse button down / up / click
mouse wheel
wait
batch begin / batch end
stop all
```

The firmware acknowledges accepted commands, reports busy status when command
execution is still in progress, and returns NACK on invalid frames or unsupported
payloads.

## LED Status

The onboard LED provides basic state feedback:

```text
Disconnected breathing   USB not connected to a host
Connected heartbeat      host connected
Activity flash           command accepted/executed
Error triple blink       invalid command or protocol error
```

## Notes

The firmware emits real USB HID input. Use host tools and SDK examples only when
the active host environment is expected.
