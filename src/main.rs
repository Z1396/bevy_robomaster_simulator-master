#![allow(dead_code)]
// 作用：允许模块内存在未使用的结构体、函数、常量，编译器不会抛出 unused 警告
// 开发阶段常用，避免新增逻辑暂时没调用就疯狂告警；正式发布可删掉此宏开启完整检查

// ====================== 业务模块拆分声明 ======================
mod auto_gen;       // 数据集全自动生成模块：自动控制机器人移动、射击、采集图像+标签数据集
mod capture;        // 画面采集、深度纹理抓取、屏幕捕获逻辑（前面那段深度拷贝代码就在这里）
mod components;     // 全局自定义ECS组件定义（相机模式、自瞄标记、子弹冷却组件等）
mod config;         // TOML配置文件解析、全局仿真配置资源管理
mod dataset;        // 数据集落地写入本地磁盘、打包存储逻辑
mod handler;        // 事件处理器：装甲命中、机甲激活的碰撞/事件回调
mod robomaster;     // RM机甲核心逻辑合集插件包（底盘、云台、装甲、子弹实体生成等）
mod setup;          // 一次性初始化函数：场景、地面、载具生成
mod statistic;      // 子弹命中统计全局资源结构体
mod systems;        // 所有业务系统：键盘遥控、云台控制、弹道空气力学、子弹清理、截图、自瞄切换等
mod telemetry;      // 遥测数据上报模块
mod util;           // 通用工具函数库（坐标转换、Transform拷贝、数学工具等）

// 条件编译：仅编译时开启 ros2 特性，才编译 ros2 对接代码，避免无依赖时编译报错
/*1. #[cfg(...)]
属性宏，全称 configure /conditional compile（条件编译）
被它包裹的代码、模块、use、函数、impl，只有括号内条件成立，才会参与编译；条件不成立时，代码直接被丢弃，完全不编译、不进最终二进制。
2. feature = "ros2"
条件：只有在编译项目时，开启名为 ros2 的特性 (feature)，条件才为 true。 */
#[cfg(feature = "ros2")]
mod ros2;
// 条件编译：开启 talos 采集特性才编译 Talos 高精度采集插件
#[cfg(feature = "talos")]
mod talos;

// ====================== 第三方库引入 ======================
// Avian3D：Bevy 官方主推3D物理引擎，替代旧 Rapier，负责碰撞、弹道物理、刚体运动
use avian3d::prelude::*;
// Bevy 引擎全套基础API：ECS、实体、资源、插件、观察者、系统等
use bevy::prelude::*;
// 帧率耗时诊断插件、日志打印帧率性能插件
use bevy::diagnostic::{FrameTimeDiagnosticsPlugin, LogDiagnosticsPlugin};
// WGPU渲染底层配置，用于修改窗口垂直同步、渲染适配器规则
use bevy::render::settings::{InstanceFlags, RenderCreation, WgpuSettings, WgpuSettingsPriority};
use bevy::render::{RenderPlugin, RenderSystems};
use bevy::window::PresentMode;
// Egui 内嵌GUI调试面板
use bevy_inspector_egui::bevy_egui::EguiPlugin;
// 运行时实体检视器，可视化查看世界所有实体、挂载的组件、资源
use bevy_inspector_egui::quick::WorldInspectorPlugin;
// clap：命令行参数解析框架，解析启动时传入的指令参数
use clap::Parser;
// 原子布尔，多线程无锁安全布尔，用于跨线程修改自瞄开关状态
use std::sync::atomic::AtomicBool;

// ====================== 导入本地各个子模块对外暴露的插件、类型 ======================
// 引入自动数据集生成插件：用来全自动对局、自动截图采集装甲数据集（YOLO训练素材）
use crate::auto_gen::AutoGenPlugin;

// 游戏内自定义组件（ECS组件，挂载在实体Entity上）
use crate::components::{
    CameraMode,                // 相机模式枚举：自由视角/跟随车辆/跟随云台
    FollowingType,             // 跟随模式类型：跟随底盘、跟随云台、跟随发射弹丸
    ProjectileCooldown,        // 弹丸发射冷却组件：记录发射CD时间，防止连发违规
    SubscribeAutoAim,          // 自动瞄准订阅标记组件：挂载后实体开启自动瞄准逻辑
};

use crate::config::{ConfigPlugin, SimulationConfig};// 全局配置插件 + 仿真配置结构体
use crate::dataset::prelude::DatasetPlugin;// 数据集模块完整导入：负责采集对局画面、装甲标签、坐标，输出标准数据集格式
use crate::handler::{on_activate, on_hit};// 事件回调处理器：碰撞激活、命中判定逻辑
use crate::robomaster::prelude::RoboMasterPlugins;// RM机器人整套功能插件合集（底盘、云台、装甲、电机物理、装甲血量、击打判定全部封装在内）
use crate::setup::{         // 世界初始化函数：场景、地面、车辆实体、碰撞体初始化
    setup,                 // 全局总初始化函数
    setup_collision,       // 初始化碰撞层、碰撞过滤规则（装甲、弹丸、地面碰撞规则）
    setup_ground,          // 生成仿真场地地面、边界围栏
    setup_vehicle,         // 生成RM战车实体，挂载底盘、云台、装甲组件
};
use crate::statistic::ProjectileStatistics;// 弹丸命中统计组件，记录发射次数、命中装甲、命中率等对战数据
// 业务系统集合（Bevy System，每一帧自动运行的逻辑函数）
use crate::systems::{
    // 观测系统：为强化学习/感知模块输出底盘状态观测帧数据
    ChassisObservationFrame,
    // 游戏主逻辑系统组，统一管理对局生命周期
    GameplaySystems,
    // 存储上一帧刚体运动状态，用于差分计算、速度求解、平滑滤波
    PreviousKinematicState,

    auto_aim_switch,                // 自动瞄准开关逻辑，控制开启/关闭自瞄
    change_appearance,              // 更换战车皮肤、装甲颜色、阵营外观
    cleanup_projectiles,            // 清理过期弹丸（飞出场地/命中后销毁子弹，避免内存堆积）
    following_controls,             // 跟随相机控制逻辑
    freecam_controls,               // 自由视角键鼠控制
    gimbal_controls,                // 本地操控云台俯仰、旋转
    projectile_aerodynamics,        // 弹丸空气阻力物理仿真，复刻真实子弹下坠、飞行轨迹
    projectile_launch,              // 发射弹丸主逻辑，检测冷却、生成子弹实体
    remote_gimbal_controls,         // 远程客户端操控云台（局域网多机操控）
    remote_vehicle_controls,        // 远程操控底盘移动
    screenshot_on_f2,               // F2快捷键手动截图
    screenshot_saving,              // 截图落地保存逻辑，配合数据集模块打标签
    setup_projectile,               // 子弹实体初始化，挂载刚体、碰撞、冷却组件
    switch_slapper_control,         // 哨兵/飞镖机构控制切换
    uav_launch,                     // 无人机投放系统（RM无人机投掷弹丸逻辑）
    update_chassis_observation,     // 刷新底盘观测数据供给AI感知
    update_help_text,               // 更新界面左下角帮助提示文字
    vehicle_controls,               // 本地键盘操控底盘前进后退转向
};

/// 命令行入参结构体，#[derive(Parser)] 由clap自动实现解析逻辑
/*
#[derive(Parser)]
clap 自动为 Args 实现 Parser trait，从而可以调用 Args::parse() 解析命令行。
#[derive(Debug)]
允许打印 Args 结构体调试查看参数。
#[command(...)]
配置程序的帮助信息、版本、作者，运行 ./程序 --help 就能自动生成帮助面板。 */
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
/*上方三行注释：文档注释，执行 --help 帮助命令时会展示这段说明。
#[arg(long)]：过程宏注解，clap 的标记：
long：代表长参数格式 --auto-gen；
长参数规则：结构体字段名 auto_gen 自动对应命令行 --auto-gen，下划线 _ 自动转为横杠 -。
    auto_gen: bool,
字段类型是 bool 布尔值：
命令行写了 --auto-gen → auto_gen = true；
不写该参数 → auto_gen = false。布尔型参数不需要额外赋值，存在即真。 */
struct Args {
    /// 启动参数：--auto-gen，开启全自动数据集生成模式
    #[arg(long)]
    auto_gen: bool,
}

// 开启ros2特性时，引入ROS通信插件
#[cfg(feature = "ros2")]
use crate::ros2::plugin::ROS2Plugin;
// 开启talos特性时引入采集插件
#[cfg(feature = "talos")]
use talos::TalosPlugin;

/// 将配置文件里的字符串配置，解析转换为 Bevy 窗口渲染同步模式 PresentMode
/// PresentMode 控制渲染帧与显示器刷新率的同步策略
fn present_mode_from_config(value: &str) -> Option<PresentMode> {
    match value.trim().to_ascii_lowercase().as_str() {
        "auto_vsync" | "vsync" => Some(PresentMode::AutoVsync),        // 自动开启垂直同步，防画面撕裂
        "auto_no_vsync" | "no_vsync" | "novsync" => Some(PresentMode::AutoNoVsync), // 关闭垂直同步，拉高帧率
        "fifo" => Some(PresentMode::Fifo),
        "fifo_relaxed" | "fifo-relaxed" => Some(PresentMode::FifoRelaxed),
        "mailbox" => Some(PresentMode::Mailbox), // 邮箱模式：低延迟渲染，VR/仿真常用
        "immediate" => Some(PresentMode::Immediate), // 无渲染缓冲，立刻提交画面，延迟最低
        _ => None, // 配置文本非法，返回None交由上层做兜底降级
    }
}

/// 检测当前运行环境是否为 WSL（Windows Linux子系统）
/// WSL 的GPU转发存在兼容性缺陷，WGPU默认配置极易渲染崩溃、黑屏，需要特殊兼容配置
fn is_wsl() -> bool {
    // 方式1：检测WSL专属环境变量
    std::env::var_os("WSL_DISTRO_NAME").is_some()
        || std::env::var_os("WSL_INTEROP").is_some()
        // 方式2：读取内核版本，内核带 microsoft 字段代表WSL2
        || std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .map(|release| release.to_ascii_lowercase().contains("microsoft"))
            .unwrap_or(false)
}

/// 根据操作系统环境生成适配的 RenderPlugin
/// WSL环境使用兼容性优先渲染配置，普通系统使用默认高性能渲染配置
fn render_plugin_for_platform() -> RenderPlugin {
    // Linux系统 + WSL环境，启用兼容模式
    if cfg!(target_os = "linux") && is_wsl() {
        return RenderPlugin {
            render_creation: RenderCreation::Automatic(WgpuSettings {
                // 允许使用不完全符合WebGPU标准的显卡适配器，修复WSL显卡初始化失败
                instance_flags: InstanceFlags::default()
                    | InstanceFlags::ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER,
                priority: WgpuSettingsPriority::Compatibility, // 渲染优先级：兼容性 > 性能
                ..default()
            }),
            ..default()
        };
    }

    // Windows / Mac / 原生Linux：默认高性能渲染配置
    RenderPlugin::default()
}

/// 仅开启 talos 编译特性时生效：判断是否启用Talos采集插件
/// 规则：ROS2正在采集数据时默认不启动Talos，避免两路采集冲突；环境变量可强制开启
#[cfg(feature = "talos")]
fn should_enable_talos_plugin(app: &App) -> bool {
    #[cfg(feature = "ros2")]
    let ros_capture_active = app
        .world()
        .contains_resource::<crate::ros2::capture::RosCaptureContext>();
    #[cfg(not(feature = "ros2"))]
    let ros_capture_active = false;

    // 环境变量 DAEDALUS_FORCE_TALOS_CAPTURE=1 强制启用Talos采集
    let force_talos_capture = std::env::var("DAEDALUS_FORCE_TALOS_CAPTURE")
        .map(|v| v == "1")
        .unwrap_or(false);

    // ROS未采集 或者 强制开启标记打开，则启用Talos
    !ros_capture_active || force_talos_capture
}

fn main() {
    // 解析控制台传入的命令行参数
    let args = Args::parse();

    // ==============================================
    // 分支一：带 --auto-gen 参数，进入【纯数据集自动生成模式】
    // 精简App，移除UI、调试面板、人机控制逻辑，最大化性能批量生成数据集
    // ==============================================
    if args.auto_gen {
        let config = SimulationConfig::default();
        // 解析垂直同步配置，非法值兜底关闭垂直同步
        let present_mode =
            present_mode_from_config(&config.window.present_mode).unwrap_or_else(|| {
                warn!(
                    "Unknown window.present_mode {:?}, falling back to auto_no_vsync",
                    config.window.present_mode
                );
                PresentMode::AutoNoVsync
            });

        App::new()
            .add_plugins((
                /*DefaultPlugins：Bevy 基础全家桶（窗口、渲染、输入、事件、音频等）；
                覆盖 WindowPlugin：使用上面解析好的垂直同步策略；fit_canvas_to_parent 窗口自适应布局；
                .set(render_plugin_for_platform())：复用前面的平台渲染适配逻辑，WSL 环境自动切换兼容渲染方案，防止 WSL 黑屏崩溃； */
                DefaultPlugins
                    .set(WindowPlugin {
                        primary_window: Some(Window {
                            present_mode,
                            fit_canvas_to_parent: true, // 窗口尺寸跟随父容器自动适配
                            ..default()
                        }),
                        ..default()
                    })
                    .set(render_plugin_for_platform()), // WSL渲染兼容
                /*PhysicsPlugins::default()：载入 Avian3D 物理引擎，用来模拟机甲运动、碰撞。 */
                PhysicsPlugins::default(), // 挂载Avian3D物理引擎
            ))
            
            .add_plugins(RoboMasterPlugins)  // 机甲基础实体插件
            .add_plugins(ConfigPlugin)      // 加载全局配置
            .add_observer(setup_vehicle)    // 自动生成机甲载具
            .insert_resource(Gravity(Vec3::ZERO)) // 数据集生成阶段关闭重力，方便均匀采集各类姿态样本
            .insert_resource(SubstepCount(config.physics.substep_count)) // 物理子迭代次数，提升弹道精度
            .add_plugins(AutoGenPlugin) // 全自动数据集生成核心插件
            /*启动数据集生成的 Bevy 游戏循环，阻塞运行，直到自动采集完成后 App 主动退出。 */
            .run();
        // 自动生成模式运行结束，直接return，不再执行下方完整交互仿真逻辑
        return;
    }

    // ==============================================
    // 分支二：默认启动模式 = 完整可交互 RM 仿真模拟器
    // 支持键鼠遥控、云台控制、射击、调试UI、截图、数据采集、ROS联动
    // ==============================================
    let config = SimulationConfig::default();
    /*Rust 标准 Option 方法：
    如果上一步结果是 Some(PresentMode)：直接取出里面的值赋值给 present_mode；
    如果上一步是 None（解析失败）：执行闭包里面的逻辑，用闭包返回值当做最终结果。
    和 unwrap_or 区别：unwrap_or 无论成功失败都会创建兜底值；unwrap_or_else 只有失败才执行闭包，性能更好。 */
    let present_mode = present_mode_from_config(&config.window.present_mode).unwrap_or_else(|| {
        warn!(
            "Unknown window.present_mode {:?}, falling back to auto_no_vsync",
            config.window.present_mode
        );
        PresentMode::AutoNoVsync
    });
    let mut app = App::new();

    // 挂载基础插件集：窗口、输入、渲染、音频、事件系统 + 物理引擎
    app.add_plugins((
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    present_mode,
                    fit_canvas_to_parent: true,
                    ..default()
                }),
                ..default()
            })
            .set(render_plugin_for_platform()),
        PhysicsPlugins::default(),
    ));

    // 配置开启调试EGUI面板时，加载EGUI + 实体检视器
    if config.debug.egui {
        app.add_plugins(EguiPlugin::default());
        if config.debug.inspector {
            app.add_plugins(WorldInspectorPlugin::new());
        }
    }

    // 挂载业务插件合集
    app.add_plugins(RoboMasterPlugins)
        .add_plugins(DatasetPlugin)       // 数据集落地存储
        .add_plugins(ConfigPlugin)        // TOML配置加载

        // 初始化全局常驻资源（不属于任何实体，全局唯一）
        .init_resource::<CameraMode>()
        .init_resource::<ProjectileStatistics>()
        .init_resource::<ChassisObservationFrame>()
        .init_resource::<PreviousKinematicState>()
        // 允许Inspector插件反射查看该资源结构
        .register_type::<ProjectileStatistics>()

        .insert_resource(Gravity(Vec3::NEG_Y * 9.81)) // 开启真实重力 9.81m/s²
        .insert_resource(SubstepCount(config.physics.substep_count))
        // 自瞄全局开关，原子布尔支持多线程安全修改
        .insert_resource(SubscribeAutoAim(AtomicBool::new(false)))
        // 子弹发射冷却计时器，从配置读取冷却时长
        .insert_resource(ProjectileCooldown(Timer::from_seconds(
            config.projectile.cooldown,
            TimerMode::Once,
        )))

        // Startup生命周期：游戏启动仅运行一次的初始化系统
        .add_systems(Startup, (setup, setup_projectile))

        // 事件观察者：对应事件触发时自动执行函数
        .add_observer(setup_ground)       // 生成地面平面
        .add_observer(setup_vehicle)       // 生成我方机甲实体
        .add_observer(setup_collision)     // 全局碰撞层分组配置
        .add_observer(on_hit)              // 子弹命中装甲事件处理
        .add_observer(on_activate)         // 机甲被激活生成时的回调

        // ====================== 严格划分 Update 阶段顺序，链式串行执行，避免时序错乱 ======================
        // 执行顺序：输入采集 → 游戏逻辑计算 → 相机位置更新 → 垃圾清理
        .configure_sets(
            Update,
            (
                GameplaySystems::Input,
                GameplaySystems::GameLogic,
                GameplaySystems::Camera,
                GameplaySystems::Cleanup,
            )
                .chain(),
        )
        // 向各个阶段挂载对应业务系统
        .add_systems(
            Update,
            (
                // Input阶段：所有输入控制系统
                (
                    auto_aim_switch,
                    following_controls,
                    switch_slapper_control,
                    // 自由视角模式下禁用底盘键盘控制
                    vehicle_controls.run_if(|mode: Res<CameraMode>| mode.0 != FollowingType::Free),
                    remote_vehicle_controls,
                    gimbal_controls,
                    remote_gimbal_controls,
                )
                    .in_set(GameplaySystems::Input),

                // GameLogic阶段：外观切换、UI帮助文本更新
                (change_appearance, update_help_text).in_set(GameplaySystems::GameLogic),

                // Camera阶段：更新相机位置，必须在渲染之前执行
                (
                    freecam_controls.run_if(|mode: Res<CameraMode>| mode.0 == FollowingType::Free),
                    systems::update_camera_follow
                        .run_if(|mode: Res<CameraMode>| mode.0 != FollowingType::Free),
                )
                    .in_set(GameplaySystems::Camera)
                    .before(RenderSystems::Render),

                // Cleanup阶段：过期子弹销毁、F2截图触发、截图异步保存
                (
                    cleanup_projectiles,
                    screenshot_on_f2
                        .run_if(|input: Res<ButtonInput<KeyCode>>| input.just_pressed(KeyCode::F2)),
                    screenshot_saving,
                )
                    .in_set(GameplaySystems::Cleanup),
            ),
        )

        // PostUpdate阶段：所有实体Transform传播完毕之后，采集底盘观测数据
        .add_systems(
            PostUpdate,
            update_chassis_observation.after(TransformSystems::Propagate),
        )

        // 空格键发射子弹：必须等待Transform传播完成，才能拿到机甲最新世界坐标生成子弹
        .add_systems(
            PostUpdate,
            projectile_launch
                .after(TransformSystems::Propagate)
                .run_if(|keyboard: Res<ButtonInput<KeyCode>>| keyboard.pressed(KeyCode::Space)),
        )

        // 无人机投放系统同样等待坐标更新完成
        .add_systems(PostUpdate, uav_launch.after(TransformSystems::Propagate))

        // FixedUpdate 固定物理帧率更新：子弹空气动力学
        // FixedUpdate 步长恒定，不受帧率波动影响，保证弹道物理结果稳定
        .add_systems(FixedUpdate, projectile_aerodynamics);

    // 配置开启性能诊断，挂载帧率耗时打印插件
    if config.debug.diagnostics {
        app.add_plugins((
            FrameTimeDiagnosticsPlugin::default(),
            LogDiagnosticsPlugin::default(),
        ));
    }

    // 条件编译挂载ROS2插件
    #[cfg(feature = "ros2")]
    {
        app.add_plugins(ROS2Plugin::default());
        info!("ROS2 integration enabled");
    }
    #[cfg(not(feature = "ros2"))]
    {
        info!("ROS2 integration disabled");
    }

    // 条件编译挂载Talos采集插件（互斥ROS采集）
    #[cfg(feature = "talos")]
    {
        if should_enable_talos_plugin(&app) {
            app.add_plugins(TalosPlugin::default());
            info!("talos integration enabled");
        } else {
            info!(
                "talos integration skipped: ROS2 capture already active \
                 (set DAEDALUS_FORCE_TALOS_CAPTURE=1 to override)"
            );
        }
    }

    // 启动Bevy主游戏循环，阻塞运行
    app.run();
}

/*main() 启动
   │
   ├─ ① Args::parse() 解析命令行
   │
   ├─ ② 判断 --auto-gen？
   │     ├─ 是 → 走【数据集自动生成模式】(精简 App) → run() → 退出
   │     └─ 否 → 走【完整交互仿真模式】(完整 App) → run()
   │
   └─ ③ 完整模式的 App 内部运行时序（run() 接管后）
         │
         ├─ Startup 阶段（只跑一次）
         │     setup()              生成地面、灯光、相机、UI
         │     setup_projectile()   初始化弹丸资源池
         │
         ├─ 观察者事件（被动触发）
         │     setup_ground / setup_vehicle / setup_collision / on_hit / on_activate
         │
         └─ 主循环（每帧重复）
              │
              ├─ Update 阶段（按 SystemSet 链式顺序）
              │     Input    输入采集 → GameLogic 业务 → Camera 相机 → Cleanup 清理
              │
              ├─ PostUpdate 阶段
              │     TransformSystems::Propagate  (Bevy 自带，传播坐标)
              │     update_chassis_observation   (算底盘观测)
              │     projectile_launch / uav_launch (发射子弹)
              │
              ├─ FixedUpdate 阶段（固定步长）
              │     projectile_aerodynamics (弹道物理)
              │
              └─ Render 阶段（渲染出图） */