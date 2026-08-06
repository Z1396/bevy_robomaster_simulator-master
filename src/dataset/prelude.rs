//! `prelude` 模块
//!
//! 数据集捕获流水线核心逻辑。该模块提供：
//! - 世界坐标到屏幕坐标的投影变换 (`world_to_screen`)
//! - 装甲屏幕坐标排序 (`sort_screen_points`)
//! - 数据集自动捕获回调 (`DatasetSnapshotCreator`)
//! - 每帧装甲数据采集与遮挡检测系统 (`capture`)
//!
//! 通过 `DatasetPlugin` 接入 Bevy ECS 生命周期，在 `ExtractSchedule` 阶段
//! 自动捕获装甲标注数据并写入数据集。

use crate::capture::CaptureCamera;
use crate::capture::driver::{
    CaptureConfig, CapturedFrame, CapturedFrameKind, GpuCaptureHandler, SnapshotAsync, SnapshotSync,
};
use crate::dataset::occlusion::Occlusion;
use crate::dataset::writer::{ArmorColor, ArmorEntry, DatasetWriter};
use crate::robomaster::prelude::{
    Armor, ArmorLabel, ArmorParts, ArmorRoot, ArmorType, MarkerData, Side, Team, VertexData,
};
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::*;
use bevy::render::{Extract, RenderApp, RenderSystems};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// 装甲遮挡系统的阶段枚举。
///
/// 用于在 Bevy 渲染系统中标注遮挡检测的执行顺序。
#[derive(Debug, Hash, PartialEq, Eq, Clone, SystemSet)]
pub enum ArmorOcclusionSystems {
    /// 传播/更新阶段，在此阶段执行遮挡检测与数据传播
    Propagate,
}

/// Shared data resource for armor entries - can be used by both manual and auto capture
#[derive(Default, Resource, Deref, DerefMut)]
pub struct ArmorData(pub Mutex<Vec<ArmorEntry>>);

/// 数据集写入器的全局句柄资源。
///
/// 包装了一个 `Arc<Mutex<DatasetWriter>>`，可在不同线程/系统间安全共享。
#[derive(Resource, Deref, DerefMut)]
pub struct DatasetHandle(pub Arc<Mutex<DatasetWriter>>);

#[derive(Resource, Deref, DerefMut)]
struct Cooldown(Mutex<Timer>);

/// 数据集捕获插件。
///
/// 接入 Bevy ECS 生命周期，在渲染子应用 (`RenderApp`) 的 `ExtractSchedule` 阶段
/// 注册数据集捕获系统，按固定时间间隔（0.25 秒）自动采集装甲标注数据。
pub struct DatasetPlugin;
impl Plugin for DatasetPlugin {
    /// 构建插件：注册资源与系统。
    ///
    /// 在 `RenderApp` 中注册 `DatasetHandle`、`Data`、`Cooldown` 资源，
    /// 并添加 `capture` 系统，按冷却时间间隔运行，依赖数字键 1 触发。
    fn build(&self, app: &mut App) {
        app.sub_app_mut(RenderApp)
            .insert_resource(DatasetHandle(Arc::new(Mutex::new(
                DatasetWriter::new("dataset").unwrap(),
            ))))
            .insert_resource(Data::default())
            .insert_resource(Cooldown(Mutex::new(Timer::from_seconds(
                0.25,
                TimerMode::Once,
            ))))
            .add_systems(
                ExtractSchedule,
                capture
                    .in_set(ArmorOcclusionSystems::Propagate)
                    .run_if(
                        |time: Res<Time>,
                         cd: Res<Cooldown>,
                         key: Extract<Res<ButtonInput<KeyCode>>>| {
                            let mut guard = cd.lock().unwrap();
                            guard.tick(time.delta());
                            if guard.is_finished() {
                                guard.reset();
                                return key.pressed(KeyCode::Digit1);
                            }
                            false
                        },
                    )
                    .before(RenderSystems::Render),
            );
    }
}

/// 将三维世界坐标投影到屏幕像素坐标。
///
/// 通过相机视图矩阵和投影矩阵执行完整的 MVP 变换流水线：
/// 世界坐标 -> 裁剪坐标 -> NDC -> 屏幕像素坐标。
///
/// 参数：
/// - `world`: 三维世界坐标点
/// - `camera_xform`: 相机的全局变换矩阵
/// - `projection`: 相机投影参数（透视/正交）
/// - `config`: 捕获配置，包含屏幕宽高
///
/// 返回值：
/// - `Some((x, y))`: 屏幕像素坐标（左上角原点），点在相机前方且在视锥内
/// - `None`: 点在相机后方或超出屏幕范围
pub fn world_to_screen(
    world: Vec3,
    camera_xform: &GlobalTransform,
    projection: &Projection,
    config: &CaptureConfig,
) -> Option<(u32, u32)> {
    let clip =
        projection.get_clip_from_view() * camera_xform.to_matrix().inverse() * world.extend(1.0);

    // point is behind camera
    if clip.w <= 0.0 {
        return None;
    }

    // clip -> ndc
    let ndc = clip.xyz() / clip.w;

    // outside of screen view (x,y out of range)
    if ndc.x < -1.0 || ndc.x > 1.0 || ndc.y < -1.0 || ndc.y > 1.0 {
        return None;
    }

    // ndc -> screen
    let screen_x = (ndc.x + 1.0) * 0.5 * (config.width as f32);
    let screen_y = (1.0 - ndc.y) * 0.5 * (config.height as f32);

    Some((screen_x as u32, screen_y as u32))
}

type CornerTuple = (Vec3, (u32, u32));

/// 将装甲的四个角点按角度排序为统一顺序（左上、右上、右下、左下）。
///
/// 计算四个角点的几何中心，以中心为原点按方位角降序排列，
/// 确保装甲标注的角点顺序在不同帧间保持一致。
///
/// 参数：
/// - `points`: 四个角点的元组数组，每个元组包含 (世界坐标, 屏幕坐标)
///
/// 返回值：排序后的四角点数组，顺序为 [左上, 右上, 右下, 左下]
pub(crate) fn sort_screen_points(points: [CornerTuple; 4]) -> [CornerTuple; 4] {
    let points_with_vec: Vec<(CornerTuple, Vec2)> = points
        .iter()
        .map(|&v| (v, Vec2::new(v.1.0 as f32, v.1.1 as f32)))
        .collect();

    let center = points_with_vec
        .iter()
        .map(|(_, v)| *v)
        .fold(Vec2::ZERO, |acc, v| acc + v)
        / 4.0;

    let mut sorted: Vec<(CornerTuple, Vec2, f32)> = points_with_vec
        .into_iter()
        .map(|(tuple, vec)| {
            let dir = (vec - center).normalize();
            let angle = dir.angle_to(Vec2::X).to_degrees();
            (tuple, vec, angle)
        })
        .collect();

    // 角度 descending 排序
    sorted.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());

    [sorted[0].0, sorted[3].0, sorted[2].0, sorted[1].0]
}

type ArmorScreenData = (ArmorType, ArmorLabel, ArmorColor, [(u32, u32); 4]);

/// GPU 捕获回调处理器，负责在捕获完成后将装甲数据传递给 GPU 快照流水线。
///
/// 实现 `GpuCaptureHandler` 特征，从 `Data` 资源中取出暂存的装甲条目，
/// 构造 `DatasetSnapshotSync` 传递给后续的同步/异步快照阶段。
#[derive(Default)]
pub struct DatasetSnapshotCreator {}

/// 装甲条目暂存资源，用于在 `ExtractSchedule` 和 GPU 捕获之间传递数据。
///
/// 由 `capture` 系统写入，由 `DatasetSnapshotCreator` 读取并清空。
#[derive(Default, Resource, Deref, DerefMut)]
pub(crate) struct Data(Mutex<Vec<ArmorEntry>>);

impl GpuCaptureHandler for DatasetSnapshotCreator {
    fn captured(&self, world: &World) -> Option<Box<dyn SnapshotSync>> {
        let mut guard = world.resource::<Data>().lock().unwrap();
        let data = guard.drain(..).collect::<Vec<_>>();
        if !data.is_empty() {
            Some(Box::new(DatasetSnapshotSync { data }))
        } else {
            None
        }
    }
}

struct DatasetSnapshotSync {
    data: Vec<ArmorEntry>,
}

impl SnapshotSync for DatasetSnapshotSync {
    fn captured(
        self: Box<Self>,
        world: &mut DeferredWorld,
        _config: &CaptureConfig,
    ) -> Box<dyn SnapshotAsync> {
        Box::new(DatasetSnapshot {
            data: self.data,
            writer: world.resource::<DatasetHandle>().0.clone(),
        })
    }
}

struct DatasetSnapshot {
    data: Vec<ArmorEntry>,
    writer: Arc<Mutex<DatasetWriter>>,
}

impl SnapshotAsync for DatasetSnapshot {
    fn captured(&mut self, frame: CapturedFrame<'_>) {
        if frame.kind != CapturedFrameKind::Rgb8 {
            return;
        }
        self.writer
            .lock()
            .unwrap()
            .write_entry(frame.height, frame.width, frame.data, &self.data)
            .unwrap();
    }
}

/// 每帧装甲数据采集系统。
///
/// 在 `ExtractSchedule` 阶段运行，执行以下流程：
/// 1. 遍历所有装甲实体，将顶点和标记点从世界坐标投影到屏幕坐标
/// 2. 对装甲角点进行排序，确保角点顺序一致
/// 3. 执行遮挡检测，过滤被遮挡的装甲
/// 4. 将可见装甲的标注数据（颜色、类型、标签、角点坐标）存入 `Data` 资源
///
/// 参数：
/// - `root_data`: 所有装甲实体的查询（实体、装甲组件、根节点、部件）
/// - `vertex_data`: 顶点实体的变换和顶点数据
/// - `marker_data`: 标记实体的变换和标记点数据
/// - `camera`: 捕获相机（投影参数和全局变换）
/// - `occlusion`: 遮挡检测参数
/// - `config`: 捕获配置（屏幕宽高）
/// - `armor_r`: 装甲条目暂存资源，用于输出采集结果
pub(crate) fn capture(
    root_data: Extract<Query<(Entity, &Armor, &ArmorRoot, &ArmorParts)>>,
    vertex_data: Extract<Query<(&GlobalTransform, &VertexData)>>,
    marker_data: Extract<Query<(&GlobalTransform, &MarkerData)>>,
    camera: Extract<Single<(&Projection, &GlobalTransform), With<CaptureCamera>>>,
    mut occlusion: Extract<Occlusion>,
    config: Res<CaptureConfig>,
    armor_r: Res<Data>,
) {
    let mut armor_screen: HashMap<Team, Vec<ArmorScreenData>> = HashMap::new();
    let (projection, camera_global_transform) = **camera;
    let camera_pos = camera_global_transform.translation();

    for (vertex_entity, armor, _root, parts) in root_data.iter() {
        let all_in_frustum = |global_transform: &GlobalTransform,
                              unmapped: &[Vec3]|
         -> Option<Vec<(Vec3, (u32, u32))>> {
            let mut mapped = Vec::with_capacity(unmapped.len());
            for elem in unmapped {
                let global = global_transform.transform_point(*elem);
                let pos = world_to_screen(global, camera_global_transform, projection, &config)?;
                mapped.push((global, pos))
            }
            Some(mapped)
        };
        let marker = parts.marker();
        let vertices = [parts.vertex(Side::Left), parts.vertex(Side::Right)];
        let mut vert = Vec::with_capacity(vertices.len());
        for vertex in vertices {
            let (tf, vertex_data) = vertex_data.get(vertex).unwrap();
            let Some(vertices) = all_in_frustum(tf, vertex_data.points.as_slice()) else {
                continue;
            };
            vert.push((
                &vertex_data.side,
                vertex,
                vertices.into_iter().map(|v| v.0).collect::<Vec<_>>(),
            ));
        }
        if vert.len() != vertices.len() {
            continue;
        }
        let (tf, MarkerData(markers)) = marker_data.get(marker).unwrap();

        let Some(markers) = all_in_frustum(tf, markers) else {
            continue;
        };
        let marker_ordered = sort_screen_points(markers.as_slice().try_into().unwrap());
        if !occlusion.visible(
            camera_pos,
            camera_global_transform.forward().as_vec3(),
            &marker_ordered,
            armor.name.as_str(),
            vertex_entity,
            vert.as_slice(),
        ) {
            continue;
        }
        armor_screen.entry(armor.team).or_insert(default()).push((
            armor.spec.armor_type(),
            armor.label,
            match armor.team {
                Team::Red => ArmorColor::Red,
                Team::Blue => ArmorColor::Blue,
            },
            marker_ordered.map(|v| v.1),
        ));
    }
    let mut rr = armor_r.lock().unwrap();
    armor_screen.drain().for_each(|(_, n)| {
        for (typ, label, color, pos) in n {
            rr.push(ArmorEntry {
                color,
                typ,
                label,
                points: pos.map(|v| {
                    Vec2::new(
                        (v.0 as f32) / (config.width as f32),
                        (v.1 as f32) / (config.height as f32),
                    )
                }),
            });
        }
    });
}
