//! 能量机关模块的预导入与插件聚合。
//!
//! 将 `power_rune` 子模块中的公共类型和函数统一重导出，方便外部使用。
//! 同时通过 `plugin_group!` 宏将构造、碰撞、更新三个子插件合并为一个总插件。
//! 作用：外部只需引入单个 `PowerRunePlugins`，就能完整加载能量机关全部逻辑，无需逐个注册子插件。

// 引入能量机关三大独立子插件
// 1. 负责能量机关模型解析、实体生成、组件挂载、初始化装配
use crate::robomaster::power_rune::construct::PowerRuneConstructorPlugin;
// 2. 负责能量机关碰撞判定、子弹命中检测、击打计数逻辑
use crate::robomaster::power_rune::collision::PowerRuneCollisionPlugin;
// 3. 负责能量机关旋转、状态流转、灯光颜色刷新、通关判定等运行时更新逻辑
use crate::robomaster::power_rune::rune::PowerRuneUpdatePlugin;

// Bevy 内置宏，用于聚合多个插件为插件组 PluginGroup
use bevy::app::plugin_group;

// pub use 重导出：把子模块内所有 pub 结构体、组件、函数、枚举全部向外暴露
// 外部其它模块 use crate::robomaster::power_rune::*; 即可直接使用全部类型，不用逐层深入子模块路径
pub use crate::robomaster::power_rune::collision::*;    // 碰撞相关组件/工具
pub use crate::robomaster::power_rune::common::*;       // 通用常量、基础类型
pub use crate::robomaster::power_rune::construct::*;   // 构造扫描组件、初始化系统
pub use crate::robomaster::power_rune::rotation::*;    // 旋转控制组件、旋转插值逻辑
pub use crate::robomaster::power_rune::rune::*;        // 机关核心状态组件
pub use crate::robomaster::power_rune::state::*;       // 机关状态枚举（待机/旋转/激活/通关等）
pub use crate::robomaster::visibility::Activation; // 激活显隐控制组件

// plugin_group! 宏内部语法不允许书写文档注释，注释写在外部
// PowerRunePlugins：能量机关整体插件组，整合全部子功能插件
// 内部包含 3 个子插件，加载顺序从上至下：
// 1. PowerRuneConstructorPlugin：场景初始化，解析能量机关3D模型、生成实体、挂载碰撞体与组件
// 2. PowerRuneCollisionPlugin：运行时碰撞检测，子弹击打机关、统计击打次数
// 3. PowerRuneUpdatePlugin：每一帧更新机关状态、控制旋转动画、切换灯光颜色、达成条件后完成激活
// Bevy 官方宏：用于批量打包多个 Plugin，生成一个 PluginGroup 聚合插件
plugin_group! {
    // 为聚合插件实现 Default 特征，方便 PowerRunePlugins::default() 写法
    #[derive(Default)]
    // 对外暴露的总插件结构体
    pub struct PowerRunePlugins {
        // 语法 `:插件类型`，代表纳入该插件组
        // 第1个子插件：能量机关构造插件（模型解析、生成实体、挂载组件，初始化阶段执行）
        :PowerRuneConstructorPlugin,
        // 第2个子插件：碰撞插件（子弹击打机关的碰撞检测、命中计数逻辑）
        :PowerRuneCollisionPlugin,
        // 第3个子插件：状态更新插件（机关旋转、灯光变色、状态流转、通关逻辑）
        :PowerRuneUpdatePlugin,
    }
}