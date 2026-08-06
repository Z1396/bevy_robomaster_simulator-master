//! 能量机关模块的预导入与插件聚合。
//!
//! 将 `power_rune` 子模块中的公共类型和函数统一重导出，方便外部使用。
//! 同时通过 `plugin_group!` 宏将构造、碰撞、更新三个子插件合并为一个总插件。

use crate::robomaster::power_rune::collision::PowerRuneCollisionPlugin;
use crate::robomaster::power_rune::construct::PowerRuneConstructorPlugin;
use crate::robomaster::power_rune::rune::PowerRuneUpdatePlugin;
use bevy::app::plugin_group;

pub use crate::robomaster::power_rune::collision::*;
pub use crate::robomaster::power_rune::common::*;
pub use crate::robomaster::power_rune::construct::*;
pub use crate::robomaster::power_rune::rotation::*;
pub use crate::robomaster::power_rune::rune::*;
pub use crate::robomaster::power_rune::state::*;
pub use crate::robomaster::visibility::Activation;

// plugin_group! 宏内不支持 doc 注释，故在此说明：
// PowerRunePlugins：聚合能量机关所有子插件的总插件。
// 包含 PowerRuneConstructorPlugin（场景构造）、
//      PowerRuneCollisionPlugin（碰撞检测与命中处理）、
//      PowerRuneUpdatePlugin（状态更新与视觉刷新）。
plugin_group! {
    #[derive(Default)]
    pub struct PowerRunePlugins {
        :PowerRuneConstructorPlugin,
        :PowerRuneCollisionPlugin,
        :PowerRuneUpdatePlugin,
    }
}