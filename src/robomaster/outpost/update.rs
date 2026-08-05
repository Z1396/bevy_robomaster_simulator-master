// ============================================================
// 模块名：outpost/update
// 作用：前哨站运行时更新
// 职责：定义前哨站的核心组件（`Outpost`、`OutpostRotator`），
//       并提供按帧驱动前哨站旋转的系统，是前哨站动态行为
//       在每帧更新阶段的入口。
// ============================================================

use crate::robomaster::outpost::rotation::{RotationController, RotationDirection};
use crate::robomaster::prelude::Team;
use bevy::app::Update;
use bevy::prelude::{Component, Query, Res, Time, Transform};
use std::hash::{Hash, Hasher};

/// 前哨站组件，标记一个实体为前哨站并记录其所属队伍。
///
/// 该组件实现了 `Hash`/`PartialEq`/`Eq`，仅以队伍作为相等性判断依据，
/// 便于在前哨站集合去重或作为键值使用。
#[derive(Component)]
pub struct Outpost {
    /// 前哨站所属队伍（红方或蓝方）。
    team: Team,
}

impl Hash for Outpost {
    /// 以队伍作为哈希依据，保证相同队伍的前哨站哈希值一致。
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.team.hash(state);
    }
}

impl PartialEq for Outpost {
    /// 仅比较队伍是否相同，用于判断两个前哨站是否属于同一队伍。
    fn eq(&self, other: &Self) -> bool {
        self.team == other.team
    }
}

impl Eq for Outpost {}

impl Outpost {
    /// 获取前哨站所属队伍。
    ///
    /// 返回值：`Team` 枚举（红方或蓝方）。
    pub fn team(&self) -> Team {
        self.team
    }

    /// 创建一个新的前哨站组件。
    ///
    /// 参数：
    /// - `team`：前哨站所属队伍。
    ///
    /// 返回值：携带指定队伍信息的 `Outpost` 实例。
    ///
    /// 可见性为 `pub(super)`，仅供前哨站子模块内部构造使用。
    pub(super) fn new(team: Team) -> Self {
        Self { team }
    }
}

/// 前哨站旋转器组件，持有旋转控制器并挂载到前哨站的旋转节点上。
///
/// 该组件在构造阶段被添加到场景树中名称以 `ROTATE` 结尾的子实体上，
/// 由旋转更新系统每帧驱动其 `Transform` 旋转。
#[derive(Component)]
pub struct OutpostRotator {
    /// 内部旋转控制器，封装速度与方向。
    rotation: RotationController,
}

impl OutpostRotator {
    /// 创建一个新的前哨站旋转器。
    ///
    /// 参数：
    /// - `direction`：旋转方向（红方顺时针，蓝方逆时针）。
    ///
    /// 返回值：使用默认旋转速度和指定方向的 `OutpostRotator` 实例。
    ///
    /// 可见性为 `pub(crate)`，供构造模块在初始化时调用。
    pub(crate) fn new(direction: RotationDirection) -> Self {
        Self {
            rotation: RotationController::new(direction),
        }
    }
}

/// 前哨站旋转更新系统。
///
/// 每帧遍历所有携带 `OutpostRotator` 组件的实体，按当前帧时间步长
/// 调用旋转控制器推进其 `Transform` 的旋转。
///
/// 参数：
/// - `time`：Bevy 时间资源，用于获取本帧时间步长。
/// - `outposts`：所有携带 `(Transform, OutpostRotator)` 的实体查询（可变借用）。
fn outpost_rotation_system(
    time: Res<Time>,
    mut outposts: Query<(&mut Transform, &OutpostRotator)>,
) {
    // 获取本帧时间步长（秒），用于计算旋转角度
    let dt = time.delta_secs();
    for (mut transform, outpost) in &mut outposts {
        // 按时间步长推进旋转控制器，更新实体的旋转
        outpost.rotation.step(&mut transform, dt);
    }
}

/// 前哨站更新插件，负责注册前哨站的每帧旋转更新系统。
#[derive(Default)]
pub(super) struct OutpostUpdatePlugin;

impl bevy::app::Plugin for OutpostUpdatePlugin {
    /// 将旋转更新系统注册到 Bevy 应用的 `Update` 调度阶段。
    fn build(&self, app: &mut bevy::app::App) {
        app.add_systems(Update, outpost_rotation_system);
    }
}
