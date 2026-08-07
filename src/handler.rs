// 引入Bevy基础依赖：资源、音频播放器、ECS观察者事件、指令系统、查询、资源只读/可变、变换组件
use bevy::{
    asset::AssetServer,
    audio::AudioPlayer,
    ecs::{
        observer::On,          // 观察者事件包装器，监听特定事件
        system::{Commands, Query, Res, ResMut},
    },
    transform::components::Transform,
};

// 内部业务模块导入
use crate::{
    // 能量机关核心事件、组件：能量机关组件、激活事件、被击中事件
    robomaster::prelude::{PowerRune, RuneActivated, RuneHit},
    // 全局子弹命中统计资源
    statistic::ProjectileStatistics,
};

/// 观察者系统：监听【能量机关成功激活事件 RuneActivated】
/// 触发时机：任意能量机关完成激活流程后执行
pub fn on_activate(
    ev: On<RuneActivated>,     // 捕获 RuneActivated 事件，ev.rune 为被激活能量机关实体ID
    mut commands: Commands,
    query: Query<&PowerRune>,   // 查询实体身上的PowerRune组件做合法性校验
    asset_server: Res<AssetServer>, // 资源服务器，加载音效文件
) {
    // 校验：该实体确实是能量机关实体，不存在则直接返回，防止脏事件
    let Ok(_rune) = query.get(ev.rune) else {
        return;
    };

    // 生成音频播放器实体，播放能量机关激活音效 rune_activated.ogg
    // AudioPlayer 自带生命周期，播放完毕实体自动销毁，无需手动管理
    commands.spawn(AudioPlayer::new(asset_server.load("rune_activated.ogg")));
}

/// 观察者系统：监听【子弹击中能量机关事件 RuneHit】
/// 触发时机：子弹碰撞到能量机关靶面时触发
pub fn on_hit(
    ev: On<RuneHit>,                       // 击中事件，携带击中结果 result、被击中机关实体 rune
    mut stats: ResMut<ProjectileStatistics>,// 全局子弹统计资源（可变，用来更新精准命中计数）
    _commands: Commands,
    query: Query<(&Transform, &PowerRune)>, // 校验被击中实体是合法能量机关
) {
    // 校验实体合法性，无效实体直接退出
    let Ok((_transform, _rune)) = query.get(ev.rune) else {
        return;
    };

    // 判断本次击中是否为精准命中（打中有效靶面、有效判定区域）
    if ev.result.accurate() {
        // 全局统计：精准命中次数 +1
        stats.increase_accurate();

        // 注释代码：精准命中时播放音效，当前暂时关闭
        //commands.spawn(AudioPlayer::new(asset_server.load("rune_activated.ogg")));
    }
}