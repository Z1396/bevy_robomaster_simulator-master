// 导入采集驱动、相机捕获底层能力、共享内存发布器
use crate::capture::{
    CameraFov, CaptureSource, ImageHandle, compute_camera_intrinsics,
    driver::{
        CameraCapturePlugin, CaptureConfig, CapturedFrame, CapturedFrameKind, GpuCaptureHandler,
        SnapshotAsync, SnapshotSync,
    },
    setup_capture_camera, setup_preview_window, sync_capture_camera,
};
// 战车业务组件
use crate::components::{Controlled, InfantryGimbal, InfantryLaunchOffset, SubscribeAutoAim};
// 数据集录制快照生成器（一边推talos实时流、一边保存数据集）
use crate::dataset::prelude::DatasetSnapshotCreator;
// 底盘观测帧资源、游戏系统执行阶段
use crate::systems::{ChassisObservationFrame, GameplaySystems};
// 坐标系转换工具：Bevy右手坐标系 → ROS标准坐标系
use crate::talos::plugin::{to_ros_quat, to_ros_translation};

// Bevy基础
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::*;
use bevy::render::{Extract, ExtractSchedule, RenderApp, RenderSystems};

use std::f32::consts::PI;
// 原子变量、跨线程锁、系统时间
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
// talos IPC共享内存库，负责共享内存读写
use talos_ipc::*;

/// 全局单调帧序号，进程全局唯一，原子类型保证主线程/渲染线程并发安全
/// 全程只会递增，永不回退，作为所有图像、位姿、观测数据的唯一时序ID
static FRAME_SEQ: AtomicU64 = AtomicU64::new(0);

/// 每一帧的时序标记资源，存在MainApp主线世界
/// 每一帧开头统一更新，本帧所有图像、位姿共用同一套 frame_seq + timestamp，保障时序对齐
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct TalosFrameStamp {
    pub frame_seq: u64,        // 全局递增帧编号
    pub timestamp_ns: u64,     // UNIX时间戳(纳秒)
}

/// 每帧开头执行：更新全局帧戳
pub fn advance_talos_frame_stamp(mut stamp: ResMut<TalosFrameStamp>) {
    // 原子自增，Relaxed内存序，仅保证自增原子性，性能更高
    stamp.frame_seq = FRAME_SEQ.fetch_add(1, Ordering::Relaxed);
    stamp.timestamp_ns = now_ns();
}

/// 【跨App同步核心快照】
/// 在 ExtractSchedule 阶段，从MainApp拷贝至RenderApp渲染世界
/// 渲染线程GPU捕获图像时，读取这份快照，保证「图像的帧号、时间戳 = 主线位姿的帧号、时间戳」
#[derive(Resource, Clone, Default)]
pub struct ExtractedPoseData {
    pub frame_seq: u64,
    pub timestamp_ns: u64,
    pub valid: bool,    // true=云台、相机、枪口实体全部存在，可以正常采集
}

/// 主线组装完毕的完整位姿数据包，最终发布至共享内存
#[derive(Clone)]
struct CapturedPoseData {
    gimbal_ros: [f32; 3],        // 云台世界坐标（ROS坐标系xyz）
    gimbal_quat: [f32; 4],       // 云台姿态四元数 wxyz (ROS标准顺序)
    muzzle_rel: [f32; 3],        // 枪口相对于云台的局部偏移
    camera_rel: [f32; 3],        // 采集相机相对于云台的局部偏移
    chassis_observation: ChassisObservation, // 底盘运动观测（速度、加速度、陀螺仪、车轮转速等IMU级数据）
}

/// 获取当前系统时间，单位：纳秒
fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// 同步阶段快照载体：渲染线程捕获图像前，先构建同步快照，绑定当前帧时序信息
/// 实现 SnapshotSync trait，是GPU捕获生命周期的中间状态
struct TalosSnapshotSync {
    frame_seq: u64,
    timestamp_ns: u64,
}

impl SnapshotSync for TalosSnapshotSync {
    /// GPU准备捕获画面完成后，转为异步等待状态，持有共享内存发布器上下文
    fn captured(
        self: Box<Self>,
        world: &mut DeferredWorld,
        _config: &CaptureConfig,
    ) -> Box<dyn SnapshotAsync> {
        // 取出渲染App里存放的共享内存发布句柄
        let ctx = world.resource::<TalosCaptureContextShared>().0.clone();

        Box::new(TalosSnapshot {
            ctx,
            frame_seq: self.frame_seq,
            timestamp_ns: self.timestamp_ns,
        })
    }
}

/// 异步快照：等待GPU完整读出RGB像素数据，拿到像素后写入共享内存
struct TalosSnapshot {
    ctx: Arc<Mutex<ShmPublisher>>, // 共享内存发布器（多线程互斥锁保护）
    frame_seq: u64,
    timestamp_ns: u64,
}

impl SnapshotAsync for TalosSnapshot {
    /// GPU回调：拿到渲染完毕的图像原始像素，校验合法性后写入共享内存
    fn captured(&mut self, frame: CapturedFrame<'_>) {
        // 只处理RGB8格式图像，丢弃其他格式
        if frame.kind != CapturedFrameKind::Rgb8 {
            return;
        }

        // 校验像素字节长度 = 宽×高×3通道(RGB)
        let expected_size = (frame.width * frame.height * 3) as usize;
        if frame.data.len() != expected_size {
            warn!(
                "图像大小不匹配: expected {} bytes, got {} bytes",
                expected_size,
                frame.data.len()
            );
            return;
        }

        // 分辨率强校验，避免分辨率变动导致外部算法解析错乱
        if frame.width != IMAGE_WIDTH || frame.height != IMAGE_HEIGHT {
            warn!(
                "image resolution mismatched: expected {}x{}, got {}x{}",
                IMAGE_WIDTH, IMAGE_HEIGHT, frame.width, frame.height
            );
            return;
        }

        // 上锁，将RGB图像二进制写入共享内存
        if let Ok(mut publisher) = self.ctx.lock() {
            publisher.publish_image(frame.data, self.frame_seq, self.timestamp_ns);
        }
    }
}

/// GPU捕获回调生成器，每一帧渲染前判定：本帧是否需要采集图像快照
#[derive(Default)]
struct TalosSnapshotCreator {}

impl GpuCaptureHandler for TalosSnapshotCreator {
    fn captured(&self, world: &World) -> Option<Box<dyn SnapshotSync>> {
        // 读取渲染App内从主线拷贝过来的位姿快照
        let extracted = world.get_resource::<ExtractedPoseData>()?;
        // 位姿组件缺失，放弃采集当前帧图像
        if !extracted.valid {
            return None;
        }

        // 创建同步快照，绑定帧号时间戳，进入捕获生命周期
        Some(Box::new(TalosSnapshotSync {
            frame_seq: extracted.frame_seq,
            timestamp_ns: extracted.timestamp_ns,
        }))
    }
}

/// 注入到RenderApp渲染世界的共享内存上下文
/// 渲染线程没有权限访问MainApp资源，因此提前克隆一份ShmPublisher放入RenderApp
#[derive(Resource, Clone, Deref, DerefMut)]
pub struct TalosCaptureContextShared(pub Arc<Mutex<ShmPublisher>>);

/// 主线程采集上下文资源，主线发布位姿、相机内参使用
#[derive(Resource, Clone)]
pub struct TalosCaptureContext {
    pub publisher: Arc<Mutex<ShmPublisher>>,
    pub fov_y: f32, // 相机垂直视场角，用于计算相机内参fx/fy/cx/cy
}

/// 主线系统：每一帧组装云台、枪口、相机、底盘观测数据，发布到位姿共享内存通道
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
    // 未初始化采集上下文直接退出
    let Some(ctx) = context else {
        return;
    };
    // 任一核心实体缺失，跳过本帧位姿发布
    let Ok(cam_transform) = camera.single() else {
        return;
    };
    let Ok(gimbal_transform) = gimbal.single() else {
        return;
    };
    let Ok((muzzle_global, muzzle_local)) = muzzle_offset.single() else {
        return;
    };

    // 把各个位姿转换为ROS坐标系格式
    let pose = captured_pose_data(
        cam_transform,
        gimbal_transform,
        muzzle_global,
        muzzle_local,
        &chassis_obs,
        frame_stamp.frame_seq,
        frame_stamp.timestamp_ns,
    );

    // 上锁发布所有位姿数据
    if let Ok(mut publisher) = ctx.publisher.lock() {
        publish_pose_data(
            &mut publisher,
            frame_stamp.frame_seq,
            frame_stamp.timestamp_ns,
            &pose,
        );
        // 附加运行状态：当前是否开启自动瞄准
        publisher.publish_runtime_state(RuntimeState {
            timestamp_ns: frame_stamp.timestamp_ns,
            following: u8::from(following.load(Ordering::Acquire)),
            _pad: [0; 55], // 内存对齐占位
        });
    }
}

/// Talos采集主插件，整合相机捕获、跨App数据同步、相机初始化、时序系统调度
pub struct TalosCapturePlugin {
    pub config: CaptureConfig,    // 采集配置：分辨率、纹理格式、渲染目标
    pub context: TalosCaptureContext,
}

impl Plugin for TalosCapturePlugin {
    fn build(&self, app: &mut App) {
        // 挂载双捕获处理器：1.talos实时共享内存流  2.dataset数据集录制保存
        let (plugin, render_target_handle) = CameraCapturePlugin::new(
            app,
            self.config.clone(),
            vec![
                Box::new(TalosSnapshotCreator::default()),
                Box::new(DatasetSnapshotCreator::default()),
            ],
        );

        {
            // 插件启动时，一次性计算相机内参fx/fy/cx/cy并写入共享内存
            // C++算法进程启动时读取一次相机内参即可完成标定
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
                distortion: [0.0; 5], // 仿真无畸变，畸变系数全部置0
                width: intrinsics.width,
                height: intrinsics.height,
                _pad: [0; 24],
            });
        }

        // 安装底层相机捕获插件，注入全局资源
        app.add_plugins(plugin)
            .insert_resource(ImageHandle(render_target_handle))
            .insert_resource(CameraFov(self.context.fov_y))
            .insert_resource(self.context.clone())
            // 启动阶段：生成专用采集相机、预览窗口
            .add_systems(Startup, setup_capture_camera)
            .add_systems(Startup, setup_preview_window)
            // 每一帧更新采集相机姿态，跟随云台视角；时序约束：玩法相机更新完成后、正式渲染前同步
            .add_systems(
                Update,
                sync_capture_camera
                    .after(GameplaySystems::Camera)
                    .before(RenderSystems::Render),
            );

        // ========== 关键：向渲染子App注入资源，注册抽取系统 ==========
        app.sub_app_mut(RenderApp)
            .insert_resource(TalosCaptureContextShared(self.context.publisher.clone()))
            .insert_resource(self.context.clone())
            .insert_resource(ExtractedPoseData::default())
            // ExtractSchedule 是Bevy专门用于「主线数据拷贝至渲染线程」的阶段
            // 每一帧渲染前自动执行 extract_pose_data，把主线位姿快照同步进RenderApp
            .add_systems(ExtractSchedule, extract_pose_data);
    }
}

/// ExtractSchedule 专属系统
/// 从MainApp主线世界抽取帧号、时间戳、相机/云台/枪口存在性标记，存入RenderApp的ExtractedPoseData
/// 实现「渲染线程拿到的图像，和主线发布的位姿严格属于同一帧时序」
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
    // 拷贝本帧时序信息
    pose_data.frame_seq = frame_stamp.frame_seq;
    pose_data.timestamp_ns = frame_stamp.timestamp_ns;

    // 校验三大核心实体是否存在，任意缺失标记本帧无效
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

    // 校验通过，标记快照有效，渲染线程可以捕获图像
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

/// 坐标转换组装函数：Bevy世界坐标系 → ROS坐标系，打包完整位姿结构体
fn captured_pose_data(
    cam_transform: &GlobalTransform,
    gimbal_transform: &GlobalTransform,
    muzzle_global: &GlobalTransform,
    muzzle_local: &Transform,
    chassis_obs: &ChassisObservationFrame,
    frame_seq: u64,
    timestamp_ns: u64,
) -> CapturedPoseData {
    // reparented_to：计算子物体相对于父物体的局部位姿
    let cam_rel = cam_transform.reparented_to(gimbal_transform);
    let muzzle_rel = muzzle_global.reparented_to(gimbal_transform);

    // 云台姿态叠加枪口旋转 + 90°欧拉补偿，对齐ROS朝向惯例
    let gimbal_rot = gimbal_transform.rotation()
        * muzzle_local.rotation
        * Quat::from_euler(EulerRot::ZYX, 0.0, 0.0, PI / 2.0);

    // 全部转为ROS右手坐标系格式
    let gimbal_ros = to_ros_translation(gimbal_transform.translation());
    let gimbal_rot = to_ros_quat(gimbal_rot);
    let muzzle = to_ros_translation(muzzle_rel.translation);
    let camera = to_ros_translation(cam_rel.translation);

    CapturedPoseData {
        gimbal_ros: [gimbal_ros.x, gimbal_ros.y, gimbal_ros.z],
        gimbal_quat: [gimbal_rot.w, gimbal_rot.x, gimbal_rot.y, gimbal_rot.z],
        muzzle_rel: [muzzle.x, muzzle.y, muzzle.z],
        camera_rel: [camera.x, camera.y, camera.z],
        // 灌入底盘运动观测全量数据
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

/// 统一发布多路位姿话题至共享内存，分通道隔离数据，外部算法按需订阅
fn publish_pose_data(
    publisher: &mut ShmPublisher,
    frame_seq: u64,
    timestamp_ns: u64,
    pose: &CapturedPoseData,
) {
    // 通道1：Odom里程计位姿，只发布云台世界坐标，旋转置单位四元数
    publisher.publish_pose(
        PoseIndex::Odom,
        pose.gimbal_ros,
        [1.0, 0.0, 0.0, 0.0],
        frame_seq,
        timestamp_ns,
    );

    // 通道2：Gimbal云台姿态，平移置0，只发布旋转四元数
    publisher.publish_pose(
        PoseIndex::Gimbal,
        [0.0, 0.0, 0.0],
        pose.gimbal_quat,
        frame_seq,
        timestamp_ns,
    );

    // 通道3：枪口相对云台偏移
    publisher.publish_pose(
        PoseIndex::Muzzle,
        pose.muzzle_rel,
        [1.0, 0.0, 0.0, 0.0],
        frame_seq,
        timestamp_ns,
    );

    // 通道4：采集相机相对云台偏移
    publisher.publish_pose(
        PoseIndex::Camera,
        pose.camera_rel,
        [1.0, 0.0, 0.0, 0.0],
        frame_seq,
        timestamp_ns,
    );

    // 通道5：完整底盘观测结构体（速度、加速度、陀螺仪、车轮转速）
    let mut observation = pose.chassis_observation;
    observation.frame_seq = frame_seq;
    observation.timestamp_ns = timestamp_ns;
    publisher.publish_chassis_observation(observation);

    // 兼容旧版算法程序：将底盘运动摘要塞进 ChassisObservation 预留Aux字段
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