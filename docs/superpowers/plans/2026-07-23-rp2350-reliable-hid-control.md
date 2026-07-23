# RP2350 Reliable External HID Control Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refactor the Pico 2/RP2350 CDC-to-HID firmware and every bundled client so multi-key state, safe retries, cancellation, real batches, disconnect recovery, and protocol v2 work consistently.

**Architecture:** Preserve the 11-byte frame envelope while adding validated v2 semantics. Move keyboard/mouse state, batching, retry admission, and lease logic into heap-free host-testable modules; then connect them to split Embassy CDC/HID tasks through fixed-capacity channels and urgent cancellation signals. Update Python, C++, Rust CLI, and Web clients only after the firmware protocol vectors are stable.

**Tech Stack:** Rust 2024 `no_std`, Embassy RP/USB/executor/sync/time, `heapless`, `usbd-hid`, Python 3.10+ with `uv`, C++17/MSVC/CMake, browser Web Serial, Node test runner, PowerShell, GitHub Actions Windows runners.

---

## Preconditions and repository boundaries

- Approved design: `docs/superpowers/specs/2026-07-23-rp2350-reliable-hid-control-design.md`.
- Firmware repository root: `tools/rp2350_keymouse_bridge_firmware`.
- Nested SDK repositories: `sdk/python` and `sdk/cpp`.
- Sibling C++ SDK used by `cpp_analyzer`: `../rp2350_hid_bridge_cpp`.
- Parent integration repository: `../..`.
- Do not run `cargo run`, picotool, or any live HID command without a separate explicit approval.
- Keep v1 framing compatibility, but make v2 the default for updated clients.

## File map

### New firmware files

- `src/input_state.rs` — pure six-key/modifier and mouse-button state transitions.
- `src/owned_command.rs` — fixed-capacity owned form of decoded commands.
- `src/batch.rs` — collecting batch, capacity accounting, and shadow-state validation.
- `src/coordinator.rs` — sequence admission, response replay, BUSY decisions, and batch lifecycle.
- `src/safety.rs` — lease timing, cancellation generation, and partial-frame deadline helpers.
- `src/runtime.rs` — Embassy CDC dispatcher, executor, response, DTR, lease, and LED-report tasks.
- `tools/flash.ps1` — Cargo runner that resolves picotool correctly.
- `.github/workflows/ci.yml` — Windows verification pipeline.

### Existing firmware files to modify

- `Cargo.toml`, `Cargo.lock`, `build.rs`, `.cargo/config.toml`.
- `src/lib.rs`, `src/main.rs`, `src/protocol.rs`, `src/commands.rs`, `src/error.rs`.
- `src/firmware_config.rs`, `src/frame_stream.rs`, `src/command_executor.rs`.
- `src/hid_report.rs`, `src/response_writer.rs`, `src/static_resources.rs`.
- `src/usb_device.rs`, `src/usb_identity.rs`, `README.md`.

### Client and integration files to modify

- Python: `sdk/python/rp2350_hid_bridge/{protocol,keys,client,script}.py`, `sdk/python/tests/test_sdk.py`, `sdk/python/README.md`.
- Nested C++: `sdk/cpp/include/rp2350_hid_bridge/{protocol,keys,serial,script}.hpp`, `sdk/cpp/tests/test_protocol.cpp`, `sdk/cpp/CMakeLists.txt`, `sdk/cpp/README.md`.
- Sibling C++: mirror the same files under `../rp2350_hid_bridge_cpp`.
- Rust CLI: `tools/hidctl/src/{client,keys,main,script}.rs` and its tests.
- Web: `tools/webui/{protocol,keys,app,script}.js`, `tools/webui/tests/protocol.test.mjs`.
- Parent docs that repeat the broken flash command: `../../docs/BUILD.md` and `../../README.md`.

## Spec coverage index

| Approved requirement | Implemented by |
|---|---|
| Protocol v2, version/flags/sequence validation | Tasks 1, 4, 5 |
| Multi-key state, exact release, modifier-only input | Tasks 2, 6, 9–11 |
| Retry deduplication, replay, BUSY | Tasks 4, 7, 9–11 |
| Cancellable waits/text/movement/clicks | Tasks 5–7 |
| Real fixed-capacity Batch | Tasks 3, 4, 7 |
| DTR and heartbeat safety release | Tasks 5, 7, 9–11 |
| Mouse click state restoration | Tasks 2, 6 |
| ASCII prevalidation, Caps Lock, CRLF | Tasks 2, 6, 8 |
| Unique serial, configurable VID/PID, Report mode | Task 8 |
| Working picotool runner and accurate docs | Task 12 |
| Cross-SDK parity, CI, regression matrix | Tasks 9–14 |
| No unapproved flashing or HID injection | Tasks 12–14 |

---

### Task 1: Add protocol v2 primitives and strict validation

**Files:**
- Modify: `src/protocol.rs`
- Modify: `src/commands.rs`
- Modify: `src/error.rs`
- Modify: `src/firmware_config.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write failing protocol v2 tests**

Add tests that pin the wire contract:

```rust
#[test]
fn v2_heartbeat_supports_no_response_flag() {
    let mut buf = [0u8; MAX_FRAME_SIZE];
    let len = encode_frame_with_flags(
        2,
        FLAG_NO_RESPONSE,
        0,
        CommandType::Heartbeat,
        &[],
        &mut buf,
    )
    .unwrap();
    let frame = decode_frame(&buf[..len]).unwrap();
    assert_eq!(validate_request(&frame), Ok(RequestKind::NoResponseHeartbeat));
}

#[test]
fn v1_rejects_flags_and_v2_rejects_zero_command_sequence() {
    let v1 = Frame { version: 1, flags: FLAG_NO_RESPONSE, sequence: 1,
        command_type: CommandType::Ping, payload: &[] };
    assert_eq!(validate_request(&v1), Err(RequestError::UnsupportedFlags));

    let v2 = Frame { version: 2, flags: 0, sequence: 0,
        command_type: CommandType::Ping, payload: &[] };
    assert_eq!(validate_request(&v2), Err(RequestError::InvalidSequence));
}

#[test]
fn wait_is_limited_to_sixty_seconds() {
    let frame = Frame { version: 2, flags: 0, sequence: 9,
        command_type: CommandType::WaitMs, payload: &[0, 0, 0xEA, 0x61] };
    assert_eq!(decode_command(&frame), Err(CommandError::WaitTooLong));
}
```

- [ ] **Step 2: Run the focused tests and verify RED**

Run:

```powershell
cargo test --target x86_64-pc-windows-msvc --lib protocol::tests
cargo test --target x86_64-pc-windows-msvc --lib commands::tests
```

Expected: compilation fails because `Heartbeat`, `FLAG_NO_RESPONSE`, `encode_frame_with_flags`, `validate_request`, `RequestKind`, `RequestError`, and `WaitTooLong` do not exist.

- [ ] **Step 3: Implement the v2 protocol surface**

Use these public definitions and keep `encode_frame` as a flags-zero compatibility wrapper:

```rust
pub const LEGACY_PROTOCOL_VERSION: u8 = 1;
pub const PROTOCOL_VERSION: u8 = 2;
pub const FLAG_NO_RESPONSE: u8 = 0x01;
pub const MAX_WAIT_MS: u32 = 60_000;

pub enum CommandType {
    Ping,
    GetInfo,
    GetCaps,
    Heartbeat, // 0x04
    // existing command and response variants remain on their current IDs
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestKind {
    ResponseExpected,
    NoResponseHeartbeat,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestError {
    UnsupportedVersion,
    UnsupportedFlags,
    InvalidSequence,
}

pub fn validate_request(frame: &Frame<'_>) -> Result<RequestKind, RequestError> {
    match frame.version {
        LEGACY_PROTOCOL_VERSION if frame.flags != 0 => Err(RequestError::UnsupportedFlags),
        LEGACY_PROTOCOL_VERSION if frame.sequence == 0 => Err(RequestError::InvalidSequence),
        LEGACY_PROTOCOL_VERSION => Ok(RequestKind::ResponseExpected),
        PROTOCOL_VERSION
            if frame.command_type == CommandType::Heartbeat
                && frame.flags == FLAG_NO_RESPONSE
                && frame.sequence == 0 => Ok(RequestKind::NoResponseHeartbeat),
        PROTOCOL_VERSION if frame.flags != 0 => Err(RequestError::UnsupportedFlags),
        PROTOCOL_VERSION if frame.sequence == 0 => Err(RequestError::InvalidSequence),
        PROTOCOL_VERSION => Ok(RequestKind::ResponseExpected),
        _ => Err(RequestError::UnsupportedVersion),
    }
}
```

Make CRC cover the caller-provided version and flags. Add `Command::Heartbeat` and reject waits above `MAX_WAIT_MS` during decode. Extend `ErrorCode` without renumbering existing values:

```rust
UnsupportedVersion = 7,
UnsupportedFlags = 8,
InvalidSequence = 9,
SequenceConflict = 10,
BatchState = 11,
BatchCapacity = 12,
TooManyKeys = 13,
WaitTooLong = 14,
KeyboardBusy = 15,
Cancelled = 16,
```

Make `capability_payload(request_version)` clear `CAP_RETRY_SAFE`, lease, and cancellation bits for v1 and set them for v2.

- [ ] **Step 4: Run protocol tests and verify GREEN**

Run:

```powershell
cargo test --target x86_64-pc-windows-msvc --lib protocol::tests
cargo test --target x86_64-pc-windows-msvc --lib commands::tests
cargo test --target x86_64-pc-windows-msvc --lib firmware_config::tests
```

Expected: all focused tests pass.

- [ ] **Step 5: Commit protocol v2 primitives**

```powershell
git add src/protocol.rs src/commands.rs src/error.rs src/firmware_config.rs src/lib.rs
git commit -m "feat: define reliable HID protocol v2"
```

---

### Task 2: Implement pure stateful keyboard and mouse transitions

**Files:**
- Create: `src/input_state.rs`
- Modify: `src/lib.rs`
- Modify: `src/commands.rs`

- [ ] **Step 1: Write failing state transition tests**

Create tests in `src/input_state.rs`:

```rust
#[test]
fn holds_multiple_keys_and_releases_exactly_one() {
    let mut state = InputState::new();
    state.keyboard.key_down(KeyStroke { modifier: 0, keycode: 0x1A }).unwrap(); // W
    state.keyboard.key_down(KeyStroke { modifier: 0, keycode: 0x07 }).unwrap(); // D
    state.keyboard.key_down(KeyStroke { modifier: 0x02, keycode: 0 }).unwrap(); // Shift
    state.keyboard.key_up(KeyStroke { modifier: 0, keycode: 0x07 });
    assert_eq!(state.keyboard.modifiers(), 0x02);
    assert_eq!(state.keyboard.keycodes(), &[0x1A]);
}

#[test]
fn seventh_distinct_key_is_rejected_without_mutation() {
    let mut keyboard = KeyboardState::new();
    for key in 0x04..=0x09 {
        keyboard.key_down(KeyStroke { modifier: 0, keycode: key }).unwrap();
    }
    let before = keyboard;
    assert_eq!(
        keyboard.key_down(KeyStroke { modifier: 0, keycode: 0x0A }),
        Err(InputError::TooManyKeys)
    );
    assert_eq!(keyboard, before);
}

#[test]
fn click_plan_restores_an_already_held_button() {
    let state = MouseState::from_buttons(0x01);
    assert_eq!(
        state.click_plan(MouseButton::Left),
        MousePulse::ReleasePressRestore { released: 0, pressed: 1, restore: 1 }
    );
}
```

- [ ] **Step 2: Run the new module tests and verify RED**

Run:

```powershell
cargo test --target x86_64-pc-windows-msvc --lib input_state::tests
```

Expected: compilation fails because the module and types are absent.

- [ ] **Step 3: Implement fixed-capacity input state**

Implement these exact responsibilities:

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyboardState {
    modifiers: u8,
    keycodes: [u8; 6],
    len: u8,
}

impl KeyboardState {
    pub const fn new() -> Self;
    pub fn modifiers(&self) -> u8;
    pub fn keycodes(&self) -> &[u8];
    pub fn is_idle(&self) -> bool;
    pub fn key_down(&mut self, stroke: KeyStroke) -> Result<(), InputError>;
    pub fn key_up(&mut self, stroke: KeyStroke);
    pub fn clear(&mut self);
    pub fn tap_plan(&self, stroke: KeyStroke) -> Result<KeyboardPulse, InputError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MouseState { buttons: u8 }

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputState {
    pub keyboard: KeyboardState,
    pub mouse: MouseState,
}
```

Duplicate down/up operations are idempotent. Modifier-only strokes use
`keycode == 0`. `KeyboardPulse` and `MousePulse` contain complete report states
for down/restore or release/press/restore sequences so embedded code does not
reimplement state decisions.

Add a pure `ascii_strokes(bytes, caps_lock, out)` validator that fills a
`heapless::Vec<KeyStroke, 240>`, normalizes CRLF to one Enter, XORs letter Shift
with Caps Lock, and returns before emitting any reports if a byte is unsupported.

- [ ] **Step 4: Run all pure state tests and verify GREEN**

```powershell
cargo test --target x86_64-pc-windows-msvc --lib input_state::tests
cargo test --target x86_64-pc-windows-msvc --lib commands::tests
```

Expected: multi-key, exact-release, modifier-only, pulse, ASCII, CRLF, and capacity tests pass.

- [ ] **Step 5: Commit input state**

```powershell
git add Cargo.toml Cargo.lock src/input_state.rs src/lib.rs src/commands.rs
git commit -m "feat: add stateful six-key HID model"
```

---

### Task 3: Add owned commands and real batch collection

**Files:**
- Create: `src/owned_command.rs`
- Create: `src/batch.rs`
- Modify: `src/lib.rs`
- Modify: `Cargo.toml`

- [ ] **Step 1: Write failing owned-command and batch tests**

```rust
#[test]
fn batch_validates_shadow_state_before_accepting_text() {
    let mut batch = BatchCollector::begin(InputState::new());
    batch.push(OwnedCommand::KeyDown(KeyStroke { modifier: 0, keycode: 0x1A })).unwrap();
    assert_eq!(
        batch.push(OwnedCommand::type_ascii(b"abc").unwrap()),
        Err(BatchError::KeyboardBusy)
    );
    assert_eq!(batch.len(), 1);
}

#[test]
fn batch_enforces_command_and_payload_capacity_without_partial_push() {
    let mut batch = BatchCollector::begin(InputState::new());
    for _ in 0..BATCH_MAX_COMMANDS {
        batch.push(OwnedCommand::WaitMs(1)).unwrap();
    }
    assert_eq!(batch.push(OwnedCommand::WaitMs(1)), Err(BatchError::Capacity));
    assert_eq!(batch.len(), BATCH_MAX_COMMANDS);
}
```

- [ ] **Step 2: Run focused tests and verify RED**

```powershell
cargo test --target x86_64-pc-windows-msvc --lib batch::tests
```

Expected: the new modules are unresolved.

- [ ] **Step 3: Implement fixed owned forms and batch shadow validation**

Add direct `heapless = "0.9"` dependency and define:

```rust
pub const BATCH_MAX_COMMANDS: usize = 32;
pub const BATCH_MAX_PAYLOAD_BYTES: usize = 8 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OwnedCommand {
    Ping,
    GetInfo,
    GetCaps,
    Heartbeat,
    KeyDown(KeyStroke),
    KeyUp(KeyStroke),
    KeyTap(KeyStroke),
    TypeAscii(heapless::Vec<u8, 240>),
    MouseMoveRel { dx: i16, dy: i16 },
    MouseButtonDown(MouseButton),
    MouseButtonUp(MouseButton),
    MouseClick(MouseButton),
    MouseWheel(i8),
    WaitMs(u32),
    StopAll,
}

impl OwnedCommand {
    pub fn from_frame(frame: &Frame<'_>) -> Result<Self, CommandError>;
    pub fn type_ascii(bytes: &[u8]) -> Result<Self, CommandError>;
    pub fn payload_len(&self) -> usize;
}

pub struct BatchCollector {
    commands: heapless::Vec<OwnedCommand, BATCH_MAX_COMMANDS>,
    payload_bytes: usize,
    shadow: InputState,
}
```

`OwnedCommand::from_frame` decodes and copies borrowed payloads once. Batch
`push` first applies the command to a copy of `shadow`, then commits both the
shadow and command only if validation and both capacities succeed. Queries,
heartbeat, nested batch markers, and `STOP_ALL` are not stored as batch actions.

- [ ] **Step 4: Run batch tests and verify GREEN**

```powershell
cargo test --target x86_64-pc-windows-msvc --lib owned_command::tests
cargo test --target x86_64-pc-windows-msvc --lib batch::tests
```

Expected: all owned-copy, shadow-state, order, and capacity tests pass.

- [ ] **Step 5: Commit real batch collection primitives**

```powershell
git add Cargo.toml Cargo.lock src/owned_command.rs src/batch.rs src/lib.rs
git commit -m "feat: add validated fixed-capacity batches"
```

---

### Task 4: Implement retry admission and response replay

**Files:**
- Create: `src/coordinator.rs`
- Modify: `src/lib.rs`
- Modify: `src/error.rs`

- [ ] **Step 1: Write failing coordinator tests**

```rust
#[test]
fn active_duplicate_is_busy_then_completed_duplicate_replays() {
    let request = request(2, 41, CommandType::MouseClick, &[1]);
    let mut coordinator = Coordinator::new();
    assert!(matches!(coordinator.admit(&request), Admission::Execute(_)));
    assert!(matches!(coordinator.admit(&request), Admission::Busy(BusyReason::ActiveDuplicate)));

    coordinator.complete(41, CachedResponse::ack(2, 41)).unwrap();
    assert_eq!(
        coordinator.admit(&request),
        Admission::Replay(CachedResponse::ack(2, 41))
    );
}

#[test]
fn reused_sequence_with_different_payload_conflicts() {
    let mut coordinator = Coordinator::new();
    let first = request(2, 9, CommandType::MouseMoveRel, &[0, 1, 0, 1]);
    let second = request(2, 9, CommandType::MouseMoveRel, &[0, 2, 0, 1]);
    assert!(matches!(coordinator.admit(&first), Admission::Execute(_)));
    assert_eq!(coordinator.admit(&second), Admission::Reject(ErrorCode::SequenceConflict));
}
```

- [ ] **Step 2: Run coordinator tests and verify RED**

```powershell
cargo test --target x86_64-pc-windows-msvc --lib coordinator::tests
```

Expected: module and types are missing.

- [ ] **Step 3: Implement coordinator and 64-entry replay cache**

Define fixed response and decision types:

```rust
pub const RESPONSE_CACHE_SIZE: usize = 64;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CachedResponse {
    pub version: u8,
    pub sequence: u16,
    pub command_type: CommandType,
    pub payload: heapless::Vec<u8, 16>,
}

impl CachedResponse {
    pub fn ack(version: u8, sequence: u16) -> Self;
    pub fn nack(version: u8, sequence: u16, error: ErrorCode) -> Self;
    pub fn busy(version: u8, sequence: u16, reason: BusyReason, retry_ms: u16) -> Self;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Admission {
    Execute(OwnedRequest),
    Collect(OwnedRequest),
    Replay(CachedResponse),
    Busy(BusyReason),
    Immediate(CachedResponse),
    Reject(ErrorCode),
}
```

Use a deterministic FNV-1a 64-bit fingerprint over version, flags, command byte,
payload length, and payload. Store `{sequence, fingerprint, state, response}` in
a 64-entry ring. BUSY is interim and is not cached as completion. `STOP_ALL`,
heartbeat, and read-only queries bypass an occupied executor; mutating ordinary
commands return BUSY instead of being hidden in a normal queue.

Implement collecting/executing batch state transitions in the coordinator,
including duplicate acceptance replay and `BATCH_END` ownership.

- [ ] **Step 4: Run coordinator and batch tests and verify GREEN**

```powershell
cargo test --target x86_64-pc-windows-msvc --lib coordinator::tests
cargo test --target x86_64-pc-windows-msvc --lib batch::tests
```

Expected: duplicate, conflict, BUSY, cache eviction, session clear, batch lifecycle, and stop-bypass tests pass.

- [ ] **Step 5: Commit reliable admission**

```powershell
git add src/coordinator.rs src/error.rs src/lib.rs
git commit -m "feat: deduplicate HID commands by sequence"
```

---

### Task 5: Add frame deadlines, lease state, and cancellation generation

**Files:**
- Create: `src/safety.rs`
- Modify: `src/frame_stream.rs`
- Modify: `src/firmware_config.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write failing safety tests**

```rust
#[test]
fn lease_expires_only_when_guarded_work_exists() {
    let mut lease = LeaseState::new(2_000);
    lease.refresh(100);
    assert!(!lease.should_release(2_101, false));
    assert!(lease.should_release(2_101, true));
}

#[test]
fn partial_frame_deadline_clears_stalled_data() {
    let mut deadline = PartialFrameDeadline::new(250);
    deadline.note_bytes(1_000, 4);
    assert!(!deadline.expired(1_249));
    assert!(deadline.expired(1_250));
    deadline.clear();
    assert!(!deadline.expired(9_999));
}

#[test]
fn cancellation_generation_changes_once_per_cancel() {
    let mut generation = CancellationGeneration::new();
    let before = generation.current();
    generation.cancel();
    assert_ne!(generation.current(), before);
}
```

- [ ] **Step 2: Run safety tests and verify RED**

```powershell
cargo test --target x86_64-pc-windows-msvc --lib safety::tests
```

Expected: safety module is absent.

- [ ] **Step 3: Implement pure safety helpers**

Use constants:

```rust
pub const PARTIAL_FRAME_TIMEOUT_MS: u64 = 250;
pub const HEARTBEAT_INTERVAL_MS: u64 = 500;
pub const CONTROL_LEASE_MS: u64 = 2_000;
```

`LeaseState` stores the most recent refresh deadline with wrapping-safe elapsed
comparisons used by host tests. `PartialFrameDeadline` starts when the first
unconsumed byte arrives, refreshes when progress arrives, and clears when the
buffer empties or is rejected. `CancellationGeneration` wraps a `u32` and never
uses zero after initialization.

- [ ] **Step 4: Run parser and safety tests and verify GREEN**

```powershell
cargo test --target x86_64-pc-windows-msvc --lib safety::tests
cargo test --target x86_64-pc-windows-msvc --lib frame_stream::tests
```

Expected: deadline, lease, cancellation, fragmented-frame, noise, and oversized-frame tests pass.

- [ ] **Step 5: Commit safety primitives**

```powershell
git add src/safety.rs src/frame_stream.rs src/firmware_config.rs src/lib.rs
git commit -m "feat: add controller lease and frame deadlines"
```

---

### Task 6: Refactor HID report execution around state and cancellation

**Files:**
- Modify: `src/hid_report.rs`
- Modify: `src/command_executor.rs`
- Modify: `src/usb_device.rs`
- Modify: `src/error.rs`

- [ ] **Step 1: Add failing pure execution-plan tests**

Keep hardware writers out of tests by asserting state-derived reports:

```rust
#[test]
fn tapping_d_does_not_release_held_w_and_shift() {
    let mut state = InputState::new();
    state.keyboard.key_down(KeyStroke { modifier: 0, keycode: 0x1A }).unwrap();
    state.keyboard.key_down(KeyStroke { modifier: 0x02, keycode: 0 }).unwrap();
    let plan = state.keyboard.tap_plan(KeyStroke { modifier: 0, keycode: 0x07 }).unwrap();
    assert_eq!(plan.restore().modifiers(), 0x02);
    assert_eq!(plan.restore().keycodes(), &[0x1A]);
}

#[test]
fn text_prevalidation_rejects_before_any_report() {
    let mut out = heapless::Vec::<KeyStroke, 240>::new();
    assert_eq!(ascii_strokes(b"ok\x01", false, &mut out), Err(InputError::UnsupportedAscii));
    assert!(out.is_empty());
}
```

- [ ] **Step 2: Run focused tests and verify RED for missing executor integration**

```powershell
cargo test --target x86_64-pc-windows-msvc --lib input_state::tests
cargo check
```

Expected: host state tests expose missing helpers or embedded compilation fails because the old executor still expects one-key `InputState`.

- [ ] **Step 3: Implement state-derived HID reports and cancellable actions**

Replace one-key helpers with:

```rust
pub async fn send_keyboard_state(
    writer: &mut KeyboardWriter,
    state: &KeyboardState,
) -> Result<(), ErrorCode>;

pub async fn send_mouse_state(
    writer: &mut MouseWriter,
    state: MouseState,
    x: i8,
    y: i8,
    wheel: i8,
) -> Result<(), ErrorCode>;
```

The keyboard report copies all six state keycodes. Executor functions apply a
transition, send the report, and retain the state only after a successful write.
Tap and click follow their pure pulse plans and restore snapshots. Text requires
an idle keyboard, validates the entire payload first, uses the atomic Caps Lock
bit, and restores idle after every character.

Introduce an embedded `CancelWait` adapter whose `delay(ms)` races
`Timer::after_millis(ms)` against an Embassy `Signal`. Check cancellation between
mouse movement steps and text characters. Any cancellation clears both logical
states and sends independent keyboard and mouse release reports.

- [ ] **Step 4: Run host tests and embedded check**

```powershell
cargo test --target x86_64-pc-windows-msvc --lib
cargo check
cargo clippy -- -D warnings
```

Expected: host tests pass; RP2350 target compiles without warnings.

- [ ] **Step 5: Commit stateful cancellable execution**

```powershell
git add src/hid_report.rs src/command_executor.rs src/usb_device.rs src/error.rs
git commit -m "feat: execute cancellable stateful HID reports"
```

---

### Task 7: Build the concurrent Embassy runtime and real emergency path

**Files:**
- Create: `src/runtime.rs`
- Modify: `src/main.rs`
- Modify: `src/static_resources.rs`
- Modify: `src/response_writer.rs`
- Modify: `Cargo.toml`

- [ ] **Step 1: Add failing coordinator scenario tests for runtime event order**

```rust
#[test]
fn stop_during_wait_cancels_wait_then_acknowledges_stop() {
    let mut model = RuntimeModel::new();
    assert_eq!(model.accept(request(2, 10, CommandType::WaitMs, &[0, 0, 0x13, 0x88])), ModelEvent::Start(10));
    assert_eq!(model.accept(request(2, 11, CommandType::StopAll, &[])), ModelEvent::CancelAndRelease { stop_sequence: 11 });
    let responses = model.complete_cancelled(10);
    assert_eq!(responses.as_slice(), &[nack(10, ErrorCode::Cancelled), ack(11)]);
}

#[test]
fn dtr_loss_clears_batch_cache_and_input_without_response() {
    let mut model = RuntimeModel::new();
    model.begin_batch(20).unwrap();
    model.note_input_held(true);
    assert_eq!(model.safety_event(SafetyEvent::DtrLost), ModelEvent::CancelReleaseAndResetSession);
    assert!(!model.has_batch());
    assert!(!model.has_cached_sequence(20));
}
```

- [ ] **Step 2: Run runtime model tests and verify RED**

```powershell
cargo test --target x86_64-pc-windows-msvc --lib coordinator::tests
```

Expected: `RuntimeModel`, `ModelEvent`, or safety transitions are missing.

- [ ] **Step 3: Implement runtime channels and tasks**

Add direct `embassy-sync = "0.8"` target dependency. Define static resources:

```rust
type RawMutex = embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

static REQUESTS: Channel<RawMutex, OwnedRequest, 8> = Channel::new();
static JOBS: Channel<RawMutex, ExecutionJob, 1> = Channel::new();
static RESULTS: Channel<RawMutex, ExecutionResult, 2> = Channel::new();
static RESPONSES: Channel<RawMutex, CachedResponse, 8> = Channel::new();
static SAFETY_EVENTS: Channel<RawMutex, SafetyEvent, 4> = Channel::new();
static CANCEL: Signal<RawMutex, u32> = Signal::new();
static LEASE_REFRESH: Signal<RawMutex, ()> = Signal::new();
```

Implement these task boundaries in `runtime.rs`:

```rust
pub async fn cdc_receive_task(receiver: CdcReceiver) -> !;
pub async fn dispatcher_task() -> !;
pub async fn executor_task(keyboard: KeyboardWriter, mouse: MouseWriter) -> !;
pub async fn response_task(sender: CdcSender) -> !;
pub async fn cdc_control_task(control: CdcControl) -> !;
pub async fn lease_task() -> !;
pub async fn keyboard_led_task(reader: KeyboardReader) -> !;
```

Complete the host-testable model used by the RED tests with:

```rust
pub struct RuntimeModel { coordinator: Coordinator, input_held: bool }

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModelEvent {
    Start(u16),
    Respond(CachedResponse),
    CancelAndRelease { stop_sequence: u16 },
    CancelReleaseAndResetSession,
}

impl RuntimeModel {
    pub fn new() -> Self;
    pub fn accept(&mut self, request: OwnedRequest) -> ModelEvent;
    pub fn complete_cancelled(&mut self, sequence: u16) -> heapless::Vec<CachedResponse, 2>;
    pub fn safety_event(&mut self, event: SafetyEvent) -> ModelEvent;
    pub fn begin_batch(&mut self, sequence: u16) -> Result<(), ErrorCode>;
    pub fn note_input_held(&mut self, held: bool);
    pub fn has_batch(&self) -> bool;
    pub fn has_cached_sequence(&self, sequence: u16) -> bool;
}
```

The receive task owns framing and the 250 ms partial-frame timer. The dispatcher
uses `select` across requests, execution results, and safety events, so it can
return BUSY/replay/query responses while the executor is active. `STOP_ALL`
signals cancellation immediately and is ACKed after the executor reports that
both release attempts ran. DTR loss and lease expiry use the same cancellation
path without requiring a response.

`main.rs` only initializes peripherals/classes, splits CDC and keyboard HID,
spawns the tasks, and runs `usb.run()`.

- [ ] **Step 4: Verify runtime tests and embedded compilation**

```powershell
cargo test --target x86_64-pc-windows-msvc --lib coordinator::tests
cargo test --target x86_64-pc-windows-msvc --lib safety::tests
cargo check
cargo clippy -- -D warnings
```

Expected: all tests pass and the task graph compiles for Cortex-M33.

- [ ] **Step 5: Commit the concurrent runtime**

```powershell
git add Cargo.toml Cargo.lock src/runtime.rs src/main.rs src/static_resources.rs src/response_writer.rs src/coordinator.rs
git commit -m "feat: add preemptible HID command runtime"
```

---

### Task 8: Correct USB identity, Report mode, and keyboard LED handling

**Files:**
- Modify: `build.rs`
- Modify: `src/usb_identity.rs`
- Modify: `src/usb_device.rs`
- Modify: `src/static_resources.rs`
- Modify: `src/main.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write failing identity and LED tests**

```rust
#[test]
fn formats_chip_id_as_stable_usb_serial() {
    let mut out = [0u8; USB_SERIAL_CAPACITY];
    let serial = format_usb_serial(0x0123_4567_89AB_CDEF, &mut out).unwrap();
    assert_eq!(serial, "EXQC-KMOUSE-0123456789ABCDEF");
}

#[test]
fn caps_lock_output_bit_is_detected() {
    assert!(!caps_lock_from_led_report(0b0000_0001));
    assert!(caps_lock_from_led_report(0b0000_0010));
}
```

- [ ] **Step 2: Run identity tests and verify RED**

```powershell
cargo test --target x86_64-pc-windows-msvc --lib usb_identity
```

Expected: formatter and LED helper do not exist.

- [ ] **Step 3: Implement configurable IDs and RP2350 OTP serial**

Extend `build.rs` to parse `RP2350_USB_VID` and `RP2350_USB_PID` as decimal or
`0x` hexadecimal, default to `0xCAFE`/`0x2350`, and write an `usb_ids.rs` file in
`OUT_DIR`:

```rust
pub const USB_VENDOR_ID: u16 = 0xCAFE;
pub const USB_PRODUCT_ID: u16 = 0x2350;
```

At startup call `embassy_rp::otp::get_chipid()`, format it into a StaticCell
buffer with `format_usb_serial`, and pass the resulting `&'static str` into
`usb_config(serial_number)`.

The pure helpers have these signatures:

```rust
pub const USB_SERIAL_CAPACITY: usize = 30;
pub fn format_usb_serial(chip_id: u64, out: &mut [u8; USB_SERIAL_CAPACITY]) -> Result<&str, UsbIdentityError>;
pub const fn caps_lock_from_led_report(leds: u8) -> bool { (leds & 0x02) != 0 }
```

Configure keyboard and mouse with:

```rust
hid_subclass: HidSubclass::No,
hid_boot_protocol: HidBootProtocol::None,
```

Create the keyboard as `HidReaderWriter::<_, 1, 8>`, split it, and update the
Caps Lock atomic from both interrupt OUT reports and a small `RequestHandler`
that accepts keyboard LED `SET_REPORT` control data. Keep the mouse writer-only.

- [ ] **Step 4: Run tests and embedded build**

```powershell
cargo test --target x86_64-pc-windows-msvc --lib usb_identity
cargo test --target x86_64-pc-windows-msvc --lib input_state::tests
cargo build --release
```

Expected: identity/LED tests pass and the release ELF links.

- [ ] **Step 5: Commit USB corrections**

```powershell
git add build.rs src/usb_identity.rs src/usb_device.rs src/static_resources.rs src/main.rs src/lib.rs
git commit -m "feat: use unique RP2350 USB identity"
```

---

### Task 9: Upgrade the Python SDK to v2 and modifier-only input

**Files:**
- Modify: `sdk/python/rp2350_hid_bridge/protocol.py`
- Modify: `sdk/python/rp2350_hid_bridge/keys.py`
- Modify: `sdk/python/rp2350_hid_bridge/client.py`
- Modify: `sdk/python/rp2350_hid_bridge/script.py`
- Modify: `sdk/python/tests/test_sdk.py`
- Modify: `sdk/python/README.md`

- [ ] **Step 1: Write failing Python tests**

```python
def test_modifier_only_combos(self):
    self.assertEqual(parse_combo("SHIFT"), (0x02, 0x00))
    self.assertEqual(parse_combo("CTRL+SHIFT"), (0x03, 0x00))

def test_v2_heartbeat_frame(self):
    frame = encode_frame(0, CommandType.HEARTBEAT, flags=FLAG_NO_RESPONSE)
    decoded = decode_frame(frame)
    self.assertEqual(decoded.version, 2)
    self.assertEqual(decoded.flags, FLAG_NO_RESPONSE)
    self.assertEqual(decoded.sequence, 0)

def test_nack_is_not_retried(self):
    serial = FakeSerial([response_frame(1, CommandType.NACK, b"\x0f")])
    bridge = bridge_with_fake_serial(serial)
    with self.assertRaises(RuntimeError):
        bridge.key_down("W")
    self.assertEqual(len(serial.writes), 1)
```

Add fake serial coverage for same-frame timeout retry, BUSY backoff, duration-aware
wait timeout, serialized heartbeat writes, DTR on/off, and close-time `STOP_ALL`.

- [ ] **Step 2: Run Python tests and verify RED**

```powershell
uv run --project sdk/python python -m unittest discover -s sdk/python/tests -v
```

Expected: modifier-only and v2/heartbeat tests fail.

- [ ] **Step 3: Implement Python v2 client behavior**

Set `PROTOCOL_VERSION = 2`, add flags to `encode_frame`, add `HEARTBEAT`, and
make `parse_combo` return keycode zero when at least one modifier was present and
no ordinary key was provided.

Refactor `send_command` so only timeout and BUSY retry the exact pre-encoded
frame. NACK exits immediately. Remove `reset_input_buffer` from retry attempts.
Use a write lock and a daemon heartbeat thread that writes sequence-zero
`NO_RESPONSE` heartbeats every 500 ms without reading a response. Compute the
deadline passed to `_read_response` from command/payload duration. Assert DTR on
open, send `STOP_ALL` best-effort on close, stop/join heartbeat, deassert DTR,
then close.

- [ ] **Step 4: Run Python tests and verify GREEN**

```powershell
uv run --project sdk/python python -m unittest discover -s sdk/python/tests -v
```

Expected: protocol, retry, heartbeat, modifier, timeout, and script tests pass.

- [ ] **Step 5: Commit Python SDK and update the firmware submodule pointer**

```powershell
git -C sdk/python add rp2350_hid_bridge tests README.md
git -C sdk/python commit -m "feat: support reliable HID protocol v2"
git add sdk/python
git commit -m "chore: update Python HID SDK"
```

---

### Task 10: Upgrade both C++ SDK worktrees and register CTest

**Files:**
- Modify nested paths under `sdk/cpp/include/rp2350_hid_bridge`
- Modify nested `sdk/cpp/tests/test_protocol.cpp`, `sdk/cpp/CMakeLists.txt`, `sdk/cpp/README.md`
- Mirror the same relative files under `../rp2350_hid_bridge_cpp`

- [ ] **Step 1: Write failing C++ v2, modifier, and retry tests**

Add assertions:

```cpp
auto shift = parse_combo("SHIFT");
assert(shift.modifier == 0x02 && shift.keycode == 0x00);
auto ctrl_shift = parse_combo("CTRL+SHIFT");
assert(ctrl_shift.modifier == 0x03 && ctrl_shift.keycode == 0x00);

auto heartbeat = encode_frame(0, CommandType::Heartbeat, {}, FLAG_NO_RESPONSE);
auto decoded_heartbeat = decode_frame(heartbeat);
assert(decoded_heartbeat.version == 2);
assert(decoded_heartbeat.flags == FLAG_NO_RESPONSE);
```

Add a fake transport seam in `serial.hpp` tests to assert NACK produces one
write, BUSY reuses the same bytes, and wait timeout equals requested duration
plus margin.

- [ ] **Step 2: Configure/build nested C++ and verify RED**

```powershell
cmake -S sdk/cpp -B sdk/cpp/build -G "Visual Studio 17 2022" -A x64
cmake --build sdk/cpp/build --config Release
ctest --test-dir sdk/cpp/build -C Release --output-on-failure
```

Expected: new symbols/tests fail before implementation; after `enable_testing()`
is added, CTest must discover `test_protocol` instead of reporting zero tests.

- [ ] **Step 3: Implement C++ v2 and heartbeat lifecycle**

Add protocol version/flags/heartbeat constants and an `encode_frame` flags
parameter. Permit modifier-only combinations. Remove `PurgeComm(PURGE_RXCLEAR)`
from retries. Add a write mutex, `std::atomic<bool>` heartbeat stop flag, and a
heartbeat thread that writes no-response frames. Use `EscapeCommFunction` for
`SETDTR`/`CLRDTR`; close with best-effort `stop_all`, thread join, then handle
close. Retry only timeout/BUSY and make NACK terminal.

Add to `CMakeLists.txt`:

```cmake
enable_testing()
add_test(NAME protocol COMMAND test_protocol)
```

Apply identical behavior and tests to both C++ worktrees.

- [ ] **Step 4: Build and test both C++ worktrees**

```powershell
cmake -S sdk/cpp -B sdk/cpp/build -G "Visual Studio 17 2022" -A x64
cmake --build sdk/cpp/build --config Release
ctest --test-dir sdk/cpp/build -C Release --output-on-failure
cmake -S ..\rp2350_hid_bridge_cpp -B ..\rp2350_hid_bridge_cpp\build -G "Visual Studio 17 2022" -A x64
cmake --build ..\rp2350_hid_bridge_cpp\build --config Release
ctest --test-dir ..\rp2350_hid_bridge_cpp\build -C Release --output-on-failure
```

Expected: both CTest runs discover and pass the protocol test.

- [ ] **Step 5: Commit both SDK worktrees and nested pointer**

```powershell
git -C sdk/cpp add include tests CMakeLists.txt README.md
git -C sdk/cpp commit -m "feat: support reliable HID protocol v2"
git -C ..\rp2350_hid_bridge_cpp add include tests CMakeLists.txt README.md
git -C ..\rp2350_hid_bridge_cpp commit -m "feat: support reliable HID protocol v2"
git add sdk/cpp
git commit -m "chore: update C++ HID SDK"
```

---

### Task 11: Upgrade `hidctl` and Web Serial clients

**Files:**
- Modify: `tools/hidctl/src/client.rs`
- Modify: `tools/hidctl/src/keys.rs`
- Modify: `tools/hidctl/src/main.rs`
- Modify: `tools/hidctl/src/script.rs`
- Modify: `tools/webui/protocol.js`
- Modify: `tools/webui/keys.js`
- Modify: `tools/webui/app.js`
- Modify: `tools/webui/script.js`
- Modify: `tools/webui/tests/protocol.test.mjs`

- [ ] **Step 1: Write failing Rust CLI and JavaScript tests**

Rust:

```rust
#[test]
fn parses_modifier_only_combos() {
    assert_eq!(parse_combo("SHIFT").unwrap(), KeyCombo { modifier: 0x02, keycode: 0 });
    assert_eq!(parse_combo("CTRL+SHIFT").unwrap(), KeyCombo { modifier: 0x03, keycode: 0 });
}
```

JavaScript:

```javascript
test("encodes a no-response v2 heartbeat", () => {
  const frame = encodeFrame(0, CommandType.Heartbeat, new Uint8Array(), FLAG_NO_RESPONSE);
  const decoded = decodeFrame(frame);
  assert.equal(decoded.version, 2);
  assert.equal(decoded.flags, FLAG_NO_RESPONSE);
});

test("parses modifier-only combos", () => {
  assert.deepEqual(parseCombo("SHIFT"), { modifier: 0x02, keycode: 0 });
  assert.deepEqual(parseCombo("CTRL+SHIFT"), { modifier: 0x03, keycode: 0 });
});
```

- [ ] **Step 2: Run both test suites and verify RED**

```powershell
cargo test --manifest-path tools/hidctl/Cargo.toml --target x86_64-pc-windows-msvc
node --test tools/webui/tests/protocol.test.mjs
```

Expected: modifier-only and v2 heartbeat tests fail.

- [ ] **Step 3: Implement v2 behavior in both clients**

For `hidctl`, add modifier-only parsing, remove receive-buffer clearing, keep the
same frame/sequence across timeout and BUSY, make NACK terminal, calculate
duration-aware timeouts, explicitly set DTR, and run a no-response heartbeat
thread using a cloned serial port plus a shared write mutex.

For Web Serial, add flags to `encodeFrame`, protocol v2/heartbeat constants, and
modifier-only parsing. On connect call:

```javascript
await this.port.setSignals({ dataTerminalReady: true });
this.heartbeatTimer = setInterval(() => this.writeHeartbeat(), 500);
```

`writeHeartbeat` writes a sequence-zero no-response frame through the existing
serialized command queue without creating a pending response. BUSY keeps the
pending request and retries the identical frame after the advertised delay.
Disconnect sends `STOP_ALL` best-effort, clears the timer, deasserts DTR, and
closes. Timeouts derive from wait/text/movement/batch duration.

- [ ] **Step 4: Run Rust and Web tests and verify GREEN**

```powershell
cargo test --manifest-path tools/hidctl/Cargo.toml --target x86_64-pc-windows-msvc
node --test tools/webui/tests/protocol.test.mjs
```

Expected: all existing and new tests pass.

- [ ] **Step 5: Commit Rust CLI and Web updates**

```powershell
git add tools/hidctl tools/webui
git commit -m "feat: update HID control clients for protocol v2"
```

---

### Task 12: Repair the picotool runner and documentation

**Files:**
- Create: `tools/flash.ps1`
- Modify: `.cargo/config.toml`
- Modify: `README.md`
- Modify: `../../docs/BUILD.md`
- Modify: `../../README.md`

- [ ] **Step 1: Write a non-flashing runner self-test mode**

Implement the script with a required artifact and optional `-ResolveOnly` test
switch:

```powershell
param(
    [Parameter(Position = 0, Mandatory = $true)] [string] $Artifact,
    [switch] $ResolveOnly
)

$picotool = if ($env:PICOTOOL_PATH) {
    $env:PICOTOOL_PATH
} else {
    (Get-Command picotool -ErrorAction Stop).Source
}

if (-not (Test-Path -LiteralPath $Artifact -PathType Leaf)) {
    throw "Firmware artifact not found: $Artifact"
}

if ($ResolveOnly) {
    Write-Output $picotool
    exit 0
}

& $picotool load -u -v -x -t elf $Artifact
exit $LASTEXITCODE
```

- [ ] **Step 2: Verify the old runner failure and script resolution without flashing**

Run only the safe script path:

```powershell
$env:PICOTOOL_PATH = (Get-Command where.exe).Source
powershell -NoProfile -ExecutionPolicy Bypass -File tools/flash.ps1 target/thumbv8m.main-none-eabihf/release/rp2350-keymouse-bridge-firmware -ResolveOnly
```

Expected: prints the resolved `where.exe` path and does not invoke `load`.

- [ ] **Step 3: Configure Cargo and update docs**

Set:

```toml
runner = "powershell -NoProfile -ExecutionPolicy Bypass -File tools/flash.ps1"
```

Document `RP2350_USB_VID`, `RP2350_USB_PID`, unique serial behavior, protocol v2,
modifier-only keys, six-key state, heartbeat/lease behavior, Batch semantics,
and the explicit rule that tests never flash or emit HID. Replace every literal
`${PICOTOOL_PATH}` runner claim in firmware and parent documentation.

- [ ] **Step 4: Run safe documentation/build checks**

```powershell
cargo build --release
powershell -NoProfile -ExecutionPolicy Bypass -File tools/flash.ps1 target/thumbv8m.main-none-eabihf/release/rp2350-keymouse-bridge-firmware -ResolveOnly
rg -n '\$\{PICOTOOL_PATH\}' .cargo README.md ..\..\docs\BUILD.md ..\..\README.md
```

Expected: build and resolver pass; `rg` finds no broken Cargo interpolation instructions. Do not run `cargo run`.

- [ ] **Step 5: Commit runner and documentation**

```powershell
git add .cargo/config.toml tools/flash.ps1 README.md
git commit -m "fix: use a working picotool runner"
git -C ..\.. add docs/BUILD.md README.md
git -C ..\.. commit -m "docs: update RP2350 v2 build guide"
```

---

### Task 13: Add CI and complete automated regression coverage

**Files:**
- Create: `.github/workflows/ci.yml`
- Modify: `src/protocol.rs`
- Modify: `src/frame_stream.rs`
- Modify: `src/input_state.rs`
- Modify: `src/batch.rs`
- Modify: `src/coordinator.rs`
- Modify: `src/safety.rs`
- Modify: `sdk/python/tests/test_sdk.py`
- Modify: `sdk/cpp/tests/test_protocol.cpp`
- Modify: `tools/hidctl/src/client.rs`
- Modify: `tools/hidctl/src/keys.rs`
- Modify: `tools/webui/tests/protocol.test.mjs`

- [ ] **Step 1: Add a local aggregate verification script to the workflow commands**

The workflow runs on `windows-latest`, checks out submodules recursively, installs
the embedded Rust target and `uv`, then executes exactly:

```yaml
- run: cargo fmt --all -- --check
- run: cargo test --target x86_64-pc-windows-msvc --lib
- run: cargo clippy --release -- -D warnings
- run: cargo build --release
- run: cargo test --manifest-path tools/hidctl/Cargo.toml --target x86_64-pc-windows-msvc
- run: node --test tools/webui/tests/protocol.test.mjs
- run: uv run --project sdk/python python -m unittest discover -s sdk/python/tests -v
- run: cmake -S sdk/cpp -B sdk/cpp/build -G "Visual Studio 17 2022" -A x64
- run: cmake --build sdk/cpp/build --config Release
- run: ctest --test-dir sdk/cpp/build -C Release --output-on-failure
```

No workflow step runs picotool or opens a serial port.

- [ ] **Step 2: Run the complete local matrix and record any failure**

Run the same commands locally. Expected: every command exits 0 before the CI
file is committed. A nonzero command blocks this task at that exact suite.

- [ ] **Step 3: Add the final named regression cases**

Ensure the final test list explicitly includes:

```text
W+D+SHIFT down, D-only up, SHIFT-only up
six-key full and seventh-key rejection
tap/click restoration from already-held state
unsupported ASCII with zero emitted steps
CRLF normalization and Caps Lock shift inversion
active duplicate BUSY, completed replay, sequence conflict
batch shadow validation, 32-command/8-KiB bounds, cancel and failure stop
DTR loss, heartbeat refresh, two-second expiry
STOP_ALL during wait, text, click, movement and batch
v1 flags rejection, v2 heartbeat flags, zero-sequence rules
250-ms partial-frame expiry and valid stream recovery
NACK no-retry and duration-aware timeouts in every client
```

- [ ] **Step 4: Re-run the complete matrix and verify GREEN**

Expected: every command exits 0, CTest discovers at least one test, and no suite
reports skipped failures.

- [ ] **Step 5: Commit CI and final automated tests**

```powershell
git add .github/workflows/ci.yml src tools sdk/cpp sdk/python
git commit -m "test: cover reliable HID control end to end"
```

---

### Task 14: Final integration audit and optional hardware handoff

**Files:**
- Review: `docs/superpowers/specs/2026-07-23-rp2350-reliable-hid-control-design.md`
- Review: `src/`, `tools/hidctl/`, `tools/webui/`, `sdk/python/`, `sdk/cpp/`
- Review: `../rp2350_hid_bridge_cpp/`
- Review: `.cargo/config.toml`, `tools/flash.ps1`, `.github/workflows/ci.yml`

- [ ] **Step 1: Audit protocol copies and forbidden legacy behavior**

```powershell
rg -n "PROTOCOL_VERSION\s*=\s*1|PurgeComm\(.*PURGE_RXCLEAR|reset_input_buffer\(\)|BatchBegin \| Command::BatchEnd|request_handler: None.*Boot|\$\{PICOTOOL_PATH\}" src tools sdk README.md .cargo
```

Expected: no stale default-v1 client, blind input-buffer clearing, no-op Batch,
unsupported Boot declaration, or broken runner interpolation remains. Explicit
legacy compatibility constants/tests are allowed and must be visibly named.

- [ ] **Step 2: Run final clean verification**

Run the complete Task 13 matrix again from a clean process and inspect every
exit code. Also run:

```powershell
git diff --check
git status --short
```

Expected: verification passes and only intentional submodule pointer/parent
integration changes remain.

- [ ] **Step 3: Synchronize parent submodule pointers**

After firmware and sibling C++ SDK commits exist:

```powershell
git -C ..\.. add tools/rp2350_keymouse_bridge_firmware tools/rp2350_hid_bridge_cpp
git -C ..\.. commit -m "feat: integrate reliable RP2350 HID control"
```

- [ ] **Step 4: Stop before any live-board action and request approval**

Report the built ELF path and the exact proposed picotool command. Do not flash,
open the COM port, or emit keyboard/mouse reports until the user explicitly
approves that separate live test.

- [ ] **Step 5: If live testing is approved, execute the acceptance checklist**

Use a harmless text editor or key-event viewer, never the active game or shell,
and verify in order:

```text
unique CDC/HID enumeration
W+D+SHIFT exact hold/release
Python SHIFT-only down/up
same-sequence click and move deduplication
STOP_ALL during WAIT and Batch
release on COM close and heartbeat expiry
Caps Lock-aware US ASCII
```

Record observed results separately from compile/test claims. Any failed physical
behavior returns to a new failing regression test before code changes.
