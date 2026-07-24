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
tools/flash.ps1       Explicit picotool runner with a safe resolve-only mode
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

`cargo build --release` only compiles and links the firmware. It does not run
picotool, flash a board, open a serial port, or emit HID input.

Output:

```text
target\thumbv8m.main-none-eabihf\release\rp2350-keymouse-bridge-firmware
```

The root package is a firmware binary. `Cargo.lock` is intentionally committed
for reproducible firmware builds.

### USB identity

Development builds default to USB VID/PID `0xCAFE:0x2350`. Override either ID
at build time with a decimal or `0x`-prefixed hexadecimal `u16` value:

```powershell
$env:RP2350_USB_VID = "0x1234"
$env:RP2350_USB_PID = "0x5678"
cargo build --release
```

Invalid values fail the build. Production distribution must use legitimately
assigned USB identifiers.

At startup the firmware reads the RP2350 OTP chip ID and exposes the USB serial
string `EXQC-KMOUSE-` followed by 16 uppercase hexadecimal digits. This gives
each chip a stable identity for CDC/HID enumeration. The current code has no
shared or fabricated fallback serial: if `embassy_rp::otp::get_chipid()` fails,
startup panics before USB enumeration.

## Run Host-Side Tests

Pure protocol and parser tests can run on the Windows host:

```powershell
cargo test --target x86_64-pc-windows-msvc --lib
```

These tests use pure state machines and fake transports. Automated tests must
never flash firmware, select a serial device, or emit real HID input; hardware
acceptance is a separate, manually approved procedure.

## Flash Firmware

`.cargo\config.toml` configures the RP2350 target and delegates its runner to a
PowerShell script:

```text
runner = "powershell -NoProfile -ExecutionPolicy Bypass -File tools/flash.ps1"
```

Set `PICOTOOL_PATH` or put `picotool` on `PATH`. The safe resolver check requires
an existing build artifact but does not invoke picotool:

```powershell
$env:PICOTOOL_PATH = "D:\Tool\picotool\picotool.exe"
powershell -NoProfile -ExecutionPolicy Bypass -File tools/flash.ps1 target\thumbv8m.main-none-eabihf\release\rp2350-keymouse-bridge-firmware -ResolveOnly
```

Flashing is a separate, explicit hardware action. Only after putting the board
in BOOTSEL mode and deciding to flash it, omit `-ResolveOnly`:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools/flash.ps1 target\thumbv8m.main-none-eabihf\release\rp2350-keymouse-bridge-firmware
```

That command runs `picotool load -u -v -x -t elf` and is intentionally not part
of build or test verification. Likewise, `cargo run --release` invokes the
configured runner and is a flashing command, not a build-only check.

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

## Protocol and safety summary

The serial endpoint uses CRC-checked framed commands. Protocol v2 is the
default. Valid v1 requests with zero flags and a nonzero sequence remain
accepted for basic commands, but v1 `GET_CAPS` does not advertise v2 retry,
lease, or cancellation guarantees. V2 reserves sequence zero plus
`NO_RESPONSE` for heartbeat traffic; normal request/response commands use a
nonzero sequence.

Supported high-level actions include:

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

Keyboard state contains eight modifier bits and up to six distinct non-modifier
keycodes. A zero keycode is a modifier-only operation, so Shift/Ctrl/Alt/GUI can
be held or released without changing ordinary keys. A seventh distinct key is
rejected transactionally: neither the key nor any modifier bits from that
request are added. `KEY_UP` removes only the requested key and modifier bits.

V2 clients send a no-response heartbeat every 500 ms. Valid v2 traffic refreshes
a two-second control lease. Lease expiry acts only while guarded work exists
(held input, an open/running batch, or active execution), then cancels work and
attempts to release all keyboard and mouse state. A falling DTR edge or USB
disable performs the same safety reset. V1 requests deliberately do not arm the
lease because legacy clients do not send heartbeats.

`BATCH_BEGIN` collects at most 32 commands and 8 KiB of payload using a shadow
input state; every accepted entry is validated before `BATCH_END` begins
exclusive, ordered execution. This is validation-transactional, not a rollback
of physical HID reports: reports already emitted before an error or stop cannot
be undone. `STOP_ALL`, DTR loss, USB disable, or lease expiry can cancel waits,
typing characters, movement chunks, tap/click delays, and batch work at their
cooperative boundaries. Unstarted batch commands are discarded and all input
state is released best-effort. Outside an explicit batch there is no hidden
ordinary-command queue.

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
the active host environment is expected. The verification documented for this
change is build-only plus `tools/flash.ps1 -ResolveOnly`; it does not flash a
board or send serial/HID commands.
