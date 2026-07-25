# RP2350 纯正向 Y HID 兼容修复设计

## 背景与证据

生产机使用协议 v2 固件、C++ SDK 和 `COM4` 进行独立桌面光标测试，得到：

```text
dx= 80, dy=  0 -> cursor_delta= 194,   0
dx=-80, dy=  0 -> cursor_delta=-193,   0
dx=  0, dy= 80 -> cursor_delta=   2,   0
dx=  0, dy=-80 -> cursor_delta=   0,-193
dx=  1, dy= 80 -> cursor_delta=   1, 193
```

测试时光标位于屏幕中央，没有触及桌面边界。结果证明：

- C++ SDK、串口、协议 v2 会话和 HID 设备均已工作；
- X 正负方向和 Y 负方向对称；
- 仅 `dx == 0 && dy > 0` 的纯正向 Y 报告没有产生主机可见位移；
- 只要同一报告带一个非零 X，正向 Y 就会恢复。

C++ SDK 的 `i16_pair_payload()` 使用大端有符号 16 位编码，固件的
`decode_mouse_move()` 使用相同格式。`RelativeMovementSteps` 也能在源码层产生
`(0, positive_y)`。因此当前证据把故障限制在刷入固件的 HID 报告提交、USB 栈或
Windows 对该报告形态的实际处理边界，不能归因于 ORT、DXGI、模型或灵敏度拟合。

## 决策

在固件执行层增加一个很小的主机兼容发送单元，不改变线协议：

1. 对 `x != 0`、`y <= 0` 或零移动，保持现有报告不变。
2. 对每个 `x == 0 && y > 0` 的相对移动分片，依次发送：

   ```text
   MouseReport(x= 1, y=y)
   MouseReport(x=-1, y=0)
   ```

3. 两份报告使用相同的鼠标按钮状态，滚轮和水平滚轮均为零。
4. 命令只有在两份报告均成功提交后才返回 ACK。
5. 大于 127 的正向 Y 继续按现有规则分片，每个纯正向分片独立补偿，保证全部报告的
   X 代数和为零、Y 代数和等于原命令。
6. 每个“辅助报告 + 补偿报告”是一个不可插入取消检查的发送单元：发送前和完成后仍
   按现有规则检查取消，避免正常取消路径在两份报告之间留下可避免的水平偏移。

该方案利用生产机已经验证有效的 `(x=1, y>0)` 报告形态，并立即用 `x=-1` 抵消辅助
位移。最坏情况下只出现一次 1 count 的瞬时水平脉冲，不产生持续漂移，也不改变最终
目标坐标。

## 代码边界

修复放在固件的命令执行/HID 报告边界，覆盖所有主机客户端：

- `src/execution_core.rs`：集中发送一个相对移动分片及必要补偿；
- `src/lib.rs`：增加使用真实执行路径的回归测试；
- 如需复用，允许在 `src/execution_core.rs` 内增加私有辅助函数，不扩展公开 API。

不修改：

- 串口协议版本、命令号、载荷格式或能力位；
- C++/Python/Web SDK；
- C++ 视觉 DLL、C API、Python 包装层；
- ORT、CUDA、cuDNN、TensorRT、DXGI 或标定曲线格式。

## 测试设计

测试必须先失败，再实现修复。至少覆盖：

1. `MouseMoveRel { dx: 0, dy: 80 }` 产生 `(1,80)`、`(-1,0)`，净位移为 `(0,80)`。
2. `MouseMoveRel { dx: 0, dy: -80 }` 仍只产生 `(0,-80)`。
3. `MouseMoveRel { dx: 1, dy: 80 }` 保持单份 `(1,80)`，不重复补偿。
4. `MouseMoveRel { dx: 0, dy: 300 }` 的所有报告净位移为 `(0,300)`，每个报告仍在
   `i8` 范围内。
5. 取消在兼容发送单元之前生效时不发送报告；单元开始后要先提交补偿，再观察下一次
   取消。
6. 既有取消、复位、批处理和安全租约测试继续通过。

完整验证：

```powershell
cargo fmt --all -- --check
cargo test --target x86_64-pc-windows-msvc --lib
cargo clippy --release -- -D warnings
cargo build --release --locked
powershell -NoProfile -ExecutionPolicy Bypass -File tools/tests/build-release-uf2.ps1
```

## 交付与硬件验收

生成新的 RP2350 ARM-S UF2，记录文件大小和 SHA-256。生产机刷写后先在桌面中央重复：

```text
(80,0) (-80,0) (0,80) (0,-80) (1,80)
```

验收要求四个纯轴方向幅度近似对称，`(0,80)` 不再为零。通过后才重新进行 CS2
灵敏度标定。若新版 UF2 仍无法让纯正向 Y 生效，则停止叠加补丁，转入 USB 抓包和 HID
描述符级调查。

## 风险与回退

- 风险：正向 Y 分片包含一个瞬时 1 count 水平脉冲；紧随其后的反向报告负责抵消。
- USB 写入在两份报告之间失败时，最多遗留一次 1 count 相对位移，不会形成保持状态。
- 回退只需重新刷入原 UF2；协议和主机软件没有迁移成本。
