// ============================================================
// 模块名：outpost/prelude
// 作用：前哨站子模块的预导入与插件聚合
// 职责：统一导出前哨站子模块对外暴露的公共 API，并将构造插件
//       （`OutpostConstructorPlugin`）与更新插件（`OutpostUpdatePlugin`）
//       聚合为 `OutpostPlugins` 插件组，供上层一键启用前哨站全部功能。
// ============================================================

use crate::robomaster::outpost::construct::OutpostConstructorPlugin;
use bevy::app::plugin_group;

// 重导出构造模块的全部公共项，便于外部通过 `outpost::prelude::*` 访问
pub use crate::robomaster::outpost::construct::*;
use crate::robomaster::outpost::update::OutpostUpdatePlugin;

plugin_group! {
    /// 前哨站插件组，聚合了前哨站仿真所需的全部子插件。
    ///
    /// 包含：
    /// - `OutpostConstructorPlugin`：负责前哨站场景节点的初始化装配。
    /// - `OutpostUpdatePlugin`：负责前哨站每帧的旋转更新。
    ///
    /// 使用时只需将 `OutpostPlugins` 加入 Bevy 应用即可启用前哨站完整功能。
    #[derive(Default)]
    pub struct OutpostPlugins {
        :OutpostConstructorPlugin,
        :OutpostUpdatePlugin,
    }
}
