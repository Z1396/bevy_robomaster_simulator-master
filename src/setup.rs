// Avian3D 3D物理引擎全套类型
use avian3d::prelude::*;
// FXAA抗锯齿
use bevy::anti_alias::fxaa::Fxaa;
// 色调映射
use bevy::core_pipeline::tonemapping::Tonemapping;
// Bevy基础ECS、资源、命令、变换、光照等
use bevy::prelude::*;
// GLB场景实例加载事件
use bevy::scene::{SceneInstance, SceneInstanceReady};
// Egui UI全局配置、主Egui上下文挂载标记
use bevy_inspector_egui::bevy_egui::{EguiGlobalSettings, PrimaryEguiContext};
use std::collections::HashMap;

// 自研自定义组件
use crate::components::{
    ActiveSlapper,        // 当前活跃击打方
    Controlled,          // 本机玩家操控的实体标记
    GameLayer,            // 碰撞分层枚举(环境、己方载具、敌方载具、子弹等)
    Infantry,             // 步兵本体核心组件(阵营+装甲配置)
    InfantryChassis,      // 步兵底盘组件
    InfantryGimbal,      // 云台组件
    InfantryLaunchOffset, // 子弹发射挂点标记
    InfantryViewOffset,   // 第一视角相机挂点标记
    MainCamera,           // 主跟随相机标记
    PreciousCollision,    // 场景碰撞配置容器：子物体名称 → 碰撞规则
    SlapperInfantry,      // AI自动击打敌方的机器人标记
};
// 全局仿真配置
use crate::config::SimulationConfig;
// RM规则体系：阵营、装甲扫描、前哨/能量机关/基地根标记、英雄装甲配置
use crate::robomaster::prelude::{
    HERO_ROBOT_CONFIG, INFANTRY_THREE_CONFIG, OutpostRoot, PowerRuneRoot, ScanArmor, Team,
    TechCoreRoot,
};
// 载具动力学物理组件
use crate::robomaster::vehicle::movement::VehicleDynamic;
// 生成屏幕文字工具函数
use crate::systems::spawn_text;
// 层级遍历查询工具（递归遍历实体子孙）
use crate::util::entity_query::HierarchyQuery;

/// 标记前哨站根实体，场景就绪后遍历子物体绑定红蓝前哨阵营
#[derive(Component)]
pub struct ScanOutpost;

/// 场景入口初始化系统：世界光照、地面场景、基地、前哨、机器人、主相机全部在此生成
pub fn setup(
    // 命令系统：用于生成实体、添加组件、设置变换等操作把东西放进world
    mut commands: Commands,
    /*1. Res<T>
    Bevy 资源专用包裹类型，代表只读访问全局资源；
    全局资源不属于任何实体，是全局单例，整个 App 里仅有一份 AssetServer；
    Res<T>：只读，不会标记资源发生变更，性能更高；
    如果写成 ResMut<AssetServer> 则是可变借用，可以修改资源。
    2. AssetServer
    Bevy 内置核心资源，职责：
    加载各类外部资源：图片、材质、模型、GLTF、音频、字体、着色器、场景；
    路径解析、异步加载、缓存资源、避免重复加载同一文件；
    加载完成后返回资源句柄 Handle<T>，后续依靠句柄绑定到实体上。 */
    asset_server: Res<AssetServer>,
    config: Res<SimulationConfig>,
    egui_global_settings: Option<ResMut<EguiGlobalSettings>>,
) {
    // 解构可选资源：只有项目启用了 bevy_egui 插件、存在 EguiGlobalSettings 全局资源时才进入分支
    // mut 代表拿到该全局配置的可变引用，可以修改配置字段
    if let Some(mut egui_global_settings) = egui_global_settings {
        // 关闭 egui 自动创建主上下文的行为
        egui_global_settings.auto_create_primary_context = false;
    }

    // 生成全局UI文字层（帧率、调试文字、状态提示）
    spawn_text(&mut commands);

    // 全局太阳光（平行光）
    commands.spawn((
        // 平行光组件：太阳光
        DirectionalLight {
            // 光源色彩：冷白色 (R0.9, G0.95, B1.0)，偏淡蓝白光，模拟白天日光
            color: Color::srgb(0.9, 0.95, 1.0),
            // 光照亮度，不从硬编码写死，读取配置文件里渲染配置的亮度数值，方便改配置调亮度不用改代码
            illuminance: config.render.illuminance,
            // 是否开启投射阴影：同样由配置控制，调试开阴影，比赛为了性能关掉阴影
            shadows_enabled: config.render.shadows,
            // DirectionalLight 其余属性使用默认值
            ..default()
        },
        // 光源位置 + 朝向
        /*⚠️ 平行光**位置本身不影响光照！**平行光只关心方向，放哪里光线都是平行的。
        这里把光源放在 (0,4,0) 只是方便理解，真正起作用的是 `looking_at` 算出的光线方向。 */
        Transform::from_xyz(0.0, 4.0, 0.0)
            // 看向世界坐标原点 (0,0,0)
            .looking_at(Vec3::ZERO, Vec3::new(1.0, 1.0, 1.0)),
    ));

    // ===================== 环境碰撞分层规则 =====================
    // 环境碰撞层：自身归属环境层，可以被 地面、己方车辆、敌方车辆、敌我子弹 碰撞检测到
    // 定义环境（地面、场景墙体、静态场地）的碰撞层级规则
    /*
    Avian3D 的 `CollisionLayers::new(membership_mask, filter_mask)` 规则：
    > `membership_mask`：**我是谁**（这个碰撞体归属的层，可以多个）
    > `filter_mask`：**我能和谁碰撞**（只要对方的 membership 在我的 filter 里面，双方就会产生碰撞） */
    let layer_env = CollisionLayers::new(
        // 第一层：自身所属层级 → 当前碰撞体属于【环境层 Environment】
        [GameLayer::Environment],
        // 第二层：可以和哪些层级发生碰撞
        [
            GameLayer::Default,
            GameLayer::VehicleSelf,     // 己方机器人
            GameLayer::VehicleOther,    // 敌方机器人
            GameLayer::ProjectileSelf,  // 己方子弹
            GameLayer::ProjectileOther, // 敌方子弹
        ],
    );

    // 闭包，延迟构造碰撞生成器，专门给地面、静态场景生成高精度碰撞体
    /*|| { ... } 无参闭包
    这个闭包就是：**一个现成的 “生成规则”，等加载 GLB 机甲模型的时候，自动照着模型的三角面片，给模型生成贴合外形的三角网格碰撞体。**
    它本身不会立刻干活，只是存好这套规则，模型加载完成后才执行。 */
    /*### 1. `ColliderConstructorHierarchy`

    作用：**遍历模型所有子节点（子网格）**。
    你的机甲 GLB 里面是分层的：底盘、云台、装甲片都是分开的子模型。这个组件会挨个找到每一个子网格，给每一部分都生成碰撞体，不是只处理模型根节点。

    ### 2. `ColliderConstructor::TrimeshFromMeshWithConfig(...)`

    意思：**拿模型自带的原始三角面片，生成三角网格碰撞体，并且可以带配置参数**。
    `TrimeshFromMesh` 是不带配置的简化版本；`TrimeshFromMeshWithConfig` 允许传入三角网格的标志位。

    ### 3. `TrimeshFlags::all()`

    `all()` = 把所有可用开关全部打开。Avian 里 TrimeshFlags 一共这几项：

    - `BACKFACE_CULLING`：背面剔除。三角形只有正面参与碰撞检测，背面不产生碰撞。比如模型内壁，子弹穿进去不会触发碰撞。
    - `COMPUTE_AABB`：预计算包围盒。提前算出这片三角网格的整体包围盒，加速碰撞粗筛，减少计算量。
    - `CONSERVATIVE`：保守碰撞检测，防止微小模型缝隙造成穿透。

    ### 4. 外层闭包 `|| { ... }`

    闭包在这里是**延迟构造**。
    不是程序启动立刻生成碰撞体。等 GLB 模型资源加载完成后，引擎才会执行这个闭包，读取已经加载好的网格数据生成碰撞。
    模型还没加载的时候，网格数据不存在，不能提前生成碰撞体，所以用闭包延迟。 */
    let trimesh = || {
        ColliderConstructorHierarchy::new(
            // 根据模型原始网格，自动生成三角面碰撞体
            ColliderConstructor::TrimeshFromMeshWithConfig(TrimeshFlags::all())
        )
    };

    // 闭包：生成【体素填充碰撞体】，适合镂空能量机关，避免内部空腔无法碰撞
    // 接收 size（体素尺寸，单位米）作为入参，返回一套「体素化三角碰撞生成规则」
    /*举例子：机甲装甲外壳是空心的模型。
    - 普通 Trimesh 三角网格：只有外壳一层薄三角面，**壳里面是空的**，子弹有可能穿过壳体进到模型内部。
    - 体素化 + 洪水填充开`detect_cavities`：引擎识别到装甲这个封闭空心区域，把装甲内部全部填满小方块。子弹打到装甲外壳就挡住，不会钻进模型空腔。 */
    let voxel = |size| {
        ColliderConstructorHierarchy::new(
            ColliderConstructor::VoxelizedTrimeshFromMesh {
                voxel_size: size, // 体素粒度，单位 m，体素越小碰撞越精细、性能开销越高
                fill_mode: FillMode::FloodFill {
                    detect_cavities: true, // 洪水填充模式：自动识别模型内部空腔，把空心模型内部填满碰撞体
                },
            }
        )
    };

    // ===================== 生成战场地面场景 GROUND.glb =====================
    commands.spawn((
        // 加载地面场景GLTF模型
        SceneRoot(asset_server.load("GROUND.glb#Scene0")),
        Transform::IDENTITY, // 放置在世界原点，无位移旋转缩放
        Friction::new(0.5),  // 地面摩擦系数0.5，机器人行驶不会过滑也不会过于卡顿
        // PreciousCollision：精细化分层碰撞规则，按子物体名称区分是否生成碰撞、用哪种碰撞规则
        PreciousCollision(HashMap::from([(
            // 只对模型内部名称叫 GROUND_DENSE 的子物体生效碰撞规则
            "GROUND_DENSE".to_string(),
            (
                trimesh(),        // 使用原版高精度三角网格碰撞体
                layer_env,        // 复用环境碰撞层级（地面能被机甲、子弹碰撞）
                Visibility::Visible,//
                Some(RigidBody::Static), // 静态刚体，固定不动，不会被机器人撞动
            ),
        )])),
    ));

    // 标定坐标系场景（校准用模型，无碰撞）
    commands.spawn((
        // 加载坐标系标定模型
        SceneRoot(asset_server.load("CALIB.glb#Scene0")),
        Transform::IDENTITY
            .with_scale(Vec3::splat(1.0))       // 缩放1倍，原始大小
            .with_translation(Vec3::new(1.0, 0.5, 1.0)), // 放置在世界坐标 (1, 0.5, 1)
    ));

    // 前哨站场景 + ScanOutpost标记，场景加载完成后自动绑定红蓝阵营
    commands.spawn((
        RigidBody::Static,
        SceneRoot(asset_server.load("OUTPOST.glb#Scene0")),
        Transform::IDENTITY,
        ScanOutpost,
    ));

    // 基地核心 TECH_CORE.glb，绑定基地根组件，地面部分生成静态碰撞
    commands.spawn((
        SceneRoot(asset_server.load("TECH_CORE.glb#Scene0")),
        Transform::IDENTITY,
        TechCoreRoot, // 基地根标记组件，代表这是一座基地本体
        PreciousCollision(HashMap::from([(
            // 只给模型内部名叫 GROUND 的子物体生成碰撞
            "GROUND".to_string(),
            (
                trimesh(),                // 使用高精度三角网格碰撞贴合基地地面
                layer_env,                // 环境碰撞层级：机甲、子弹均可碰撞
                Visibility::Visible,
                Some(RigidBody::Static),  // 静态地面刚体
            ),
        )])),
    ));

    // ===================== 能量机关 POWER.glb 碰撞配置 =====================
    // 构建能量机关整套子部件的碰撞规则映射表
    let mut power_rune_col = HashMap::from([(
        // 匹配模型内名为 BASE 的底座部件
        "BASE".to_string(),
        (
            trimesh(),                  // 底座用完整三角网格碰撞，贴合底座外形
            layer_env,                  // 环境碰撞层级：机甲、子弹均可碰撞
            Visibility::Visible,
            Some(RigidBody::Static),    // 底座设置静态刚体，固定不动
        ),
    )]);
    // 遍历所有能量机关激活面片，使用体素碰撞 0.015m粒度，不绑定刚体（跟随父节点静态）
    for i in 1..=2 {
        for j in 1..=5 {
            // 四种状态面片：已激活、激活中、完成、禁用
            for k in ["ACTIVATED", "ACTIVE", "COMPLETED", "DISABLED"] {
                power_rune_col.insert(
                    // 拼接子物体命名规则：FACE_组别_TARGET_序号_状态
                    format!("FACE_{}_TARGET_{}_{}", i, j, k).to_string(),
                    (
                        voxel(0.015),       // 体素化碰撞，体素尺寸 1.5 厘米
                        layer_env,
                        Visibility::Visible,
                        None,               // 不单独给面片创建刚体，继承父物体的 Static 刚体
                    ),
                );
            }
        }
    }
    // 生成能量机关场景
    commands.spawn((
        RigidBody::Static,                          // 父实体整体为静态刚体
        CollisionMargin(0.001),                     // 碰撞扩张余量 1mm，防止机关面片和子弹物理粘连卡住
        Restitution::ZERO,                          // 恢复系数=0，子弹打到机关完全不反弹、不会弹跳乱飞
        SceneRoot(asset_server.load("POWER.glb#Scene0")), // 加载能量机关模型
        Transform::IDENTITY,
        PowerRuneRoot,                              // 自定义标记：代表该实体是能量机关根节点
        PreciousCollision(power_rune_col),          // 传入上面配置好的碰撞规则表
    ));

    // ===================== 生成玩家操控 红方步兵3号 =====================
    commands.spawn((
        // 加载步兵机器人模型 vehicle.glb
        SceneRoot(asset_server.load("vehicle.glb#Scene0")),
        // 出生坐标 (0, 1, 0)，Y=1 抬高避免机器人出生卡在地里
        Transform::from_xyz(0.0, 1.0, 0.0),
        // 构造步兵实体：红方队伍、套用三号步兵配置参数（底盘速度、血量、射速、装甲尺寸等）
        Infantry::new(Team::Red, INFANTRY_THREE_CONFIG),
        Controlled, // 标记：这台机器人由本地玩家键盘/鼠标操控
    ));

    // AI蓝方步兵
    commands.spawn((
        SceneRoot(asset_server.load("vehicle.glb#Scene0")),
        Transform::from_xyz(1.0, 1.0, 1.0),
        Infantry::new(Team::Blue, INFANTRY_THREE_CONFIG),
        SlapperInfantry, // AI自动击打
    ));

    // AI蓝方英雄机器人，具备主动击打权限
    commands.spawn((
        SceneRoot(asset_server.load("HERO.glb#Scene0")),
        Transform::from_xyz(2.0, 1.0, 1.0),
        Infantry::new(Team::Blue, HERO_ROBOT_CONFIG),
        SlapperInfantry,
        ActiveSlapper,
    ));

    // ===================== 主相机生成 =====================
    // 生成主3D相机实体
    let mut main_camera = commands.spawn((
        Camera3d::default(),
        Camera {
            // 条件编译：编译开启 ros2 / talos 特性时，主相机直接禁用渲染
            #[cfg(any(feature = "ros2", feature = "talos"))]
            is_active: false,
            // 不开启ROS/共享内存采集时，相机是否启用由配置文件 preview.enabled 控制
            #[cfg(not(any(feature = "ros2", feature = "talos")))]
            is_active: config.preview.enabled,
            ..default()
        },
        // 透视投影配置
        Projection::Perspective(PerspectiveProjection {
            fov: config.camera.fov.to_radians(), // 视角FOV从配置读取，角度转弧度
            near: 0.1,          // 近裁剪面：距离相机小于0.1m的物体不会渲染，避免贴脸模型错乱
            far: 500000000.0,  // 远裁剪面极大，远处场地、物体不会被裁切掉消失
            ..default()
        }),
        Tonemapping::None, // 关闭HDR色调映射，画面原生色彩，方便视觉算法做颜色识别
        Msaa::Off,         // 关闭硬件多重采样抗锯齿，后续靠FXAA后处理抗锯齿，性能开销更低
        // 相机初始位置：(0,10,15) 高空俯视角，看向世界原点场地中心
        Transform::from_xyz(0.0, 10.0, 15.0).looking_at(Vec3::new(0.0, 0.0, 0.0), Vec3::Y),
        // 自定义组件：标记为主相机，存储跟随偏移量，后续实现跟随机器人视角
        MainCamera {
            follow_offset: Vec3::from_array(config.camera.follow_offset),
        },
    ));

    // 如果渲染配置开启 FXAA，给主相机挂载 FXAA 后处理抗锯齿组件
    if config.render.main_camera_fxaa {
        main_camera.insert(Fxaa::default());
    }

    // 开启调试Egui面板时，将当前主相机设为 Egui 的默认渲染上下文
    if config.debug.egui {
        main_camera.insert(PrimaryEguiContext);
    }

    // 编译开启 ros2 / talos（共享内存画面采集）特性时，挂载画面捕获组件
    #[cfg(any(feature = "ros2", feature = "talos"))]
    main_camera.insert(crate::capture::CaptureSource);
}

/// OUTPOST场景加载完毕回调：遍历子物体 OUTPOST_1/OUTPOST_2 绑定红蓝前哨阵营组件
/// 场景加载完毕后触发，解析前哨站场景子物体，给红蓝前哨打上阵营标记
pub fn setup_ground(
    // 触发条件：某个场景实体完成加载实例化事件 SceneInstanceReady
    events: On<SceneInstanceReady>,
    mut commands: Commands,
    //查询实体的子节点，用来递归遍历所有子物体。`iter_descendants` = 把所有子、孙物体全部遍历一遍。
    children: Query<&Children>,
    //查询实体的名字（GLB 模型里面子物体的名字）。
    name: Query<&Name>,
    // 查询全局唯一带有 ScanOutpost 组件的实体（前哨站根物体）
    ground: Single<Entity, With<ScanOutpost>>,
) {
    // 拿到本次完成加载的场景实体
    let root = events.entity;

    // 校验：只处理【带有 ScanOutpost 标记的前哨站根节点】，其它场景加载事件直接忽略
    if ground.into_inner() != root {
        return;
    }

    // 递归遍历该前哨场景下所有子孙子物体
    children.iter_descendants(root).for_each(|e| {
        // 获取子物体的名称，取不到名称直接跳过
        let Ok(name) = name.get(e) else {
            return;
        };

        // 模型内命名 OUTPOST_1 → 判定为红色前哨，挂载红方前哨根组件
        if name.as_str() == "OUTPOST_1" {
            commands.entity(e).insert(OutpostRoot::new(Team::Red));
        }
        // OUTPOST_2 = 蓝色阵营前哨
        if name.as_str() == "OUTPOST_2" {
            commands.entity(e).insert(OutpostRoot::new(Team::Blue));
        }
    })
}

/// 机器人GLB场景加载完成回调：挂载物理刚体、底盘/云台、发射口、装甲扫描组件、分层碰撞
/// 机器人模型场景加载完毕后自动装配机器人物理、碰撞、业务组件
pub fn setup_vehicle(
    // 事件：任意场景实例加载完成时触发此系统
    events: On<SceneInstanceReady>,
    // ECS指令管理器，用来给实体新增/删除组件
    mut commands: Commands,
    // 层级查询工具：遍历父实体的所有子孙子节点
    query: HierarchyQuery,
    // 主查询：筛选【场景根实体】，取出 实体ID、Infantry机器人核心组件、是否本机操控、是否主动进攻AI
    root_query: Query<(
        Entity,
        &Infantry,
        Option<&Controlled>,
        Option<&ActiveSlapper>,
    )>,
    // 未使用的查询占位，用来规避 Bevy 未使用参数编译警告，无业务作用
    _secondary_query: Query<&ChildOf, (Without<Infantry>, Without<SceneInstance>)>,
    // 未使用的名称+父子关系查询，占位防警告
    _node_query: Query<(&Name, &ChildOf), (Without<Infantry>, Without<SceneInstance>)>,
    // 全局仿真配置资源（车辆最大速度、加速度等参数）
    sim_config: Res<SimulationConfig>,
) {
    // 获取本次刚刚加载完毕的场景根实体ID
    let root = events.entity;

    // 过滤：只有场景根实体挂载了 Infantry（机器人核心组件），才是机器人模型，继续执行；其它场景直接退出
    if root_query.get(root).is_err() {
        return;
    }

    // 解包根实体数据：实体ID、机器人属性、是否玩家操控、是否进攻型AI
    let (root, infantry, is_local_ctrl, has_active_slapper) = root_query.get(root).unwrap();
    // 读取机器人所属红蓝阵营
    let team = infantry.team;
    // 读取机器人配置（血量、装甲、射速等）
    let config = infantry.config;
    // bool：true=本机玩家操控的己方机器人
    let is_local = is_local_ctrl.is_some();
    // bool：true=主动进攻型AI（敌方英雄机器人）
    let is_active = has_active_slapper.is_some();

    // ===================== 步骤1：给机器人所有子模型批量打上操控/AI标签 =====================
    if is_local {
        // 本机操控机器人：递归遍历机器人所有子部件，全部挂上 Controlled 标记
        // 后续输入系统、相机跟随系统识别 Controlled，只操控本机机甲
        query.children.iter_descendants(root).for_each(|child_entity| {
            commands.entity(child_entity).insert(Controlled);
        });
    } else {
        // AI机器人分支：所有子节点挂载 SlapperInfantry（AI自动作战基础标记）
        query.children.iter_descendants(root).for_each(|child_entity| {
            commands.entity(child_entity).insert(SlapperInfantry);
            // 如果是进攻型AI，额外追加 ActiveSlapper，赋予主动冲锋进攻逻辑
            if is_active {
                commands.entity(child_entity).insert(ActiveSlapper);
            }
        });
    }

    // ===================== 步骤2：设置机器人专属碰撞分层规则 =====================
    // 己方机器人归属层级：VehicleSelf；敌方AI机器人归属：VehicleOther
    let vehicle_layers = if is_local {
        GameLayer::VehicleSelf
    } else {
        GameLayer::VehicleOther
    };

    // 该机器人能够发生碰撞的层级白名单
    let vehicle_filters = [
        GameLayer::Default,        // 默认通用物体
        GameLayer::VehicleSelf,    // 己方机器人
        GameLayer::VehicleOther,  // 敌方机器人
        GameLayer::ProjectileOther,// 敌方发射的子弹（敌方子弹可以打中自己）
        GameLayer::Environment,   // 地面、围墙、环境
    ];

    // 构造 Rapier 碰撞层规则：自身属于 vehicle_layers，只和 vehicle_filters 内层级碰撞
    let vehicle_collision_layers = CollisionLayers::new(vehicle_layers, vehicle_filters);

    // ===================== 步骤3：给机器人根实体挂载完整物理刚体配置 =====================
    commands.entity(root).insert((
        RigidBody::Dynamic, // 动态刚体：受重力、摩擦力、外力驱动，可以自由移动（机器人可行驶）

        // 自定义载具动力学组件：存储底盘运动参数，底盘控制系统依靠该组件实现加减速
        VehicleDynamic::new(
            sim_config.vehicle.max_speed,          // 最大行驶速度
            sim_config.vehicle.linear_acceleration,// 线性加速度
            sim_config.vehicle.acceleration_exponent,// 加速度曲线指数，用来实现起步顺滑、高速加减速放缓
        ),

        // 复合碰撞体：机器人整体碰撞外形 = 圆柱体胶囊，贴合战车外形，性能远优于三角网格碰撞
        Collider::compound(vec![(
            // 碰撞体相对机器人中心的偏移
            Vec3::new(0.0, -0.115649, 0.0),
            Quat::IDENTITY,
            // 圆柱体：半径0.259m，高度0.231m，模拟战车底盘轮廓
            Collider::cylinder(0.2593615, 0.231298),
        )]),

        CollisionMargin(0.005), // 碰撞体向外扩充5mm安全余量，防止高速行驶时机器人卡在墙体缝隙里
        vehicle_collision_layers,// 绑定上面定义的碰撞层级规则
        Mass(15.0),             // 机器人整体质量 15kg，匹配RM步兵机器人重量
        Restitution::new(0.01), // 恢复系数仅0.01，几乎没有弹性，撞到墙壁不会弹跳乱晃
        AngularDamping(50.0),   // 极高角阻尼，强力抑制车身自转、侧翻、抖动，保证行驶平稳
    ));

    // ===================== 步骤4：机器人所有子模型继承相同碰撞层级 =====================
    // 机器人身上所有零件、装甲、云台都共用一套碰撞规则，避免子部件碰撞层级错乱
    query.children.iter_descendants(root).for_each(|child_e| {
        commands.entity(child_e).insert(vehicle_collision_layers);
    });

    // ===================== 步骤5：检索模型内命名节点，给底盘、云台绑定业务组件 =====================
    // 查找机器人模型里名为 VEHICLE 的节点，再向下精确匹配 BASE（底盘）、GIMBAL（云台）
    let iter = query.of(root).any().exact("VEHICLE").flatten();
    // 精准拿到底盘实体（模型内名称 BASE）
    let base_entity = iter.clone().exact("BASE").one().unwrap();

    // 底盘挂载组件：
    // InfantryChassis：底盘空标记组件；
    // ScanArmor：装甲命中检测器，子弹碰撞到底盘装甲时，用来判定命中部位、扣血量、装甲强度
    commands.entity(base_entity).insert((
        InfantryChassis::default(),
        ScanArmor::new(team, config.armor),
    ));

    // 精准拿到云台实体（模型内名称 GIMBAL）
    let gimbal_entity = iter.exact("GIMBAL").one().unwrap();
    // 标记该实体为机器人云台，云台旋转控制系统识别此组件控制俯仰yaw
    commands.entity(gimbal_entity).insert(InfantryGimbal::default());

    // ===================== 步骤6：仅本机玩家操控机甲，挂载发射口、相机视角点位 =====================
    if is_local {
        let gimbal_child_iter = query.of(gimbal_entity).flatten();

        // 找到云台内 SHOT_DIRECTION 发射点，挂载发射偏移组件，子弹从此坐标生成发射
        commands
            .entity(gimbal_child_iter.clone().exact("SHOT_DIRECTION").one().unwrap())
            .insert(InfantryLaunchOffset);

        // 找到云台 CAM_DIRECTION 视角挂载点，相机跟随系统以此点位作为跟随视角原点
        commands
            .entity(gimbal_child_iter.exact("CAM_DIRECTION").one().unwrap())
            .insert(InfantryViewOffset);
    }
}


/// 通用场景碰撞生成回调：SceneInstanceReady 后读取 PreciousCollision 配置，给指定命名子物体生成碰撞体
/// 场景GLTF/GLB加载完成后，根据 PreciousCollision 内的名称映射规则，自动给子物体生成碰撞体
pub fn setup_collision(
    // 触发事件：任意场景实体实例加载完毕
    events: On<SceneInstanceReady>,
    // ECS 指令管理器，增删实体组件
    mut commands: Commands,
    // 层级查询：遍历某个父实体所有后代子节点
    children: Query<&Children>,
    // 查询实体名称；仅查询「拥有子物体」的实体，过滤叶子节点优化性能
    name: Query<&Name, With<Children>>,
    // 查询场景根实体：必须携带 PreciousCollision 碰撞配置组件才会处理
    root_query: Query<(Entity, &PreciousCollision)>,
) {
    // 取出本次加载完成的场景实体，尝试获取它身上的 PreciousCollision 碰撞配置表
    // 场景没有 PreciousCollision 组件 → 直接退出，不处理碰撞
    let Ok((_, PreciousCollision(map))) = root_query.get(events.entity) else {
        return;
    };

    // 递归遍历当前场景根节点下所有子子孙物体
    for child_entity in children.iter_descendants(events.entity) {
        // 获取当前子物体的名字，拿不到名称就跳过该物体
        let Ok(obj_name) = name.get(child_entity) else {
            continue;
        };

        // 用物体名称去碰撞规则哈希表查找对应的碰撞配置：
        // 配置 = (碰撞生成器, 碰撞层级, 可见性, 是否单独挂载刚体)
        if let Some((constructor, layer, visibility, rigid_opt)) = map.get(&obj_name.to_string())
        {
            // 分支1：配置要求该子物体单独挂载刚体（如基地底座、地面实心板块）
            if let Some(rigid_body_type) = rigid_opt {
                commands
                    .entity(child_entity)
                    // 同时挂载：刚体类型 + 碰撞构造规则 + 碰撞层级规则
                    .insert((*rigid_body_type, constructor.clone(), *layer));
            } else {
                // 分支2：None = 不创建独立刚体，子物体依附父实体的刚体
                // 能量机关击打面板、装饰碰撞面片走这个分支，避免多层刚体嵌套引发物理BUG
                commands.entity(child_entity).insert((constructor.clone(), *layer));
            }

            // 如果配置要求该部件隐藏，则设置 Visibility::Hidden
            if visibility == &Visibility::Hidden {
                commands.entity(child_entity).insert(*visibility);
            }
        }
    }

    // 碰撞规则已经全部应用完毕，删除根实体上的 PreciousCollision 配置组件
    // 防止下一帧再次触发场景事件时重复生成碰撞、造成重复挂载组件、内存冗余
    commands.entity(events.entity).remove::<PreciousCollision>();
}