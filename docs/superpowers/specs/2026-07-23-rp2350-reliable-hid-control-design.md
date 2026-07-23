# RP2350 Reliable External HID Control Design

## Goal

Turn the Raspberry Pi Pico 2 / RP2350 firmware into a reliable externally
controlled USB keyboard and mouse. A Windows host application sends framed
commands over the device's CDC serial interface, and the RP2350 emits standard
USB HID keyboard and mouse reports to the same host.

The finished system must support stateful multi-key input, exact key release,
modifier-only operations, safe retries, cancellable long-running commands,
real buffered batches, automatic input release after controller failure, and
consistent behavior across the Rust CLI, Web Serial UI, Python SDK, and C++ SDK.

## Clarified Product Boundary

The Pico 2 is a command-controlled HID device, not a USB Host passthrough
device. It does not accept or forward a physical keyboard or mouse. Adding a
second USB Host port, PIO-USB wiring, downstream-device power, or physical HID
passthrough is outside this design.

The existing composite USB shape remains correct:

```text
Windows controller application
        |
        | CDC framed commands
        v
RP2350 scheduler and HID state machine
        |
        | USB HID reports
        v
Windows keyboard and mouse input stack
```

## Problems Covered

This design resolves the fourteen actionable problems remaining after the
original audit and screenshot were merged and deduplicated:

1. `KEY_DOWN` overwrites the previous keyboard report instead of holding
   multiple keys.
2. `KEY_UP` releases the whole keyboard instead of one requested key.
3. Python cannot represent modifier-only operations such as `SHIFT` or `CTRL`.
4. Closing or crashing the controller can leave keys or mouse buttons held.
5. SDK retries can execute non-idempotent movement, typing, or clicks twice.
6. Long waits, text entry, and movement block emergency commands.
7. `BUSY` is declared but never represents real device state.
8. `BATCH_BEGIN` and `BATCH_END` are currently marker-only ACKs.
9. Clicking an already-held mouse button corrupts its final state.
10. Protocol versions, flags, sequence conflicts, and partial-frame timeouts are
    not enforced.
11. ASCII typing can partially execute before rejection and ignores Caps Lock.
12. USB identity is fixed, development VID/PID values are not configurable, and
    the interfaces advertise unsupported Boot Protocol behavior.
13. Cargo's picotool runner contains a literal, non-expanded
    `${PICOTOOL_PATH}` executable name.
14. Host-state, integration, cross-SDK, recovery, and hardware acceptance tests
    are incomplete.

## Chosen Approach

Keep the existing frame envelope and command IDs where practical, but refactor
the firmware internals into independently testable receiver, scheduler, safety,
and HID execution components. Protocol v2 becomes the default. Valid v1 frames
remain accepted for basic compatibility, but new reliability guarantees are
advertised only to v2 clients.

This approach avoids a full ecosystem rewrite while removing the single-loop
architecture that makes preemption and reliable retries impossible.

## Firmware Architecture

### USB and CDC receiver

The CDC class is split into sender, receiver, and control-state handles. The
receiver owns byte-stream framing and performs these checks before a command can
reach the scheduler:

- magic and declared length;
- CRC16-CCITT-FALSE;
- supported protocol version;
- allowed flags for the specific command;
- payload length and command-specific payload shape;
- nonzero sequence for response-bearing commands;
- a 250 ms timeout for an incomplete frame.

Noise and malformed data are resynchronized without allowing an indefinitely
declared partial frame to block later traffic.

### Command scheduler

The scheduler owns all protocol-level mutable state:

- one active command or active batch;
- a high-priority cancellation path;
- the collecting batch buffer and its shadow input state;
- a 64-entry recent-response cache;
- the current controller lease deadline;
- the current cancellation generation.

Incoming payloads are copied into an owned, fixed-capacity `OwnedCommand` before
the CDC receive buffer is reused. Firmware code remains heap-free.

The scheduler is written as a host-testable state machine. Embassy-specific
channels, signals, timers, and USB writers are adapters around that state
machine rather than being embedded in its decision logic.

### HID executor

Only the HID executor may mutate or publish keyboard and mouse state. It owns:

```text
KeyboardState
  modifiers: u8
  keycodes: up to six distinct HID usages

MouseState
  buttons: u8
```

Long actions are cooperative. Timers, per-character typing, split mouse
movement, and batch steps all observe the cancellation generation between
units of work. `STOP_ALL`, DTR loss, lease expiry, and USB disable therefore
preempt an active operation without waiting for its full requested duration.

### Response sender

A single CDC sender serializes ACK, NACK, STATUS, and BUSY frames. Responses are
also represented as owned fixed-size values so the scheduler can cache and
replay them safely.

### Safety monitor

The control-state task observes DTR changes. A falling DTR edge cancels active
work, clears collected or executing batch work, clears retry state for the
ended session, and releases keyboard and mouse state.

Updated v2 clients send a heartbeat every 500 ms. The lease duration is 2 s.
Every valid v2 command also refreshes the lease. Lease expiry only takes action
while a key/button is held, a batch is open or running, or a long operation is
active. Expiry performs the same cancellation and release sequence as a DTR
disconnect.

Keyboard and mouse release are attempted independently. Failure to release one
interface does not prevent a best-effort release of the other.

## Protocol v2

### Frame compatibility

The existing frame envelope remains unchanged:

```text
magic[2] | version[1] | flags[1] | sequence[2] | command[1]
payload_length[2] | payload[0..240] | crc16[2]
```

Requests receive a response using their accepted request version. V1 accepts
only `flags = 0`. V2 accepts `flags = 0` for normal commands and defines
`NO_RESPONSE = 0x01` only for heartbeat frames.

When a CRC-valid frame carries an unsupported version, the device returns an
`UNSUPPORTED_VERSION` NACK encoded with the current v2 response format and the
request's extracted sequence. Transport errors that do not contain a reliable
sequence continue to use sequence zero.

Sequence zero is reserved for `NO_RESPONSE` heartbeat traffic. All commands
that expect a response use sequences 1 through 65535.

### Heartbeat

`HEARTBEAT` uses command ID `0x04` and an empty payload.

- `sequence = 0`, `NO_RESPONSE`: refresh the lease without producing CDC input
  traffic for the client reader.
- nonzero sequence, `flags = 0`: refresh the lease and return ACK, allowing
  manual diagnostics.

Any other use of `NO_RESPONSE` is rejected as unsupported flags.

### Retry and sequence semantics

The scheduler derives a 64-bit fingerprint from the accepted version, flags,
command type, payload length, and payload bytes.

For a sequence already known in the current connection session:

- same sequence and fingerprint, still executing: return `BUSY`;
- same sequence and fingerprint, already completed: replay the cached final
  response without executing again;
- same sequence but a different fingerprint: return `SEQUENCE_CONFLICT`.

The cache is a 64-entry ring. Reuse of a sequence after its entry ages out is a
new command, which is compatible with normal 16-bit sequence wraparound.

V2 `GET_CAPS` sets `RETRY_SAFE` only when these semantics are active. A v1
capability response does not claim the v2 retry guarantee. The firmware may
still deduplicate accepted v1 requests conservatively, but old clients are not
promised the lease, heartbeat, or extended-error contract.

### BUSY

BUSY becomes a real response. Its payload is:

```text
reason[1] | retry_after_ms[2]
```

Reasons cover an active duplicate, an exclusively executing batch, and an
occupied executor. Ordinary commands are not silently queued: ACK means their
action has completed, while an occupied executor returns BUSY and lets the
client retry the same frame. `STOP_ALL`, heartbeat, `PING`, `GET_INFO`, and
`GET_CAPS` bypass ordinary BUSY handling where safe.

### Extended errors

Existing error values remain stable. V2 adds symbolic errors for:

- unsupported version;
- unsupported flags;
- invalid or reserved sequence;
- sequence conflict;
- invalid batch state;
- batch capacity exceeded;
- too many simultaneously held keys;
- wait duration over 60 s;
- keyboard state incompatible with text entry;
- command cancelled.

NACK is terminal for that request. SDKs do not retry it automatically.

## Keyboard Semantics

`KEY_DOWN(modifiers, keycode)` applies the following state transition:

- OR the requested modifier bits into the held modifier mask;
- add a nonzero keycode if it is not already present;
- treat `keycode = 0` as a modifier-only operation;
- reject a seventh distinct non-modifier key without changing state.

`KEY_UP(modifiers, keycode)` clears only the requested modifier bits and removes
only the requested nonzero keycode. Releasing an already-released key is an
idempotent ACK.

This permits sequences such as:

```text
KEY_DOWN W
KEY_DOWN D
KEY_DOWN SHIFT-only
KEY_UP D
```

The final state is `W + SHIFT`.

`KEY_TAP` snapshots the preexisting keyboard state. For a key not currently
held, it publishes the union of the snapshot and tapped stroke, waits the
configured tap duration, and restores the snapshot. If the target is already
held, it emits a release/press pulse for that target and restores the original
held state, so the command produces an edge without losing other keys.

Cancellation always resolves to the all-released state rather than restoring a
pre-cancellation snapshot.

## Text Entry

`TYPE_ASCII` remains an explicitly US-keyboard ASCII facility rather than
claiming layout-independent Unicode support.

Before the first HID report is emitted, the complete payload is checked for:

- payload and command limits;
- supported ASCII characters;
- valid US-keyboard usage mapping;
- an idle keyboard state.

Rejecting text therefore has no partial side effects. Text entry while another
key or modifier is held returns a keyboard-busy error instead of generating
unintended chords.

The keyboard interface receives output LED reports and tracks Caps Lock. Letter
shift selection is adjusted so requested upper/lower case remains correct under
the US layout. CRLF is normalized to one Enter action.

Chinese and general Unicode text are outside the HID usage protocol and remain
unsupported. Raw key commands used by games are independent of the active
input method and keyboard character layout.

## Mouse Semantics

Mouse button down/up operations are stateful and idempotent.

`MOUSE_CLICK` snapshots the original button state:

- if the target button was released, emit down then restore released;
- if the target button was already held, emit a release/press pulse and restore
  held;
- preserve all unrelated mouse buttons in both cases.

Large relative movement remains split into HID-sized steps, but cancellation is
checked between steps. Wheel reports preserve held button state.

## Cancellable Commands

`WAIT_MS` accepts 0 through 60,000 ms. It races its timer against the urgent
cancellation signal.

Typing, tap delays, click delays, split mouse movement, and batch execution use
the same cancellation mechanism. When cancelled by `STOP_ALL`, the active
request or `BATCH_END` receives `CANCELLED` when the CDC connection can still
carry a response. `STOP_ALL` itself receives ACK after release has been
attempted.

DTR loss, USB disable, and lease expiry do not depend on being able to return a
response; safety release still occurs.

## Batch Semantics

`BATCH_BEGIN` opens one collecting batch. Nested begin, end-without-begin, and a
second concurrent batch are rejected.

While collecting:

- batchable keyboard, mouse, wait, and text commands are fully decoded and
  copied into fixed memory;
- a shadow input state initialized from the current real input state validates
  six-key limits, text-idle requirements, and state-dependent transitions
  before execution;
- each accepted command returns ACK meaning “validated and queued”;
- duplicate queued sequences replay that acceptance ACK without being queued
  twice;
- heartbeat, `PING`, `GET_INFO`, `GET_CAPS`, and `STOP_ALL` remain immediate;
- batch capacity is 32 commands and 8 KiB total payload.

`BATCH_END` starts exclusive ordered execution and receives its final response
after the batch completes. Ordinary mutating commands receive BUSY during that
execution. `STOP_ALL` can interrupt at every cooperative boundary, clears all
unstarted commands, and releases all input state. Outside an explicit batch,
there is no hidden command queue.

All commands are validated before execution and no unrelated command can
interleave. Already emitted physical HID events cannot be rolled back. If a
runtime HID transport failure occurs, remaining commands are discarded and a
best-effort all-input release is performed.

## USB Device Configuration

The device continues to expose CDC ACM, keyboard HID, and mouse HID interfaces.

The HID interfaces advertise standard Report mode rather than an unsupported
Boot subclass/protocol combination. The keyboard gains an output-report path
for LED state, including Caps Lock. Pre-OS BIOS keyboard/mouse operation is not
a goal.

The USB serial string is generated from the RP2350 flash/chip unique ID and
formatted as:

```text
EXQC-KMOUSE-<UNIQUE_HEX>
```

Build-time environment variables `RP2350_USB_VID` and `RP2350_USB_PID` override
the defaults. Development builds retain `0xCAFE:0x2350` with documentation that
production distribution requires legitimately assigned USB identifiers.

## SDK and Tool Behavior

The Python SDK, C++ SDK, Rust `hidctl`, and Web Serial UI use v2 by default and
share these policies:

- serialize writes so heartbeat and command frames cannot interleave;
- send a sequence-zero `NO_RESPONSE` heartbeat every 500 ms while open;
- retry only timeout and BUSY with the exact same sequence and frame;
- stop immediately on NACK;
- never clear the input buffer before a retry;
- ignore or dispatch stale responses without corrupting the pending request;
- calculate timeouts from command duration instead of applying 1 s globally;
- send `STOP_ALL` best-effort before an orderly close;
- explicitly assert DTR on open and deassert it on close where the platform API
  exposes control signals.

Timeout calculation includes a bounded transport margin:

- ordinary commands: 1 s minimum;
- `WAIT_MS`: requested duration plus 500 ms;
- ASCII typing: character count times tap delay plus 500 ms;
- mouse movement: HID step estimate plus 500 ms;
- `BATCH_END`: accumulated known batch duration plus 1 s.

Python, C++, Rust CLI, Web UI, and script parsers accept modifier-only forms.
`SHIFT`, `CTRL`, `ALT`, `GUI`, and combinations such as `CTRL+SHIFT` encode a
zero keycode with the requested modifier mask. More than one non-modifier key
in a single combo string remains invalid; simultaneous ordinary keys use
multiple `KEY_DOWN` calls.

The firmware repository's nested SDKs and the sibling C++ SDK used by
`cpp_analyzer` are synchronized to identical protocol behavior. Existing public
high-level method names remain stable where their semantics are being fixed
rather than removed.

## Flashing Workflow

Cargo cannot interpolate `${PICOTOOL_PATH}` in a runner string. The repository
adds a PowerShell runner script that:

- accepts Cargo's ELF artifact argument;
- uses `PICOTOOL_PATH` when set;
- otherwise resolves `picotool` from `PATH`;
- fails with a precise setup message when picotool is unavailable;
- invokes `load -u -v -x -t elf` only when explicitly run through `cargo run`.

`.cargo/config.toml` calls that script without embedding an unexpanded variable.
README and parent build documentation use the same verified commands.

No automated test or build step flashes hardware.

## Resource Limits

The implementation uses fixed capacities suitable for the Pico 2's 512 KiB
main SRAM:

| Resource | Limit |
|---|---:|
| Protocol payload | 240 bytes |
| Complete frame | 251 bytes |
| Held ordinary keys | 6 |
| Held modifiers | 8 bits |
| Wait duration | 60,000 ms |
| Batch commands | 32 |
| Batch payload storage | 8 KiB |
| Recent response cache | 64 entries |
| Partial-frame timeout | 250 ms |
| Heartbeat interval | 500 ms |
| Safety lease | 2,000 ms |

Capacity failures return explicit errors and leave input state unchanged.

## Testing Strategy

### Host-side firmware tests

Pure Rust tests cover:

- multi-key down/up and modifier-only state;
- six-key capacity and idempotent duplicate state operations;
- tap snapshot/restore and already-held tap pulses;
- mouse click state restoration;
- complete ASCII prevalidation, Caps Lock mapping, CRLF normalization, and
  keyboard-busy rejection;
- sequence replay, active BUSY, conflicts, cache eviction, and wraparound;
- batch collection, shadow-state validation, capacity limits, order,
  exclusivity, failure stop, and urgent cancellation;
- DTR reset, lease refresh/expiry, and independent best-effort interface
  release;
- cancellation of waits, typing, clicks, and split movement;
- v1/v2 version and flag rules, heartbeat forms, CRC, fragmented frames,
  malformed lengths, noise recovery, and 250 ms partial-frame timeout.

Fake keyboard, mouse, clock, CDC response, and cancellation adapters verify
observable reports without RP2350 hardware.

### SDK and tool tests

All implementations check the same published frame vectors for v1, v2,
heartbeat, BUSY, NACK, and status responses. SDK-specific tests cover:

- modifier-only combo parsing;
- same-frame retry behavior;
- no retry after NACK;
- duration-aware timeouts;
- heartbeat lifecycle and serialized writes;
- script batch begin/end and stop-on-error behavior.

CMake enables CTest and registers the C++ protocol executable. Python tests run
with `uv`, JavaScript tests run with Node, and Rust tools run with Cargo.

### Continuous verification

A Windows CI workflow runs formatting, host Rust tests, embedded `cargo check`,
embedded Clippy with warnings denied, release firmware build, `hidctl` tests,
Python SDK tests, Web protocol tests, and the C++ SDK test executable.

### Hardware acceptance

Hardware validation is manual and requires explicit user approval before
flashing or emitting HID input. The checklist verifies:

1. CDC, keyboard, and mouse enumeration with a unique serial number.
2. `W + D + SHIFT` simultaneous hold and exact release ordering.
3. Python modifier-only commands.
4. retrying the same click and movement sequence without duplicate actions.
5. immediate `STOP_ALL` during a long wait and batch.
6. automatic release after COM close, controller termination, and heartbeat
   expiry.
7. batch order and exclusion of interleaved ordinary commands.
8. Caps Lock-aware US ASCII entry.

## Error and Recovery Policy

Protocol or payload errors do not change HID state. Runtime HID errors abort the
active command or batch, discard remaining batched work when ordering can no
longer be guaranteed, signal the LED error pattern, and attempt an all-input
release.

Transport loss always favors released input over preserving a requested hold.
Activity and error signals remain best-effort observability and do not control
correctness.

## Non-Goals

- Physical USB keyboard or mouse passthrough.
- USB Host or PIO-USB support.
- Layout-independent Unicode or Chinese text injection.
- BIOS/pre-OS Boot Protocol operation.
- Cryptographic authentication of a local CDC controller.
- Automatic hardware flashing during tests or CI.

## Acceptance Criteria

The work is complete when all automated checks pass, no source tree contains a
known divergent protocol implementation, and every software-verifiable item in
the hardware checklist is covered by deterministic host tests. Claims about
enumeration or physical HID behavior require a separately approved live-board
run; they are not inferred from compilation alone.
