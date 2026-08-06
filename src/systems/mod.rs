// 声明当前模块下的各个子模块
mod camera;                // 相机视角控制模块（前面相机切换、跟随、自由视角逻辑都在这里）
mod chassis_observation;    // 底盘观测模块：观测战车姿态、位姿、运动状态、状态输出
mod debug;                 // 调试模块：绘制调试线条、碰撞框、弹道、坐标、打印日志等调试功能
mod input;                 // 输入控制系统模块：刚刚注释的战车操控、云台控制、按键切换逻辑全部放在此处
mod projectile;            // 弹丸模块：子弹发射、弹道物理、碰撞判定、伤害逻辑
mod uav;                   // 无人机模块（无人机视角、飞行控制、侦察等扩展功能）

// 将各个子模块的公开内容导出到当前作用域，外部文件 use 本模块时可直接使用，不用再逐层嵌套
pub use camera::*;
pub use chassis_observation::*;
pub use debug::*;
pub use input::*;
pub use projectile::*;
pub use uav::*;

use bevy::prelude::*;

/// 游戏玩法阶段枚举：定义 **游戏主线业务的系统执行阶段（SystemSet）**
/// 作用：划分游戏逻辑执行顺序，严格控制系统先后执行，避免时序错乱
#[derive(SystemSet, Clone, PartialEq, Eq, Hash, Debug)]
pub enum GameplaySystems {
    Input,          // 阶段1：读取键盘/鼠标输入，更新输入状态
    GameLogic,      // 阶段2：游戏核心逻辑（底盘运动、云台转动、自瞄、弹道、对战规则）
    Camera,         // 阶段3：更新相机位置、视角跟随
    Cleanup,        // 阶段4：帧末清理临时实体、过期子弹、无用标记，避免内存堆积
}