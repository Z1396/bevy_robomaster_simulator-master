// ====================================================================
// 模块名：power_rune::visual
// 作用：能量机关的视觉表现控制
// 职责：根据当前机关模式与各目标激活状态，将外观切换指令下发到
//       StatefulAppearance，驱动场景中材质/可见性的同步刷新
// ====================================================================

use crate::all_arg_constructor;
use crate::robomaster::power_rune::common::{RUNE_TARGET_COUNT, RuneMode};
use crate::robomaster::power_rune::state::MechanismState;
use crate::robomaster::visibility::{Activation, Control, Controller, StatefulAppearance};
use bevy::prelude::Component;

// RuneVisual：单个目标面的视觉控制器集合，聚合了目标本体、装饰段、进度段等多组外观开关。
// 每组 Controller 负责在给定激活状态下切换对应场景实体的可见性/材质。
// 字段说明：
//   target           - 目标本体外观控制器（目标面本身的高亮/熄灭切换）
//   legging_segments - 三段装饰条外观控制器（机关花瓣上的装饰亮带）
//   padding_segments - 边缘填充段外观控制器
//   progress_segments- 激活进度条外观控制器（显示当前激活进度）
all_arg_constructor!(
    pub struct RuneVisual {
        target: Controller,
        legging_segments: [Controller; 3],
        padding_segments: Controller,
        progress_segments: Controller,
    }
);

impl RuneVisual {
    /// 根据机关模式与目标激活状态，将外观切换应用到指定外观上下文。
    ///
    /// # 参数
    /// - `mode`：机关模式（小/大），决定装饰条与目标面的联动逻辑
    /// - `activation`：该目标的当前激活状态
    /// - `appearance`：可变外观上下文，用于累积应用切换指令
    ///
    /// # 算法步骤
    /// 1. 小机关：目标本体与全部装饰段统一按 activation 切换
    /// 2. 大机关：已激活时目标面反转为熄灭（仅装饰段点亮），
    ///    非已激活时全部装饰段同步；已激活状态仅点亮首段装饰
    /// 3. 最后统一切换填充段与进度段
    pub fn apply(
        &mut self,
        mode: RuneMode,
        activation: Activation,
        appearance: &mut StatefulAppearance,
    ) {
        match mode {
            RuneMode::Small => {
                // 小机关：目标本体与装饰段统一切换
                self.target.set(activation, appearance);
                for swap in &mut self.legging_segments {
                    swap.set(activation, appearance);
                }
            }
            RuneMode::Large => {
                // 大机关：已激活时目标面熄灭，仅靠装饰段表示点亮
                self.target.set(
                    match activation {
                        Activation::Activated => Activation::Deactivated,
                        _ => activation,
                    },
                    appearance,
                );
                match activation {
                    // 已激活仅点亮首段装饰，形成"完成"视觉
                    Activation::Activated => self.legging_segments[0].set(activation, appearance),
                    _ => {
                        for legging in &mut self.legging_segments {
                            legging.set(activation, appearance);
                        }
                    }
                }
            }
        }

        // 填充段与进度段在两种模式下行为一致
        self.padding_segments.set(activation, appearance);
        self.progress_segments.set(activation, appearance);
    }
}

/// 能量机关整体视觉组件，包含根外观控制器与若干目标面视觉。
#[derive(Component)]
pub struct PowerRuneVisuals {
    /// 根节点外观控制器（控制机关整体框架的点亮状态）
    root: Controller,
    /// 5 个目标面的视觉控制器数组
    targets: [RuneVisual; RUNE_TARGET_COUNT],
}

impl PowerRuneVisuals {
    /// 由根控制器与目标面视觉数组构造整体视觉组件。
    pub fn new(root: Controller, targets: [RuneVisual; RUNE_TARGET_COUNT]) -> Self {
        Self { root, targets }
    }

    /// 根据机关模式与当前机制状态，刷新整体视觉。
    ///
    /// # 参数
    /// - `mode`：机关模式，传递给各目标面的 apply
    /// - `state`：当前机制状态机，提供根激活态与各目标激活态
    /// - `appearance`：可变外观上下文
    ///
    /// # 算法步骤
    /// 1. 设置根节点外观为机关当前根激活态
    /// 2. 将每个目标面与其激活状态配对，逐一应用外观切换
    pub fn apply(
        &mut self,
        mode: RuneMode,
        state: &MechanismState,
        appearance: &mut StatefulAppearance,
    ) {
        self.root.set(state.root_activation(), appearance);
        for (target, activation) in self.targets.iter_mut().zip(state.target_states()) {
            target.apply(mode, activation, appearance);
        }
    }
}
