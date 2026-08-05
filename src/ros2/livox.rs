// ============================================================================
// 模块名：ros2::livox
// 作  用：ROS2 Livox 雷达点云发布插件，将深度图转换为 Livox 风格点云
// 职  责：
//   1. 注册深度相机捕获插件，配置 Depth32Float 渲染目标
//   2. 在 GPU 捕获完成后，将深度图反投影为 3D 点云
//   3. 应用 Livox 坐标系约定（x=depth, y=-x_cam, z=-y_cam）与线号映射
//   4. 按配置频率限速发布 sensor_msgs/PointCloud2 消息
//   5. 字段布局：x/y/z (float32) + intensity (float32) + tag (uint8) + line (uint8)
// ============================================================================

use crate::capture::compute_camera_intrinsics;
use crate::capture::depth::{
    DepthCameraSettings, DepthTextureCopyPlugin, setup_depth_capture_camera,
    sync_depth_capture_camera,
};
use crate::capture::driver::{
    CameraCapturePlugin, CaptureConfig, CapturedFrame, CapturedFrameKind, GpuCaptureHandler,
    SnapshotAsync, SnapshotSync,
};
use crate::ros2::topic::{LivoxPointCloudTopic, TopicPublisher};
use crate::systems::GameplaySystems;
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::*;
use bevy::render::{RenderApp, RenderSystems};
use r2r::Clock;
use r2r::sensor_msgs::msg::{PointCloud2, PointField};
use r2r::std_msgs::msg::Header;
use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// 渲染应用共享的 Livox 上下文资源：包装 `Arc<RosLivoxContext>`，
/// 用于在 RenderApp 中跨线程访问点云发布器与时钟。
#[derive(Resource, Clone, Deref, DerefMut)]
pub struct RosLivoxContextShared(pub Arc<RosLivoxContext>);

/// Livox 雷达上下文：持有深度相机参数、点云发布器与频率控制状态。
///
/// 作为 Bevy 资源插入主 App，同时克隆一份插入 RenderApp，
/// 供同步/异步快照处理器在渲染线程中访问。
#[derive(Resource, Clone)]
pub struct RosLivoxContext {
    /// ROS2 时钟（基于 SystemTime），用于生成消息时间戳
    pub clock: Arc<Mutex<Clock>>,
    /// 点云消息的 frame_id（如 "livox_frame"）
    pub frame_id: String,
    /// 相机垂直 FOV（弧度），用于反投影时计算内参
    pub fov_y: f32,
    /// 近裁剪面（米），用于线性化深度
    pub near: f32,
    /// 远裁剪面（米），超出此距离的点云被丢弃
    pub far: f32,
    /// 发布周期（纳秒）= 1e9 / 发布频率，用于限速
    pub publish_period_ns: u64,
    /// 每次发布的目标点数（由每秒点数 / 发布频率得出）
    pub points_per_publish: usize,
    /// 雷达线数（用于将像素行映射到扫描线号）
    pub line_num: u8,
    /// 默认 tag 字段值（Livox 点云的标记位）
    pub tag_default: u8,
    /// 默认 intensity 字段值（反射强度，仿真中为常量）
    pub intensity_default: f32,
    /// 点云发布器（/livox/lidar）
    pub pointcloud: TopicPublisher<LivoxPointCloudTopic>,
    /// 上次发布的时间戳（纳秒），用于限速与 CAS 判定
    pub last_publish_ns: Arc<AtomicU64>,
}

/// Livox 雷达插件：封装深度捕获与点云发布的完整配置。
pub struct RosLivoxPlugin {
    /// 捕获配置（分辨率、纹理格式为 Depth32Float）
    pub config: CaptureConfig,
    /// Livox 上下文
    pub context: RosLivoxContext,
}

/// 将 ROS2 时间戳转换为纳秒整数，便于做周期比较与 CAS。
///
/// # 参数
/// - `stamp`：ROS2 时间戳（sec + nanosec）
///
/// # 返回值
/// 返回 `sec * 1e9 + nanosec`，sec 为负时按 0 处理（饱和）。
fn stamp_to_ns(stamp: &r2r::builtin_interfaces::msg::Time) -> u64 {
    let sec = stamp.sec.max(0) as u64;
    // 使用 saturating_mul 避免溢出
    sec.saturating_mul(1_000_000_000) + stamp.nanosec as u64
}

/// 构造 Livox 风格的 PointCloud2 字段定义。
///
/// 字段布局（每点 18 字节）：
///   - x          : float32 @ offset 0
///   - y          : float32 @ offset 4
///   - z          : float32 @ offset 8
///   - intensity  : float32 @ offset 12
///   - tag        : uint8   @ offset 16
///   - line       : uint8   @ offset 17
fn point_fields() -> Vec<PointField> {
    vec![
        PointField {
            name: "x".to_string(),
            offset: 0,
            datatype: PointField::FLOAT32 as u8,
            count: 1,
        },
        PointField {
            name: "y".to_string(),
            offset: 4,
            datatype: PointField::FLOAT32 as u8,
            count: 1,
        },
        PointField {
            name: "z".to_string(),
            offset: 8,
            datatype: PointField::FLOAT32 as u8,
            count: 1,
        },
        PointField {
            name: "intensity".to_string(),
            offset: 12,
            datatype: PointField::FLOAT32 as u8,
            count: 1,
        },
        PointField {
            name: "tag".to_string(),
            offset: 16,
            datatype: PointField::UINT8 as u8,
            count: 1,
        },
        PointField {
            name: "line".to_string(),
            offset: 17,
            datatype: PointField::UINT8 as u8,
            count: 1,
        },
    ]
}

/// 将 Bevy 的反向 Z 深度值线性化为真实距离（米）。
///
/// Bevy 使用反向 Z（reverse-z）：深度缓冲值越接近 0 表示越远，越接近 1 表示越近。
/// 线性化公式：`z_linear = near / depth`，其中 depth 为采样值。
///
/// # 参数
/// - `depth`：深度缓冲采样值（0.0~1.0）
/// - `near`：近裁剪面距离
///
/// # 返回值
/// 返回线性化后的真实距离；depth 趋近 0 时返回无穷大。
fn linearize_reverse_z(depth: f32, near: f32) -> f32 {
    if depth <= f32::EPSILON {
        return f32::INFINITY;
    }
    near / depth
}

/// 同步阶段快照处理器：在渲染线程中捕获时间戳，准备进入异步阶段。
struct RosLivoxSnapshotSync {
    /// 当前帧时间戳，使用 `RefCell` 以便在 `captured` 中 move 出来
    stamp: RefCell<r2r::builtin_interfaces::msg::Time>,
}

impl SnapshotSync for RosLivoxSnapshotSync {
    /// 进入异步阶段：将时间戳与 Livox 上下文封装为 `RosLivoxSnapshot`。
    fn captured(
        self: Box<Self>,
        world: &mut DeferredWorld,
        _config: &CaptureConfig,
    ) -> Box<dyn SnapshotAsync> {
        Box::new(RosLivoxSnapshot {
            stamp: self.stamp,
            ctx: world.resource::<RosLivoxContextShared>().0.clone(),
        })
    }
}

/// 异步阶段快照处理器：在 GPU 帧完成后将深度图反投影为点云并发布。
///
/// 持有时间戳与 Livox 上下文，在 `captured` 中完成深度图到点云的转换。
struct RosLivoxSnapshot {
    /// 当前帧时间戳
    stamp: RefCell<r2r::builtin_interfaces::msg::Time>,
    /// Livox 上下文（跨线程共享）
    ctx: Arc<RosLivoxContext>,
}

impl SnapshotAsync for RosLivoxSnapshot {
    /// 处理捕获到的深度帧：反投影为 3D 点云并发布。
    ///
    /// # 参数
    /// - `frame`：捕获到的深度帧（Depth32Float 格式）
    ///
    /// # 算法步骤
    /// 1. 校验帧格式与尺寸，过滤无效帧
    /// 2. 计算采样步长 `sample_step`，使输出点数接近 `points_per_publish`
    /// 3. 遍历像素（按步长采样），对每个像素：
    ///    a. 读取 32 位浮点深度值
    ///    b. 线性化深度为真实距离 z，过滤超出 [near, far] 范围的点
    ///    c. 反投影：x = (u - cx) / fx * z, y = (v - cy) / fy * z
    ///    d. 转换到 Livox 坐标系：x_livox = z, y_livox = -x, z_livox = -y
    ///    e. 计算扫描线号 line（基于像素行 v 与 line_num）
    ///    f. 写入点云字节流（18 字节/点）
    /// 4. 发布 PointCloud2 消息（无有效点时跳过）
    fn captured(&mut self, frame: CapturedFrame<'_>) {
        // 仅处理 Depth32Float 格式
        if frame.kind != CapturedFrameKind::Depth32F {
            return;
        }
        // 过滤无效尺寸的帧
        if frame.width == 0 || frame.height == 0 || frame.data.len() < 4 {
            return;
        }

        // 根据分辨率与 FOV 计算相机内参（焦距 fx/fy、主点 cx/cy）
        let intrinsics = compute_camera_intrinsics(frame.width, frame.height, self.ctx.fov_y);
        // 像素总数（每个像素 4 字节 float）
        let pixel_count = frame.data.len() / 4;
        let target_points = self.ctx.points_per_publish.max(1);
        // 采样步长：保证输出点数接近 target_points（向上取整，至少 1）
        let sample_step = ((pixel_count as f32 / target_points as f32).ceil() as usize).max(1);
        let line_num = self.ctx.line_num.max(1);
        // 预分配点云字节缓冲区：每点 18 字节
        let mut data = Vec::with_capacity(target_points * 18);
        let mut valid_points = 0usize;

        // 按步长采样像素，反投影为 3D 点
        for idx in (0..pixel_count).step_by(sample_step) {
            let off = idx * 4;
            // 读取小端 32 位浮点深度值
            let depth = f32::from_le_bytes([
                frame.data[off],
                frame.data[off + 1],
                frame.data[off + 2],
                frame.data[off + 3],
            ]);
            // 跳过非有限值（NaN/Inf）
            if !depth.is_finite() {
                continue;
            }
            // 线性化反向 Z 深度为真实距离
            let z = linearize_reverse_z(depth, self.ctx.near);
            // 过滤无效距离与超出 [near, far] 范围的点
            if !z.is_finite() || z <= self.ctx.near || z > self.ctx.far {
                continue;
            }

            // 像素坐标 (u, v)
            let u = (idx as u32 % frame.width) as f32;
            let v = (idx as u32 / frame.width) as f32;
            // 相机坐标系下的反投影（针孔模型）
            let x = ((u - intrinsics.cx as f32) / intrinsics.fx as f32) * z;
            let y = ((v - intrinsics.cy as f32) / intrinsics.fy as f32) * z;

            // 转换到 Livox 雷达坐标系约定：
            //   x_livox = z_cam   （前方）
            //   y_livox = -x_cam  （左方）
            //   z_livox = -y_cam  （上方）
            let x_livox = z;
            let y_livox = -x;
            let z_livox = -y;
            // 扫描线号：根据像素行 v 线性映射到 [0, line_num-1]
            let line = ((v * line_num as f32 / frame.height as f32).floor() as i32)
                .clamp(0, line_num as i32 - 1) as u8;

            // 写入点云字节流（小端序）
            data.extend_from_slice(&x_livox.to_le_bytes());
            data.extend_from_slice(&y_livox.to_le_bytes());
            data.extend_from_slice(&z_livox.to_le_bytes());
            // intensity 使用默认值
            data.extend_from_slice(&self.ctx.intensity_default.to_le_bytes());
            // tag 使用默认值
            data.push(self.ctx.tag_default);
            // line 为扫描线号
            data.push(line);
            valid_points += 1;
            // 达到目标点数后提前结束
            if valid_points >= target_points {
                break;
            }
        }

        // 无有效点时跳过发布（避免发送空点云）
        if valid_points == 0 {
            return;
        }

        // 发布 PointCloud2 消息
        self.ctx.pointcloud.publish(PointCloud2 {
            header: Header {
                stamp: self.stamp.take(),
                frame_id: self.ctx.frame_id.clone(),
            },
            // height=1 表示无序点云
            height: 1,
            width: valid_points as u32,
            fields: point_fields(),
            is_bigendian: false,
            // 每点 18 字节
            point_step: 18,
            row_step: (valid_points as u32) * 18,
            data,
            is_dense: true,
        });
    }
}

/// Livox 快照创建器：实现 `GpuCaptureHandler`，按发布周期限速生成快照。
///
/// 通过 CAS（compare_exchange）操作更新 `last_publish_ns`，
/// 保证多线程下只有一个快照能通过限速检查。
#[derive(Default)]
struct RosLivoxSnapshotCreator;

impl GpuCaptureHandler for RosLivoxSnapshotCreator {
    /// 在 GPU 捕获开始前调用：检查发布周期，决定是否生成快照。
    ///
    /// # 算法步骤
    /// 1. 读取当前 ROS2 时间戳，转换为纳秒
    /// 2. 检查距上次发布是否已超过 `publish_period_ns`；未超过则返回 None
    /// 3. 使用 CAS 更新 `last_publish_ns`；CAS 失败（被其他线程抢占）则返回 None
    /// 4. CAS 成功则生成 `RosLivoxSnapshotSync`
    fn captured(&self, world: &World) -> Option<Box<dyn SnapshotSync>> {
        let ctx = world.resource::<RosLivoxContextShared>().0.clone();
        let now = ctx.clock.lock().ok()?.get_now().ok()?;
        let stamp = Clock::to_builtin_time(&now);
        let now_ns = stamp_to_ns(&stamp);
        let last = ctx.last_publish_ns.load(Ordering::Relaxed);
        // 限速检查：距上次发布不足一个周期则跳过
        if now_ns < last.saturating_add(ctx.publish_period_ns) {
            return None;
        }
        // CAS 更新 last_publish_ns：失败说明被其他线程抢占，本次跳过
        if ctx
            .last_publish_ns
            .compare_exchange(last, now_ns, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            return None;
        }
        Some(Box::new(RosLivoxSnapshotSync {
            stamp: RefCell::new(stamp),
        }))
    }
}

impl Plugin for RosLivoxPlugin {
    /// 构建 Livox 插件：注册深度纹理拷贝与深度捕获插件。
    ///
    /// # 算法步骤
    /// 1. 创建 `DepthTextureCopyPlugin`，配置深度纹理尺寸
    /// 2. 基于已有深度纹理句柄创建 `CameraCapturePlugin`，注册 `RosLivoxSnapshotCreator`
    /// 3. 插入 `DepthCameraSettings`、Livox 上下文等资源
    /// 4. 在 Startup 阶段注册深度相机初始化系统
    /// 5. 在 Update 阶段注册深度相机同步系统（位于 GameplaySystems::Camera 之后、Render 之前）
    /// 6. 将 Livox 上下文克隆一份插入 RenderApp，供渲染线程访问
    fn build(&self, app: &mut App) {
        // 创建深度纹理拷贝插件，返回深度纹理句柄
        let (depth_copy_plugin, depth_texture_handle) =
            DepthTextureCopyPlugin::new(app, self.config.width, self.config.height);
        // 基于已有深度纹理句柄创建捕获插件，复用纹理而非新建渲染目标
        let depth_capture_plugin = CameraCapturePlugin::from_existing_handle(
            self.config.clone(),
            depth_texture_handle,
            vec![Box::new(RosLivoxSnapshotCreator)],
        );

        app.add_plugins(depth_copy_plugin)
            .add_plugins(depth_capture_plugin)
            // 插入深度相机设置（供渲染管线使用）
            .insert_resource(DepthCameraSettings {
                width: self.config.width,
                height: self.config.height,
                fov_y: self.context.fov_y,
                near: self.context.near,
                far: self.context.far,
            })
            .insert_resource(self.context.clone())
            // Startup：创建深度捕获相机实体
            .add_systems(Startup, setup_depth_capture_camera)
            // Update：在游戏相机系统之后、渲染之前同步深度捕获相机位姿
            .add_systems(
                Update,
                sync_depth_capture_camera
                    .after(GameplaySystems::Camera)
                    .before(RenderSystems::Render),
            );

        // 将 Livox 上下文注入 RenderApp，供渲染线程中的快照处理器访问
        app.sub_app_mut(RenderApp)
            .insert_resource(self.context.clone())
            .insert_resource(RosLivoxContextShared(Arc::new(self.context.clone())));
    }
}
