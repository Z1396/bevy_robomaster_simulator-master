//! 能量机关的公共类型定义。
//!
//! 定义了机关模式、命中结果、状态转换等核心枚举与常量，
//! 被 `power_rune` 模块内其他子模块共同引用。

/// 能量机关的模式。
#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
pub enum RuneMode {
    /// 小机关：每次激活点亮一个目标，命中后切换至下一个目标。
    Small,
    /// 大机关：每次激活点亮两个目标，需要先命中主目标再命中次级目标。
    Large,
}

/// 每个机关面包含的目标数量。
pub const RUNE_TARGET_COUNT: usize = 5;

/// 命中目标后的结果分类。
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum RuneHitOutcome {
    /// 命中被忽略（机关不在激活状态）。
    Ignored,
    /// 命中了错误的目标（当前不是可命中状态的目标）。
    WrongTarget,
    /// 命中了主目标（小机关或大机关的主目标）。
    PrimaryHit,
    /// 命中了次级目标（大机关次级窗口中的目标）。
    SecondaryHit,
    /// 所有目标均已命中，机关成功激活。
    Activated,
}

impl RuneHitOutcome {
    /// 判断该结果是否为有效命中（准确命中了一个可命中的目标）。
    pub const fn is_accurate(self) -> bool {
        matches!(
            self,
            Self::PrimaryHit | Self::SecondaryHit | Self::Activated
        )
    }

    /// 判断该结果是否触发了机关完全激活。
    pub const fn activates_rune(self) -> bool {
        matches!(self, Self::Activated)
    }
}

/// 机关状态机的转换事件。
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum RuneTransition {
    /// 无状态变化。
    None,
    /// 激活流程开始（从非激活进入激活状态）。
    Started,
    /// 激活进度推进（进入下一个目标轮次）。
    Advanced,
    /// 激活失败（超时或命中错误目标）。
    Failed,
    /// 机关成功激活。
    Activated,
    /// 重置回非激活状态（激活保持时间结束或全局超时）。
    ResetToInactive,
}
