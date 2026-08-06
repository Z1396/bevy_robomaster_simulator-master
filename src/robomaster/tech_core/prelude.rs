// ============================================================
// 模块名：tech_core/prelude
// 作用：科技核心子模块的预导入与插件聚合
// 职责：统一导出科技核心子模块对外暴露的公共 API (如 TechCore,
//       TechCorePhase, LightProgram 等)，并将构造插件
//       (`TechCorePlugin`) 聚合为 `TechCorePlugins` 插件组，
//       供上层一键启用科技核心全部功能。
// ============================================================

use crate::robomaster::tech_core::construct::TechCorePlugin;
use bevy::app::plugin_group;

#[allow(unused_imports)]
pub use crate::robomaster::tech_core::construct::{
    AssemblyLightProgram, BlinkRate, LightColor, LightProgram, TechCore, TechCoreFirstLightSegment,
    TechCoreLightGroup, TechCorePhase, TechCoreRoot, TechCoreStep5Lights, tech_core_state_json,
    tech_core_state_json_from_phases,
};

plugin_group! {
    #[derive(Default)]
    pub struct TechCorePlugins {
        :TechCorePlugin,
    }
}
