# Daedalus 项目知识库

> 本文档为 RoboMaster 视觉算法验证模拟器的技术背景知识库，涵盖前置概念、框架原理、项目结构、环境配置等,帮助理解项目的技术背景和实现逻辑。

---

## 第一章：前置技术知识

### 1.1 Rust 语言基础

本项目使用 Rust 2024 edition，需要掌握以下核心概念：

#### 所有权与借用

```rust
// 所有权转移
let s1 = String::from("hello");
let s2 = s1;          // s1 的所有权转移给 s2，s1 不再可用

// 借用（不可变引用，可多个）
let r1 = &s2;
let r2 = &s2;         // OK，多个不可变引用共存

// 可变借用（独占）
let r3 = &mut s2;     // OK，但不能再有其他引用
```

**项目中的应用**：Bevy ECS 系统参数就是借用世界中的数据。`Query<&Transform>` 是不可变借用（多个系统可同时读），`Query<&mut Transform>` 是可变借用（独占写）。这就是为什么有些系统要用 `Single<&mut T>` 而不是 `&mut T`——ECS 调度器需要知道你要独占还是共享。

#### 生命周期

```rust
// 'a 表示引用的有效期
fn longest<'a>(x: &'a str, y: &'a str) -> &'a str {
    if x.len() > y.len() { x } else { y }
}
```

**项目中的应用**：`StatefulAppearance<'w, 's>` 带两个生命周期参数。`'w` 绑定 World（世界），`'s` 绑定 SystemState（系统状态）。这是 Bevy SystemParam 的标准写法，表示这个参数借用了世界中的资源，其有效期不能超过世界本身。

#### trait 与泛型

```rust
// trait 定义接口
trait Control {
    fn set(&self, state: Activation, param: &mut StatefulAppearance);
}

// 泛型约束
fn process<T: Control>(item: &T) { ... }

// 动态分发（trait object）
let controller: Box<dyn Control> = Box::new(MaterialController { ... });
```

**项目中的应用**：`Controller` 枚举实现了 `Control` trait，有 `Material` / `Visibility` / `Combined` 三个变体。调用 `controller.set(state, param)` 时，根据变体分发到不同实现。这是 Rust 式的多态——用枚举代替继承。

#### 枚举与模式匹配

```rust
enum Activation {
    Deactivated,
    Activating,
    Activated,
    Completed,
}

match state {
    Activation::Deactivated => { /* 熄灭 */ }
    Activation::Activating  => { /* 闪烁 */ }
    Activation::Activated   => { /* 常亮 */ }
    Activation::Completed   => { /* 完成色 */ }
}
```

**项目中的应用**：RM 规则中的所有"状态"都用枚举表达——`Activation`（激活态）、`TechCorePhase`（科技核心 9 阶段）、`PowerRuneMode`（大小机关）、`Team`（红蓝方）。模式匹配是状态机实现的基础。

#### 宏

```rust
// 声明式宏
macro_rules! vec_of_strings {
    ($($x:expr),*) => { vec![$($x.to_string()),*] };
}

// 派生宏
#[derive(Debug, Clone, Deserialize)]
struct Config { ... }
```

**项目中的应用**：
- `entity_root!`：声明式宏，按名称模式匹配场景树节点挂组件
- `material!` / `visibility!`：声明式宏，一行生成外观控制器
- `plugin_group!`：声明式宏，聚合多个插件
- `#[derive(Deserialize)]`：派生宏，自动生成 TOML 反序列化代码
- `#[derive(SystemParam)]`：派生宏，把结构体包装成 Bevy 系统参数

#### 条件编译

```rust
#[cfg(feature = "ros2")]
{
    app.add_plugins(ROS2Plugin::default());
}
```

**项目中的应用**：`ros2` / `talos` / `ffmpeg` 三个 feature 开关。`#[cfg(feature = "ros2")]` 是**编译时**决策，不是运行时。没开 feature 的代码根本不会编译进二进制，零运行时开销。

#### 错误处理

```rust
// Result<T, E> 强制处理错误
fn load_config() -> Result<Config, ConfigError> { ... }

// ? 运算符：成功取值，失败提前返回
let content = std::fs::read_to_string("config.toml")?;

// unwrap / expect：成功取值，失败 panic
let mode = present_mode_from_config(&config).unwrap_or_else(|| PresentMode::AutoNoVsync);
```

**项目中的应用**：配置加载用 `Result` + `?` 链式传播错误；资源初始化用 `unwrap_or_else` 提供兜底值；不可恢复的致命错误才 `panic`。

---

### 1.2 数学基础

#### 坐标系

| 坐标系 | 朝向 | 使用场景 |
|---|---|---|
| Bevy 世界坐标 | Y-up，-Y 为重力方向 | 仿真器内部 |
| ROS REP-103 | Z-up，X 向前 | ROS2 话题、TF 树 |
| 相机坐标 | Z 向前，Y 向下 | 图像采集 |

**转换关系**（项目中 `M_ALIGN_MAT3`）：

```
Bevy (x, y, z) → ROS (x, -z, y)
旋转：绕 Z 轴旋转 ±90° 的对齐矩阵
```

#### 四元数

```rust
// Bevy 用 glam::Quat 表示旋转
let rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);

// 旋转组合：q1 * q2 表示先 q2 后 q1
let combined = q1 * q2;

// 逆旋转（反向旋转）
let inverse = q1.inverse();

// 世界→局部坐标转换
let local = parent_rotation.inverse() * world_rotation;
```

**项目中的应用**（C2 bug 的核心）：
```rust
// 错误写法：把世界增量直接乘到局部旋转
let delta = expected_world * current_world.inverse();
gimbal_local = delta * gimbal_local;  // ← 缺少共轭转换

// 正确写法：把期望世界旋转转回局部空间
gimbal_local = parent_world.inverse() * expected_world;
```

#### 欧拉角

```rust
// EulerRot::YXZ 表示按 Y→X→Z 顺序应用旋转
let q = Quat::from_euler(EulerRot::YXZ, yaw, pitch, roll);
```

**项目中的应用**：云台指令的 yaw/pitch 用 `EulerRot::YXZ` 构造四元数。顺序很重要——不同顺序会得到不同结果，必须和发送端约定一致。

---

## 第二章：Bevy 引擎核心原理

### 2.1 ECS 架构

ECS = Entity + Component + System，是 Bevy 的核心设计模式。

#### 三要素

```rust
// Entity：只是一个 ID，不存数据
let entity = commands.spawn(()).id();

// Component：挂在实体上的数据片段
#[derive(Component)]
struct Armor { team: Team, spec: ArmorSpec }

// System：每帧自动执行的函数，查询并处理组件
fn update_armors(mut query: Query<&mut Armor>) {
    for mut armor in &mut query {
        // 处理每个带 Armor 组件的实体
    }
}

// Resource：全局单例数据，不挂在实体上
#[derive(Resource)]
struct SimulationConfig { ... }
```

#### 数据驱动设计

传统 OOP：
```python
class Armor:
    def __init__(self): self.hp = 100
    def take_damage(self, dmg): self.hp -= dmg

armor = Armor()
armor.take_damage(10)
```

ECS：
```rust
// 数据（Armor 组件）和逻辑（take_damage 系统）分离
#[derive(Component)]
struct Armor { hp: i32 }

fn take_damage_system(mut query: Query<&mut Armor>, ...) {
    for mut armor in &mut query {
        armor.hp -= 10;
    }
}
```

**优势**：数据连续存储（CPU 缓存友好）、批量处理、可灵活组合查询条件。

### 2.2 调度阶段

Bevy 的主循环按固定阶段执行：

```
每一帧：
├─ First          最先执行
├─ PreUpdate      Update 之前
├─ Update         ← 业务主逻辑（大部分系统在这里）
├─ PostUpdate     Update 之后，渲染之前
│   └─ TransformSystems::Propagate  ← 坐标传播
├─ Last           最后执行
└─ Render         渲染出图（独立的子应用）

FixedUpdate       ← 固定步长，独立于帧率（物理仿真专用）
```

**项目中的应用**：

```rust
// Update 阶段：业务逻辑，按 SystemSet 链式排序
.add_systems(Update, (
    (auto_aim_switch, vehicle_controls, ...).in_set(GameplaySystems::Input),
    (change_appearance, ...).in_set(GameplaySystems::GameLogic),
    (update_camera_follow, ...).in_set(GameplaySystems::Camera),
    (cleanup_projectiles, ...).in_set(GameplaySystems::Cleanup),
).chain())

// PostUpdate 阶段：必须等坐标传播完成
.add_systems(PostUpdate, projectile_launch
    .after(TransformSystems::Propagate)  // ← 关键时序约束
    .run_if(|kb: Res<ButtonInput<KeyCode>>| kb.pressed(KeyCode::Space)))

// FixedUpdate 阶段：物理仿真，步长恒定
.add_systems(FixedUpdate, projectile_aerodynamics)
```

**为什么弹丸发射放 PostUpdate 而不是 Update**：Update 阶段云台刚转完，但坐标还没传播到子节点（枪口）。PostUpdate.after(Propagate) 才能拿到最新枪口位置，否则弹丸出生点用的是上一帧的坐标。

### 2.3 插件系统

```rust
// Plugin trait：一组打包好的资源+系统+事件
trait Plugin {
    fn build(&self, app: &mut App);
}

// 自定义插件
struct MyPlugin;
impl Plugin for MyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MyResource>()
           .add_systems(Update, my_system);
    }
}

// 挂载
app.add_plugins(MyPlugin);
```

**项目中的应用**：`RoboMasterPlugins` 是聚合插件，用 `plugin_group!` 宏把 5 个子插件打包：

```rust
plugin_group!(RoboMasterPlugins {
    StatefulAppearancePlugin,  // 外观控制基础设施
    ArmorPlugins,              // 装甲系统
    PowerRunePlugins,          // 能量机关
    OutpostPlugins,            // 前哨站
    TechCorePlugins,           // 科技核心
});
```

### 2.4 观察者与事件

```rust
// 事件类型
#[derive(Event)]
struct SceneInstanceReady { entity: Entity }

// 观察者：事件触发时自动执行
app.add_observer(setup_vehicle);  // 监听 SceneInstanceReady

fn setup_vehicle(trigger: On<SceneInstanceReady>, ...) {
    let entity = trigger.entity();  // 获取触发事件的实体
    // 处理逻辑
}
```

**Observer vs System 的区别**：
- System：每帧自动跑
- Observer：事件触发时跑，可能一帧跑多次，也可能多帧不跑

**项目中的应用**：
- `SceneInstanceReady` → `setup_vehicle` / `setup_power_rune` / `setup_tech_core`（场景加载后构造）
- `CollisionEnd` → `handle_armor_collision` / `handle_rune_collision`（碰撞命中）
- `RuneHit` / `RuneActivated` → `on_hit` / `on_activate`（业务事件链）

### 2.5 查询与过滤

```rust
// 基础查询：获取所有带 Armor 组件的实体
fn system(query: Query<&Armor>) { ... }

// 多组件查询
fn system(query: Query<(&Armor, &mut Transform)>) { ... }

// 过滤器
fn system(
    query: Query<
        &Armor,                              // 要读取的组件
        (With<Activated>, Without<Dead>)     // 过滤条件
    >
) { ... }

// Added 过滤器：只在组件首次添加的那帧执行
fn system(query: Query<&Armor, Added<Armor>>) { ... }

// Changed 过滤器：只在组件值变化时执行
fn system(query: Query<&Armor, Changed<Armor>>) { ... }

// Single：确保只有一个匹配实体（否则 panic）
fn system(single: Single<&mut Transform, With<Controlled>>) { ... }
```

**项目中的应用**：
```rust
// 只在实体首次加上 ScanArmor 组件时扫描装甲
fn insert(root: Query<(Entity, Read<ScanArmor>), Added<ScanArmor>>, ...)

// 只在贴纸选择变化时同步
fn sync_armor_stickers(
    selections: Query<&ArmorStickerSelection, Changed<ArmorStickerSelection>>,
    ...
)
```

### 2.6 条件系统 run_if

```rust
// 闭包返回 true 才执行该系统
.add_systems(Update, 
    vehicle_controls
        .run_if(|mode: Res<CameraMode>| mode.0 != FollowingType::Free)
)
```

**项目中的应用**：自由视角模式下 WASD 给相机用，不操控底盘；跟随模式下 WASD 操控底盘。用 `run_if` 而不是 `if` 分支，因为系统注册了但被跳过，比分支更高效。

---

## 第三章：关键依赖库原理

### 3.1 avian3d 物理引擎

负责刚体动力学、碰撞检测、重力模拟。

#### 核心概念

```rust
// 刚体组件
#[derive(Component)]
struct RigidBody;  // 标记为动态刚体，受力和碰撞影响

// 碰撞体
#[derive(Component)]
struct Collider;   // 碰撞形状（球体/方块/三角网格）

// 碰撞层（分组）
#[derive(PhysicsLayer)]
enum GameLayer {
    Player,      // 玩家
    Enemy,       // 敌人
    Projectile,  // 弹丸
    Ground,      // 地面
}

// 子步迭代
app.insert_resource(SubstepCount(20));  // 每帧物理迭代 20 次
```

#### 碰撞事件

```rust
// CollisionStart：碰撞开始
fn handler(event: On<CollisionStart>) { ... }

// CollisionEnd：碰撞分离
fn handler(event: On<CollisionEnd>) { ... }
```

**项目中的应用**：装甲和能量机关的命中判定都基于碰撞事件。弹丸实体带 `CollisionEventsEnabled` 组件才会触发事件，命中后移除该组件防止重复计数。

#### 子步迭代的影响

`SubstepCount` 越大，物理仿真越精确但越慢。Debug 模式下 `substep_count=20` 是帧率低的重要原因之一，日常调试可降到 6-8。

### 3.2 r2r（ROS2 Rust 客户端）

提供 ROS2 话题发布/订阅、TF 树、QoS 等功能。

#### 核心概念

```rust
// 创建节点
let ctx = r2r::Context::create()?;
let mut node = ctx.create_node("daedalus")?;

// 发布话题
let publisher = node.create_publisher::<sensor_msgs::msg::Image>(
    "/image_raw", r2r::QoS::default()
)?;

// 订阅话题
let subscriber = node.subscribe::<rm_interfaces::msg::GimbalCmd>(
    "/rm_gimbal/cmd", r2r::QoS::sensor_data()
)?;
```

#### QoS（服务质量）

| QoS 策略 | 含义 | 适用场景 |
|---|---|---|
| `default()` | 可靠传输 + 保持最近 10 条 | 配置、命令 |
| `sensor_data()` | 尽力传输 + 保持最近 5 条 | 传感器数据（图像/点云） |

**项目中的应用**：图像话题用 `sensor_data()`（允许丢帧，保证实时性）；云台指令用 `sensor_data()`（只关心最新指令）。

#### TF 树

ROS2 用 TF 树描述坐标系之间的变换关系：

```
map → odom → base_link → gimbal_link → camera_link → muzzle_link
```

**项目中的应用**：[ros2/plugin.rs](src/ros2/plugin.rs) 用 `tf_tree!` 宏声明式描述 6 层 TF 树，每帧发布各坐标系的位姿。

### 3.3 talos-ipc（自研零拷贝 IPC）

项目自研的共享内存通信库，用于和 C++ 自瞄程序高速交换图像/位姿数据。

#### 三缓冲模式

```
槽位 0: 生产者写入中
槽位 1: 消费者读取中
槽位 2: 待用（下一轮生产者写这里）

状态字节：FLAG_NEW(bit7) + 槽位索引(bit0-1)
- 生产者写完：置 FLAG_NEW，切换到下一个空槽
- 消费者读最新：找 FLAG_NEW 的槽，读取，清 FLAG_NEW
```

#### C-ABI 兼容

```rust
#[repr(C, align(64))]  // C 布局 + 64 字节对齐（cache line）
struct ImageFrame {
    seq: u64,
    timestamp_ns: u64,
    data: [u8; IMAGE_SIZE],
}

// const assert 锁死字段偏移，防止 Rust 版本升级改变布局
const _: () = assert!(std::mem::offset_of!(ImageFrame, seq) == 0);
```

**项目中的应用**：[crates/talos-ipc/](crates/talos-ipc/) 是独立 workspace 成员，不依赖 Bevy，可被 C++ 程序和 Rust 程序同时链接。

### 3.4 serde + TOML（配置反序列化）

```rust
#[derive(Deserialize)]
struct Config {
    #[serde(default)]                    // 字段缺失时用 Default
    physics: PhysicsConfig,
    
    #[serde(default = "default_fps")]    // 字段缺失时调用函数
    fps: f32,
}

fn default_fps() -> f32 { 60.0 }

let config: Config = toml::from_str(&content)?;
```

**项目中的应用**：[config.rs](src/config.rs) 定义所有配置结构体，`SimulationConfig::load()` 从 `config.toml` 反序列化。配合 `notify` crate 监听文件变化，实现热重载。

---

## 第四章：项目结构说明

### 4.1 目录结构

```
bevy_robomaster_simulator-master/
├── Cargo.toml              # 项目清单（依赖、feature、workspace）
├── config.toml             # 运行时配置（热重载）
├── assets/                 # 3D 模型文件（.glb）
│   ├── GROUND.glb
│   ├── vehicle.glb
│   ├── OUTPOST.glb
│   ├── TECH_CORE.glb
│   ├── POWER_RUNE.glb
│   └── CALIB.glb
├── src/
│   ├── main.rs             # 入口：双模式启动（交互/数据集生成）
│   ├── setup.rs            # 场景初始化（地面/相机/灯光/战车）
│   ├── config.rs           # 配置定义 + 热重载
│   ├── handler.rs          # 事件处理（命中/激活）
│   ├── components/         # 领域组件定义
│   │   ├── camera.rs       #   相机相关
│   │   ├── infantry.rs     #   步兵组件
│   │   └── physics.rs      #   碰撞层
│   ├── systems/            # 跨实体业务系统
│   │   ├── input.rs        #   键盘/遥控输入
│   │   ├── camera.rs       #   相机控制
│   │   ├── projectile.rs   #   弹丸发射 + 空气动力学
│   │   ├── uav.rs          #   无人机投放
│   │   ├── chassis_observation.rs  # 底盘观测
│   │   └── debug.rs        #   调试（截图/外观切换）
│   ├── robomaster/         # 机甲业务域
│   │   ├── prelude.rs      #   插件聚合
│   │   ├── common.rs       #   公共类型（Team/RobotConfig）
│   │   ├── visibility.rs   #   外观控制基础设施
│   │   ├── armor/          #   装甲系统
│   │   ├── power_rune/     #   能量机关
│   │   ├── outpost/        #   前哨站
│   │   ├── tech_core/      #   科技核心
│   │   └── vehicle/        #   载具动力学
│   ├── capture/            # GPU 图像采集
│   │   ├── driver.rs       #   GPU→CPU 拷贝
│   │   └── depth.rs        #   深度图采集
│   ├── ros2/               # ROS2 通信
│   │   ├── plugin.rs       #   ROS2Plugin 主插件
│   │   ├── topic.rs        #   话题声明宏
│   │   ├── capture.rs      #   采集上下文
│   │   ├── image.rs        #   图像消息构造
│   │   └── livox.rs        #   Livox 点云
│   ├── talos/              # Talos 共享内存通信
│   │   └── plugin.rs       #   TalosPlugin
│   ├── dataset/            # 数据集生成
│   ├── telemetry/          # 遥测（未接线）
│   ├── util/               # 工具库
│   │   ├── bevy.rs         #   Bevy 工具函数
│   │   ├── derive.rs       #   派生宏
│   │   ├── either.rs       #   Either 枚举
│   │   └── entity_query.rs #   层级查询
│   └── bin/
│       └── talos_gimbal_mock_server.rs  # Mock 服务器
└── crates/                 # 独立子库
    ├── exact/              # 精确长度迭代器收集
    └── talos-ipc/          # 零拷贝共享内存 IPC
```

### 4.2 分层架构

```
L1 入口/装配层    main.rs · bin/talos_gimbal_mock_server
L2 配置层        config.rs（热重载）
L3 域逻辑层      components/ · systems/ · robomaster/ · handler.rs
L4 渲染/采集层   capture/ · setup.rs（双相机：屏幕 + off-screen）
L5 数据/IO层     dataset/ · telemetry/ · ros2/ · talos/
L6 基础设施      util/ · crates/exact · crates/talos-ipc
```

依赖方向单向：上层依赖下层，L3 通过 `robomaster/prelude.rs` 统一对外导出。

### 4.3 数据流

#### 感知数据流（仿真器 → 算法）

```
Bevy 场景实体
  → CaptureSource 相机
  → off-screen 渲染 → GPU→CPU 拷贝
  → ROS2: /image_raw, /camera_info, /tf, *_pose
  → talos: 共享内存图像池 + 位姿三缓冲
```

#### 控制闭环（算法 → 仿真器）

```
自瞄程序发 GimbalCmd
  → 独立线程轮询 → Arc<Mutex> 缓存（只保留最新一条）
  → process_subscription（Update/Last 阶段）
  → 云台 Transform 旋转
  → fire_advice → run_system_once(projectile_launch)
```

#### 数据集流水线

```
遍历 距离×偏航×俯仰 姿态网格
  → 移动相机 → 等待稳定
  → 采集 → 装甲顶点投影 → 遮挡过滤
  → JSON 落盘
```

---

## 第五章：开发环境配置

### 5.1 系统要求

| 项 | 要求 |
|---|---|
| 操作系统 | Linux（推荐 Ubuntu 22.04） |
| Rust 版本 | 2024 edition（rustup 最新 stable） |
| 图形会话 | X11 或 Wayland（winit 必需） |
| GPU | Vulkan 兼容（Intel/NVIDIA/AMD 均可） |

### 5.2 依赖安装

```bash
# Rust 工具链
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# 系统依赖（Ubuntu）
sudo apt install -y \
    libasound2-dev libudev-dev pkg-config \
    libvulkan-dev mesa-vulkan-drivers \
    libx11-dev libwayland-dev libxkbcommon-dev

# ROS2（可选，仅 --features ros2 时需要）
# 参考 https://docs.ros.org/en/installation.html

# ffmpeg（可选，仅 mock server 需要）
sudo apt install libavcodec-dev libavformat-dev libavutil-dev \
                 libswscale-dev libavdevice-dev
```

### 5.3 编译运行

```bash
# 快速语法检查
cargo check

# Debug 运行（编译快，帧率低 ~17 FPS）
DISPLAY=:0 cargo run

# Release 运行（编译慢，帧率高 ~80-150 FPS）← 日常推荐
DISPLAY=:0 cargo run --release

# 关闭默认 talos 采集（不接自瞄时减负）
DISPLAY=:0 cargo run --release --no-default-features

# 启用 ROS2 通道
DISPLAY=:0 cargo run --release --features ros2 --no-default-features

# 数据集自动生成模式
cargo run --release -- --auto-gen

# 调整日志级别
RUST_LOG=daedalus=debug cargo run --release
```

### 5.4 常见环境问题

| 问题 | 原因 | 解决 |
|---|---|---|
| `Failed to build event loop` | 无图形会话（DISPLAY 未设） | `export DISPLAY=:0` |
| 17 FPS 帧率低 | Debug 构建 + substep_count=20 | 用 `--release` + 降 substep |
| WSL 黑屏 | WSL GPU 驱动兼容性 | 项目自动切换兼容渲染 |
| `config.toml not found` | 工作目录不对 | 在项目根目录运行 |
| ROS2 编译失败 | r2r 依赖 ROS2 环境 | 先 `source /opt/ros/humble/setup.bash` |

### 5.5 运行时调试

| 按键 | 功能 |
|---|---|
| F2 | 截图 |
| F3 | 切换视角（自由/跟随/第三人称） |
| F5 | 自瞄订阅开关 |
| Tab | 切换操控战车 |
| Shift+C | 切换装甲外观 |
| 1 | 采集一帧数据集标注 |

config.toml 中开启 `[debug] egui=true` + `inspector=true` 可显示运行时实体检视器。

---

## 第六章：RM 领域知识

### 6.1 机器人类型

| 代号 | 名称 | 特点 |
|---|---|---|
| 1 | 步兵 | 可切换外观（1/2/3 号），常规作战 |
| 2 | 英雄 | 高伤害弹丸 |
| 3 | 工程 | 无武装，负责补给 |
| 4 | 哨兵 | 全自动，固定区域巡逻 |
| 5 | 无人机 | 空中支援，按 P 投放 |
| 6 | 飞镖 | 远程打击 |
| 7 | 雷达 | 侦查 |

### 6.2 装甲系统

- 每个机器人有多个装甲板（前后左右）
- 装甲板上有**灯条**（红/蓝）供视觉识别
- 装甲板有**标记点**（MARKER）供位姿估计
- 装甲板有**贴纸**（数字标签）供分类

**项目中的命名约定**：
```
ARMOR_ROOT            装甲根节点
  ARMOR               碰撞体
  MARKER              灯条标记点
  VERTEX_L/VERTEX_R   碰撞顶点
  L_L/L_R             蓝队灯条
  L_L_RED/L_R_RED     红队灯条
  _C_1/_C_2...        贴纸槽
```

### 6.3 能量机关

- **小机关**：匀速旋转，命中 1 个目标激活
- **大机关**：变速旋转，命中 2 个目标激活
- 目标面有 5 个子目标，每个有 4 种状态：DISABLED → ACTIVE → ACTIVATED → COMPLETED
- 激活后旋转速度变化（大机关切换变速曲线）

### 6.4 科技核心

- 双方各 3 组灯光阵（First/Second/Third）
- 9 个阶段（Step1Started → Step5Completed）
- 每个阶段对应不同的灯光模式：常亮/闪烁/流水/组装
- 调试用 Shift+C 循环切换阶段

### 6.5 前哨站

- 红蓝双方各一个
- 持续匀速旋转（红方顺时针，蓝方逆时针）
- 复用装甲系统，可被命中

---

## 第七章：学习路径建议

### 7.1 按角色分级

#### 仿真使用者（只想跑算法验证）

需要掌握：
- ✅ config.toml 参数调整
- ✅ 编译运行命令（--release / --features）
- ✅ ROS2/talos 通道接入
- ✅ 基本按键操作
- ✅ 看懂报错信息

**不需要**：Rust 语法、Bevy ECS、项目源码

#### 参数调整者（想改参数 + 加配置字段）

额外需要：
- Rust 结构体 + serde 基础
- config.rs 的配置结构定义
- 能读懂系统的参数读取代码

#### 功能开发者（想加新功能/修 bug）

额外需要：
- Rust 所有权/借用/生命周期
- Bevy ECS（组件/系统/资源/插件/观察者）
- 调度阶段与 SystemSet
- 查询过滤（Added/Changed/With/Without）
- 相关领域知识（坐标系/物理/ROS2）

### 7.2 推荐学习资源

| 资源 | 用途 | 优先级 |
|---|---|---|
| [The Rust Book](https://doc.rust-lang.org/book/) | Rust 基础语法 | 开发者必读 |
| [Bevy 官方文档](https://bevyengine.org/learn/) | ECS + 插件系统 | 开发者必读 |
| [Bevy Cheat Sheet](https://bevyengine.org/learn/quick-reference/) | API 速查 | 开发者必读 |
| ROS2 官方教程 | 话题/TF/QoS | ROS2 接入者必读 |
| 项目 README.md | 接入指南 | 全员必读 |
| 项目源码注释 | 深入理解 | 按需查阅 |

### 7.3 带问题学习法

**比无目的通读源码高效 10 倍的做法**：

1. 遇到具体问题（如"弹道为什么偏"）
2. 定位相关代码（projectile.rs）
3. 读懂那段代码的数据流
4. 顺手学习涉及的 Rust/Bevy 概念
5. 修改验证

不要试图从头到尾读一遍源码——那会浪费时间在你暂时用不到的模块上。

---

## 附录：关键概念速查表

| 概念 | 一句话解释 | 项目中的例子 |
|---|---|---|
| Entity | 实体 ID，不存数据 | 一辆战车、一个装甲、一颗弹丸 |
| Component | 挂在实体上的数据 | `Armor`、`Transform`、`Projectile` |
| System | 每帧自动执行的函数 | `vehicle_controls`、`projectile_launch` |
| Resource | 全局单例数据 | `SimulationConfig`、`SubscribeAutoAim` |
| Plugin | 打包好的资源+系统集合 | `RoboMasterPlugins`、`ROS2Plugin` |
| Observer | 事件触发的函数 | `setup_vehicle`（监听场景加载） |
| SystemSet | 系统分组，控制执行顺序 | `GameplaySystems::Input/GameLogic/...` |
| Query | 查询符合条件的实体 | `Query<&Armor, Added<Armor>>` |
| run_if | 条件系统 | `vehicle_controls.run_if(...)` |
| Commands | 延迟执行的命令队列 | `commands.spawn(...)` |
| GlobalTransform | 世界坐标 | 枪口的世界位置 |
| Transform | 局部坐标 | 枪口相对云台的位置 |
| Propagate | 坐标传播（父→子） | `TransformSystems::Propagate` |
| Feature | 编译时开关 | `--features ros2` |
| QoS | ROS2 服务质量策略 | `sensor_data()` 用于图像 |
| GLB | 3D 模型二进制格式 | `GROUND.glb`、`vehicle.glb` |

---

## 第八章：WGPU 渲染管线与 GPU 采集

本章是理解项目"图像采集"功能的关键。仿真器不仅要"画出来给人看"，还要"采出来给算法吃"——这背后是两套独立的渲染管线。

### 8.1 WGPU 渲染后端

WGPU 是 Rust 生态的跨平台图形 API 抽象层，Bevy 用它做渲染。

```
Bevy 渲染层（高级 API）
    ↓
WGPU（跨平台抽象层）
    ↓
不同平台的后端：
  - Linux: Vulkan
  - Windows: DirectX 12
  - macOS: Metal
  - Web: WebGPU
    ↓
GPU 硬件
```

**项目中的应用**：日志里 `backend: Vulkan` 表示 Linux 下走 Vulkan 后端。WGPU 让同一份代码跨平台运行，不用关心具体图形 API。

### 8.2 Bevy 的双 App 架构

这是 Bevy 最容易被新手忽略的设计——**其实有两个 App 在跑**：

```
主 App（Main World）
├─ 所有业务系统（Update / PostUpdate / FixedUpdate）
├─ 业务组件（Armor / Transform / Projectile ...）
├─ 业务资源（SimulationConfig / SubscribeAutoAim ...）
│
└─ ExtractSchedule（桥接阶段）
      ↓ 把主 World 的数据"提取"到渲染 World
      
Render App（Render World）
├─ Render 系统（RenderSet 阶段）
├─ 渲染组件（Mesh / Material / GPU buffer ...）
├─ 渲染资源（RenderDevice / RenderQueue ...）
│
└─ 最终输出到屏幕
```

**为什么要两个 World**：
- 主 World 用业务数据结构（Transform / Armor）
- 渲染 World 用 GPU 数据结构（Buffer / Texture / Pipeline）
- 两者数据结构完全不同，隔离后渲染系统不会污染业务逻辑
- 渲染系统在独立线程跑，不阻塞主线程

**项目中的应用**：[capture/driver.rs](src/capture/driver.rs) 的 GPU 拷贝逻辑跑在 RenderApp 的 `Render` 阶段，而不是主 App 的 Update 阶段。这就是为什么它用 `ExtractSchedule` 把主 World 的相机参数传到渲染 World。

### 8.3 双相机渲染

项目有两个相机，用途不同：

```rust
// 主相机：用户看屏幕用的
Camera3d {
    is_active: true,  // 开采集时设为 false（不在屏幕上渲染）
    target: RenderTarget::Window(window),
}

// 采集相机：给算法喂数据用的
Camera3d {
    target: RenderTarget::Image(image_handle),  // 渲染到纹理，不显示
    is_active: true,
}
```

**为什么主相机会 `is_active: false`**：开启 ROS2/talos 采集后，屏幕预览实际是"把采集纹理 blit 到屏幕"，而不是"重新渲染一次场景"。这样场景只渲染一次，兼顾预览和采集性能。

### 8.4 off-screen 渲染流程

```
① 采集相机配置 target = RenderTarget::Image(handle)
    → GPU 创建一个纹理（Image resource）作为渲染目标
    → 场景渲染到这个纹理而非屏幕

② RenderApp 的 Render 阶段
    → GPU 执行渲染管线，输出像素到纹理

③ ExtractSchedule 把主 World 的"需要拷贝的帧序号"传到渲染 World

④ capture/driver.rs 在 Render 阶段
    → 创建 CPU 端 buffer
    → GPU→CPU 拷贝（map_buffer 异步操作）
    → 通过 channel 发送数据回主线程

⑤ 主线程接收数据
    → ROS2: 封装成 sensor_msgs::Image 发布
    → talos: 写入共享内存
```

**关键概念 `ExtractSchedule`**：主 World 和渲染 World 是隔离的，不能直接访问对方数据。Extract 是"桥梁"——把主 World 需要传递的数据复制到渲染 World。每帧渲染前执行。

### 8.5 深度图采集与 Livox 点云

除了彩色图，项目还采集深度图用于 Livox 激光雷达仿真：

```
深度相机（Depth32Float 格式）
    → 采集深度图（每个像素 = 该点到相机的距离）
    → Livox 采样系统：在相机平面上随机采样点
    → 把每个像素的深度值反投影成 3D 点
    → 生成 PointCloud2 消息发布到 /livox/lidar
```

**反投影公式**：
```
// 像素 (u, v) 的深度为 d
// 相机内参 fx, fy, cx, cy
x = (u - cx) * d / fx
y = (v - cy) * d / fy
z = d
// (x, y, z) 就是该像素在相机坐标系下的 3D 坐标
```

---

## 第九章：Bevy 进阶机制

### 9.1 场景树与坐标传播

Bevy 用 `Parent` / `ChildOf` 组件表达实体间的父子关系，形成场景树。

```
root（底盘，Transform = 位置 A）
  └─ GIMBAL（云台，Transform = 相对底盘的偏移 B）
       └─ SHOT_DIRECTION（枪口，Transform = 相对云台的偏移 C）
```

**两种 Transform**：
- `Transform`：**局部坐标**，相对父节点的位置/旋转/缩放
- `GlobalTransform`：**世界坐标**，相对世界原点的绝对位置/旋转

**坐标传播（Propagate）**：每帧 PostUpdate 阶段，Bevy 自动从根到叶遍历场景树，把父节点的 GlobalTransform × 子节点的 Transform = 子节点的 GlobalTransform。

```
root.GlobalTransform = root.Transform
GIMBAL.GlobalTransform = root.GlobalTransform × GIMBAL.Transform
SHOT_DIRECTION.GlobalTransform = GIMBAL.GlobalTransform × SHOT_DIRECTION.Transform
```

**项目中的应用**：这就是为什么 `projectile_launch` 必须 `.after(TransformSystems::Propagate)`——等坐标传播完，`GlobalTransform` 才是最新的枪口世界位置。否则拿到的是上一帧的位置，弹丸出生点会错（正是 C1 bug 的背景）。

### 9.2 Commands 延迟执行模型

```rust
// 方式 A：Commands（延迟执行）
fn system(mut commands: Commands) {
    commands.spawn((Armor { hp: 100 }, Transform::default()));
    // 这里不会立即创建实体，而是排队等系统跑完才执行
}

// 方式 B：直接查询修改（立即执行）
fn system(mut query: Query<&mut Armor>) {
    for mut armor in &mut query {
        armor.hp -= 10;  // 立即生效
    }
}
```

**为什么要延迟**：ECS 世界在同一时刻只能被一个系统独占修改。如果每个系统都直接改世界，就会冲突。Commands 把修改操作打包成队列，等系统执行完毕、调度器释放世界锁后，统一应用。

**项目中的应用**：
```rust
// 发射弹丸：用 Commands 排队
commands.spawn((
    Transform::IDENTITY.with_translation(muzzle_pos),
    RigidBody::Dynamic,
    Collider::sphere(0.017),
    Projectile { ... },
));

// 自瞄开火：用 Commands.queue 在命令队列里调 run_system_once
commands.queue(|world: &mut World| {
    world.run_system_once(projectile_launch).unwrap();
});
```

### 9.3 资产系统（Asset System）

Bevy 的资产系统管理从磁盘加载的资源（模型/纹理/音频）。

```rust
// 加载资产，返回 Handle（轻量引用）
let handle: Handle<Scene> = asset_server.load("vehicle.glb#Scene0");

// Handle 是克隆友好的，多处共享同一份资产
let handle2 = handle.clone();  // 不重新加载，只增加引用计数

// 资产异步加载
asset_server.load("xxx.glb");  // 立即返回，后台加载
// 加载完成触发 SceneInstanceReady 事件
```

**资产状态**：
- `AssetState::NotLoaded`：未加载
- `AssetState::Loading`：加载中
- `AssetState::Loaded`：加载完成

**项目中的应用**：
```rust
// setup.rs：加载模型
commands.spawn(SceneRoot(asset_server.load("GROUND.glb#Scene0")));

// 观察者监听加载完成事件
app.add_observer(setup_vehicle);  // SceneInstanceReady → setup_vehicle
```

`SceneInstanceReady` 事件就是资产系统在 GLB 加载完成后触发的。这就是为什么构造逻辑用观察者而不是 Startup 系统——必须等模型加载完才能遍历它的子节点挂组件。

### 9.4 反射系统（Reflection）

```rust
#[derive(Reflect)]
struct ProjectileStatistics {
    total: u32,
    accurate: u32,
}

// 注册到类型注册表
app.register_type::<ProjectileStatistics>();

// 运行时通过类型名访问字段
let value: &dyn Reflect = world.resource_ref::<ProjectileStatistics>().unwrap();
let total = value.field("total").unwrap();
```

**用途**：
- Egui Inspector 运行时编辑组件/资源（不注册就看不到）
- 序列化/反序列化（场景保存/加载）
- 跨进程通信（网络同步）

**项目中的应用**：
```rust
.register_type::<ProjectileStatistics>()  // 让 Inspector 能显示命中统计
```

### 9.5 Time 与 FixedUpdate

```rust
// 每帧的 delta time（随帧率变化）
fn system(time: Res<Time>) {
    let dt = time.delta_secs();  // 0.016 (60 FPS) 或 0.007 (144 FPS)
}

// FixedUpdate：固定步长（默认 1/60 秒）
fn system(time: Res<Time<Fixed>>) {
    let dt = time.delta_secs();  // 恒为 1/60
}
```

**为什么物理仿真用 FixedUpdate**：帧率波动时，如果每帧用不同的 dt 算物理，结果不稳定。FixedUpdate 保证步长恒定，物理仿真可复现。

**项目中的应用**：
```rust
// 弹道空气动力学放 FixedUpdate，保证弹道可复现
.add_systems(FixedUpdate, projectile_aerodynamics);
```

### 9.6 States 状态机

```rust
#[derive(States, Clone, Copy, Default, PartialEq, Eq, Hash, Debug)]
enum GameState {
    #[default]
    Loading,
    Playing,
    Paused,
}

app.add_systems(Update, gameplay.run_if(in_state(GameState::Playing)));
```

**项目中的应用**：项目没有用 States 管理全局游戏状态，而是用 `SubscribeAutoAim(AtomicBool)` / `CameraMode` 等资源做开关。这是因为项目是仿真器不是游戏，没有"菜单/游戏中/暂停"这种状态切换需求。

---

## 第十章：Rust 进阶特性

### 10.1 模块系统与 prelude 模式

```rust
// mod 声明：定义子模块
mod armor;
mod power_rune;

// pub mod：公开子模块，外部可访问
pub mod prelude;

// pub use：重新导出
pub use armor::prelude::ArmorPlugins;
```

**prelude 模式**：Rust 社区约定，每个库提供一个 `prelude` 模块，导出最常用的类型/函数。用户只需 `use xxx::prelude::*` 就能导入所有常用项，不用逐个 use。

**项目中的应用**：
```rust
// src/robomaster/prelude.rs
pub use crate::robomaster::visibility::StatefulAppearancePlugin;
pub use armor::prelude::*;
pub use power_rune::prelude::*;
pub use crate::robomaster::outpost::prelude::OutpostPlugins;
pub use crate::robomaster::tech_core::prelude::TechCorePlugins;
```

外部只需 `use crate::robomaster::prelude::RoboMasterPlugins`，不用关心各子模块的内部路径。

### 10.2 迭代器与函数式

```rust
// 命令式
let mut result = Vec::new();
for x in &data {
    if x > 0 {
        result.push(x * 2);
    }
}

// 函数式（Rust 推荐写法）
let result: Vec<_> = data.iter()
    .filter(|&&x| x > 0)
    .map(|&x| x * 2)
    .collect();

// fold：累加
let sum: i32 = data.iter().fold(0, |acc, &x| acc + x);

// 管道链式
let result = data.iter()
    .filter(|x| **x > 0)
    .map(|x| x * 2)
    .fold(0, |acc, x| acc + x);
```

**项目中的应用**：
```rust
// 从场景实体 HashMap 构造名称→实体映射
let name_to_entity: HashMap<&str, Entity> = entities
    .iter()
    .filter_map(|(name, &entity)| name.as_deref().map(|n| (n, entity)))
    .collect();

// 遍历装甲目标，fold 成控制器 HashMap
let controllers = targets.iter().fold(HashMap::new(), |mut acc, target| {
    acc.insert(target.id, build_controller(target));
    acc
});
```

### 10.3 错误处理三种模式

```rust
// 模式 1：Result + ?（传播错误，适合可恢复错误）
fn load_config() -> Result<Config, ConfigError> {
    let content = std::fs::read_to_string("config.toml")?;  // 失败提前返回
    let config: Config = toml::from_str(&content)?;
    Ok(config)
}

// 模式 2：unwrap_or_else（兜底默认值，适合非致命错误）
let mode = present_mode_from_config(&config)
    .unwrap_or_else(|| { warn!("fallback"); PresentMode::AutoNoVsync });

// 模式 3：unwrap / panic（致命错误，不可恢复）
let entity = query.single().expect("必须有且只有一个玩家");
```

**项目中的应用**：
- 配置加载用模式 1 + `unwrap_or_else` 兜底（配置失败用默认值，不崩溃）
- 资源初始化用模式 2（缺字段用默认值）
- 系统参数查询用模式 3（`Single` 匹配多个实体说明逻辑有 bug，应该 panic）

### 10.4 智能指针与内部可变性

```rust
// Box<T>：堆分配，独占所有权
let b = Box::new(42);

// Rc<T>：引用计数，单线程共享
let r = Rc::new(42);

// Arc<T>：原子引用计数，多线程共享
let a = Arc::new(42);

// Mutex<T> / RwLock<T>：线程安全可变
let m = Arc::new(Mutex::new(42));
{
    let mut guard = m.lock().unwrap();
    *guard += 1;
}

// Arc<Mutex<T>>：多线程共享 + 可变（项目常用模式）
```

**项目中的应用**：ROS2/talos 接收线程和主线程共享数据：
```rust
// 接收线程写入
let latest_cmd = Arc::new(Mutex::new(None::<GimbalCmd>);
// 接收线程
latest_cmd.lock().unwrap() = Some(cmd);
// 主线程读取
let cmd = latest_cmd.lock().unwrap().take();
```

### 10.5 trait object vs 泛型

```rust
// 泛型（静态分发，零开销，编译期单态化）
fn process<T: Control>(item: &T) {
    item.set(state);  // 编译期确定具体类型，内联优化
}

// trait object（动态分发，有虚函数表开销）
fn process(item: &dyn Control) {
    item.set(state);  // 运行时查虚函数表
}

// Box<dyn Trait>：堆分配的 trait object
let controllers: Vec<Box<dyn Control>> = vec![...];
```

**项目中的应用**：`Controller` 是枚举而非 trait object，因为变体数量固定（Material/Visibility/Combined）。Rust 中枚举比 trait object 更常用，因为：
- 无虚函数开销
- 模式匹配比动态分发更安全（编译器检查完备性）
- 数据和逻辑在一起，更易读

### 10.6 unsafe 与 FFI

```rust
unsafe {
    // 解引用裸指针
    let ptr: *const i32 = &42;
    println!("{}", *ptr);
    
    // 调用 C 函数
    extern "C" { fn abs(x: i32) -> i32; }
    println!("{}", abs(-5));
}
```

**项目中的应用**：[crates/talos-ipc/](crates/talos-ipc/) 用 `#[repr(C, align(64))]` 保证内存布局与 C++ 兼容，共享内存的读写本身不依赖 unsafe（memmap2 封装了），但布局约定是 FFI 兼容的关键：

```rust
#[repr(C, align(64))]  // C 布局 + 64 字节对齐
struct ImageFrame {
    seq: u64,
    timestamp_ns: u64,
    data: [u8; IMAGE_SIZE],
}

// 编译期断言：锁死字段偏移，防止版本升级改变布局
const _: () = assert!(std::mem::offset_of!(ImageFrame, seq) == 0);
```

---

## 第十一章：项目特有设计模式

### 11.1 声明式宏体系

项目大量使用声明式宏消除样板代码，这是最显著的设计风格。

#### `entity_root!` 宏：场景树匹配

```rust
// 使用：按名称模式匹配场景节点挂组件
entity_root!(root_query, world, children, |entity, name| {
    if name.contains("ARMOR_ROOT") {
        process_armor_root(entity, ...);
    }
});
```

**展开后**大致是：遍历 `children`，对每个实体取 `Name` 组件，调用闭包判断是否匹配。省去了手写递归遍历的样板。

#### `material!` / `visibility!` 宏：外观控制器

```rust
// 一行声明：激活中和已激活阶段发光
material!(on = {activated, activating})

// 展开为
Controller::Material(
    entity,
    [muted, muted, original, original],  // 4 个阶段对应的材质
)
```

#### `topic!` 宏：ROS2 话题声明

```rust
// 一行声明一个话题
topic!(image_publisher, "/image_raw", sensor_msgs::msg::Image, default);
```

**展开后**生成：publisher 句柄 + 注册函数 + 类型检查。增删话题只需改一行。

#### `plugin_group!` 宏：插件聚合

```rust
plugin_group!(RoboMasterPlugins {
    StatefulAppearancePlugin,
    ArmorPlugins,
    PowerRunePlugins,
    OutpostPlugins,
    TechCorePlugins,
});
```

**展开后**生成 `Plugin` trait 的 `build` 实现，依次 `add_plugins` 所有子插件。

### 11.2 SystemParam 聚合模式

```rust
// 问题：系统需要多个查询/资源，参数列表很长
fn system(
    materials: ResMut<Assets<StandardMaterial>>,
    cache: ResMut<MaterialCache>,
    mesh_materials: Query<&mut MeshMaterial3d>,
    visibilities: Query<&mut Visibility>,
) { ... }

// 解决：用 SystemParam 打包
#[derive(SystemParam)]
struct StatefulAppearance<'w, 's> {
    materials: ResMut<'w, Assets<StandardMaterial>>,
    cache: ResMut<'w, MaterialCache>,
    mesh_materials: Query<'w, 's, &'static mut MeshMaterial3d>,
    visibilities: Query<'w, 's, &'static mut Visibility>,
}

// 系统参数简化
fn system(mut appearance: StatefulAppearance) { ... }
```

**优势**：
- 参数列表简洁
- 多个系统复用同一组参数
- 避免重复借用冲突（SystemParam 内部用 `ParamSet` 解决）

### 11.3 状态机封装模式

```rust
// 状态机外部包装，隔离内部状态
pub struct PowerRuneMechanism {
    state: MechanismState,  // 内部状态
}

impl PowerRuneMechanism {
    pub fn state(&self) -> &MechanismState { &self.state }
    pub fn state_mut(&mut self) -> &mut MechanismState { &mut self.state }
}

// 内部状态机（封装具体状态转换）
struct MechanismState { ... }
impl MechanismState {
    fn tick(&mut self, dt: f32, rng: &mut impl Rng) { ... }
    fn hit(&mut self, target: usize, rng: &mut impl Rng) -> RuneHitOutcome { ... }
}
```

**设计思想**：外部只看到 `PowerRuneMechanism`，不关心内部状态如何转换。状态机的复杂性被封装，业务代码只调 `tick()` / `hit()` / `state()`。

### 11.4 懒初始化模式

```rust
// 首次使用时才创建，后续复用
fn ensure_muted(&mut self, handle, materials) -> Handle<StandardMaterial> {
    if let Some(existing) = self.muted.get(&id) {
        return existing.clone();  // 缓存命中
    }
    // 未命中：创建并缓存
    let muted = materials.add(original.clone().with_emissive(BLACK));
    self.muted.insert(id, muted.clone());
    muted
}
```

**项目中的应用**：
- `MaterialCache`：懒生成熄灭材质
- `TechCoreMaterialHandles`：首帧才创建 5 种颜色材质句柄
- `AssetServer::load`：异步加载，加载完触发事件

### 11.5 命名约定驱动开发

这是项目最核心的设计哲学：**程序逻辑由模型命名约定驱动**。

```
美术在 Blender 建模时按约定命名
    ↓
程序按命名查找节点
    ↓
挂载对应业务组件
```

**约定示例**：
```
FACE_<index>            → 能量机关目标面（index 编码队伍+大小机关）
ARMOR_ROOT              → 装甲根节点
*_ROTATE                → 前哨站旋转节点
L_L_RED / L_R_RED       → 红队灯条
*_C_1 / *_C_2           → 贴纸槽
```

**优势**：换模型不用改代码，只要新模型遵守命名约定。

**劣势**：命名约定是隐性契约，没有编译期检查。模型改名但忘了改代码 → 运行时找不到节点 → 静默失败。

---

## 第十二章：跨线程与异步通信

### 12.1 Bevy 的线程模型

```
主线程（Main）
├─ ECS 调度器
│   ├─ Update 系统串行执行
│   └─ 大部分系统在这里
│
└─ Render 线程（RenderApp）
    └─ 渲染系统独立执行

外部线程
├─ ROS2 spin 线程（r2r 内部）
├─ talos 接收线程
└─ 配置热重载监听线程（notify）
```

### 12.2 Arc<Mutex> + AtomicBool 模式

跨线程共享数据的标准模式：

```rust
// 共享一个可变数据
let latest_cmd = Arc::new(Mutex::new(None::<GimbalCmd>));

// 共享一个布尔标志
let auto_aim_enabled = Arc::new(AtomicBool::new(false));

// 线程 A 写入
{
    let mut guard = latest_cmd.lock().unwrap();
    *guard = Some(cmd);
}

// 线程 B 读取
let cmd = latest_cmd.lock().unwrap().take();  // 读出并清空

// 原子布尔操作（无锁）
auto_aim_enabled.store(true, Ordering::Relaxed);  // 写
let enabled = auto_aim_enabled.load(Ordering::Relaxed);  // 读
```

**项目中的应用**：
- `SubscribeAutoAim(AtomicBool)`：F5 按键在主线程改，ROS2 接收线程读
- `Arc<Mutex<Option<GimbalCmd>>>`：ROS2 线程写入最新指令，主线程 `process_subscription` 读取

### 12.3 crossbeam-channel 跨线程消息传递

```rust
use crossbeam_channel::{unbounded, Receiver, Sender};

// 创建无界通道
let (tx, rx) = unbounded::<Event>();

// 线程 A 发送
tx.send(Event::ConfigChanged)?;

// 线程 B 接收（非阻塞）
while let Ok(event) = rx.try_recv() {
    handle(event);
}
```

**项目中的应用**：配置热重载
```rust
// notify 线程监听文件变化
watcher.receiver.send(ConfigEvent::Modified);

// 主线程 Update 阶段非阻塞消费
while let Ok(Ok(event)) = watcher.receiver.try_recv() {
    if event.kind.is_modify() {
        match SimulationConfig::load() { ... }
    }
}
```

### 12.4 ROS2 异步回调到主线程

r2r 内部用异步 runtime（tokio），回调在异步线程触发。但 Bevy 的 ECS 世界不能跨线程访问。解决模式：

```
ROS2 线程（异步）
  ↓ 收到 GimbalCmd
  ↓ 写入 Arc<Mutex<Option<GimbalCmd>>>
  ↓
主线程 Update 阶段
  ↓ process_subscription 系统
  ↓ lock().take() 读出最新指令
  ↓ 通过 Commands 修改云台 Transform
```

**关键**：ROS2 线程**绝不直接访问 ECS 世界**，只写共享内存。主线程系统才读取并应用。

---

## 第十三章：实战代码走读

### 13.1 从按键到弹丸发射的完整链路

以"按空格发射弹丸"为例，跟踪一帧内的完整执行顺序：

```
帧开始
  │
  ├─【First 阶段】
  │   └─ 初始化帧计时
  │
  ├─【Update 阶段】（按 SystemSet 链式执行）
  │   ├─ Input 集合
  │   │   └─ vehicle_controls：WASD 改底盘 Transform
  │   │   └─ gimbal_controls：方向键改云台 Transform（局部旋转）
  │   │       ↳ 此时云台 Transform 变了，但 GlobalTransform 还没更新
  │   │
  │   ├─ GameLogic 集合
  │   │   └─ change_appearance / update_help_text
  │   │
  │   ├─ Camera 集合
  │   │   └─ update_camera_follow：跟随相机位置更新
  │   │
  │   └─ Cleanup 集合
  │       └─ cleanup_projectiles：清理过期弹丸
  │
  ├─【PostUpdate 阶段】
  │   ├─ TransformSystems::Propagate
  │   │   └─ Bevy 自动遍历场景树
  │   │   └─ 父 GlobalTransform × 子 Transform = 子 GlobalTransform
  │   │   └─ 此时枪口的 GlobalTransform 是最新的世界坐标
  │   │
  │   ├─ projectile_launch（.after(Propagate).run_if(空格按下)）
  │   │   ├─ 读取 launch_offset 的 GlobalTransform → 枪口世界位置
  │   │   ├─ 计算发射方向（云台旋转 × 枪管朝向）
  │   │   ├─ 读取 config.projectile.speed → 弹丸初速
  │   │   ├─ commands.spawn() 排队创建弹丸实体
  │   │   │   ↳ 弹丸 = Transform + RigidBody::Dynamic + Collider + Projectile
  │   │   └─ stats.increase_total() → 命中统计 +1
  │   │
  │   └─ update_chassis_observation（.after(Propagate)）
  │       └─ 采集底盘 IMU/轮速观测数据
  │
  ├─【FixedUpdate 阶段】（固定步长，可能一帧执行多次或零次）
  │   └─ projectile_aerodynamics
  │       ├─ 对每个弹丸：v += g * dt（重力）
  │       ├─ v *= (1 - drag * dt)（风阻）
  │       └─ pos += v * dt（位置积分）
  │
  ├─【Last 阶段】
  │   └─ Commands 队列应用（弹丸实体真正创建）
  │
  └─【Render 阶段】（RenderApp 线程）
      ├─ ExtractSchedule：提取相机/网格数据到渲染 World
      ├─ Render：GPU 执行渲染管线
      └─ 画面输出到屏幕 / 采集纹理
```

### 13.2 从 GLB 加载到装甲可命中的完整链路

```
① setup.rs 启动时
   commands.spawn(SceneRoot(asset_server.load("vehicle.glb#Scene0")));

② Bevy 资产系统后台加载 vehicle.glb
   解析 GLB → 构建场景树（实体 + Name 组件 + Mesh + Material）
   加载完成 → 触发 SceneInstanceReady 事件

③ setup_vehicle 观察者被触发
   按名称匹配 "BASE" / "GIMBAL" / "SHOT_DIRECTION" 节点
   挂载 Infantry / InfantryGimbal / InfantryLaunchOffset 组件
   挂载 ScanArmor 组件（触发装甲扫描）

④ ArmorConstructorPlugin 的 insert 系统（下一帧 Update）
   检测到 Added<ScanArmor>
   遍历后代找 ARMOR_ROOT 节点
   对每个 ARMOR_ROOT：
     ├─ 挂 ColliderConstructorHierarchy（碰撞体）
     ├─ 挂 Armor 组件（team/spec/label）
     ├─ 按队伍保留灯条（L_L_RED 或 L_L），despawn 另一组
     ├─ 从 MARKER 网格提取顶点 → MarkerData
     ├─ 从 VERTEX_L/R 网格提取顶点 → VertexData
     └─ 处理贴纸 _C_N → ArmorSticker + Visibility

⑤ avian3d 物理引擎
   检测到 ColliderConstructorHierarchy
   自动从网格生成碰撞体
   注册到碰撞检测系统

⑥ 游戏循环中
   弹丸实体带 Collider + CollisionEventsEnabled
   弹丸飞行时与装甲碰撞体发生碰撞
   avian3d 触发 CollisionEnd 事件

⑦ handle_armor_collision 观察者
   从碰撞双方识别弹丸和装甲
   查 armor 实体是否有 Armor 组件
   命中 → 移除弹丸的 CollisionEventsEnabled（防重复）
   命中 → stats.increase_accurate()（命中统计 +1）
```

### 13.3 从算法指令到云台转动的完整链路

```
① 外部自瞄程序发布 GimbalCmd 到 /rm_gimbal/cmd

② ROS2 spin 线程（r2r 内部）收到消息
   回调写入 Arc<Mutex<Option<GimbalCmd>>>
   ↳ 不直接访问 ECS 世界

③ 主线程 Update 阶段
   process_subscription 系统执行（.run_if(SubscribeAutoAim)）
   ├─ lock().take() 读出最新指令
   ├─ 检查 distance == -1（无效指令跳过）
   ├─ 解析 yaw/pitch 构造 expected_rotation（四元数）
   ├─ 计算云台局部旋转（parent⁻¹ × expected）
   ├─ commands.get_entity(gimbal_entity) → 修改 Transform.rotation
   └─ 如果 fire_advice == true：
       commands.queue(|world| world.run_system_once(projectile_launch))

④ PostUpdate 阶段
   TransformSystems::Propagate 更新 GlobalTransform
   云台和枪口的世界坐标更新

⑤ 下一帧 Render 阶段
   相机跟随新的云台位置
   屏幕画面更新
```

---

## 第十四章：调试与排查指南

### 14.1 日志级别与过滤

```bash
# 只看项目自身日志（过滤掉 Bevy/依赖库的日志）
RUST_LOG=daedalus=debug cargo run --release

# 看特定模块
RUST_LOG=daedalus::ros2=trace cargo run --release

# 全部 debug
RUST_LOG=debug cargo run --release

# 关闭所有日志（只留 panic）
RUST_LOG=off cargo run --release
```

**日志级别**：`error > warn > info > debug > trace`，设成 `debug` 就能看到 `debug` 及以上所有日志。

### 14.2 常见问题排查清单

#### 编译问题

| 报错 | 原因 | 解决 |
|---|---|---|
| `cannot borrow as mutable` | 同一帧多个系统修改同一资源 | 用 `Single<&mut T>` 或 `ParamSet` |
| `no rules expected #` | 宏内用 `///` 文档注释 | 改用宏外注释，或 `//` |
| `unused import` | 引入了没用到的类型 | 删除 import，或加 `#[allow(unused_imports)]` |
| `feature not found` | Cargo.toml 没定义该 feature | 检查 `[features]` 节 |

#### 运行时问题

| 现象 | 排查方向 |
|---|---|
| 窗口启动 panic | 检查 `DISPLAY` 环境变量 |
| 帧率极低 | 确认是否 `--release`，检查 `substep_count` |
| 模型不显示 | 检查 GLB 文件是否在 assets/ 目录 |
| 装甲不被命中 | 检查碰撞体是否生成（用 egui inspector） |
| 自瞄不生效 | 检查 F5 是否开启、topic 名是否匹配 |
| 弹道偏移 | 检查 launch_offset 用的是 GlobalTransform 还是 Transform |

### 14.3 Egui Inspector 调试

```toml
# config.toml
[debug]
egui = true
inspector = true
```

开启后运行时弹出窗口，可以：
- 查看所有实体及其组件
- 实时编辑组件值（Transform / 配置）
- 检查资源（SimulationConfig / ProjectileStatistics）

**注意**：Inspector 会显著降低帧率，调试完关闭。

### 14.4 截图与数据采集调试

- `F2`：截图保存为 `screenshot-N.png`，验证渲染输出
- `1`：采集一帧数据集标注，验证装甲顶点投影是否正确
- `F3`：切到 Robot 第一视角，对照准星和弹丸落点验证枪口对齐

---

## 附录 B：推荐学习顺序

### 第一周：环境与运行

1. 安装 Rust + 系统依赖
2. `cargo run --release` 跑起来
3. 熟悉按键操作（WASD / F2 / F3 / F5）
4. 改 config.toml 参数，观察效果
5. 读 README.md 和本文档第一/五/六章

### 第二周：Rust 基础

1. 读 The Rust Book 第 1-10 章（基础语法 + 所有权 + 错误处理）
2. 读本文档第一章（对照项目代码理解 Rust 概念）
3. 能看懂 config.rs 的结构体定义
4. 能改 config 字段并接系统读取

### 第三周：Bevy ECS 基础

1. 读 Bevy 官方文档 ECS 部分
2. 读本文档第二章
3. 跟踪 main.rs 的插件挂载和系统注册
4. 理解 Update / PostUpdate / FixedUpdate 的区别

### 第四周：项目代码走读

1. 跟着本文档第十三章走读三条完整链路
2. 读懂 setup.rs → RoboMasterPlugins 的初始化流程
3. 读懂 projectile.rs 的弹丸发射逻辑
4. 读懂 ros2/plugin.rs 或 talos/plugin.rs 的通信闭环

### 第五周以后：按需深入

- 想改渲染 → 第八章（WGPU + 采集）
- 想加新机器人 → 第六章（RM 规则）+ armor/ 构造代码
- 想加新话题 → 第十一章（宏体系）+ ros2/topic.rs
- 想修 bug → CODE_ANALYSIS_REPORT.md + 对应模块源码

---

## 附录 C：术语对照表

| 术语 | 英文 | 解释 |
|---|---|---|
| 实体 | Entity | ECS 中的 ID，不存数据 |
| 组件 | Component | 挂在实体上的数据片段 |
| 系统 | System | 每帧自动执行的函数 |
| 资源 | Resource | 全局单例数据 |
| 插件 | Plugin | 打包好的资源+系统集合 |
| 观察者 | Observer | 事件触发的函数 |
| 世界 | World | 所有实体+资源的容器 |
| 调度器 | Scheduler | 决定系统执行顺序 |
| 查询 | Query | 查找符合条件的实体 |
| 借用 | Borrow | 临时引用数据，不获取所有权 |
| 生命周期 | Lifetime | 引用的有效期 |
| trait | Trait | Rust 的接口概念 |
| 泛型 | Generic | 类型参数化 |
| 枚举 | Enum | 可取多个变体之一的类型 |
| 模式匹配 | Pattern Matching | 根据枚举变体分支处理 |
| 所有权 | Ownership | Rust 的内存管理机制 |
| 四元数 | Quaternion | 表示 3D 旋转的数学结构 |
| 欧拉角 | Euler Angle | 用 yaw/pitch/roll 表示旋转 |
| 坐标传播 | Transform Propagation | 父→子坐标计算 |
| 场景树 | Scene Tree | 父子实体层级关系 |
| 资产 | Asset | 从磁盘加载的资源 |
| 句柄 | Handle | 资产的轻量引用 |
| 反射 | Reflection | 运行时类型信息 |
| 特性 | Feature | 编译时开关 |
| 宏 | Macro | 代码生成代码 |
| FFI | Foreign Function Interface | 与 C 互操作 |
| ABI | Application Binary Interface | 二进制接口约定 |

---

## 第十五章：项目技术栈逐项对照（必读）

本章基于全项目代码精确扫描，把"**这个项目实际用到的技术**"和"**相关但项目没用到的扩展知识**"严格分开。新手只学左列就够用，右列是进阶扩展。

### 15.1 Rust 语言特性对照

#### ✅ 项目实际用到的（学这些就够）

| 特性 | 用在哪 | 代码位置 | 为什么用 |
|---|---|---|---|
| **所有权与借用** | 所有 ECS 系统参数 | 全项目 | `Query<&T>` 共享读，`Query<&mut T>` 独占写 |
| **生命周期 `'w 's`** | SystemParam 定义 | [visibility.rs](src/robomaster/visibility.rs) `StatefulAppearance<'w, 's>` | Bevy SystemParam 标准写法 |
| **trait + 泛型约束** | 旋转控制器、随机数 | `impl Rng` 约束、`RotationController` | 接口抽象 |
| **枚举 + 模式匹配** | 所有状态机 | `Activation`、`Team`、`TechCorePhase`、`PowerRuneMode` | RM 规则的状态表达 |
| **声明式宏 `macro_rules!`** | 场景匹配/外观控制/话题声明 | `entity_root!`、`material!`、`topic!`、`plugin_group!` | 消除样板代码 |
| **派生宏 `#[derive]`** | 组件/配置/反射 | `#[derive(Component)]`、`#[derive(Deserialize)]`、`#[derive(Reflect)]`、`#[derive(SystemParam)]` | 自动生成代码 |
| **条件编译 `#[cfg]`** | 通信通道开关 | `#[cfg(feature = "ros2")]`、`#[cfg(feature = "talos")]` | 编译时裁剪功能 |
| **错误处理 `Result + ?`** | 配置加载、IO | [config.rs:319](src/config.rs) `load()` | 可恢复错误传播 |
| **错误兜底 `unwrap_or_else`** | 配置解析兜底 | [main.rs](src/main.rs) `present_mode` 解析 | 非致命错误用默认值 |
| **模块系统 + prelude** | 整个项目组织 | `mod`/`pub mod`/`pub use` | 代码组织 + 统一导出 |
| **迭代器链式** | 数据转换 | `filter`/`map`/`fold`/`collect`/`filter_map` | 函数式数据处理 |
| **`Arc<Mutex<T>>`** | 跨线程共享指令 | ROS2/talos 接收线程 ↔ 主线程 | 多线程安全共享可变数据 |
| **`AtomicBool`** | 自瞄开关 | `SubscribeAutoAim(AtomicBool)` | 无锁跨线程布尔标志 |
| **`crossbeam-channel`** | 配置热重载 | notify 线程 → 主线程 | 跨线程消息传递 |
| **`Box<dyn Trait>`** | GPU 采集/遥测导出 | [capture/driver.rs:57](src/capture/driver.rs) `Box<dyn GpuCaptureHandler>` | 运行时多态（变体不固定时） |
| **`RefCell<T>`** | ROS2 时间戳 | [ros2/capture.rs:44](src/ros2/capture.rs) `stamp: RefCell<...>` | 单线程内部可变性（r2r 回调） |
| **`async/await`** | ROS2 话题、GPU 拷贝 | [ros2/topic.rs:75](src/ros2/topic.rs)、[capture/driver.rs:370](src/capture/driver.rs) | 异步 IO（r2r/tokio） |
| **`unsafe` 块** | 共享内存 IPC | [crates/talos-ipc/src/shm.rs:108](crates/talos-ipc/src/shm.rs) `MmapMut::map_mut` | 裸指针操作（仅 talos-ipc） |
| **`unsafe impl Send/Sync`** | 共享内存跨线程 | [crates/talos-ipc/src/shm.rs:70](crates/talos-ipc/src/shm.rs) | 让裸指针类型跨线程 |
| **`#[repr(C, align)]`** | IPC 内存布局 | [crates/talos-ipc/src/layout.rs](crates/talos-ipc/src/layout.rs) | C-ABI 兼容 |
| **`const _: () = assert!(...)`** | 编译期布局检查 | [crates/talos-ipc/src/layout.rs](crates/talos-ipc/src/layout.rs) | 锁死字段偏移 |
| **闭包** | 宏展开、回调 | `entity_root!` 闭包参数、`commands.queue(\|world\| ...)` | 延迟执行回调 |
| **`Local<T>`** | 系统局部状态 | [debug.rs:91](src/systems/debug.rs) 截图计数、[uav.rs:35](src/systems/uav.rs) 冷却计时 | 每个系统实例独有 |

#### ❌ 项目没用到的（扩展阅读，不用学）

| 特性 | 为什么没用 | 什么时候需要学 |
|---|---|---|
| **`Rc<T>`** | 项目用 `Arc<T>`（多线程场景） | 写单线程应用时 |
| **`Pin<T>` / `Unpin`** | 项目不自己构造自引用类型 | 写底层异步 runtime 时 |
| **自己写过程宏** | 项目用声明式宏够了 | 写 `#[derive]` 库时 |
| **GATs（泛型关联类型）** | 项目 trait 不需要 | 写高级 trait 库时 |
| **`unsafe` 裸指针算术** | 主项目不用，仅 talos-ipc 用 memmap2 封装 | 写内存分配器时 |
| **`async fn` in trait** | 项目 trait 不异步 | Rust 1.75+ 的新特性 |
| **trait object 的关联类型 | 项目 `dyn Trait` 都是无关联类型的简单 trait | 设计复杂 trait 层级时 |

---

### 15.2 Bevy 引擎机制对照

#### ✅ 项目实际用到的（学这些就够）

| 机制 | 用在哪 | 代码位置 | 为什么用 |
|---|---|---|---|
| **Entity / Component** | 所有业务实体 | `Armor`、`Transform`、`Projectile`、`Infantry` | ECS 基础三要素 |
| **System** | 每帧逻辑 | `vehicle_controls`、`projectile_launch`、`update_camera_follow` | 每帧自动执行 |
| **Resource** | 全局单例 | `SimulationConfig`、`SubscribeAutoAim`、`ProjectileCooldown` | 全局共享数据 |
| **Plugin / PluginGroup** | 模块装配 | `RoboMasterPlugins`、`ROS2Plugin`、`TalosPlugin` | 打包功能模块 |
| **SystemSet + `.chain()`** | Update 阶段排序 | `GameplaySystems::Input/GameLogic/Camera/Cleanup` | 强制系统串行 |
| **Observer + Event** | 事件驱动 | `SceneInstanceReady`→`setup_vehicle`、`CollisionEnd`→`handle_armor_collision` | 被动触发逻辑 |
| **Query + 过滤器** | 实体查询 | `Added<ScanArmor>`、`Changed<ArmorStickerSelection>`、`With<Controlled>` | 按需查询 |
| **`Single<&mut T>`** | 独占实体 | `projectile_launch` 的 `launch_offset: Single<...>` | 确保唯一匹配 |
| **`run_if`** | 条件系统 | `vehicle_controls.run_if(\|mode\| mode.0 != Free)` | 按状态启用系统 |
| **Commands（延迟执行）** | 实体创建/修改 | `commands.spawn(...)`、`commands.queue(...)` | 避免借用冲突 |
| **Transform / GlobalTransform** | 坐标系统 | 所有有位置实体 | 局部 vs 世界坐标 |
| **`TransformSystems::Propagate`** | 坐标传播时序 | `projectile_launch.after(Propagate)` | 等坐标更新完再读 |
| **AssetServer + Handle** | 资产加载 | `asset_server.load("vehicle.glb#Scene0")` | 异步加载模型 |
| **SceneRoot + SceneInstanceReady** | 场景构造 | [setup.rs](src/setup.rs) spawn SceneRoot | GLB 加载完触发构造 |
| **Time / Time<Fixed>** | 时间 | `time.delta_secs()` | 帧时间 |
| **FixedUpdate** | 物理仿真 | `projectile_aerodynamics` | 固定步长保证可复现 |
| **`#[derive(SystemParam)]`** | 参数聚合 | `StatefulAppearance`、`ArmorConstructor` | 简化系统参数 |
| **Reflect + register_type** | 运行时反射 | `register_type::<ProjectileStatistics>()` | Egui Inspector 可见 |
| **Camera3d + RenderTarget** | 双相机 | 主相机→Window，采集相机→Image | 屏幕 + 采集 |
| **ChildOf 查询** | 场景树遍历 | [armor/construct.rs:155](src/robomaster/armor/construct.rs) `Query<&ChildOf>` | 找父节点/祖先 |
| **PostUpdate** | 坐标传播后 | `projectile_launch`、`update_chassis_observation` | 拿最新世界坐标 |
| **ExtractSchedule** | 主→渲染 World 桥接 | [capture/driver.rs](src/capture/driver.rs) | GPU 采集数据传递 |

#### ❌ 项目没用到的（扩展阅读）

| 机制 | 为什么没用 | 什么时候需要学 |
|---|---|---|
| **States 状态机** | 仿真器没有"菜单/游戏中/暂停"切换，用资源做开关 | 做有关卡/菜单的游戏时 |
| **EventReader / EventWriter** | 项目全用 Observer（更现代的 API） | Bevy 0.13 之前的旧写法 |
| **AudioSink / PlaybackSettings** | 项目加载了 bevy_audio 但代码里没播放逻辑 | 需要背景音乐/音效时 |
| **AnimationPlayer / AnimationGraph** | 项目不用骨骼动画（模型是静态的） | 做角色动画时 |
| **bevy_ui（Node/TextBundle）** | 项目用 egui 做 UI | 做游戏内 HUD 时 |
| **DynamicScene（场景保存）** | 项目不需要运行时保存场景 | 做关卡编辑器时 |
| **NonSend / NonSendMut** | 项目没有非 Send 资源 | 用 C 库的句柄时 |
| **ParallelIterator / par_iter** | 项目数据量小，普通迭代够 | 大规模实体并行处理时 |
| **Bevy 网络同步** | 项目用 ROS2/talos 做通信 | 做多人游戏时 |

---

### 15.3 依赖库使用对照

#### ✅ 项目实际用到的库

| 库 | 版本 | 用在哪 | 用途 |
|---|---|---|---|
| **bevy** | 0.18 | 全项目 | ECS + 渲染 + 窗口 + 输入 + 资产 |
| **avian3d** | 0.6 | 物理仿真 | 刚体动力学 + 碰撞检测 |
| **r2r** | feature 门控 | [ros2/](src/ros2/) | ROS2 客户端 |
| **serde + toml** | — | [config.rs](src/config.rs) | 配置文件反序列化 |
| **clap** | 4.6 | [main.rs](src/main.rs) | 命令行参数解析 |
| **notify** | — | [config.rs](src/config.rs) | 文件变化监听（热重载） |
| **crossbeam-channel** | — | [config.rs](src/config.rs) | 跨线程消息传递 |
| **bevy-inspector-egui** | 0.36 | 调试 UI | 运行时实体检视器 |
| **image** | — | [ros2/image.rs](src/ros2/image.rs)、[dataset/writer.rs](src/dataset/writer.rs) | JPEG 压缩 + 数据集写图 |
| **rand** | — | [power_rune/](src/robomaster/power_rune/) | 随机选目标/方向 |
| **memmap2** | — | [crates/talos-ipc/](crates/talos-ipc/) | 共享内存映射 |
| **ffmpeg-next** | feature 门控 | [bin/talos_gimbal_mock_server.rs](src/bin/talos_gimbal_mock_server.rs) | mock server 视频输入 |
| **bevy_transform_interpolation** | 0.4 | 插值 | Transform 平滑插值 |
| **glam** | 通过 bevy | 全项目 | 数学（Vec3/Quat/Mat3） |

#### ❌ 项目没用到的常见库（扩展）

| 库 | 用途 | 什么时候需要 |
|---|---|---|
| **tokio** | 异步 runtime | r2r 内部用，项目代码不直接用 |
| **rayon** | 数据并行 | 大规模并行计算时 |
| **anyhow** | 错误处理 | 项目用 `Box<dyn Error>` 够了 |
| **tracing** | 结构化日志 | 项目用 `log` 宏够 |
| **serde_json** | JSON 序列化 | 数据集用自定义写入 |

---

### 15.4 数学知识对照

#### ✅ 项目实际用到的

| 知识点 | 用在哪 | 代码位置 |
|---|---|---|
| **四元数 `Quat`** | 所有旋转 | `Quat::from_euler`、`q.inverse()`、`q1 * q2` |
| **欧拉角 `EulerRot::YXZ`** | 云台指令解析 | [ros2/plugin.rs:446](src/ros2/plugin.rs) |
| **坐标系转换 Y-up↔Z-up** | Bevy↔ROS | `M_ALIGN_MAT3` 对齐矩阵 |
| **向量运算 `Vec3`** | 位置/速度/方向 | `vec.normalize()`、`vec.length()`、`dot`/`cross` |
| **矩阵乘法 `Mat3`** | 坐标系对齐 | [talos/plugin.rs](src/talos/plugin.rs) |
| **相机内参** | Livox 点云反投影 | `fx/fy/cx/cy` + 深度→3D 点 |

#### ❌ 没用到的（扩展）

| 知识点 | 什么时候需要 |
|---|---|
| 四元数球面插值 slerp | 平滑旋转过渡 |
| 卡尔曼滤波 | 传感器融合 |
| 李群/李代数 | 高级 SLAM |
| 微分方程数值解 | 复杂物理仿真 |

---

### 15.5 学习优先级总结

**新手只学"用到"列就能理解整个项目**。按优先级：

```
第 1 优先（1-2 周）：
  ├─ Rust 基础：所有权/借用/生命周期/枚举/模式匹配/错误处理
  ├─ Bevy 基础：Entity/Component/System/Resource/Plugin/Query
  └─ 能看懂 config.rs + main.rs 的装配逻辑

第 2 优先（2-3 周）：
  ├─ Bevy 进阶：Observer/SystemSet/Commands/Transform 传播
  ├─ Rust 进阶：迭代器/闭包/泛型/trait
  └─ 能跟踪 projectile.rs 的弹丸发射完整链路

第 3 优先（3-4 周）：
  ├─ 宏体系：entity_root!/material!/topic!/plugin_group!
  ├─ 跨线程：Arc<Mutex>/AtomicBool/channel
  ├─ GPU 采集：ExtractSchedule/双 App 架构
  └─ 能读懂 ros2/plugin.rs 或 talos/plugin.rs 的通信闭环

第 4 优先（按需）：
  ├─ unsafe + FFI（只看 talos-ipc）
  ├─ async/await（只看 ros2/topic.rs）
  └─ Box<dyn Trait>（只看 capture/driver.rs）
```

**右列"扩展"知识，等项目用到了再学不迟**。不要提前学 States/AnimationPlayer/bevy_ui——项目没用，学了也忘。

---

*本文档随项目演进持续更新。如有疑问，对照源码注释为准。*
