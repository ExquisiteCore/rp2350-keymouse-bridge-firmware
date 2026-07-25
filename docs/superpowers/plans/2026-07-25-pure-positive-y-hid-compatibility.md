# RP2350 Pure Positive Y HID Compatibility Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a pure positive-Y relative mouse command produce the requested vertical displacement on the validated Windows/RP2350 hardware path without changing the wire protocol or leaving horizontal drift.

**Architecture:** Keep decoding and signed movement chunking unchanged. Add one private execution-layer sender that maps only a pure positive-Y HID chunk to a proven-compatible diagonal report followed immediately by its horizontal compensation; all other report shapes remain byte-for-byte unchanged. Exercise the real `execute_command()` path with a fake HID backend before building a new RP2350 ARM-S UF2.

**Tech Stack:** Rust 2024, `embassy-usb` 0.6, `usbd-hid` 0.10, host-side Rust tests on `x86_64-pc-windows-msvc`, RP2350 `thumbv8m.main-none-eabihf`, PowerShell UF2 verification.

---

## File map

- Modify `src/lib.rs`: add execution-path regression tests for pure positive Y, chunking, unchanged report shapes, and cancellation boundaries.
- Modify `src/execution_core.rs`: add the private balanced compatibility sender and route relative movement chunks through it.
- Modify `README.md`: expand manual hardware acceptance to test all four pure-axis directions and the mixed `(1, 80)` diagnostic.
- Generate, do not commit, `dist/rp2350-keymouse-bridge-firmware.uf2` and a versioned copy in `C:\Users\xiaol\Downloads`.

### Task 1: Reproduce and fix pure positive-Y HID execution

**Files:**
- Modify: `src/lib.rs` inside `execution_core_tests`
- Modify: `src/execution_core.rs` around `Command::MouseMoveRel`

- [ ] **Step 1: Add failing execution-path regression tests**

Add these tests after `run_ready()` in `src/lib.rs`:

```rust
    fn mouse_report(x: i8, y: i8) -> Report {
        mouse_report_with_state(MouseState::new(), x, y)
    }

    fn mouse_report_with_state(state: MouseState, x: i8, y: i8) -> Report {
        Report::Mouse {
            state,
            x,
            y,
            wheel: 0,
        }
    }

    #[test]
    fn pure_positive_y_uses_balanced_horizontal_compatibility_reports() {
        let mut backend = FakeExecutionBackend::cancelling_after_report(usize::MAX);
        let mut state = InputState::new();

        let result = run_ready(execute_command(
            Command::MouseMoveRel { dx: 0, dy: 80 },
            2,
            false,
            &mut backend,
            &mut state,
        ));

        assert_eq!(result, Ok(DeviceResponse::Ack));
        assert_eq!(backend.reports, [mouse_report(1, 80), mouse_report(-1, 0)]);
    }

    #[test]
    fn pure_positive_y_preserves_held_mouse_buttons_in_both_reports() {
        let mut backend = FakeExecutionBackend::cancelling_after_report(usize::MAX);
        let mut state = InputState::new();
        state.mouse.button_down(MouseButton::Right);
        let held = state.mouse;

        let result = run_ready(execute_command(
            Command::MouseMoveRel { dx: 0, dy: 80 },
            2,
            false,
            &mut backend,
            &mut state,
        ));

        assert_eq!(result, Ok(DeviceResponse::Ack));
        assert_eq!(
            backend.reports,
            [
                mouse_report_with_state(held, 1, 80),
                mouse_report_with_state(held, -1, 0),
            ]
        );
        assert_eq!(state.mouse, held);
    }

    #[test]
    fn pure_positive_y_chunks_keep_zero_net_x_and_full_y() {
        let mut backend = FakeExecutionBackend::cancelling_after_report(usize::MAX);
        let mut state = InputState::new();

        let result = run_ready(execute_command(
            Command::MouseMoveRel { dx: 0, dy: 300 },
            2,
            false,
            &mut backend,
            &mut state,
        ));

        assert_eq!(result, Ok(DeviceResponse::Ack));
        assert_eq!(
            backend.reports,
            [
                mouse_report(1, 127),
                mouse_report(-1, 0),
                mouse_report(1, 127),
                mouse_report(-1, 0),
                mouse_report(1, 46),
                mouse_report(-1, 0),
            ]
        );
    }

    #[test]
    fn negative_y_and_mixed_xy_keep_single_reports() {
        for (command, expected) in [
            (Command::MouseMoveRel { dx: 0, dy: -80 }, mouse_report(0, -80)),
            (Command::MouseMoveRel { dx: 1, dy: 80 }, mouse_report(1, 80)),
        ] {
            let mut backend = FakeExecutionBackend::cancelling_after_report(usize::MAX);
            let mut state = InputState::new();

            let result = run_ready(execute_command(
                command,
                2,
                false,
                &mut backend,
                &mut state,
            ));

            assert_eq!(result, Ok(DeviceResponse::Ack));
            assert_eq!(backend.reports, [expected]);
        }
    }

    #[test]
    fn pure_positive_y_finishes_compensation_before_observing_cancellation() {
        let mut backend = FakeExecutionBackend::cancelling_after_report(1);
        let mut state = InputState::new();

        let result = run_ready(execute_command(
            Command::MouseMoveRel { dx: 0, dy: 80 },
            2,
            false,
            &mut backend,
            &mut state,
        ));

        assert_eq!(result, Err(ErrorCode::Cancelled));
        assert_eq!(
            backend.reports,
            [
                mouse_report(1, 80),
                mouse_report(-1, 0),
                Report::Keyboard(KeyboardState::new()),
                mouse_report(0, 0),
            ]
        );
        assert!(state.is_idle());
    }
```

- [ ] **Step 2: Run the focused tests and verify RED**

Run:

```powershell
cargo test --target x86_64-pc-windows-msvc --lib pure_positive_y -- --nocapture
```

Expected: the new tests fail because the existing executor emits `(0, positive_y)` directly and observes cancellation after the first report instead of completing a compensation pair.

- [ ] **Step 3: Add the minimal balanced compatibility sender**

Add this private helper above `execute_command()` in `src/execution_core.rs`:

```rust
async fn send_relative_mouse_step<B: ExecutionBackend>(
    backend: &mut B,
    state: MouseState,
    x: i8,
    y: i8,
) -> Result<(), ErrorCode> {
    if x == 0 && y > 0 {
        backend.send_mouse(state, 1, y, 0).await?;
        backend.send_mouse(state, -1, 0, 0).await
    } else {
        backend.send_mouse(state, x, y, 0).await
    }
}
```

Replace only the relative-movement HID write inside `Command::MouseMoveRel`:

```rust
        Command::MouseMoveRel { dx, dy } => {
            for (x, y) in RelativeMovementSteps::new(dx, dy) {
                backend.check_cancelled()?;
                send_relative_mouse_step(backend, state.mouse, x, y).await?;
                backend.check_cancelled()?;
            }
            DeviceResponse::Ack
        }
```

- [ ] **Step 4: Run the focused tests and verify GREEN**

Run:

```powershell
cargo test --target x86_64-pc-windows-msvc --lib pure_positive_y -- --nocapture
```

Expected: all tests whose names contain `pure_positive_y` pass.

- [ ] **Step 5: Run the complete firmware library suite**

Run:

```powershell
cargo test --target x86_64-pc-windows-msvc --lib
```

Expected: all library tests pass with zero failures.

- [ ] **Step 6: Commit the tested firmware change**

```powershell
git add src/lib.rs src/execution_core.rs
git commit -m "fix: preserve pure positive Y mouse movement"
```

### Task 2: Make four-direction hardware acceptance mandatory

**Files:**
- Modify: `README.md` under `### 手动硬件验收`

- [ ] **Step 1: Expand the manual mouse test commands**

Replace the two X-only movement commands with:

```powershell
.\tools\hidctl\target\x86_64-pc-windows-msvc\release\hidctl.exe --port COM3 mouse move 20 0
.\tools\hidctl\target\x86_64-pc-windows-msvc\release\hidctl.exe --port COM3 mouse move -20 0
.\tools\hidctl\target\x86_64-pc-windows-msvc\release\hidctl.exe --port COM3 mouse move 0 20
.\tools\hidctl\target\x86_64-pc-windows-msvc\release\hidctl.exe --port COM3 mouse move 0 -20
.\tools\hidctl\target\x86_64-pc-windows-msvc\release\hidctl.exe --port COM3 mouse move 1 20
.\tools\hidctl\target\x86_64-pc-windows-msvc\release\hidctl.exe --port COM3 stop
```

Immediately below the block, add:

```markdown
四个纯轴方向必须都产生幅度近似对称的光标位移。`mouse move 0 20` 若不移动、但
`mouse move 1 20` 可以移动，说明纯正向 Y HID 报告路径仍未通过验收。
```

- [ ] **Step 2: Check formatting and the exact acceptance text**

Run:

```powershell
git diff --check
rg -n "mouse move 0 20|mouse move 0 -20|mouse move 1 20|纯正向 Y" README.md
```

Expected: `git diff --check` exits zero and `rg` prints all four new acceptance references.

- [ ] **Step 3: Commit the hardware acceptance documentation**

```powershell
git add README.md
git commit -m "docs: require four-direction HID acceptance"
```

### Task 3: Verify and build the replacement UF2

**Files:**
- Generate: `dist/rp2350-keymouse-bridge-firmware.uf2`
- Generate: `C:\Users\xiaol\Downloads\rp2350-keymouse-bridge-firmware-pure-y-fix.uf2`

- [ ] **Step 1: Verify formatting and host behavior**

Run:

```powershell
cargo fmt --all -- --check
cargo test --target x86_64-pc-windows-msvc --lib
```

Expected: formatting check exits zero and all firmware library tests pass.

- [ ] **Step 2: Verify the embedded build with Clippy**

Run:

```powershell
cargo clippy --release -- -D warnings
cargo build --release --locked
```

Expected: both commands exit zero for `thumbv8m.main-none-eabihf` with no warnings promoted to errors.

- [ ] **Step 3: Build and structurally validate the RP2350 ARM-S UF2**

Run:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools/tests/build-release-uf2.ps1
```

Expected: output reports RP2350 ARM-S family `0xE48BFF59`, prints a SHA-256 digest, and ends with a `PASS` line.

- [ ] **Step 4: Copy the verified UF2 to the user-facing delivery path**

Run:

```powershell
$source = (Resolve-Path '.\dist\rp2350-keymouse-bridge-firmware.uf2').Path
$destination = 'C:\Users\xiaol\Downloads\rp2350-keymouse-bridge-firmware-pure-y-fix.uf2'
Copy-Item -LiteralPath $source -Destination $destination -Force
$item = Get-Item -LiteralPath $destination
$hash = Get-FileHash -LiteralPath $destination -Algorithm SHA256
"PATH=$($item.FullName)"
"SIZE=$($item.Length)"
"SHA256=$($hash.Hash)"
```

Expected: the delivery file exists, is a nonzero multiple of 512 bytes, and its SHA-256 equals the digest printed by the UF2 build test.

- [ ] **Step 5: Confirm repository scope before branch completion**

Run:

```powershell
git status --short --branch
git log -3 --oneline --decorate
```

Expected: the firmware feature branch is clean; only committed firmware and documentation changes exist. The generated `dist` and Downloads artifacts remain untracked/ignored and outside the commit.

### Task 4: Production-machine acceptance sequence

**Files:**
- Flash only: `rp2350-keymouse-bridge-firmware-pure-y-fix.uf2`
- Reuse unchanged: existing `cs2-vision-runtime-sm61` package

- [ ] **Step 1: Flash the delivered UF2 in BOOTSEL mode**

Copy the verified UF2 to the RP2350 BOOTSEL drive, wait for automatic reboot, and confirm the CDC endpoint reappears as a COM port. Do not reinstall ORT/CUDA/TensorRT and do not replace the vision runtime package.

- [ ] **Step 2: Repeat the independent cursor tests before calibration**

From the existing runtime folder, with the pointer near the desktop center:

```powershell
Set-ExecutionPolicy -Scope Process -ExecutionPolicy Bypass -Force
. .\scripts\common.ps1
Invoke-WithRuntimeEnvironment { & .\app\vision_analyzer.exe --hid-port COM4 --test-hid-move 80 0 }
Invoke-WithRuntimeEnvironment { & .\app\vision_analyzer.exe --hid-port COM4 --test-hid-move -80 0 }
Invoke-WithRuntimeEnvironment { & .\app\vision_analyzer.exe --hid-port COM4 --test-hid-move 0 80 }
Invoke-WithRuntimeEnvironment { & .\app\vision_analyzer.exe --hid-port COM4 --test-hid-move 0 -80 }
Invoke-WithRuntimeEnvironment { & .\app\vision_analyzer.exe --hid-port COM4 --test-hid-move 1 80 }
```

Expected: the first four pure-axis deltas have approximately symmetric magnitudes; specifically `(0,80)` has a large positive Y delta instead of zero. The mixed command also has a large positive Y delta.

- [ ] **Step 3: Resume full-screen calibration only after cursor acceptance passes**

Run:

```powershell
Start-Sleep -Seconds 5; python .\examples\runtime_live_move.py --hid-port COM4 --player-side ct --enable-live-output --show-every 1
```

Switch to CS2 during the five-second delay. Expected: calibration reports a valid X/Y profile instead of failing because one pure Y direction was absent. Hardware acceptance remains the final proof; host-side tests alone cannot claim the board is fixed.
