// ============================================================================
// 模块名：ros2::capture
// 作  用：ROS2 彩色相机捕获插件，将 Bevy 渲染结果转换为 ROS2 图像消息发布
// 职  责：
//   1. 注册彩色相机捕获插件，配置渲染目标与快照处理器
//   2. 在 GPU 捕获完成后，将 RGB 帧封装为 sensor_msgs/Image 或 CompressedImage
//   3. 计算并发布 camera_info（内参矩阵，基于 FOV 与分辨率）
//   4. 维护 ROS2 上下文（时钟、FOV、发布器）供渲染线程访问
// ============================================================================

//! ROS2 图像捕获实现

use crate::capture::{
    CameraFov, ImageHandle, compute_camera_intrinsics,
    driver::{
        CameraCapturePlugin, CaptureConfig, CapturedFrame, CapturedFrameKind, GpuCaptureHandler,
        SnapshotAsync, SnapshotSync,
    },
    setup_capture_camera, setup_preview_window, sync_capture_camera,
};
use crate::dataset::prelude::DatasetSnapshotCreator;
use crate::ros2::image::compress_image;
use crate::ros2::topic::{CameraInfoTopic, ImageCompressedTopic, ImageRawTopic, TopicPublisher};
use crate::systems::GameplaySystems;
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::*;
use bevy::render::{RenderApp, RenderSystems};
use r2r::Clock;
use r2r::sensor_msgs::msg::{CameraInfo, RegionOfInterest};
use r2r::std_msgs::msg::Header;
use std::cell::RefCell;
use std::sync::{Arc, Mutex};

/// 同步阶段快照处理器：在渲染线程中捕获时间戳，并准备进入异步阶段。
///
/// Bevy 的 GPU 捕获分两阶段：
///   1. `SnapshotSync::captured`：在主世界（同步上下文）中调用，可访问 World 资源
///   2. `SnapshotAsync::captured`：在 GPU 帧完成后调用，可访问像素数据
///
/// 本结构体负责第一阶段：从 `RoboMasterClock` 获取当前时间戳，
/// 并将其传递给异步阶段的 `RosSnapshot`。
struct RosSnapshotSync {
    /// 当前帧的时间戳，使用 `RefCell` 以便在 `captured` 中 move 出来
    stamp: RefCell<r2r::builtin_interfaces::msg::Time>,
}

impl SnapshotSync for RosSnapshotSync {
    /// 进入异步阶段：将时间戳与 ROS2 上下文封装为 `RosSnapshot`。
    fn captured(
        self: Box<Self>,
        world: &mut DeferredWorld,
        _config: &CaptureConfig,
    ) -> Box<dyn SnapshotAsync> {
        Box::new(RosSnapshot {
            stamp: self.stamp,
            ctx: world.resource::<RosCaptureContextShared>().0.clone(),
        })
    }
}

/// 异步阶段快照处理器：在 GPU 帧完成后接收像素数据并发布 ROS2 消息。
///
/// 持有时间戳与 ROS2 上下文（包含所有发布器），在 `captured` 中
/// 根据配置发布 CameraInfo、Image 或 CompressedImage。
struct RosSnapshot {
    /// 当前帧时间戳
    stamp: RefCell<r2r::builtin_interfaces::msg::Time>,
    /// ROS2 上下文（时钟、FOV、发布器），跨线程共享
    ctx: Arc<RosCaptureContext>,
}

impl SnapshotAsync for RosSnapshot {
    /// 处理捕获到的帧：仅处理 RGB8 格式，发布 CameraInfo 与图像消息。
    ///
    /// # 参数
    /// - `frame`：捕获到的帧数据（宽、高、像素字节）
    ///
    /// # 算法步骤
    /// 1. 校验帧格式为 Rgb8，否则跳过
    /// 2. 构造消息头（使用 camera_optical_frame 作为 frame_id）
    /// 3. 发布 CameraInfo（含内参矩阵）
    /// 4. 根据 `publish_compressed` 配置发布 CompressedImage 或原始 Image
    fn captured(&mut self, frame: CapturedFrame<'_>) {
        // 仅处理 RGB8 格式的彩色帧
        if frame.kind != CapturedFrameKind::Rgb8 {
            return;
        }

        // 消息头：使用相机光学坐标系作为 frame_id，时间戳取捕获时刻
        let optical_frame_hdr = Header {
            stamp: self.stamp.take(),
            frame_id: "camera_optical_frame".to_string(),
        };
        // 发布相机内参（每次随图像一起发布，便于订阅端同步）
        self.ctx.camera_info.publish(ros_camera_info(
            optical_frame_hdr.clone(),
            frame.width,
            frame.height,
            self.ctx.fov_y,
        ));
        // 根据配置发布压缩图像或原始图像
        if self.ctx.publish_compressed {
            self.ctx.image_compressed.publish(compress_image(
                optical_frame_hdr,
                frame.width,
                frame.height,
                frame.data,
            ));
        } else {
            self.ctx.image_raw.publish(raw_image(
                optical_frame_hdr,
                frame.width,
                frame.height,
                frame.data,
            ));
        }
    }
}

/// 快照创建器：实现 `GpuCaptureHandler`，在每帧捕获前生成 `RosSnapshotSync`。
///
/// 职责是从主世界读取 ROS2 时钟，生成当前时间戳，
/// 并将其封装为同步阶段的快照处理器。
#[derive(Default)]
struct RosSnapshotCreator {}

impl GpuCaptureHandler for RosSnapshotCreator {
    /// 在 GPU 捕获开始前调用：读取当前 ROS2 时间，生成同步快照处理器。
    fn captured(&self, world: &World) -> Option<Box<dyn SnapshotSync>> {
        let clock = world.resource::<RosCaptureContext>();
        Some(Box::new(RosSnapshotSync {
            stamp: RefCell::new(Clock::to_builtin_time(
                &clock.clock.lock().unwrap().get_now().unwrap(),
            )),
        }))
    }
}

/// 渲染应用共享的 ROS2 上下文资源：包装 `Arc<RosCaptureContext>`，
/// 用于在 RenderApp 中跨线程访问发布器与时钟。
#[derive(Resource, Clone, Deref, DerefMut)]
pub struct RosCaptureContextShared(Arc<RosCaptureContext>);

/// ROS2 捕获上下文：持有相机参数与所有图像相关发布器。
///
/// 作为 Bevy 资源插入主 App，同时克隆一份插入 RenderApp，
/// 供同步/异步快照处理器在渲染线程中访问。
#[derive(Resource, Clone)]
pub struct RosCaptureContext {
    /// ROS2 时钟（基于 SystemTime），用于生成消息时间戳
    pub clock: Arc<Mutex<Clock>>,
    /// 相机垂直 FOV（弧度），用于计算内参
    pub fov_y: f32,
    /// 是否发布压缩图像（true: CompressedImage, false: Image）
    pub publish_compressed: bool,
    /// 相机内参发布器（/camera_info）
    pub camera_info: TopicPublisher<CameraInfoTopic>,
    /// 原始图像发布器（/image_raw）
    pub image_raw: TopicPublisher<ImageRawTopic>,
    /// 压缩图像发布器（/image_compressed）
    pub image_compressed: TopicPublisher<ImageCompressedTopic>,
}

/// ROS2 彩色相机捕获插件：封装相机捕获与图像发布的完整配置。
pub struct RosCapturePlugin {
    /// 捕获配置（分辨率、纹理格式、帧类型）
    pub config: CaptureConfig,
    /// ROS2 上下文（时钟、FOV、发布器）
    pub context: RosCaptureContext,
}

impl Plugin for RosCapturePlugin {
    /// 构建捕获插件：注册相机捕获插件、预览窗口、同步系统。
    ///
    /// # 算法步骤
    /// 1. 创建 `CameraCapturePlugin`，注册两个快照创建器：
    ///    - `RosSnapshotCreator`：发布 ROS2 图像消息
    ///    - `DatasetSnapshotCreator`：用于数据集录制
    /// 2. 插入 `ImageHandle`、`CameraFov`、ROS2 上下文等资源
    /// 3. 在 Startup 阶段注册相机初始化与预览窗口系统
    /// 4. 在 Update 阶段注册相机同步系统（位于 GameplaySystems::Camera 之后、Render 之前）
    /// 5. 将 ROS2 上下文克隆一份插入 RenderApp，供渲染线程访问
    fn build(&self, app: &mut App) {
        let (plugin, render_target_handle) = CameraCapturePlugin::new(
            app,
            self.config.clone(),
            vec![
                Box::new(RosSnapshotCreator::default()),
                Box::new(DatasetSnapshotCreator::default()),
            ],
        );
        app.add_plugins(plugin)
            .insert_resource(ImageHandle(render_target_handle))
            .insert_resource(CameraFov(self.context.fov_y))
            .insert_resource(self.context.clone())
            // Startup：创建捕获相机实体与预览窗口
            .add_systems(Startup, setup_capture_camera)
            .add_systems(Startup, setup_preview_window)
            // Update：在游戏相机系统之后、渲染之前同步捕获相机位姿
            .add_systems(
                Update,
                sync_capture_camera
                    .after(GameplaySystems::Camera)
                    .before(RenderSystems::Render),
            );
        // 将 ROS2 上下文注入 RenderApp，供渲染线程中的快照处理器访问
        app.sub_app_mut(RenderApp)
            .insert_resource(RosCaptureContextShared(Arc::new(self.context.clone())))
            .insert_resource(self.context.clone());
    }
}

/// 构造原始图像消息（sensor_msgs/Image，rgb8 编码）。
///
/// # 参数
/// - `hdr`：消息头（含时间戳与 frame_id）
/// - `width` / `height`：图像分辨率
/// - `data`：RGB 像素数据（每像素 3 字节）
///
/// # 返回值
/// 返回 `r2r::sensor_msgs::msg::Image`，encoding 为 "rgb8"，step = width * 3。
fn raw_image(hdr: Header, width: u32, height: u32, data: &[u8]) -> r2r::sensor_msgs::msg::Image {
    r2r::sensor_msgs::msg::Image {
        header: hdr,
        height,
        width,
        encoding: "rgb8".to_string(),
        is_bigendian: 0,
        // 每行字节数 = 宽度 × 3（每像素 3 字节）
        step: width * 3,
        data: Vec::from(data),
    }
}

/// 构造相机内参消息（sensor_msgs/CameraInfo）。
///
/// 基于 FOV 与分辨率计算内参矩阵 K，并填充标准的 P、R 矩阵与 ROI。
/// 畸变模型设为 "plumb_bob" 但畸变系数全为 0（仿真无畸变）。
///
/// # 参数
/// - `hdr`：消息头
/// - `width` / `height`：图像分辨率
/// - `fov_y`：垂直 FOV（弧度）
///
/// # 矩阵说明
/// - `k`：3×3 内参矩阵（行优先），fx=fy（由 FOV 推导），cx/cy 为图像中心
/// - `p`：3×4 投影矩阵，等于 K 加一列零（无立体相机）
/// - `r`：3×3 矫正矩阵，设为单位阵（无矫正）
fn ros_camera_info(hdr: Header, width: u32, height: u32, fov_y: f32) -> CameraInfo {
    // 根据 FOV 与分辨率计算焦距与主点
    let intrinsics = compute_camera_intrinsics(width, height, fov_y);

    CameraInfo {
        header: hdr,
        height,
        width,
        // 针孔畸变模型（仿真中无畸变，系数全为 0）
        distortion_model: "plumb_bob".to_string(),
        d: vec![0.000, 0.000, 0.000, 0.000, 0.000],
        // 内参矩阵 K（3×3，行优先）
        k: vec![
            intrinsics.fx,
            0.0,
            intrinsics.cx,
            0.0,
            intrinsics.fy,
            intrinsics.cy,
            0.0,
            0.0,
            1.0,
        ],
        // 投影矩阵 P（3×4，行优先）：K 加一列零（无立体偏移）
        p: vec![
            intrinsics.fx,
            0.0,
            intrinsics.cx,
            0.0,
            0.0,
            intrinsics.fy,
            intrinsics.cy,
            0.0,
            0.0,
            0.0,
            1.0,
            0.0,
        ],
        // 矫正矩阵 R（3×3）：单位阵
        r: vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        binning_x: 0,
        binning_y: 0,
        // ROI 覆盖整幅图像，启用矫正标志
        roi: RegionOfInterest {
            x_offset: 0,
            y_offset: 0,
            height,
            width,
            do_rectify: true,
        },
    }
}
