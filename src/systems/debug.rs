// 引入 Bevy 全部常用基础类型：实体生成、UI、文本、资源、键盘输入等
use bevy::prelude::*;
// 截图功能专用模块：Capturing标记正在截图、Screenshot截图组件、save_to_disk保存截图到本地
use bevy::render::view::screenshot::{Capturing, Screenshot, save_to_disk};
// 窗口相关：鼠标光标样式、系统默认光标、窗口组件
use bevy::window::{CursorIcon, SystemCursorIcon, Window};

// 项目内部自定义组件
use crate::components::{SlapperInfantry, SubscribeAutoAim};
// RM装甲相关逻辑组件
use crate::robomaster::prelude::{Armor, ArmorStickerSelection};
// 子弹发射统计全局资源
use crate::statistic::ProjectileStatistics;

/// 拼接底部状态栏的提示文本内容
/// auto_aim：是否开启自瞄
/// stats：子弹发射统计数据
fn create_help_text(auto_aim: bool, stats: &ProjectileStatistics) -> Text {
    format!(
        // 自瞄开关状态、总发射子弹数、命中装甲数量、命中率百分比
        "auto-aim={} total={} accurate={} pct={:.2}\nControls: F2-Screenshot F3-Change Camera | WASD-Move Mouse-Look Space-Shoot",
        if auto_aim { "ON " } else { "OFF" },
        stats.launch_count,    // 总发射子弹数量
        stats.accurate_count,  // 命中有效装甲数量
        stats.accurate_pct()   // 命中率 = 命中数 / 总发射数，保留2位小数
    )
    // 将字符串转为 Bevy 的 Text UI 文本对象
    .into()
}

/// 【启动时执行一次】在屏幕左下角生成UI文本控件（状态栏）
pub fn spawn_text(commands: &mut Commands) {
    commands.spawn((
        // 初始为空文本，后续由 update_help_text 实时刷新内容
        Text::new(""),
        Node {
            position_type: PositionType::Absolute, // 绝对定位，不受相机渲染影响，固定在屏幕角落
            bottom: Val::Px(12.0),                 // 距离屏幕底部 12 像素
            left: Val::Px(12.0),                  // 距离屏幕左侧 12 像素
            ..default()                            // 剩余布局属性使用默认配置
        },
    ));
}

/// 【每帧执行】实时刷新左下角状态栏文字
pub fn update_help_text(
    mut text: Query<&mut Text>,                // 查询场景里所有UI文本组件（就是上面创建的状态栏）
    auto_aim: Res<SubscribeAutoAim>,            // 全局资源：自瞄开关状态
    stats: Res<ProjectileStatistics>,           // 全局子弹统计数据
) {
    // 遍历文本控件，更新文字内容
    for mut text in text.iter_mut() {
        // load(Acquire) 原子安全读取布尔值，多线程环境防止数据竞争
        *text = create_help_text(auto_aim.load(std::sync::atomic::Ordering::Acquire), &stats);
    }
}

/// 快捷键逻辑：LeftShift + C 切换战车装甲贴纸样式
pub fn change_appearance(
    keyboard: Res<ButtonInput<KeyCode>>,                       // 键盘按键输入资源
    selections: Query<&mut ArmorStickerSelection, With<SlapperInfantry>>, // 己方战车贴纸选择器
    owned: Query<&mut Armor, With<SlapperInfantry>>,           // 己方战车装甲组件
) {
    // 判断触发条件：按住左Shift的同时，刚按下C键（just_pressed 只按下瞬间触发一次，按住不会重复触发）
    if keyboard.pressed(KeyCode::ShiftLeft) && keyboard.just_pressed(KeyCode::KeyC) {
        let mut n_type = None;
        // 步进切换贴纸编号
        for mut selection in selections {
            let new_typ = selection.advance_debug_sequence();
            n_type = Some(new_typ);
        }
        // 将新的贴纸编号赋值给装甲组件，更换装甲外观贴图
        if let Some(n_type) = n_type {
            for mut own in owned {
                own.label = n_type;
            }
        }
    }
}

/// F2 按键截图系统：按下F2截取主窗口画面保存到本地
/// Local<u32> counter：局部计数器，系统独有，不受ECS资源管理，用来给截图命名编号
pub fn screenshot_on_f2(mut commands: Commands, mut counter: Local<u32>) {
    // 拼接保存路径：screenshot-0.png、screenshot-1.png 自动递增编号
    let path = format!("./screenshot-{}.png", *counter);
    *counter += 1;

    // 生成截图实体，绑定观察者，截图完成自动保存到对应路径
    commands
        .spawn(Screenshot::primary_window())
        .observe(save_to_disk(path));
}

/// 截图状态监控系统：截图过程中鼠标变为加载转圈样式，截图结束恢复默认鼠标
pub fn screenshot_saving(
    mut commands: Commands,
    screenshot_saving: Query<Entity, With<Capturing>>, // 查询正在执行截图的实体，Capturing代表截图进行中
    window: Single<Entity, With<Window>>,               // 拿到主窗口实体
) {
    // 统计当前正在截图的实体数量
    match screenshot_saving.iter().count() {
        // 没有截图任务：移除自定义光标，恢复系统默认鼠标样式
        0 => {
            commands.entity(*window).remove::<CursorIcon>();
        }
        // 存在正在截图的任务：把鼠标光标改成「加载等待样式」
        x if x > 0 => {
            commands
                .entity(*window)
                .insert(CursorIcon::from(SystemCursorIcon::Progress));
        }
        _ => {}
    }
}