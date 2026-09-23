//! 装甲构造模块
//!
//! 负责从 3D 模型网格中自动构建装甲实体，包括：
//! - 提取装甲标记点 (marker) 和顶点数据 (vertex)
//! - 根据队伍颜色配置灯光条 (light strip)
//! - 管理贴纸 (sticker) 的可见性
//! - 为装甲子物体添加碰撞体
//! 模块文档注释，cargo doc 生成文档时展示，不参与编译执行

// 内部自定义查询工具
use crate::query;
// 项目装甲全局类型、提取标记点工具函数
use crate::robomaster::prelude::{ArmorLabel, ArmorSpec, MarkerData, Team, extract_markers};
// 层级遍历查询工具，遍历父子物体
use crate::util::entity_query::HierarchyQuery;
// Avian3D 物理引擎：碰撞体构造、层级生成碰撞、碰撞三角网格配置
use avian3d::prelude::{ColliderConstructor, ColliderConstructorHierarchy, TrimeshFlags};
// Bevy 应用插件基础
use bevy::app::App;
// 系统参数相关
use bevy::ecs::system::SystemParam;
use bevy::ecs::system::lifetimeless::Read;
// 网格顶点属性枚举
use bevy::mesh::VertexAttributeValues;
// Bevy 基础类型全部导入
use bevy::prelude::{
    Added, Assets, Changed, ChildOf, Children, Commands, Component, Entity, Mesh, Mesh3d, Name,
    Plugin, Query, Res, Update, Vec3, Visibility, With, info,
};
// 原子类型：多线程安全的全局自增装甲ID
use std::sync::atomic::{AtomicUsize, Ordering};

/// 标记组件：标记待扫描构建的装甲根实体，携带队伍和规格信息
/// 给装甲父物体挂上这个组件，引擎就知道「这是一个待自动装配的装甲」
#[derive(Component, Debug)]
pub struct ScanArmor {
    /// 队伍标识（红方/蓝方）
    pub team: Team,
    /// 装甲规格（小装甲/大装甲 + 标签A/B/C/D）
    pub spec: ArmorSpec,
}

impl ScanArmor {
    /// 创建新的装甲扫描标记
    /// const fn：编译期可构造，无运行时开销
    pub const fn new(team: Team, spec: ArmorSpec) -> Self {
        Self { team, spec }
    }
}

/// 装甲顶点数据组件，存储从装甲模型提取的碰撞体顶点集合
/// 装甲轮廓顶点，用于后续PnP解算、装甲定位
#[derive(Component, Clone, Debug)]
pub struct VertexData {
    /// 顶点所属侧边（左侧/右侧装甲面）
    pub side: Side,
    /// 顶点坐标列表，装甲轮廓的所有三维坐标
    pub points: Vec<Vec3>,
}

/// 装甲灯光条组件，标记装甲上的 LED 灯带实体并记录其所在侧
/// 绑定在装甲灯带子物体上，区分左右灯带
#[derive(Component, Clone, Debug)]
pub struct LightStrip {
    /// 灯光条所在侧边（左侧/右侧）
    pub side: Side,
}

/// 装甲核心组件，附加到装甲的每个子物体上，携带完整的装甲标识信息
/// 装甲所有零件（外壳、灯带、贴纸、标记点）都会挂载该组件，统一归属装甲
#[derive(Component, Clone, Debug)]
pub struct Armor {
    /// 装甲名称（取自模型物体名）
    pub name: String,
    /// 队伍标识（红方/蓝方）
    pub team: Team,
    /// 装甲规格（大小装甲）
    pub spec: ArmorSpec,
    /// 装甲标签 A/B/C/D
    pub label: ArmorLabel,
}

/// 贴纸组件，标记装甲上的贴纸实体，记录其所属根实体和标签
/// 每个装甲贴纸物体挂载，记录属于哪个装甲、是几号标签贴纸
#[derive(Component, Clone, Copy, Debug)]
pub struct ArmorSticker {
    /// 所属装甲根实体ID
    pub root: Entity,
    /// 贴纸对应的装甲标签
    pub label: ArmorLabel,
}

/// 贴纸选择组件，用于在调试中切换当前显示的贴纸
/// 挂载在装甲根实体，控制当前装甲展示哪一张阵营贴纸
#[derive(Component, Clone, Debug)]
pub struct ArmorStickerSelection {
    /// 当前选中需要显示的贴纸标签
    pub label: ArmorLabel,
    /// 在小型装甲贴纸序列中的下标
    pub sequence_index: usize,
}

impl ArmorStickerSelection {
    /// 根据装甲标签，初始化贴纸选择器，并计算序列下标
    pub fn new(label: ArmorLabel) -> Self {
        Self {
            label,
            sequence_index: ArmorLabel::index_from_small(label),
        }
    }

    /// 切换到序列中的下一个贴纸标签（循环轮转），调试用：键盘按键切换装甲贴纸样式
    pub fn advance_debug_sequence(&mut self) -> ArmorLabel {
        // 获取小装甲贴纸完整顺序 [A,B,C,D...]
        let sequence = ArmorLabel::sequence_small();
        // 下标+1
        self.sequence_index += 1;
        // 取模循环，到末尾回到0
        self.sequence_index %= sequence.len();
        // 更新当前展示标签
        self.label = sequence[self.sequence_index];
        self.label
    }
}

/// 装甲侧边枚举，标识装甲的左侧或右侧装甲面
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Side {
    /// 装甲左侧
    Left,
    /// 装甲右侧
    Right,
}

impl Side {
    /// 将侧边转为数组下标 Left=0, Right=1，方便用数组存放左右灯光、左右顶点
    pub const fn index(self) -> usize {
        match self {
            Self::Left => 0,
            Self::Right => 1,
        }
    }
}

/// 装甲构造器系统参数
/// #[derive(SystemParam)]：把多个查询、资源打包成一个参数，简化系统入参
/// 封装装甲构建全过程需要的所有ECS工具：命令、层级查询、网格资源等
/// `'w` 世界生命周期，`'s` 系统生命周期。
#[derive(SystemParam)]
pub struct ArmorConstructor<'w, 's> {
    /// ECS指令队列：新增组件、删除实体、修改实体属性
    commands: Commands<'w, 's>,
    /// 查询某个实体的所有子物体 Children
    children: Query<'w, 's, Read<Children>>,
    /// 查询某个实体的父实体 ChildOf
    child_of: Query<'w, 's, Read<ChildOf>>,
    /// 读取实体名称Name，限定必须是某个物体的子物体
    name: Query<'w, 's, Read<Name>, With<ChildOf>>,
    /// 查询实体身上的Mesh3d网格组件（拿到网格句柄）
    mesh_query: Query<'w, 's, Read<Mesh3d>>,
    /// 全局网格资产仓库，通过Mesh3d句柄拿到真正的Mesh网格数据
    mesh_assets: Res<'w, Assets<Mesh>>,
}

/// 装甲根实体标记组件，挂载装甲最顶层父物体，携带全局唯一装甲ID
#[derive(Component, Clone)]
pub struct ArmorRoot {
    /// 全局唯一装甲编号
    pub id: ArmorId,
}

/// 装甲ID包装类型，避免usize裸类型混用造成BUG
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct ArmorId(usize);

impl ArmorId {
    /// 取出内部usize原始数值
    pub const fn as_usize(self) -> usize {
        self.0
    }
}

/// 装甲部件组件，挂载装甲根实体，记录装甲所有关键子物体的Entity编号
/// 方便后续逻辑快速获取装甲标记点、左右灯带、左右顶点实体
#[derive(Component, Clone)]
pub struct ArmorParts {
    /// 装甲标记点实体（用于视觉PnP识别）
    marker: Entity,
    /// 左右灯光实体数组 [左灯, 右灯]
    lights: [Entity; 2],
    /// 左右顶点轮廓实体数组 [左轮廓, 右轮廓]
    vertices: [Entity; 2],
}

/// 辅助宏：批量生成Side侧取方法
/// 例如 impl_side!(light, lights) 自动生成 fn light(&self, side: Side) -> Entity
macro_rules! impl_side {
    ($method_name:ident, $field:ident) => {
        #[inline]
        #[must_use]
        pub fn $method_name(&self, side: Side) -> Entity {
            // 利用Side.index()转下标，从数组取出对应侧实体
            self.$field[side.index()]
        }
    };
}

impl ArmorParts {
    // 生成 light(side) 方法，外部调用 armor_parts.light(Side::Left) 即可拿到左侧灯带实体
    impl_side!(light, lights);
    // 生成 vertex(side) 方法，获取左侧/右侧轮廓顶点实体
    impl_side!(vertex, vertices);

    /// 直接获取装甲标记点实体
    #[inline]
    #[must_use]
    pub fn marker(&self) -> Entity {
        self.marker
    }
}

impl ArmorConstructor<'_, '_> {
    /// 根据实体Entity，读取它身上的Mesh网格对象
    /// 返回 Option<&Mesh>，取不到网格返回None
    fn get_mesh(&self, entity: Entity) -> Option<&Mesh> {
        // 尝试获取实体的Mesh3d网格句柄，失败直接return None
        let mesh_handle = self.mesh_query.get(entity).ok()?;
        // 通过句柄在全局网格资源库拿到真正网格数据
        self.mesh_assets.get(mesh_handle)
    }

    /// 处理 MARKER 标记点物体：提取识别用特征点坐标，挂载MarkerData组件，隐藏标记点模型
    fn process_marker(
        &mut self,
        entity: Entity,
        name: &str,
        armor_data: &ScanArmor,
    ) -> Option<MarkerData> {
        // 获取标记点的网格
        let mesh = self.get_mesh(entity)?;
        // 调用工具函数，从网格提取识别标记点
        let vertices = extract_markers(mesh)?;

        // 控制台日志：打印当前正在装配哪一个装甲、提取了多少标记点
        info!(
            "Armor {:?}_{:?}_{:?}@'{}': Added marker with {} points",
            armor_data.team,
            armor_data.spec.armor_type(),
            armor_data.spec.label(),
            name,
            vertices.len()
        );

        // 给标记点实体挂载MarkerData（存放特征点），并设置隐藏模型（标记点只是用来提取坐标，不需要渲染出来）
        self.commands
            .entity(entity)
            .insert((MarkerData(vertices), Visibility::Hidden));
        Some(MarkerData(vertices))
    }

    /// 提取装甲轮廓顶点集合，返回三维坐标列表
    fn extract_vertex(
        &mut self,
        entity: Entity,
        name: &str,
        armor_data: &ScanArmor,
    ) -> Option<Vec<Vec3>> {
        let mesh = self.get_mesh(entity)?;
        // 解析网格所有顶点
        let vertices = extract_vertices(mesh)?;

        // 日志打印提取顶点数量
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

    /// 核心函数：处理单个装甲根物体，完成整套装甲装配逻辑
    fn process_armor_root(
        &mut self,
        root: Entity,
        armor_name: String,
        armor_data: &ScanArmor,
    ) -> Option<ArmorRoot> {
        // 创建层级查询器，遍历装甲的父子层级结构
        let query = HierarchyQuery::new(self.child_of, self.children, self.name);
        let root_query = query.of(root).flatten();

        // 1. 为名称带 ARMOR 的子物体自动生成碰撞体
        {
            // 匹配子物体名字包含ARMOR的实体，生成三角网格碰撞体
            // TrimeshFlags::MERGE_DUPLICATE_VERTICES：合并重复顶点，简化碰撞网格、减少物理运算开销
            self.commands.entity(query!(root_query, .."ARMOR")?).insert(
                ColliderConstructorHierarchy::new(ColliderConstructor::TrimeshFromMeshWithConfig(
                    TrimeshFlags::MERGE_DUPLICATE_VERTICES,
                )),
            );
        }

        // 2. 遍历装甲所有后代子物体，全部挂载 Armor 核心组件，归属当前装甲
        {
            let children = self.children;
            let name = self.name;
            // 递归遍历装甲根的所有子孙实体
            children
                .iter_descendants(root)
                // 过滤：能拿到实体名称的才继续
                .filter_map(|v| name.get(v).ok().map(|name| (name, v)))
                .for_each(|(elem_name, armor_elem)| {
                    // 每个装甲零件挂载Armor组件，记录队伍、装甲规格、标签
                    self.commands.entity(armor_elem).insert(Armor {
                        name: elem_name.to_string(),
                        team: armor_data.team,
                        spec: armor_data.spec,
                        label: armor_data.spec.label(),
                    });
                });
        }

        // 3. 根据队伍保留对应颜色灯带，删除敌方颜色灯带
        // 两套灯带：默认红蓝两套灯带都在模型里，红队删掉蓝色灯带，蓝队删掉红色灯带
        let lights = [
            [query!(root_query, .."L_L")?, query!(root_query, .."L_R")?], // 蓝色左右灯带
            [
                query!(root_query, .."L_L_RED")?,
                query!(root_query, .."L_R_RED")?,
            ], // 红色左右灯带
        ];
        // 红队保留红色灯带，销毁蓝色；蓝队保留蓝色灯带，销毁红色
        let (lights, hide) = match armor_data.team {
            Team::Red => (lights[1], lights[0]),
            Team::Blue => (lights[0], lights[1]),
        };
        // 销毁不需要的另一套灯带实体
        for hide in hide {
            self.commands.entity(hide).despawn();
        }

        // 给保留下来的左右灯带挂载LightStrip组件，标记左右侧
        self.commands
            .entity(lights[0])
            .insert(LightStrip { side: Side::Left });
        self.commands
            .entity(lights[1])
            .insert(LightStrip { side: Side::Right });

        // 4. 找到MARKER标记点实体，提取特征点
        let marker = query!(root_query, .."MARKER", ...)?;
        self.process_marker(marker, &armor_name, armor_data)?;

        // 5. 分别提取左侧VERTEX_L、右侧VERTEX_R轮廓顶点
        let vertex = [
            (Side::Left, query!(root_query, .."VERTEX_L", ...)?),
            (Side::Right, query!(root_query, .."VERTEX_R", ...)?),
        ];
        let vertices = vertex.map(|(side, vertex)| {
            // 提取顶点坐标
            let v = self
                .extract_vertex(vertex, &armor_name, armor_data)
                .unwrap();
            // 挂载VertexData组件存放顶点，隐藏顶点模型
            self.commands.entity(vertex).insert((
                VertexData {
                    side,
                    points: v.clone(),
                },
                Visibility::Hidden,
            ));
            vertex
        });

        // 6. 处理装甲阵营贴纸：默认全部隐藏，只展示当前装甲对应标签的贴纸
        {
            // 匹配所有后缀带 _C 的贴纸物体
            let c_query = query!(root_query, .."_C", ref).flatten();
            // 先把全部贴纸隐藏
            c_query.clone().any().into_iter().for_each(|e| {
                self.commands.entity(e).insert(Visibility::Hidden);
            });
            // 遍历当前装甲可用贴纸槽位
            for slot in armor_data.spec.sticker_slots() {
                // 找到对应名称的贴纸实体
                let sticker = c_query.clone().suffix(slot.name_suffix).one()?;
                // 挂载ArmorSticker组件归属装甲，匹配标签则显示贴纸，其余隐藏
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

        // 装甲根实体自身也挂载Armor组件
        self.commands.entity(root).insert(Armor {
            name: armor_name.clone(),
            team: armor_data.team,
            spec: armor_data.spec,
            label: armor_data.spec.label(),
        });

        // 7. 生成全局自增装甲唯一ID（原子类型，多线程安全）
        static ID: AtomicUsize = AtomicUsize::new(0);
        // fetch_add：取值并自增，SeqCst保证多线程顺序安全
        let ar = ArmorRoot {
            id: ArmorId(ID.fetch_add(1, Ordering::SeqCst)),
        };

        // 组装ArmorParts结构，保存标记点、左右灯光、左右顶点实体
        let parts = ArmorParts {
            marker,
            lights,
            vertices,
        };

        // 装甲根挂载三大组件：唯一ID、部件记录表、贴纸选择控制器
        self.commands.entity(root).insert((
            ar.clone(),
            parts,
            ArmorStickerSelection::new(armor_data.spec.label()),
        ));

        Some(ar)
    }
}

/// 全局工具函数：解析Mesh网格，提取所有顶点坐标Vec<Vec3>
pub fn extract_vertices(mesh: &Mesh) -> Option<Vec<Vec3>> {
    // 获取网格的位置顶点属性
    mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        .and_then(|values| {
            // 判断顶点格式是否为Float32x3三维浮点坐标
            if let VertexAttributeValues::Float32x3(vec) = values {
                // 把 [f32;3] 转为 Bevy Vec3 存入集合
                Some(vec.iter().map(|&p| Vec3::from(p)).collect())
            } else {
                None
            }
        })
        // 过滤空顶点列表，空则返回None
        .filter(|points: &Vec<Vec3>| !points.is_empty())
}

/// 装甲初始化系统
/// 触发规则：仅当某个实体**刚刚新增 ScanArmor 组件的那一帧**运行一次，装甲只会被构建一次，不会反复重复构建
fn insert(
    /*1. `(Entity, Read<ScanArmor>)`：拿到实体 ID + **只读**读取`ScanArmor`（阵营、装甲参数，不修改）
    2. `Added<ScanArmor>`：只匹配**本帧刚添加 ScanArmor 的实体**，只执行一次，防止重复创建碰撞体 / 组件。
    3. `mut constructor: ArmorConstructor`：`ArmorConstructor`是`SystemParam`，需要`mut`才能用 Commands 增删实体、组件。 */
    root: Query<(Entity, Read<ScanArmor>), Added<ScanArmor>>,
    // 打包好的装甲构造工具集（封装 Commands、层级查询、网格资源等 SystemParam），可变因为内部要新增组件、生成碰撞体
    mut constructor: ArmorConstructor,
) {
    // 循环遍历所有本帧刚挂载 ScanArmor 的顶层父实体（机器人根物体）
    /*- `iter_descendants`：**递归遍历所有后代**，不管嵌套多少层子物体，不用手动多层循环
    - `filter_map`：**边查询边过滤**，拿不到 Name / 名字不匹配 ARMOR_ROOT 直接丢弃
    - `Added<ScanArmor>`保证整套扫描**只执行一次**，不会每帧重复扫描、重复生成碰撞体造成 bug */
    for (root_entity, armor_data) in root.iter() {
        // 解构取出构造器内的子物体查询、名称查询，简化后续书写
        let children = constructor.children;
        let name = constructor.name;

        // 递归遍历 root_entity 下所有后代子物体（所有层级的子子孙孙）
        children
            .iter_descendants(root_entity)
            // filter_map：过滤无效项，同时做类型映射
            /*递归遍历 root_entity 全部后代子物体（深层嵌套的装甲子部件也能搜到）。 */
            .filter_map(|child| {
                // 尝试获取当前子物体的 Name 名称组件
                name.get(child)
                    .ok()
                    // 过滤：只保留物体名称中包含 "ARMOR_ROOT" 的实体，这才是装甲真正的根节点
                    .filter(|name| name.contains("ARMOR_ROOT"))
                    // 匹配成功，则返回 (装甲实体, 装甲名称) 二元组
                    .map(|name| (child, name))
            })
            // 每找到一个装甲根节点，就执行装甲完整装配逻辑
            /*name.get(child).ok()：获取子物体名称，获取失败（无 Name 组件）则丢弃该物体；
            .filter(|name| name.contains("ARMOR_ROOT"))：精准筛选装甲根物体；
            .map(|name| (child, name))：保留「装甲实体 + 装甲名称」；
            filter_map 自动丢弃返回 None 的无效物体。 */
            .for_each(|(ent, name)| {
                constructor.process_armor_root(
                    ent,                // 装甲真正根实体
                    name.to_string(),   // 装甲物体名称，转为字符串存入装甲信息
                    armor_data          // 上层携带的阵营、装甲规格数据
                );
            /*把找到的装甲根送入核心装配函数 process_armor_root，完成：
            生成碰撞体、分配阵营灯光、提取识别标记点、控制贴纸显隐、挂载全套装甲组件。 */
            })
    }
}

/// 系统 sync_armor_stickers：
/// 触发条件：当装甲根实体的 ArmorStickerSelection 组件发生修改时执行
/// 作用：同步刷新装甲所有贴纸的显示/隐藏，切换贴纸样式
fn sync_armor_stickers(
    mut commands: Commands,
    // 只查询发生「修改」的贴纸控制器组件
    selections: Query<(Entity, &ArmorStickerSelection), Changed<ArmorStickerSelection>>,
    // 查询全部贴纸实体
    stickers: Query<(Entity, &ArmorSticker)>,
) {
    // 遍历被修改的装甲贴纸控制器
    for (root, selection) in &selections {
        // 遍历场景所有贴纸
        for (entity, sticker) in &stickers {
            // 贴纸不属于当前装甲，跳过
            if sticker.root != root {
                continue;
            }
            // 贴纸标签和选中标签一致则显示，其余隐藏
            commands
                .entity(entity)
                .insert(match sticker.label == selection.label {
                    true => Visibility::Visible,
                    false => Visibility::Hidden,
                });
        }
    }
}

/// 装甲构造插件，仅本装甲模块内部可访问 pub(super)
#[derive(Default)]
pub(super) struct ArmorConstructorPlugin;

impl Plugin for ArmorConstructorPlugin {
    fn build(&self, app: &mut App) {
        // Update阶段挂载两个系统
        // insert：新装甲生成时自动装配结构、碰撞、组件
        // sync_armor_stickers：贴纸切换时同步刷新显隐
        app.add_systems(Update, (insert, sync_armor_stickers));
    }
}