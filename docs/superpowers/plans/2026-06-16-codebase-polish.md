# Codebase Polish Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 整理 RP2350 固件和 Python/C++ SDK，让结构更清晰、测试边界更明确、SDK 文档更详细。

**Architecture:** 固件保持现有 USB 协议和 VID/PID 不变，把 `main.rs` 中的配置、帧流处理、响应写入、HID report 执行拆到小模块。C++ SDK 保持 `#include "rp2350_hid_bridge.hpp"` 兼容，同时拆成协议、键名、脚本、串口多头文件。Python SDK 保持现有包结构，补详细 README、端口列表示例和脚本示例。

**Tech Stack:** Rust 2024/no_std/Embassy, Python 3.10+ with pyserial, C++17 with Win32 serial API, CMake, unit tests.

---

### Task 1: Firmware Structure

**Files:**
- Create: `src/firmware_config.rs`
- Create: `src/error.rs`
- Create: `src/frame_stream.rs`
- Create: `src/hid_report.rs`
- Create: `src/command_executor.rs`
- Create: `src/usb_device.rs`
- Create: `src/static_resources.rs`
- Create: `src/response_writer.rs`
- Modify: `src/main.rs`
- Modify: `src/lib.rs`

- [x] Add host-side tests for USB identity, capability payload, and frame stream behavior.
- [x] Move protocol constants and capability payload out of `main.rs`.
- [x] Move NACK error code mapping out of `main.rs`.
- [x] Move stream frame detection and buffer shifting out of `main.rs`.
- [x] Move HID keyboard/mouse report helpers out of `main.rs`.
- [x] Move command execution out of `main.rs`.
- [x] Move USB config and static resource helpers out of `main.rs`.
- [x] Keep `main.rs` focused on wiring Embassy tasks and the CDC control loop.

### Task 2: C++ SDK Structure

**Files:**
- Create: `sdk/cpp/include/rp2350_hid_bridge/protocol.hpp`
- Create: `sdk/cpp/include/rp2350_hid_bridge/keys.hpp`
- Create: `sdk/cpp/include/rp2350_hid_bridge/script.hpp`
- Create: `sdk/cpp/include/rp2350_hid_bridge/serial.hpp`
- Modify: `sdk/cpp/include/rp2350_hid_bridge.hpp`
- Modify: `sdk/cpp/tests/test_protocol.cpp`

- [x] Preserve the existing top-level include path.
- [x] Split protocol, key parsing, script parsing, and Windows serial client into focused headers.
- [x] Add concise API comments for public SDK types and functions.
- [x] Keep C++ protocol tests passing without changing user-facing behavior.

### Task 3: SDK Documentation And Examples

**Files:**
- Modify: `sdk/python/README.md`
- Modify: `sdk/cpp/README.md`
- Create: `sdk/python/examples/list_ports.py`
- Create: `sdk/python/examples/script_demo.py`
- Create: `sdk/cpp/examples/script_demo.cpp`

- [x] Document Python installation, port auto-detection, direct control methods, script syntax, and errors.
- [x] Document C++ CMake integration, header layout, direct control methods, and script syntax.
- [x] Add examples that avoid accidental real input unless the user intentionally runs them.

### Task 4: Verification

**Files:**
- All modified files

- [x] Run `cargo test --target x86_64-pc-windows-msvc --lib`.
- [x] Run `cargo build --target thumbv8m.main-none-eabihf`.
- [x] Run `$env:PYTHONPATH='sdk\python'; python -m unittest discover -s sdk\python\tests`.
- [x] Run `cmake --build sdk/cpp/build --config Debug`.
- [x] Run `sdk\cpp\build\Debug\test_protocol.exe`.
- [x] Run `node --test tools\webui\tests\protocol.test.mjs`.
