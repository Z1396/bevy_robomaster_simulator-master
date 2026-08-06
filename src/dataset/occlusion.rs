//! `occlusion` 模块
//!
//! 装甲遮挡检测子系统。该模块通过射线投射技术判断装甲板是否被其他物体遮挡，
//! 用于过滤不可见的装甲，确保数据集标注只包含可视装甲。
//!
//! 核心逻辑：
//! - 从装甲采样点向相机发射射线
//! - 根据碰撞物体的类型（己方机体、敌方灯带、环境物体等）分级判定遮挡类型
//! - 单侧装甲所有采样点均通过可见性校验后，才判定该装甲可见

// 己方受控实体标记组件
use crate::components::Controlled;
// RM机甲专属组件：装甲板、灯带、阵营侧边、装甲顶点数据
use crate::robomaster::prelude::{Armor, LightStrip, Side, VertexData};
use bevy::{
    ecs::system::{SystemParam, lifetimeless::Read},
    prelude::*,
};

/// 将多个Query统一封装为SystemParam
/// 在任意system中直接声明 `mut occ: Occlusion` 即可使用，不用重复写一堆Query
#[derive(SystemParam)]
pub struct Occlusion<'w, 's> {
    // 查询父子关系，遍历实体所有父节点，判断物体归属哪个机甲
    child_of: Query<'w, 's, Read<ChildOf>>,
    // 装甲板组件只读查询
    armor: Query<'w, 's, Read<Armor>>,
    // 装甲采样顶点数据
    vertex: Query<'w, 's, Read<VertexData>>,
    // 机甲灯带组件
    light_strip: Query<'w, 's, Read<LightStrip>>,
    // 实体名称，调试用
    names: Query<'w, 's, Read<Name>>,
    // 所有被玩家/程序控制的己方机甲实体
    controlled: Query<'w, 's, Entity, With<Controlled>>,
    // 全局世界坐标（世界变换矩阵）
    global_transforms: Query<'w, 's, Read<GlobalTransform>>,
    // Bevy网格射线投射工具，实现物理射线检测遮挡
    ray_cast: MeshRayCast<'w, 's>,
}

/// 遮挡判定结果枚举
enum OcclusionType {
    None,        // 无遮挡：装甲采样点直接暴露在视野中
    Tolerated,   // 被普通障碍物遮挡（墙体、地面等，允许判定装甲可见）
    Untolerated, // 被敌方另一侧灯带遮挡，严格判定装甲不可见
}

impl<'w, 's> Occlusion<'w, 's> {
    /// 单个采样点遮挡采样逻辑
    /// camera_pos：相机世界坐标
    /// ident：装甲标识（调试字符串）
    /// armor_entity：当前待检测的装甲板实体
    /// side：当前装甲所属机甲侧边（左/右）
    /// _vertex_entity：装甲顶点实体
    /// sample_pos：装甲表面采样点世界坐标
    fn sample_occluded(
        &mut self,
        camera_pos: Vec3,
        _ident: &str,
        armor_entity: Entity,
        side: &Side,
        _vertex_entity: Entity,
        sample_pos: Vec3,
    ) -> OcclusionType {
        // 射线方向：采样点 → 相机位置
        let dir = camera_pos - sample_pos;
        let total_dist = dir.length();

        // 采样点和相机几乎重合，不存在遮挡
        if total_dist < f32::EPSILON {
            return OcclusionType::None;
        }

        // 构造射线：起点=装甲采样点，朝向相机
        let ray = Ray3d::new(sample_pos, Dir3::new(dir.normalize()).unwrap());
        // 发射射线，附带过滤规则
        let hits = self.ray_cast.cast_ray(
            ray,
            &MeshRayCastSettings {
                visibility: RayCastVisibility::VisibleInView,
                // 射线碰撞过滤函数：决定哪些物体可以挡住装甲视线
                filter: &|e| -> bool {
                    // 规则1：如果碰撞物体属于【己方受控机甲】，忽略碰撞（己方机体不会挡住敌方装甲）
                    if self
                        .child_of
                        .iter_ancestors(e)
                        .any(|parent| self.controlled.get(parent).is_ok())
                    {
                        return false;
                    }

                    // 规则2：如果碰撞物体是敌方机甲另一侧的装甲顶点部件，忽略
                    let is_vertex = self.child_of.iter_ancestors(e).any(|parent| {
                        let Ok(parent) = self.vertex.get(parent) else {
                            return false;
                        };
                        parent.side != *side
                    });
                    if is_vertex {
                        return false;
                    }

                    // 规则3：判断碰撞物体是不是当前这块装甲自身的子物体
                    let is_self = self
                        .child_of
                        .iter_ancestors(e)
                        .any(|parent| parent == armor_entity);
                    if is_self {
                        // 属于装甲自身的子物体，区分是否是灯带
                        let is_light_strip = self
                            .child_of
                            .iter_ancestors(e)
                            .any(|parent| self.light_strip.get(parent).is_ok());

                        if is_light_strip {
                            // 如果是灯带：只有【另一侧的灯带】才参与遮挡判定
                            let is_other_side = self.child_of.iter_ancestors(e).any(|parent| {
                                self.light_strip
                                    .get(parent)
                                    .map(|ls| ls.side != *side)
                                    .unwrap_or(false)
                            });
                            return is_other_side;
                        }

                        // 装甲自身盖板、结构件，允许参与遮挡
                        return true;
                    }

                    // 其余环境物体、敌方机身都可以阻挡视线
                    true
                },
                ..default()
            },
        );

        // 遍历所有射线碰撞结果
        for &(e, ref hit) in hits {
            // 分支：碰撞物是敌方另一侧灯带 → 判定为严重遮挡，直接返回不可见
            'g: for ancestor in self.child_of.iter_ancestors(e) {
                let Ok(ancestor) = self.light_strip.get(ancestor) else {
                    continue 'g;
                };
                if ancestor.side != *side {
                    return OcclusionType::Untolerated;
                }
            }

            // 障碍物在采样点与相机之间，产生遮挡
            let is_occluded = hit.distance < total_dist - f32::EPSILON;

            if is_occluded {
                return OcclusionType::Tolerated;
            }
        }

        // 全程没有任何障碍物挡住射线，无遮挡
        OcclusionType::None
    }

    /// 对外暴露接口：判断一整块装甲侧边是否整体可见
    /// vertices：装甲多个采样顶点集合，机甲一侧装甲包含多个采样点
    pub fn visible(
        &mut self,
        camera_pos: Vec3,
        _forward: Vec3,
        _markers: &[(Vec3, (u32, u32)); 4],
        ident: &str,
        armor_entity: Entity,
        vertices: &[(&Side, Entity, Vec<Vec3>)],
    ) -> bool {
        // 该侧边所有装甲采样区域全部通过可见校验，才判定整块装甲可见
        vertices
            .iter()
            .all(move |v| self.side_visible(camera_pos, ident, armor_entity, v))
    }

    /// 单侧装甲多个采样点综合判定可见性
    fn side_visible(
        &mut self,
        camera_pos: Vec3,
        ident: &str,
        armor_entity: Entity,
        vertex_entity: &(&Side, Entity, Vec<Vec3>),
    ) -> bool {
        let (side, vertex_entity, ref samples) = *vertex_entity;
        // 遍历该装甲面上所有采样坐标，逐个发射射线检测遮挡
        let iter = samples.iter().map(move |&sample| {
            self.sample_occluded(camera_pos, ident, armor_entity, side, vertex_entity, sample)
        });

        let mut visible = false;
        for result in iter {
            match result {
                OcclusionType::None => {
                    // 任意采样点无遮挡，标记该装甲具备可见区域
                    visible = true;
                }
                OcclusionType::Tolerated => {}
                OcclusionType::Untolerated => {
                    // 只要任意采样点被敌方另一侧灯带遮挡，整块装甲直接判定不可见
                    return false;
                }
            }
        }
        // 只要存在至少一个未被遮挡的采样点，就认为装甲可见
        visible
    }
}