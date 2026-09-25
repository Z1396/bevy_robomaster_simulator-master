// 引入 Bevy 引擎全部基础类型
use bevy::prelude::*;
// 哈希集合：记录已计入命中的子弹实体，用于防止同一发子弹重复计数
use std::collections::HashSet;

/// 全局资源：子弹发射命中率统计面板
/// 全局唯一实例，用来统计发射总次数、命中次数、实时命中率，用于算法调试、自瞄精度评估
#[derive(Resource, Default, Reflect)]
#[reflect(Resource)] // 实现反射，支持 Bevy 编辑器可视化查看、热重载、Inspector 检视面板查看数值
pub struct ProjectileStatistics {
    /// 子弹总共发射次数
    pub launch_count: u32,
    /// 成功命中敌方装甲的子弹数量
    pub accurate_count: u32,
    /// 已计入命中的子弹实体集合
    /// 不参与反射（HashSet 无 Reflect 实现），仅运行时去重使用
    /// 去重数据直接写资源、立即生效，不像 Commands 插组件要等指令刷新，
    /// 避免同一物理子步内一发子弹接触多块装甲时被重复计数
    #[reflect(ignore)]
    counted_projectiles: HashSet<Entity>,
}

impl ProjectileStatistics {
    /// 发射子弹时调用：发射计数 +1
    pub fn increase_launch(&mut self) {
        self.launch_count += 1;
    }

    /// 子弹命中目标装甲时调用：命中计数 +1
    pub fn increase_accurate(&mut self) {
        self.accurate_count += 1;
    }

    /// 计算当前命中率（命中数 / 总发射数）
    pub fn accurate_pct(&self) -> f32 {
        // 防止除以0崩溃：还没发射过子弹时命中率直接返回 0%
        if self.launch_count == 0 {
            return 0.0;
        }
        // 转为浮点做除法，得到 0.0 ~ 1.0 的命中率（0~100%）
        (self.accurate_count as f32) / (self.launch_count as f32)
    }

    /// 标记某颗子弹已计入命中
    /// 返回 true = 首次命中，调用方应累加 accurate_count
    /// 返回 false = 该子弹此前已计过，调用方必须跳过，防止重复计数
    pub fn mark_counted(&mut self, entity: Entity) -> bool {
        self.counted_projectiles.insert(entity)
    }

    /// 子弹销毁时调用：清理命中去重记录，防止集合随发射量无限增长
    pub fn uncount(&mut self, entity: Entity) {
        self.counted_projectiles.remove(&entity);
    }
}
