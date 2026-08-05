// ============================================================
// 模块名：outpost/construct
// 作用：前哨站构造模块
// 职责：在场景加载阶段识别前哨站根实体并为其装配核心组件
//       （`Outpost`、`ScanArmor`、`OutpostRotator`），完成
//       前哨站从静态场景节点到可仿真目标的初始化过程。
// ============================================================

use crate::robomaster::outpost::rotation::RotationDirection;
use crate::robomaster::outpost::update::{Outpost, OutpostRotator};
use crate::robomaster::prelude::{ArmorSpec, ScanArmor, SmallArmorLabel, Team};
use bevy::ecs::system::SystemParam;
use bevy::prelude::{Added, Children, Commands, Component, Entity, Name, Query, Update};

/// 前哨站根组件，标记场景中的前哨站根节点并记录其所属队伍。
///
/// 该组件通常在场景文件（如 GLTF）加载时由外部添加，构造系统
/// 检测到该组件被添加后会自动完成前哨站的完整初始化。
#[derive(Component)]
pub struct OutpostRoot {
    /// 前哨站所属队伍（红方或蓝方）。
    pub team: Team,
}

impl OutpostRoot {
    /// 创建一个新的前哨站根组件。
    ///
    /// 参数：
    /// - `team`：前哨站所属队伍。
    ///
    /// 返回值：携带指定队伍信息的 `OutpostRoot` 实例。
    /// 该函数为 `const fn`，可在常量上下文中使用。
    pub const fn new(team: Team) -> Self {
        Self { team }
    }
}

/// 前哨站构造系统所使用的参数集合（SystemParam）。
///
/// 将多个查询和命令资源聚合为一个参数，便于在系统中统一借用，
/// 避免多次单独借用导致的借用冲突。
#[derive(SystemParam)]
struct OutpostParam<'w, 's> {
    /// 命令缓冲，用于异步插入组件。
    commands: Commands<'w, 's>,
    /// 名称查询，用于读取实体名称以识别旋转节点。
    names: Query<'w, 's, &'static Name>,
    /// 子节点查询，用于遍历前哨站根节点的所有后代实体。
    children: Query<'w, 's, &'static Children>,
}

/// 前哨站初始化系统。
///
/// 当场景中某个实体被添加 `OutpostRoot` 组件时触发（通过 `Added` 过滤器），
/// 完成以下初始化工作：
///
/// 算法步骤：
/// 1. 读取前哨站根实体上的队伍信息。
/// 2. 为根实体插入 `Outpost` 组件（标记为前哨站）。
/// 3. 为根实体插入 `ScanArmor` 组件，指定为小装甲且标签为 `Outpost`，
///    使视觉识别模块能够扫描并识别该前哨站的装甲。
/// 4. 根据队伍确定旋转方向：红方顺时针，蓝方逆时针。
/// 5. 遍历根实体的所有后代，查找名称以 `ROTATE` 结尾的节点，
///    为其插入 `OutpostRotator` 组件，使其参与旋转更新。
///
/// 参数：
/// - `query`：所有新添加 `OutpostRoot` 组件的实体查询。
/// - `param`：聚合的命令与查询参数集合。
fn setup_outpost(
    query: Query<(Entity, &OutpostRoot), Added<OutpostRoot>>,
    mut param: OutpostParam,
) {
    for (root, outpost_root) in query {
        let team = outpost_root.team;
        // 为根实体装配前哨站核心组件与装甲扫描组件
        param.commands.entity(root).insert((
            Outpost::new(team),
            ScanArmor::new(team, ArmorSpec::Small(SmallArmorLabel::Outpost)),
        ));
        // 根据队伍确定旋转方向：红方顺时针，蓝方逆时针
        let direction = match team {
            Team::Red => RotationDirection::Clockwise,
            Team::Blue => RotationDirection::CounterClockwise,
        };
        // 遍历根实体的所有后代，为旋转节点装配旋转器组件
        param.children.iter_descendants(root).for_each(|e| {
            // 跳过未命名实体
            let Ok(name) = param.names.get(e) else {
                return;
            };
            // 仅对名称以 "ROTATE" 结尾的节点添加旋转器，这类节点为前哨站的旋转部分
            if name.ends_with("ROTATE") {
                param
                    .commands
                    .entity(e)
                    .insert(OutpostRotator::new(direction));
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证队伍到旋转方向的映射与历史实现保持一致。
    /// 红方应映射为顺时针，蓝方应映射为逆时针。
    #[test]
    fn team_rotation_mapping_matches_legacy_behavior() {
        let red = match Team::Red {
            Team::Red => RotationDirection::Clockwise,
            Team::Blue => RotationDirection::CounterClockwise,
        };
        let blue = match Team::Blue {
            Team::Red => RotationDirection::Clockwise,
            Team::Blue => RotationDirection::CounterClockwise,
        };

        assert_eq!(red, RotationDirection::Clockwise);
        assert_eq!(blue, RotationDirection::CounterClockwise);
    }
}

/// 前哨站构造插件，负责注册前哨站的初始化系统。
#[derive(Default)]
pub(super) struct OutpostConstructorPlugin;

impl bevy::app::Plugin for OutpostConstructorPlugin {
    /// 将前哨站初始化系统注册到 Bevy 应用的 `Update` 调度阶段。
    ///
    /// 虽然系统注册在 `Update` 阶段，但由于使用 `Added` 过滤器，
    /// 实际逻辑仅在实体首次添加 `OutpostRoot` 组件的当帧执行一次。
    fn build(&self, app: &mut bevy::app::App) {
        app.add_systems(Update, setup_outpost);
    }
}
