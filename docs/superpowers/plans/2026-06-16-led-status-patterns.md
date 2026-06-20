# LED Status Patterns Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the simple board LED toggle with a clearer single-color status indicator: disconnected breathing, connected heartbeat, command activity flash, and error triple blink.

**Architecture:** Add a small `no_std` LED animator module with deterministic state-machine tests. The firmware owns PIN_25 in a dedicated Embassy task and receives mode/event updates through atomics from the USB command loop.

**Tech Stack:** Rust 2024, Embassy async task, RP2350 GPIO output, `core::sync::atomic`, root host-side unit tests.

---

### Task 1: LED Animator State Machine

**Files:**
- Create: `src/led.rs`
- Modify: `src/lib.rs`

- [ ] Add failing tests for disconnected breathing, connected heartbeat, activity flash, and error triple blink.
- [ ] Implement `LedAnimator`, `LedMode`, `LedSignal`, `LED_TICK_MS`, and brightness/output helpers.
- [ ] Export the `led` module from `src/lib.rs`.
- [ ] Run `cargo test --target x86_64-pc-windows-msvc --lib`.

### Task 2: Firmware LED Task Integration

**Files:**
- Modify: `src/main.rs`

- [ ] Spawn an Embassy LED task that owns `PIN_25`.
- [ ] Replace direct LED `set_high/set_low/toggle` calls with atomic mode/event updates.
- [ ] Publish connected/disconnected mode changes from the CDC loop.
- [ ] Publish activity on successful command handling and error on rejected/NACK command paths.
- [ ] Run `cargo clippy -- -D warnings` and `cargo build --release`.

### Task 3: Verification

**Files:**
- All changed files

- [ ] Run `cargo fmt -- --check`.
- [ ] Run `cargo test --target x86_64-pc-windows-msvc --lib`.
- [ ] Run `cargo clippy -- -D warnings`.
- [ ] Run `cargo build --release`.
- [ ] Report the new firmware path for flashing.
