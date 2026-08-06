//! 能量机关的碰撞检测与命中处理。
//!
//! 定义了弹丸（`Projectile`）组件、目标索引（`RuneIndex`）组件，
//! 以及碰撞事件到机关状态机的命中转发逻辑。
//! 通过 `PowerRuneCollisionPlugin` 注册碰撞观察者与资源清理系统。

use crate::robomaster::power_rune::common::RuneHitOutcome;
use crate::robomaster::power_rune::rotation::PowerRuneRotation;
use crate::robomaster::power_rune::rune::PowerRuneMechanism;
use avian3d::prelude::{CollisionEnd, CollisionEventsEnabled};
use bevy::prelude::{
    ChildOf, Commands, Component, Entity, EntityEvent, On, Query, ResMut, Resource, Update, With,
};
use std::collections::HashSet;

/// 弹丸组件，标记一个实体为可命中机关目标的弹丸。
///
/// 自动附带 `CollisionEventsEnabled`，确保碰撞事件被正确触发。
#[derive(Component)]
#[require(CollisionEventsEnabled)]
pub struct Projectile;

/// 已消耗的弹丸集合，防止同一个弹丸多次触发机关命中。
#[derive(Resource, Default)]
struct ConsumedRuneProjectiles(HashSet<Entity>);

/// 目标索引组件，标记场景实体所属的机关面与目标编号。
#[derive(Component, Debug, Copy, Clone)]
pub struct RuneIndex {
    /// 目标在机关面中的逻辑索引（0..RUNE_TARGET_COUNT）。
    pub target: usize,
    /// 所属机关面的实体引用。
    pub rune: Entity,
}

/// 命中结果，封装 `RuneHitOutcome` 并提供便捷查询方法。
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct HitResult {
    /// 命中的具体结果分类。
    pub outcome: RuneHitOutcome,
}

impl HitResult {
    /// 判断本次命中是否为有效命中（准确命中了一个可命中的目标）。
    pub const fn accurate(self) -> bool {
        self.outcome.is_accurate()
    }
}

/// 机关成功激活事件，在机关完成所有目标激活后触发。
#[derive(EntityEvent)]
pub struct RuneActivated {
    /// 被激活的机关实体。
    #[event_target]
    pub rune: Entity,
}

/// 机关命中事件，在弹丸命中机关目标时触发。
#[derive(EntityEvent)]
pub struct RuneHit {
    /// 被命中的机关实体。
    #[event_target]
    pub rune: Entity,
    /// 命中结果。
    pub result: HitResult,
}

/// 碰撞事件处理函数：当弹丸与机关目标发生碰撞时，将命中事件转发到机关状态机。
///
/// # 流程
/// 1. 从碰撞事件中识别弹丸实体与目标碰撞体
/// 2. 通过 `RuneIndex` 查找目标对应的机关面
/// 3. 将弹丸标记为已消耗，防止重复触发
/// 4. 调用状态机的 `hit` 方法处理命中判定
/// 5. 同步旋转控制器的激活状态
/// 6. 触发 `RuneHit` 事件，如果机关完全激活则额外触发 `RuneActivated` 事件
fn handle_rune_collision(
    event: On<CollisionEnd>,
    mut commands: Commands,
    mut consumed_projectiles: ResMut<ConsumedRuneProjectiles>,
    mut runes: Query<(&mut PowerRuneMechanism, &mut PowerRuneRotation)>,
    targets: Query<&RuneIndex>,
    projectiles: Query<Entity, With<Projectile>>,
    child_of: Query<&ChildOf>,
) {
    // 确定碰撞双方中哪一个是弹丸，哪一个是目标
    let projectile_body1 = event.body1.and_then(|body| projectiles.get(body).ok());
    let projectile_body2 = event.body2.and_then(|body| projectiles.get(body).ok());

    let (projectile_entity, target_collider) = match (projectile_body1, projectile_body2) {
        (Some(projectile), _) => (projectile, event.collider2),
        (_, Some(projectile)) => (projectile, event.collider1),
        _ => return,
    };

    // 通过目标碰撞体查找对应的机关目标索引
    let Some(target) = find_rune_target(target_collider, &targets, &child_of) else {
        return;
    };

    let Ok((mut mechanism, mut rotation)) = runes.get_mut(target.rune) else {
        return;
    };

    // 如果弹丸已被消耗，忽略本次碰撞
    if !consumed_projectiles.0.insert(projectile_entity) {
        return;
    }

    // 移除弹丸的碰撞事件，避免后续碰撞
    commands
        .entity(projectile_entity)
        .remove::<CollisionEventsEnabled>();

    let mut rng = rand::rng();
    let outcome = mechanism.state_mut().hit(target.target, &mut rng);
    // 同步旋转控制器的激活状态，确保大机关在激活时使用变量旋转
    rotation.sync_activation(
        mechanism.state().mode(),
        mechanism.state().is_activating(),
        &mut rng,
    );

    // 触发命中事件
    commands.trigger(RuneHit {
        rune: target.rune,
        result: HitResult { outcome },
    });

    // 如果机关完全激活，额外触发激活事件
    if outcome.activates_rune() {
        commands.trigger(RuneActivated { rune: target.rune });
    }
}

/// 清理已消耗弹丸集合的系统：移除不再存在的弹丸实体引用。
///
/// 防止已消耗集合无限增长，确保内存安全。
fn cleanup_consumed_rune_projectiles(
    mut consumed_projectiles: ResMut<ConsumedRuneProjectiles>,
    projectiles: Query<(), With<Projectile>>,
) {
    consumed_projectiles
        .0
        .retain(|entity| projectiles.contains(*entity));
}

/// 从碰撞体实体向上遍历层次结构，查找第一个附带 `RuneIndex` 的祖先。
///
/// 机关目标的面板可能由多个子实体组成，碰撞可能发生在任意子实体上。
/// 此函数通过 `ChildOf` 层次遍历，找到最近的机关目标索引。
///
/// # 参数
/// - `entity`：当前碰撞体实体
/// - `targets`：`RuneIndex` 组件查询
/// - `child_of`：`ChildOf` 组件查询，用于向上遍历
///
/// # 返回值
/// 找到的 `RuneIndex`，如果未找到则返回 `None`。
fn find_rune_target(
    entity: Entity,
    targets: &Query<&RuneIndex>,
    child_of: &Query<&ChildOf>,
) -> Option<RuneIndex> {
    // 先检查实体本身是否带有 RuneIndex
    if let Ok(target) = targets.get(entity) {
        return Some(*target);
    }

    // 向上遍历父级链，查找带有 RuneIndex 的祖先
    child_of
        .iter_ancestors(entity)
        .find_map(|ancestor| targets.get(ancestor).ok().copied())
}

/// 能量机关碰撞处理插件，注册弹丸消耗集合与碰撞观察者。
#[derive(Default)]
pub(super) struct PowerRuneCollisionPlugin;

impl bevy::app::Plugin for PowerRuneCollisionPlugin {
    fn build(&self, app: &mut bevy::app::App) {
        app.init_resource::<ConsumedRuneProjectiles>()
            // 每帧清理已消耗弹丸记录
            .add_systems(Update, cleanup_consumed_rune_projectiles)
            // 注册碰撞事件观察者
            .add_observer(handle_rune_collision);
    }
}
