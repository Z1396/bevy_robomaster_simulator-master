// 导入 Avian3D 3D物理引擎全套常用类型（碰撞层、刚体、碰撞体、物理系统等）
use avian3d::prelude::*;
// Bevy游戏引擎基础万物：组件、资源、枚举、计时器、材质、网格、可见性等
use bevy::prelude::*;
// 哈希表，用来存储命名碰撞体配置
use std::collections::HashMap;

/// 物理碰撞分层枚举，Avian3D 依靠该枚举实现**碰撞过滤**
/// 控制「谁和谁能发生碰撞、谁忽略谁碰撞」，避免子弹打己方、车体自碰撞等问题
#[derive(PhysicsLayer, Default, Clone, Copy, Debug)]
pub enum GameLayer {
    // 默认层级，通用物体
    #[default]
    Default,
    // 己方载具
    VehicleSelf,
    // 敌方载具
    VehicleOther,
    // 己方发射的子弹/弹丸
    ProjectileSelf,
    // 敌方子弹
    ProjectileOther,
    // 场景环境：墙体、地面、障碍物
    Environment,
}

/// 组件：挂载在子弹实体上，控制子弹生命周期
/// Deref/DerefMut 自动解包内部Timer，使用时直接 .tick() 即可
#[derive(Component, Deref, DerefMut)]
pub struct ProjectileLifetime(pub Timer);

/// 全局资源：全局发射冷却计时器，用来限制发射频率（防止一秒射出几十发子弹）
#[derive(Resource, Deref, DerefMut)]
pub struct ProjectileCooldown(pub Timer);

/// 全局资源：子弹公用的网格+材质资源
/// 所有子弹复用同一套Mesh与材质，减少GPU资源重复创建，优化性能
#[derive(Resource)]
pub struct ProjectileSetting(pub Handle<Mesh>, pub Handle<StandardMaterial>);

/// 组件：精细分层碰撞配置容器
/// 给载具使用，可按名称配置车身不同部位碰撞体（炮塔、底盘、装甲、炮管等）
#[derive(Component, Deref, DerefMut)]
pub struct PreciousCollision(
    pub  HashMap<
        // Key：碰撞部位名称，例如 "chassis"(底盘)、"turret"(炮塔)、"barrel"(炮管)
        String,
        // Value：单个碰撞体完整配置组
        (
            // 分层碰撞构造器：可给子物体挂载不同形状碰撞盒（胶囊、方块、球体）
            ColliderConstructorHierarchy,
            // 该碰撞层的碰撞规则：能和哪些Layer产生碰撞
            CollisionLayers,
            // 碰撞体是否可见（调试时开启碰撞盒可视化，正式游戏隐藏）
            Visibility,
            // 可选刚体类型：None=静态碰撞体、Some(RigidBody::Dynamic)=动态刚体
            Option<RigidBody>,
        ),
    >,
);

// 子弹存活最大时长：5秒，超时自动销毁防止子弹无限存在内存泄漏
pub const PROJECTILE_LIFETIME_SECS: f32 = 5.0;