//! 能量机关的常量配置。
//!
//! 定义了机关激活超时、旋转速度基准、失败恢复时间等核心参数。
//! 所有常量均为 `pub(super)`，仅在 `power_rune` 模块内部可见。

/// 主目标命中超时（秒）。从出现可命中目标开始，若在此时间内未命中则激活失败。
pub(super) const ACTIVATION_PRIMARY_TIMEOUT: f32 = 2.5;
/// 大机关次级目标命中窗口（秒）。主目标命中后进入次级窗口，超时后自动进入下一轮。
pub(super) const LARGE_SECONDARY_TIMEOUT: f32 = 1.0;
/// 非激活状态等待时间（秒）。机关处于非激活状态后，等待此时间后自动开始新一轮激活。
pub(super) const INACTIVE_WAIT: f32 = 1.0;
/// 失败恢复时间（秒）。激活失败后等待此时间后回到非激活状态。
pub(super) const FAILURE_RECOVER: f32 = 1.5;
/// 激活保持时间（秒）。机关成功激活后，保持激活状态此时间后自动回到非激活状态。
pub(super) const ACTIVATED_HOLD: f32 = 6.0;
/// 全局激活超时（秒）。激活流程开始后，若在此时间内未完成所有目标则重置为非激活状态。
pub(super) const ACTIVATION_GLOBAL_TIMEOUT: f32 = 20.0;
/// 小机关基础角速度（弧度/秒）。小机关始终以恒定角速度旋转。
pub(super) const ROTATION_BASELINE_SMALL: f32 = std::f32::consts::PI / 3.0;
/// 趣味模式开关：是否忽略错误目标命中导致的失败。
/// 当为 `true` 时，命中错误目标不会导致激活失败，仅返回 `WrongTarget`。
pub(super) const FUNNY_IGNORE_WRONG_TARGET_FAILURE: bool = true;