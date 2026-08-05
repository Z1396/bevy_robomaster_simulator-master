// ====================================================================
// 模块名: talos::capture
// 作用:   将 Bevy 渲染管线的图像与位姿数据采集并发布到 talos 共享内存
// 职责:   1. 维护全局帧序号与时间戳（TalosFrameStamp）
//         2. 在 ExtractSchedule 阶段从 MainApp 抽取位姿数据到 RenderApp
//         3. 在渲染线程通过 GPU 捕获拿到 RGB 像素并发布到共享内存
//         4. 在主线程发布位姿、底盘观测、运行时状态等
// 说明:   关键点是图像捕获发生在 RenderApp 的 GPU 捕获回调中，而位姿
//         等数据来自 MainApp。为保证时间戳与帧序号同步，二者必须共享
//         同一份 ExtractedPoseData 快照，由 extract_pose_data 在
//         ExtractSchedule 中写入。
// ====================================================================

use crate::capture::{
    CameraFov, CaptureSource, ImageHandle, compute_camera_intrinsics,
    driver::{
        CameraCapturePlugin, CaptureConfig, CapturedFrame, CapturedFrameKind, GpuCaptureHandler,
        SnapshotAsync, SnapshotSync,
    },
    setup_capture_camera, setup_preview_window, sync_capture_camera,
};
use crate::components::{Controlled, InfantryGimbal, InfantryLaunchOffset, SubscribeAutoAim};
use crate::dataset::prelude::DatasetSnapshotCreator;
use crate::systems::{ChassisObservationFrame, GameplaySystems};
use crate::talos::plugin::{to_ros_quat, to_ros_translation};
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::*;
use bevy::render::{Extract, ExtractSchedule, RenderApp, RenderSystems};
use std::f32::consts::PI;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use talos_ipc::*;

/// 全局帧序号生成器，进程级单调递增
///
/// 使用 AtomicU64 保证线程安全，跨主线程与渲染线程共享。
static FRAME_SEQ: AtomicU64 = AtomicU64::new(0);

/// 当前帧的序号与时间戳，作为 Bevy 资源在系统中传递
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct TalosFrameStamp {
    /// 帧序号，由 FRAME_SEQ fetch_add 生成
    pub frame_seq: u64,
    /// 时间戳（纳秒，UNIX epoch）
    pub timestamp_ns: u64,
}

/// 推进帧戳到下一帧
///
/// 每帧调用一次，原子地递增全局帧序号并刷新时间戳。
/// 该资源会被 ExtractSchedule 读取，进而传递给 RenderApp。
pub fn advance_talos_frame_stamp(mut stamp: ResMut<TalosFrameStamp>) {
    stamp.frame_seq = FRAME_SEQ.fetch_add(1, Ordering::Relaxed);
    stamp.timestamp_ns = now_ns();
}

/// 从 MainApp 抽取到 RenderApp 的位姿数据快照
///
/// 用于在 GPU 捕获回调中拿到与图像同帧的位姿信息，保证时间戳一致。
#[derive(Resource, Clone, Default)]
pub struct ExtractedPoseData {
    /// 帧序号
    pub frame_seq: u64,
    /// 时间戳（纳秒）
    pub timestamp_ns: u64,
    /// 是否有效（相机/云台/枪口组件是否齐全）
    pub valid: bool,
}

/// 帧快照时刻捕获的位姿数据集合
///
/// 包含云台、枪口、相机相对云台的位姿，以及底盘观测，
/// 在主线程组装后通过 publish_pose_data 一次性发布到共享内存。
#[derive(Clone)]
struct CapturedPoseData {
    /// 云台在 ROS 坐标系下的平移
    gimbal_ros: [f32; 3],
    /// 云台在 ROS 坐标系下的四元数 (w, x, y, z)
    gimbal_quat: [f32; 4],
    /// 枪口相对云台的平移（ROS 系）
    muzzle_rel: [f32; 3],
    /// 相机相对云台的平移（ROS 系）
    camera_rel: [f32; 3],
    /// 底盘观测快照
    chassis_observation: ChassisObservation,
}

/// 获取当前 UNIX 纳秒时间戳
fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// 同步阶段快照：携带帧序号与时间戳，等待 GPU 捕获完成
struct TalosSnapshotSync {
    /// 帧序号
    frame_seq: u64,
    /// 时间戳（纳秒）
    timestamp_ns: u64,
}

impl SnapshotSync for TalosSnapshotSync {
    /// 当 GPU 捕获同步完成时调用，把快照转为异步等待状态
    ///
    /// 算法步骤:
    ///   1. 从 DeferredWorld 取出 RenderApp 中共享的 publisher 上下文
    ///   2. 构造 TalosSnapshot 等待图像像素就绪
    fn captured(
        self: Box<Self>,
        world: &mut DeferredWorld,
        _config: &CaptureConfig,
    ) -> Box<dyn SnapshotAsync> {
        let ctx = world.resource::<TalosCaptureContextShared>().0.clone();

        Box::new(TalosSnapshot {
            ctx,
            frame_seq: self.frame_seq,
            timestamp_ns: self.timestamp_ns,
        })
    }
}

/// 异步快照：持有 publisher 与帧信息，等待 GPU 像素数据
struct TalosSnapshot {
    /// 共享内存生产者
    ctx: Arc<Mutex<ShmPublisher>>,
    /// 帧序号
    frame_seq: u64,
    /// 时间戳（纳秒）
    timestamp_ns: u64,
}

impl SnapshotAsync for TalosSnapshot {
    /// 当 GPU 捕获到完整像素数据时调用，将其发布到共享内存
    ///
    /// 算法步骤:
    ///   1. 校验帧格式必须为 Rgb8
    ///   2. 校验数据长度与分辨率匹配
    ///   3. 加锁 publisher 并调用 publish_image
    fn captured(&mut self, frame: CapturedFrame<'_>) {
        if frame.kind != CapturedFrameKind::Rgb8 {
            return;
        }

        let expected_size = (frame.width * frame.height * 3) as usize;
        if frame.data.len() != expected_size {
            warn!(
                "图像大小不匹配: expected {} bytes, got {} bytes",
                expected_size,
                frame.data.len()
            );
            return;
        }

        if frame.width != IMAGE_WIDTH || frame.height != IMAGE_HEIGHT {
            warn!(
                "image reesolution mismatched: expected {}x{}, got {}x{}",
                IMAGE_WIDTH, IMAGE_HEIGHT, frame.width, frame.height
            );
            return;
        }

        if let Ok(mut publisher) = self.ctx.lock() {
            publisher.publish_image(frame.data, self.frame_seq, self.timestamp_ns);
        }
    }
}

/// 快照创建器：决定是否为本帧生成 talos 快照
#[derive(Default)]
struct TalosSnapshotCreator {}

impl GpuCaptureHandler for TalosSnapshotCreator {
    /// 在每帧渲染前检查是否需要采集 talos 快照
    ///
    /// 时间戳、帧序号与位姿必须来自同一份 ExtractSchedule 快照，
    /// 否则会出现图像与位姿时间戳不一致的问题。
    fn captured(&self, world: &World) -> Option<Box<dyn SnapshotSync>> {
        let extracted = world.get_resource::<ExtractedPoseData>()?;
        if !extracted.valid {
            // 位姿数据无效时不生成快照
            return None;
        }

        Some(Box::new(TalosSnapshotSync {
            frame_seq: extracted.frame_seq,
            timestamp_ns: extracted.timestamp_ns,
        }))
    }
}

/// RenderApp 中共享的 publisher 上下文
///
/// 用于在渲染线程的 GPU 捕获回调中访问共享内存生产者。
#[derive(Resource, Clone, Deref, DerefMut)]
pub struct TalosCaptureContextShared(pub Arc<Mutex<ShmPublisher>>);

/// 主线程的采集上下文资源
///
/// 持有 publisher 与视场角，供 publish_talos_pose_system 等系统使用。
#[derive(Resource, Clone)]
pub struct TalosCaptureContext {
    /// 共享内存生产者
    pub publisher: Arc<Mutex<ShmPublisher>>,
    /// 垂直视场角（弧度），用于计算相机内参
    pub fov_y: f32,
}

/// talos 图像与位姿采集插件
///
/// 内部注册 CameraCapturePlugin 处理 GPU 捕获，并设置相机内参、
/// 注册渲染目标、配置 RenderApp 的位姿抽取系统。
pub struct TalosCapturePlugin {
    /// 采集配置（分辨率、纹理格式等）
    pub config: CaptureConfig,
    /// 采集上下文（publisher 与视场角）
    pub context: TalosCaptureContext,
}

/// 主线程位姿发布系统
///
/// 算法步骤:
///   1. 取出采集上下文，无则直接返回
///   2. 单例查询相机、云台、枪口的 GlobalTransform/Transform
///   3. 调用 captured_pose_data 组装位姿数据
///   4. 加锁 publisher，调用 publish_pose_data 发布全部位姿
///   5. 额外发布运行时状态（是否跟随）
pub fn publish_talos_pose_system(
    context: Option<Res<TalosCaptureContext>>,
    frame_stamp: Res<TalosFrameStamp>,
    camera: Query<&GlobalTransform, With<CaptureSource>>,
    gimbal: Query<&GlobalTransform, (With<Controlled>, With<InfantryGimbal>)>,
    muzzle_offset: Query<
        (&GlobalTransform, &Transform),
        (With<InfantryLaunchOffset>, With<Controlled>),
    >,
    chassis_obs: Res<ChassisObservationFrame>,
    following: Res<SubscribeAutoAim>,
) {
    let Some(ctx) = context else {
        return;
    };
    // 单例查询：相机、云台、枪口组件必须存在
    let Ok(cam_transform) = camera.single() else {
        return;
    };
    let Ok(gimbal_transform) = gimbal.single() else {
        return;
    };
    let Ok((muzzle_global, muzzle_local)) = muzzle_offset.single() else {
        return;
    };

    // 组装本帧位姿数据
    let pose = captured_pose_data(
        cam_transform,
        gimbal_transform,
        muzzle_global,
        muzzle_local,
        &chassis_obs,
        frame_stamp.frame_seq,
        frame_stamp.timestamp_ns,
    );

    if let Ok(mut publisher) = ctx.publisher.lock() {
        // 发布全部位姿通道
        publish_pose_data(
            &mut publisher,
            frame_stamp.frame_seq,
            frame_stamp.timestamp_ns,
            &pose,
        );
        // 发布运行时状态：是否处于自瞄跟随
        publisher.publish_runtime_state(RuntimeState {
            timestamp_ns: frame_stamp.timestamp_ns,
            following: u8::from(following.load(Ordering::Acquire)),
            _pad: [0; 55],
        });
    }
}

impl Plugin for TalosCapturePlugin {
    /// 构建采集插件
    ///
    /// 算法步骤:
    ///   1. 创建 CameraCapturePlugin，挂载 TalosSnapshotCreator 与 Dataset 快照
    ///   2. 加锁 publisher，根据视场角计算并写入相机内参
    ///   3. 注册相机、预览窗口、同步系统等
    ///   4. 在 RenderApp 注入共享上下文与 ExtractedPoseData，注册 extract_pose_data
    fn build(&self, app: &mut App) {
        // 注册两个 GPU 捕获处理器：talos 与 dataset
        let (plugin, render_target_handle) = CameraCapturePlugin::new(
            app,
            self.config.clone(),
            vec![
                Box::new(TalosSnapshotCreator::default()),
                Box::new(DatasetSnapshotCreator::default()),
            ],
        );

        {
            // 计算相机内参并写入共享内存，C++ 端启动后读取
            let mut publisher = self.context.publisher.lock().unwrap();
            let intrinsics = compute_camera_intrinsics(
                self.config.width,
                self.config.height,
                self.context.fov_y,
            );

            publisher.set_camera_info(CameraInfo {
                timestamp_ns: now_ns(),
                fx: intrinsics.fx,
                fy: intrinsics.fy,
                cx: intrinsics.cx,
                cy: intrinsics.cy,
                distortion: [0.0; 5],
                width: intrinsics.width,
                height: intrinsics.height,
                _pad: [0; 24],
            });
        }

        app.add_plugins(plugin)
            .insert_resource(ImageHandle(render_target_handle))
            .insert_resource(CameraFov(self.context.fov_y))
            .insert_resource(self.context.clone())
            .add_systems(Startup, setup_capture_camera)
            .add_systems(Startup, setup_preview_window)
            // 同步相机在玩法相机系统之后、渲染之前
            .add_systems(
                Update,
                sync_capture_camera
                    .after(GameplaySystems::Camera)
                    .before(RenderSystems::Render),
            );

        // 在 RenderApp 注入共享资源并注册位姿抽取系统
        app.sub_app_mut(RenderApp)
            .insert_resource(TalosCaptureContextShared(self.context.publisher.clone()))
            .insert_resource(self.context.clone())
            .insert_resource(ExtractedPoseData::default())
            .add_systems(ExtractSchedule, extract_pose_data);
    }
}

/// Extract pose data from MainApp to RenderApp
/// 在 ExtractSchedule 阶段把主世界的位姿与帧戳抽取到 RenderApp
///
/// 这样 GPU 捕获回调拿到的位姿与图像严格同帧，避免时间戳漂移。
fn extract_pose_data(
    mut pose_data: ResMut<ExtractedPoseData>,
    frame_stamp: Extract<Res<TalosFrameStamp>>,
    camera: Extract<Query<&GlobalTransform, With<CaptureSource>>>,
    gimbal: Extract<Query<&GlobalTransform, (With<Controlled>, With<InfantryGimbal>)>>,
    muzzle_offset: Extract<
        Query<(&GlobalTransform, &Transform), (With<InfantryLaunchOffset>, With<Controlled>)>,
    >,
    chassis_obs: Extract<Res<ChassisObservationFrame>>,
) {
    pose_data.frame_seq = frame_stamp.frame_seq;
    pose_data.timestamp_ns = frame_stamp.timestamp_ns;

    // 任一单例查询失败都标记为无效，跳过本帧图像发布
    let Ok(cam_transform) = camera.single() else {
        pose_data.valid = false;
        return;
    };
    let Ok(gimbal_transform) = gimbal.single() else {
        pose_data.valid = false;
        return;
    };
    let Ok((muzzle_global, muzzle_local)) = muzzle_offset.single() else {
        pose_data.valid = false;
        return;
    };

    // 此处调用 captured_pose_data 仅为校验组件齐全，结果暂不使用
    let _pose = captured_pose_data(
        cam_transform,
        gimbal_transform,
        muzzle_global,
        muzzle_local,
        &chassis_obs,
        pose_data.frame_seq,
        pose_data.timestamp_ns,
    );
    pose_data.valid = true;
}

/// 组装本帧的位姿数据
///
/// 算法步骤:
///   1. 计算相机相对云台、枪口相对云台的局部变换
///   2. 云台旋转叠加枪口局部旋转与一个固定的 ZYX 偏航补偿（PI/2）
///   3. 全部通过 to_ros_translation / to_ros_quat 转到 ROS 坐标系
///   4. 填充 ChassisObservation 字段（速度、加速度、IMU 等）
///
/// 参数:
///   - cam_transform: 相机全局变换
///   - gimbal_transform: 云台全局变换
///   - muzzle_global: 枪口全局变换
///   - muzzle_local: 枪口局部变换
///   - chassis_obs: 底盘观测资源
///   - frame_seq: 帧序号
///   - timestamp_ns: 时间戳（纳秒）
///
/// 返回: 组装好的 CapturedPoseData
fn captured_pose_data(
    cam_transform: &GlobalTransform,
    gimbal_transform: &GlobalTransform,
    muzzle_global: &GlobalTransform,
    muzzle_local: &Transform,
    chassis_obs: &ChassisObservationFrame,
    frame_seq: u64,
    timestamp_ns: u64,
) -> CapturedPoseData {
    // 计算相机/枪口相对云台的局部平移
    let cam_rel = cam_transform.reparented_to(gimbal_transform);
    let muzzle_rel = muzzle_global.reparented_to(gimbal_transform);

    // 云台旋转 = 云台自身旋转 * 枪口局部旋转 * 固定 ZYX 偏航补偿
    let gimbal_rot = gimbal_transform.rotation()
        * muzzle_local.rotation
        * Quat::from_euler(EulerRot::ZYX, 0.0, 0.0, PI / 2.0);

    // 全部转到 ROS 坐标系
    let gimbal_ros = to_ros_translation(gimbal_transform.translation());
    let gimbal_rot = to_ros_quat(gimbal_rot);
    let muzzle = to_ros_translation(muzzle_rel.translation);
    let camera = to_ros_translation(cam_rel.translation);

    CapturedPoseData {
        gimbal_ros: [gimbal_ros.x, gimbal_ros.y, gimbal_ros.z],
        // 四元数顺序为 (w, x, y, z)
        gimbal_quat: [gimbal_rot.w, gimbal_rot.x, gimbal_rot.y, gimbal_rot.z],
        muzzle_rel: [muzzle.x, muzzle.y, muzzle.z],
        camera_rel: [camera.x, camera.y, camera.z],
        chassis_observation: ChassisObservation {
            frame_seq,
            timestamp_ns,
            dt_s: chassis_obs.dt_s,
            v_body: [chassis_obs.v_body.x, chassis_obs.v_body.y],
            wz_radps: chassis_obs.wz_radps,
            wheel_linear_mps: chassis_obs.wheel_linear_mps,
            wheel_angular_radps: chassis_obs.wheel_angular_radps,
            a_body: [chassis_obs.a_body.x, chassis_obs.a_body.y],
            alpha_z_radps2: chassis_obs.alpha_z_radps2,
            rpy_rad: [
                chassis_obs.rpy_rad.x,
                chassis_obs.rpy_rad.y,
                chassis_obs.rpy_rad.z,
            ],
            gyro_xyz_radps: [
                chassis_obs.gyro_xyz_radps.x,
                chassis_obs.gyro_xyz_radps.y,
                chassis_obs.gyro_xyz_radps.z,
            ],
            accel_xyz_mps2: [
                chassis_obs.accel_xyz_mps2.x,
                chassis_obs.accel_xyz_mps2.y,
                chassis_obs.accel_xyz_mps2.z,
            ],
            _pad: [0; 16],
        },
    }
}

/// 一次性发布所有位姿通道与底盘观测
///
/// 算法步骤:
///   1. 发布 Odom 位姿（云台在里程计系的位置，四元数置为单位）
///   2. 发布 Gimbal 位姿（云台旋转，平移置零）
///   3. 发布 Muzzle 位姿（枪口相对云台平移）
///   4. 发布 Camera 位姿（相机相对云台平移）
///   5. 发布 ChassisObservation 结构体
///   6. 兼容旧通道：通过 publish_pose_with_aux 把底盘观测摘要塞进 PoseIndex::ChassisObservation
fn publish_pose_data(
    publisher: &mut ShmPublisher,
    frame_seq: u64,
    timestamp_ns: u64,
    pose: &CapturedPoseData,
) {
    // 1. 里程计位姿：仅平移，旋转为单位四元数
    publisher.publish_pose(
        PoseIndex::Odom,
        pose.gimbal_ros,
        [1.0, 0.0, 0.0, 0.0],
        frame_seq,
        timestamp_ns,
    );

    // 2. 云台位姿：仅旋转，平移置零
    publisher.publish_pose(
        PoseIndex::Gimbal,
        [0.0, 0.0, 0.0],
        pose.gimbal_quat,
        frame_seq,
        timestamp_ns,
    );

    // 3. 枪口位姿：相对云台的平移
    publisher.publish_pose(
        PoseIndex::Muzzle,
        pose.muzzle_rel,
        [1.0, 0.0, 0.0, 0.0],
        frame_seq,
        timestamp_ns,
    );

    // 4. 相机位姿：相对云台的平移
    publisher.publish_pose(
        PoseIndex::Camera,
        pose.camera_rel,
        [1.0, 0.0, 0.0, 0.0],
        frame_seq,
        timestamp_ns,
    );

    // 5. 发布完整的底盘观测结构体
    let mut observation = pose.chassis_observation;
    observation.frame_seq = frame_seq;
    observation.timestamp_ns = timestamp_ns;
    publisher.publish_chassis_observation(observation);

    // Legacy compatibility path for consumers still reading pose slot 4.
    // 6. 旧版兼容通道：把底盘观测摘要塞进 PoseIndex::ChassisObservation 的 aux 字段
    publisher.publish_pose_with_aux(
        PoseIndex::ChassisObservation,
        [
            observation.v_body[0],
            observation.v_body[1],
            observation.wz_radps,
        ],
        observation.wheel_angular_radps,
        [
            observation.a_body[0],
            observation.a_body[1],
            observation.alpha_z_radps2,
            observation.dt_s,
        ],
        frame_seq,
        timestamp_ns,
    );
}
