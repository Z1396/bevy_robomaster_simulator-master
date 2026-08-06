//! 能量机关的场景构造与初始化。
//!
//! 监听 `SceneInstanceReady` 事件，在 GLTF 场景加载完成后，
//! 通过实体名称匹配构建机关的目标面、视觉控制器与状态机组件，
//! 最后将机关所需的所有组件一次性插入到对应实体上。

use crate::robomaster::power_rune::collision::RuneIndex;
use crate::robomaster::power_rune::common::{RUNE_TARGET_COUNT, RuneMode};
use crate::robomaster::power_rune::rotation::PowerRuneRotation;
use crate::robomaster::power_rune::rune::{PowerRune, PowerRuneMechanism};
use crate::robomaster::power_rune::visual::{PowerRuneVisuals, RuneVisual};
use crate::robomaster::prelude::Team;
use crate::robomaster::visibility::{Controller, StatefulAppearanceCreator};
use crate::util::bevy::{drain_entities_by, insert_all_child};
use crate::{material, visibility};
use avian3d::prelude::CollisionEventsEnabled;
use bevy::ecs::system::SystemParam;
use bevy::prelude::{
    Children, Commands, Component, Entity, Name, On, Query, Res, SceneSpawner, With,
};
use bevy::scene::SceneInstanceReady;
use rand::RngExt;
use std::collections::HashMap;

/// 能量机关根节点标记组件，用于识别场景中属于能量机关的实体。
#[derive(Component)]
pub struct PowerRuneRoot;

/// 为一个机关面构建 5 个目标面的视觉控制器。
///
/// 从场景实体名称映射中提取各目标面的子实体（PADDING、LEGGING_PROGRESSING、
/// ACTIVATED、ACTIVE、DISABLED、COMPLETED、LEGGING_1/2/3），
/// 创建对应的 `Controller` 并组装为 `[RuneVisual; 5]`。
///
/// # 参数
/// - `face_index`：机关面的索引
/// - `face_entity`：机关面实体
/// - `name_map`：实体名称到实体的映射（可变，会从中移除已使用的实体）
/// - `param`：场景构造参数
/// - `creator`：外观控制器创建器
///
/// # 返回值
/// 成功时返回 5 个 `RuneVisual` 的数组，失败时返回 `None`。
fn build_targets(
    face_index: usize,
    face_entity: Entity,
    name_map: &mut HashMap<&str, Entity>,
    param: &mut PowerRuneParam,
    creator: &mut StatefulAppearanceCreator,
) -> Option<[RuneVisual; RUNE_TARGET_COUNT]> {
    let mut targets = Vec::new();
    // 遍历目标编号 1~5，为每个目标构建视觉控制器
    for target_idx in 1..=5 {
        let prefix = format!("FACE_{}_TARGET_{}", face_index, target_idx);

        // 创建边缘填充段的控制器（激活完成时显示特定材质）
        let padding_segments = creator.create_controller(
            drain_entities_by(name_map, |name| {
                name.starts_with(&format!("{}_PADDING", prefix))
            }),
            material!(on = { completed }),
        );
        // 创建激活进度条的控制器（激活中时可见）
        let progress_segments = creator.create_controller(
            drain_entities_by(name_map, |name| {
                name.starts_with(&format!("{}_LEGGING_PROGRESSING", prefix))
            }),
            visibility!(activating),
        );

        // 提取目标面的四种状态实体
        let ad = format!("{}_ACTIVATED", prefix);
        let at = format!("{}_ACTIVE", prefix);
        let d = format!("{}_DISABLED", prefix);
        let c = format!("{}_COMPLETED", prefix);
        let activated = ad.as_str();
        let active = at.as_str();
        let deactivated = d.as_str();
        let completed = c.as_str();

        let activated = name_map.remove(activated);
        let activating = name_map.remove(active);
        let deactivated = name_map.remove(deactivated);
        let completed = name_map.remove(completed);

        // 为所有目标状态实体添加 RuneIndex 和碰撞事件组件
        let logical_index = targets.len();
        for entity in [deactivated, activating, activated, completed]
            .into_iter()
            .flatten()
        {
            insert_all_child(&mut param.commands, entity, &param.children, || {
                (
                    RuneIndex {
                        target: logical_index,
                        rune: face_entity,
                    },
                    CollisionEventsEnabled,
                )
            });
        }

        // 创建三段装饰条的控制器（激活完成或已激活时显示特定材质）
        let mut legging_segments: [Controller; 3] = [
            Controller::new_combined(vec![]),
            Controller::new_combined(vec![]),
            Controller::new_combined(vec![]),
        ];
        for legging_idx in 1..=3 {
            legging_segments[legging_idx - 1] = creator.create_controller(
                drain_entities_by(name_map, |name| {
                    name.starts_with(&format!("{}_LEGGING_{}", prefix, legging_idx))
                        && !name.contains("PROGRESSING")
                }),
                material!(on = {activated, completed}),
            )
        }

        // 组装当前目标的视觉控制器
        targets.push(RuneVisual::new(
            Controller::new_visibility(deactivated, activating, activated, completed),
            legging_segments,
            padding_segments,
            progress_segments,
        ));
    }
    targets.try_into().ok()
}

/// 能量机关场景构造的系统参数，封装了 Commands、SceneSpawner 以及相关查询。
#[derive(SystemParam)]
struct PowerRuneParam<'w, 's> {
    commands: Commands<'w, 's>,
    scene_spawner: Res<'w, SceneSpawner>,

    /// 用于判断事件实体是否为能量机关根节点。
    power_query: Query<'w, 's, (), With<PowerRuneRoot>>,
    /// 场景中所有实体的名称组件。
    names: Query<'w, 's, &'static Name>,
    /// 场景中所有实体的子节点列表。
    children: Query<'w, 's, &'static Children>,
}

/// 能量机关初始化函数，监听 `SceneInstanceReady` 事件。
///
/// # 流程
/// 1. 检查事件实体是否带有 `PowerRuneRoot` 组件
/// 2. 构建实体名称到实体的映射
/// 3. 解析所有机关面（FACE_0, FACE_1, ...）
/// 4. 为每个机关面构建目标视觉、状态机、旋转控制器
/// 5. 将所有组件插入到机关面实体
fn setup_power_rune(
    events: On<SceneInstanceReady>,
    mut param: PowerRuneParam,
    mut creator: StatefulAppearanceCreator,
) {
    // 仅处理能量机关场景实例
    if !param.power_query.contains(events.entity) {
        return;
    }

    // 构建实体名称到实体的 HashMap
    let names = param.names;
    let mut name_map = param
        .scene_spawner
        .iter_instance_entities(events.instance_id)
        .filter_map(|entity| names.get(entity).map(|n| (n.as_str(), entity)).ok())
        .fold(HashMap::new(), |mut m, (name, entity)| {
            m.insert(name, entity);
            m
        });

    if name_map.is_empty() {
        return;
    }

    // 查找所有机关面实体（名称格式 "FACE_<index>"，不含下划线后缀）
    let mut faces: Vec<(usize, Entity)> = name_map
        .iter()
        .filter_map(|(name, &entity)| {
            let rest = name.strip_prefix("FACE_")?;
            if rest.contains('_') {
                return None;
            }
            let index = rest.parse::<usize>().ok()?;
            Some((index, entity))
        })
        .collect();

    // 按索引排序
    faces.sort_by_key(|(idx, _)| *idx);
    if faces.is_empty() {
        return;
    }

    // 随机决定红方机关的旋转方向
    let red_clockwise = rand::rng().random_bool(0.5);

    for (index, face_entity) in faces {
        // 机关模式：索引第 2 位为 1 的为大机关，否则为小机关
        let mode = if index & 2 > 0 {
            RuneMode::Large
        } else {
            RuneMode::Small
        };

        // 提取机关面根节点的激活/非激活实体
        let deactivated = name_map.remove(format!("FACE_{}_R_UNPOWERED", index).as_str());
        let activated = name_map.remove(format!("FACE_{}_R_POWERED", index).as_str());

        // 构建该机关面的 5 个目标视觉控制器
        let Some(targets) =
            build_targets(index, face_entity, &mut name_map, &mut param, &mut creator)
        else {
            continue;
        };

        // 队伍归属：索引第 0 位为 1 的为红方，否则为蓝方
        let team = if (index & 1) > 0 {
            Team::Red
        } else {
            Team::Blue
        };
        // 旋转方向：红蓝双方相反，增加视觉变化
        let clockwise = match team {
            Team::Red => red_clockwise,
            Team::Blue => !red_clockwise,
        };

        // 组装机关组件并插入实体
        let mut visuals = PowerRuneVisuals::new(
            Controller::new_visibility(deactivated, activated, activated, activated),
            targets,
        );
        let mechanism = PowerRuneMechanism::new(mode);
        // 首次应用视觉，将机关初始状态同步到外观控制器
        visuals.apply(mode, mechanism.state(), &mut creator.appearance);

        param.commands.entity(face_entity).insert((
            PowerRune::new(team, mode),
            mechanism,
            PowerRuneRotation::new(clockwise),
            visuals,
        ));
    }
}

/// 能量机关构造插件，注册场景初始化观察者。
#[derive(Default)]
pub(super) struct PowerRuneConstructorPlugin;

impl bevy::app::Plugin for PowerRuneConstructorPlugin {
    fn build(&self, app: &mut bevy::app::App) {
        app.add_observer(setup_power_rune);
    }
}
