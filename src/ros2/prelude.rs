// ============================================================================
// 模块名：ros2::prelude
// 作  用：ROS2 模块的公共导出与工具集，提供坐标转换、频率限制与 TF 构造宏
// 职  责：
//   1. 提供 `AverageRateLimiter`：基于时间预算的平均频率限制器
//   2. 定义 Bevy 到 ROS 的坐标系对齐矩阵 `M_ALIGN_MAT3` 与 `transform` 转换函数
//   3. 提供 `add_tf_frame!` 宏：向 TF 列表追加一帧 `TransformStamped`
//   4. 提供 `pose!` 宏：构造零偏移的 `PoseStamped`（用于坐标系原点发布）
// ============================================================================

use bevy::math::{Mat3, Quat, Vec3};
use bevy::prelude::Transform;
use std::time::Duration;

/// 平均频率限制器：基于时间预算的令牌桶式限速器。
///
/// 内部维护一个 `elapsed` 累计时间，每帧通过 `tick` 累加 delta。
/// 当 `elapsed >= period` 时，`allow` 返回 true 并重置预算。
///
/// 与简单的"每 N 帧一次"限速器不同，本实现基于实际时间间隔，
/// 即使帧率波动也能保持平均频率稳定。
#[derive(Debug, Clone)]
pub struct AverageRateLimiter {
    /// 目标周期（= 1 / 目标频率）
    period: Duration,
    /// 已累积的时间预算，上限为 `period`
    elapsed: Duration,
}

impl AverageRateLimiter {
    /// 创建指定周期的限速器。
    pub fn new(period: Duration) -> Self {
        Self {
            period,
            // 初始化为 period，使首次 allow() 立即返回 true
            elapsed: period,
        }
    }

    /// 按目标频率（Hz）创建限速器。
    ///
    /// # 参数
    /// - `hz`：目标频率，必须为正有限值
    pub fn from_hz(hz: f32) -> Self {
        assert!(hz.is_finite() && hz > 0.0);
        Self::new(Duration::from_secs_f32(1.0 / hz))
    }

    /// 推进限速器：累加本帧的 delta 时间，但不超过 `period`。
    pub fn tick(&mut self, delta: Duration) {
        self.elapsed = self.elapsed.saturating_add(delta).min(self.period);
    }

    /// 检查是否允许执行：若预算充足则重置并返回 true。
    pub fn allow(&mut self) -> bool {
        if self.elapsed < self.period {
            return false;
        }
        // 消耗全部预算
        self.elapsed = Duration::ZERO;
        true
    }
}

/// Bevy 到 ROS 的坐标系对齐矩阵（右手系转换）。
///
/// Bevy 使用 Y-Up、Z-Forward 的左手/右手混合约定，
/// ROS 使用 Z-Up、X-Forward 的右手系（REP-103）。
///
/// 该矩阵实现如下映射：
///   - ROS X（前）← Bevy Z（前）：M[0,2] = -1
///   - ROS Y（左）← Bevy X（右）：M[1,0] = -1（取反得到左）
///   - ROS Z（上）← Bevy Y（上）：M[2,1] = 1
///
/// 列向量含义（Mat3::from_cols 的参数顺序为列）：
///   - 第一列 [0, 0, -1]：原 X 轴变换后的新基
///   - 第二列 [-1, 0, 0]：原 Y 轴变换后的新基
///   - 第三列 [0, 1, 0]：原 Z 轴变换后的新基
pub const M_ALIGN_MAT3: Mat3 = Mat3::from_cols(
    Vec3::new(0.0, -1.0, 0.0), // M[0,0], M[1,0], M[2,0]
    Vec3::new(0.0, 0.0, 1.0),  // M[0,1], M[1,1], M[2,1]
    Vec3::new(-1.0, 0.0, 0.0), // M[0,2], M[1,2], M[2,2]
);

/// 将 Bevy 的 `Transform` 转换为 ROS2 的 `geometry_msgs/Transform`。
///
/// 应用 `M_ALIGN_MAT3` 对齐坐标系：
///   - 平移：`new_translation = M_ALIGN_MAT3 * bevy_translation`
///   - 旋转：`new_rotation = align_quat * bevy_rotation * align_quat^{-1}`
///
/// 旋转的共轭变换（左乘 align、右乘 align 逆）保证旋转在两个坐标系中描述一致。
///
/// # 参数
/// - `bevy_transform`：Bevy 坐标系下的 Transform
///
/// # 返回值
/// 返回 ROS2 坐标系下的 `r2r::geometry_msgs::msg::Transform`（f64 精度）。
#[inline]
pub fn transform(bevy_transform: Transform) -> r2r::geometry_msgs::msg::Transform {
    let align_rot_mat = M_ALIGN_MAT3;
    // 将对齐矩阵转换为四元数，用于旋转的共轭变换
    let align_quat = Quat::from_mat3(&align_rot_mat);
    // 旋转：先将对齐四元数左乘，再右乘其逆（共轭变换）
    let new_rotation = align_quat * bevy_transform.rotation * align_quat.inverse();
    // 平移：直接用对齐矩阵变换
    let new_translation = align_rot_mat * bevy_transform.translation;
    r2r::geometry_msgs::msg::Transform {
        translation: r2r::geometry_msgs::msg::Vector3 {
            x: new_translation.x as f64,
            y: new_translation.y as f64,
            z: new_translation.z as f64,
        },
        rotation: r2r::geometry_msgs::msg::Quaternion {
            x: new_rotation.x as f64,
            y: new_rotation.y as f64,
            z: new_rotation.z as f64,
            w: new_rotation.w as f64,
        },
    }
}

/// 向 TF 列表追加一帧 `TransformStamped`。
///
/// 提供两种形式：
///   1. 分离平移与旋转：`add_tf_frame!($ls, $hdr, $id, $translation, $rotation)`
///   2. 完整 Transform：`add_tf_frame!($ls, $hdr, $id, $transform)`
///
/// 内部调用 `transform` 函数完成 Bevy 到 ROS 的坐标转换。
#[macro_export]
macro_rules! add_tf_frame {
    // 分离平移与旋转形式：构造 Transform::IDENTITY 后填充平移与旋转
    ($ls:ident, $hdr:expr, $id:expr, $translation:expr, $rotation:expr) => {
        $ls.push(::r2r::geometry_msgs::msg::TransformStamped {
            header: $hdr.clone(),
            child_frame_id: $id.to_string(),
            transform: $crate::ros2::prelude::transform(
                ::bevy::prelude::Transform::IDENTITY
                    .with_translation($translation)
                    .with_rotation($rotation),
            ),
        });
    };
    // 完整 Transform 形式：直接传入 Transform
    ($ls:ident, $hdr:expr, $id:expr, $transform:expr) => {
        $ls.push(::r2r::geometry_msgs::msg::TransformStamped {
            header: $hdr.clone(),
            child_frame_id: $id.to_string(),
            transform: $crate::ros2::prelude::transform($transform),
        });
    };
}

/// 构造零偏移的 `PoseStamped`（位置为零、姿态为单位四元数）。
///
/// 用于在 TF 树中发布某个坐标系的原点位姿：因为子坐标系相对父坐标系的
/// 平移与旋转已经体现在 TransformStamped 中，这里只需发布"原点"即可，
/// 订阅端通过 frame_id 关联到对应坐标系。
#[macro_export]
macro_rules! pose {
    ($hdr:expr) => {
        ::r2r::geometry_msgs::msg::PoseStamped {
            header: $hdr.clone(),
            pose: ::r2r::geometry_msgs::msg::Pose {
                // 位置：原点
                position: ::r2r::geometry_msgs::msg::Point {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                },
                // 姿态：单位四元数（无旋转）
                orientation: ::r2r::geometry_msgs::msg::Quaternion {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    w: 1.0,
                },
            },
        }
    };
}
