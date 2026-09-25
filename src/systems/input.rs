// Bevy ECS 核心基础库，包含实体、组件、资源、变换、输入等基础类型
use bevy::prelude::*;
// 原子类型内存序，多线程无锁读写布尔标记，用于自瞄开关
use std::sync::atomic::Ordering;
// 项目自定义组件模块
use crate::components::{
    ActiveSlapper,        // 标记当前正在被操控的副战车实体（备用战车）
    Controlled,           // 玩家主操控战车标记，主战车带此组件
    Infantry,             // 战车本体标签组件，区分战车实体和云台/底盘子实体
    InfantryChassis,      // 底盘状态组件：保存底盘yaw偏航角、yaw角速度
    InfantryGimbal,       // 云台状态组件：云台本地偏航、俯仰角
    SlapperInfantry,      // 可切换操控的备用战车标签，所有副战车都挂载
    SubscribeAutoAim,     // 全局资源（原子bool）：全局自瞄订阅开关
};
// 全局仿真配置结构体，存放战车最大速度、转速、云台限位等参数
use crate::config::SimulationConfig;
// 麦克纳姆轮底盘动力学模型，实现全向移动力计算
use crate::robomaster::vehicle::movement::VehicleDynamic;
// Avian3D 物理引擎（Bevy新一代物理引擎，替代旧Rapier），刚体受力、质量等
use avian3d::prelude::*;

/// 自定义声明宏 input!，重载两套实现，消除重复键盘按键检测代码
/// 宏重载1：读取前后左右按键，返回Vec2二维输入向量，用于底盘平面平移
macro_rules! input {
    // 模式1：5个参数：键盘资源 + 前、左、后、右按键，返回Vec2
    ($keyboard:ident, $forward:ident,$left:ident,$backward:ident,$right:ident) => {{
        // 初始化输入向量为零向量
        let mut input = Vec2::ZERO;
        // 前进按键按下，Y轴+1
        if $keyboard.pressed(KeyCode::$forward) {
            input.y += 1.0;
        }
        // 后退按键按下，Y轴-1
        if $keyboard.pressed(KeyCode::$backward) {
            input.y -= 1.0;
        }
        // 右移按键按下，X轴+1
        if $keyboard.pressed(KeyCode::$right) {
            input.x += 1.0;
        }
        // 左移按键按下，X轴-1
        if $keyboard.pressed(KeyCode::$left) {
            input.x -= 1.0;
        }
        input
    }};
    // 模式2：2个参数：键盘资源 + 左转、右转按键，返回f32一维转向量 [-1,1]
    ($keyboard:ident, $left:ident,$right:ident) => {{
        let mut input: f32 = 0.0;
        // 左转按键，输入+1
        if $keyboard.pressed(KeyCode::$left) {
            input += 1.0;
        }
        // 右转按键，输入-1
        if $keyboard.pressed(KeyCode::$right) {
            input += -1.0;
        }
        input
    }};
}

// ===================== 底盘旋转调参常量 =====================
/// 底盘旋转平滑惯性系数：数值越大，加速/刹车响应越快；90为快速响应
const CHASSIS_ROTATION_RESPONSE: f32 = 90.0;
/// 底盘角速度死区阈值：角速度小于该值直接置0，消除微小漂移抖动
const CHASSIS_ROTATION_STOP_EPSILON: f32 = 1e-3;

/**
 * @brief 底盘平滑旋转更新函数：一阶指数低通滤波，模拟电机惯性，实现缓启停
 * @param chassis_transform 底盘实体Transform，用于写入旋转姿态
 * @param chassis_data 底盘状态组件，保存yaw角度与角速度
 * @param input 转向操纵输入，范围 [-1, 1]
 * @param rotation_speed 底盘最大旋转角速度 rad/s
 * @param dt 帧时间增量（秒）
 */
fn update_chassis_rotation(
    chassis_transform: &mut Transform,
    chassis_data: &mut InfantryChassis,
    input: f32,
    rotation_speed: f32,
    dt: f32,
) {
    // 目标角速度 = 操纵输入 × 最大转速
    let target_yaw_velocity = input * rotation_speed;
    // 指数平滑alpha系数：alpha = 1-exp(-k*dt)，经典一阶低通滤波
    // k就是CHASSIS_ROTATION_RESPONSE，控制收敛速度
    let response = 1.0 - (-CHASSIS_ROTATION_RESPONSE * dt).exp();
    // 低通滤波更新角速度：逐步向目标角速度逼近，不会阶跃突变
    chassis_data.yaw_velocity += (target_yaw_velocity - chassis_data.yaw_velocity) * response;

    // 死区逻辑：无输入 + 当前转速极低 → 强制清零角速度，消除静止漂移抖动
    if chassis_data.yaw_velocity.abs() < CHASSIS_ROTATION_STOP_EPSILON
        && target_yaw_velocity.abs() < CHASSIS_ROTATION_STOP_EPSILON
    {
        chassis_data.yaw_velocity = 0.0;
    }

    // 角速度积分，累加得到底盘总偏航角yaw
    chassis_data.yaw += chassis_data.yaw_velocity * dt;
    // 欧拉角YXZ：只绕Y轴旋转（水平面原地旋转），俯仰/横滚固定0
    chassis_transform.rotation = Quat::from_euler(EulerRot::YXZ, chassis_data.yaw, 0.0, 0.0);
}

/**
 * @brief ECS系统：F5按键切换全局自动瞄准开关
 * @param keyboard 键盘输入资源
 * @param enabled 全局原子布尔资源：自瞄开关
 */
pub fn auto_aim_switch(keyboard: Res<ButtonInput<KeyCode>>, enabled: Res<SubscribeAutoAim>) {
    // just_pressed：仅按键按下瞬间触发一次，按住不重复执行
    if keyboard.just_pressed(KeyCode::F5) {
        info!("Toggling auto-aim subscription.");
        // fetch_xor：原子异或翻转布尔值；AcqRel内存序：读+写，多线程安全
        // fetch_xor返回翻转**之前**的值，所以新状态需要取反
        let new_state = !enabled.fetch_xor(true, Ordering::AcqRel);
        info!(
            "Auto-aim subscription is now {}.",
            if new_state { "ENABLED" } else { "DISABLED" }
        );
    }
}

/**
 * @brief ECS系统：主战车底盘WASD移动控制系统
 * 主战车标记：With<Infantry> + With<Controlled>
 */
pub fn vehicle_controls(
    time: Res<Time>,
    keyboard: Res<ButtonInput<KeyCode>>,
    config: Res<SimulationConfig>,
    // Single：查询**唯一**匹配实体，不满足唯一会panic
    // 主战车刚体受力、质量、麦克纳姆轮动力学结构体
    infantry: Single<(Forces, &Mass, &mut VehicleDynamic), (With<Infantry>, With<Controlled>)>,
    // 云台全局姿态：主战车云台实体，带Controlled，不带底盘组件
    gimbal: Single<
        (&GlobalTransform, &InfantryGimbal),
        (With<Controlled>, Without<InfantryChassis>),
    >,
    // 底盘实体变换 + 底盘状态组件：主战车底盘，有Controlled、InfantryChassis，无云台、无Infantry本体标签
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
    // WASD采集二维平面移动输入向量
    let input = input!(keyboard, KeyW, KeyA, KeyS, KeyD);
    // 左Shift开启2倍冲刺加速，否则倍率1.0
    let boost = if keyboard.pressed(KeyCode::ShiftLeft) {
        2.0
    } else {
        1.0
    };
    // 解包Single查询结果
    let (mut forces, &Mass(mass), mut dynamic) = infantry.into_inner();
    let dt = time.delta_secs();

    // 麦克纳姆轮动力学计算：施加平面移动力，实现全向平移
    // 输入：物理力组件、车体质量、云台全局姿态、摇杆输入向量、帧间隔、加速倍率
    dynamic.linear(
        &mut forces,
        mass,
        gimbal.into_inner().0,
        input,
        time.delta_secs(),
        boost,
    );

    // Q/E按键采集底盘原地旋转输入
    let input = input!(keyboard, KeyQ, KeyE);
    let (mut chassis_transform, mut chassis_data) = chassis.into_inner();
    // 执行底盘平滑旋转更新
    update_chassis_rotation(
        &mut chassis_transform,
        &mut chassis_data,
        input,
        config.vehicle.rotation_speed,
        dt,
    );
}

/**
 * @brief ECS系统：备用战车远程底盘控制系统
 * 被选中的备用战车标记：ActiveSlapper，不带Controlled
 */
pub fn remote_vehicle_controls(
    time: Res<Time>,
    keyboard: Res<ButtonInput<KeyCode>>,
    config: Res<SimulationConfig>,
    // 激活的备用战车刚体、质量、动力学
    infantry: Single<
        (Forces, &Mass, &mut VehicleDynamic),
        (With<ActiveSlapper>, With<Infantry>, Without<Controlled>),
    >,
    // 备用战车云台全局姿态
    gimbal: Single<
        (&GlobalTransform, &InfantryGimbal),
        (With<ActiveSlapper>, Without<InfantryChassis>),
    >,
    // 备用战车底盘Transform与底盘状态
    chassis: Single<
        (&mut Transform, &mut InfantryChassis),
        (With<ActiveSlapper>, Without<InfantryGimbal>),
    >,
) {
    // IJKL 控制备用战车前后左右平移
    let input = input!(keyboard, KeyI, KeyJ, KeyK, KeyL);
    // 右Shift开启备用战车冲刺加速2倍
    let boost = if keyboard.pressed(KeyCode::ShiftRight) {
        2.0
    } else {
        1.0
    };
    let (mut forces, &Mass(mass), mut dynamic) = infantry.into_inner();
    let dt = time.delta_secs();

    // 麦克纳姆轮全向移动力计算
    dynamic.linear(
        &mut forces,
        mass,
        gimbal.into_inner().0,
        input,
        time.delta_secs(),
        boost,
    );

    // U/O 控制备用战车底盘原地旋转
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

/**
 * @brief ECS系统：主战车云台手动操控
 * 方向键控制炮管云台水平旋转、俯仰抬升
 */
pub fn gimbal_controls(
    time: Res<Time>,
    keyboard: Res<ButtonInput<KeyCode>>,
    config: Res<SimulationConfig>,
    // enabled: Res<SubscribeAutoAim>, // 注释：自瞄开关资源，暂时注释掉控制权互斥逻辑
    gimbal: Single<
        (&mut Transform, &mut InfantryGimbal),
        (With<Controlled>, Without<InfantryChassis>),
    >,
) {
    // 原逻辑：开启自瞄时直接return，禁止手动云台，防止手动与自瞄争夺控制权
    //if enabled.load(Ordering::Acquire) {
    //    return;
    //}
    let dt = time.delta_secs();
    let (mut gimbal_transform, mut gimbal_data) = gimbal.into_inner();

    // 从云台当前四元数，解析欧拉角YXZ，读取本地偏航、俯仰角存入gimbal_data
    (gimbal_data.local_yaw, gimbal_data.pitch, _) =
        gimbal_transform.rotation.to_euler(EulerRot::YXZ);

    // ← → 方向键：云台水平旋转（local_yaw）
    gimbal_data.local_yaw +=
        input!(keyboard, ArrowLeft, ArrowRight) * config.vehicle.gimbal_rotation_speed * dt;
    // ↑ ↓ 方向键：云台俯仰角（pitch）
    gimbal_data.pitch +=
        input!(keyboard, ArrowUp, ArrowDown) * config.vehicle.gimbal_rotation_speed * dt;

    // 俯仰硬限位，防止炮管翻转卡死，限制在[-limit, limit]区间
    gimbal_data.pitch = gimbal_data.pitch.clamp(
        -config.vehicle.gimbal_pitch_limit,
        config.vehicle.gimbal_pitch_limit,
    );

    // 根据更新后的偏航、俯仰重新生成云台旋转四元数，写入实体Transform
    let gimbal_rotation =
        Quat::from_euler(EulerRot::YXZ, gimbal_data.local_yaw, gimbal_data.pitch, 0.0);
    gimbal_transform.rotation = gimbal_rotation;
}

/**
 * @brief ECS系统：备用战车云台操控
 * C/B水平旋转；F/V俯仰；左Shift锁定水平，仅能上下俯仰
 */
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
    // 解析当前云台欧拉角
    (gimbal_data.local_yaw, gimbal_data.pitch, _) =
        gimbal_transform.rotation.to_euler(EulerRot::YXZ);

    // 不按左Shift，才允许云台水平旋转；按住Shift锁定水平，只能俯仰
    if !keyboard.pressed(KeyCode::ShiftLeft) {
        gimbal_data.local_yaw +=
            input!(keyboard, KeyC, KeyB) * config.vehicle.gimbal_rotation_speed * dt;
    }
    // F/V控制云台俯仰
    gimbal_data.pitch += input!(keyboard, KeyF, KeyV) * config.vehicle.gimbal_rotation_speed * dt;
    // 俯仰硬限位保护
    gimbal_data.pitch = gimbal_data.pitch.clamp(
        -config.vehicle.gimbal_pitch_limit,
        config.vehicle.gimbal_pitch_limit,
    );

    // 生成云台姿态
    let gimbal_rotation =
        Quat::from_euler(EulerRot::YXZ, gimbal_data.local_yaw, gimbal_data.pitch, 0.0);
    gimbal_transform.rotation = gimbal_rotation;
}

/**
 * @brief ECS系统：Tab按键切换备用战车（多台副战车轮换操控）
 * 切换逻辑：摘除当前ActiveSlapper，给下一台战车挂载ActiveSlapper
 * 注意：战车是父子实体结构，需要递归给**全部子实体**增删ActiveSlapper标记
 */
pub fn switch_slapper_control(
    mut commands: Commands,
    keyboard: Res<ButtonInput<KeyCode>>,
    children: Query<&Children>,
    // 查询所有可切换备用战车根实体：Infantry本体 + SlapperInfantry标签
    slapper_roots: Query<Entity, (With<Infantry>, With<SlapperInfantry>)>,
    // 查询当前正在激活操控的备用战车根实体
    active_root: Query<Entity, (With<Infantry>, With<SlapperInfantry>, With<ActiveSlapper>)>,
) {
    // Tab刚按下瞬间执行一次，按住不重复执行
    if !keyboard.just_pressed(KeyCode::Tab) {
        return;
    }
    // 收集全部备用战车根实体列表
    let roots: Vec<Entity> = slapper_roots.iter().collect();
    // 备用战车数量≤1，无需切换，直接返回
    if roots.len() <= 1 {
        return;
    }

    // 获取当前激活战车实体，.ok()处理无激活实体的情况
    let current = active_root.single().ok();
    // 查找当前激活战车在roots数组中的下标
    let current_idx = current.and_then(|e| roots.iter().position(|&r| r == e));
    // 环形索引：当前idx+1，取模；无激活战车则默认0号
    let next_idx = match current_idx {
        Some(idx) => (idx + 1) % roots.len(),
        None => 0,
    };

    // ========= 1. 清除旧战车的ActiveSlapper标记 =========
    if let Some(current_root) = current {
        // 移除根实体标记
        commands.entity(current_root).remove::<ActiveSlapper>();
        // iter_descendants：递归遍历根实体下**所有后代子实体**（底盘、云台等），一并移除标记
        for descendant in children.iter_descendants(current_root) {
            commands.entity(descendant).remove::<ActiveSlapper>();
        }
    }

    // ========= 2. 给下一台战车挂载ActiveSlapper =========
    let next_root = roots[next_idx];
    commands.entity(next_root).insert(ActiveSlapper);
    // 递归给所有子实体添加ActiveSlapper，保证子实体查询过滤生效
    for descendant in children.iter_descendants(next_root) {
        commands.entity(descendant).insert(ActiveSlapper);
    }
}

// ===================== 单元测试模块 =====================
#[cfg(test)]
mod tests {
    use super::*;

    /// 单元测试：底盘角速度平滑上升，不会瞬间跳变到目标最大转速
    #[test]
    fn chassis_rotation_smoothly_ramps_towards_target_speed() {
        let mut transform = Transform::default();
        let mut chassis = InfantryChassis::default();
        // 执行一帧更新，输入1.0，最大角速度9.42rad/s，dt=0.016s（约60fps）
        update_chassis_rotation(&mut transform, &mut chassis, 1.0, 9.42, 0.016);
        // 角速度大于0，但是小于目标最大转速，证明滤波生效，不是阶跃
        assert!(chassis.yaw_velocity > 0.0);
        assert!(chassis.yaw_velocity < 9.42);
        // 底盘yaw角度有积分增量
        assert!(chassis.yaw > 0.0);
    }

    /// 单元测试：松开转向按键后，底盘平滑减速，最终趋近静止
    #[test]
    fn chassis_rotation_smoothly_brakes_to_stop() {
        let mut transform = Transform::default();
        // 初始角速度拉满9.42rad/s
        let mut chassis = InfantryChassis {
            yaw: 0.0,
            yaw_velocity: 9.42,
        };
        // 连续60帧，输入0（无转向），模拟松开按键
        for _ in 0..60 {
            update_chassis_rotation(&mut transform, &mut chassis, 0.0, 9.42, 0.016);
        }
        // 角速度衰减到很小阈值以内
        assert!(chassis.yaw_velocity.abs() < 1e-2);
    }
}
