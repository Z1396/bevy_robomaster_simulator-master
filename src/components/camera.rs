// 引入 Bevy 引擎全部常用前置模块
use bevy::prelude::*;
// 原子布尔类型，多线程安全的布尔标记，无锁并发读写
use std::sync::atomic::AtomicBool;

/// 组件：挂载在主相机实体上，记录相机跟随偏移量
/// 只有带有 MainCamera 组件的实体，才会被视作游戏主视角相机
#[derive(Component)]
pub struct MainCamera {
    /// 相机相对于跟随目标（机器人）的位置偏移
    /// 例如 Vec3(0.0, 2.0, -5.0) = 在机器人后上方5米处第三人称跟随
    pub follow_offset: Vec3,
}

/// 全局资源：控制当前相机跟随模式
/// #[derive(PartialEq)]：允许判断两种模式是否相等
/// Deref + DerefMut：解引用语法糖，使用时可直接 `*camera_mode` 拿到内部 FollowingType
#[derive(Resource, PartialEq, Deref, DerefMut)]
pub struct CameraMode(pub FollowingType);

// 默认相机模式：默认跟随机器人本体
impl Default for CameraMode {
    fn default() -> Self {
        Self(FollowingType::Robot)
    }
}

/// 全局资源：是否订阅自瞄IPC指令的开关
/// AtomicBool：原子布尔，支持主线程 + IPC子线程并发安全读写
/// true = 接收 talos-mock-server 下发的云台指令、跟随自瞄视角
/// false = 关闭自瞄订阅，相机由键鼠自由控制
#[derive(Resource, Deref, DerefMut)]
pub struct SubscribeAutoAim(pub AtomicBool);

/// 相机跟随模式枚举，决定主相机的行为逻辑
#[derive(PartialEq, Clone, Copy)]
pub enum FollowingType {
    /// Free：自由视角，键鼠拖拽旋转、移动，不绑定机器人
    Free,
    /// Robot：紧贴机器人机身跟随，相机固定在机器人头部/云台位置
    Robot,
    /// ThirdPerson：第三人称尾随视角，在机器人后方偏移观察整机
    ThirdPerson,
}