#![allow(dead_code)] // 允许存在未使用的函数/结构体，编译不会警告报错，方便开发调试

// 拆分业务模块
mod auto_gen;       // 数据集自动生成模块
mod capture;        // 画面/数据采集模块
mod components;     // ECS 自定义组件定义
mod config;         // 全局配置解析模块(TOML配置读取)
mod dataset;        // 数据集落地存储模块
mod handler;        // 碰撞命中、激活事件的事件处理器
mod robomaster;     // 机甲主体逻辑插件包
mod setup;          // 场景、地面、车辆初始化构建函数
mod statistic;      // 弹丸命中统计资源
mod systems;        // 所有业务系统(控制、瞄准、弹道、清理等)
mod telemetry;      // 遥测数据上报
mod util;           // 通用工具函数

// 条件编译模块：开启ros2 feature才编译ros2对接代码
#[cfg(feature = "ros2")]
mod ros2;
// 条件编译：开启talos feature才编译Talos采集插件
#[cfg(feature = "talos")]
mod talos;

// 引入3D物理引擎 Avian3D（Bevy官方物理库，替代旧rapier）
use avian3d::prelude::*;
// Bevy游戏引擎核心全部导入
use bevy::prelude::*;
// 帧率诊断插件、日志性能插件
use bevy::diagnostic::{FrameTimeDiagnosticsPlugin, LogDiagnosticsPlugin};
// Bevy渲染窗口垂直同步模式配置
use bevy::render::settings::{InstanceFlags, RenderCreation, WgpuSettings, WgpuSettingsPriority};
use bevy::render::{RenderPlugin, RenderSystems};
use bevy::window::PresentMode;
// egui 可视化调试面板插件
use bevy_inspector_egui::bevy_egui::EguiPlugin;
// 实体世界检视器插件，运行时查看所有实体、组件
use bevy_inspector_egui::quick::WorldInspectorPlugin;
// 命令行参数解析库
use clap::Parser;
// 原子布尔类型，多线程安全布尔标记
use std::sync::atomic::AtomicBool;

// 导入各个子模块内部需要的插件、结构体
use crate::auto_gen::AutoGenPlugin;
use crate::components::{CameraMode, FollowingType, ProjectileCooldown, SubscribeAutoAim};
use crate::config::{ConfigPlugin, SimulationConfig};
use crate::dataset::prelude::DatasetPlugin;
use crate::handler::{on_activate, on_hit};
use crate::robomaster::prelude::RoboMasterPlugins;
use crate::setup::{setup, setup_collision, setup_ground, setup_vehicle};
use crate::statistic::ProjectileStatistics;
use crate::systems::{
    ChassisObservationFrame, GameplaySystems, PreviousKinematicState, auto_aim_switch,
    change_appearance, cleanup_projectiles, following_controls, freecam_controls, gimbal_controls,
    projectile_aerodynamics, projectile_launch, remote_gimbal_controls, remote_vehicle_controls,
    screenshot_on_f2, screenshot_saving, setup_projectile, switch_slapper_control, uav_launch,
    update_chassis_observation, update_help_text, vehicle_controls,
};

/// 程序命令行参数结构体，使用clap解析命令行入参
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// 命令行参数：--auto-gen 开启数据集全自动生成模式
    #[arg(long)]
    auto_gen: bool,
}

// 开启ros2编译特性时，引入ros2插件
#[cfg(feature = "ros2")]
use crate::ros2::plugin::ROS2Plugin;
// 开启talos编译特性时，引入Talos采集插件
#[cfg(feature = "talos")]
use talos::TalosPlugin;

/// 将配置文件中的字符串垂直同步配置，转为Bevy窗口渲染PresentMode枚举
fn present_mode_from_config(value: &str) -> Option<PresentMode> {
    match value.trim().to_ascii_lowercase().as_str() {
        "auto_vsync" | "vsync" => Some(PresentMode::AutoVsync),        // 自动开启垂直同步
        "auto_no_vsync" | "no_vsync" | "novsync" => Some(PresentMode::AutoNoVsync), // 关闭垂直同步(高帧率)
        "fifo" => Some(PresentMode::Fifo),
        "fifo_relaxed" | "fifo-relaxed" => Some(PresentMode::FifoRelaxed),
        "mailbox" => Some(PresentMode::Mailbox), // 邮箱模式，低延迟渲染
        "immediate" => Some(PresentMode::Immediate), // 无缓冲立刻渲染
        _ => None, // 配置值非法返回None，外部做兜底降级
    }
}

/// 判断当前系统是否运行在WSL环境（WSL显卡渲染存在兼容bug，需要特殊渲染配置）
fn is_wsl() -> bool {
    // 检测WSL专属环境变量
    std::env::var_os("WSL_DISTRO_NAME").is_some()
        || std::env::var_os("WSL_INTEROP").is_some()
        // 读取内核版本字符串，包含microsoft说明是WSL2
        || std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .map(|release| release.to_ascii_lowercase().contains("microsoft"))
            .unwrap_or(false)
}

/// 根据运行平台生成适配的渲染插件：WSL环境使用兼容模式渲染，规避WSL显卡bug
fn render_plugin_for_platform() -> RenderPlugin {
    // Linux系统 + WSL环境，使用兼容WGPU渲染配置
    if cfg!(target_os = "linux") && is_wsl() {
        return RenderPlugin {
            render_creation: RenderCreation::Automatic(WgpuSettings {
                // 允许使用不完全兼容的显卡适配器，修复WSL显卡初始化失败
                instance_flags: InstanceFlags::default()
                    | InstanceFlags::ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER,
                priority: WgpuSettingsPriority::Compatibility, // 渲染策略：兼容性优先而非性能优先
                ..default()
            }),
            ..default()
        };
    }

    // 普通Windows/Linux/macOS环境，使用默认渲染配置
    RenderPlugin::default()
}

/// 条件编译talos插件时，判断是否启用Talos数据采集
#[cfg(feature = "talos")]
fn should_enable_talos_plugin(app: &App) -> bool {
    // 如果开启ROS2特性，判断ROS采集是否正在运行
    #[cfg(feature = "ros2")]
    let ros_capture_active = app
        .world()
        .contains_resource::<crate::ros2::capture::RosCaptureContext>();
    // 未开启ROS2编译特性，则ROS采集必然未启用
    #[cfg(not(feature = "ros2"))]
    let ros_capture_active = false;

    // 环境变量 DAEDALUS_FORCE_TALOS_CAPTURE=1 强制开启Talos采集
    let force_talos_capture = std::env::var("DAEDALUS_FORCE_TALOS_CAPTURE")
        .map(|v| v == "1")
        .unwrap_or(false);

    // 逻辑：ROS采集没在运行 或者 强制开启标记打开，就启用Talos插件
    !ros_capture_active || force_talos_capture
}

fn main() {
    // 解析命令行启动参数
    let args = Args::parse();

    // ========== 分支1：命令行携带 --auto-gen，进入【纯数据集自动生成模式】精简运行 ==========
    if args.auto_gen {
        let config = SimulationConfig::default();
        // 读取配置里的渲染同步模式，非法配置降级为AutoNoVsync
        let present_mode =
            present_mode_from_config(&config.window.present_mode).unwrap_or_else(|| {
                warn!(
                    "Unknown window.present_mode {:?}, falling back to auto_no_vsync",
                    config.window.present_mode
                );
                PresentMode::AutoNoVsync
            });

        // 构建极简App实例，去掉UI、调试面板，专注高速生成数据集
        App::new()
            .add_plugins((
                DefaultPlugins
                    .set(WindowPlugin {
                        primary_window: Some(Window {
                            present_mode,
                            fit_canvas_to_parent: true, // 窗口适配父容器尺寸
                            ..default()
                        }),
                        ..default()
                    })
                    .set(render_plugin_for_platform()), // 适配WSL渲染
                PhysicsPlugins::default(), // 加载Avian3D物理引擎
            ))
            .add_plugins(RoboMasterPlugins)  // 机甲核心逻辑插件
            .add_plugins(ConfigPlugin)      // 配置加载插件
            .add_observer(setup_vehicle)    // 观测器：生成机甲载具实体
            .insert_resource(Gravity(Vec3::ZERO)) // 自动生成数据集时关闭重力，便于批量采集姿态样本
            .insert_resource(SubstepCount(config.physics.substep_count)) // 物理子迭代步数（提升物理精度）
            .add_plugins(AutoGenPlugin) // 数据集自动生成插件
            .run();
        return; // 生成模式运行完毕直接退出main，不再执行下方完整仿真逻辑
    }

    // ========== 分支2：默认模式，完整 RoboMaster 人机交互仿真模拟器 ==========
    let config = SimulationConfig::default();
    // 读取窗口垂直同步配置，非法配置兜底降级
    let present_mode = present_mode_from_config(&config.window.present_mode).unwrap_or_else(|| {
        warn!(
            "Unknown window.present_mode {:?}, falling back to auto_no_vsync",
            config.window.present_mode
        );
        PresentMode::AutoNoVsync
    });
    let mut app = App::new();
    // 挂载默认插件集(窗口、输入、渲染、音频、事件等基础能力)+物理引擎
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

    // 如果配置开启egui调试UI，则挂载EGUI面板 + 实体世界检视器
    if config.debug.egui {
        app.add_plugins(EguiPlugin::default());
        if config.debug.inspector {
            app.add_plugins(WorldInspectorPlugin::new());
        }
    }

    // 挂载业务插件
    app.add_plugins(RoboMasterPlugins)
        .add_plugins(DatasetPlugin)       // 数据集保存插件
        .add_plugins(ConfigPlugin)        // 全局配置插件
        // 初始化全局资源(常驻数据，不属于任何实体)
        .init_resource::<CameraMode>()
        .init_resource::<ProjectileStatistics>()
        .init_resource::<ChassisObservationFrame>()
        .init_resource::<PreviousKinematicState>()
        // 允许Inspector插件查看该资源结构
        .register_type::<ProjectileStatistics>()
        // 开启真实重力加速度
        .insert_resource(Gravity(Vec3::NEG_Y * 9.81))
        .insert_resource(SubstepCount(config.physics.substep_count))
        // 自动瞄准订阅开关，原子布尔保证多线程安全修改
        .insert_resource(SubscribeAutoAim(AtomicBool::new(false)))
        // 弹丸发射冷却计时器，读取配置文件中的冷却时长
        .insert_resource(ProjectileCooldown(Timer::from_seconds(
            config.projectile.cooldown,
            TimerMode::Once,
        )))
        // Startup阶段运行的一次性初始化系统
        .add_systems(Startup, (setup, setup_projectile))
        // 事件观测器：触发对应事件时自动执行函数
        .add_observer(setup_ground)       // 生成地面场景
        .add_observer(setup_vehicle)       // 生成机甲载具
        .add_observer(setup_collision)     // 全局碰撞层配置
        .add_observer(on_hit)              // 命中装甲事件回调
        .add_observer(on_activate)         // 机甲激活生成事件回调
        // 划分Update阶段执行顺序：输入 → 游戏逻辑 → 相机更新 → 垃圾清理，串行执行避免时序错乱
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
        // Update输入阶段：所有按键、遥控器输入控制系统
        .add_systems(
            Update,
            (
                // Input阶段系统集合
                (
                    auto_aim_switch,
                    following_controls,
                    switch_slapper_control,
                    // 仅非自由视角时，启用底盘键盘控制
                    vehicle_controls.run_if(|mode: Res<CameraMode>| mode.0 != FollowingType::Free),
                    remote_vehicle_controls,
                    gimbal_controls,
                    remote_gimbal_controls,
                )
                    .in_set(GameplaySystems::Input),
                // GameLogic阶段：外观切换、帮助文本更新
                (change_appearance, update_help_text).in_set(GameplaySystems::GameLogic),
                // Camera阶段：自由视角控制 / 跟随机甲视角控制，在渲染前更新相机位置
                (
                    freecam_controls.run_if(|mode: Res<CameraMode>| mode.0 == FollowingType::Free),
                    systems::update_camera_follow
                        .run_if(|mode: Res<CameraMode>| mode.0 != FollowingType::Free),
                )
                    .in_set(GameplaySystems::Camera)
                    .before(RenderSystems::Render),
                // Cleanup阶段：过期弹丸清理、F2截图、截图异步保存
                (
                    cleanup_projectiles,
                    screenshot_on_f2
                        .run_if(|input: Res<ButtonInput<KeyCode>>| input.just_pressed(KeyCode::F2)),
                    screenshot_saving,
                )
                    .in_set(GameplaySystems::Cleanup),
            ),
        )
        // PostUpdate阶段：变换传播完毕后，采集底盘观测数据
        .add_systems(
            PostUpdate,
            update_chassis_observation.after(TransformSystems::Propagate),
        )
        // 空格按下时发射弹丸，必须等待世界坐标Transform传播完成再生成子弹
        .add_systems(
            PostUpdate,
            projectile_launch
                .after(TransformSystems::Propagate)
                .run_if(|keyboard: Res<ButtonInput<KeyCode>>| keyboard.pressed(KeyCode::Space)),
        )
        // 无人机发射系统
        .add_systems(PostUpdate, uav_launch.after(TransformSystems::Propagate))
        // FixedUpdate固定物理帧率更新：子弹空气动力学（恒定步长保证弹道物理稳定）
        .add_systems(FixedUpdate, projectile_aerodynamics);

    // 配置开启性能诊断时，挂载帧率打印插件
    if config.debug.diagnostics {
        app.add_plugins((
            FrameTimeDiagnosticsPlugin::default(),
            LogDiagnosticsPlugin::default(),
        ));
    }

    // 开启ros2编译特性，挂载ROS2通信插件
    #[cfg(feature = "ros2")]
    {
        app.add_plugins(ROS2Plugin::default());
        info!("ROS2 integration enabled");
    }
    #[cfg(not(feature = "ros2"))]
    {
        info!("ROS2 integration disabled");
    }

    // 开启talos编译特性，判断条件后挂载Talos采集插件
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

    // 启动bevy游戏循环
    app.run();
}