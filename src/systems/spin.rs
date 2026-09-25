use avian3d::prelude::*;
use bevy::prelude::*;

use crate::components::{ActiveSlapper, Spinning};

/// 纯展示战车自转角速度（rad/s），约 5 秒转一圈
const SPIN_ANGULAR_SPEED: f32 = 2.0;

/// setup_vehicle 给根刚体配置的角阻尼值，停转时恢复，防止车身自转、侧翻、抖动
const VEHICLE_ANGULAR_DAMPING: f32 = 50.0;

/// 展示战车持续自转系统（两种形态自动切换）：
/// 1. 未被 Tab 选中：给挂载 Spinning 的刚体写入恒定角速度，绕竖直轴一直转，
///    同时把角阻尼清零，防止物理引擎把转速衰减掉
/// 2. 被 Tab 选中接手操控（挂上 ActiveSlapper）：立即停转，
///    角速度清零并恢复原角阻尼，交还给玩家操控
pub fn spin_display_vehicle(
    mut query: Query<(&mut AngularVelocity, &mut AngularDamping, Option<&ActiveSlapper>), With<Spinning>>,
) {
    for (mut angular_velocity, mut angular_damping, active_slapper) in &mut query {
        // Tab 选中、进入操控形态：停转，退出自转展示
        if active_slapper.is_some() {
            // 角速度清零，停止自转
            angular_velocity.0 = Vec3::ZERO;
            // 恢复 setup_vehicle 配置的角阻尼，保证后续操控车身平稳
            angular_damping.0 = VEHICLE_ANGULAR_DAMPING;
            continue;
        }

        // 展示形态：关闭角阻尼，保持恒定转速，否则转速会被每步物理积分衰减
        angular_damping.0 = 0.0;
        // 绕世界竖直轴（Y轴）匀速自转
        angular_velocity.0 = Vec3::new(0.0, SPIN_ANGULAR_SPEED, 0.0);
    }
}
