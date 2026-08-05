// ============================================================
// 模块名：outpost/rotation
// 作用：前哨站旋转运动控制
// 职责：定义旋转方向枚举与旋转控制器，负责按帧驱动前哨站绕
//       竖直轴（Y 轴）的匀速旋转，是前哨站动态仿真的核心。
// ============================================================

use crate::robomaster::outpost::consts::ROTATION_SPEED;
use bevy::prelude::Transform;

/// 旋转方向枚举，表示前哨站绕竖直轴的旋转方向。
///
/// 比赛中红、蓝双方前哨站的旋转方向相反，以模拟真实赛场上
/// 两座前哨站分别按相反方向旋转的物理事实。
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum RotationDirection {
    /// 顺时针旋转（红方前哨站方向）。
    Clockwise,
    /// 逆时针旋转（蓝方前哨站方向）。
    CounterClockwise,
}

impl RotationDirection {
    /// 返回该方向对应的符号因子。
    ///
    /// 顺时针返回 `1.0`，逆时针返回 `-1.0`，
    /// 用于在计算旋转角度时统一为带符号的标量，便于和旋转速度相乘。
    ///
    /// 返回值：`f32` 类型的方向符号（+1.0 或 -1.0）。
    pub const fn sign(self) -> f32 {
        match self {
            Self::Clockwise => 1.0,
            Self::CounterClockwise => -1.0,
        }
    }
}

/// 旋转控制器，封装前哨站的旋转速度与方向。
///
/// 该结构体不直接挂载到实体上，而是由 `OutpostRotator` 组件持有，
/// 负责按时间步长驱动 `Transform` 的旋转更新。
pub struct RotationController {
    /// 旋转角速度（弧度/秒），取自 `consts::ROTATION_SPEED`。
    speed: f32,
    /// 旋转方向。
    direction: RotationDirection,
}

impl RotationController {
    /// 创建一个新的旋转控制器。
    ///
    /// 参数：
    /// - `direction`：旋转方向（顺时针或逆时针）。
    ///
    /// 返回值：使用默认旋转速度和指定方向的 `RotationController` 实例。
    pub fn new(direction: RotationDirection) -> Self {
        Self {
            speed: ROTATION_SPEED,
            direction,
        }
    }

    /// 对指定 `Transform` 绕 Y 轴旋转给定角度。
    ///
    /// 参数：
    /// - `transform`：待旋转的变换引用（可变借用）。
    /// - `angle`：旋转角度（弧度），正值表示顺时针，负值表示逆时针。
    fn rotate(&self, transform: &mut Transform, angle: f32) {
        // 绕局部 Y 轴（竖直轴）旋转，符合前哨站实际旋转方式
        transform.rotate_y(angle);
    }

    /// 按时间步长推进一次旋转更新。
    ///
    /// 算法步骤：
    /// 1. 根据方向符号、角速度和时间步长计算本帧旋转角度：
    ///    `角度 = 方向符号 * 角速度 * 时间步长`。
    /// 2. 调用 `rotate` 将该角度应用到 `Transform` 上。
    ///
    /// 参数：
    /// - `transform`：待更新的变换引用（可变借用）。
    /// - `dt`：本帧时间步长（秒）。
    pub fn step(&self, transform: &mut Transform, dt: f32) {
        // 计算本帧旋转角度：方向符号 × 角速度 × 时间步长
        self.rotate(transform, self.direction.sign() * self.speed * dt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证旋转方向的符号因子与历史布尔实现保持一致。
    /// 顺时针应为 +1.0，逆时针应为 -1.0。
    #[test]
    fn rotation_direction_sign_matches_legacy_bool() {
        assert_eq!(RotationDirection::Clockwise.sign(), 1.0);
        assert_eq!(RotationDirection::CounterClockwise.sign(), -1.0);
    }
}
