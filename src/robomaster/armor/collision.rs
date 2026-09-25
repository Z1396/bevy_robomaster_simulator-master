//! 装甲碰撞命中检测模块
//!
//! 定义了 ArmorCollisionPlugin，通过 Bevy 观察者模式监听 Avian3D 物理引擎的
//! CollisionStart（碰撞开始）事件，判断子弹是否命中了敌方装甲，并更新全局命中统计。

// 引入 Avian3D 3D物理库：碰撞开始事件
use avian3d::prelude::CollisionStart;
// Bevy 基础ECS类型：父子组件、指令队列、实体、生命周期移除事件、观察者、插件、查询、资源、实体过滤器
use bevy::ecs::lifecycle::Remove;
use bevy::prelude::{ChildOf, Commands, Entity, On, Plugin, Query, ResMut, With};

// 同级模块 construct 内的装甲标记组件 Armor
use super::construct::Armor;
// 项目模块：本机操控标记、步兵组件（射手队伍）、队伍枚举、子弹标记组件
use crate::components::{Controlled, Infantry};
use crate::robomaster::power_rune::prelude::Projectile;
use crate::robomaster::prelude::Team;
// 全局统计资源：子弹命中数据统计结构体
use crate::statistic::ProjectileStatistics;

/// 观察者回调系统：处理「碰撞开始」事件，判断子弹是否打中敌方装甲、统计有效命中
/// On<CollisionStart>：观察者仅在发生 CollisionStart 碰撞接触事件时执行
/// 用碰撞开始而非碰撞结束：
/// 1. 命中判定本就该是接触瞬间；
/// 2. 碰撞结束事件会被子弹寿命到期销毁时误触发，导致没打中的子弹也计数
fn handle_armor_collision(
    // 本次碰撞开始事件本体
    event: On<CollisionStart>,
    // ECS指令队列（当前无组件增删需求，保留占位）
    _commands: Commands,
    // 可写全局资源：命中统计数据
    mut stats: ResMut<ProjectileStatistics>,
    // 查询：所有子弹实体
    projectiles: Query<Entity, With<Projectile>>,
    // 查询：所有装甲实体，需要读取队伍字段区分敌我
    armors: Query<&Armor>,
    // 查询：本机操控战车，取其队伍作为射手阵营
    controlled: Query<&Infantry, With<Controlled>>,
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

    // 已命中去重：该子弹此前已计过命中，同一发弹接触多块装甲只算一次
    if !stats.mark_counted(projectile_entity) {
        return;
    }

    // 射手（本机操控战车）所属队伍，没有操控战车时按红方处理
    let shooter_team = controlled
        .single()
        .map(|infantry| infantry.team)
        .unwrap_or(Team::Red);

    // 取出碰撞的另一方（非子弹的碰撞体，大概率是装甲/墙体/地面）
    let other_collider = if projectile_body1.is_some() {
        event.collider2
    } else {
        event.collider1
    };

    // 取另一方装甲的队伍：碰撞体自身带 Armor，
    // 或者碰撞体是装甲的子物体，向上遍历祖先找到带 Armor 的父实体
    let other_armor_team = armors
        .get(other_collider)
        .ok()
        .map(|armor| armor.team)
        .or_else(|| {
            child_of
                .iter_ancestors(other_collider)
                .find_map(|ancestor| armors.get(ancestor).ok().map(|armor| armor.team))
        });

    // 只有命中敌方装甲才算有效命中：
    // 命中自己/队友装甲（含发射瞬间与车身重叠）不计入统计，撤销去重记录并跳过
    let is_enemy_hit = other_armor_team.is_some_and(|team| team != shooter_team);
    if !is_enemy_hit {
        stats.uncount(projectile_entity);
        return;
    }

    // 命中敌方装甲，统计有效命中 +1
    stats.increase_accurate();
}

/// 子弹销毁/移除时触发的清理观察者
/// 把该子弹的命中去重记录从统计资源中删除，防止集合随发射量无限增长
fn cleanup_counted_projectile(event: On<Remove, Projectile>, mut stats: ResMut<ProjectileStatistics>) {
    stats.uncount(event.entity);
}

/// 装甲碰撞命中插件，私有化仅当前模块可见
#[derive(Default)]
pub(super) struct ArmorCollisionPlugin;

impl Plugin for ArmorCollisionPlugin {
    fn build(&self, app: &mut bevy::app::App) {
        // 注册观察者：监听全局 CollisionStart 碰撞接触事件，触发命中检测函数
        app.add_observer(handle_armor_collision);
        // 注册观察者：子弹实体移除时清理命中去重记录
        app.add_observer(cleanup_counted_projectile);
    }
}
