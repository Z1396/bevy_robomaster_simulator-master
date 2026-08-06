//! `frame_data` 模块
//!
//! 定义遥测系统的帧数据结构，包括装甲标注和帧数据包。
//! 用于在 Bevy ECS 中传递每帧的装甲检测结果和实体位姿信息。

use bevy::prelude::*;
use std::collections::HashMap;

use crate::robomaster::prelude::{ArmorLabel, Team};

/// 单块装甲的完整标注信息。
///
/// 包含装甲的阵营、编号、四角屏幕坐标和三维中心坐标，以及遮挡状态标记。
#[derive(Clone, Debug)]
pub struct ArmorAnnotation {
    /// 装甲所属阵营（红方/蓝方）
    pub team: Team,
    /// 装甲编号标签
    pub label: ArmorLabel,
    /// 装甲板四个角在屏幕上的像素坐标，按顺时针或逆时针顺序排列
    pub corners: [Vec2; 4],
    /// 装甲板中心的三维世界坐标
    pub center_3d: Vec3,
    /// 是否被遮挡（true 表示该装甲被其他物体遮挡，不可见）
    pub occluded: bool,
}

/// 单帧遥测数据包。
///
/// 包含时间戳、所有检测到的装甲板标注信息，以及各实体的位姿映射。
#[derive(Clone)]
pub struct FrameData {
    /// 当前帧的时间戳（秒，从程序启动开始累积）
    pub timestamp: f64,
    /// 本帧中检测到的所有装甲板标注列表
    pub armors: Vec<ArmorAnnotation>,
    /// 实体名称到位姿的映射表，记录各机器人/相机在当前帧的位姿
    pub poses: HashMap<String, Transform>,
}
