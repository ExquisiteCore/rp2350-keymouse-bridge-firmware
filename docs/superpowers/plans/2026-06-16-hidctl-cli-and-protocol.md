# HID Control CLI And Protocol Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a host CLI, key-level command support, stronger protocol reliability, and script-file execution for the RP2350 HID bridge.

**Architecture:** Keep the firmware command surface small and deterministic: add `GET_CAPS`, `BATCH_BEGIN`, and `BATCH_END` to the existing framed protocol, then leave retry and script orchestration on the PC. Add an independent Rust host tool under `tools/hidctl` that depends on the root protocol library, auto-detects the board by VID/PID, sends framed commands with ACK/NACK validation and retries, and parses a simple script format into the same binary commands.

**Tech Stack:** Rust 2024, root `no_std` protocol library, `serialport 4.9`, `clap 4.6`, host unit tests, RP2350 Embassy firmware.

---

### Task 1: Firmware Protocol Additions

**Files:**
- Modify: `src/protocol.rs`
- Modify: `src/commands.rs`
- Modify: `src/main.rs`

- [ ] Add tests for `GET_CAPS`, `BATCH_BEGIN`, and `BATCH_END` command decoding.
- [ ] Add `CommandType` values: `GetCaps = 0x03`, `BatchBegin = 0x40`, `BatchEnd = 0x41`, and `Busy = 0x83`.
- [ ] Add `Command` variants and decode empty payloads.
- [ ] Make firmware ACK batch begin/end and return a capability payload from `GET_CAPS`.
- [ ] Run `cargo test --target x86_64-pc-windows-msvc --lib` and `cargo check`.

### Task 2: Host CLI Package

**Files:**
- Create: `tools/hidctl/Cargo.toml`
- Create: `tools/hidctl/src/main.rs`
- Create: `tools/hidctl/src/client.rs`
- Create: `tools/hidctl/src/keys.rs`
- Create: `tools/hidctl/src/script.rs`

- [ ] Add host-only Rust package with a local config forcing `x86_64-pc-windows-msvc`.
- [ ] Implement board auto-detection by USB VID/PID and optional `--port`.
- [ ] Implement reliable `send_command`: write frame, read response, validate sequence/type, retry on timeout, stop on NACK.
- [ ] Implement CLI subcommands: `list`, `ping`, `info`, `caps`, `type`, `key`, `mouse`, `wait`, `stop`, `run`.

### Task 3: Key-Level And Script Parsing

**Files:**
- Modify: `tools/hidctl/src/keys.rs`
- Modify: `tools/hidctl/src/script.rs`

- [ ] Add tests for key combos such as `CTRL+C`, `SHIFT+R`, `ENTER`, and `F5`.
- [ ] Add tests for script lines: `type "abc"`, `key tap ENTER`, `mouse move 10 -5`, `wait 100`, `stop`.
- [ ] Implement parsing to binary command payloads.
- [ ] Make `run` wrap script execution in `BATCH_BEGIN` / `BATCH_END`, stop on first failure, and send `STOP_ALL` on failure.

### Task 4: Verification

**Files:**
- All changed files

- [ ] Run root tests and firmware build: `cargo fmt -- --check`, `cargo test --target x86_64-pc-windows-msvc --lib`, `cargo clippy -- -D warnings`, `cargo build --release`.
- [ ] Run host tool tests/build: `cargo test --manifest-path tools/hidctl/Cargo.toml --target x86_64-pc-windows-msvc`, `cargo build --manifest-path tools/hidctl/Cargo.toml --target x86_64-pc-windows-msvc`.
- [ ] Run live protocol checks against COM3: `hidctl ping`, `hidctl caps`, `hidctl key tap ENTER`, and a safe script file if the board is connected.
