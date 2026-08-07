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
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    config: Res<SimulationConfig>,
    egui_global_settings: Option<ResMut<EguiGlobalSettings>>,
) {
    // 关闭 Egui 自动创建全局上下文，由主相机手动挂载，避免多相机UI错乱
    if let Some(mut egui_global_settings) = egui_global_settings {
        egui_global_settings.auto_create_primary_context = false;
    }

    // 生成全局UI文字层（帧率、调试文字、状态提示）
    spawn_text(&mut commands);

    // 全局太阳光（平行光）
    commands.spawn((
        DirectionalLight {
            color: Color::srgb(0.9, 0.95, 1.0), // 偏冷白光
            illuminance: config.render.illuminance, // 光照亮度读取配置文件
            shadows_enabled: config.render.shadows, // 是否开启阴影
            ..default()
        },
        // 光源高度4米，朝向原点
        Transform::from_xyz(0.0, 4.0, 0.0).looking_at(Vec3::ZERO, Vec3::new(1.0, 1.0, 1.0)),
    ));

    // ===================== 环境碰撞分层规则 =====================
    // 环境碰撞层：自身归属环境层，可以被 地面、己方车辆、敌方车辆、敌我子弹 碰撞检测到
    let layer_env = CollisionLayers::new(
        [GameLayer::Environment],
        [
            GameLayer::Default,
            GameLayer::VehicleSelf,
            GameLayer::VehicleOther,
            GameLayer::ProjectileSelf,
            GameLayer::ProjectileOther,
        ],
    );

    // 闭包：生成【完整三角网格碰撞体】（贴合模型外形，用于地面、静态场景）
    let trimesh = || {
        ColliderConstructorHierarchy::new(ColliderConstructor::TrimeshFromMeshWithConfig(
            TrimeshFlags::all(),
        ))
    };

    // 闭包：生成【体素填充碰撞体】，适合镂空能量机关，避免内部空腔无法碰撞
    let voxel = |size| {
        ColliderConstructorHierarchy::new(ColliderConstructor::VoxelizedTrimeshFromMesh {
            voxel_size: size, // 体素粒度 单位m
            fill_mode: FillMode::FloodFill {
                detect_cavities: true, // 自动识别空腔并填充碰撞
            },
        })
    };

    // ===================== 生成战场地面场景 GROUND.glb =====================
    commands.spawn((
        SceneRoot(asset_server.load("GROUND.glb#Scene0")),
        Transform::IDENTITY,
        Friction::new(0.5), // 地面摩擦系数
        // 碰撞配置：只有名字为 GROUND_DENSE 的子物体生成静态三角网格碰撞
        PreciousCollision(HashMap::from([(
            "GROUND_DENSE".to_string(),
            (
                trimesh(),
                layer_env,
                Visibility::Visible,
                Some(RigidBody::Static), // 静态刚体
            ),
        )])),
    ));

    // 标定坐标系场景（校准用模型，无碰撞）
    commands.spawn((
        SceneRoot(asset_server.load("CALIB.glb#Scene0")),
        Transform::IDENTITY
            .with_scale(Vec3::splat(1.0))
            .with_translation(Vec3::new(1.0, 0.5, 1.0)),
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
        TechCoreRoot,
        PreciousCollision(HashMap::from([(
            "GROUND".to_string(),
            (
                trimesh(),
                layer_env,
                Visibility::Visible,
                Some(RigidBody::Static),
            ),
        )])),
    ));

    // ===================== 能量机关 POWER.glb 碰撞配置 =====================
    let mut power_rune_col = HashMap::from([(
        "BASE".to_string(),
        (
            trimesh(),
            layer_env,
            Visibility::Visible,
            Some(RigidBody::Static),
        ),
    )]);
    // 遍历所有能量机关激活面片，使用体素碰撞 0.015m粒度，不绑定刚体（跟随父节点静态）
    for i in 1..=2 {
        for j in 1..=5 {
            for k in ["ACTIVATED", "ACTIVE", "COMPLETED", "DISABLED"] {
                power_rune_col.insert(
                    format!("FACE_{}_TARGET_{}_{}", i, j, k).to_string(),
                    (voxel(0.015), layer_env, Visibility::Visible, None),
                );
            }
        }
    }
    // 生成能量机关场景
    commands.spawn((
        RigidBody::Static,
        CollisionMargin(0.001), // 碰撞余量防止粘连
        Restitution::ZERO,      // 完全无弹性，不会弹跳
        SceneRoot(asset_server.load("POWER.glb#Scene0")),
        Transform::IDENTITY,
        PowerRuneRoot,
        PreciousCollision(power_rune_col),
    ));

    // ===================== 生成玩家操控 红方步兵3号 =====================
    commands.spawn((
        SceneRoot(asset_server.load("vehicle.glb#Scene0")),
        Transform::from_xyz(0.0, 1.0, 0.0),
        Infantry::new(Team::Red, INFANTRY_THREE_CONFIG),
        Controlled, // 本机操控标记
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
    let mut main_camera = commands.spawn((
        Camera3d::default(),
        Camera {
            // 如果开启ROS2/Talos图像采集，屏幕主相机禁用渲染，画面由离屏纹理UI贴图实现，避免两次渲染浪费性能
            #[cfg(any(feature = "ros2", feature = "talos"))]
            is_active: false,
            #[cfg(not(any(feature = "ros2", feature = "talos")))]
            is_active: config.preview.enabled,
            ..default()
        },
        // 透视投影，FOV从配置读取转为弧度，远裁剪极大防止远处场景消失
        Projection::Perspective(PerspectiveProjection {
            fov: config.camera.fov.to_radians(),
            near: 0.1,
            far: 500000000.0,
            ..default()
        }),
        Tonemapping::None, // 关闭色调映射
        Msaa::Off,         // 关闭多重采样，改用FXAA后处理抗锯齿
        // 初始俯视角位置
        Transform::from_xyz(0.0, 10.0, 15.0).looking_at(Vec3::new(0.0, 0.0, 0.0), Vec3::Y),
        MainCamera {
            follow_offset: Vec3::from_array(config.camera.follow_offset),
        },
    ));

    // 配置开启FXAA则挂载抗锯齿
    if config.render.main_camera_fxaa {
        main_camera.insert(Fxaa::default());
    }
    // 调试模式下，主相机作为Egui渲染上下文
    if config.debug.egui {
        main_camera.insert(PrimaryEguiContext);
    }
    // ROS2/Talos采集模式挂载画面采集组件
    #[cfg(any(feature = "ros2", feature = "talos"))]
    main_camera.insert(crate::capture::CaptureSource);
}

/// OUTPOST场景加载完毕回调：遍历子物体 OUTPOST_1/OUTPOST_2 绑定红蓝前哨阵营组件
pub fn setup_ground(
    events: On<SceneInstanceReady>,
    mut commands: Commands,
    children: Query<&Children>,
    name: Query<&Name>,
    ground: Single<Entity, With<ScanOutpost>>,
) {
    let root = events.entity;
    // 只处理带 ScanOutpost 的场景实体
    if ground.into_inner() != root {
        return;
    }
    // 递归遍历所有子节点
    children.iter_descendants(root).for_each(|e| {
        let Ok(name) = name.get(e) else {
            return;
        };
        // OUTPOST_1 = 红方前哨
        if name.as_str() == "OUTPOST_1" {
            commands.entity(e).insert(OutpostRoot::new(Team::Red));
        }
        // OUTPOST_2 = 蓝方前哨
        if name.as_str() == "OUTPOST_2" {
            commands.entity(e).insert(OutpostRoot::new(Team::Blue));
        }
    })
}

/// 机器人GLB场景加载完成回调：挂载物理刚体、底盘/云台、发射口、装甲扫描组件、分层碰撞
pub fn setup_vehicle(
    events: On<SceneInstanceReady>,
    mut commands: Commands,
    query: HierarchyQuery,
    root_query: Query<(
        Entity,
        &Infantry,
        Option<&Controlled>,
        Option<&ActiveSlapper>,
    )>,
    _secondary_query: Query<&ChildOf, (Without<Infantry>, Without<SceneInstance>)>,
    _node_query: Query<(&Name, &ChildOf), (Without<Infantry>, Without<SceneInstance>)>,
    sim_config: Res<SimulationConfig>,
) {
    let root = events.entity;
    // 仅处理带有 Infantry 步兵根组件的场景
    if root_query.get(root).is_err() {
        return;
    }
    let (root, infantry, is_local, is_active) = root_query.get(root).unwrap();
    let team = infantry.team;
    let config = infantry.config;
    let is_local = is_local.is_some();
    let is_active = is_active.is_some();

    // 本机操控：所有子物体打上 Controlled
    if is_local {
        query.children.iter_descendants(root).for_each(|e| {
            commands.entity(e).insert(Controlled);
        });
    } else {
        // AI机器人：全部标记SlapperInfantry，可主动击打敌方
        query.children.iter_descendants(root).for_each(|e| {
            commands.entity(e).insert(SlapperInfantry);
            if is_active {
                commands.entity(e).insert(ActiveSlapper);
            }
        });
    }

    // 区分己方/敌方载具碰撞层
    let vehicle_layers = if is_local {
        GameLayer::VehicleSelf
    } else {
        GameLayer::VehicleOther
    };
    // 载具碰撞过滤规则：可以和环境、敌我车辆、敌方子弹发生碰撞
    let vehicle_filters = [
        GameLayer::Default,
        GameLayer::VehicleSelf,
        GameLayer::VehicleOther,
        GameLayer::ProjectileOther,
        GameLayer::Environment,
    ];
    let vehicle_collision_layers = CollisionLayers::new(vehicle_layers, vehicle_filters);

    // 给机器人根实体挂载动态刚体、载具动力学、圆柱复合碰撞体、物理参数
    commands.entity(root).insert((
        RigidBody::Dynamic,
        // 载具运动动力学（最大速度、加速度、加速度曲线指数）
        VehicleDynamic::new(
            sim_config.vehicle.max_speed,
            sim_config.vehicle.linear_acceleration,
            sim_config.vehicle.acceleration_exponent,
        ),
        // 复合碰撞体：圆柱胶囊碰撞模拟车身
        Collider::compound(vec![(
            Vec3::new(0.0, -0.115649, 0.0),
            Quat::IDENTITY,
            Collider::cylinder(0.2593615, 0.231298),
        )]),
        CollisionMargin(0.005),
        vehicle_collision_layers,
        Mass(15.0),                // 机器人质量15kg
        Restitution::new(0.01),    // 极小弹性，防止弹跳
        AngularDamping(50.0),      // 巨大角阻尼，抑制车身乱晃自转
    ));

    // 机器人所有子物体继承碰撞分层
    query.children.iter_descendants(root).for_each(|e| {
        commands.entity(e).insert(vehicle_collision_layers);
    });

    // 按模型节点名称查找 BASE底盘、GIMBAL云台
    let iter = query.of(root).any().exact("VEHICLE").flatten();
    let base = iter.clone().exact("BASE").one().unwrap();
    // 底盘挂载装甲扫描组件，击打装甲后用来判定命中部位、结算血量
    commands.entity(base).insert((
        InfantryChassis::default(),
        ScanArmor::new(team, config.armor),
    ));
    let gimbal = iter.exact("GIMBAL").one().unwrap();
    commands.entity(gimbal).insert(InfantryGimbal::default());

    // 本机操控才挂载发射口、视角偏移标记
    if is_local {
        let q = query.of(gimbal).flatten();
        commands
            .entity(q.clone().exact("SHOT_DIRECTION").one().unwrap())
            .insert(InfantryLaunchOffset);
        commands
            .entity(q.exact("CAM_DIRECTION").one().unwrap())
            .insert(InfantryViewOffset);
    }
}

/// 通用场景碰撞生成回调：SceneInstanceReady 后读取 PreciousCollision 配置，给指定命名子物体生成碰撞体
pub fn setup_collision(
    events: On<SceneInstanceReady>,
    mut commands: Commands,
    children: Query<&Children>,
    name: Query<&Name, With<Children>>,
    root_query: Query<(Entity, &PreciousCollision)>,
) {
    // 场景根实体必须携带碰撞配置资源 PreciousCollision
    let Ok((_, PreciousCollision(map))) = root_query.get(events.entity) else {
        return;
    };

    // 遍历所有子节点，匹配名称后生成碰撞
    for e in children.iter_descendants(events.entity) {
        let Ok(name) = name.get(e) else {
            continue;
        };
        if let Some((constructor, layer, visibility, rigid)) = map.get(&name.to_string()) {
            // 绑定刚体 + 碰撞构造器 + 碰撞层
            if let Some(rigid) = rigid {
                commands
                    .entity(e)
                    .insert((*rigid, constructor.clone(), *layer));
            } else {
                // 不单独创建刚体，继承父实体刚体
                commands.entity(e).insert((constructor.clone(), *layer));
            }
            // 配置隐藏则设置Visibility
            if visibility == &Visibility::Hidden {
                commands.entity(e).insert(*visibility);
            }
        }
    }
    // 配置用完删除，避免残留组件反复执行逻辑
    commands.entity(events.entity).remove::<PreciousCollision>();
}