// 导入 robomaster 下四大子模块：装甲、前哨基地、能量机关、战术核心
use crate::robomaster::{armor, outpost, power_rune, tech_core};
// Bevy 程序实例
use bevy::app::App;
// Bevy 插件标准 trait
use bevy::prelude::Plugin;

// 对外导出 robomaster/common 下所有公开内容，外部使用时无需层层use
pub use crate::robomaster::common::*;
// 导入各个子系统的插件入口
use crate::robomaster::outpost::prelude::OutpostPlugins;
use crate::robomaster::tech_core::prelude::TechCorePlugins;
use crate::robomaster::visibility::StatefulAppearancePlugin;

// pub 向外导出各个子模块 prelude 全部内容
// 外部只要 use robomaster::prelude::* 就能拿到装甲、能量机关等所有组件/系统
pub use armor::prelude::*;
pub use outpost::prelude::*;
pub use power_rune::prelude::*;
pub use tech_core::prelude::*;

/// RM 机器人整体总插件，聚合所有机器人子功能
#[derive(Default)]
pub struct RoboMasterPlugins;

// 实现 Bevy 的 Plugin 特征，这是 Bevy 插件标准写法
impl Plugin for RoboMasterPlugins {
    /// app.add_plugins() 时自动执行，挂载所有机器人业务插件
    fn build(&self, app: &mut App) {
        app
            // 实体外观状态插件：控制装甲亮灯、护甲变红、被击打变色等外观逻辑
            .add_plugins(StatefulAppearancePlugin)
            // 装甲系统：装甲碰撞、击打检测、血量、装甲组件挂载
            .add_plugins(ArmorPlugins)
            // 能量机关系统
            .add_plugins(PowerRunePlugins)
            // 前哨站逻辑
            .add_plugins(OutpostPlugins)
            // 战术核心模块
            .add_plugins(TechCorePlugins);
    }
}

// 宏导出标记：其他 crate（外部包）也能使用这个 entity_root! 宏
#[macro_export]
macro_rules! entity_root {
    // ========== 主入口分支：用户调用宏的外层语法 ==========
    // 调用范例：
    // entity_root!(
    //     super parent_entity => children_map;
    //     name name_component;
    //     root_entity {
    //         match {
    //             "armor_1" => armor_node { spawn_armor() };
    //             :"_gimbal" => gimbal_node { setup_gimbal() };
    //         }
    //     }
    // );
    (
        super $child_of:expr => $children:expr;
        name $name:expr;
        $root:ident {
            $($expr:tt)*
        }
    ) => {{
        // 绑定传入的参数，避免重复求值
        let _child_of = &$child_of;
        let _children = &$children;
        let _name = &$name;
        let _root = $root;
        // 转发至内部核心处理分支
        $crate::entity_root!(@internal _root, _name, _child_of, _children, { $($expr)* });
    }};

    // ========== 匹配分支 1：精确字符串匹配 ==========
    // 语法："标签名" => 变量名 { 内部语句 };
    // 作用：实体名称完全等于 label 时匹配成功
    (@match $root:expr, $name:ident, $child_of:ident, $children:ident,
            $name_str:ident,
            $label:expr => $ident:ident {$($tt:tt)*}; $($rest:tt)*
    )=>{
        if $name_str == $label {
            // 匹配成功，将当前实体绑定为 $ident
            let $ident = $root;
            // 进入内部执行用户写的业务语句
            $crate::entity_root!(@internal $ident, $name, $child_of, $children, {$($tt)*});
            // continue：匹配命中后终止后续匹配分支
            continue;
        }
        // 未命中，继续匹配剩下的规则
        $crate::entity_root!(@match $root, $name, $child_of, $children, $name_str, $($rest)*);
    };

    // ========== 匹配分支2：后缀匹配 :后缀 => ==========
    // 语法 :"_gimbal" => node { ... }
    // 实体名字 以 label 结尾就匹配，比如 chassis_gimbal、armor_gimbal 都命中
    (@match $root:expr, $name:ident, $child_of:ident, $children:ident,
            $name_str:ident,
            :$label:literal => $ident:ident {$($tt:tt)*}; $($rest:tt)*
    )=>{
        if $name_str.ends_with(&$label) {
            let $ident = $root;
            $crate::entity_root!(@internal $ident, $name, $child_of, $children, {$($tt)*});
            continue;
        }
        $crate::entity_root!(@match $root, $name, $child_of, $children, $name_str, $($rest)*);
    };

    // ========== 匹配分支3：前缀匹配 前缀: => ==========
    // 语法 "armor_": => node {}
    // 实体名称以 armor_ 开头就匹配：armor_0 armor_1 armor_2
    (@match $root:expr, $name:ident, $child_of:ident, $children:ident,
            $name_str:ident,
            $label:literal: => $ident:ident {$($tt:tt)*}; $($rest:tt)*
    )=>{
        if $name_str.starts_with(&$label) {
            let $ident = $root;
            $crate::entity_root!(@internal $ident, $name, $child_of, $children, {$($tt)*});
            continue;
        }
        $crate::entity_root!(@match $root, $name, $child_of, $children, $name_str, $($rest)*);
    };

    // ========== 匹配分支4：包含匹配 :包含内容: => ==========
    // 语法 :"armor": => node {}
    // 只要实体名称中间包含该字符串就匹配，all_armor、armor_left 全都命中
    (@match $root:expr, $name:ident, $child_of:ident, $children:ident,
            $name_str:ident,
            :$label:literal: => $ident:ident {$($tt:tt)*}; $($rest:tt)*
    )=>{
        if $name_str.contains(&$label) {
            let $ident = $root;
            $crate::entity_root!(@internal $ident, $name, $child_of, $children, {$($tt)*});
            continue;
        }
        $crate::entity_root!(@match $root, $name, $child_of, $children, $name_str, $($rest)*);
    };

    // ========== 内部分支 A：遇到 match 区块 → 递归遍历子实体 ==========
    // 当 {} 内部包裹 match { ... } 时：遍历当前实体的所有子实体，挨个拿子实体名称去走 @match 匹配规则
    (@internal $root:expr, $name:ident, $child_of:ident, $children:ident, {
        match {
            $($rest:tt)*
        }
    }) => {{
        // children: HashMap<Entity, Vec<Entity>>，key=父实体，value=子实体列表
        if let Ok(children) = $children.get($root) {
            // 遍历当前实体所有子节点
            for &child in children.iter() {
                // 根据 child 实体拿到它绑定的 Name 组件字符串
                let Ok(name) = $name.get(child) else { continue; };
                let name_str = name.as_str();
                // 将子实体送入匹配规则链进行名字匹配
                $crate::entity_root!(@match child, $name, $child_of, $children, name_str, $($rest)*);
            }
        }
    }};

    // ========== 内部分支 B：普通语句块，无match，直接执行用户代码 ==========
    // 匹配命中后，执行你写的初始化逻辑（挂载装甲组件、添加标签、设置Transform等）
    (@internal $root:expr, $name:ident, $child_of:ident, $children:ident, {
        $($stmt:stmt);* $(;)?
    }) => {{
        // 消除未使用变量警告
        let _ = $root;
        // 执行用户书写的多条语句
        $($stmt)*
    }};

    // ========== 内部分支 C：空内容，什么都不做 ==========
    (@internal $root:expr, $name:ident, $child_of:ident, $children:ident, $(;)?) => {};

    // ========== 兜底分支：所有匹配规则走完无命中，结束匹配 ==========
    (@match $root:expr, $name:ident, $child_of:ident, $children:ident,
            $name_str:ident,
            $(;)?
    )=>{};
}