//! 能量机关的核心组件与更新系统。
//!
//! 定义了 `PowerRune`（机关标识）和 `PowerRuneMechanism`（机关状态机）两个组件，
//! 以及驱动状态推进、视觉刷新、旋转动画的三个 `Update` 阶段系统。
//! 通过 `PowerRuneUpdatePlugin` 统一注册。

use crate::robomaster::power_rune::common::RuneMode;
use crate::robomaster::power_rune::rotation::PowerRuneRotation;
use crate::robomaster::power_rune::state::MechanismState;
use crate::robomaster::power_rune::visual::PowerRuneVisuals;
use crate::robomaster::prelude::Team;
use crate::robomaster::visibility::StatefulAppearance;
use bevy::app::Update;
use bevy::prelude::{Component, IntoScheduleConfigs, Query, Res, Time, Transform};

/// 能量机关标识组件，记录所属队伍与机关模式。
#[derive(Component, Debug, Copy, Clone, Hash, PartialEq, Eq)]
pub struct PowerRune {
    /// 所属队伍（红方/蓝方）。
    team: Team,
    /// 机关模式（小机关/大机关）。
    mode: RuneMode,
}

/// 能量机关状态机组件，封装机关激活流程的状态切换逻辑。
#[derive(Component, Debug, Clone, PartialEq)]
pub struct PowerRuneMechanism {
    /// 内部状态机实例。
    state: MechanismState,
}

impl PowerRune {
    /// 创建新的能量机关标识。
    ///
    /// # 参数
    /// - `team`：所属队伍
    /// - `mode`：机关模式
    pub fn new(team: Team, mode: RuneMode) -> Self {
        Self { team, mode }
    }

    /// 获取机关所属队伍。
    pub fn team(&self) -> Team {
        self.team
    }

    /// 获取机关模式。
    pub fn mode(&self) -> RuneMode {
        self.mode
    }
}

impl PowerRuneMechanism {
    /// 创建新的机关状态机，初始为非激活状态。
    ///
    /// # 参数
    /// - `mode`：机关模式，决定激活流程的行为差异
    pub fn new(mode: RuneMode) -> Self {
        Self {
            state: MechanismState::inactive(mode),
        }
    }

    /// 获取对状态机的不可变引用。
    pub fn state(&self) -> &MechanismState {
        &self.state
    }

    /// 获取对状态机的可变引用。
    pub fn state_mut(&mut self) -> &mut MechanismState {
        &mut self.state
    }
}

impl Default for PowerRuneMechanism {
    fn default() -> Self {
        Self::new(RuneMode::Small)
    }
}

/// 驱动机关状态机按时间推进的 tick 系统。
///
/// 每帧调用 `MechanismState::tick` 推进状态，同时同步旋转控制器的激活标志。
fn rune_activation_tick(
    time: Res<Time>,
    mut runes: Query<(&mut PowerRuneMechanism, &mut PowerRuneRotation)>,
) {
    let delta_secs = time.delta_secs();
    let mut rng = rand::rng();

    for (mut mechanism, mut rotation) in &mut runes {
        mechanism.state.tick(delta_secs, &mut rng);
        rotation.sync_activation(
            mechanism.state.mode(),
            mechanism.state.is_activating(),
            &mut rng,
        );
    }
}

/// 根据机关当前状态刷新视觉表现的系统。
///
/// 将 `MechanismState` 中的根激活态与各目标激活态映射到 `PowerRuneVisuals` 的外观切换。
fn apply_power_rune_visuals(
    mut runes: Query<(&PowerRune, &PowerRuneMechanism, &mut PowerRuneVisuals)>,
    mut appearance: StatefulAppearance,
) {
    for (rune, mechanism, mut visuals) in &mut runes {
        visuals.apply(rune.mode, mechanism.state(), &mut appearance);
    }
}

/// 驱动机关旋转的系统。
///
/// 根据机关模式与旋转控制器的当前速度，每帧更新 Transform 的旋转角度。
fn rune_rotation_system(
    time: Res<Time>,
    mut runes: Query<(&PowerRune, &mut PowerRuneRotation, &mut Transform)>,
) {
    let dt = time.delta_secs();
    for (rune, mut rotation, mut transform) in &mut runes {
        rotation.rotate(rune.mode, &mut transform, dt);
    }
}

/// 能量机关更新插件，注册了状态 tick、视觉刷新、旋转动画三个系统。
#[derive(Default)]
pub(super) struct PowerRuneUpdatePlugin;

impl bevy::app::Plugin for PowerRuneUpdatePlugin {
    fn build(&self, app: &mut bevy::app::App) {
        // 三个系统按顺序链式执行：先推进状态，再刷新视觉，最后更新旋转
        app.add_systems(
            Update,
            (
                rune_activation_tick,
                apply_power_rune_visuals,
                rune_rotation_system,
            )
                .chain(),
        );
    }
}
