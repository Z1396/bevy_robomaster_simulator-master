//! 能量机关的状态机实现。
//!
//! 定义了 `MechanismState` 状态机枚举，包含非激活、激活中、已激活、失败四种状态。
//! 激活流程（`ActivationRun`）管理目标队列的命中判定与超时逻辑，
//! 区分小机关（单目标轮次）与大机关（主-次级双目标轮次）两种模式。

use crate::robomaster::power_rune::common::{
    RUNE_TARGET_COUNT, RuneHitOutcome, RuneMode, RuneTransition,
};
use crate::robomaster::power_rune::consts::{
    ACTIVATED_HOLD, ACTIVATION_GLOBAL_TIMEOUT, ACTIVATION_PRIMARY_TIMEOUT, FAILURE_RECOVER,
    FUNNY_IGNORE_WRONG_TARGET_FAILURE, INACTIVE_WAIT, LARGE_SECONDARY_TIMEOUT,
};
use crate::robomaster::visibility::Activation;
use rand::Rng;
use rand::prelude::SliceRandom;

/// 五个目标面的激活状态数组。
pub type RuneTargetStates = [Activation; RUNE_TARGET_COUNT];

/// 能量机关状态机，表示机关的整体状态。
#[derive(Debug, Clone, PartialEq)]
pub enum MechanismState {
    /// 非激活状态，等待 `remaining` 秒后自动进入激活流程。
    Inactive { mode: RuneMode, remaining: f32 },
    /// 激活流程中，包含目标命中与超时管理。
    Activating(ActivationRun),
    /// 已成功激活，保持 `remaining` 秒后自动回到非激活状态。
    Activated { mode: RuneMode, remaining: f32 },
    /// 激活失败，等待 `remaining` 秒后自动回到非激活状态。
    Failed { mode: RuneMode, remaining: f32 },
}

/// 一次激活流程的运行上下文。
///
/// 管理全局超时、五个目标面的激活状态以及当前轮次（小机关轮次/大机关运行）。
#[derive(Debug, Clone, PartialEq)]
pub struct ActivationRun {
    /// 全局激活超时剩余时间，超时后重置为非激活状态。
    global_remaining: f32,
    /// 五个目标面的当前激活状态数组。
    targets: RuneTargetStates,
    /// 当前轮次类型（小机关单目标轮次/大机关多阶段运行）。
    round: ActivationRound,
}

/// 激活轮次类型，区分小机关与大机关的不同行为。
#[derive(Debug, Clone, PartialEq)]
enum ActivationRound {
    /// 小机关的单目标轮次。
    Small(SmallRound),
    /// 大机关的多阶段运行。
    Large(LargeRun),
}

/// 小机关的单目标轮次。
///
/// 每次随机选择一个未激活的目标，在 `primary_remaining` 超时前命中即可推进。
#[derive(Debug, Clone, PartialEq)]
struct SmallRound {
    /// 主目标命中超时剩余时间。
    primary_remaining: f32,
}

/// 大机关的完整运行流程。
///
/// 需要经历多轮主-次级命中，每轮完成 `completed_groups` 计数加一，
/// 直到所有 5 个目标均被激活。
#[derive(Debug, Clone, PartialEq)]
struct LargeRun {
    /// 已完成的主-次级目标组数。
    completed_groups: usize,
    /// 当前阶段（主目标阶段/次级目标阶段）。
    phase: LargePhase,
}

/// 大机关的阶段枚举。
#[derive(Debug, Clone, PartialEq)]
enum LargePhase {
    /// 主目标阶段：等待命中当前轮次的主目标。
    Primary {
        /// 主目标命中超时剩余时间。
        primary_remaining: f32,
    },
    /// 次级目标阶段：主目标命中后进入次级窗口。
    Secondary {
        /// 次级目标命中窗口剩余时间。
        secondary_remaining: f32,
        /// 次级目标的索引，命中该目标可完成本轮。
        target: Option<usize>,
    },
}

/// 内部状态转换事件，用于 `ActivationRun::tick` 的返回值。
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum RunTransition {
    /// 无变化。
    None,
    /// 激活进度推进。
    Advanced,
    /// 激活失败。
    Failed,
    /// 机关完全激活。
    Activated,
    /// 重置为非激活状态。
    ResetToInactive,
}

impl MechanismState {
    /// 创建非激活状态，使用默认的非激活等待时长。
    ///
    /// # 参数
    /// - `mode`：机关模式
    pub fn inactive(mode: RuneMode) -> Self {
        Self::Inactive {
            mode,
            remaining: INACTIVE_WAIT,
        }
    }

    /// 创建并立即进入激活流程。
    ///
    /// # 参数
    /// - `mode`：机关模式
    /// - `rng`：随机数生成器，用于随机选择目标
    pub fn start(mode: RuneMode, rng: &mut impl Rng) -> Self {
        Self::Activating(ActivationRun::new(mode, rng))
    }

    /// 获取当前状态对应的机关模式。
    pub fn mode(&self) -> RuneMode {
        match self {
            Self::Inactive { mode, .. }
            | Self::Activated { mode, .. }
            | Self::Failed { mode, .. } => *mode,
            Self::Activating(run) => run.mode(),
        }
    }

    /// 按时间增量推进状态机。
    ///
    /// 处理各状态的超时逻辑，自动完成状态间的转换。
    ///
    /// # 参数
    /// - `delta_secs`：时间增量（秒）
    /// - `rng`：随机数生成器
    ///
    /// # 返回值
    /// 返回本次 tick 产生的状态转换事件。
    pub fn tick(&mut self, delta_secs: f32, rng: &mut impl Rng) -> RuneTransition {
        // 防止负的时间增量
        let delta_secs = delta_secs.max(0.0);
        let mut next = None;

        // 根据当前状态分支处理超时逻辑
        let transition = match self {
            Self::Inactive { mode, remaining } => {
                // 非激活状态计时结束，进入激活流程
                if expire_after(remaining, delta_secs) {
                    next = Some(Self::start(*mode, rng));
                    RuneTransition::Started
                } else {
                    RuneTransition::None
                }
            }
            Self::Activating(run) => match run.tick(delta_secs, rng) {
                RunTransition::None => RuneTransition::None,
                RunTransition::Advanced => RuneTransition::Advanced,
                RunTransition::Failed => {
                    next = Some(Self::failed(run.mode()));
                    RuneTransition::Failed
                }
                RunTransition::Activated => {
                    next = Some(Self::activated(run.mode()));
                    RuneTransition::Activated
                }
                RunTransition::ResetToInactive => {
                    next = Some(Self::inactive(run.mode()));
                    RuneTransition::ResetToInactive
                }
            },
            Self::Activated { mode, remaining } => {
                // 激活保持时间结束，回到非激活状态
                if expire_after(remaining, delta_secs) {
                    next = Some(Self::inactive(*mode));
                    RuneTransition::ResetToInactive
                } else {
                    RuneTransition::None
                }
            }
            Self::Failed { mode, remaining } => {
                // 失败恢复时间结束，回到非激活状态
                if expire_after(remaining, delta_secs) {
                    next = Some(Self::inactive(*mode));
                    RuneTransition::ResetToInactive
                } else {
                    RuneTransition::None
                }
            }
        };

        // 执行状态转换（如有）
        if let Some(state) = next {
            *self = state;
        }

        transition
    }

    /// 处理命中事件，返回命中结果。
    ///
    /// 仅在 `Activating` 状态下处理命中；否则返回 `Ignored`。
    /// 如果 `FUNNY_IGNORE_WRONG_TARGET_FAILURE` 为 `false`，错误目标命中会导致状态切换到 `Failed`。
    ///
    /// # 参数
    /// - `target_index`：被命中的目标索引
    /// - `rng`：随机数生成器
    pub fn hit(&mut self, target_index: usize, rng: &mut impl Rng) -> RuneHitOutcome {
        let Self::Activating(run) = self else {
            return RuneHitOutcome::Ignored;
        };
        let mode = run.mode();

        match run.hit(target_index, rng) {
            RuneHitOutcome::WrongTarget => {
                // 趣味模式开关：为 false 时错误目标命中直接导致失败
                if !FUNNY_IGNORE_WRONG_TARGET_FAILURE {
                    *self = Self::failed(mode);
                }
                RuneHitOutcome::WrongTarget
            }
            RuneHitOutcome::Activated => {
                *self = Self::activated(mode);
                RuneHitOutcome::Activated
            }
            outcome => outcome,
        }
    }

    /// 判断当前是否处于激活流程中。
    pub fn is_activating(&self) -> bool {
        matches!(self, Self::Activating(_))
    }

    /// 判断当前是否处于大机关的激活流程中。
    pub fn is_activating_large(&self) -> bool {
        matches!(
            self,
            Self::Activating(ActivationRun {
                round: ActivationRound::Large(_),
                ..
            })
        )
    }

    /// 获取大机关的已完成目标组数。非大机关激活状态返回 `None`。
    pub fn large_progress(&self) -> Option<usize> {
        match self {
            Self::Activating(run) => run.large_progress(),
            Self::Inactive { .. } | Self::Activated { .. } | Self::Failed { .. } => None,
        }
    }

    /// 获取五个目标面的当前激活状态数组。
    ///
    /// 非激活/失败状态全为 `Deactivated`，已激活状态全为 `Completed`。
    pub fn target_states(&self) -> RuneTargetStates {
        match self {
            Self::Inactive { .. } | Self::Failed { .. } => {
                [Activation::Deactivated; RUNE_TARGET_COUNT]
            }
            Self::Activating(run) => run.targets,
            Self::Activated { .. } => [Activation::Completed; RUNE_TARGET_COUNT],
        }
    }

    /// 获取机关根节点的激活状态（用于整体外观控制）。
    pub fn root_activation(&self) -> Activation {
        match self {
            Self::Inactive { .. } | Self::Failed { .. } => Activation::Deactivated,
            Self::Activating(_) | Self::Activated { .. } => Activation::Activated,
        }
    }

    /// 创建已激活状态，使用默认的激活保持时长。
    fn activated(mode: RuneMode) -> Self {
        Self::Activated {
            mode,
            remaining: ACTIVATED_HOLD,
        }
    }

    /// 创建失败状态，使用默认的失败恢复时长。
    fn failed(mode: RuneMode) -> Self {
        Self::Failed {
            mode,
            remaining: FAILURE_RECOVER,
        }
    }
}

impl ActivationRun {
    /// 创建新的激活运行上下文，初始化全局超时与目标状态。
    ///
    /// # 参数
    /// - `mode`：机关模式，决定初始轮次类型
    /// - `rng`：随机数生成器，用于随机选择首发目标
    fn new(mode: RuneMode, rng: &mut impl Rng) -> Self {
        let mut run = Self {
            global_remaining: ACTIVATION_GLOBAL_TIMEOUT,
            targets: [Activation::Deactivated; RUNE_TARGET_COUNT],
            round: match mode {
                RuneMode::Small => ActivationRound::Small(SmallRound {
                    primary_remaining: ACTIVATION_PRIMARY_TIMEOUT,
                }),
                RuneMode::Large => ActivationRound::Large(LargeRun {
                    completed_groups: 0,
                    phase: LargePhase::Primary {
                        primary_remaining: ACTIVATION_PRIMARY_TIMEOUT,
                    },
                }),
            },
        };
        // 随机选择第一轮的目标
        run.start_round(mode, rng);
        run
    }

    /// 获取当前激活运行对应的机关模式。
    pub fn mode(&self) -> RuneMode {
        match &self.round {
            ActivationRound::Small(_) => RuneMode::Small,
            ActivationRound::Large(_) => RuneMode::Large,
        }
    }

    /// 获取各目标面的当前激活状态数组。
    pub fn target_states(&self) -> RuneTargetStates {
        self.targets
    }

    /// 获取大机关的已完成目标组数（小机关返回 `None`）。
    pub fn large_progress(&self) -> Option<usize> {
        match &self.round {
            ActivationRound::Large(run) => Some(run.completed_groups),
            ActivationRound::Small(_) => None,
        }
    }

    /// 按时间增量推进激活流程。
    ///
    /// 处理全局超时与当前轮次超时，超时发生时触发相应状态转换。
    fn tick(&mut self, delta_secs: f32, rng: &mut impl Rng) -> RunTransition {
        match &mut self.round {
            ActivationRound::Small(round) => {
                // 小机关：全局超时或主目标超时
                tick_two_timers(
                    &mut self.global_remaining,
                    &mut round.primary_remaining,
                    delta_secs,
                    RunTransition::ResetToInactive,
                    RunTransition::Failed,
                )
            }
            ActivationRound::Large(run) => match &mut run.phase {
                LargePhase::Primary { primary_remaining } => {
                    // 大机关主目标阶段：全局超时或主目标超时
                    tick_two_timers(
                        &mut self.global_remaining,
                        primary_remaining,
                        delta_secs,
                        RunTransition::ResetToInactive,
                        RunTransition::Failed,
                    )
                }
                LargePhase::Secondary {
                    secondary_remaining,
                    ..
                } => {
                    // 大机关次级阶段：全局超时或次级窗口超时
                    match tick_two_timers(
                        &mut self.global_remaining,
                        secondary_remaining,
                        delta_secs,
                        RunTransition::ResetToInactive,
                        RunTransition::Advanced,
                    ) {
                        // 次级窗口超时后自动进入下一轮主目标阶段
                        RunTransition::Advanced => self.start_large_primary_round(rng),
                        transition => transition,
                    }
                }
            },
        }
    }

    /// 处理命中事件，根据当前轮次类型分发到对应的命中处理函数。
    fn hit(&mut self, target_index: usize, rng: &mut impl Rng) -> RuneHitOutcome {
        if target_index >= RUNE_TARGET_COUNT {
            return RuneHitOutcome::WrongTarget;
        }

        match &self.round {
            ActivationRound::Small(_) => self.hit_small(target_index, rng),
            ActivationRound::Large(run) => match &run.phase {
                LargePhase::Primary { .. } => self.hit_large_primary(target_index, rng),
                LargePhase::Secondary { target, .. } => {
                    self.hit_large_secondary(target_index, *target)
                }
            },
        }
    }

    /// 处理小机关的主目标命中。
    ///
    /// 如果命中正确目标则标记为已激活，并随机选择下一个目标开始新一轮。
    /// 如果所有目标均已激活，则返回 `Activated`。
    fn hit_small(&mut self, target_index: usize, rng: &mut impl Rng) -> RuneHitOutcome {
        if self.targets[target_index] != Activation::Activating {
            return RuneHitOutcome::WrongTarget;
        }

        self.targets[target_index] = Activation::Activated;
        if self.all_targets_activated() {
            // 所有目标均已激活，机关完全激活
            RuneHitOutcome::Activated
        } else {
            // 选择下一个目标，开始新一轮
            self.start_small_round(rng);
            RuneHitOutcome::PrimaryHit
        }
    }

    /// 处理大机关的主目标命中。
    ///
    /// 命中后将当前轮次的主目标标记为已激活，进入次级阶段。
    /// 如果所有 5 个目标均已激活，则返回 `Activated`。
    fn hit_large_primary(&mut self, target_index: usize, _rng: &mut impl Rng) -> RuneHitOutcome {
        if self.targets[target_index] != Activation::Activating {
            return RuneHitOutcome::WrongTarget;
        }

        // 查找另一个处于激活中的目标作为次级目标
        let secondary_target = self.targets.iter().enumerate().find_map(|(idx, state)| {
            (idx != target_index && *state == Activation::Activating).then_some(idx)
        });

        self.targets[target_index] = Activation::Activated;
        let ActivationRound::Large(run) = &mut self.round else {
            unreachable!("large primary hit requires a large run");
        };
        run.completed_groups += 1;
        if run.completed_groups == RUNE_TARGET_COUNT {
            return RuneHitOutcome::Activated;
        }

        // 切换到次级阶段，打开次级命中窗口
        run.phase = LargePhase::Secondary {
            secondary_remaining: LARGE_SECONDARY_TIMEOUT,
            target: secondary_target,
        };
        RuneHitOutcome::PrimaryHit
    }

    /// 处理大机关的次级目标命中。
    ///
    /// 必须命中预先指定的次级目标，且该目标必须处于 `Activating` 状态。
    fn hit_large_secondary(
        &mut self,
        target_index: usize,
        secondary_target: Option<usize>,
    ) -> RuneHitOutcome {
        if secondary_target != Some(target_index)
            || self.targets[target_index] != Activation::Activating
        {
            return RuneHitOutcome::WrongTarget;
        }

        self.targets[target_index] = Activation::Activated;
        RuneHitOutcome::SecondaryHit
    }

    /// 启动一轮新的激活轮次，根据模式选择小机关或大机关的首轮目标。
    fn start_round(&mut self, mode: RuneMode, rng: &mut impl Rng) -> RunTransition {
        match mode {
            RuneMode::Small => self.start_small_round(rng),
            RuneMode::Large => self.start_large_primary_round(rng),
        }
    }

    /// 启动小机关的新一轮：清除非永久激活的目标，随机选择一个目标进入激活状态。
    fn start_small_round(&mut self, rng: &mut impl Rng) -> RunTransition {
        // 清除本轮临时目标状态（保留已永久激活的目标）
        self.clear_transient_targets();
        // 从尚未激活的目标中随机选择一个
        let Some(target) = self.choose_targets(1, rng).into_iter().next() else {
            return RunTransition::Activated;
        };
        self.targets[target] = Activation::Activating;
        self.round = ActivationRound::Small(SmallRound {
            primary_remaining: ACTIVATION_PRIMARY_TIMEOUT,
        });
        RunTransition::Advanced
    }

    /// 启动大机关的新一轮主目标阶段：清除所有目标，随机选择两个目标进入激活状态。
    fn start_large_primary_round(&mut self, rng: &mut impl Rng) -> RunTransition {
        // 保留已完成组数信息
        let completed_groups = match &self.round {
            ActivationRound::Large(run) => run.completed_groups,
            ActivationRound::Small(_) => 0,
        };
        // 大机关每轮重新选择所有目标
        self.clear_all_targets();
        let targets = self.choose_targets_from_all(2, rng);
        if targets.is_empty() {
            return RunTransition::Activated;
        }
        for target in targets {
            self.targets[target] = Activation::Activating;
        }
        self.round = ActivationRound::Large(LargeRun {
            completed_groups,
            phase: LargePhase::Primary {
                primary_remaining: ACTIVATION_PRIMARY_TIMEOUT,
            },
        });
        RunTransition::Advanced
    }

    /// 从尚未永久激活的目标中随机选择指定数量的目标。
    fn choose_targets(&self, count: usize, rng: &mut impl Rng) -> Vec<usize> {
        let mut available = self
            .targets
            .iter()
            .enumerate()
            .filter_map(|(idx, state)| (*state != Activation::Activated).then_some(idx))
            .collect::<Vec<_>>();
        available.shuffle(rng);
        available.truncate(count.min(available.len()));
        available
    }

    /// 从所有目标（无论当前状态）中随机选择指定数量的目标。
    /// 用于大机关每轮重新随机选择两个目标。
    fn choose_targets_from_all(&self, count: usize, rng: &mut impl Rng) -> Vec<usize> {
        let mut targets = (0..RUNE_TARGET_COUNT).collect::<Vec<_>>();
        targets.shuffle(rng);
        targets.truncate(count.min(RUNE_TARGET_COUNT));
        targets
    }

    /// 清除所有非永久激活的目标状态（将 `Activating` 重置为 `Deactivated`）。
    fn clear_transient_targets(&mut self) {
        for state in &mut self.targets {
            if *state != Activation::Activated {
                *state = Activation::Deactivated;
            }
        }
    }

    /// 将所有目标重置为 `Deactivated` 状态。
    fn clear_all_targets(&mut self) {
        self.targets = [Activation::Deactivated; RUNE_TARGET_COUNT];
    }

    /// 检查是否所有目标均已激活。
    fn all_targets_activated(&self) -> bool {
        self.targets
            .iter()
            .all(|state| *state == Activation::Activated)
    }
}

/// 递减计时器，如果 `delta_secs` 大于等于剩余时间则返回 `true`（计时到期）。
///
/// # 参数
/// - `remaining`：可变引用，剩余时间（秒）
/// - `delta_secs`：时间增量（秒）
///
/// # 返回值
/// 如果计时到期返回 `true`，否则返回 `false` 并将剩余时间减去增量。
fn expire_after(remaining: &mut f32, delta_secs: f32) -> bool {
    if delta_secs >= *remaining {
        *remaining = 0.0;
        true
    } else {
        *remaining -= delta_secs;
        false
    }
}

/// 同时递减两个计时器，返回最先到期者对应的转换事件。
///
/// 如果 `delta_secs` 小于两个计时器中较小的值，则仅递减，返回 `None`。
/// 否则，返回先到期计时器对应的转换事件，另一计时器减去已消耗的时间。
///
/// # 参数
/// - `global_remaining`：全局计时器（可变引用）
/// - `local_remaining`：局部计时器（可变引用）
/// - `delta_secs`：时间增量（秒）
/// - `global_transition`：全局计时器到期时返回的转换事件
/// - `local_transition`：局部计时器到期时返回的转换事件
fn tick_two_timers(
    global_remaining: &mut f32,
    local_remaining: &mut f32,
    delta_secs: f32,
    global_transition: RunTransition,
    local_transition: RunTransition,
) -> RunTransition {
    // 取两个计时器中较早到期的时刻
    let next_event = (*global_remaining).min(*local_remaining);
    if delta_secs < next_event {
        // 两个计时器均未到期，同时递减
        *global_remaining -= delta_secs;
        *local_remaining -= delta_secs;
        return RunTransition::None;
    }

    // 至少一个计时器到期，返回先到期者对应的转换
    if *global_remaining <= *local_remaining {
        *global_remaining = 0.0;
        global_transition
    } else {
        *global_remaining -= *local_remaining;
        *local_remaining = 0.0;
        local_transition
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active_indices(state: &MechanismState) -> Vec<usize> {
        state
            .target_states()
            .iter()
            .enumerate()
            .filter_map(|(idx, activation)| (*activation == Activation::Activating).then_some(idx))
            .collect()
    }

    fn activated_count(state: &MechanismState) -> usize {
        state
            .target_states()
            .iter()
            .filter(|activation| **activation == Activation::Activated)
            .count()
    }

    #[test]
    fn small_rune_lights_one_target_and_advances_on_hit() {
        let mut rng = rand::rng();
        let mut state = MechanismState::start(RuneMode::Small, &mut rng);
        let active = active_indices(&state);

        assert_eq!(active.len(), 1);
        assert_eq!(state.hit(active[0], &mut rng), RuneHitOutcome::PrimaryHit);
        assert_eq!(activated_count(&state), 1);
        assert_eq!(active_indices(&state).len(), 1);
    }

    #[test]
    fn funny_mode_keeps_small_rune_activating_after_wrong_target() {
        let mut rng = rand::rng();
        let mut state = MechanismState::start(RuneMode::Small, &mut rng);
        let active = active_indices(&state)[0];
        let wrong = (0..RUNE_TARGET_COUNT).find(|idx| *idx != active).unwrap();

        assert_eq!(state.hit(wrong, &mut rng), RuneHitOutcome::WrongTarget);
        assert!(matches!(state, MechanismState::Activating(_)));
        assert_eq!(active_indices(&state), vec![active]);
    }

    #[test]
    fn small_rune_primary_timeout_fails() {
        let mut rng = rand::rng();
        let mut state = MechanismState::start(RuneMode::Small, &mut rng);

        assert_eq!(
            state.tick(ACTIVATION_PRIMARY_TIMEOUT, &mut rng),
            RuneTransition::Failed
        );
        assert!(matches!(state, MechanismState::Failed { .. }));
    }

    #[test]
    fn large_rune_lights_two_targets_and_enters_secondary_window() {
        let mut rng = rand::rng();
        let mut state = MechanismState::start(RuneMode::Large, &mut rng);
        let active = active_indices(&state);

        assert_eq!(active.len(), 2);
        assert_eq!(state.hit(active[0], &mut rng), RuneHitOutcome::PrimaryHit);
        assert!(state.is_activating_large());
        assert_eq!(state.large_progress(), Some(1));
        assert_eq!(activated_count(&state), 1);
        assert_eq!(active_indices(&state).len(), 1);
    }

    #[test]
    fn large_rune_secondary_hit_waits_for_window_timeout() {
        let mut rng = rand::rng();
        let mut state = MechanismState::start(RuneMode::Large, &mut rng);
        let active = active_indices(&state);

        assert_eq!(state.hit(active[0], &mut rng), RuneHitOutcome::PrimaryHit);
        assert_eq!(state.hit(active[1], &mut rng), RuneHitOutcome::SecondaryHit);
        assert_eq!(state.large_progress(), Some(1));
        assert_eq!(active_indices(&state).len(), 0);

        assert_eq!(
            state.tick(LARGE_SECONDARY_TIMEOUT * 0.5, &mut rng),
            RuneTransition::None
        );
        assert_eq!(state.large_progress(), Some(1));
        assert_eq!(active_indices(&state).len(), 0);

        assert_eq!(
            state.tick(LARGE_SECONDARY_TIMEOUT * 0.5, &mut rng),
            RuneTransition::Advanced
        );
        assert_eq!(state.large_progress(), Some(1));
        assert_eq!(active_indices(&state).len(), 2);
    }

    #[test]
    fn large_rune_secondary_timeout_advances_without_failure() {
        let mut rng = rand::rng();
        let mut state = MechanismState::start(RuneMode::Large, &mut rng);
        let first = active_indices(&state)[0];
        state.hit(first, &mut rng);

        assert_eq!(
            state.tick(LARGE_SECONDARY_TIMEOUT, &mut rng),
            RuneTransition::Advanced
        );
        assert!(matches!(state, MechanismState::Activating(_)));
        assert!(state.is_activating_large());
        assert_eq!(state.large_progress(), Some(1));
        assert_eq!(active_indices(&state).len(), 2);
    }

    #[test]
    fn large_rune_activates_after_five_primary_hits() {
        let mut rng = rand::rng();
        let mut state = MechanismState::start(RuneMode::Large, &mut rng);

        for expected_progress in 1..RUNE_TARGET_COUNT {
            let active = active_indices(&state);
            assert_eq!(active.len(), 2);
            assert_eq!(state.hit(active[0], &mut rng), RuneHitOutcome::PrimaryHit);
            assert_eq!(state.large_progress(), Some(expected_progress));
            assert_eq!(
                state.tick(LARGE_SECONDARY_TIMEOUT, &mut rng),
                RuneTransition::Advanced
            );
        }

        let active = active_indices(&state);
        assert_eq!(active.len(), 2);
        assert_eq!(state.hit(active[0], &mut rng), RuneHitOutcome::Activated);
        assert!(matches!(state, MechanismState::Activated { .. }));
    }

    #[test]
    fn large_rune_primary_timeout_fails() {
        let mut rng = rand::rng();
        let mut state = MechanismState::start(RuneMode::Large, &mut rng);

        assert_eq!(
            state.tick(ACTIVATION_PRIMARY_TIMEOUT, &mut rng),
            RuneTransition::Failed
        );
        assert!(matches!(state, MechanismState::Failed { .. }));
    }

    #[test]
    fn large_rune_global_timeout_resets_to_inactive() {
        let mut state = MechanismState::Activating(ActivationRun {
            global_remaining: 1.0,
            targets: [
                Activation::Activating,
                Activation::Activating,
                Activation::Deactivated,
                Activation::Deactivated,
                Activation::Deactivated,
            ],
            round: ActivationRound::Large(LargeRun {
                completed_groups: 1,
                phase: LargePhase::Primary {
                    primary_remaining: ACTIVATION_PRIMARY_TIMEOUT,
                },
            }),
        });
        let mut rng = rand::rng();

        assert_eq!(state.tick(1.0, &mut rng), RuneTransition::ResetToInactive);
        assert!(matches!(
            state,
            MechanismState::Inactive {
                mode: RuneMode::Large,
                ..
            }
        ));
    }
}
