use avian3d::prelude::*;
use bevy::prelude::*;

use crate::components::{ActiveSlapper, Spinning};

/// 纯展示战车自转角速度（rad/s），约 5 秒转一圈
const SPIN_ANGULAR_SPEED: f32 = 3.0;

/// 展示战车闲置自转系统
///
/// Spinning 是永久挂载的能力标记（spawn 时挂上，从不摘除），
/// 加 Without<ActiveSlapper> 过滤后天然只有两种形态：
/// 1. 闲置（未被 Tab 选中，根上无 ActiveSlapper）：角阻尼清零 + 恒定角速度匀速自转
/// 2. 接管（Tab 选中，根上有 ActiveSlapper）：本系统查询不匹配，完全不碰刚体，
///    IJKL/UO 操控独占控制权；刹停由 switch_slapper_control 切中瞬间执行一次
///
/// Tab 切走后 ActiveSlapper 被移除，本系统下帧自动重新匹配、恢复匀速自转
pub fn spin_display_vehicle(
    mut query: Query<
        (&mut AngularVelocity, &mut AngularDamping),
        (With<Spinning>, Without<ActiveSlapper>),
    >,
) {
    for (mut angular_velocity, mut angular_damping) in &mut query {
        // 关闭角阻尼，保持恒定转速，否则转速会被每步物理积分衰减
        angular_damping.0 = 0.0;
        // 绕世界竖直轴（Y轴）匀速自转
        angular_velocity.0 = Vec3::new(0.0, SPIN_ANGULAR_SPEED, 0.0);
    }
}
