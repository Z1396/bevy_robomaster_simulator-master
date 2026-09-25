use avian3d::prelude::*;
use bevy::prelude::*;

use crate::components::Spinning;

/// 纯展示战车自转角速度（rad/s），约 5 秒转一圈
const SPIN_ANGULAR_SPEED: f32 = 5.0;

/// 展示战车持续自转系统
/// 给挂载 Spinning 的刚体直接写入恒定角速度，让它绕着竖直轴一直转
/// 同时把角阻尼清零，防止物理引擎把转速衰减掉
pub fn spin_display_vehicle(
    mut query: Query<(&mut AngularVelocity, &mut AngularDamping), With<Spinning>>,
) {
    for (mut angular_velocity, mut angular_damping) in &mut query {
        // 关闭角阻尼，保持恒定转速，否则转速会被每步物理积分衰减
        angular_damping.0 = 0.0;
        // 绕世界竖直轴（Y轴）匀速自转
        angular_velocity.0 = Vec3::new(0.0, SPIN_ANGULAR_SPEED, 0.0);
    }
}
