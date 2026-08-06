//! Automatic dataset generation mode
//! Usage: cargo run -- --auto-gen

// 运行单次系统
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
// 渲染纹理格式
use bevy::render::render_resource::TextureFormat;
use bevy::render::{Extract, RenderApp};

// 相机截图驱动插件、截图配置、帧类型定义
use crate::capture::driver::{CameraCapturePlugin, CaptureConfig, CapturedFrameKind};
use crate::capture::{
    CameraFov, CaptureSource, IMAGE_HEIGHT, IMAGE_WIDTH, ImageHandle, setup_capture_camera,
    sync_capture_camera,
};
// 步兵装甲组件
use crate::components::Infantry;
// 数据集存储模块、真正执行截图的 capture 系统
use crate::dataset::prelude::{DatasetPlugin, capture};
// 机器人战队、装甲扫描组件、英雄机器人配置
use crate::robomaster::prelude::*;

// ==================== 全局采集遍历配置参数 ====================
/// 相机离目标机器人最小距离
const DIST_MIN: f32 = 2.0;
/// 相机离目标机器人最大距离
const DIST_MAX: f32 = 8.0;
/// 距离步进 0.5m
const DIST_STEP: f32 = 0.5;

/// 水平旋转范围：[-180°, 180°] 弧度
const YAW_MIN: f32 = -std::f32::consts::PI;
const YAW_MAX: f32 = std::f32::consts::PI;
/// 水平每次转动 30°
const YAW_STEP: f32 = 0.5236;

/// 俯仰角：上下±45°
const PITCH_MIN: f32 = -0.7854;
const PITCH_MAX: f32 = 0.7854;
/// 俯仰步进 15°
const PITCH_STEP: f32 = 0.2618;

/// 相机高度补偿偏移，避免贴地
const HEIGHT_OFFSET: f32 = 0.5;
/// 相机移动完成后静置帧数（防止运动模糊、物理抖动，画面稳定后再截图）
const SETTLE_FRAMES: u32 = 5;
/// 相机视场角 45°
const FOV: f32 = 45.0;
// =================================================

/// 标记组件：待被拍摄的敌方英雄机器人实体
#[derive(Component)]
struct AutoGenTarget;

/// 自动采集全局状态资源，记录遍历进度、静置计时器、采集开关
#[derive(Resource, Clone)]
struct AutoGenState {
    distances: Vec<f32>,    // 所有采样距离列表
    yaws: Vec<f32>,         // 所有水平角度列表
    pitches: Vec<f32>,      // 所有俯仰角度列表
    d_idx: usize,           // 当前距离游标
    y_idx: usize,           // 当前yaw游标
    p_idx: usize,           // 当前pitch游标
    settle_counter: u32,    // 静置倒计时
    frame_count: usize,     // 已成功保存图片总数
    capturing: bool,        // 是否正在执行截图（避免移动相机和截图并发冲突）
}

/// 数据集自动生成插件入口
pub struct AutoGenPlugin;

impl Plugin for AutoGenPlugin {
    fn build(&self, app: &mut App) {
        // 截图配置：分辨率、纹理格式、像素格式
        let capture_config = CaptureConfig {
            width: IMAGE_WIDTH,
            height: IMAGE_HEIGHT,
            texture_format: TextureFormat::Bgra8UnormSrgb,
            frame_kind: CapturedFrameKind::Rgb8,
        };

        use crate::dataset::prelude::DatasetSnapshotCreator;
        // 构造相机截图插件，挂载「数据集快照生成器」：截图时同步读取装甲包围盒标注
        let (camera_capture_plugin, image_handle) = CameraCapturePlugin::new(
            app,
            capture_config.clone(),
            vec![Box::new(DatasetSnapshotCreator::default())],
        );

        // 注册插件与全局资源
        app.add_plugins(camera_capture_plugin)
            .add_plugins(DatasetPlugin)
            .insert_resource(ImageHandle(image_handle))
            .insert_resource(CameraFov(FOV))
            .insert_resource(capture_config)
            // 启动阶段：初始化场景、生成机器人、生成采集相机
            .add_systems(Startup, (setup_auto_gen, setup_capture_camera))
            // 更新阶段：
            // 1. auto_gen_loop 驱动相机不断换位
            // 2. sync_capture_camera 同步相机渲染参数，必须在换位之后执行
            .add_systems(
                Update,
                (auto_gen_loop, sync_capture_camera.after(auto_gen_loop)),
            );

        // 渲染子App配置
        let render_app = app.sub_app_mut(RenderApp);
        // ExtractSchedule：主线数据同步到渲染线程的阶段
        render_app
            // 写入是否需要截图的标记
            .add_systems(ExtractSchedule, write_flag)
            // 全局标记：(是否应该截图, 上一帧pitch下标)
            .insert_resource(ShouldCapture(true, 0))
            // 判断标记，触发单次截图系统
            .add_systems(ExtractSchedule, capture_condition);
    }
}

/// 渲染线程：判断标记，执行一次截图采集
fn capture_condition(world: &mut World) {
    let mut res = world.resource_mut::<ShouldCapture>();
    if res.0 {
        res.0 = false;
        // 运行一次截图系统，生成图片+标注文件
        world.run_system_once(capture).unwrap();
    }
}

/// Startup系统：初始化场景地面、目标英雄机器人、采集相机、遍历状态
fn setup_auto_gen(mut commands: Commands, asset_server: Res<AssetServer>) {
    info!(
        "Config: dist {:.1}-{:.1} step {:.1}, yaw {:.1}-{:.1} step {:.1}, pitch {:.1}-{:.1} step {:.1}",
        DIST_MIN,
        DIST_MAX,
        DIST_STEP,
        YAW_MIN.to_degrees(),
        YAW_MAX.to_degrees(),
        YAW_STEP.to_degrees(),
        PITCH_MIN.to_degrees(),
        PITCH_MAX.to_degrees(),
        PITCH_STEP.to_degrees()
    );

    // 生成地面场景
    commands.spawn((
        SceneRoot(asset_server.load("GROUND.glb#Scene0")),
        Transform::IDENTITY,
    ));

    // 生成待拍摄的蓝色英雄机器人
    commands.spawn((
        SceneRoot(asset_server.load("HERO.glb#Scene0")),
        Transform::from_xyz(0.0, 1.0, 0.0),
        Infantry::new(Team::Blue, HERO_ROBOT_CONFIG),
        ScanArmor::new(Team::Blue, HERO_ROBOT_CONFIG.armor),
        AutoGenTarget,
    ));

    // 创建采集专用相机，标记 CaptureSource，专门用来渲染截图
    commands.spawn((
        Camera3d::default(),
        Camera {
            is_active: true,
            ..default()
        },
        Projection::Perspective(PerspectiveProjection {
            fov: FOV.to_radians(),
            near: 0.1,
            far: 100.0,
            ..default()
        }),
        Transform::from_xyz(3.0, 2.0, 3.0).looking_at(Vec3::ZERO, Vec3::Y),
        CaptureSource,
        Name::new("AutoGenCamera"),
    ));

    // 生成等差角度/距离数组
    let distances = gen_range(DIST_MIN, DIST_MAX, DIST_STEP);
    let yaws = gen_range(YAW_MIN, YAW_MAX, YAW_STEP);
    let pitches = gen_range(PITCH_MIN, PITCH_MAX, PITCH_STEP);
    let total = distances.len() * yaws.len() * pitches.len();

    info!("Total poses to capture: {}", total);

    // 初始化全局遍历状态
    commands.insert_resource(AutoGenState {
        distances,
        yaws,
        pitches,
        d_idx: 0,
        y_idx: 0,
        p_idx: 0,
        settle_counter: 0,
        frame_count: 0,
        capturing: false,
    });
}

/// Update主循环：控制相机移动、静置、流转姿态下标
fn auto_gen_loop(
    _commands: Commands,
    mut state: ResMut<AutoGenState>,
    // 唯一持有 CaptureSource 的相机
    mut camera: Single<&mut Transform, With<CaptureSource>>,
    // 唯一待拍摄机器人全局坐标
    target: Single<&GlobalTransform, With<AutoGenTarget>>,
) {
    // 上一轮截图还在执行，等待截图完成后再切换下一个姿态
    if state.capturing {
        state.capturing = false;
        next_pose(&mut state);
        return;
    }

    // 相机已经就位，进入静置倒计时阶段
    if state.settle_counter > 0 {
        state.settle_counter -= 1;
        // 静置完毕，允许下一帧执行截图
        if state.settle_counter == 0 {
            state.capturing = true;
        }
        return;
    }

    // 所有姿态遍历完毕，结束程序
    if state.d_idx >= state.distances.len() {
        info!("=== Dataset Generation Complete! ===");
        info!("Total frames captured: {}", state.frame_count);
        std::process::exit(0);
    }

    // 取出当前遍历的距离、yaw、pitch
    let dist = state.distances[state.d_idx];
    let yaw = state.yaws[state.y_idx];
    let pitch = state.pitches[state.p_idx];

    let target_pos = target.translation();
    // 球坐标系转笛卡尔坐标：以机器人为圆心摆放相机
    let x = dist * yaw.cos() * pitch.cos();
    let y = dist * pitch.sin() + HEIGHT_OFFSET;
    let z = dist * yaw.sin() * pitch.cos();

    // 更新相机位置，并始终看向机器人本体
    camera.translation = target_pos + Vec3::new(x, y, z);
    camera.look_at(target_pos, Vec3::Y);

    // 计算当前进度
    let done = state.d_idx * state.yaws.len() * state.pitches.len()
        + state.y_idx * state.pitches.len()
        + state.p_idx;
    let total = state.distances.len() * state.yaws.len() * state.pitches.len();

    // 每隔10帧、首帧、最后一帧打印进度日志
    if state.frame_count % 10 == 0
        || (state.d_idx == 0 && state.y_idx == 0 && state.p_idx == 0)
        || done == total - 1
    {
        info!(
            "Progress: {}/{} (dist={:.1}, yaw={:.1}°, pitch={:.1}°)",
            done + 1,
            total,
            dist,
            yaw.to_degrees(),
            pitch.to_degrees()
        );
    }

    // 相机移动完毕，开启静置倒计时
    state.settle_counter = SETTLE_FRAMES;
}

/// Extract阶段：把主线的遍历状态同步到渲染线程，判断是否需要触发截图
fn write_flag(q: Extract<Res<AutoGenState>>, mut r: ResMut<ShouldCapture>) {
    let old_frame_id = r.1;
    // pitch下标发生变化 = 进入新姿态，需要截图
    if q.p_idx != 0 && q.p_idx != old_frame_id {
        r.0 = true;
        r.1 = q.p_idx;
    } else {
        r.0 = false;
    }
}

/// 渲染线程标记资源
#[derive(Resource)]
struct ShouldCapture(bool, usize);

/// 游标进位逻辑：pitch优先递增，pitch走完循环归零，yaw+1；yaw走完归零，distance+1
fn next_pose(state: &mut AutoGenState) {
    state.frame_count += 1;
    state.p_idx += 1;
    if state.p_idx >= state.pitches.len() {
        state.p_idx = 0;
        state.y_idx += 1;
        if state.y_idx >= state.yaws.len() {
            state.y_idx = 0;
            state.d_idx += 1;
        }
    }
}

/// 生成 [min, max] 闭区间等差数组，浮点容错 +0.0001 避免精度丢失漏掉终点
fn gen_range(min: f32, max: f32, step: f32) -> Vec<f32> {
    let mut v = Vec::new();
    let mut x = min;
    while x <= max + 0.0001 {
        v.push(x);
        x += step;
    }
    v
}