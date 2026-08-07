# Daedalus 代码分析与优化报告

> **项目**：RoboMaster 视觉算法验证模拟器（Daedalus）
> **技术栈**：Rust + Bevy 0.18 + avian3d + WGPU + ROS2/Talos IPC
> **审查范围**：弹丸发射、碰撞处理、能量机关状态机、ROS2/Talos 通信闭环、载具动力学、配置热重载、GPU 采集管线、可见性控制、无人机发射、旋转控制等全部核心模块
> **日期**：2026-08-08

---

## 目录

- [第一章：逻辑错误 — Critical 级别](#第一章逻辑错误--critical-级别)
- [第二章：逻辑错误 — High 级别](#第二章逻辑错误--high-级别)
- [第三章：逻辑错误 — Medium 级别](#第三章逻辑错误--medium-级别)
- [第四章：代码质量问题 — Low 级别](#第四章代码质量问题--low-级别)
- [第五章：性能优化建议](#第五章性能优化建议)
- [第六章：修复优先级总表](#第六章修复优先级总表)

---

## 第一章：逻辑错误 — Critical 级别

### C1. 弹丸出生点漏算云台安装平移

| 项 | 内容 |
|---|---|
| **位置** | `src/systems/projectile.rs` 第 68、124-126 行；`src/systems/uav.rs` 第 33、57 行 |
| **严重程度** | Critical |
| **影响范围** | 所有弹丸发射、无人机投放 |

#### 问题描述

```rust
launch_offset: Single<&Transform, (With<Controlled>, With<InfantryLaunchOffset>)>,
// ...
Transform::IDENTITY.with_translation(
    infantry.0.translation + (gimbal.0.rotation() * launch_offset.translation),
)
```

`launch_offset` 用的是 `&Transform`（局部坐标），其 `translation` 是 SHOT_DIRECTION 相对 GIMBAL 的偏移。代码只用 `gimbal.rotation()`（GlobalTransform 旋转）旋转该偏移，**漏掉了 GIMBAL 节点相对 BASE 的安装平移**（高度、前后偏移量）。

#### 原因分析

场景层级为 `root → BASE → GIMBAL → SHOT_DIRECTION`，`launch_offset.translation` 只是最后一段偏移。应直接用 `&GlobalTransform` 的 `translation()` 一步到位。

- 云台安装点一般在底盘上方中央，缺了这段 Y 高度 → 弹丸系统性偏低出生
- 云台 pitch 转动时，真实枪口绕云台安装点画弧，而代码让出生点绕底盘原点画弧 → 偏差大小和方向随角度变化
- 底盘旋转时偏移方向也跟着变 → "时好时偏"的根因

#### 修改建议

```rust
// 修改前
launch_offset: Single<&Transform, (With<Controlled>, With<InfantryLaunchOffset>)>,

// 修改后
launch_offset: Single<&GlobalTransform, (With<Controlled>, With<InfantryLaunchOffset>)>,
```

```rust
// 修改前
Transform::IDENTITY.with_translation(
    infantry.0.translation + (gimbal.0.rotation() * launch_offset.translation),
)

// 修改后
Transform::IDENTITY.with_translation(launch_offset.translation())
```

`uav.rs` 第 57 行 `spawn_pos` 同理修改。

---

### C2. 自瞄云台旋转增量在底盘旋转时计算错误

| 项 | 内容 |
|---|---|
| **位置** | `src/ros2/plugin.rs` 第 446-452 行；`src/talos/plugin.rs` 第 220-224 行 |
| **严重程度** | Critical |
| **影响范围** | 自瞄模式下的云台追踪 |

#### 问题描述

```rust
let expected_rotation = Quat::from_euler(EulerRot::YXZ, yaw_f32, pitch_f32, 0.0);
let current_rotation = muzzle_offset.0.rotation();          // 枪口世界旋转
let delta = expected_rotation * current_rotation.inverse();  // 世界空间增量
gimbal_transform.rotation = delta * gimbal_transform.rotation; // 直接乘到局部旋转
```

`delta` 是世界空间的旋转差，但 `gimbal_transform.rotation` 是 GIMBAL 相对父节点（BASE）的局部旋转。把世界增量直接乘到局部旋转上，**缺少共轭转换** `parent⁻¹ × delta × parent`。

#### 原因分析

正确公式为：

```
新局部旋转 = parent_rotation.inverse() * expected_rotation
```

当前写法 `delta * gimbal_local` 只在 `parent_rotation ≈ identity`（底盘不旋转）时碰巧正确。底盘一旦旋转，云台会转到错误方向。

- 静止时表现正常 → 隐蔽性强
- 实车对接时底盘移动/旋转后自瞄偏移甚至反向

#### 修改建议

```rust
// 需额外查询 GIMBAL 父节点（BASE）的 GlobalTransform
let parent_rotation = base_global_transform.rotation();
gimbal_transform.rotation = parent_rotation.inverse() * expected_rotation;
```

---

## 第二章：逻辑错误 — High 级别

### H1. ROS2 与 Talos 两通道 pitch 修正不一致

| 项 | 内容 |
|---|---|
| **位置** | `src/ros2/plugin.rs` 第 442 行 vs `src/talos/plugin.rs` 第 216 行 |
| **严重程度** | High |
| **影响范围** | 自瞄 pitch 控制方向 |

#### 问题描述

```rust
// ROS2 侧：pitch - 90
let pitch_f32 = (cmd.pitch as f32 - 90.0).to_radians();

// Talos 侧：-pitch - 90
let pitch_f32 = (-cmd.pitch_deg - 90.0).to_radians();
```

ROS2 是 `pitch - 90`，Talos 是 `-pitch - 90`，符号相反。同一物理姿态两通道表达的 pitch 角符号不同，算法从一个通道切到另一个时俯仰方向反转。

#### 修改建议

抽取统一的转换函数，两通道共用：

```rust
/// 将外部指令的 yaw/pitch（度）转换为云台期望旋转四元数
fn gimbal_cmd_to_rotation(yaw_deg: f64, pitch_deg: f64) -> Quat {
    let yaw = (yaw_deg as f32).to_radians();
    let pitch = (pitch_deg as f32 - 90.0).to_radians();
    Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0)
}
```

---

### H2. 载具加速曲线 max_speed=0 时除零产生 NaN

| 项 | 内容 |
|---|---|
| **位置** | `src/robomaster/vehicle/movement.rs` 第 65 行 |
| **严重程度** | High |
| **影响范围** | 配置误设时物理引擎崩溃 |

#### 问题描述

```rust
dirc * accel * (1.0 - (current_velocity.length() / max_speed).powf(self.n))
```

当 `max_speed == 0` 时，`0.0 / 0.0 = NaN`，`NaN.powf(n) = NaN`，`1.0 - NaN = NaN`，施加 NaN 的力会导致物理引擎崩溃。

#### 修改建议

```rust
let max_speed = self.max_speed * boost;
if max_speed <= 0.0 {
    return Vec3::ZERO;
}
let accel = self.linear_acceleration * boost;
dirc * accel * (1.0 - (current_velocity.length() / max_speed).powf(self.n))
```

---

### H3. 能量机关碰撞用 CollisionEnd 而非 CollisionStart

| 项 | 内容 |
|---|---|
| **位置** | `src/robomaster/power_rune/collision.rs` 第 78 行 |
| **严重程度** | High |
| **影响范围** | 能量机关命中判定时序 |

#### 问题描述

```rust
fn handle_rune_collision(
    event: On<CollisionEnd>,  // 碰撞结束才触发
```

弹丸碰上目标面板时，命中判定在碰撞**结束**时才触发，而非碰撞**开始**时。这会导致：

- 命中延迟（弹丸弹开/滑过后才触发）
- 弹丸如果碰撞期间被生命周期清理销毁，可能永远不触发 `CollisionEnd`

#### 修改建议

改用 `On<CollisionStart>`：

```rust
fn handle_rune_collision(
    event: On<CollisionStart>,
```

或确认是否有特殊设计意图（如等待弹丸稳定接触后再判定）。

---

### H4. 配置热重载无 debounce

| 项 | 内容 |
|---|---|
| **位置** | `src/config.rs` 第 505-519 行 |
| **严重程度** | High |
| **影响范围** | 配置保存时短暂解析失败 |

#### 问题描述

```rust
while let Ok(Ok(event)) = watcher.receiver.try_recv() {
    if event.kind.is_modify() {
        match SimulationConfig::load() { ... }
    }
}
```

文本编辑器保存文件时触发多个 modify 事件（临时文件写入 → rename，或多次 flush）。循环为每个事件重新加载一次，中间可能读到半写入的文件。

#### 修改建议

加 200ms debounce，只在最后一次事件后重新加载：

```rust
use std::time::{Duration, Instant};

fn config_hot_reload(
    mut config: ResMut<SimulationConfig>,
    mut last_event: Local<Option<Instant>>,
    watcher: Option<Res<ConfigWatcher>>,
) {
    let Some(watcher) = watcher else { return; };

    let mut has_events = false;
    while let Ok(Ok(event)) = watcher.receiver.try_recv() {
        if event.kind.is_modify() {
            has_events = true;
        }
    }

    if has_events {
        *last_event = Some(Instant::now());
    }

    // 等 200ms 无新事件后再重新加载
    if let Some(t) = *last_event {
        if t.elapsed() >= Duration::from_millis(200) {
            match SimulationConfig::load() {
                Ok(new_config) => {
                    info!("Config reloaded successfully");
                    *config = new_config;
                }
                Err(e) => warn!("Failed to reload config: {}", e),
            }
            *last_event = None;
        }
    }
}
```

---

## 第三章：逻辑错误 — Medium 级别

### M1. PhysicsConfig 等 4 个配置节缺少 #[serde(default)]

| 项 | 内容 |
|---|---|
| **位置** | `src/config.rs` 第 52-61 行 |
| **严重程度** | Medium |
| **影响范围** | 配置容错性 |

`physics`、`vehicle`、`projectile`、`camera` 没有 `#[serde(default)]`，缺失时整份配置解析失败、全部回退硬编码默认值。用户的正确配置被全部丢弃。

#### 修改建议

```rust
#[serde(default)]
pub physics: PhysicsConfig,

#[serde(default)]
pub vehicle: VehicleConfig,

#[serde(default)]
pub projectile: ProjectileConfig,

#[serde(default)]
pub camera: CameraConfig,
```

并为这 4 个结构体实现 `Default` trait。

---

### M2. 大机关每轮清除所有目标状态

| 项 | 内容 |
|---|---|
| **位置** | `src/robomaster/power_rune/state.rs` 第 507-514 行 |
| **严重程度** | Medium |
| **影响范围** | 能量机关视觉效果 |

```rust
fn start_large_primary_round(&mut self, rng: &mut impl Rng) -> RunTransition {
    self.clear_all_targets();  // 清除所有目标
    let targets = self.choose_targets_from_all(2, rng);
```

大机关每轮清除所有目标状态（包括上一轮已激活的），然后重新随机选 2 个。上一轮已命中的目标灯会熄灭，可能不符合 RM 规则。

#### 修改建议

如需保留已激活目标，改为：

```rust
self.clear_transient_targets();  // 只清除 Activating，保留 Activated
let targets = self.choose_targets(2, rng);  // 仅从未激活中选
```

---

### M3. fire_advice 的 run_system_once unwrap 可能 panic

| 项 | 内容 |
|---|---|
| **位置** | `src/ros2/plugin.rs` 第 436 行、`src/talos/plugin.rs` 第 211 行 |
| **严重程度** | Medium |
| **影响范围** | 程序稳定性 |

```rust
commands.queue(|w: &mut World| {
    w.run_system_once(projectile_launch).unwrap();
});
```

#### 修改建议

```rust
commands.queue(|w: &mut World| {
    if let Err(e) = w.run_system_once(projectile_launch) {
        warn!("projectile_launch failed: {e}");
    }
});
```

---

### M4. distance == -1.0 浮点精确比较

| 项 | 内容 |
|---|---|
| **位置** | `src/ros2/plugin.rs` 第 429 行、`src/talos/plugin.rs` 第 205 行 |
| **严重程度** | Medium |

```rust
if cmd.distance == -1.0 { return; }
```

#### 修改建议

```rust
if (cmd.distance + 1.0).abs() < 1e-6 { return; }
```

---

### M5. config.toml 路径硬编码为相对路径

| 项 | 内容 |
|---|---|
| **位置** | `src/config.rs` 第 326、473 行 |
| **严重程度** | Medium |

```rust
std::fs::read_to_string("config.toml")?;
watcher.watch(Path::new("config.toml"), ...)
```

依赖当前工作目录，从不同目录运行程序会找不到文件。

#### 修改建议

```rust
let config_path = std::env::current_exe()
    .ok()
    .and_then(|exe| exe.parent().map(|dir| dir.join("config.toml")))
    .unwrap_or_else(|| PathBuf::from("config.toml"));

let content = std::fs::read_to_string(&config_path)?;
watcher.watch(&config_path, RecursiveMode::NonRecursive)?;
```

---

### M6. 弹丸继承底盘角速度而非自旋

| 项 | 内容 |
|---|---|
| **位置** | `src/systems/projectile.rs` 第 122 行 |
| **严重程度** | Medium |

```rust
AngularVelocity(infantry.2.0),    // 继承战车自转的角速度
```

子弹出膛后应绕枪管轴自旋，不是底盘旋转。需确认设计意图。

#### 修改建议

如需自旋效果：

```rust
AngularVelocity(direction * SPIN_RATE),  // 绕枪管轴自旋
```

---

### M7. uav.rs 冷却计时器在未按键时也 reset

| 项 | 内容 |
|---|---|
| **位置** | `src/systems/uav.rs` 第 39-51 行 |
| **严重程度** | Medium |

```rust
timer.reset();        // 无条件 reset
if keyboard.pressed(KeyCode::KeyP) {  // 然后才检查按键
```

冷却结束后无论是否按 P 都 reset，导致未按键时也消耗冷却周期。

#### 修改建议

```rust
if keyboard.pressed(KeyCode::KeyP) && timer.is_finished() {
    timer.reset();
    // spawn...
}
```

---

### M8. telemetry 模块整体未接线

| 项 | 内容 |
|---|---|
| **位置** | `src/telemetry/` 整个模块 |
| **严重程度** | Medium |

`TelemetryPlugin` 已实现但从未在 `main.rs` 注册，`FrameData` 数据流是断的。

#### 修改建议

在 `main.rs` 添加：

```rust
app.add_plugins(TelemetryPlugin);
```

或如果暂不使用则删除死代码。

---

## 第四章：代码质量问题 — Low 级别

### L1. movement.rs forward/right 重复 with_y(0.0)

**位置**：`src/robomaster/vehicle/movement.rs` 第 58-61 行

```rust
let forward = gimbal_transform.forward().with_y(0.0);
let right = gimbal_transform.right().with_y(0.0);
let forward_xz = forward.with_y(0.0).normalize_or_zero();  // 重复 with_y
let right_xz = right.with_y(0.0).normalize_or_zero();      // 重复 with_y
```

**修改建议**：删除中间变量，直接链式调用。

---

### L2. capture_rune 中 targets 变量名遮蔽

**位置**：`src/ros2/plugin.rs` 第 201、235 行

Query 参数 `targets` 被 fold 结果（HashMap）遮蔽。

**修改建议**：重命名 fold 结果为 `activated_targets`。

---

### L3. GPU 采集管线多处 unwrap/expect

**位置**：`src/capture/driver.rs` 第 244、273、346、356、370 行

```rust
let block_size = src_image.texture_format.block_copy_size(None).unwrap();
res.expect("Failed to map buffer");
s.send(dat).expect("Failed to send map update");
r.await.expect("Failed to receive the map_async message");
```

**修改建议**：异步回调中改为 log 错误而非 panic。

---

### L4. visibility.rs set_visibility 的 unwrap

**位置**：`src/robomaster/visibility.rs` 第 112、116 行

```rust
set_visibility(*entity, Visibility::Hidden, &mut param.visibilities).unwrap();
```

**修改建议**：改为 `if let Ok(_) = ...` 或 `.ok()`。

---

### L5. rotation.rs speed() 范围注释不准确

**位置**：`src/robomaster/power_rune/rotation.rs` 第 38 行

```rust
/// 速度在 `[0, 2*2.090 - 2*a]` 范围内变化，保证非负。
```

实际范围是 `[2.090 - 2*a, 2.090]`。

**修改建议**：修正注释为 `/// 速度在 [2.090 - 2*a, 2.090] 范围内变化，保证非负。`

---

### L6. 6 个编译警告

| 文件 | 行号 | 警告 |
|---|---|---|
| `src/capture.rs` | 4 | unused import `bevy::anti_alias::fxaa::Fxaa` |
| `src/capture.rs` | 8 | unused imports `Bloom`, `BloomCompositeMode`, `BloomPrefilter` |
| `src/capture.rs` | 10 | unused import `bevy::render::view::Hdr` |
| `src/systems/uav.rs` | 5 | unused import `ProjectileCooldown` |
| `src/systems/uav.rs` | 24 | variable does not need to be mutable (`mut timer`) |
| `src/main.rs` | 122 | unused variable `app` |

**修改建议**：

- 删除未使用的 import
- `let mut timer` → `let timer`（uav.rs:24）
- `fn should_enable_talos_plugin(app: &App)` → `fn should_enable_talos_plugin(_app: &App)`（main.rs:122）

---

### L7. README 话题名过期

**位置**：`README.md`

文档写订阅 `/armor_solver/cmd_gimbal`，实际代码是 `/rm_gimbal/cmd`（消息类型 `rm_interfaces/msg/GimbalCmd`，QoS `sensor_data`）。

**修改建议**：更新 README 中的话题名、消息类型、QoS 配置。

---

## 第五章：性能优化建议

### P1. 使用 release 构建运行

**问题**：debug 模式 17 FPS，release 模式可达 80-150 FPS。

**建议**：

```bash
# 日常使用
cargo run --release

# 不采图时关掉 talos 减负
cargo run --release --no-default-features
```

---

### P2. 降低物理子步数

**问题**：`substep_count = 20`，每帧 20 次物理迭代，debug 下尤其致命。

**建议**：在 `config.toml` 中改为 6-8，热重载即可生效：

```toml
[physics]
substep_count = 6
```

---

### P3. 关闭阴影

**问题**：`[render] shadows = true` 开销大。

**建议**：调试时设为 `false`：

```toml
[render]
shadows = false
```

---

### P4. 排查每帧 117 次资源加载

**问题**：`started_load_count: 117` 恒定，疑似资产被反复 load。

**建议**：

```bash
RUST_LOG=bevy_asset=debug cargo run --release
```

检查是否有系统在每帧 `asset_server.load(...)` 而非缓存 handle。

---

### P5. 默认 feature 改为空

**问题**：`Cargo.toml` 中 `default = ["talos"]`，默认开启 talos 共享内存采集，每帧拷贝 1440×1080 图像。

**建议**：

```toml
[features]
default = []
```

需要采集时显式 `--features talos`。

---

### P6. egui inspector 仅调试时开

**问题**：`egui = true` + `inspector = true` 显著掉帧。

**建议**：仅在需要时通过 `config.toml` 热重载开启，用完即关。

---

## 第六章：修复优先级总表

| 优先级 | 编号 | 问题 | 工作量 | 预期效果 |
|---|---|---|---|---|
| **立即** | C1 | 弹丸出生点漏算平移 | 改 2 文件各 2 行 | 弹道对齐枪口 |
| **立即** | C2 | 云台旋转增量计算错误 | 改 2 文件各 3 行 | 底盘旋转时自瞄正确 |
| **尽快** | H1 | pitch 修正不一致 | 抽取公共函数 | 两通道行为一致 |
| **尽快** | H2 | 除零产生 NaN | 加 1 行检查 | 防止崩溃 |
| **尽快** | H3 | CollisionEnd 改 Start | 改 1 行 | 命中时序正确 |
| **尽快** | H4 | 热重载 debounce | 加 ~15 行 | 配置保存稳定 |
| **短期** | M1 | serde default | 加 4 个注解 | 配置容错性提升 |
| **短期** | M2 | 大机关目标清除 | 改 2 行 | 视觉效果正确 |
| **短期** | M3 | unwrap 改 log | 改 2 处 | 防止 panic |
| **短期** | M4 | 浮点比较 | 改 2 处 | 健壮性提升 |
| **短期** | M5 | 配置路径 | 改 ~5 行 | 跨目录运行 |
| **短期** | M6 | 弹丸角速度 | 确认设计 | 碰撞行为正确 |
| **短期** | M7 | UAV 冷却逻辑 | 改 3 行 | 冷却语义正确 |
| **短期** | M8 | telemetry 接线 | 加 1 行 | 遥测可用 |
| **短期** | L1-L5 | 代码质量 | 各 1-2 行 | 可读性提升 |
| **短期** | L6 | 编译警告清理 | 删 import | 代码卫生 |
| **短期** | L7 | README 更新 | 改文档 | 文档准确 |
| **中期** | P1-P6 | 性能优化 | 配置调整 | 帧率提升 |

---

## 附录：问题统计

| 严重程度 | 数量 |
|---|---|
| Critical | 2 |
| High | 4 |
| Medium | 8 |
| Low | 7 |
| 性能优化 | 6 |
| **合计** | **27** |

> **最关键的两个修复是 C1 和 C2**，它们直接导致弹道偏移和自瞄不准现象。建议优先修复这两项后进行实弹验证。
