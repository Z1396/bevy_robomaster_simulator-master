// 引入 Avian3D 3D物理库：碰撞结束事件、碰撞事件启用标记组件
use avian3d::prelude::{CollisionEnd, CollisionEventsEnabled};
// Bevy 基础ECS类型：父子组件、指令队列、实体、观察者、插件、查询、资源、实体过滤器
use bevy::prelude::{ChildOf, Commands, Entity, On, Plugin, Query, ResMut, With};

// 同级模块 construct 内的装甲标记组件 Armor
use super::construct::Armor;
// 项目模块：子弹标记组件 Projectile
use crate::robomaster::power_rune::prelude::Projectile;
// 全局统计资源：子弹命中数据统计结构体
use crate::statistic::ProjectileStatistics;

/// 观察者回调系统：处理「碰撞结束」事件，判断子弹是否打中装甲、统计有效命中
/// On<CollisionEnd>：观察者仅在发生 CollisionEnd 碰撞分离事件时执行
fn handle_armor_collision(
    // 本次碰撞结束事件本体
    event: On<CollisionEnd>,
    // ECS指令队列，用来增删实体组件
    mut commands: Commands,
    // 可写全局资源：命中统计数据
    mut stats: ResMut<ProjectileStatistics>,
    // 查询：所有带有 Projectile 标记的实体 = 子弹实体
    projectiles: Query<Entity, With<Projectile>>,
    // 查询：只要实体带有 Armor 组件，就判定为装甲碰撞体
    armors: Query<(), With<Armor>>,
    // 父子关系查询，用于遍历碰撞体的所有父级实体
    child_of: Query<&ChildOf>,
) {
    // 判定碰撞双方里，body1 是否是子弹实体；不是则返回 None
    let projectile_body1 = event.body1.and_then(|body| projectiles.get(body).ok());
    // 判定碰撞双方里，body2 是否是子弹实体
    let projectile_body2 = event.body2.and_then(|body| projectiles.get(body).ok());

    // 二选一拿到子弹实体；两个都不是子弹，直接终止函数
    let projectile_entity = match (projectile_body1, projectile_body2) {
        (Some(e), _) => e,
        (_, Some(e)) => e,
        _ => return,
    };

    // 取出碰撞的另一方（非子弹的碰撞体，大概率是装甲/墙体/地面）
    let other_collider = if projectile_body1.is_some() {
        event.collider2
    } else {
        event.collider1
    };

    // 判断两个命中条件任一成立，代表子弹打中装甲：
    // 条件1：碰撞另一方本身就是装甲实体（装甲碰撞体自身带 Armor）
    // 条件2：碰撞体是装甲的子物体（比如装甲附属碰撞片、挂载碰撞盒，遍历祖先找到带Armor的父实体）
    if armors.contains(other_collider)
        || child_of
            .iter_ancestors(other_collider)
            .any(|ancestor| armors.contains(ancestor))
    {
        // 移除子弹身上的 CollisionEventsEnabled 组件
        // Avian3D 规则：没有该组件的实体不会再产生碰撞事件
        // 作用：同一颗子弹穿透多层装甲时，只会统计第一次命中，防止多次累加命中次数
        commands
            .entity(projectile_entity)
            .remove::<CollisionEventsEnabled>();

        // 命中装甲，统计有效命中 +1
        stats.increase_accurate();
    }
}

/// 装甲碰撞命中插件，私有化仅当前模块可见
#[derive(Default)]
pub(super) struct ArmorCollisionPlugin;

impl Plugin for ArmorCollisionPlugin {
    fn build(&self, app: &mut bevy::app::App) {
        // 注册观察者：监听全局 CollisionEnd（碰撞分离）事件，触发命中检测函数
        app.add_observer(handle_armor_collision);
    }
}