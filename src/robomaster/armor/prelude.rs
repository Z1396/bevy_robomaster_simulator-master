//! 装甲模块预导出
//!
//! 重新导出 common、construct、marker 子模块的所有公共类型，
//! 并定义 ArmorPlugins 插件组，统一注册装甲构造和碰撞检测插件。
//! 上面两行是 **文档注释**，只用来写说明，编译不会执行，给开发者看的。

// 引入碰撞逻辑插件，当前模块内部使用，不对外暴露
use super::collision::ArmorCollisionPlugin;

// 【批量重新导出】
// 把 armor/common 模块里所有 pub 公开的结构体、组件、函数、事件全部导出
// 后续别的文件想用 ArmorComponent、装甲常量等，只需要引入 ArmorPlugins 所在模块即可，不用层层嵌套导入
pub use crate::robomaster::armor::common::*;
pub use crate::robomaster::armor::construct::*;
pub use crate::robomaster::armor::marker::*;

// 导入 Bevy 用来创建插件组的宏
use bevy::app::plugin_group;

// plugin_group! 宏：批量打包多个插件，合成一个整体插件 ArmorPlugins
plugin_group! {
    // 对外公开的装甲总插件
    pub struct ArmorPlugins {
        // 冒号语法：往插件组里塞入两个子插件
        :ArmorConstructorPlugin,  // 装甲生成插件：负责生成装甲实体、挂载装甲组件、绑定材质模型
        :ArmorCollisionPlugin     // 装甲碰撞插件：负责装甲击打检测、扣血、命中判定逻辑
    }
}