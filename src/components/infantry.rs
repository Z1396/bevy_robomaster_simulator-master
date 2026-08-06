// 导入 Bevy 引擎基础全部类型
use bevy::prelude::*;
// 导入项目内部机甲模块的队伍枚举、机器人配置结构体
use crate::robomaster::prelude::{RobotConfig, Team};

/// 标记组件：被玩家/键鼠/指令操控的机器人
/// 只要实体带有 Controlled，就会接收键盘、遥控器、云台指令来运动
#[derive(Component)]
pub struct Controlled;

/// 步兵主体组件，挂载在机器人根实体上
/// 存储机器人所属阵营、全局配置参数
#[derive(Component)]
pub struct Infantry {
    pub team: Team,         // 所属红方/蓝方
    pub config: RobotConfig,// 机器人全局配置：最大速度、血量、弹丸参数、摩擦系数等
}

impl Infantry {
    /// 常量构造函数，编译期可构造实例，零开销
    pub const fn new(team: Team, config: RobotConfig) -> Self {
        Self { team, config }
    }
}

/// 步兵底盘组件：存放底盘偏航状态
/// 底盘只能原地旋转yaw，负责车身转向
#[derive(Component, Default)]
pub struct InfantryChassis {
    pub yaw: f32,           // 底盘当前朝向角度（弧度）
    pub yaw_velocity: f32, // 底盘旋转角速度 rad/s，用于动力学积分更新角度
}

/// 步兵云台组件：云台独立于底盘转动
/// local_yaw：云台相对底盘的水平转角；pitch：俯仰角度
#[derive(Component, Default)]
pub struct InfantryGimbal {
    pub local_yaw: f32, // 云台相对底盘的水平偏航（云台左右转）
    pub pitch: f32,    // 云台俯仰角（枪管上下抬）
}

/// 标记组件：相机挂载点偏移实体，云台视角安装位置
/// 在子实体上附加该组件，代表这个坐标是车载相机的安装位置
#[derive(Component)]
pub struct InfantryViewOffset;

/// 标记组件：发射机构坐标偏移实体
/// 标记枪口位置，用来生成弹丸、计算弹道起点
#[derive(Component)]
pub struct InfantryLaunchOffset;

/// 标记组件：具备撞击碰敌机构的步兵（工程车/撞击步兵）
#[derive(Component)]
pub struct SlapperInfantry;

/// 标记组件：当前正在被操控的撞击型步兵
/// 全局只会有1个实体同时拥有 ActiveSlapper，用来切换操控目标
#[derive(Component)]
pub struct ActiveSlapper;