// 引入物理引擎所需：线速度、角速度组件
use avian3d::prelude::{AngularVelocity, LinearVelocity};
// Bevy 引擎基础能力
use bevy::prelude::*;

// 自定义组件：玩家操控标记、步兵战车本体标记
use crate::components::{Controlled, Infantry};
// 全局仿真配置、麦克纳姆底盘专属配置
use crate::config::{MecanumConfig, SimulationConfig};

// 麦克纳姆轮数量：四轮布局 FL 左前 / FR 右前 / RL 左后 / RR 右后
const NUM_WHEELS: usize = 4;
// 车轮半径最小值保护，防止除以 0
const MIN_RADIUS_M: f32 = 1e-6;

/// 【全局资源】单帧底盘观测数据包
/// 等效真实机器人：IMU + 四轮编码器融合后的一帧观测数据
#[derive(Resource, Debug, Clone)]
pub struct ChassisObservationFrame {
    pub stamp_s: f64,                // 时间戳(秒)
    pub dt_s: f32,                   // 本帧时间步长
    pub v_body: Vec2,                // 车体局部坐标系：x前进、y侧向平移速度 (m/s)
    pub wz_radps: f32,               // 车体绕Z轴旋转角速度 rad/s
    // 四轮线速度 [FL, FR, RL, RR] 单位 m/s
    pub wheel_linear_mps: [f32; NUM_WHEELS],
    // 四轮旋转角速度 rad/s
    pub wheel_angular_radps: [f32; NUM_WHEELS],
    pub a_body: Vec2,                // 车体平面加速度 ax ay
    pub alpha_z_radps2: f32,         // Z轴角加速度 rad/s²
    pub rpy_rad: Vec3,               // 车体欧拉角 roll/pitch/yaw 翻滚、俯仰、偏航
    pub gyro_xyz_radps: Vec3,        // 三轴陀螺仪原始角速度
    pub accel_xyz_mps2: Vec3,        // 三轴加速度计数值
}

// 默认初始化全部置零
impl Default for ChassisObservationFrame {
    fn default() -> Self {
        Self {
            stamp_s: 0.0,
            dt_s: 0.0,
            v_body: Vec2::ZERO,
            wz_radps: 0.0,
            wheel_linear_mps: [0.0; NUM_WHEELS],
            wheel_angular_radps: [0.0; NUM_WHEELS],
            a_body: Vec2::ZERO,
            alpha_z_radps2: 0.0,
            rpy_rad: Vec3::ZERO,
            gyro_xyz_radps: Vec3::ZERO,
            accel_xyz_mps2: Vec3::ZERO,
        }
    }
}

/// 【全局资源】上一帧底盘运动状态，用于差分求加速度
#[derive(Resource, Debug, Clone, Default)]
pub struct PreviousKinematicState {
    initialized: bool,    // 是否已经记录过上一帧有效数据
    v_body: Vec2,         // 上一帧车体速度
    wz_radps: f32,        // 上一帧转向角速度
}

/// 核心系统：每一帧更新底盘观测数据包
pub fn update_chassis_observation(
    time: Res<Time>,
    config: Res<SimulationConfig>,
    mut frame: ResMut<ChassisObservationFrame>,
    mut previous: ResMut<PreviousKinematicState>,
    // 查询唯一受控战车：全局位姿、世界线速度、世界角速度
    chassis: Query<
        (&GlobalTransform, &LinearVelocity, &AngularVelocity),
        (With<Infantry>, With<Controlled>),
    >,
) {
    // 如果找不到受控战车，观测帧清零、历史状态重置
    let Ok((chassis_tf, linear_velocity, angular_velocity)) = chassis.single() else {
        *frame = ChassisObservationFrame::default();
        *previous = PreviousKinematicState::default();
        return;
    };

    let stamp_s = time.elapsed_secs_f64(); // 程序启动累计时间戳
    let dt_s = time.delta_secs();          // 本帧与上一帧时间间隔
    let rotation = chassis_tf.compute_transform().rotation; // 战车当前姿态四元数

    // ========== 1. 世界坐标系速度 → 车体局部坐标系，并修正轴系差异 ==========
    // rotation.inverse()：世界速度逆旋转，转到战车自身局部坐标系
    let linear_local_bevy = rotation.inverse() * linear_velocity.0;
    // Bevy 默认坐标系与机器人车体坐标系不一致，调用函数做轴映射转换
    let linear_body = bevy_local_to_body(linear_local_bevy);
    // 提取车体前进速度x、侧向速度y
    let v_body = Vec2::new(linear_body.x, linear_body.y);

    // 角速度同理转换到车体坐标系
    let angular_local_bevy = rotation.inverse() * angular_velocity.0;
    let gyro_body = bevy_local_to_body(angular_local_bevy);
    let wz_radps = gyro_body.z; // 车体偏航角速度

    // ========== 2. 差分计算车体加速度、角加速度 ==========
    let (a_body, alpha_z_radps2) = compute_body_acceleration(&previous, v_body, wz_radps, dt_s);

    // ========== 3. 姿态四元数转换为车体欧拉角 RPY ==========
    let body_rotation = bevy_to_body_quat(rotation);
    let (roll, pitch, yaw) = body_rotation.to_euler(EulerRot::XYZ);

    // ========== 4. 麦克纳姆逆运动学：整车速度反推四个轮子线速度 ==========
    let wheel_linear_mps = mecanum_wheel_linear(v_body.x, v_body.y, wz_radps, &config.mecanum);
    // 轮子线速度 → 轮子旋转角速度 ω = v / r
    let wheel_angular_radps =
        wheel_linear_to_angular(wheel_linear_mps, config.mecanum.wheel_radius_m);

    // ========== 5. 组装完整观测帧 ==========
    *frame = ChassisObservationFrame {
        stamp_s,
        dt_s,
        v_body,
        wz_radps,
        wheel_linear_mps,
        wheel_angular_radps,
        a_body,
        alpha_z_radps2,
        rpy_rad: Vec3::new(roll, pitch, yaw),
        gyro_xyz_radps: gyro_body,
        accel_xyz_mps2: Vec3::new(a_body.x, a_body.y, 0.0),
    };

    // 更新历史状态，供下一帧差分计算加速度
    previous.initialized = true;
    previous.v_body = v_body;
    previous.wz_radps = wz_radps;
}

/// 差分求解加速度：a = Δv / Δt
fn compute_body_acceleration(
    previous: &PreviousKinematicState,
    v_body: Vec2,
    wz_radps: f32,
    dt_s: f32,
) -> (Vec2, f32) {
    // 未初始化 / 时间步几乎为0，避免除以0，加速度返回0
    if !previous.initialized || dt_s <= f32::EPSILON {
        return (Vec2::ZERO, 0.0);
    }

    let inv_dt = 1.0 / dt_s;
    (
        (v_body - previous.v_body) * inv_dt,
        (wz_radps - previous.wz_radps) * inv_dt,
    )
}

/// 麦克纳姆逆运动学公式
/// 输入车体 vx前进, vy横移, wz自转，输出四个车轮各自线速度 [FL,FR,RL,RR]
fn mecanum_wheel_linear(vx: f32, vy: f32, wz: f32, config: &MecanumConfig) -> [f32; NUM_WHEELS] {
    // k = 半轴距 + 半轮距，底盘几何常数
    let k = config.half_wheelbase_m + config.half_trackwidth_m;
    // 四轮麦克纳姆标准逆解公式
    [
        vx - vy - k * wz,
        vx + vy + k * wz,
        vx + vy - k * wz,
        vx - vy + k * wz,
    ]
}

/// 车轮线速度(m/s) → 车轮角速度(rad/s)
fn wheel_linear_to_angular(
    wheel_linear_mps: [f32; NUM_WHEELS],
    wheel_radius_m: f32,
) -> [f32; NUM_WHEELS] {
    // 半径下限保护，防止除0崩溃
    let radius = wheel_radius_m.max(MIN_RADIUS_M);
    wheel_linear_mps.map(|wheel_linear| wheel_linear / radius)
}

/// 坐标系转换1：Bevy引擎局部坐标轴 → 机器人车体标准坐标轴
/// Bevy局部：右X、上Y、后Z
/// 机器人车体：前进X、左Y、上Z
fn bevy_local_to_body(vector: Vec3) -> Vec3 {
    Vec3::new(-vector.z, -vector.x, vector.y)
}

/// 坐标系转换2：姿态四元数做轴系对齐修正
fn bevy_to_body_quat(rotation: Quat) -> Quat {
    let align = Quat::from_mat3(&Mat3::from_cols(
        Vec3::new(0.0, -1.0, 0.0),
        Vec3::new(0.0, 0.0, 1.0),
        Vec3::new(-1.0, 0.0, 0.0),
    ));
    // 四元数相似变换，把姿态旋转映射到车体坐标系
    align * rotation * align.inverse()
}

// 单元测试模块：校验麦克纳姆运动学正反解正确性
#[cfg(test)]
mod tests {
    use super::*;

    // 浮点数近似相等判断，规避浮点精度误差
    fn approx_eq(a: f32, b: f32) {
        assert!((a - b).abs() < 1e-5, "lhs={a}, rhs={b}");
    }

    // 构造测试用麦克纳姆底盘参数
    fn test_cfg() -> MecanumConfig {
        MecanumConfig {
            wheel_radius_m: 0.076,
            half_wheelbase_m: 0.18,
            half_trackwidth_m: 0.15,
        }
    }

    /// 麦克纳姆正运动学：四轮角速度 → 整车 vx vy wz
    fn mecanum_forward_from_angular(
        wheel_angular_radps: [f32; NUM_WHEELS],
        config: &MecanumConfig,
    ) -> (f32, f32, f32) {
        let r = config.wheel_radius_m;
        let k = config.half_wheelbase_m + config.half_trackwidth_m;
        let [fl, fr, rl, rr] = wheel_angular_radps;

        // 四轮转速合成车体速度公式
        let vx = r * (fl + fr + rl + rr) * 0.25;
        let vy = r * (-fl + fr + rl - rr) * 0.25;
        let wz = r * (-fl + fr - rl + rr) / (4.0 * k);
        (vx, vy, wz)
    }

    // 测试：纯前进时四个轮子线速度完全一致
    #[test]
    fn inverse_forward_motion_has_same_sign_and_magnitude() {
        let cfg = test_cfg();
        let linear = mecanum_wheel_linear(1.2, 0.0, 0.0, &cfg);
        approx_eq(linear[0], linear[1]);
        approx_eq(linear[1], linear[2]);
        approx_eq(linear[2], linear[3]);
    }

    // 测试：纯横移左右轮子速度对称反向
    #[test]
    fn inverse_lateral_motion_is_symmetric() {
        let cfg = test_cfg();
        let linear = mecanum_wheel_linear(0.0, 0.8, 0.0, &cfg);
        approx_eq(linear[0], -linear[1]);
        approx_eq(linear[2], -linear[3]);
        approx_eq(linear[0], linear[3]);
    }

    // 测试：原地自转时轮子速度分布符合麦克纳姆规律
    #[test]
    fn inverse_spin_motion_has_expected_pattern() {
        let cfg = test_cfg();
        let linear = mecanum_wheel_linear(0.0, 0.0, 2.0, &cfg);
        approx_eq(linear[0], -linear[1]);
        approx_eq(linear[0], linear[2]);
        approx_eq(linear[1], linear[3]);
    }

    // 核心闭环测试：整车速度反解轮速，再由轮速正解还原整车速度，结果一致
    #[test]
    fn inverse_then_forward_roundtrip_is_consistent() {
        let cfg = test_cfg();
        // 多组随机运动样本校验
        let samples = [(0.5, 0.3, 1.2), (1.1, -0.4, -0.7), (-0.6, 0.2, 0.9)];

        for (vx, vy, wz) in samples {
            let linear = mecanum_wheel_linear(vx, vy, wz, &cfg);
            let angular = wheel_linear_to_angular(linear, cfg.wheel_radius_m);
            let (vx_back, vy_back, wz_back) = mecanum_forward_from_angular(angular, &cfg);
            approx_eq(vx_back, vx);
            approx_eq(vy_back, vy);
            approx_eq(wz_back, wz);
        }
    }

    // 无历史状态时加速度输出为0
    #[test]
    fn acceleration_is_zero_without_history() {
        let previous = PreviousKinematicState::default();
        let (accel, alpha) = compute_body_acceleration(&previous, Vec2::new(1.0, 1.0), 0.5, 0.01);
        approx_eq(accel.x, 0.0);
        approx_eq(accel.y, 0.0);
    }
}