# Web Serial HID UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a static browser UI that connects to the RP2350 CDC serial interface and exercises the existing keyboard, mouse, protocol, and script commands.

**Architecture:** Keep the WebUI self-contained under `tools/webui`. Split pure protocol helpers from DOM code so CRC, frame parsing, key parsing, and script parsing can be tested with Node's built-in test runner.

**Tech Stack:** Static HTML/CSS/ES modules, Web Serial API, Node `--test` for pure JavaScript tests.

---

### Task 1: Protocol And Parser Modules

**Files:**
- Create: `tools/webui/protocol.js`
- Create: `tools/webui/keys.js`
- Create: `tools/webui/script.js`
- Create: `tools/webui/tests/protocol.test.mjs`

- [ ] Add tests for frame round-trip, CRC rejection, key combo parsing, and script parsing.
- [ ] Implement command constants, frame encoding/decoding, CRC16, key combo parsing, mouse button parsing, and script parsing.
- [ ] Run `node --test tools/webui/tests/protocol.test.mjs`.

### Task 2: Web Serial Client And UI

**Files:**
- Create: `tools/webui/index.html`
- Create: `tools/webui/styles.css`
- Create: `tools/webui/app.js`

- [ ] Implement a compact control surface for connection, protocol checks, keyboard, mouse, script, and logs.
- [ ] Implement Web Serial open/close, read loop, sequence validation, NACK handling, and timeout handling.
- [ ] Wire UI actions to encoded protocol commands.

### Task 3: Verification

**Files:**
- All `tools/webui/*` files

- [ ] Run Node tests.
- [ ] Start a localhost static server.
- [ ] Inspect the UI in a browser at desktop and mobile widths.
- [ ] Run a safe live `ping/caps/stop` if the board is connected.
