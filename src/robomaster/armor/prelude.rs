//! 装甲模块预导出
//!
//! 重新导出 common、construct、marker 子模块的所有公共类型，
//! 并定义 ArmorPlugins 插件组，统一注册装甲构造和碰撞检测插件。

use super::collision::ArmorCollisionPlugin;
pub use crate::robomaster::armor::common::*;
pub use crate::robomaster::armor::construct::*;
pub use crate::robomaster::armor::marker::*;
use bevy::app::plugin_group;

plugin_group! {
    pub struct ArmorPlugins {
        :ArmorConstructorPlugin,
        :ArmorCollisionPlugin
    }
}
