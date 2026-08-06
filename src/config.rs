// Bevy 游戏引擎基础导入
use bevy::prelude::*;
// 跨线程安全通道，用来传递配置文件变更事件
use crossbeam_channel::{Receiver, Sender, unbounded};
// notify：文件系统监听库，监视 config.toml 是否被修改
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
// serde 序列化解析 TOML
use serde::Deserialize;
use std::path::Path;

/// 顶层全局配置资源，对应 config.toml 完整结构
/// #[derive(Resource)]：标记为 Bevy 全局资源，全局可 Res<SimulationConfig> 获取
/// Deserialize：支持从 TOML 字符串解析
/// Reflect：支持 Bevy 反射，方便 egui 面板查看修改配置
/// Clone：允许整体拷贝配置
#[derive(Resource, Deserialize, Reflect, Clone)]
#[reflect(Resource)]
pub struct SimulationConfig {
    /// 窗口、垂直同步配置
    #[serde(default)]
    pub window: WindowConfig,
    /// 调试开关：egui面板、实体检视器、性能诊断
    #[serde(default)]
    pub debug: DebugConfig,
    /// 视角预览总开关
    #[serde(default)]
    pub preview: PreviewConfig,
    /// 渲染光照、阴影、FXAA抗锯齿配置
    #[serde(default)]
    pub render: RenderConfig,
    /// 画面/深度图自动采集管线（auto-gen 数据集截图依赖此配置）
    #[serde(default)]
    pub capture: CapturePipelineConfig,
    /// Livox 激光雷达 ROS 发布配置
    #[serde(default)]
    pub livox_ros: LivoxRosConfig,
    /// 物理引擎配置，物理子步数决定物理精度
    pub physics: PhysicsConfig,
    /// 车辆云台、底盘速度、俯仰限位参数
    pub vehicle: VehicleConfig,
    /// 麦轮底盘机械尺寸，#[serde(default)] TOML缺失此字段就用Default
    #[serde(default)]
    pub mecanum: MecanumConfig,
    /// 弹丸弹道物理：射速、弹丸大小、空气动力学、风阻
    pub projectile: ProjectileConfig,
    /// 观测相机FOV、移动速度、跟随机甲偏移、鼠标灵敏度
    pub camera: CameraConfig,
}

/// 窗口配置结构体
/*#[derive(特征1,特征2)] 是 Rust 派生宏语法。
作用：编译阶段自动帮你实现 trait（接口），不用手写 impl 代码。
类比 C++：编译器自动生成默认拷贝构造、默认构造函数。 */
#[derive(Deserialize, Reflect, Clone)]
pub struct WindowConfig {
    /// 渲染同步策略字符串：auto_no_vsync / vsync / mailbox 等
    pub present_mode: String,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            // 默认关闭垂直同步，解除60帧限制
            // 用途：auto-gen 离屏采集时可以跑满帧率加速生成数据集
            present_mode: "auto_no_vsync".to_string(),
        }
    }
}

/// 各类调试功能总开关
#[derive(Deserialize, Reflect, Clone)]
/*serde 是 Rust 最主流的序列化 / 反序列化第三方库。
序列化：结构体 → 文本 (TOML/JSON/YAML)，存文件、网络传输；
反序列化：配置文本 → 内存结构体，也就是你读取配置的核心。 */
#[serde(default)] // TOML没有debug节点自动填充Default
pub struct DebugConfig {
    pub egui: bool,        // 是否开启UI调试面板
    pub inspector: bool,   // 是否开启实体检视器（查看场景物体组件）
    pub diagnostics: bool, // 是否开启帧率、CPU耗时性能诊断
}

impl Default for DebugConfig {
    // 默认全部关闭调试面板，auto-gen模式不会加载UI占用性能
    fn default() -> Self {
        Self {
            egui: false,
            inspector: false,
            diagnostics: false,
        }
    }
}

/// 预览总开关
#[derive(Deserialize, Reflect, Clone)]
pub struct PreviewConfig {
    pub enabled: bool,
}

impl Default for PreviewConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// 渲染画质配置：光照亮度、阴影、抗锯齿
#[derive(Deserialize, Reflect, Clone)]
#[serde(default)]
pub struct RenderConfig {
    pub illuminance: f32,       // 环境光照亮度
    pub shadows: bool,          // 全局阴影开关
    pub main_camera_fxaa: bool, // 相机FXAA抗锯齿
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            illuminance: 50.0,
            shadows: false,       // 默认关闭阴影提速，数据集生成更快
            main_camera_fxaa: false,
        }
    }
}

/// 物理引擎配置
#[derive(Deserialize, Reflect, Clone)]
pub struct PhysicsConfig {
    pub substep_count: u32, // 物理子迭代步数，数值越大碰撞越精准、消耗更高
}

/// 车辆运动参数：底盘、云台转速限制
#[derive(Deserialize, Reflect, Clone)]
pub struct VehicleConfig {
    pub rotation_speed: f32,          // 车身旋转速度
    pub gimbal_rotation_speed: f32,   // 云台水平旋转速度
    pub gimbal_pitch_limit: f32,      // 云台俯仰最大限位(弧度) 0.785≈45°
    pub max_speed: f32,               // 底盘最大前进速度 m/s
    pub linear_acceleration: f32,     // 底盘加速度
    pub acceleration_exponent: f32,   // 加速度曲线指数，控制加速手感
}

/// 麦轮底盘机械尺寸（麦轮运动学解算依赖）
#[derive(Deserialize, Reflect, Clone)]
#[serde(default)]
pub struct MecanumConfig {
    pub wheel_radius_m: f32,       // 轮子半径 m
    pub half_wheelbase_m: f32,     // 前后轮轴距一半
    pub half_trackwidth_m: f32,    // 左右轮轮距一半
}

impl Default for MecanumConfig {
    // 实体英雄车实测尺寸
    fn default() -> Self {
        Self {
            wheel_radius_m: 0.076,
            half_wheelbase_m: 0.18,
            half_trackwidth_m: 0.15,
        }
    }
}

/// 弹丸完整弹道配置
#[derive(Deserialize, Reflect, Clone)]
pub struct ProjectileConfig {
    pub lifetime: f32,        // 子弹存活时长 s
    pub speed: f32,           // 出膛初速度 m/s
    pub cooldown: f32,        // 发射冷却时间
    pub diameter: f32,        // 弹丸直径
    pub uav_size: f32,        // 无人机弹药尺寸
    pub uav_vel: f32,         // 无人机弹药速度
    pub mass: f32,            // 子弹质量 kg
    pub friction: f32,        // 摩擦系数
    pub linear_damping: f32,  // 速度阻尼
    #[serde(default)]
    pub aerodynamics: ProjectileAerodynamicsConfig, // 空气动力学风阻配置
}

/// 子弹空气动力学（实现真实下坠、风阻效果）
#[derive(Deserialize, Reflect, Clone)]
pub struct ProjectileAerodynamicsConfig {
    pub enabled: bool,
    pub air_density: f32,        // 空气密度 kg/m³
    pub drag_coefficient: f32,   // 球形弹丸风阻系数
    pub wind: [f32; 3],          // 全局风速 xyz
}

impl Default for ProjectileAerodynamicsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            air_density: 1.225,
            drag_coefficient: 0.47,
            wind: [0.0, 0.0, 0.0],
        }
    }
}

/// 主视角相机参数
#[derive(Deserialize, Reflect, Clone)]
pub struct CameraConfig {
    pub fov: f32,               // 相机视场角 °
    pub free_move_speed: f32,   // 自由视角移动速度
    pub follow_offset: [f32; 3],// 跟随机甲时相机偏移位置
    pub mouse_sensitivity: f32,// 鼠标灵敏度
}

/// 画面采集管线：auto-gen自动截图、深度图保存的配置
#[derive(Deserialize, Reflect, Clone)]
#[serde(default)]
pub struct CapturePipelineConfig {
    pub color: CaptureStreamConfig,  // RGB彩色截图尺寸
    pub depth: DepthCaptureConfig,   // 深度图采集配置
}

impl Default for CapturePipelineConfig {
    fn default() -> Self {
        Self {
            color: CaptureStreamConfig::default(),
            depth: DepthCaptureConfig::default(),
        }
    }
}

/// 彩色截图分辨率
#[derive(Deserialize, Reflect, Clone)]
#[serde(default)]
pub struct CaptureStreamConfig {
    pub width: u32,
    pub height: u32,
}

impl Default for CaptureStreamConfig {
    // 默认 1440*1080 采集装甲图片
    fn default() -> Self {
        Self {
            width: 1440,
            height: 1080,
        }
    }
}

/// 深度图采集参数（测距、手眼标定、深度感知可用）
#[derive(Deserialize, Reflect, Clone)]
#[serde(default)]
pub struct DepthCaptureConfig {
    pub width: u32,
    pub height: u32,
    pub near: f32,  // 深度相机近裁剪面
    pub far: f32,   // 深度相机最远测距距离 80米
}

impl Default for DepthCaptureConfig {
    fn default() -> Self {
        Self {
            width: 640,
            height: 480,
            near: 0.1,
            far: 80.0,
        }
    }
}

/// Livox激光雷达 ROS话题发布配置
#[derive(Deserialize, Reflect, Clone)]
#[serde(default)]
pub struct LivoxRosConfig {
    pub enabled: bool,
    pub frame_id: String,
    pub publish_freq: f32,
    pub points_per_second: u32,
    pub line_num: u8,
    pub tag_default: u8,
    pub intensity_default: f32,
}

impl Default for LivoxRosConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            frame_id: "livox_frame".to_string(),
            publish_freq: 10.0,
            points_per_second: 100_000,
            line_num: 6,
            tag_default: 0,
            intensity_default: 100.0,
        }
    }
}

impl SimulationConfig {
    /// 从本地 config.toml 加载配置
    pub fn load() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        // 读取toml文本
        let content = std::fs::read_to_string("config.toml")?;
        // toml::from_str 解析文本映射到 SimulationConfig 结构体
        Ok(toml::from_str(&content)?)
    }
}

impl Default for SimulationConfig {
    /// 全局配置默认逻辑：优先加载 config.toml，文件缺失/损坏则使用硬编码默认值
    fn default() -> Self {
        Self::load().unwrap_or_else(|e| {
            warn!("Failed to load config.toml: {}, using defaults", e);
            // 全部模块填充出厂默认参数
            Self {
                // 窗口相关配置：分辨率、标题、全屏、帧率限制、VSync等，全部使用内置默认配置
                window: WindowConfig::default(),
                // 调试配置：是否开启调试绘制、碰撞框、坐标轴、日志等级、调试面板开关等，默认参数
                debug: DebugConfig::default(),
                // 预览窗口配置：额外小窗预览相机画面、装甲识别画面、雷达画面，采用默认设置
                preview: PreviewConfig::default(),
                // 渲染管线配置：抗锯齿、阴影质量、光照、后处理、纹理精度、渲染分层等默认渲染参数
                render: RenderConfig::default(),
                // 图像采集流水线配置：仿真内虚拟相机成像、画面裁剪、畸变模拟、图像编码输出，默认配置
                capture: CapturePipelineConfig::default(),
                // Livox雷达ROS对接配置：仿真Livox激光雷达、点云话题名称、发布频率、雷达安装位姿，默认值
                livox_ros: LivoxRosConfig::default(),

                // 物理引擎配置
                physics: PhysicsConfig {
                    // 物理子步数：每帧画面执行10轮物理迭代计算
                    // 数值越大物理碰撞、弹道模拟越精准，但是CPU开销越高；机器人仿真常用8~15
                    substep_count: 10,
                },

                // 底盘+云台整车运动参数配置
                vehicle: VehicleConfig {
                    // 底盘自身旋转最大角速度，单位 rad/s，3rad/s ≈ 172°/s
                    rotation_speed: 3.0,
                    // 云台yaw水平旋转最大角速度 rad/s
                    gimbal_rotation_speed: 3.0,
                    // 云台俯仰角上下限位 0.785rad = π/4 = 45°，防止云台过度俯仰卡死
                    gimbal_pitch_limit: 0.785,
                    // 底盘最大平移速度 m/s，战车极速4m/s
                    max_speed: 4.0,
                    // 底盘最大线性加速度 m/s²
                    linear_acceleration: 8.0,
                    // 加速度曲线指数：控制起步手感
                    // 数值越大，低速阶段加速平缓，踩满油门瞬间爆发加速，更贴合真实战车油门特性
                    acceleration_exponent: 10.0,
                },

                // 麦轮底盘专属配置：轮间距、轮半径、电机差补系数、麦轮运动解算矩阵，使用默认参数
                mecanum: MecanumConfig::default(),

                // 弹丸弹道物理配置
                projectile: ProjectileConfig {
                    // 弹丸生命周期 单位秒，子弹射出5秒后自动销毁，避免场景堆积大量子弹浪费性能
                    lifetime: 5.0,
                    // 子弹初速度 m/s，发射初速度25m/s
                    speed: 25.0,
                    // 发射冷却时间 0.1s，每秒最多发射10发子弹
                    cooldown: 0.1,
                    // 弹丸直径 0.017m = 17mm，标准RM17mm弹丸尺寸
                    diameter: 0.017,
                    // 弹丸质量 0.017kg = 17g，匹配真实比赛弹丸重量
                    mass: 0.017,
                    // 碰撞摩擦系数，子弹撞击装甲、地面时的摩擦力大小
                    friction: 1.1,
                    // 速度线性阻尼，0代表子弹飞行过程不受空气线性阻尼减速
                    linear_damping: 0.0,
                    // 空气动力学细分配置（风阻、下坠补偿等）使用默认配置
                    aerodynamics: ProjectileAerodynamicsConfig::default(),
                    // 无人机尺寸系数，用于无人机碰撞判定
                    uav_size: 1.0,
                    // 无人机移动速度上限 m/s
                    uav_vel: 2.0,
                },

                // 视角相机配置（玩家旁观视角）
                camera: CameraConfig {
                    // 相机视场角 FOV 45°，视野适中，不会过宽畸变严重也不会视野太窄
                    fov: 45.0,
                    // 自由漫游模式下相机移动速度 m/s
                    free_move_speed: 8.0,
                    // 跟随战车时相机偏移坐标 [X, Y, Z]
                    // 在战车后方Y=3、高度Z=2的位置跟随观看战车
                    follow_offset: [0.0, 3.0, 2.0],
                    // 鼠标灵敏度，控制视角转动快慢，数值越小越顺滑
                    mouse_sensitivity: 0.003,
                },
            }
        })
    }
}

/// 配置热重载监听器资源
#[derive(Resource)]
pub struct ConfigWatcher {
    /// 文件监视器实例，保存在资源里防止被回收
    _watcher: RecommendedWatcher,
    /// 接收文件变更事件的通道接收端
    receiver: Receiver<Result<Event, notify::Error>>,
}

/// 配置插件入口，挂载到App即可拥有配置加载+热重载能力
pub struct ConfigPlugin;

impl Plugin for ConfigPlugin {
    fn build(&self, app: &mut App) {
        // 初始化配置（读config.toml or default）
        let config = SimulationConfig::default();

        // 创建无界通道，用于监听线程和主线程传递文件修改事件
        let (tx, rx): (
            Sender<Result<Event, notify::Error>>,
            Receiver<Result<Event, notify::Error>>,
        ) = unbounded();

        // 创建文件监视器
        let watcher_result = RecommendedWatcher::new(
            move |res| {
                // 文件发生变化，把事件发送进通道
                let _ = tx.send(res);
            },
            notify::Config::default(),
        );

        match watcher_result {
            Ok(mut watcher) => {
                // 监听 config.toml，非递归监听（只监听这个文件，不监听文件夹内其他文件）
                if let Err(e) = watcher.watch(Path::new("config.toml"), RecursiveMode::NonRecursive)
                {
                    warn!("Failed to watch config.toml: {}", e);
                } else {
                    info!("Config hot-reload enabled for config.toml");
                    // 监视器存入全局资源
                    app.insert_resource(ConfigWatcher {
                        _watcher: watcher,
                        receiver: rx,
                    });
                    // 每帧执行热重载检测系统
                    app.add_systems(Update, config_hot_reload);
                }
            }
            Err(e) => {
                warn!("Failed to create config watcher: {}", e);
            }
        }

        // 将全局配置插入App资源、注册反射
        app.insert_resource(config)
            .register_type::<SimulationConfig>();
    }
}

/// 每帧执行的热重载系统：非阻塞读取文件变更，重载配置
fn config_hot_reload(mut config: ResMut<SimulationConfig>, watcher: Option<Res<ConfigWatcher>>) {
    let Some(watcher) = watcher else {
        return;
    };

    // 循环取出通道内所有文件事件，try_recv() 非阻塞，不会卡住渲染循环
    while let Ok(Ok(event)) = watcher.receiver.try_recv() {
        // 判断事件类型：文件被修改
        if event.kind.is_modify() {
            match SimulationConfig::load() {
                Ok(new_config) => {
                    info!("Config reloaded successfully");
                    // 覆盖全局配置资源，全游戏所有系统下一次读取就是新配置
                    *config = new_config;
                }
                Err(e) => {
                    // TOML写错格式不会崩溃，仅警告
                    warn!("Failed to reload config: {}", e);
                }
            }
        }
    }
}