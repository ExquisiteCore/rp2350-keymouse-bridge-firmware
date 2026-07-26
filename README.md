# RP2350 KeyMouse 桥接器固件

面向 ExquisiteCore RP2350 KeyMouse Bridge 的 Rust 固件。

板卡会枚举为 USB 复合设备：

```text
CDC 串口端点             接收带帧格式的控制命令
USB HID 键盘             生成标准键盘报告
USB HID 鼠标             生成标准相对鼠标报告
```

主机应用可以通过 `tools/hidctl`、C++ SDK 或 Python SDK，使用串口协议控制板卡。

## 仓库结构

```text
src/                  RP2350 固件源码
tools/hidctl/         用于协议检查的 Windows 主机 CLI
tools/webui/          Web Serial 协议/调试界面
tools/flash.ps1       显式调用 picotool 的脚本，支持安全的仅解析模式
sdk/cpp/              C++17 共享库 SDK 子模块
sdk/python/           Python SDK 子模块
.cargo/config.toml    RP2350 目标和 picotool runner 配置
rp2350.x              链接脚本
```

嵌套 SDK 仓库：

```text
sdk/cpp    -> https://github.com/ExquisiteCore/rp2350-hid-bridge-cpp
sdk/python -> https://github.com/ExquisiteCore/rp2350-hid-bridge-python
```

## 环境要求

```text
支持 edition 2024 的 Rust stable
rustup 目标 thumbv8m.main-none-eabihf
用于生成 UF2 的 elf2uf2-rs
用于 USB 刷写的 picotool
用于 Windows 主机工具的 Visual Studio 2022 Build Tools
```

安装嵌入式目标和 UF2 工具：

```powershell
rustup target add thumbv8m.main-none-eabihf
cargo install elf2uf2-rs --locked
```

## 构建固件

在固件仓库根目录执行：

```powershell
cargo build --release
```

`cargo build --release` 只编译和链接固件，不会运行 picotool、刷写板卡、打开串口或
产生 HID 输入。

输出：

```text
target\thumbv8m.main-none-eabihf\release\rp2350-keymouse-bridge-firmware
```

根软件包是固件二进制程序。仓库有意提交 `Cargo.lock`，以保证固件构建可复现。

### 构建 BOOTSEL UF2

运行 Release 包装脚本，一步完成固件编译和 Pico 2 UF2 打包：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools/build-release.ps1
```

输出：

```text
target\thumbv8m.main-none-eabihf\release\rp2350-keymouse-bridge-firmware
dist\rp2350-keymouse-bridge-firmware.uf2
```

包装脚本优先从 `PATH` 查找 `elf2uf2-rs`，也可以使用 `ELF2UF2_PATH` 指定的可执行
文件。它会验证每个 UF2 块，并写入 RP2350 ARM Secure family ID `0xE48BFF59`；
不要在 Pico 2 上使用未经修正的 RP2040 family UF2。脚本会输出 UF2 的 SHA-256
摘要，但不会刷写板卡、打开串口或产生 HID 输入。

运行集成检查：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools/tests/build-release-uf2.ps1
```

### USB 身份

开发构建默认使用 USB VID/PID `0xCAFE:0x2350`。构建时可使用十进制或带 `0x` 前缀的
十六进制 `u16` 值覆盖任一 ID：

```powershell
$env:RP2350_USB_VID = "0x1234"
$env:RP2350_USB_PID = "0x5678"
cargo build --release
```

无效值会导致构建失败。生产分发必须使用合法分配的 USB 标识符。

固件启动时会读取 RP2350 OTP 芯片 ID，并公开由 `EXQC-KMOUSE-` 和 16 位大写十六进制
数字组成的 USB 序列号，使每颗芯片在 CDC/HID 枚举时都具有稳定身份。当前代码不会使用
共享或伪造的备用序列号：如果 `embassy_rp::otp::get_chipid()` 失败，程序会在 USB
枚举前触发 panic。

## 自动化验证

纯协议和解析器测试可以在 Windows 主机上运行：

```powershell
cargo test --target x86_64-pc-windows-msvc --lib
```

CI 使用的完整无硬件验证如下：

```powershell
cargo fmt --all -- --check
cargo test --target x86_64-pc-windows-msvc --lib
cargo clippy --release -- -D warnings
cargo build --release
cargo test --manifest-path tools/hidctl/Cargo.toml --target x86_64-pc-windows-msvc
node --test tools/webui/tests/protocol.test.mjs
uv run --project sdk/python python -m unittest discover -s sdk/python/tests -v
cmake -S sdk/cpp -B sdk/cpp/build
cmake --build sdk/cpp/build --config Release
ctest --test-dir sdk/cpp/build -C Release --output-on-failure
```

这些命令使用纯状态机和模拟传输层，绝不会刷写固件、选择串口设备或产生真实 HID 输入。
硬件验收是独立且必须显式启动的流程。

## 刷写固件

`.cargo\config.toml` 配置 RP2350 目标，并把 runner 交给 PowerShell 脚本：

```text
runner = "powershell -NoProfile -ExecutionPolicy Bypass -File tools/flash.ps1"
```

设置 `PICOTOOL_PATH`，或把 `picotool` 加入 `PATH`。安全的解析检查需要已有构建产物，
但不会调用 picotool：

```powershell
$env:PICOTOOL_PATH = "D:\Tool\picotool\picotool.exe"
powershell -NoProfile -ExecutionPolicy Bypass -File tools/flash.ps1 target\thumbv8m.main-none-eabihf\release\rp2350-keymouse-bridge-firmware -ResolveOnly
```

刷写是独立且必须显式执行的硬件操作。只有在将板卡置于 BOOTSEL 模式并确认需要刷写后，
才去掉 `-ResolveOnly`：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools/flash.ps1 target\thumbv8m.main-none-eabihf\release\rp2350-keymouse-bridge-firmware
```

该命令会运行 `picotool load -u -v -x -t elf`，因此有意不将其纳入构建或测试验证。
同样，`cargo run --release` 会调用配置的 runner，它是刷写命令，不是仅构建检查。

刷写后，设备应公开一个 CDC 串口 COM 端口以及 USB HID 键盘/鼠标接口。

## 构建 hidctl

`tools/hidctl` 是 Windows 主机命令行工具，用于检查串口协议并发送受控命令。

构建：

```powershell
cargo build --manifest-path tools\hidctl\Cargo.toml --release --target x86_64-pc-windows-msvc
```

运行：

```powershell
.\tools\hidctl\target\x86_64-pc-windows-msvc\release\hidctl.exe --help
```

列出端口：

```powershell
.\tools\hidctl\target\x86_64-pc-windows-msvc\release\hidctl.exe list
```

Ping 板卡：

```powershell
.\tools\hidctl\target\x86_64-pc-windows-msvc\release\hidctl.exe --port COM3 ping
```

读取设备信息和能力：

```powershell
.\tools\hidctl\target\x86_64-pc-windows-msvc\release\hidctl.exe --port COM3 info
.\tools\hidctl\target\x86_64-pc-windows-msvc\release\hidctl.exe --port COM3 caps
```

检查鼠标移动：

```powershell
.\tools\hidctl\target\x86_64-pc-windows-msvc\release\hidctl.exe --port COM3 mouse move 100 0
```

运行脚本：

```powershell
.\tools\hidctl\target\x86_64-pc-windows-msvc\release\hidctl.exe --port COM3 run examples\hidctl-demo.txt
```

### 手动硬件验收

固件或协议发生变化后，请重新构建 `hidctl`，确保主机工具与板卡使用相同的线协议格式。
先执行不会产生 HID 输入的命令：

```powershell
.\tools\hidctl\target\x86_64-pc-windows-msvc\release\hidctl.exe list
.\tools\hidctl\target\x86_64-pc-windows-msvc\release\hidctl.exe --port COM3 ping
.\tools\hidctl\target\x86_64-pc-windows-msvc\release\hidctl.exe --port COM3 info
.\tools\hidctl\target\x86_64-pc-windows-msvc\release\hidctl.exe --port COM3 caps
```

仅在可控的活动桌面上执行小幅真实输入检查，并以显式释放命令结束：

```powershell
.\tools\hidctl\target\x86_64-pc-windows-msvc\release\hidctl.exe --port COM3 mouse move 20 0
.\tools\hidctl\target\x86_64-pc-windows-msvc\release\hidctl.exe --port COM3 mouse move -20 0
.\tools\hidctl\target\x86_64-pc-windows-msvc\release\hidctl.exe --port COM3 mouse move 0 20
.\tools\hidctl\target\x86_64-pc-windows-msvc\release\hidctl.exe --port COM3 mouse move 0 -20
.\tools\hidctl\target\x86_64-pc-windows-msvc\release\hidctl.exe --port COM3 mouse move 1 20
.\tools\hidctl\target\x86_64-pc-windows-msvc\release\hidctl.exe --port COM3 stop
```

四个纯轴方向必须都产生幅度近似对称的光标位移。`mouse move 0 20` 若不移动、但
`mouse move 1 20` 可以移动，说明纯正向 Y HID 报告路径仍未通过验收。

进行安全生命周期验收时，通过 SDK 保持一个纯修饰键输入，然后终止该客户端，并确认
主机观察到自动释放。该流程会打开真实串口并产生真实 HID，因此有意要求手动执行。

## SDK 使用

生产调用统一通过 `rp2350_hid_bridge.dll` 的稳定 C ABI。C++ `HidSession` 是该 C ABI
的 RAII 包装，Python `HidSession` 是同一 DLL 的 `ctypes` 包装；两边都不再各自实现
串口协议、心跳或响应读取。

一个 COM 口只能由一个原生 session 拥有。主控先创建并打开 session，其他组件只能
retain/attach 同一个不透明句柄，不能再用 pyserial、hidctl 或第二个 SDK 实例同时打开
该 COM。全局释放由 session 所有者显式执行 `stop_all()`；最后一个引用释放时 DLL 才
停止心跳、取消 DTR 并关闭端口。

C++ SDK：

```powershell
cd sdk\cpp
cmake -S . -B build
cmake --build build --config Release
.\build\Release\test_protocol.exe
```

Python SDK：

```powershell
uv sync --project sdk/python
uv run --project sdk/python python -m unittest discover -s sdk/python/tests -v
```

协议 v2 重试、并发、关闭/重新打开、心跳和脚本会话保证请参阅 `sdk/cpp/README.md` 和
`sdk/python/README.md`。

## 协议与安全摘要

串口端点使用经过 CRC 校验的帧命令，默认采用协议 v2。标志为零且序列号非零的有效 v1
请求仍可用于基础命令，但 v1 `GET_CAPS` 不会声明 v2 的重试、租约或取消保证。v2 为
心跳流量保留序列号零和 `NO_RESPONSE`；普通请求/响应命令使用非零序列号。

支持的高级操作包括：

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

固件会确认已接受的命令；命令仍在执行时会报告忙状态；遇到无效帧或不支持的载荷时会
返回 NACK。

键盘状态包含八个修饰位和最多六个不同的非修饰键码。零键码表示纯修饰键操作，因此可以
在不改变普通按键的情况下保持或释放 Shift/Ctrl/Alt/GUI。第七个不同按键会以事务方式
被拒绝：该按键及请求中的修饰位都不会加入状态。`KEY_UP` 只移除请求的按键和修饰位。

v2 客户端每 500 毫秒发送一次无响应心跳。有效的 v2 流量会刷新两秒控制租约。只有存在
受保护工作（保持中的输入、已打开/正在运行的批处理或活动执行）时，租约到期才会取消
工作并尝试释放所有键盘和鼠标状态。DTR 下降沿或 USB 禁用会执行相同的安全重置。由于
旧客户端不发送心跳，v1 请求有意不武装租约。

`BATCH_BEGIN` 使用影子输入状态收集最多 32 条命令和 8 KiB 载荷；每个被接受的条目都会
先完成验证，`BATCH_END` 才开始独占、有序执行。这只保证验证阶段的事务性，不会回滚
物理 HID 报告：错误或停止发生前已经发出的报告无法撤回。`STOP_ALL`、DTR 中断、USB
禁用或租约到期可在协作边界取消等待、文本字符、移动分块、点击延迟和批处理工作。尚未
启动的批处理命令会被丢弃，所有输入状态会尽力释放。显式批处理之外不存在隐藏的普通
命令队列。

## LED 状态

板载 LED 提供基础状态反馈：

```text
断开时呼吸                USB 未连接主机
连接时双闪心跳            主机已连接
活动闪烁                  命令已接受/执行
错误三连闪                命令无效或发生协议错误
```

## 注意事项

固件会产生真实 USB HID 输入。只有在确认活动主机环境符合预期时，才使用主机工具和 SDK
示例。CI 始终不访问硬件；刷写和上述手动验收是独立且必须显式执行的操作。
