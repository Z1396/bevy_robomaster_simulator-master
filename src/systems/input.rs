// Bevy ECS基础库
use bevy::prelude::*;
// 原子内存序，用于多线程安全读写自瞄开关布尔值
use std::sync::atomic::Ordering;

// 项目自定义组件
use crate::components::{
    ActiveSlapper,        // 标记当前正在被操控的副战车
    Controlled,           // 玩家主操控战车标记
    Infantry,             // 战车本体标签
    InfantryChassis,      // 底盘状态组件：记录底盘yaw角度、角速度
    InfantryGimbal,       // 云台状态组件：云台偏航、俯仰角
    SlapperInfantry,      // 可被切换操控的备用战车标签
    SubscribeAutoAim,     // 全局原子布尔资源：是否开启自动瞄准
};
// 全局仿真配置文件
use crate::config::SimulationConfig;
// 麦克纳姆轮底盘动力学实现模块
use crate::robomaster::vehicle::movement::VehicleDynamic;
// Avian3D 物理引擎（新版Bevy物理引擎，替代旧版Rapier）
use avian3d::prelude::*;

/// 自定义宏：封装键盘输入采集逻辑，重载两套实现
macro_rules! input {
    // 重载1：采集二维平面输入（前后+左右，返回Vec2），用于底盘平移
    ($keyboard:ident, $forward:ident,$left:ident,$backward:ident,$right:ident) => {{
        let mut input = Vec2::ZERO;
        if $keyboard.pressed(KeyCode::$forward) {
            input.y += 1.0;
        }
        if $keyboard.pressed(KeyCode::$backward) {
            input.y -= 1.0;
        }
        if $keyboard.pressed(KeyCode::$right) {
            input.x += 1.0;
        }
        if $keyboard.pressed(KeyCode::$left) {
            input.x -= 1.0;
        }
        input
    }};
    // 重载2：采集一维转向输入（左/右，返回f32），用于底盘旋转、云台水平转动
    ($keyboard:ident, $left:ident,$right:ident) => {{
        let mut input: f32 = 0.0;
        if $keyboard.pressed(KeyCode::$left) {
            input += 1.0;
        }
        if $keyboard.pressed(KeyCode::$right) {
            input += -1.0;
        }
        input
    }};
}

// 底盘旋转平滑惯性系数：数值越大，加速/刹车越迅速；90对应快速响应
const CHASSIS_ROTATION_RESPONSE: f32 = 90.0;
// 底盘角速度死区阈值：速度小于该值直接置0，防止微小抖动
const CHASSIS_ROTATION_STOP_EPSILON: f32 = 1e-3;

/// 底盘平滑旋转更新函数（一阶指数滤波，实现缓启动、缓刹车）
/// chassis_transform：底盘实体变换
/// chassis_data：底盘状态结构体
/// input：转向输入量 [-1, 1]
/// rotation_speed：最大旋转角速度 rad/s
/// dt：帧时间间隔
fn update_chassis_rotation(
    chassis_transform: &mut Transform,
    chassis_data: &mut InfantryChassis,
    input: f32,
    rotation_speed: f32,
    dt: f32,
) {
    // 目标角速度 = 操纵杆输入 × 最大转速
    let target_yaw_velocity = input * rotation_speed;
    // 指数平滑滤波系数 alpha = 1 - exp(-k*dt)，模拟电机惯性
    let response = 1.0 - (-CHASSIS_ROTATION_RESPONSE * dt).exp();
    // 低通滤波更新实际角速度，不会瞬间跳变到目标值
    chassis_data.yaw_velocity += (target_yaw_velocity - chassis_data.yaw_velocity) * response;

    // 死区处理：无输入且转速极低时，强制转速归零，消除漂移抖动
    if chassis_data.yaw_velocity.abs() < CHASSIS_ROTATION_STOP_EPSILON
        && target_yaw_velocity.abs() < CHASSIS_ROTATION_STOP_EPSILON
    {
        chassis_data.yaw_velocity = 0.0;
    }

    // 积分更新底盘总偏航角 yaw
    chassis_data.yaw += chassis_data.yaw_velocity * dt;
    // 赋值底盘旋转姿态（仅绕Y轴旋转，YXZ欧拉顺序）
    chassis_transform.rotation = Quat::from_euler(EulerRot::YXZ, chassis_data.yaw, 0.0, 0.0);
}

/// 系统：F5 按键切换自动瞄准开关
pub fn auto_aim_switch(keyboard: Res<ButtonInput<KeyCode>>, enabled: Res<SubscribeAutoAim>) {
    if keyboard.just_pressed(KeyCode::F5) {
        info!("Toggling auto-aim subscription.");
        // 原子异或翻转布尔状态，AcqRel内存序保证多线程安全
        let new_state = !enabled.fetch_xor(true, Ordering::AcqRel);
        info!(
            "Auto-aim subscription is now {}.",
            if new_state { "ENABLED" } else { "DISABLED" }
        );
    }
}

/// 主战车底盘控制系统（玩家WASD操控）
pub fn vehicle_controls(
    time: Res<Time>,
    keyboard: Res<ButtonInput<KeyCode>>,
    config: Res<SimulationConfig>,
    // 查询主战车：物理受力、质量、底盘动力学结构体
    infantry: Single<(Forces, &Mass, &mut VehicleDynamic), (With<Infantry>, With<Controlled>)>,
    // 查询云台全局姿态
    gimbal: Single<
        (&GlobalTransform, &InfantryGimbal),
        (With<Controlled>, Without<InfantryChassis>),
    >,
    // 单独查询底盘实体变换与底盘状态
    chassis: Single<
        (&mut Transform, &mut InfantryChassis),
        (
            With<Controlled>,
            Without<InfantryGimbal>,
            With<InfantryChassis>,
            Without<Infantry>,
        ),
    >,
) {
    // WASD 采集二维移动输入
    let input = input!(keyboard, KeyW, KeyA, KeyS, KeyD);
    // 左Shift开启2倍加速冲刺
    let boost = if keyboard.pressed(KeyCode::ShiftLeft) {
        2.0
    } else {
        1.0
    };

    let (mut forces, &Mass(mass), mut dynamic) = infantry.into_inner();
    let dt = time.delta_secs();

    // 麦克纳姆轮动力学函数：施加平面移动力，实现全向移动
    // 入参：物理力组件、车体质量、云台姿态、摇杆输入、帧间隔、加速倍率
    dynamic.linear(
        &mut forces,
        mass,
        gimbal.into_inner().0,
        input,
        time.delta_secs(),
        boost,
    );

    // Q/E 采集底盘旋转输入
    let input = input!(keyboard, KeyQ, KeyE);
    let (mut chassis_transform, mut chassis_data) = chassis.into_inner();
    // 执行平滑旋转逻辑
    update_chassis_rotation(
        &mut chassis_transform,
        &mut chassis_data,
        input,
        config.vehicle.rotation_speed,
        dt,
    );
}

/// 备用战车远程控制系统（IJKL操控，用来控制敌方/第二台战车）
pub fn remote_vehicle_controls(
    time: Res<Time>,
    keyboard: Res<ButtonInput<KeyCode>>,
    config: Res<SimulationConfig>,
    // 筛选带有ActiveSlapper标记的被激活备用战车
    infantry: Single<
        (Forces, &Mass, &mut VehicleDynamic),
        (With<ActiveSlapper>, With<Infantry>, Without<Controlled>),
    >,
    gimbal: Single<
        (&GlobalTransform, &InfantryGimbal),
        (With<ActiveSlapper>, Without<InfantryChassis>),
    >,
    chassis: Single<
        (&mut Transform, &mut InfantryChassis),
        (With<ActiveSlapper>, Without<InfantryGimbal>),
    >,
) {
    // IJKL 控制备用战车前后左右
    let input = input!(keyboard, KeyI, KeyJ, KeyK, KeyL);
    // 右Shift冲刺加速
    let boost = if keyboard.pressed(KeyCode::ShiftRight) {
        2.0
    } else {
        1.0
    };

    let (mut forces, &Mass(mass), mut dynamic) = infantry.into_inner();
    let dt = time.delta_secs();
    dynamic.linear(
        &mut forces,
        mass,
        gimbal.into_inner().0,
        input,
        time.delta_secs(),
        boost,
    );

    // U/O 旋转备用战车底盘
    let input = input!(keyboard, KeyU, KeyO);
    let (mut chassis_transform, mut chassis_data) = chassis.into_inner();
    update_chassis_rotation(
        &mut chassis_transform,
        &mut chassis_data,
        input,
        config.vehicle.rotation_speed,
        dt,
    );
}

/// 主战车云台手动操控（方向键控制炮管上下左右）
pub fn gimbal_controls(
    time: Res<Time>,
    keyboard: Res<ButtonInput<KeyCode>>,
    config: Res<SimulationConfig>,
    // enabled: Res<SubscribeAutoAim>,
    gimbal: Single<
        (&mut Transform, &mut InfantryGimbal),
        (With<Controlled>, Without<InfantryChassis>),
    >,
) {
    // 注释代码：开启自瞄时禁用手动云台，防止人手与自瞄互相抢占控制权
    //if enabled.load(Ordering::Acquire) {
    //    return;
    //}

    let dt = time.delta_secs();
    let (mut gimbal_transform, mut gimbal_data) = gimbal.into_inner();

    // 读取云台当前欧拉角：水平偏航local_yaw、俯仰pitch
    (gimbal_data.local_yaw, gimbal_data.pitch, _) =
        gimbal_transform.rotation.to_euler(EulerRot::YXZ);

    // ← → 方向键控制云台水平旋转
    gimbal_data.local_yaw +=
        input!(keyboard, ArrowLeft, ArrowRight) * config.vehicle.gimbal_rotation_speed * dt;
    // ↑ ↓ 方向键控制云台俯仰
    gimbal_data.pitch +=
        input!(keyboard, ArrowUp, ArrowDown) * config.vehicle.gimbal_rotation_speed * dt;

    // 俯仰角度硬限位，避免炮管朝上/朝下翻转卡死
    gimbal_data.pitch = gimbal_data.pitch.clamp(
        -config.vehicle.gimbal_pitch_limit,
        config.vehicle.gimbal_pitch_limit,
    );

    // 重新生成云台旋转姿态并赋值
    let gimbal_rotation =
        Quat::from_euler(EulerRot::YXZ, gimbal_data.local_yaw, gimbal_data.pitch, 0.0);
    gimbal_transform.rotation = gimbal_rotation;
}

/// 备用战车云台操控（C/B水平、F/V俯仰）
pub fn remote_gimbal_controls(
    time: Res<Time>,
    keyboard: Res<ButtonInput<KeyCode>>,
    config: Res<SimulationConfig>,
    gimbal: Single<
        (&mut Transform, &mut InfantryGimbal),
        (With<ActiveSlapper>, Without<InfantryChassis>),
    >,
) {
    let dt = time.delta_secs();
    let (mut gimbal_transform, mut gimbal_data) = gimbal.into_inner();

    (gimbal_data.local_yaw, gimbal_data.pitch, _) =
        gimbal_transform.rotation.to_euler(EulerRot::YXZ);

    // 按住左Shift锁定备用战车云台水平旋转，只能上下俯仰
    if !keyboard.pressed(KeyCode::ShiftLeft) {
        gimbal_data.local_yaw +=
            input!(keyboard, KeyC, KeyB) * config.vehicle.gimbal_rotation_speed * dt;
    }
    // F/V 控制俯仰
    gimbal_data.pitch += input!(keyboard, KeyF, KeyV) * config.vehicle.gimbal_rotation_speed * dt;
    gimbal_data.pitch = gimbal_data.pitch.clamp(
        -config.vehicle.gimbal_pitch_limit,
        config.vehicle.gimbal_pitch_limit,
    );

    let gimbal_rotation =
        Quat::from_euler(EulerRot::YXZ, gimbal_data.local_yaw, gimbal_data.pitch, 0.0);
    gimbal_transform.rotation = gimbal_rotation;
}

/// Tab键切换当前操控的备用战车（多车轮换控制）
pub fn switch_slapper_control(
    mut commands: Commands,
    keyboard: Res<ButtonInput<KeyCode>>,
    children: Query<&Children>,
    // 查询所有可切换的备用战车根实体
    slapper_roots: Query<Entity, (With<Infantry>, With<SlapperInfantry>)>,
    // 查询当前正在操控的备用战车
    active_root: Query<Entity, (With<Infantry>, With<SlapperInfantry>, With<ActiveSlapper>)>,
) {
    if !keyboard.just_pressed(KeyCode::Tab) {
        return;
    }

    let roots: Vec<Entity> = slapper_roots.iter().collect();
    // 只有一台备用战车时无需切换
    if roots.len() <= 1 {
        return;
    }

    // 获取当前激活战车下标
    let current = active_root.single().ok();
    let current_idx = current.and_then(|e| roots.iter().position(|&r| r == e));
    let next_idx = match current_idx {
        Some(idx) => (idx + 1) % roots.len(),
        None => 0,
    };

    // 1. 移除当前战车及其所有子实体的ActiveSlapper标记，解除操控
    if let Some(current_root) = current {
        commands.entity(current_root).remove::<ActiveSlapper>();
        for descendant in children.iter_descendants(current_root) {
            commands.entity(descendant).remove::<ActiveSlapper>();
        }
    }

    // 2. 给下一台战车及其所有子实体挂载ActiveSlapper，接管操控
    let next_root = roots[next_idx];
    commands.entity(next_root).insert(ActiveSlapper);
    for descendant in children.iter_descendants(next_root) {
        commands.entity(descendant).insert(ActiveSlapper);
    }
}

// 单元测试模块
#[cfg(test)]
mod tests {
    use super::*;

    // 测试：底盘转速平滑上升，不会瞬间拉满
    #[test]
    fn chassis_rotation_smoothly_ramps_towards_target_speed() {
        let mut transform = Transform::default();
        let mut chassis = InfantryChassis::default();

        update_chassis_rotation(&mut transform, &mut chassis, 1.0, 9.42, 0.016);

        assert!(chassis.yaw_velocity > 0.0);
        assert!(chassis.yaw_velocity < 9.42);
        assert!(chassis.yaw > 0.0);
    }

    // 测试：松开转向按键后，底盘平滑减速至静止
    #[test]
    fn chassis_rotation_smoothly_brakes_to_stop() {
        let mut transform = Transform::default();
        let mut chassis = InfantryChassis {
            yaw: 0.0,
            yaw_velocity: 9.42,
        };

        // 连续60帧无转向输入，观察刹车效果
        for _ in 0..60 {
            update_chassis_rotation(&mut transform, &mut chassis, 0.0, 9.42, 0.016);
        }

        assert!(chassis.yaw_velocity.abs() < 1e-2);
    }
}