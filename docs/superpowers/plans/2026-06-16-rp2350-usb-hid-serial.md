# RP2350 USB HID Serial Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build RP2350 firmware that enumerates as a USB keyboard, mouse, and CDC serial command endpoint.

**Architecture:** Keep command framing and validation in a host-testable `protocol` library module. Run Embassy USB as a composite device with one CDC ACM class, one keyboard HID interface, and one mouse HID interface. Convert validated serial command frames into standard HID reports and return ACK/NACK frames over CDC.

**Tech Stack:** Rust `no_std`, `embassy-rp 0.10`, `embassy-usb 0.6`, `embassy-executor 0.10`, `usbd-hid 0.10`, `defmt`, `panic-probe`.

---

### Task 1: Protocol Library

**Files:**
- Create: `src/lib.rs`
- Create: `src/protocol.rs`

- [ ] **Step 1: Write host unit tests for frame parsing**

Add tests for valid PING, CRC rejection, short frame rejection, and payload length validation.

- [ ] **Step 2: Run tests and verify they fail before implementation**

Run: `cargo test --target x86_64-pc-windows-msvc --lib`

- [ ] **Step 3: Implement frame encoder/decoder**

Add constants, `CommandType`, `Frame`, `DecodeError`, CRC16-CCITT, `encode_frame`, and `decode_frame`.

- [ ] **Step 4: Run tests and verify they pass**

Run: `cargo test --target x86_64-pc-windows-msvc --lib`

### Task 2: Command Decoding and HID Mapping

**Files:**
- Create: `src/commands.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write host unit tests for command payloads**

Cover ASCII typing, keyboard tap payload, mouse move payload, mouse click payload, wait command, and stop-all command.

- [ ] **Step 2: Run command tests and verify they fail before implementation**

Run: `cargo test --target x86_64-pc-windows-msvc --lib`

- [ ] **Step 3: Implement command decoder and key mapping helpers**

Expose `Command`, `KeyStroke`, `MouseButton`, `AsciiKey`, and `decode_command`.

- [ ] **Step 4: Run tests and verify they pass**

Run: `cargo test --target x86_64-pc-windows-msvc --lib`

### Task 3: Embassy USB Firmware

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/main.rs`
- Modify: `build.rs` only if linker setup needs no change

- [ ] **Step 1: Replace HAL dependencies with Embassy dependencies**

Use Embassy RP2350 features `rp235xb`, `rt`, `time-driver`, `critical-section-impl`, and `defmt`.

- [ ] **Step 2: Build USB composite device**

Create one CDC ACM class, one keyboard HID class, and one mouse HID class with static descriptor buffers.

- [ ] **Step 3: Implement CDC command loop**

Read CDC packets into a frame buffer, decode complete frames, execute commands, and send ACK/NACK response frames.

- [ ] **Step 4: Implement HID output helpers**

Send keyboard reports, mouse reports, split large mouse movement into multiple `i8` steps, and release all buttons/keys on STOP_ALL.

- [ ] **Step 5: Verify embedded build**

Run: `cargo fmt -- --check`, `cargo test --target x86_64-pc-windows-msvc --lib`, `cargo check`, `cargo clippy -- -D warnings`, and `cargo build --release`.
