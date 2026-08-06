//! 能量机关的旋转控制。
//!
//! 定义了机关旋转的核心逻辑，包括小机关的恒定角速度旋转
//! 和大机关在激活状态下的正弦变速旋转。
//! 通过 `RotationController` 和 `PowerRuneRotation` 组件封装旋转行为。

use crate::robomaster::power_rune::common::RuneMode;
use crate::robomaster::power_rune::consts::ROTATION_BASELINE_SMALL;
use bevy::math::Dir3;
use bevy::prelude::{Component, Transform};
use rand::{Rng, RngExt};

/// 正弦变速旋转参数，用于大机关激活时的可变转速。
struct VariableRotation {
    /// 正弦振幅系数，随机范围 [0.780, 1.045]。
    a: f32,
    /// 正弦角频率，随机范围 [1.884, 2.0]。
    omega: f32,
    /// 累计时间（秒），用于计算当前正弦值。
    t: f32,
}

impl VariableRotation {
    /// 创建随机参数的正弦变速旋转。
    pub fn random(rng: &mut impl Rng) -> Self {
        let a = rng.random_range(0.780..=1.045);
        let omega = rng.random_range(1.884..=2.0);
        Self { a, omega, t: 0.0 }
    }

    /// 推进时间累积。
    pub fn advance(&mut self, dt: f32) {
        self.t += dt;
    }

    /// 计算当前速度：`a * sin(omega * t) + b`，其中 `b = 2.090 - a`。
    ///
    /// 速度在 `[0, 2*2.090 - 2*a]` 范围内变化，保证非负。
    pub fn speed(&self) -> f32 {
        let b = 2.090 - self.a;
        self.a * (self.omega * self.t).sin() + b
    }
}

/// 旋转控制器，管理机关面的旋转速度与方向。
///
/// 小机关始终以恒定角速度旋转；大机关在激活状态下使用正弦变速旋转。
pub struct RotationController {
    /// 基础角速度（小机关固定值，大机关非激活时使用）。
    baseline: f32,
    /// 旋转轴方向。
    direction: Dir3,
    /// 大机关激活时的正弦变速旋转参数，`None` 表示使用恒定速度。
    variable: Option<VariableRotation>,
    /// 旋转方向：`true` 为顺时针，`false` 为逆时针。
    clockwise: bool,
}

impl RotationController {
    /// 创建新的旋转控制器。
    ///
    /// # 参数
    /// - `clockwise`：`true` 为顺时针旋转，`false` 为逆时针
    pub fn new(clockwise: bool) -> Self {
        Self {
            baseline: ROTATION_BASELINE_SMALL,
            direction: Dir3::from_xyz(-1.0, 0.0, -1.0).unwrap(),
            variable: None,
            clockwise,
        }
    }

    /// 获取正弦变速旋转的参数 `(a, omega, t)`，无变速时返回 `None`。
    pub fn variable_params(&self) -> Option<(f32, f32, f32)> {
        self.variable.as_ref().map(|v| (v.a, v.omega, v.t))
    }

    /// 判断是否为顺时针旋转。
    pub fn is_clockwise(&self) -> bool {
        self.clockwise
    }

    /// 绕旋转轴旋转指定角度。
    ///
    /// # 参数
    /// - `transform`：目标变换组件（可变引用）
    /// - `angle`：旋转角度（弧度）
    pub fn rotate(&self, transform: &mut Transform, angle: f32) {
        transform.rotate_local_axis(self.direction, angle);
    }

    /// 启用正弦变速旋转（大机关激活时调用）。
    pub fn set_variable(&mut self, rng: &mut impl Rng) {
        self.variable = Some(VariableRotation::random(rng));
    }

    /// 禁用正弦变速旋转，恢复恒定速度。
    pub fn clear_variable(&mut self) {
        self.variable = None;
    }

    /// 开始激活：清除变速状态，若为大机关则重新生成随机变速参数。
    pub fn begin_activation(&mut self, mode: RuneMode, rng: &mut impl Rng) {
        self.clear_variable();
        if mode == RuneMode::Large {
            self.set_variable(rng);
        }
    }

    /// 结束激活：清除变速状态，恢复恒定速度。
    pub fn end_activation(&mut self) {
        self.clear_variable();
    }

    /// 同步激活状态：根据当前模式与激活标志，决定是否启用/保持/清除变速旋转。
    ///
    /// 大机关激活中且尚未启用变速时，生成新的随机参数；
    /// 大机关激活中且已启用变速时，保持现有参数不变；
    /// 其他情况（小机关或非激活）清除变速。
    pub fn sync_activation(&mut self, mode: RuneMode, activating: bool, rng: &mut impl Rng) {
        match (mode, activating, self.variable.is_some()) {
            (RuneMode::Large, true, false) => self.set_variable(rng),
            (RuneMode::Large, true, true) => {} // 保持现有变速参数
            _ => self.clear_variable(),
        }
    }

    /// 计算当前帧的旋转速度（弧度/秒）。
    ///
    /// 小机关返回恒定基础速度；大机关在激活时返回正弦变速速度，非激活时返回基础速度。
    /// 速度正负号取决于旋转方向。
    ///
    /// # 参数
    /// - `mode`：机关模式
    /// - `dt`：时间增量（秒），用于推进变速旋转的时间累积
    pub fn current_speed(&mut self, mode: RuneMode, dt: f32) -> f32 {
        let sgn = if self.clockwise { 1.0 } else { -1.0 };
        if mode == RuneMode::Small {
            return sgn * self.baseline;
        }
        // 大机关只有在激活状态下使用变量旋转
        if let Some(variable) = &mut self.variable {
            let speed = variable.speed();
            variable.advance(dt);
            return sgn * speed;
        }
        sgn * self.baseline
    }
}

/// 能量机关旋转组件，封装 `RotationController` 作为 Bevy 组件。
#[derive(Component)]
pub struct PowerRuneRotation {
    /// 内部旋转控制器。
    controller: RotationController,
}

impl PowerRuneRotation {
    /// 创建新的旋转组件。
    ///
    /// # 参数
    /// - `clockwise`：旋转方向
    pub fn new(clockwise: bool) -> Self {
        Self {
            controller: RotationController::new(clockwise),
        }
    }

    /// 获取对旋转控制器的不可变引用。
    pub fn controller(&self) -> &RotationController {
        &self.controller
    }

    /// 开始激活，同步到旋转控制器。
    pub fn begin_activation(&mut self, mode: RuneMode, rng: &mut impl Rng) {
        self.controller.begin_activation(mode, rng);
    }

    /// 结束激活，同步到旋转控制器。
    pub fn end_activation(&mut self) {
        self.controller.end_activation();
    }

    /// 同步激活状态，确保旋转控制器的变速状态与机关状态机一致。
    pub fn sync_activation(&mut self, mode: RuneMode, activating: bool, rng: &mut impl Rng) {
        self.controller.sync_activation(mode, activating, rng);
    }

    /// 执行一帧的旋转，更新 `Transform`。
    ///
    /// # 参数
    /// - `mode`：机关模式
    /// - `transform`：目标变换组件
    /// - `dt`：时间增量（秒）
    pub fn rotate(&mut self, mode: RuneMode, transform: &mut Transform, dt: f32) {
        let speed = self.controller.current_speed(mode, dt);
        self.controller.rotate(transform, speed * dt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_rune_rotation_is_baseline_speed() {
        let mut controller = RotationController::new(true);

        assert_eq!(
            controller.current_speed(RuneMode::Small, 0.25),
            ROTATION_BASELINE_SMALL
        );
        assert!(controller.variable_params().is_none());
    }

    #[test]
    fn large_rune_activation_uses_fresh_sine_params() {
        let mut rng = rand::rng();
        let mut controller = RotationController::new(true);

        controller.begin_activation(RuneMode::Large, &mut rng);
        let (a, omega, t) = controller.variable_params().unwrap();
        assert!((0.780..=1.045).contains(&a));
        assert!((1.884..=2.0).contains(&omega));
        assert_eq!(t, 0.0);

        let expected_initial_speed = 2.090 - a;
        assert_eq!(
            controller.current_speed(RuneMode::Large, 0.5),
            expected_initial_speed
        );
        assert_eq!(controller.variable_params().unwrap().2, 0.5);

        controller.current_speed(RuneMode::Large, 0.5);
        assert_eq!(controller.variable_params().unwrap().2, 1.0);

        controller.end_activation();
        assert!(controller.variable_params().is_none());
    }

    #[test]
    fn counter_clockwise_rotation_negates_speed() {
        let mut controller = RotationController::new(false);

        assert_eq!(
            controller.current_speed(RuneMode::Small, 0.25),
            -ROTATION_BASELINE_SMALL
        );
    }

    #[test]
    fn large_rune_sync_preserves_active_variable_rotation() {
        let mut rng = rand::rng();
        let mut controller = RotationController::new(true);

        controller.sync_activation(RuneMode::Large, true, &mut rng);
        let first_params = controller.variable_params().unwrap();

        controller.sync_activation(RuneMode::Large, true, &mut rng);
        assert_eq!(controller.variable_params().unwrap(), first_params);

        controller.sync_activation(RuneMode::Large, false, &mut rng);
        assert!(controller.variable_params().is_none());
    }
}
