// 子模块声明：深度图相关逻辑、底层图像驱动模块
pub mod depth;
pub mod driver;

// Bevy相机渲染目标类型
use bevy::camera::RenderTarget;
// 深度预通道，用于渲染深度图（用于目标测距、视觉算法）
use bevy::core_pipeline::prepass::DepthPrepass;
// 色调映射，控制HDR颜色转换到屏幕色域
use bevy::core_pipeline::tonemapping::Tonemapping;
// Bevy基础ECS、变换、资源、组件等基础类型
use bevy::prelude::*;

/// 标记组件：CaptureSource 捕获源实体
/// 云台/炮管实体，CaptureCamera相机跟随这个实体的位姿
#[derive(Component)]
pub struct CaptureSource;

/// 标记组件：CaptureCamera 图像采集相机
/// 离线渲染相机，**不直接渲染到窗口屏幕**，渲染到一张GPU纹理图片，供视觉算法使用
#[derive(Component)]
pub struct CaptureCamera;

/// 全局资源：ImageHandle，保存采集相机渲染输出的图片资源句柄
/// Deref 解包，方便直接访问内部 Handle<Image>；Clone 允许多处克隆句柄
#[derive(Resource, Deref, Clone)]
pub struct ImageHandle(pub Handle<Image>);

/// 全局资源：相机垂直视场角FOV
#[derive(Resource, Clone, Copy)]
pub struct CameraFov(pub f32);

/// CaptureCamera渲染顺序：-100，优先渲染，在主窗口相机之前绘制
pub const CAPTURE_CAMERA_ORDER: isize = -100;

/**
 * @brief 生成图像采集相机（CaptureCamera）
 * 直接操作World，适合在App启动阶段一次性生成；防止重复生成相机实体
 * 相机输出渲染到一张GPU纹理图片，不是窗口画面，用于YOLO、PnP等视觉推理
 */
pub fn setup_capture_camera(world: &mut World) {
    // 查询是否已经存在CaptureCamera实体，避免多次spawn重复创建相机
    let capture_camera_exists = {
        let mut query = world.query_filtered::<Entity, With<CaptureCamera>>();
        query.iter(world).next().is_some()
    };
    if capture_camera_exists {
        return;
    }

    // 从全局资源拿到渲染目标图片句柄，克隆一份给相机使用
    let render_target_handle = world.resource::<ImageHandle>().0.clone();
    // 读取全局FOV配置
    let fov = world.resource::<CameraFov>().0;

    // 生成采集相机实体
    world.spawn((
        Camera3d::default(),
        // 关闭色调映射：直接输出线性原始颜色，给视觉算法，不要做屏幕色彩矫正
        Tonemapping::None,
        // 渲染目标：不渲染到窗口，渲染到指定Image纹理
        RenderTarget::Image(render_target_handle.into()),
        Camera {
            order: CAPTURE_CAMERA_ORDER,
            // clear_color: ClearColorConfig::Custom(Color::BLACK), // 可选：背景清空为黑色，注释状态
            ..default()
        },
        // 透视投影相机
        Projection::Perspective(PerspectiveProjection {
            fov,          // 垂直视场角
            near: 0.1,    // 近裁剪面，小于0.1m物体不渲染
            far: 10000.0, // 远裁剪面，最远10000m，足够RoboMaster大场景
            ..default()
        }),
        // 关闭多重采样抗锯齿MSAA：视觉算法不需要抗锯齿，减少GPU开销，避免边缘伪影
        Msaa::Off,
        // 开启深度预通道，生成深度纹理，用于深度测距、点云
        DepthPrepass,
        CaptureCamera, // 打上采集相机标记组件
    ));
}

/// 标记组件：PreviewCamera 预览UI相机（2D正交相机，用来渲染UI）
#[derive(Component)]
pub struct PreviewCamera;

/// 标记组件：PreviewImageNode 预览图像UI节点
/// 用于把CaptureCamera渲染出来的纹理贴到屏幕上做可视化预览窗口
#[derive(Component)]
pub struct PreviewImageNode;

/**
 * @brief 创建画面预览窗口UI
 * 从SimulationConfig读取preview.enabled开关；关闭时直接跳过，不生成预览UI
 * 作用：把采集相机输出的GPU纹理渲染到屏幕，方便调试视觉画面，不影响算法输入
 */
pub fn setup_preview_window(world: &mut World) {
    // 读取全局仿真配置，预览开关
    let preview_enabled = world
        .resource::<crate::config::SimulationConfig>()
        .preview
        .enabled;
    if !preview_enabled {
        return;
    }

    // 获取采集相机输出图片句柄
    let render_target_handle = world.resource::<ImageHandle>().0.clone();

    // 检查预览2D相机是否已创建，防止重复生成
    let preview_camera_exists = {
        let mut query = world.query_filtered::<Entity, With<PreviewCamera>>();
        query.iter(world).next().is_some()
    };
    if !preview_camera_exists {
        // 生成2D UI相机，用于渲染UI节点
        world.spawn((
            Camera2d::default(),
            Camera {
                order: 1,
                ..default()
            },
            PreviewCamera,
        ));
    }

    // 检查预览图片UI节点是否存在
    let preview_node_exists = {
        let mut query = world.query_filtered::<Entity, With<PreviewImageNode>>();
        query.iter(world).next().is_some()
    };
    if !preview_node_exists {
        // 生成全屏UI节点，贴入相机渲染纹理
        world.spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            // GlobalZIndex(-1)：放到UI最底层；文字、调试UI绘制在图像预览之上
            GlobalZIndex(-1),
            // ImageNode：将GPU纹理渲染到UI节点
            ImageNode::new(render_target_handle),
            PreviewImageNode,
        ));
    }
}

/**
 * @brief 复制Transform位姿
 * 把目标实体的平移、旋转、缩放完整拷贝到our（相机）
 * 用于相机跟随云台CaptureSource
 */
pub fn copy_transform(target: &Transform, our: &mut Transform) {
    our.translation = target.translation;
    our.scale = target.scale;
    our.rotation = target.rotation;
}

/**
 * @brief ECS系统：同步采集相机位姿
 * CaptureCamera相机跟随CaptureSource实体（云台炮管）实时更新位置姿态
 * Single保证唯一：只有一个CaptureSource、一个CaptureCamera
 */
pub fn sync_capture_camera(
    // CaptureSource：跟随源（云台实体），不带CaptureCamera标签
    target: Single<&Transform, (With<CaptureSource>, Without<CaptureCamera>)>,
    // CaptureCamera采集相机，可变Transform，不带CaptureSource标签
    mut our: Single<&mut Transform, (With<CaptureCamera>, Without<CaptureSource>)>,
) {
    copy_transform(&target, &mut our);
}

/**
 * @brief 相机内参结构体
 * 针孔相机模型，给PnP、目标解算、位姿估计用
 * fx,fy：x/y方向焦距（像素单位）
 * cx,cy：主点（图像中心像素坐标）
 * width/height：图像分辨率
 */
#[derive(Clone, Copy, Debug)]
pub struct CameraIntrinsics {
    pub fx: f64,
    pub fy: f64,
    pub cx: f64,
    pub cy: f64,
    pub width: u32,
    pub height: u32,
}

/**
 * @brief 根据分辨率+垂直FOY，计算针孔相机内参
 * @param width 图像像素宽
 * @param height 图像像素高
 * @param fov_y 垂直视场角（弧度）
 * @return CameraIntrinsics 完整相机内参
 */
pub fn compute_camera_intrinsics(width: u32, height: u32, fov_y: f32) -> CameraIntrinsics {
    let fov_y = fov_y as f64;
    // 宽高比
    let aspect = width as f64 / height as f64;
    // 由垂直FOV推导水平FOV
    let fov_x = 2.0 * ((fov_y / 2.0).tan() * aspect).atan();
    // 焦距fx,fy 像素单位
    let fx = width as f64 / (2.0 * (fov_x / 2.0).tan());
    let fy = height as f64 / (2.0 * (fov_y / 2.0).tan());
    // 图像中心，主点cx,cy
    let cx = width as f64 / 2.0;
    let cy = height as f64 / 2.0;
    CameraIntrinsics {
        fx,
        fy,
        cx,
        cy,
        width,
        height,
    }
}

/// 采集相机输出图像分辨率：1440 × 1080，和RoboMaster相机常用比例匹配
pub const IMAGE_WIDTH: u32 = 1440;
pub const IMAGE_HEIGHT: u32 = 1080;
