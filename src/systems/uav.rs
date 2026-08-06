// Avian3D 3D物理引擎（刚体、速度相关组件）
use avian3d::prelude::*;
// Bevy ECS基础
use bevy::prelude::*;

// 项目自定义实体组件
use crate::components::{
    Controlled,               // 玩家操控的主战车标记
    Infantry,                  // 战车本体标签
    InfantryChassis,           // 底盘组件
    InfantryGimbal,            // 云台组件，获取炮管朝向
    InfantryLaunchOffset,     // 无人机发射挂载点（炮管前端坐标偏移）
    ProjectileCooldown,        // 发射冷却组件（本段暂未使用）
};

/// 无人机发射系统：按下 P 键，从战车炮口位置生成无人机场景模型
/// 内置1秒冷却，防止连点瞬间生成大量无人机
pub fn uav_launch(
    time: Res<Time>,
    mut commands: Commands,
    // 查询玩家操控战车：自身坐标、线速度、角速度
    infantry: Single<
        (&Transform, &LinearVelocity, &AngularVelocity),
        (With<Infantry>, With<Controlled>),
    >,
    // 查询云台全局变换 + 云台数据，用来获取炮管朝向旋转
    gimbal: Single<
        (&GlobalTransform, &InfantryGimbal),
        (With<Controlled>, Without<InfantryChassis>),
    >,
    asset_server: Res<AssetServer>, // 资源加载器，加载glb无人机模型
    // 炮管发射挂载点的局部偏移Transform
    launch_offset: Single<&Transform, (With<Controlled>, With<InfantryLaunchOffset>)>,
    // Local局部定时器：系统独有状态，不会存入世界ECS，每个系统独占一份
    mut timer: Local<Option<Timer>>,
    keyboard: Res<ButtonInput<KeyCode>>, // 键盘输入资源
) {
    // 初始化冷却计时器：首次运行创建 1s 一次性定时器
    let mut timer = timer.get_or_insert(Timer::from_seconds(1.0, TimerMode::Once));
    // 推进计时器时间
    timer.tick(time.delta());

    // 冷却未结束，直接退出函数，禁止发射
    if !timer.is_finished() {
        return;
    }
    // 重置计时器，重新进入1秒冷却
    timer.reset();

    // 按下 P 触发无人机生成
    if keyboard.pressed(KeyCode::KeyP) {
        let (tank_transform, _, _) = infantry.into_inner();
        let (gimbal_global_tf, _) = gimbal.into_inner();

        // 计算无人机生成的世界坐标：
        // 战车底盘世界位置 + 云台旋转 × 炮口局部偏移 = 炮口真实世界坐标
        let spawn_pos = tank_transform.translation + (gimbal_global_tf.rotation() * launch_offset.translation);

        // 生成无人机实体
        commands.spawn((
            RigidBody::Static,              // 静态刚体，不受物理碰撞、重力影响，悬浮静止
            SceneRoot(asset_server.load("uav.glb#Scene0")), // 加载无人机glb模型场景
            Transform::IDENTITY.with_translation(spawn_pos),// 设置生成位置，初始旋转为默认
        ));
    }
}