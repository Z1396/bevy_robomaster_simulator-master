// ============================================================
// 模块名：tech_core/construct
// 作用：科技核心构造与更新模块
// 职责：定义科技核心的全部数据结构 (灯光组、颜色、阶段、灯光程序等)，
//       提供场景初始化、每帧灯光更新和调试阶段切换功能，以及
//       科技核心状态的 JSON 序列化接口。
// ============================================================

use super::consts::{
    BLUE_LIGHT_NAMES, FIRST_LIGHT_SEGMENT_COUNT, FLOW_SEGMENT_HZ, RED_LIGHT_NAMES,
};
use crate::robomaster::common::Team;
use bevy::app::{App, Update};
use bevy::color::LinearRgba;
use bevy::ecs::system::Local;
use bevy::pbr::{MeshMaterial3d, StandardMaterial};
use bevy::prelude::{
    Assets, ButtonInput, Children, Color, Commands, Component, Entity, Handle, KeyCode, Name, On,
    Plugin, Query, Res, ResMut, SceneSpawner, Time, With, info, warn,
};
use bevy::scene::SceneInstanceReady;
use serde_json::{Value, json};
use std::collections::HashMap;

/// 科技核心根组件，标记场景中的科技核心根节点。
///
/// 该组件在场景文件 (如 GLTF) 加载时由外部添加，`setup_tech_core` 系统
/// 检测到场景加载完成后会自动搜索灯光实体并完成初始化。
#[derive(Component, Debug)]
pub struct TechCoreRoot;

/// 科技核心的灯光组标识，用于区分三组环形灯光。
///
/// 第一组 (First) 为分段式流光灯带，支持分段独立控制；
/// 第二组 (Second) 和第三组 (Third) 为整体式灯环，仅支持整体颜色控制。
/// 不同比赛阶段下各组灯光显示不同的颜色、闪烁和流水效果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TechCoreLightGroup {
    /// 第一组灯光，分段式环形灯带，支持流水和组装指示效果。
    First,
    /// 第二组灯光，整体式灯环。
    Second,
    /// 第三组灯光，整体式灯环。
    Third,
}

impl TechCoreLightGroup {
    /// 所有灯光组的常量数组，按 First, Second, Third 顺序排列，便于遍历。
    pub const ALL: [Self; 3] = [Self::First, Self::Second, Self::Third];

    /// 返回灯光组的零基索引 (0, 1, 2)，用于数组下标访问。
    const fn index(self) -> usize {
        match self {
            Self::First => 0,
            Self::Second => 1,
            Self::Third => 2,
        }
    }

    /// 返回灯光组的 1-based 序号，用于 JSON 输出中的 group 字段。
    ///
    /// 返回值：First 返回 1, Second 返回 2, Third 返回 3。
    pub const fn number(self) -> u8 {
        self.index() as u8 + 1
    }
}

/// 灯光颜色枚举，定义科技核心支持的颜色种类。
///
/// - `White`: 白色灯光
/// - `Team`: 队伍颜色 (红队为红色，蓝队为蓝色)
/// - `Green`: 绿色灯光
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LightColor {
    /// 白色灯光。
    White,
    /// 队伍颜色，根据队伍不同显示红色或蓝色。
    Team,
    /// 绿色灯光。
    Green,
}

impl LightColor {
    /// 根据队伍将颜色转换为对应的 JSON 字符串标识。
    ///
    /// 参数：
    /// - `team`: 队伍信息 (红队或蓝队)，仅在 `Team` 变体时使用。
    ///
    /// 返回值：颜色字符串，如 "white", "red", "blue", "green"。
    pub const fn as_str_for_team(self, team: Team) -> &'static str {
        match self {
            Self::White => "white",
            Self::Green => "green",
            Self::Team => match team {
                Team::Red => "red",
                Team::Blue => "blue",
            },
        }
    }
}

/// 闪烁频率枚举，定义两种闪烁速率。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlinkRate {
    /// 1 Hz 闪烁 (每秒 1 次完整亮灭周期)。
    Hz1,
    /// 3 Hz 闪烁 (每秒 3 次完整亮灭周期)。
    Hz3,
}

impl BlinkRate {
    /// 返回闪烁频率的数值 (Hz)。
    ///
    /// 返回值：`Hz1` 返回 1.0, `Hz3` 返回 3.0。
    pub const fn hz(self) -> f64 {
        match self {
            Self::Hz1 => 1.0,
            Self::Hz3 => 3.0,
        }
    }
}

/// 第五步组装灯光程序枚举，描述组装进行中与完成后的灯光状态。
///
/// - `InProgress`: 组装进行中，目标段显示队伍色，能量单元段显示白色。
/// - `Completed`: 组装完成，目标段显示绿色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssemblyLightProgram {
    /// 组装进行中，目标段和能量单元段分别显示不同颜色。
    InProgress,
    /// 组装完成，目标段显示绿色。
    Completed,
}

/// 灯光程序枚举，定义科技核心灯光的全部显示模式。
///
/// - `Off`: 关闭，所有灯光熄灭。
/// - `Solid(LightColor)`: 常亮指定颜色。
/// - `Blink { color, rate }`: 按指定频率闪烁，亮半周期显示颜色，暗半周期熄灭。
/// - `Flow { color }`: 流水灯效果，灯光沿环形灯带依次点亮再折返。
/// - `Assembly(AssemblyLightProgram)`: 第五步组装专用灯光程序，分段控制目标段和能量单元段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LightProgram {
    /// 灯光关闭。
    Off,
    /// 常亮指定颜色。
    Solid(LightColor),
    /// 按指定频率闪烁。
    Blink { color: LightColor, rate: BlinkRate },
    /// 流水灯效果。
    Flow { color: LightColor },
    /// 第五步组装专用灯光程序。
    Assembly(AssemblyLightProgram),
}

impl LightProgram {
    /// 返回当前灯光程序在给定时刻的活跃颜色。
    ///
    /// 对于 `Off` 返回 `None`；对于 `Solid` 返回固定颜色；
    /// 对于 `Blink` 根据时间计算闪烁相位，亮半周期返回颜色，暗半周期返回 `None`；
    /// 对于 `Flow` 返回流水颜色；对于 `Assembly(InProgress)` 返回 `None` (分段控制而非整体颜色)；
    /// 对于 `Assembly(Completed)` 返回绿色。
    ///
    /// 参数：
    /// - `elapsed_secs`: 当前阶段已过去的秒数。
    ///
    /// 返回值：当前活跃的颜色，如果灯光熄灭则返回 `None`。
    pub fn active_color(self, elapsed_secs: f64) -> Option<LightColor> {
        match self {
            Self::Off => None,
            Self::Solid(color) => Some(color),
            Self::Blink { color, rate } => {
                // 亮半周期返回颜色，暗半周期返回 None
                ((elapsed_secs * rate.hz()).fract() < 0.5).then_some(color)
            }
            Self::Flow { color } => Some(color),
            Self::Assembly(AssemblyLightProgram::InProgress) => None,
            Self::Assembly(AssemblyLightProgram::Completed) => Some(LightColor::Green),
        }
    }

    /// 将灯光程序序列化为 JSON 值，用于状态上报。
    ///
    /// 参数：
    /// - `team`: 队伍信息，用于将 `Team` 颜色解析为具体颜色字符串。
    ///
    /// 返回值：包含模式、颜色、频率等字段的 JSON 值。
    fn json_value(self, team: Team) -> Value {
        match self {
            Self::Off => json!({ "mode": "off" }),
            Self::Solid(color) => json!({
                "mode": "solid",
                "color": color.as_str_for_team(team),
            }),
            Self::Blink { color, rate } => json!({
                "mode": "blink",
                "color": color.as_str_for_team(team),
                "hz": rate.hz(),
            }),
            Self::Flow { color } => json!({
                "mode": "flow",
                "color": color.as_str_for_team(team),
                "segment_hz": FLOW_SEGMENT_HZ,
            }),
            Self::Assembly(AssemblyLightProgram::InProgress) => json!({
                "mode": "step5_in_progress",
                "target_color": LightColor::Team.as_str_for_team(team),
                "energy_unit_color": LightColor::White.as_str_for_team(team),
            }),
            Self::Assembly(AssemblyLightProgram::Completed) => json!({
                "mode": "step5_completed",
                "target_color": LightColor::Green.as_str_for_team(team),
            }),
        }
    }
}

/// 第一组灯光的灯段索引，表示环形灯带上的一个具体分段。
///
/// 灯段编号从 1 到 `FIRST_LIGHT_SEGMENT_COUNT`，支持通过角度或序号创建。
/// 用于流水灯效果和第五步组装灯光中指定目标段与能量单元段。
/// 内部存储为零基索引 (0-based)，对外提供 1-based 编号。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TechCoreFirstLightSegment(usize);

impl TechCoreFirstLightSegment {
    /// 最小有效段号，值为 1。
    pub const MIN_NUMBER: usize = 1;
    /// 最大有效段号，值为 `FIRST_LIGHT_SEGMENT_COUNT`。
    pub const MAX_NUMBER: usize = FIRST_LIGHT_SEGMENT_COUNT;

    /// 从零基索引创建灯段。
    ///
    /// 参数：
    /// - `index`: 零基索引，范围为 `0..FIRST_LIGHT_SEGMENT_COUNT`。
    ///
    /// 返回值：如果索引有效则返回 `Some(Self)`，否则返回 `None`。
    pub const fn from_zero_based(index: usize) -> Option<Self> {
        if index < FIRST_LIGHT_SEGMENT_COUNT {
            Some(Self(index))
        } else {
            None
        }
    }

    /// 从 1-based 编号创建灯段。
    ///
    /// 参数：
    /// - `number`: 1-based 编号，范围为 `MIN_NUMBER..=MAX_NUMBER`。
    ///
    /// 返回值：如果编号有效则返回 `Some(Self)`，否则返回 `None`。
    pub const fn from_number(number: usize) -> Option<Self> {
        if number >= Self::MIN_NUMBER && number <= Self::MAX_NUMBER {
            Some(Self(number - 1))
        } else {
            None
        }
    }

    /// 从弧度角计算对应的灯段。
    ///
    /// 将角度归一化到 `[0, 2π)` 范围，然后按比例映射到灯段索引。
    /// 映射关系：角度 0 对应段 1，角度随角度增加递增。
    ///
    /// 参数：
    /// - `radians`: 弧度角度值。
    ///
    /// 返回值：对应的灯段实例。
    pub fn from_angle_radians(radians: f64) -> Self {
        // 将角度归一化到 [0, 2π) 范围，避免负角度或超过一圈的角度
        let normalized = radians.rem_euclid(std::f64::consts::TAU);
        // 按比例映射到灯段索引，每个段对应 2π / FIRST_LIGHT_SEGMENT_COUNT 弧度
        let index = (normalized / std::f64::consts::TAU * FIRST_LIGHT_SEGMENT_COUNT as f64).floor()
            as usize;
        // 防止浮点误差导致索引越界
        Self(index.min(FIRST_LIGHT_SEGMENT_COUNT - 1))
    }

    /// 从角度计算对应的灯段 (角度制)。
    ///
    /// 参数：
    /// - `degrees`: 角度值 (0-360)。
    ///
    /// 返回值：对应的灯段实例。
    pub fn from_angle_degrees(degrees: f64) -> Self {
        Self::from_angle_radians(degrees.to_radians())
    }

    /// 返回内部存储的零基索引。
    const fn index(self) -> usize {
        self.0
    }

    /// 返回灯段的 1-based 编号。
    ///
    /// 返回值：`1` 到 `FIRST_LIGHT_SEGMENT_COUNT` 之间的值。
    pub const fn number(self) -> usize {
        self.0 + 1
    }
}

/// 第五步组装中目标段与能量单元段的配置。
///
/// 该结构体指定灯光的目标段 (target) 和能量单元段 (energy_unit)，
/// 用于控制组装过程中不同灯段的颜色显示。
/// 目标段在组装进行中显示队伍色，完成后显示绿色；
/// 能量单元段在组装进行中显示白色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TechCoreStep5Lights {
    /// 目标灯段，组装完成后显示绿色。
    target: TechCoreFirstLightSegment,
    /// 能量单元灯段，组装进行中显示白色。
    energy_unit: TechCoreFirstLightSegment,
}

impl TechCoreStep5Lights {
    /// 创建新的第五步灯光配置。
    ///
    /// 参数：
    /// - `target`: 目标灯段，组装完成后显示绿色。
    /// - `energy_unit`: 能量单元灯段，组装进行中显示白色。
    ///
    /// 返回值：新的 `TechCoreStep5Lights` 实例。
    pub const fn new(
        target: TechCoreFirstLightSegment,
        energy_unit: TechCoreFirstLightSegment,
    ) -> Self {
        Self {
            target,
            energy_unit,
        }
    }

    /// 返回目标灯段。
    pub const fn target(self) -> TechCoreFirstLightSegment {
        self.target
    }

    /// 返回能量单元灯段。
    pub const fn energy_unit(self) -> TechCoreFirstLightSegment {
        self.energy_unit
    }
}

// 默认第五步灯光配置：目标段为第 1 段，能量单元段为中间段。
impl Default for TechCoreStep5Lights {
    fn default() -> Self {
        Self {
            target: TechCoreFirstLightSegment::from_zero_based(0).unwrap(),
            energy_unit: TechCoreFirstLightSegment::from_zero_based(FIRST_LIGHT_SEGMENT_COUNT / 2)
                .unwrap(),
        }
    }
}

// 流水灯激活状态：指定单个段索引或所有段全亮。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum FlowActivation {
    Segment(usize),
    All,
}

fn flow_activation(elapsed_secs: f64) -> FlowActivation {
    // 确保时间非负，避免负时间导致的计算错误
    let elapsed_secs = elapsed_secs.max(0.0);
    // 计算当前所处的流水灯步进索引
    let step = (elapsed_secs * FLOW_SEGMENT_HZ).floor() as usize;
    let forward_len = FIRST_LIGHT_SEGMENT_COUNT;
    // 完整往返 (forward + backward) 的步数
    let round_trip_len = forward_len * 2;

    if step < forward_len {
        // 正向阶段：从第 0 段向第 N-1 段依次点亮
        FlowActivation::Segment(step)
    } else if step < round_trip_len {
        // 反向阶段：从第 N-1 段向第 0 段依次熄灭 (折返效果)
        FlowActivation::Segment(round_trip_len - 1 - step)
    } else {
        // 往返完成后所有段全亮
        FlowActivation::All
    }
}

// 生成流水灯当前活跃段的 JSON 描述，包含左右两侧的段索引。
fn flow_active_segments_json(elapsed_secs: f64) -> Value {
    fn segment_json(side: &'static str, index: usize) -> Value {
        json!({
            "side": side,
            "index": index + 1,
        })
    }

    match flow_activation(elapsed_secs) {
        FlowActivation::Segment(index) => {
            // 单个段激活时，左右两侧对应索引同时亮起
            json!([segment_json("left", index), segment_json("right", index),])
        }
        FlowActivation::All => Value::Array(
            // 所有段激活时，生成左右两侧所有段的完整列表
            (0..FIRST_LIGHT_SEGMENT_COUNT)
                .flat_map(|index| [segment_json("left", index), segment_json("right", index)])
                .collect(),
        ),
    }
}

// 生成单个灯段左右两侧的 JSON 描述对，包含颜色和角色信息。
// 用于第五步组装灯光中描述目标段和能量单元段的详细状态。
fn segment_pair_json(
    segment: TechCoreFirstLightSegment,
    color: &'static str,
    role: &'static str,
) -> [Value; 2] {
    [
        json!({
            "side": "left",
            "index": segment.number(),
            "color": color,
            "role": role,
        }),
        json!({
            "side": "right",
            "index": segment.number(),
            "color": color,
            "role": role,
        }),
    ]
}

// 生成第五步组装阶段活跃段的 JSON 描述。
// 组装进行中时，目标段显示队伍色，能量单元段显示白色 (如果与目标段不同)；
// 组装完成时，目标段显示绿色。
fn step5_active_segments_json(
    team: Team,
    assembly: AssemblyLightProgram,
    step5_lights: TechCoreStep5Lights,
) -> Value {
    let mut segments = Vec::with_capacity(4);
    let target = step5_lights.target();

    match assembly {
        AssemblyLightProgram::InProgress => {
            // 目标段显示队伍色，标识需要组装到的位置
            segments.extend(segment_pair_json(
                target,
                LightColor::Team.as_str_for_team(team),
                "target",
            ));

            // 能量单元段显示白色，标识当前能量单元位置
            let energy_unit = step5_lights.energy_unit();
            if energy_unit != target {
                segments.extend(segment_pair_json(
                    energy_unit,
                    LightColor::White.as_str_for_team(team),
                    "energy_unit",
                ));
            }
        }
        AssemblyLightProgram::Completed => {
            // 组装完成后，目标段显示绿色
            segments.extend(segment_pair_json(
                target,
                LightColor::Green.as_str_for_team(team),
                "target",
            ));
        }
    }

    Value::Array(segments)
}

/// 科技核心阶段枚举，定义比赛过程中科技核心经历的全部状态。
///
/// 每个阶段对应一组灯光程序，视觉上通过三组灯光的不同颜色、闪烁和流水效果
/// 来反映当前所处的比赛阶段。阶段按顺序推进：
/// MatchRunningIdle -> DifficultySelectedArmNotReady -> DifficultySelectedArmReady ->
/// Step2Completed -> Step3Completed -> Step4Completed -> Step5InProgress ->
/// Step5Completed -> ConfirmedRecovering。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TechCorePhase {
    /// 比赛运行中，空闲状态。第一组关闭，第二、三组常亮队伍色。
    MatchRunningIdle,
    /// 难度已选择，机械臂未就绪。第一组白色流水，第二、三组常亮队伍色。
    DifficultySelectedArmNotReady,
    /// 难度已选择，机械臂已就绪。三组均以 1Hz 闪烁。
    DifficultySelectedArmReady,
    /// 第二步完成。第一组白色 1Hz 闪烁，第二组队伍色 1Hz 闪烁，第三组队伍色 3Hz 闪烁。
    Step2Completed,
    /// 第三步完成。第一组白色 1Hz 闪烁，第二、三组队伍色 3Hz 闪烁。
    Step3Completed,
    /// 第四步完成。第一组白色 1Hz 闪烁，第二、三组常亮队伍色。
    Step4Completed,
    /// 第五步进行中。第一组组装指示灯光，第二、三组常亮队伍色。
    Step5InProgress,
    /// 第五步完成。第一组组装完成灯光，第二、三组常亮队伍色。
    Step5Completed,
    /// 确认恢复中。三组均以 3Hz 闪烁。
    ConfirmedRecovering,
}

impl TechCorePhase {
    /// 调试用阶段顺序列表，按比赛流程的完整推进顺序排列，共 9 个阶段。
    ///
    /// 用于 `next_debug` 方法实现阶段循环切换。
    pub const DEBUG_SEQUENCE: [Self; 9] = [
        Self::MatchRunningIdle,
        Self::DifficultySelectedArmNotReady,
        Self::DifficultySelectedArmReady,
        Self::Step2Completed,
        Self::Step3Completed,
        Self::Step4Completed,
        Self::Step5InProgress,
        Self::Step5Completed,
        Self::ConfirmedRecovering,
    ];

    /// 返回当前阶段对应的三组灯光程序。
    ///
    /// 返回值：长度为 3 的数组，依次对应 First, Second, Third 灯光组的程序。
    pub const fn programs(self) -> [LightProgram; 3] {
        use AssemblyLightProgram::{Completed, InProgress};
        use BlinkRate::{Hz1, Hz3};
        use LightColor::{Team, White};
        use LightProgram::{Assembly, Blink, Flow, Off, Solid};

        match self {
            Self::MatchRunningIdle => [Off, Solid(Team), Solid(Team)],
            Self::DifficultySelectedArmNotReady => {
                [Flow { color: White }, Solid(Team), Solid(Team)]
            }
            Self::DifficultySelectedArmReady => [
                Blink {
                    color: White,
                    rate: Hz1,
                },
                Blink {
                    color: Team,
                    rate: Hz1,
                },
                Blink {
                    color: Team,
                    rate: Hz1,
                },
            ],
            Self::Step2Completed => [
                Blink {
                    color: White,
                    rate: Hz1,
                },
                Blink {
                    color: Team,
                    rate: Hz1,
                },
                Blink {
                    color: Team,
                    rate: Hz3,
                },
            ],
            Self::Step3Completed => [
                Blink {
                    color: White,
                    rate: Hz1,
                },
                Blink {
                    color: Team,
                    rate: Hz3,
                },
                Blink {
                    color: Team,
                    rate: Hz3,
                },
            ],
            Self::Step4Completed => [
                Blink {
                    color: White,
                    rate: Hz1,
                },
                Solid(Team),
                Solid(Team),
            ],
            Self::Step5InProgress => [Assembly(InProgress), Solid(Team), Solid(Team)],
            Self::Step5Completed => [Assembly(Completed), Solid(Team), Solid(Team)],
            Self::ConfirmedRecovering => [
                Blink {
                    color: White,
                    rate: Hz3,
                },
                Blink {
                    color: Team,
                    rate: Hz3,
                },
                Blink {
                    color: Team,
                    rate: Hz3,
                },
            ],
        }
    }

    /// 返回当前阶段的数字标识 (0-based)，用于 JSON 输出中的 id 字段。
    ///
    /// 返回值：0 到 8 之间的整数，按比赛流程顺序递增。
    pub const fn id(self) -> u8 {
        match self {
            Self::MatchRunningIdle => 0,
            Self::DifficultySelectedArmNotReady => 1,
            Self::DifficultySelectedArmReady => 2,
            Self::Step2Completed => 3,
            Self::Step3Completed => 4,
            Self::Step4Completed => 5,
            Self::Step5InProgress => 6,
            Self::Step5Completed => 7,
            Self::ConfirmedRecovering => 8,
        }
    }

    /// 返回当前阶段的字符串标识，用于 JSON 输出中的 name 字段。
    ///
    /// 返回值：如 "match_running_idle", "step2_completed" 等。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MatchRunningIdle => "match_running_idle",
            Self::DifficultySelectedArmNotReady => "difficulty_selected_arm_not_ready",
            Self::DifficultySelectedArmReady => "difficulty_selected_arm_ready",
            Self::Step2Completed => "step2_completed",
            Self::Step3Completed => "step3_completed",
            Self::Step4Completed => "step4_completed",
            Self::Step5InProgress => "step5_in_progress",
            Self::Step5Completed => "step5_completed",
            Self::ConfirmedRecovering => "confirmed_recovering",
        }
    }

    /// 切换到调试序列中的下一个阶段 (循环)。
    ///
    /// 按照 `DEBUG_SEQUENCE` 定义的顺序切换到下一个阶段，到达末尾后回到开头。
    ///
    /// 返回值：下一个阶段的 `TechCorePhase` 实例。
    pub fn next_debug(self) -> Self {
        // 查找当前阶段在 DEBUG_SEQUENCE 中的位置
        let index = Self::DEBUG_SEQUENCE
            .iter()
            .position(|phase| *phase == self)
            .unwrap_or(0);
        // 切换到下一个阶段，到达末尾后循环回到开头
        Self::DEBUG_SEQUENCE[(index + 1) % Self::DEBUG_SEQUENCE.len()]
    }
}

// 将 Team 枚举转换为 JSON 字符串标识。
fn team_name(team: Team) -> &'static str {
    match team {
        Team::Red => "red",
        Team::Blue => "blue",
    }
}

// 生成单个灯光组的完整 JSON 描述，包含队伍、组号、程序和活跃颜色等信息。
// 对于第一组灯光，额外包含活跃段信息 (流水灯或组装指示)。
fn light_json_value(
    phase: TechCorePhase,
    team: Team,
    group: TechCoreLightGroup,
    elapsed_secs: f64,
    step5_lights: TechCoreStep5Lights,
) -> Value {
    let program = phase.programs()[group.index()];
    // 解析活跃颜色：组装进行中为 "mixed"，组装完成取绿色，其他情况由程序计算
    let active_color = match program {
        LightProgram::Assembly(AssemblyLightProgram::InProgress) => "mixed",
        LightProgram::Assembly(AssemblyLightProgram::Completed) => {
            LightColor::Green.as_str_for_team(team)
        }
        _ => program
            .active_color(elapsed_secs)
            .map(|color| color.as_str_for_team(team))
            .unwrap_or("off"),
    };

    let mut value = json!({
        "team": team_name(team),
        "group": group.number(),
        "program": program.json_value(team),
        "active_color": active_color,
    });

    // 第一组流水灯模式：添加活跃段信息
    if matches!(program, LightProgram::Flow { .. }) && group == TechCoreLightGroup::First {
        if let Some(object) = value.as_object_mut() {
            object.insert(
                "active_segments".to_string(),
                flow_active_segments_json(elapsed_secs),
            );
        }
    }

    // 第一组组装模式：添加目标段和能量单元段的详细信息
    if let LightProgram::Assembly(assembly) = program {
        if group == TechCoreLightGroup::First {
            if let Some(object) = value.as_object_mut() {
                object.insert(
                    "active_segments".to_string(),
                    step5_active_segments_json(team, assembly, step5_lights),
                );
            }
        }
    }

    value
}

// 生成单个科技核心的完整阶段 JSON 描述，包含阶段标识和红蓝双方所有灯光组的信息。
fn tech_core_phase_json_value(
    phase: TechCorePhase,
    elapsed_secs: f64,
    step5_lights: TechCoreStep5Lights,
) -> Value {
    // 预分配容量：红蓝两队 x 三组灯光 = 6 个灯光组
    let mut lights = Vec::with_capacity(6);
    for team in [Team::Red, Team::Blue] {
        for group in TechCoreLightGroup::ALL {
            lights.push(light_json_value(
                phase,
                team,
                group,
                elapsed_secs,
                step5_lights,
            ));
        }
    }

    json!({
        "phase": {
            "id": phase.id(),
            "name": phase.as_str(),
        },
        "lights": lights,
    })
}

/// 从阶段迭代器生成科技核心状态 JSON 字符串。
///
/// 将每个阶段解析为包含阶段标识、时间戳和所有灯光组详细信息的 JSON 对象。
/// 灯光组包括红蓝两队的 First, Second, Third 三组灯光，每个灯光的程序、
/// 活跃颜色和活跃段等信息均被序列化。此函数适用于不依赖 `TechCore` 组件
/// 的状态上报场景 (如模拟或测试)。
///
/// 参数：
/// - `stamp_sec`: 时间戳的秒部分。
/// - `stamp_nanosec`: 时间戳的纳秒部分。
/// - `elapsed_secs`: 当前已过去的秒数，用于计算闪烁和流水灯相位。
/// - `phases`: 科技核心阶段迭代器，每个元素对应一个核心的阶段。
///
/// 返回值：格式化后的 JSON 字符串。
pub fn tech_core_state_json_from_phases<I>(
    stamp_sec: i32,
    stamp_nanosec: u32,
    elapsed_secs: f64,
    phases: I,
) -> String
where
    I: IntoIterator<Item = TechCorePhase>,
{
    // 将每个阶段解析为完整的 JSON 阶段描述
    let cores = phases
        .into_iter()
        .map(|phase| {
            tech_core_phase_json_value(phase, elapsed_secs, TechCoreStep5Lights::default())
        })
        .collect::<Vec<_>>();

    json!({
        "stamp": {
            "sec": stamp_sec,
            "nanosec": stamp_nanosec,
        },
        "cores": cores,
    })
    .to_string()
}

/// 从 `TechCore` 组件迭代器生成科技核心状态 JSON 字符串。
///
/// 与 `tech_core_state_json_from_phases` 类似，但直接从 `TechCore` 组件
/// 读取阶段信息、阶段已用时间和第五步灯光配置，适用于运行时状态上报。
/// 每个核心的 `phase_elapsed_secs` 用于计算阶段内已用时间，确保闪烁和流水灯
/// 的相位在阶段切换时正确重置。
///
/// 参数：
/// - `stamp_sec`: 时间戳的秒部分。
/// - `stamp_nanosec`: 时间戳的纳秒部分。
/// - `elapsed_secs`: 当前已过去的秒数，用于计算阶段内已用时间。
/// - `cores`: `TechCore` 组件引用迭代器。
///
/// 返回值：格式化后的 JSON 字符串。
pub fn tech_core_state_json<'a, I>(
    stamp_sec: i32,
    stamp_nanosec: u32,
    elapsed_secs: f64,
    cores: I,
) -> String
where
    I: IntoIterator<Item = &'a TechCore>,
{
    // 从每个 TechCore 组件读取阶段、阶段已用时间和第五步灯光配置
    let cores = cores
        .into_iter()
        .map(|core| {
            tech_core_phase_json_value(
                core.phase(),
                core.phase_elapsed_secs(elapsed_secs),
                core.step5_lights(),
            )
        })
        .collect::<Vec<_>>();

    json!({
        "stamp": {
            "sec": stamp_sec,
            "nanosec": stamp_nanosec,
        },
        "cores": cores,
    })
    .to_string()
}

// 第一组灯光的实体引用集合，包含整体式灯环和左右两侧的分段灯带。
// 如果场景中存在分段式灯带 (left/right)，则优先使用分段控制；
// 否则回退到整体式灯环 (whole) 进行整体颜色控制。
#[derive(Debug, Clone, Copy)]
struct FirstLightSet {
    // 整体式灯环实体，当没有分段灯带时使用。
    whole: Option<Entity>,
    // 左侧分段灯带，每个元素对应一个灯段实体。
    left: [Option<Entity>; FIRST_LIGHT_SEGMENT_COUNT],
    // 右侧分段灯带，每个元素对应一个灯段实体。
    right: [Option<Entity>; FIRST_LIGHT_SEGMENT_COUNT],
}

impl FirstLightSet {
    // 创建新的 FirstLightSet 实例。
    fn new(
        whole: Option<Entity>,
        left: [Option<Entity>; FIRST_LIGHT_SEGMENT_COUNT],
        right: [Option<Entity>; FIRST_LIGHT_SEGMENT_COUNT],
    ) -> Self {
        Self { whole, left, right }
    }

    // 检查是否存在任何分段灯带实体。
    fn has_segments(&self) -> bool {
        self.left
            .iter()
            .chain(self.right.iter())
            .any(Option::is_some)
    }

    // 返回缺失的分段灯带名称列表，用于日志警告。
    fn missing_segments(&self, prefix: &str) -> Vec<String> {
        let mut missing = Vec::new();

        for (side, segments) in [("L", &self.left), ("R", &self.right)] {
            for (index, entity) in segments.iter().enumerate() {
                if entity.is_none() {
                    missing.push(format!("{prefix}_{side}_{}", index + 1));
                }
            }
        }

        missing
    }

    // 返回所有存在的分段灯带实体的迭代器。
    fn segment_entities(&self) -> impl Iterator<Item = Entity> + '_ {
        self.left
            .iter()
            .chain(self.right.iter())
            .filter_map(|entity| *entity)
    }

    // 将所有灯光实体 (分段或整体) 设置为指定材质。
    // 优先使用分段灯带，如果没有分段则使用整体式灯环。
    fn assign_all(
        &self,
        handle: Handle<StandardMaterial>,
        children: &Query<&Children>,
        mesh_materials: &mut Query<&mut MeshMaterial3d<StandardMaterial>>,
    ) {
        if self.has_segments() {
            // 遍历所有分段灯带实体，逐个设置材质
            for entity in self.segment_entities() {
                assign_material(entity, handle.clone(), children, mesh_materials);
            }
        } else if let Some(entity) = self.whole {
            // 没有分段灯带时，使用整体式灯环
            assign_material(entity, handle, children, mesh_materials);
        }
    }

    // 应用流水灯效果：先关闭所有段，再根据流水灯激活状态点亮对应段。
    fn assign_flow(
        &self,
        team: Team,
        color: LightColor,
        elapsed_secs: f64,
        handles: &TechCoreMaterialHandles,
        children: &Query<&Children>,
        mesh_materials: &mut Query<&mut MeshMaterial3d<StandardMaterial>>,
    ) {
        // 如果没有分段灯带，使用整体颜色 (回退模式)
        if !self.has_segments() {
            self.assign_all(handles.resolve_color(team, color), children, mesh_materials);
            return;
        }

        // 先关闭所有段
        self.assign_all(handles.off.clone(), children, mesh_materials);
        let active_handle = handles.resolve_color(team, color);

        match flow_activation(elapsed_secs) {
            FlowActivation::Segment(index) => {
                // 仅点亮当前活跃的左右对应段
                for entity in [self.left[index], self.right[index]].into_iter().flatten() {
                    assign_material(entity, active_handle.clone(), children, mesh_materials);
                }
            }
            FlowActivation::All => {
                // 所有段全亮
                self.assign_all(active_handle, children, mesh_materials);
            }
        }
    }

    // 为指定灯段的左右两侧同时设置材质。
    fn assign_segment_pair(
        &self,
        segment: TechCoreFirstLightSegment,
        handle: Handle<StandardMaterial>,
        children: &Query<&Children>,
        mesh_materials: &mut Query<&mut MeshMaterial3d<StandardMaterial>>,
    ) {
        let index = segment.index();
        for entity in [self.left[index], self.right[index]].into_iter().flatten() {
            assign_material(entity, handle.clone(), children, mesh_materials);
        }
    }

    // 应用第五步组装灯光效果。
    // 组装进行中：目标段显示队伍色，能量单元段显示白色。
    // 组装完成：目标段显示绿色。
    fn assign_assembly(
        &self,
        team: Team,
        assembly: AssemblyLightProgram,
        step5_lights: TechCoreStep5Lights,
        handles: &TechCoreMaterialHandles,
        children: &Query<&Children>,
        mesh_materials: &mut Query<&mut MeshMaterial3d<StandardMaterial>>,
    ) {
        // 如果没有分段灯带，使用整体颜色回退
        if !self.has_segments() {
            let fallback_color = match assembly {
                AssemblyLightProgram::InProgress => LightColor::Team,
                AssemblyLightProgram::Completed => LightColor::Green,
            };
            self.assign_all(
                handles.resolve_color(team, fallback_color),
                children,
                mesh_materials,
            );
            return;
        }

        // 先关闭所有段，然后按需点亮目标段和能量单元段
        self.assign_all(handles.off.clone(), children, mesh_materials);

        match assembly {
            AssemblyLightProgram::InProgress => {
                let target = step5_lights.target();
                let energy_unit = step5_lights.energy_unit();

                // 能量单元段显示白色 (如果与目标段不同)
                if energy_unit != target {
                    self.assign_segment_pair(
                        energy_unit,
                        handles.resolve_color(team, LightColor::White),
                        children,
                        mesh_materials,
                    );
                }

                // 目标段显示队伍色
                self.assign_segment_pair(
                    target,
                    handles.resolve_color(team, LightColor::Team),
                    children,
                    mesh_materials,
                );
            }
            AssemblyLightProgram::Completed => {
                // 组装完成后，目标段显示绿色
                self.assign_segment_pair(
                    step5_lights.target(),
                    handles.resolve_color(team, LightColor::Green),
                    children,
                    mesh_materials,
                );
            }
        }
    }

    // 根据灯光程序类型分发到对应的材质分配方法。
    fn assign_program(
        &self,
        team: Team,
        program: LightProgram,
        elapsed_secs: f64,
        step5_lights: TechCoreStep5Lights,
        handles: &TechCoreMaterialHandles,
        children: &Query<&Children>,
        mesh_materials: &mut Query<&mut MeshMaterial3d<StandardMaterial>>,
    ) {
        match program {
            LightProgram::Flow { color } => {
                self.assign_flow(team, color, elapsed_secs, handles, children, mesh_materials);
            }
            LightProgram::Assembly(assembly) => {
                self.assign_assembly(
                    team,
                    assembly,
                    step5_lights,
                    handles,
                    children,
                    mesh_materials,
                );
            }
            _ => {
                // Off, Solid, Blink 等模式：统一通过 resolve 解析颜色并设置
                let handle = handles.resolve(team, program, elapsed_secs);
                self.assign_all(handle, children, mesh_materials);
            }
        }
    }
}

// 单方队伍的三组灯光实体引用集合。
#[derive(Debug, Clone, Copy)]
struct TeamCoreLights {
    // 所属队伍 (红方或蓝方)。
    team: Team,
    // 第一组灯光 (分段式流光灯带) 的实体引用。
    first: FirstLightSet,
    // 第二组灯光 (整体式灯环) 的实体句柄。
    second: Entity,
    // 第三组灯光 (整体式灯环) 的实体句柄。
    third: Entity,
}

impl TeamCoreLights {
    fn new(team: Team, first: FirstLightSet, second: Entity, third: Entity) -> Self {
        Self {
            team,
            first,
            second,
            third,
        }
    }
}

/// 科技核心组件，管理科技核心的灯光状态和阶段切换。
///
/// 该组件存储红蓝双方的灯光实体引用、当前阶段、阶段开始时间以及
/// 第五步组装的灯光配置。每帧由 `update_tech_core_lights` 系统
/// 根据当前阶段和已用时间更新灯光材质。外部系统可通过 `set_phase`
/// 等方法控制比赛阶段推进。
#[derive(Component, Debug)]
pub struct TechCore {
    /// 当前阶段。
    phase: TechCorePhase,
    /// 上一次渲染的阶段，用于检测阶段变化以重置阶段计时器。
    last_rendered_phase: TechCorePhase,
    /// 当前阶段开始的时间戳 (秒)，用于计算阶段内已用时间。
    phase_started_at_secs: f64,
    /// 第五步组装的灯光配置 (目标段和能量单元段)。
    step5_lights: TechCoreStep5Lights,
    /// 红方灯光组引用。
    red: TeamCoreLights,
    /// 蓝方灯光组引用。
    blue: TeamCoreLights,
}

impl TechCore {
    /// 创建新的科技核心组件，初始阶段为 `MatchRunningIdle`。
    ///
    /// 参数：
    /// - `red`: 红方灯光组引用。
    /// - `blue`: 蓝方灯光组引用。
    ///
    /// 返回值：初始化后的 `TechCore` 实例。
    fn new(red: TeamCoreLights, blue: TeamCoreLights) -> Self {
        Self {
            phase: TechCorePhase::MatchRunningIdle,
            last_rendered_phase: TechCorePhase::MatchRunningIdle,
            phase_started_at_secs: 0.0,
            step5_lights: TechCoreStep5Lights::default(),
            red,
            blue,
        }
    }

    /// 返回当前阶段。
    pub fn phase(&self) -> TechCorePhase {
        self.phase
    }

    /// 设置当前阶段。
    ///
    /// 参数：
    /// - `phase`: 要设置的新阶段。灯光更新系统会在下一帧根据新阶段更新灯光效果。
    pub fn set_phase(&mut self, phase: TechCorePhase) {
        self.phase = phase;
    }

    /// 返回当前第五步组装的灯光配置。
    ///
    /// 返回值：目标段和能量单元段的配置。
    pub const fn step5_lights(&self) -> TechCoreStep5Lights {
        self.step5_lights
    }

    /// 设置第五步组装的灯光配置。
    ///
    /// 参数：
    /// - `step5_lights`: 新的灯光配置，包含目标段和能量单元段。
    pub fn set_step5_lights(&mut self, step5_lights: TechCoreStep5Lights) {
        self.step5_lights = step5_lights;
    }

    /// 设置第五步组装的目标灯段。
    ///
    /// 参数：
    /// - `segment`: 目标灯段，组装完成后显示绿色。
    pub fn set_step5_target_segment(&mut self, segment: TechCoreFirstLightSegment) {
        self.step5_lights.target = segment;
    }

    /// 设置第五步组装的能量单元灯段。
    ///
    /// 参数：
    /// - `segment`: 能量单元灯段，组装进行中显示白色。
    pub fn set_step5_energy_unit_segment(&mut self, segment: TechCoreFirstLightSegment) {
        self.step5_lights.energy_unit = segment;
    }

    /// 通过弧度角设置第五步组装的目标灯段。
    ///
    /// 参数：
    /// - `radians`: 弧度角度值，将自动映射到最近的灯段。
    pub fn set_step5_target_angle_radians(&mut self, radians: f64) {
        self.set_step5_target_segment(TechCoreFirstLightSegment::from_angle_radians(radians));
    }

    /// 通过弧度角设置第五步组装的能量单元灯段。
    ///
    /// 参数：
    /// - `radians`: 弧度角度值，将自动映射到最近的灯段。
    pub fn set_step5_energy_unit_angle_radians(&mut self, radians: f64) {
        self.set_step5_energy_unit_segment(TechCoreFirstLightSegment::from_angle_radians(radians));
    }

    /// 通过角度 (角度制) 设置第五步组装的目标灯段。
    ///
    /// 参数：
    /// - `degrees`: 角度值 (0-360)，将自动映射到最近的灯段。
    pub fn set_step5_target_angle_degrees(&mut self, degrees: f64) {
        self.set_step5_target_segment(TechCoreFirstLightSegment::from_angle_degrees(degrees));
    }

    /// 通过角度 (角度制) 设置第五步组装的能量单元灯段。
    ///
    /// 参数：
    /// - `degrees`: 角度值 (0-360)，将自动映射到最近的灯段。
    pub fn set_step5_energy_unit_angle_degrees(&mut self, degrees: f64) {
        self.set_step5_energy_unit_segment(TechCoreFirstLightSegment::from_angle_degrees(degrees));
    }

    /// 将当前阶段推进到调试序列中的下一个阶段。
    ///
    /// 用于调试快捷键 (Shift+C) 触发阶段切换，方便测试各阶段的灯光效果。
    /// 阶段按照 `DEBUG_SEQUENCE` 定义的顺序循环推进。
    pub fn advance_debug(&mut self) {
        self.phase = self.phase.next_debug();
    }

    // 返回当前阶段内已过去的时间 (秒)。如果阶段已变化 (尚未渲染新阶段)，返回 0。
    fn phase_elapsed_secs(&self, elapsed_secs: f64) -> f64 {
        if self.phase == self.last_rendered_phase {
            // 阶段未变化，计算从阶段开始到现在的经过时间
            (elapsed_secs - self.phase_started_at_secs).max(0.0)
        } else {
            // 阶段已变化但尚未渲染，返回 0 使灯光从初始状态开始
            0.0
        }
    }

    // 渲染阶段计时器：检测阶段变化，在阶段切换时重置计时器。
    // 返回当前阶段内已过去的时间，用于灯光相位计算。
    fn render_elapsed_secs(&mut self, elapsed_secs: f64) -> f64 {
        if self.phase != self.last_rendered_phase {
            // 阶段切换时，记录新阶段的开始时间
            self.last_rendered_phase = self.phase;
            self.phase_started_at_secs = elapsed_secs;
        }

        self.phase_elapsed_secs(elapsed_secs)
    }

    // 返回红蓝双方灯光组的数组，用于遍历更新。
    fn teams(&self) -> [TeamCoreLights; 2] {
        [self.red, self.blue]
    }
}

// 预创建的材质句柄集合，用于快速切换灯光颜色。
// 包含关闭、白色、红色、蓝色、绿色五种材质，避免每帧重复创建。
#[derive(Clone)]
struct TechCoreMaterialHandles {
    off: Handle<StandardMaterial>,
    white: Handle<StandardMaterial>,
    red: Handle<StandardMaterial>,
    blue: Handle<StandardMaterial>,
    green: Handle<StandardMaterial>,
}

impl TechCoreMaterialHandles {
    // 创建所有预定义灯光材质，初始化时一次性创建以减少运行时开销。
    fn new(materials: &mut Assets<StandardMaterial>) -> Self {
        Self {
            off: materials.add(material(0.02, 0.02, 0.02, 0.0)),
            white: materials.add(material(1.0, 1.0, 1.0, 1.5)),
            red: materials.add(material(1.0, 0.0, 0.0, 1.8)),
            blue: materials.add(material(0.0, 0.12, 1.0, 1.8)),
            green: materials.add(material(0.0, 1.0, 0.18, 1.8)),
        }
    }

    // 根据灯光程序和已用时间解析对应的材质句柄。
    // 先通过 active_color 获取当前活跃颜色，再映射到具体材质。
    fn resolve(
        &self,
        team: Team,
        program: LightProgram,
        elapsed_secs: f64,
    ) -> Handle<StandardMaterial> {
        // 如果灯光当前熄灭，返回关闭材质
        let Some(color) = program.active_color(elapsed_secs) else {
            return self.off.clone();
        };

        self.resolve_color(team, color)
    }

    // 将 LightColor 枚举映射到具体的材质句柄。
    fn resolve_color(&self, team: Team, color: LightColor) -> Handle<StandardMaterial> {
        match color {
            LightColor::White => self.white.clone(),
            LightColor::Green => self.green.clone(),
            LightColor::Team => match team {
                Team::Red => self.red.clone(),
                Team::Blue => self.blue.clone(),
            },
        }
    }
}

// 创建灯光材质，使用 base_color 控制颜色，emissive 控制自发光强度和颜色。
// emissive_exposure_weight 设为 -1.0 使自发光不受曝光影响，保持稳定的灯光效果。
fn material(red: f32, green: f32, blue: f32, emissive_strength: f32) -> StandardMaterial {
    StandardMaterial {
        base_color: Color::srgb(red, green, blue),
        emissive: LinearRgba::new(
            red * emissive_strength,
            green * emissive_strength,
            blue * emissive_strength,
            1.0,
        ),
        emissive_exposure_weight: -1.0,
        ..Default::default()
    }
}

// 在场景实体的名称映射中查找第一组灯光 (First) 的实体引用。
// 按命名约定 {prefix}_L_{n} 和 {prefix}_R_{n} 查找左右两侧的分段灯带，
// 同时查找整体式灯环 {prefix} 作为回退。
fn find_first_light_set(name_map: &HashMap<String, Entity>, prefix: &str) -> FirstLightSet {
    let mut left = [None; FIRST_LIGHT_SEGMENT_COUNT];
    let mut right = [None; FIRST_LIGHT_SEGMENT_COUNT];

    for index in 0..FIRST_LIGHT_SEGMENT_COUNT {
        left[index] = name_map.get(&format!("{prefix}_L_{}", index + 1)).copied();
        right[index] = name_map.get(&format!("{prefix}_R_{}", index + 1)).copied();
    }

    FirstLightSet::new(name_map.get(prefix).copied(), left, right)
}

// 在场景实体的名称映射中查找单方队伍的三组灯光实体引用。
// names 数组包含三个元素的名称前缀：[First, Second, Third]。
// 如果 Second 或 Third 灯光实体缺失，则返回 None 并发出警告。
fn find_team_lights(
    name_map: &HashMap<String, Entity>,
    team: Team,
    names: [&str; 3],
) -> Option<TeamCoreLights> {
    let [first_name, second_name, third_name] = names;
    let first = find_first_light_set(name_map, first_name);
    let Some(second) = name_map.get(second_name).copied() else {
        warn!("TECH_CORE.glb is missing {second_name}");
        return None;
    };
    let Some(third) = name_map.get(third_name).copied() else {
        warn!("TECH_CORE.glb is missing {third_name}");
        return None;
    };

    Some(TeamCoreLights::new(team, first, second, third))
}

// 检查第一组灯光的分段灯带是否完整，如果不完整则发出警告日志。
// 当既没有分段灯带也没有整体式灯环时，或分段灯带部分缺失时发出警告。
fn warn_incomplete_first_light_set(prefix: &str, lights: &FirstLightSet) {
    if !lights.has_segments() {
        // 没有分段灯带，检查是否有整体式灯环
        if lights.whole.is_none() {
            warn!(
                "TECH_CORE.glb is missing {prefix} and segmented {prefix}_{{L,R}}_1..{FIRST_LIGHT_SEGMENT_COUNT}"
            );
        }
        return;
    }

    // 有分段灯带，检查是否有缺失的段
    let missing = lights.missing_segments(prefix);
    if missing.is_empty() {
        return;
    }

    // 限制显示前 6 个缺失项，避免日志过长
    let preview = missing
        .iter()
        .take(6)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let suffix = if missing.len() > 6 {
        format!(" (+{} more)", missing.len() - 6)
    } else {
        String::new()
    };

    warn!("TECH_CORE.glb has incomplete {prefix} segments; missing {preview}{suffix}");
}

// 科技核心场景初始化系统，在 GLTF 场景实例化完成后执行。
// 搜索场景中按约定命名的灯光实体，绑定材质并创建 TechCore 组件。
fn setup_tech_core(
    events: On<SceneInstanceReady>,
    mut commands: Commands,
    scene_spawner: Res<SceneSpawner>,
    roots: Query<(), With<TechCoreRoot>>,
    names: Query<&Name>,
) {
    // 仅处理标记了 TechCoreRoot 的实体
    if !roots.contains(events.entity) {
        return;
    }

    // 构建场景中所有实体的名称到实体句柄的映射，用于按名称查找灯光节点
    let name_map = scene_spawner
        .iter_instance_entities(events.instance_id)
        .filter_map(|entity| {
            names
                .get(entity)
                .map(|name| (name.to_string(), entity))
                .ok()
        })
        .collect::<HashMap<_, _>>();

    // 查找红蓝双方的灯光组 (First, Second, Third)
    let Some(red) = find_team_lights(&name_map, Team::Red, RED_LIGHT_NAMES) else {
        return;
    };
    let Some(blue) = find_team_lights(&name_map, Team::Blue, BLUE_LIGHT_NAMES) else {
        return;
    };

    // 检查第一组灯光的分段是否完整，不完整时发出警告
    warn_incomplete_first_light_set(RED_LIGHT_NAMES[0], &red.first);
    warn_incomplete_first_light_set(BLUE_LIGHT_NAMES[0], &blue.first);

    // 为根实体插入 TechCore 组件，启动灯光更新
    commands
        .entity(events.entity)
        .insert(TechCore::new(red, blue));
    info!("Tech core lights bound");
}

// 为指定实体及其所有后代递归设置材质句柄。
// 先尝试设置根实体自身的材质，再遍历所有后代实体设置。
fn assign_material(
    root: Entity,
    handle: Handle<StandardMaterial>,
    children: &Query<&Children>,
    mesh_materials: &mut Query<&mut MeshMaterial3d<StandardMaterial>>,
) {
    // 设置根实体自身的材质
    if let Ok(mut mesh_material) = mesh_materials.get_mut(root) {
        mesh_material.0 = handle.clone();
    }

    // 遍历所有后代实体，为每个带有 MeshMaterial3d 的实体设置材质
    for child in children.iter_descendants(root) {
        if let Ok(mut mesh_material) = mesh_materials.get_mut(child) {
            mesh_material.0 = handle.clone();
        }
    }
}

// 每帧更新科技核心灯光系统，根据当前阶段和已用时间更新灯光材质。
fn update_tech_core_lights(
    time: Res<Time>,
    mut handles: Local<Option<TechCoreMaterialHandles>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    children: Query<&Children>,
    mut mesh_materials: Query<&mut MeshMaterial3d<StandardMaterial>>,
    mut cores: Query<&mut TechCore>,
) {
    // 首次运行时惰性初始化材质句柄缓存
    let handles = handles.get_or_insert_with(|| TechCoreMaterialHandles::new(&mut materials));
    let elapsed_secs = time.elapsed_secs_f64();

    for mut core in &mut cores {
        // 获取阶段内已用时间，阶段切换时自动重置计时器
        let phase_elapsed_secs = core.render_elapsed_secs(elapsed_secs);
        let programs = core.phase.programs();
        let step5_lights = core.step5_lights();
        // 分别更新红蓝双方的灯光组
        for team in core.teams() {
            for group in TechCoreLightGroup::ALL {
                let program = programs[group.index()];
                match group {
                    // 第一组灯光：支持流水、组装等分段控制模式
                    TechCoreLightGroup::First => team.first.assign_program(
                        team.team,
                        program,
                        phase_elapsed_secs,
                        step5_lights,
                        handles,
                        &children,
                        &mut mesh_materials,
                    ),
                    // 第二组灯光：整体式灯环，直接解析颜色并设置材质
                    TechCoreLightGroup::Second => {
                        let handle = handles.resolve(team.team, program, phase_elapsed_secs);
                        assign_material(team.second, handle, &children, &mut mesh_materials);
                    }
                    // 第三组灯光：整体式灯环，与第二组逻辑相同
                    TechCoreLightGroup::Third => {
                        let handle = handles.resolve(team.team, program, phase_elapsed_secs);
                        assign_material(team.third, handle, &children, &mut mesh_materials);
                    }
                }
            }
        }
    }
}

// 调试用系统：按下 Shift+C 键切换科技核心到下一个阶段，用于测试各阶段的灯光效果。
fn debug_cycle_tech_core_phase(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut cores: Query<&mut TechCore>,
) {
    // 检测 Shift (左或右) + C 键组合
    if !(keyboard.pressed(KeyCode::ShiftLeft) || keyboard.pressed(KeyCode::ShiftRight))
        || !keyboard.just_pressed(KeyCode::KeyC)
    {
        return;
    }

    // 将所有科技核心切换到下一个阶段
    for mut core in &mut cores {
        core.advance_debug();
        info!("Tech core phase: {:?}", core.phase());
    }
}

/// 科技核心插件，负责注册科技核心的初始化、灯光更新和调试系统。
///
/// 注册以下系统：
/// - `setup_tech_core`: 场景加载完成后初始化科技核心组件，搜索灯光实体并绑定材质。
/// - `update_tech_core_lights`: 每帧根据当前阶段和已用时间更新灯光材质。
/// - `debug_cycle_tech_core_phase`: 调试用，按下 Shift+C 快捷键切换阶段，方便测试灯光效果。
#[derive(Default)]
pub(super) struct TechCorePlugin;

impl Plugin for TechCorePlugin {
    fn build(&self, app: &mut App) {
        // 添加场景加载观察者，在 GLTF 场景实例化完成后自动初始化科技核心
        app.add_observer(setup_tech_core).add_systems(
            Update,
            (debug_cycle_tech_core_phase, update_tech_core_lights),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tech_core_phase_programs_match_spec() {
        use AssemblyLightProgram::{Completed, InProgress};
        use BlinkRate::{Hz1, Hz3};
        use LightColor::{Team, White};
        use LightProgram::{Assembly, Blink, Flow, Off, Solid};

        assert_eq!(
            TechCorePhase::MatchRunningIdle.programs(),
            [Off, Solid(Team), Solid(Team)]
        );
        assert_eq!(
            TechCorePhase::DifficultySelectedArmNotReady.programs(),
            [Flow { color: White }, Solid(Team), Solid(Team)]
        );
        assert_eq!(
            TechCorePhase::DifficultySelectedArmReady.programs(),
            [
                Blink {
                    color: White,
                    rate: Hz1
                },
                Blink {
                    color: Team,
                    rate: Hz1
                },
                Blink {
                    color: Team,
                    rate: Hz1
                },
            ]
        );
        assert_eq!(
            TechCorePhase::Step2Completed.programs(),
            [
                Blink {
                    color: White,
                    rate: Hz1
                },
                Blink {
                    color: Team,
                    rate: Hz1
                },
                Blink {
                    color: Team,
                    rate: Hz3
                },
            ]
        );
        assert_eq!(
            TechCorePhase::Step3Completed.programs(),
            [
                Blink {
                    color: White,
                    rate: Hz1
                },
                Blink {
                    color: Team,
                    rate: Hz3
                },
                Blink {
                    color: Team,
                    rate: Hz3
                },
            ]
        );
        assert_eq!(
            TechCorePhase::Step4Completed.programs(),
            [
                Blink {
                    color: White,
                    rate: Hz1
                },
                Solid(Team),
                Solid(Team),
            ]
        );
        assert_eq!(
            TechCorePhase::Step5InProgress.programs(),
            [Assembly(InProgress), Solid(Team), Solid(Team)]
        );
        assert_eq!(
            TechCorePhase::Step5Completed.programs(),
            [Assembly(Completed), Solid(Team), Solid(Team)]
        );
        assert_eq!(
            TechCorePhase::ConfirmedRecovering.programs(),
            [
                Blink {
                    color: White,
                    rate: Hz3
                },
                Blink {
                    color: Team,
                    rate: Hz3
                },
                Blink {
                    color: Team,
                    rate: Hz3
                },
            ]
        );
    }

    #[test]
    fn tech_core_debug_sequence_wraps() {
        let mut phase = TechCorePhase::MatchRunningIdle;
        for expected in TechCorePhase::DEBUG_SEQUENCE.into_iter().skip(1) {
            phase = phase.next_debug();
            assert_eq!(phase, expected);
        }

        assert_eq!(phase.next_debug(), TechCorePhase::MatchRunningIdle);
    }

    #[test]
    fn tech_core_phase_ids_are_stable() {
        let ids = TechCorePhase::DEBUG_SEQUENCE.map(TechCorePhase::id);
        assert_eq!(ids, [0, 1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn light_program_resolves_blink_active_color() {
        let program = LightProgram::Blink {
            color: LightColor::White,
            rate: BlinkRate::Hz1,
        };

        assert_eq!(program.active_color(0.25), Some(LightColor::White));
        assert_eq!(program.active_color(0.75), None);
    }

    #[test]
    fn tech_core_segment_maps_angles_to_first_light_indices() {
        assert_eq!(
            TechCoreFirstLightSegment::from_angle_degrees(0.0).number(),
            1
        );
        assert_eq!(
            TechCoreFirstLightSegment::from_angle_degrees(20.0).number(),
            2
        );
        assert_eq!(
            TechCoreFirstLightSegment::from_angle_degrees(359.9).number(),
            FIRST_LIGHT_SEGMENT_COUNT
        );
        assert_eq!(
            TechCoreFirstLightSegment::from_angle_degrees(-1.0).number(),
            FIRST_LIGHT_SEGMENT_COUNT
        );
    }

    #[test]
    fn tech_core_flow_activation_runs_forward_back_then_all() {
        assert_eq!(flow_activation(0.0), FlowActivation::Segment(0));
        assert_eq!(
            flow_activation((FIRST_LIGHT_SEGMENT_COUNT as f64 - 1.0) / FLOW_SEGMENT_HZ),
            FlowActivation::Segment(FIRST_LIGHT_SEGMENT_COUNT - 1)
        );
        assert_eq!(
            flow_activation(FIRST_LIGHT_SEGMENT_COUNT as f64 / FLOW_SEGMENT_HZ),
            FlowActivation::Segment(FIRST_LIGHT_SEGMENT_COUNT - 1)
        );
        assert_eq!(
            flow_activation((FIRST_LIGHT_SEGMENT_COUNT as f64 * 2.0 - 1.0) / FLOW_SEGMENT_HZ),
            FlowActivation::Segment(0)
        );
        assert_eq!(
            flow_activation(FIRST_LIGHT_SEGMENT_COUNT as f64 * 2.0 / FLOW_SEGMENT_HZ),
            FlowActivation::All
        );
    }

    #[test]
    fn tech_core_state_json_contains_flow_segments() {
        let value: Value = serde_json::from_str(&tech_core_state_json_from_phases(
            0,
            0,
            0.0,
            [TechCorePhase::DifficultySelectedArmNotReady],
        ))
        .unwrap();
        let red_first = &value["cores"][0]["lights"][0];

        assert_eq!(red_first["program"]["mode"], "flow");
        assert_eq!(red_first["program"]["segment_hz"], FLOW_SEGMENT_HZ);
        assert_eq!(red_first["active_segments"][0]["side"], "left");
        assert_eq!(red_first["active_segments"][0]["index"], 1);
        assert_eq!(red_first["active_segments"][1]["side"], "right");
        assert_eq!(red_first["active_segments"][1]["index"], 1);

        let value: Value = serde_json::from_str(&tech_core_state_json_from_phases(
            0,
            0,
            FIRST_LIGHT_SEGMENT_COUNT as f64 * 2.0 / FLOW_SEGMENT_HZ,
            [TechCorePhase::DifficultySelectedArmNotReady],
        ))
        .unwrap();

        assert_eq!(
            value["cores"][0]["lights"][0]["active_segments"]
                .as_array()
                .unwrap()
                .len(),
            FIRST_LIGHT_SEGMENT_COUNT * 2
        );
    }

    #[test]
    fn tech_core_state_json_contains_resolved_light_state() {
        let value: Value = serde_json::from_str(&tech_core_state_json_from_phases(
            12,
            34,
            0.0,
            [TechCorePhase::Step5Completed],
        ))
        .unwrap();

        assert_eq!(value["stamp"]["sec"], 12);
        assert_eq!(value["stamp"]["nanosec"], 34);
        assert_eq!(value["cores"][0]["phase"]["id"], 7);
        assert_eq!(value["cores"][0]["phase"]["name"], "step5_completed");
        assert_eq!(value["cores"][0]["lights"][0]["team"], "red");
        assert_eq!(value["cores"][0]["lights"][0]["group"], 1);
        assert_eq!(
            value["cores"][0]["lights"][0]["program"]["mode"],
            "step5_completed"
        );
        assert_eq!(
            value["cores"][0]["lights"][0]["program"]["target_color"],
            "green"
        );
        assert_eq!(value["cores"][0]["lights"][0]["active_color"], "green");
        assert_eq!(
            value["cores"][0]["lights"][0]["active_segments"][0]["role"],
            "target"
        );
        assert_eq!(
            value["cores"][0]["lights"][0]["active_segments"][0]["color"],
            "green"
        );
        assert_eq!(value["cores"][0]["lights"][3]["team"], "blue");
        assert_eq!(
            value["cores"][0]["lights"][3]["program"]["target_color"],
            "green"
        );
    }

    #[test]
    fn tech_core_state_json_contains_step5_in_progress_segments() {
        let value: Value = serde_json::from_str(&tech_core_state_json_from_phases(
            0,
            0,
            0.0,
            [TechCorePhase::Step5InProgress],
        ))
        .unwrap();
        let red_first = &value["cores"][0]["lights"][0];

        assert_eq!(red_first["program"]["mode"], "step5_in_progress");
        assert_eq!(red_first["program"]["target_color"], "red");
        assert_eq!(red_first["program"]["energy_unit_color"], "white");
        assert_eq!(red_first["active_color"], "mixed");
        assert_eq!(red_first["active_segments"][0]["role"], "target");
        assert_eq!(red_first["active_segments"][0]["color"], "red");
        assert_eq!(red_first["active_segments"][2]["role"], "energy_unit");
        assert_eq!(red_first["active_segments"][2]["color"], "white");
    }

    #[test]
    fn tech_core_state_json_marks_blink_off_half() {
        let value: Value = serde_json::from_str(&tech_core_state_json_from_phases(
            0,
            0,
            0.75,
            [TechCorePhase::DifficultySelectedArmReady],
        ))
        .unwrap();

        assert_eq!(value["cores"][0]["lights"][0]["program"]["mode"], "blink");
        assert_eq!(value["cores"][0]["lights"][0]["program"]["hz"], 1.0);
        assert_eq!(value["cores"][0]["lights"][0]["active_color"], "off");
    }
}
