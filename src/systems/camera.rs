// 读取鼠标移动事件
use bevy::input::mouse::MouseMotion;
// Bevy 核心系统、资源、输入、变换
use bevy::prelude::*;
// 圆周率常量，用于相机旋转修正
use std::f32::consts::PI;

// 本项目自定义组件
use crate::components::{
    CameraMode,              // 全局相机模式资源
    Controlled,              // 标记被玩家操控的战车
    FollowingType,           // 相机枚举模式：Free自由视角 / Robot车载第一视角 / ThirdPerson第三人称
    Infantry,                // 步兵战车主体组件
    InfantryGimbal,          // 战车云台组件
    InfantryLaunchOffset,    // 炮口偏移挂载点
    InfantryViewOffset,      // 车载视角相机挂载偏移点
    MainCamera,              // 标记主相机实体
};
// 全局仿真配置（相机灵敏度、移动速度等参数在配置里）
use crate::config::SimulationConfig;

/// 系统1：F3快捷键循环切换三种相机视角
pub fn following_controls(mut mode: ResMut<CameraMode>, keyboard: Res<ButtonInput<KeyCode>>) {
    // just_pressed：按下瞬间触发一次，按住不会连续切换
    if keyboard.just_pressed(KeyCode::F3) {
        // 三种模式循环轮换：自由视角 → 车载云台视角 → 第三人称 → 自由视角
        mode.0 = match mode.0 {
            FollowingType::Free => FollowingType::Robot,
            FollowingType::Robot => FollowingType::ThirdPerson,
            FollowingType::ThirdPerson => FollowingType::Free,
        };
    }
}

/// 系统2：根据当前相机模式，更新相机位置与朝向（车载视角 + 第三人称跟随逻辑）
pub fn update_camera_follow(
    // 主相机实体：可变变换 + 相机配置，过滤掉战车实体
    camera_query: Single<(&mut Transform, &MainCamera), Without<Controlled>>,
    // 玩家操控战车的世界位姿
    infantry: Single<&Transform, (With<Infantry>, With<Controlled>)>,
    // 战车云台局部变换
    gimbal: Single<&Transform, (With<Controlled>, With<InfantryGimbal>)>,
    // 车载相机挂载偏移节点
    view_offset: Single<&Transform, (With<Controlled>, With<InfantryViewOffset>)>,
    // 炮口挂载点变换
    launch_offset: Single<&Transform, (With<Controlled>, With<InfantryLaunchOffset>)>,
    mode: Res<CameraMode>, // 当前相机模式只读资源
) {
    let gimbal_transform = gimbal.into_inner();
    let (mut camera_transform, camera_offset) = camera_query.into_inner();

    match mode.0 {
        // ========== 模式一：Robot 车载云台第一视角（相机绑定云台，跟随炮口朝向） ==========
        FollowingType::Robot => {
            // 计算云台在世界空间下的旋转 = 底盘整体旋转 * 云台自身相对底盘的旋转
            let gimbal_world_rotation = infantry.rotation * gimbal_transform.rotation;
            // 相机挂载点的世界坐标偏移：云台旋转 × 挂载点局部偏移
            let view_offset_world = gimbal_world_rotation * view_offset.translation;

            // 相机位置 = 战车底盘世界位置 + 云台挂载点偏移，实现相机跟着云台走
            camera_transform.translation = infantry.translation + view_offset_world;

            // 相机朝向对齐云台炮口方向，并额外旋转90°修正Bevy默认朝向轴差异
            camera_transform.rotation = gimbal_world_rotation
                * launch_offset.rotation
                * Quat::from_euler(EulerRot::ZYX, 0.0, 0.0, PI / 2.0);
        }

        // ========== 模式二：ThirdPerson 第三人称尾随视角 ==========
        FollowingType::ThirdPerson => {
            let base_transform = infantry.into_inner();
            // 将配置里的第三人称偏移向量，跟随战车旋转做姿态转换
            let offset = base_transform.rotation * camera_offset.follow_offset;
            // 相机处在战车后方偏移位置
            camera_transform.translation = base_transform.translation + offset;
            // 相机始终看向战车中心，实现尾随注视效果
            camera_transform.look_at(base_transform.translation, Vec3::Y);
        }

        // ========== 模式三：Free自由视角，本系统不处理，交由 freecam_controls 控制 ==========
        FollowingType::Free => {}
    }
}

/// 系统3：自由视角模式下的键鼠漫游控制（WASD移动、鼠标拖拽旋转视角、NJ上下）
pub fn freecam_controls(
    time: Res<Time>,
    mode: Res<CameraMode>,
    config: Res<SimulationConfig>,
    mut mouse_motion_events: MessageReader<MouseMotion>, // 读取鼠标每一帧移动增量
    keyboard: Res<ButtonInput<KeyCode>>,
    // 拿到主相机的变换，排除战车实体
    camera_query: Single<&mut Transform, (With<MainCamera>, Without<Infantry>)>,
) {
    // 只有处于自由视角模式才执行漫游逻辑
    if mode.0 != FollowingType::Free {
        return;
    }

    let delta = time.delta_secs();
    let mut camera_transform = camera_query.into_inner();

    // 累计本帧所有鼠标移动偏移量
    let mut mouse_delta = Vec2::ZERO;
    for event in mouse_motion_events.read() {
        mouse_delta += event.delta;
    }

    // 鼠标拖动旋转视角逻辑
    if mouse_delta != Vec2::ZERO {
        // 提取相机当前的 偏航(yaw左右转头)、俯仰(pitch抬头低头)、滚转
        let (yaw, pitch, roll) = camera_transform.rotation.to_euler(EulerRot::YXZ);

        // 水平鼠标移动修改偏航，乘以灵敏度系数
        let new_yaw = yaw - mouse_delta.x * config.camera.mouse_sensitivity;
        // 垂直鼠标修改俯仰，限制俯仰角度 [-1.4rad ~ 1.4rad]，防止镜头倒翻
        let new_pitch = (pitch - mouse_delta.y * config.camera.mouse_sensitivity).clamp(-1.4, 1.4);

        // 重新组合相机旋转四元数
        camera_transform.rotation = Quat::from_euler(EulerRot::YXZ, new_yaw, new_pitch, roll);
    }

    // 本帧移动距离 = 移动速度 × 帧间隔时间
    let speed = config.camera.free_move_speed * delta;
    // 获取相机自身前后、左右、上下方向向量
    let forward = camera_transform.forward();
    let right = camera_transform.right();
    let up = camera_transform.up();

    // WASD 前后左右漫游
    if keyboard.pressed(KeyCode::KeyW) {
        camera_transform.translation += forward * speed;
    }
    if keyboard.pressed(KeyCode::KeyS) {
        camera_transform.translation -= forward * speed;
    }
    if keyboard.pressed(KeyCode::KeyA) {
        camera_transform.translation -= right * speed;
    }
    if keyboard.pressed(KeyCode::KeyD) {
        camera_transform.translation += right * speed;
    }
    // N / J 上升、下降
    if keyboard.pressed(KeyCode::KeyN) {
        camera_transform.translation += up * speed;
    }
    if keyboard.pressed(KeyCode::KeyJ) {
        camera_transform.translation -= up * speed;
    }
}