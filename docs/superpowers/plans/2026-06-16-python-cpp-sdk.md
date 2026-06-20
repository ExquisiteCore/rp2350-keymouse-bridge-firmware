# Python And C++ SDK Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Python and C++ SDKs for controlling the RP2350 HID bridge through the existing framed CDC protocol.

**Architecture:** Keep protocol logic duplicated in small, dependency-light SDK modules so each language can be used independently. Python provides optional auto-detection through `pyserial`; C++ provides a header-only Windows serial client with explicit port selection.

**Tech Stack:** Python 3 standard library + optional `pyserial`, C++17 + Win32 serial API, unit tests for pure protocol/key/script behavior.

---

### Task 1: Python SDK

**Files:**
- Create: `sdk/python/rp2350_hid_bridge/protocol.py`
- Create: `sdk/python/rp2350_hid_bridge/keys.py`
- Create: `sdk/python/rp2350_hid_bridge/script.py`
- Create: `sdk/python/rp2350_hid_bridge/client.py`
- Create: `sdk/python/rp2350_hid_bridge/__init__.py`
- Create: `sdk/python/tests/test_sdk.py`
- Create: `sdk/python/examples/basic.py`
- Create: `sdk/python/README.md`

- [x] Add protocol, combo, and script unit tests first.
- [x] Implement Python protocol helpers, key parser, script parser, and high-level client methods.
- [x] Run `python -m unittest discover -s sdk/python/tests`.

### Task 2: C++ SDK

**Files:**
- Create: `sdk/cpp/include/rp2350_hid_bridge.hpp`
- Create: `sdk/cpp/tests/test_protocol.cpp`
- Create: `sdk/cpp/examples/basic.cpp`
- Create: `sdk/cpp/CMakeLists.txt`
- Create: `sdk/cpp/README.md`

- [x] Add C++ protocol/key/script assertions.
- [x] Implement header-only protocol helpers, key parser, script parser, and Windows serial client.
- [x] Build and run the C++ protocol test on Windows.

### Task 3: Verification

**Files:**
- All SDK files

- [x] Run Python unit tests.
- [x] Compile C++ protocol test.
- [x] Run C++ protocol test.
- [x] Run existing Rust and WebUI protocol tests if time allows.
