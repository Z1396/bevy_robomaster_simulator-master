// ====================================================================
// 模块名: talos::plugin
// 作用:   将 talos-ipc 共享内存通信能力集成为 Bevy 插件
// 职责:   1. 创建 ShmPublisher 并注册为 Bevy 资源
//         2. 注册 TalosCapturePlugin 子插件负责图像与位姿采集
//         3. 尝试连接 C++ talos-cpp 的 ShmSubscriber（失败则降级）
//         4. 调度帧戳推进、心跳、位姿发布、真值发布、指令处理等系统
//         5. 提供 Bevy ↔ ROS 坐标系对齐的工具函数
// 说明:   与 ROS2 通道互斥：开启 talos 时不应同时开启 ros2。
//         坐标对齐矩阵 M_ALIGN_MAT3 将 Bevy 的 Y-up 转为 ROS 的 Z-up。
// ====================================================================

use crate::capture::driver::{CaptureConfig, CapturedFrameKind};
use crate::capture::{IMAGE_HEIGHT, IMAGE_WIDTH};
use crate::components::{
    Controlled, InfantryChassis, InfantryGimbal, InfantryLaunchOffset, SubscribeAutoAim,
};
use crate::config::SimulationConfig;
use crate::systems::projectile_launch;
use crate::talos::capture::{
    TalosCaptureContext, TalosCapturePlugin, TalosFrameStamp, advance_talos_frame_stamp,
    publish_talos_pose_system,
};
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use talos_ipc::*;

/// 包装 ShmSubscriber 的 Bevy 资源
///
/// 使用 Arc<Mutex<>> 是因为 ShmSubscriber 内部状态（read_idx）需要在
/// 多个系统间共享且需要可变访问，而 Bevy 资源本身是单例。
#[derive(Resource)]
pub struct ShmSubscriberRes(pub Arc<Mutex<ShmSubscriber>>);

/// 标记 talos 是否启用的 Bevy 资源
///
/// 通过原子布尔值实现无锁查询，供其他系统判断是否应执行 talos 相关逻辑。
#[derive(Resource, Deref, DerefMut)]
pub struct TalosEnabled(pub AtomicBool);

/// TalosPlugin 的配置项
///
/// 由使用方在注册插件前填充，决定图像分辨率、视场角与像素格式。
pub struct TalosPluginConfig {
    /// 图像宽度（像素）
    pub width: u32,
    /// 图像高度（像素）
    pub height: u32,
    /// 垂直视场角（弧度），用于计算相机内参
    pub fov_y: f32,
    /// Bevy 纹理格式，决定 GPU 捕获的像素布局
    pub texture_format: TextureFormat,
}

impl Default for TalosPluginConfig {
    /// 默认配置：使用 capture 模块的常量分辨率与仿真配置中的视场角
    fn default() -> Self {
        let config = SimulationConfig::default();
        Self {
            width: IMAGE_WIDTH,
            height: IMAGE_HEIGHT,
            fov_y: config.camera.fov.to_radians(),
            texture_format: TextureFormat::bevy_default(),
        }
    }
}

/// talos 通信插件入口
///
/// 注册后会在主进程创建共享内存生产者，并尝试连接外部 C++ 消费者。
#[derive(Default)]
pub struct TalosPlugin {
    /// 插件配置
    pub config: TalosPluginConfig,
}

impl Plugin for TalosPlugin {
    /// 构建 talos 插件，注册资源与系统
    ///
    /// 算法步骤:
    ///   1. 创建 ShmPublisher，失败则直接返回（不阻塞仿真器启动）
    ///   2. 将 publisher 包成 Arc<Mutex> 注入 TalosCaptureContext
    ///   3. 注册帧戳资源并添加 TalosCapturePlugin 子插件
    ///   4. 尝试连接 C++ talos-cpp，成功则注入 ShmSubscriberRes
    ///   5. 在 Last 调度表中注册帧戳推进、心跳、位姿/真值发布、指令处理
    fn build(&self, app: &mut App) {
        // 1. 创建共享内存生产者
        let publisher = match ShmPublisher::create() {
            Ok(p) => {
                info!("talos shm created");
                p
            }
            Err(e) => {
                error!("cannot create talos shm: {}", e);
                // 创建失败时不阻塞仿真器，直接返回
                return;
            }
        };

        // 包成 Arc<Mutex> 供多系统共享
        let publisher = Arc::new(Mutex::new(publisher));

        // 2. 构造图像采集配置
        let capture_config = CaptureConfig {
            width: self.config.width,
            height: self.config.height,
            texture_format: self.config.texture_format,
            frame_kind: CapturedFrameKind::Rgb8,
        };

        // 3. 构造采集上下文，持有 publisher 与视场角
        let capture_context = TalosCaptureContext {
            publisher: publisher.clone(),
            fov_y: self.config.fov_y,
        };

        app.init_resource::<TalosFrameStamp>();

        // 注册图像采集子插件（内部会注册相机、渲染目标、采集系统）
        app.add_plugins(TalosCapturePlugin {
            config: capture_config,
            context: capture_context,
        });

        // 4. 尝试连接外部 C++ talos-cpp 程序
        match ShmSubscriber::connect() {
            Ok(subscriber) => {
                info!("connected to talos-cpp");
                app.insert_resource(ShmSubscriberRes(Arc::new(Mutex::new(subscriber))));
            }
            Err(_) => {
                // 连接失败不影响图像发布，仅无法接收云台指令
                info!("could not connect to talos-cpp");
            }
        }

        // 5. 注册运行时资源与 Last 阶段系统
        app.insert_resource(TalosEnabled(AtomicBool::new(true)));
        // 帧戳推进与心跳并行执行
        app.add_systems(Last, (advance_talos_frame_stamp, heartbeat_system));
        // 位姿发布必须在帧戳推进之后，确保使用本帧时间戳
        app.add_systems(
            Last,
            publish_talos_pose_system.after(advance_talos_frame_stamp),
        );
        // 真值发布在位姿发布之后，共享同一帧戳
        app.add_systems(
            Last,
            crate::talos::ground_truth::publish_ground_truth_system
                .after(publish_talos_pose_system),
        );
        // 指令处理仅在开启自瞄跟随时执行
        app.add_systems(
            Last,
            process_subscription
                .run_if(|enabled: Res<SubscribeAutoAim>| enabled.load(Ordering::Acquire)),
        );
    }
}

/// 处理来自 C++ talos-cpp 的云台指令
///
/// 算法步骤:
///   1. 取出 ShmSubscriberRes，若无则直接返回
///   2. 通过 recv_gimbal_cmd 读取最新指令，无新数据则返回
///   3. distance_m == -1.0 表示无效指令，跳过
///   4. fire_advice == 1 时排队执行 projectile_launch 开火
///   5. 将 yaw/pitch 写入 InfantryGimbal 数据组件
///   6. 根据枪口期望旋转与当前旋转计算增量，应用到云台 Transform
///
/// 参数:
///   - context: 可选的订阅者资源
///   - commands: 用于排队开火命令
///   - gimbal: 受控云台组件（单例查询）
///   - muzzle_offset: 枪口偏移组件（单例查询），用于计算旋转增量
fn process_subscription(
    context: Option<Res<ShmSubscriberRes>>,
    mut commands: Commands,
    gimbal: Single<
        (&mut Transform, &mut InfantryGimbal),
        (
            With<Controlled>,
            Without<InfantryChassis>,
            Without<InfantryLaunchOffset>,
        ),
    >,
    muzzle_offset: Single<
        (&GlobalTransform, &Transform),
        (With<InfantryLaunchOffset>, With<Controlled>),
    >,
) {
    let Some(ctx) = context else {
        return;
    };
    let (mut gimbal_transform, mut gimbal_data) = gimbal.into_inner();

    // 读取最新云台指令
    let Some(cmd) = recv_gimbal_cmd(&ctx) else {
        return;
    };
    // distance_m == -1.0 表示 C++ 端未识别到目标
    if cmd.distance_m == -1.0 {
        return;
    }
    // 开火建议：排队在 world 上执行 projectile_launch 系统
    if cmd.fire_advice == 1 {
        commands.queue(|w: &mut World| {
            w.run_system_once(projectile_launch).unwrap();
        });
    }
    // 注意 pitch 角度的符号与基准偏移：(-pitch - 90) 转弧度
    let yaw_f32 = (cmd.yaw_deg).to_radians();
    let pitch_f32 = (-cmd.pitch_deg - 90.0).to_radians();
    gimbal_data.local_yaw = yaw_f32;
    gimbal_data.pitch = pitch_f32;
    // 计算期望的云台姿态（YXZ 欧拉序）
    let expected_rotation = Quat::from_euler(EulerRot::YXZ, yaw_f32, pitch_f32, 0.0);
    let current_rotation = muzzle_offset.0.rotation();
    // delta = expected * current^-1，将当前姿态对齐到期望姿态
    let delta = expected_rotation * current_rotation.inverse();
    gimbal_transform.rotation = delta * gimbal_transform.rotation;
    //info!("yaw={} pitch={}", cmd.yaw_deg, cmd.pitch_deg);
}

/// 心跳系统：周期性更新共享内存中的心跳时间戳
///
/// C++ 端通过心跳判断仿真器是否存活。
fn heartbeat_system(context: Option<Res<TalosCaptureContext>>) {
    if let Some(ctx) = context {
        if let Ok(mut publisher) = ctx.publisher.lock() {
            publisher.update_heartbeat();
        }
    }
}

/// 发布位姿到共享内存
///
/// 提供 capture 模块之外的位姿发布入口，例如外部系统主动推送。
///
/// 参数:
///   - context: 采集上下文，持有 publisher
///   - index: 位姿通道索引
///   - position: 平移向量
///   - quaternion: 旋转四元数
///   - frame_seq: 帧序号
///   - timestamp_ns: 时间戳（纳秒）
pub fn publish_pose(
    context: &TalosCaptureContext,
    index: PoseIndex,
    position: [f32; 3],
    quaternion: [f32; 4],
    frame_seq: u64,
    timestamp_ns: u64,
) {
    if let Ok(mut publisher) = context.publisher.lock() {
        publisher.publish_pose(index, position, quaternion, frame_seq, timestamp_ns);
    }
}

/// 从订阅者资源接收云台指令
///
/// 封装锁获取与指令读取，调用方只需处理 Option 结果。
pub fn recv_gimbal_cmd(subscriber: &ShmSubscriberRes) -> Option<GimbalCmd> {
    subscriber.0.lock().ok()?.recv_gimbal_cmd()
}

/// Bevy → ROS 坐标系对齐矩阵
///
/// 将 Bevy 的 Y-up 右手系映射到 ROS 的 Z-up 右手系。
/// 列向量对应 ROS 系下 Bevy 各基向量的方向：
///   - Bevy X 轴 → ROS -Y 轴
///   - Bevy Y 轴 → ROS Z 轴
///   - Bevy Z 轴 → ROS -X 轴
pub const M_ALIGN_MAT3: Mat3 = Mat3::from_cols(
    Vec3::new(0.0, -1.0, 0.0), // M[0,0], M[1,0], M[2,0]
    Vec3::new(0.0, 0.0, 1.0),  // M[0,1], M[1,1], M[2,1]
    Vec3::new(-1.0, 0.0, 0.0), // M[0,2], M[1,2], M[2,2]
);

/// 将 Bevy Transform 整体转换到 ROS 坐标系
///
/// 同时对平移与旋转应用对齐矩阵。
#[inline]
pub fn to_ros(bevy_transform: Transform) -> Transform {
    let new_rotation = to_ros_quat(bevy_transform.rotation);
    let new_translation = to_ros_translation(bevy_transform.translation);
    Transform::from_translation(new_translation).with_rotation(new_rotation)
}

/// 将 Bevy 平移向量转换到 ROS 坐标系
pub fn to_ros_translation(vec3: Vec3) -> Vec3 {
    let align_rot_mat = M_ALIGN_MAT3;
    let new_translation = align_rot_mat * vec3;
    new_translation
}

/// 将 Bevy 四元数转换到 ROS 坐标系
///
/// 算法: q_ros = R_align * q_bevy * R_align^-1
/// 即通过对齐矩阵对四元数做共轭变换。
pub fn to_ros_quat(quat: Quat) -> Quat {
    let align_rot_mat = M_ALIGN_MAT3;
    let align_quat = Quat::from_mat3(&align_rot_mat);
    let new_rotation = align_quat * quat * align_quat.inverse();
    new_rotation
}
