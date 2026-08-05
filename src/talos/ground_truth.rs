// ====================================================================
// 模块名: talos::ground_truth
// 作用:   把仿真器内部的真值数据（机器人位姿、能量机关状态）发布到共享内存
// 职责:   1. 遍历所有步兵机器人（含受控与非受控）收集位姿与角速度
//         2. 遍历所有能量机关收集旋转角度、激活状态、正弦参数等
//         3. 组装为 GroundTruthBatch 一次性发布
// 说明:   真值数据用于离线评估 C++ 自瞄算法的检测/识别精度，
//         不参与闭环控制。所有坐标均通过对齐矩阵转到 ROS 坐标系。
// ====================================================================

use crate::components::{Controlled, Infantry};
use crate::robomaster::prelude::{
    Activation, MechanismState, PowerRune, PowerRuneMechanism, PowerRuneRotation, RuneMode, Team,
};
use crate::talos::capture::{TalosCaptureContext, TalosFrameStamp};
use crate::talos::plugin::M_ALIGN_MAT3;
use avian3d::prelude::AngularVelocity;
use bevy::prelude::*;
use talos_ipc::*;

/// 将 Bevy 三维向量转换到 ROS 坐标系
///
/// 直接左乘对齐矩阵 M_ALIGN_MAT3。
fn to_ros_vec3(v: Vec3) -> Vec3 {
    M_ALIGN_MAT3 * v
}

/// 队伍枚举转 u8 编码
///
/// Red = 0, Blue = 1，与 C++ 端约定一致。
fn team_to_u8(team: &Team) -> u8 {
    match team {
        Team::Red => 0,
        Team::Blue => 1,
    }
}

/// 激活状态枚举转 u8 编码
fn activation_to_u8(a: &Activation) -> u8 {
    match a {
        Activation::Deactivated => 0,
        Activation::Activating => 1,
        Activation::Activated => 2,
        Activation::Completed => 3,
    }
}

/// 机构状态枚举转 u8 编码
fn mechanism_state_to_u8(s: &MechanismState) -> u8 {
    match s {
        MechanismState::Inactive { .. } => 0,
        MechanismState::Activating(_) => 1,
        MechanismState::Activated { .. } => 2,
        MechanismState::Failed { .. } => 3,
    }
}

/// 能量机关模式枚举转 u8 编码
fn rune_mode_to_u8(m: &RuneMode) -> u8 {
    match m {
        RuneMode::Small => 0,
        RuneMode::Large => 1,
    }
}

/// Compute yaw in the ROS reference frame from a Bevy GlobalTransform.
///
/// The alignment matrix maps Bevy (Y-up) → ROS (Z-up).
/// We convert the rotation quaternion through the alignment to extract the Z-up yaw.
///
/// 算法步骤:
///   1. 由对齐矩阵构造四元数 align_quat
///   2. 对 Bevy 旋转做共轭变换得到 ROS 系下的旋转
///   3. 以 ZYX 欧拉序分解出 yaw（即 Z 轴旋转分量）
fn ros_yaw(global_tf: &GlobalTransform) -> f32 {
    let align_quat = Quat::from_mat3(&M_ALIGN_MAT3);
    let ros_rot = align_quat * global_tf.rotation() * align_quat.inverse();
    let (_, _, yaw) = ros_rot.to_euler(EulerRot::ZYX);
    yaw
}

/// 真值发布系统：每帧收集所有机器人与能量机关的真值并发布
///
/// 算法步骤:
///   1. 取出采集上下文与帧戳
///   2. 初始化 GroundTruthBatch
///   3. 遍历所有步兵（受控 + 非受控），填充 targets 数组
///   4. 遍历所有能量机关，填充 runes 数组
///   5. 加锁 publisher 一次性发布批量数据
///
/// 参数:
///   - context: 可选的采集上下文
///   - frame_stamp: 当前帧戳
///   - infantry_query: 非受控步兵查询
///   - controlled_query: 受控步兵查询
///   - rune_query: 能量机关查询
pub fn publish_ground_truth_system(
    context: Option<Res<TalosCaptureContext>>,
    frame_stamp: Res<TalosFrameStamp>,
    infantry_query: Query<
        (&GlobalTransform, Option<&AngularVelocity>, &Infantry),
        Without<Controlled>,
    >,
    controlled_query: Query<
        (&GlobalTransform, Option<&AngularVelocity>, &Infantry),
        With<Controlled>,
    >,
    rune_query: Query<(
        &GlobalTransform,
        &Transform,
        &PowerRune,
        &PowerRuneMechanism,
        &PowerRuneRotation,
    )>,
) {
    let Some(ctx) = context else {
        return;
    };

    let frame_seq = frame_stamp.frame_seq;
    let timestamp_ns = frame_stamp.timestamp_ns;

    let mut batch = GroundTruthBatch::default();
    batch.frame_seq = frame_seq;
    batch.timestamp_ns = timestamp_ns;

    // Collect robot ground truth from all infantry robots
    // 链接受控与非受控步兵迭代器，统一遍历
    let all_robots = infantry_query.iter().chain(controlled_query.iter());

    for (global_tf, ang_vel, infantry) in all_robots {
        // 位置转到 ROS 坐标系
        let pos_ros = to_ros_vec3(global_tf.translation());
        let team = &infantry.team;
        let config = infantry.config;

        // z 轴角速度：对 AngularVelocity 应用对齐矩阵后取 z 分量
        let vyaw = ang_vel
            .map(|av| {
                let ros_ang = to_ros_vec3(av.0);
                ros_ang.z
            })
            .unwrap_or(0.0);

        let yaw = ros_yaw(global_tf);

        // 容量检查：超出 GROUND_TRUTH_MAX_TARGETS 的目标被丢弃
        if (batch.target_count as usize) < GROUND_TRUTH_MAX_TARGETS {
            let idx = batch.target_count as usize;
            batch.targets[idx] = GroundTruthTarget {
                frame_seq,
                timestamp_ns,
                team: team_to_u8(team),
                armor_label: config.armor.label() as u8,
                is_outpost: 0,
                _pad1: 0,
                position: [pos_ros.x, pos_ros.y, pos_ros.z],
                vyaw,
                yaw,
                _pad: [0; 24],
            };
            batch.target_count += 1;
        }
    }

    // Collect rune ground truth
    for (global_tf, local_tf, power_rune, mechanism, rotation) in rune_query.iter() {
        // 容量检查：超出 GROUND_TRUTH_MAX_RUNES 直接 break
        if (batch.rune_count as usize) >= GROUND_TRUTH_MAX_RUNES {
            break;
        }

        let pos_ros = to_ros_vec3(global_tf.translation());

        // Extract current rotation angle around the actual rune axis (-1, 0, -1).
        // The rune rotates via `rotate_local_axis(direction, angle)`, so we must
        // project the quaternion back onto that axis — not extract an Euler X angle.
        // 能量机关绕 (-1, 0, -1) 轴旋转，不能直接取 Euler X 角，
        // 必须将四元数投影到该轴上还原实际旋转角度。
        let rune_axis = Dir3::from_xyz(-1.0, 0.0, -1.0).unwrap();
        let (axis, angle) = local_tf.rotation.to_axis_angle();
        // 通过点积符号修正旋转方向，得到有符号的当前角度
        let current_angle = angle * axis.dot(*rune_axis).signum();

        let controller = rotation.controller();
        // 顺时针 = 1，逆时针 = -1
        let direction = if controller.is_clockwise() { 1 } else { -1 };

        // 大能量机关有可变参数 (振幅, 角速度, 相对时间)，小能量机关则全为 0
        // sin_offset = 2.090 - amplitude，是固定相位基准
        let (sin_amplitude, sin_omega, relative_time, sin_offset) = controller
            .variable_params()
            .map(|(a, omega, t)| (a, omega, t, 2.090 - a))
            .unwrap_or((0.0, 0.0, 0.0, 0.0));

        // 收集 5 个扇叶的激活状态
        let mut target_activations = [0u8; 5];
        for (i, a) in mechanism.state().target_states().iter().enumerate() {
            if i < 5 {
                target_activations[i] = activation_to_u8(a);
            }
        }

        let idx = batch.rune_count as usize;
        batch.runes[idx] = GroundTruthRune {
            frame_seq,
            timestamp_ns,
            team: team_to_u8(&power_rune.team()),
            rune_mode: rune_mode_to_u8(&power_rune.mode()),
            mechanism_state: mechanism_state_to_u8(mechanism.state()),
            _pad1: 0,
            r_center_odom: [pos_ros.x, pos_ros.y, pos_ros.z],
            radius: 0.0,
            current_angle,
            v_roll: 0.0,
            direction,
            sin_amplitude,
            sin_omega,
            sin_phase: 0.0,
            sin_offset,
            relative_time,
            blade_id: -1,
            target_activations,
            _pad: [0; 20],
        };
        batch.rune_count += 1;
    }

    // 加锁 publisher 一次性发布批量真值
    if let Ok(mut publisher) = ctx.publisher.lock() {
        publisher.publish_ground_truth(&batch);
    }
}
