//! 装甲构造模块
//!
//! 负责从 3D 模型网格中自动构建装甲实体，包括：
//! - 提取装甲标记点 (marker) 和顶点数据 (vertex)
//! - 根据队伍颜色配置灯光条 (light strip)
//! - 管理贴纸 (sticker) 的可见性
//! - 为装甲子物体添加碰撞体

use crate::query;
use crate::robomaster::prelude::{ArmorLabel, ArmorSpec, MarkerData, Team, extract_markers};
use crate::util::entity_query::HierarchyQuery;
use avian3d::prelude::{ColliderConstructor, ColliderConstructorHierarchy, TrimeshFlags};
use bevy::app::App;
use bevy::ecs::system::SystemParam;
use bevy::ecs::system::lifetimeless::Read;
use bevy::mesh::VertexAttributeValues;
use bevy::prelude::{
    Added, Assets, Changed, ChildOf, Children, Commands, Component, Entity, Mesh, Mesh3d, Name,
    Plugin, Query, Res, Update, Vec3, Visibility, With, info,
};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Component, Debug)]
/// 标记组件：标记待扫描构建的装甲根实体，携带队伍和规格信息
pub struct ScanArmor {
    /// 队伍标识（红方/蓝方）
    pub team: Team,
    /// 装甲规格（小装甲/大装甲 + 标签）
    pub spec: ArmorSpec,
}

impl ScanArmor {
    /// 创建新的装甲扫描标记
    ///
    /// # 参数
    /// - `team`: 队伍标识
    /// - `spec`: 装甲规格
    pub const fn new(team: Team, spec: ArmorSpec) -> Self {
        Self { team, spec }
    }
}

#[derive(Component, Clone, Debug)]
/// 装甲顶点数据组件，存储从装甲模型提取的碰撞体顶点集合
pub struct VertexData {
    /// 顶点所属侧边（左侧/右侧）
    pub side: Side,
    /// 顶点坐标列表
    pub points: Vec<Vec3>,
}

#[derive(Component, Clone, Debug)]
/// 装甲灯光条组件，标记装甲上的 LED 灯带实体并记录其所在侧
pub struct LightStrip {
    /// 灯光条所在侧边（左侧/右侧）
    pub side: Side,
}

#[derive(Component, Clone, Debug)]
/// 装甲核心组件，附加到装甲的每个子物体上，携带完整的装甲标识信息
pub struct Armor {
    /// 装甲名称
    pub name: String,
    /// 队伍标识（红方/蓝方）
    pub team: Team,
    /// 装甲规格（小装甲/大装甲 + 标签）
    pub spec: ArmorSpec,
    /// 装甲标签
    pub label: ArmorLabel,
}

#[derive(Component, Clone, Copy, Debug)]
/// 贴纸组件，标记装甲上的贴纸实体，记录其所属根实体和标签
pub struct ArmorSticker {
    /// 所属装甲根实体
    pub root: Entity,
    /// 贴纸的装甲标签
    pub label: ArmorLabel,
}

#[derive(Component, Clone, Debug)]
/// 贴纸选择组件，用于在调试中切换当前显示的贴纸
pub struct ArmorStickerSelection {
    /// 当前选中的贴纸标签
    pub label: ArmorLabel,
    /// 在 sequence_small() 序列中的索引
    pub sequence_index: usize,
}

impl ArmorStickerSelection {
    /// 创建新的贴纸选择器，根据标签初始化其在序列中的索引
    pub fn new(label: ArmorLabel) -> Self {
        Self {
            label,
            sequence_index: ArmorLabel::index_from_small(label),
        }
    }

    /// 切换到序列中的下一个贴纸标签（循环），用于调试时切换装甲贴纸
    ///
    /// # 返回值
    /// 切换后的新 ArmorLabel
    pub fn advance_debug_sequence(&mut self) -> ArmorLabel {
        let sequence = ArmorLabel::sequence_small();
        self.sequence_index += 1;
        self.sequence_index %= sequence.len();
        self.label = sequence[self.sequence_index];
        self.label
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
/// 装甲侧边枚举，标识装甲的左侧或右侧
pub enum Side {
    /// 左侧
    Left,
    /// 右侧
    Right,
}

impl Side {
    /// 将侧边转换为数组索引（Left=0, Right=1）
    pub const fn index(self) -> usize {
        match self {
            Self::Left => 0,
            Self::Right => 1,
        }
    }
}

#[derive(SystemParam)]
/// 装甲构造器系统参数，封装了构造装甲所需的全部 ECS 查询和资源
pub struct ArmorConstructor<'w, 's> {
    /// ECS 指令队列，用于增删组件、生成/销毁实体
    commands: Commands<'w, 's>,
    /// 子实体查询，用于遍历装甲的子物体层级
    children: Query<'w, 's, Read<Children>>,
    /// 父子关系查询，用于查找实体的父级
    child_of: Query<'w, 's, Read<ChildOf>>,
    /// 名称查询，用于按名称匹配装甲子物体
    name: Query<'w, 's, Read<Name>, With<ChildOf>>,
    /// 网格组件查询，用于获取实体的 3D 网格句柄
    mesh_query: Query<'w, 's, Read<Mesh3d>>,
    /// 网格资产资源，用于从句柄获取实际网格数据
    mesh_assets: Res<'w, Assets<Mesh>>,
}

#[derive(Component, Clone)]
/// 装甲根实体标记组件，携带全局唯一的装甲 ID
pub struct ArmorRoot {
    /// 全局唯一的装甲标识符
    pub id: ArmorId,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
/// 装甲 ID 包装类型，内部使用 usize 实现全局递增编号
pub struct ArmorId(usize);

impl ArmorId {
    /// 获取内部 usize 值
    pub const fn as_usize(self) -> usize {
        self.0
    }
}

#[derive(Component, Clone)]
/// 装甲部件组件，记录装甲各子部件的实体引用（标记点、灯光、顶点）
pub struct ArmorParts {
    /// 标记点实体
    marker: Entity,
    /// 灯光条实体数组 [左侧, 右侧]
    lights: [Entity; 2],
    /// 顶点数据实体数组 [左侧, 右侧]
    vertices: [Entity; 2],
}

// 辅助宏：为 ArmorParts 生成根据 Side 获取对应侧部件的访问方法
macro_rules! impl_side {
    ($method_name:ident, $field:ident) => {
        #[inline]
        #[must_use]
        pub fn $method_name(&self, side: Side) -> Entity {
            self.$field[side.index()]
        }
    };
}

impl ArmorParts {
    // 生成 light(side) 方法，获取指定侧的灯光条实体
    impl_side!(light, lights);
    // 生成 vertex(side) 方法，获取指定侧的顶点数据实体
    impl_side!(vertex, vertices);

    /// 获取装甲标记点实体
    #[inline]
    #[must_use]
    pub fn marker(&self) -> Entity {
        self.marker
    }
}

impl ArmorConstructor<'_, '_> {
    // 从实体中获取 Mesh 网格数据
    fn get_mesh(&self, entity: Entity) -> Option<&Mesh> {
        let mesh_handle = self.mesh_query.get(entity).ok()?;
        self.mesh_assets.get(mesh_handle)
    }

    fn process_marker(
        &mut self,
        entity: Entity,
        name: &str,
        armor_data: &ScanArmor,
    ) -> Option<MarkerData> {
        let mesh = self.get_mesh(entity)?;
        let vertices = extract_markers(mesh)?;

        info!(
            "Armor {:?}_{:?}_{:?}@'{}': Added marker with {} points",
            armor_data.team,
            armor_data.spec.armor_type(),
            armor_data.spec.label(),
            name,
            vertices.len()
        );

        self.commands
            .entity(entity)
            .insert((MarkerData(vertices), Visibility::Hidden));
        Some(MarkerData(vertices))
    }

    fn extract_vertex(
        &mut self,
        entity: Entity,
        name: &str,
        armor_data: &ScanArmor,
    ) -> Option<Vec<Vec3>> {
        let mesh = self.get_mesh(entity)?;

        let vertices = extract_vertices(mesh)?;

        info!(
            "Armor {:?}_{:?}_{:?}@'{}': Extracted {} vertices",
            armor_data.team,
            armor_data.spec.armor_type(),
            armor_data.spec.label(),
            name,
            vertices.len()
        );

        Some(vertices)
    }

    fn process_armor_root(
        &mut self,
        root: Entity,
        armor_name: String,
        armor_data: &ScanArmor,
    ) -> Option<ArmorRoot> {
        let query = HierarchyQuery::new(self.child_of, self.children, self.name);
        let root_query = query.of(root).flatten();
        {
            self.commands.entity(query!(root_query, .."ARMOR")?).insert(
                ColliderConstructorHierarchy::new(ColliderConstructor::TrimeshFromMeshWithConfig(
                    TrimeshFlags::MERGE_DUPLICATE_VERTICES,
                )),
            );
        }
        {
            let children = self.children;

            let name = self.name;
            children
                .iter_descendants(root)
                .filter_map(|v| name.get(v).ok().map(|name| (name, v)))
                .for_each(|(elem_name, armor_elem)| {
                    self.commands.entity(armor_elem).insert(Armor {
                        name: elem_name.to_string(),
                        team: armor_data.team,
                        spec: armor_data.spec,
                        label: armor_data.spec.label(),
                    });
                });
        }
        //let _base = query!(root_query, .."BASE")?;
        let lights = [
            [query!(root_query, .."L_L")?, query!(root_query, .."L_R")?],
            [
                query!(root_query, .."L_L_RED")?,
                query!(root_query, .."L_R_RED")?,
            ],
        ];
        let (lights, hide) = match armor_data.team {
            Team::Red => (lights[1], lights[0]),
            Team::Blue => (lights[0], lights[1]),
        };
        for hide in hide {
            self.commands.entity(hide).despawn();
        }

        self.commands
            .entity(lights[0])
            .insert(LightStrip { side: Side::Left });
        self.commands
            .entity(lights[1])
            .insert(LightStrip { side: Side::Right });

        let marker = query!(root_query, .."MARKER", ...)?;
        self.process_marker(marker, &armor_name, armor_data)?;

        let vertex = [
            (Side::Left, query!(root_query, .."VERTEX_L", ...)?),
            (Side::Right, query!(root_query, .."VERTEX_R", ...)?),
        ];
        let vertices = vertex.map(|(side, vertex)| {
            let v = self
                .extract_vertex(vertex, &armor_name, armor_data)
                .unwrap();
            self.commands.entity(vertex).insert((
                VertexData {
                    side,
                    points: v.clone(),
                },
                Visibility::Hidden,
            ));
            vertex
        });
        {
            let c_query = query!(root_query, .."_C", ref).flatten();
            c_query.clone().any().into_iter().for_each(|e| {
                self.commands.entity(e).insert(Visibility::Hidden);
            });
            for slot in armor_data.spec.sticker_slots() {
                let sticker = c_query.clone().suffix(slot.name_suffix).one()?;
                self.commands.entity(sticker).insert((
                    ArmorSticker {
                        root,
                        label: slot.label,
                    },
                    match slot.label == armor_data.spec.label() {
                        true => Visibility::Visible,
                        false => Visibility::Hidden,
                    },
                ));
            }
        }

        self.commands.entity(root).insert(Armor {
            name: armor_name.clone(),
            team: armor_data.team,
            spec: armor_data.spec,
            label: armor_data.spec.label(),
        });

        static ID: AtomicUsize = AtomicUsize::new(0);

        let ar = ArmorRoot {
            id: ArmorId(ID.fetch_add(1, Ordering::SeqCst)),
        };
        let parts = ArmorParts {
            marker,
            lights,
            vertices,
        };
        self.commands.entity(root).insert((
            ar.clone(),
            parts,
            ArmorStickerSelection::new(armor_data.spec.label()),
        ));
        Some(ar)
    }
}

/// 从Mesh中提取所有顶点
pub fn extract_vertices(mesh: &Mesh) -> Option<Vec<Vec3>> {
    mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        .and_then(|values| {
            if let VertexAttributeValues::Float32x3(vec) = values {
                Some(vec.iter().map(|&p| Vec3::from(p)).collect())
            } else {
                None
            }
        })
        .filter(|points: &Vec<Vec3>| !points.is_empty())
}

fn insert(
    root: Query<(Entity, Read<ScanArmor>), Added<ScanArmor>>,
    mut constructor: ArmorConstructor,
) {
    for (root_entity, armor_data) in root.iter() {
        let children = constructor.children;
        let name = constructor.name;
        children
            .iter_descendants(root_entity)
            .filter_map(|child| {
                name.get(child)
                    .ok()
                    .filter(|name| name.contains("ARMOR_ROOT"))
                    .map(|name| (child, name))
            })
            .for_each(|(ent, name)| {
                constructor.process_armor_root(ent, name.to_string(), armor_data);
            })
    }
}

fn sync_armor_stickers(
    mut commands: Commands,
    selections: Query<(Entity, &ArmorStickerSelection), Changed<ArmorStickerSelection>>,
    stickers: Query<(Entity, &ArmorSticker)>,
) {
    for (root, selection) in &selections {
        for (entity, sticker) in &stickers {
            if sticker.root != root {
                continue;
            }
            commands
                .entity(entity)
                .insert(match sticker.label == selection.label {
                    true => Visibility::Visible,
                    false => Visibility::Hidden,
                });
        }
    }
}

#[derive(Default)]
pub(super) struct ArmorConstructorPlugin;

impl Plugin for ArmorConstructorPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (insert, sync_armor_stickers));
    }
}
