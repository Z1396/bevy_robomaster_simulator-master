// 引入3D物理引擎Avian3D全部常用类型（刚体、碰撞体、速度、受力等物理组件）
use avian3d::prelude::*;
// 引入Bevy游戏引擎基础：命令队列、资源、网格、材质、时间、变换、查询等核心API
use bevy::prelude::*;
// 导入常量 π，用于计算弹丸迎风截面积
use core::f32::consts::PI;

// 引入当前crate内部自定义组件
use crate::components::{
    Controlled,              // 标记实体是玩家操控的己方步兵战车
    GameLayer,               // 碰撞分层枚举（区分子弹、己方战车、敌方、环境，防止子弹击中自己）
    Infantry,                // 步兵战车主体标记组件
    InfantryChassis,         // 战车底盘组件
    InfantryGimbal,          // 云台组件
    InfantryLaunchOffset,    // 发射口偏移组件：记录炮口相对云台的安装位置
    ProjectileCooldown,      // 发射冷却计时器全局资源
    ProjectileLifetime,      // 子弹存活生命周期计时器组件
    ProjectileSetting,       // 全局资源：子弹共用的球体网格、发光材质（所有子弹复用一套材质节省显存）
};
use crate::config::SimulationConfig; // 全局仿真配置结构体（就是你前面解析TOML的配置）
use crate::robomaster::prelude::Projectile; // 子弹标记组件，打上此组件代表实体是弹丸
use crate::statistic::ProjectileStatistics; // 子弹发射统计全局资源（发射计数）

// ====================== 系统1：初始化子弹全局材质与网格资源 ======================
/// 启动阶段执行一次，创建子弹共用球体模型+发光材质，存入全局资源，避免每发子弹新建材质造成性能浪费
pub fn setup_projectile(
    mut commands: Commands,                // ECS命令队列，用来插入全局资源、生成实体
    config: Res<SimulationConfig>,         // 只读获取全局仿真配置（弹丸直径参数）
    mut meshes: ResMut<Assets<Mesh>>,      // 可变资源：全局网格资源管理器，存放所有模型
    mut materials: ResMut<Assets<StandardMaterial>>, // 可变材质管理器
) {
    // 将子弹模型+材质打包为全局资源 ProjectileSetting
    commands.insert_resource(ProjectileSetting(
        // 创建球体网格：半径 = 弹丸直径 / 2
        meshes.add(Sphere::new(config.projectile.diameter / 2.0)),
        materials.add(StandardMaterial {
            base_color: Color::srgba(0.132866, 1.0, 0.132869, 0.85), // 主体淡绿色
            emissive: LinearRgba::new(0.132866, 1.0, 0.132869, 0.85), // 自发光颜色，子弹发亮
            emissive_exposure_weight: -1.0, // 调低曝光，发光不会过度泛白
            alpha_mode: AlphaMode::Opaque,  // 不透明材质
            ..default() // 其余材质参数使用默认值
        }),
    ));
}

// ====================== 系统2：发射子弹逻辑（每一帧持续运行） ======================
/// 检测发射冷却、获取云台朝向、生成子弹实体、赋予物理属性
pub fn projectile_launch(
    time: Res<Time>,                               // 全局时间资源，获取帧间隔delta_time
    mut cooldown: ResMut<ProjectileCooldown>,     // 可变全局发射冷却计时器
    mut stats: ResMut<ProjectileStatistics>,     // 可变子弹统计器，记录发射次数
    config: Res<SimulationConfig>,                // 仿真配置：弹速、质量、摩擦系数等
    _asset_server: Res<AssetServer>,               // 未使用的资源服务器，下划线规避未使用警告
    mut commands: Commands,                       // 命令队列，用于生成子弹实体
    setting: Res<ProjectileSetting>,              // 子弹全局材质网格资源
    // 查询【玩家操控的步兵本体】：底盘变换、底盘线速度、底盘角速度
    // Single：确保只查到唯一一台被控战车，避免多车冲突
    infantry: Single<
        (&Transform, &LinearVelocity, &AngularVelocity),
        (With<Infantry>, With<Controlled>),
    >,
    // 查询【玩家操控的云台】：全局世界变换、云台专属组件
    gimbal: Single<
        (&GlobalTransform, &InfantryGimbal),
        (With<Controlled>, Without<InfantryChassis>),
    >,
    // 查询【炮口偏移点位】：炮口相对云台的局部坐标
    launch_offset: Single<&Transform, (With<Controlled>, With<InfantryLaunchOffset>)>,
) {
    // 冷却计时器推进时间
    cooldown.tick(time.delta());
    // 冷却没结束，直接退出本帧，禁止发射
    if !cooldown.is_finished() {
        return;
    }
    // 冷却走完，重置冷却计时器，开启新一轮冷却
    cooldown.reset();

    // 发射次数 +1，用于对战数据统计
    stats.increase_launch();

    // 计算炮口朝前的发射方向向量
    // gimbal.0.rotation()：云台在世界中的旋转四元数
    // launch_offset.rotation：炮口自身额外旋转修正
    // mul_vec3(Vec3::Y)：以局部Y轴作为炮口朝前方向
    // normalize_or_zero：归一化方向向量；如果是零向量直接返回零防止除以0
    let direction = (gimbal.0.rotation() * launch_offset.rotation)
        .mul_vec3(Vec3::Y)
        .normalize_or_zero();

    // 方向向量无效，不生成子弹
    if direction == Vec3::ZERO {
        return;
    }

    // 子弹绝对初速度 = 战车自身移动速度 + 枪口朝向 × 弹丸初速度
    // 物理真实逻辑：车在动时打出的子弹会继承车体速度
    let vel = infantry.1.0 + direction * config.projectile.speed;

    // 生成子弹实体，挂载一系列物理、渲染、生命周期组件
    commands.spawn((
        RigidBody::Dynamic, // 动态刚体，受重力、碰撞、外力影响
        // 球形碰撞体，半径与弹丸实际直径一致
        Collider::sphere(config.projectile.diameter / 2.0),
        Mass(config.projectile.mass), // 子弹质量
        Friction::new(config.projectile.friction), // 碰撞摩擦力
        Restitution::ZERO, // 恢复系数0，子弹击中装甲不反弹
        LinearDamping(config.projectile.linear_damping), // 基础线性速度阻尼
        // 碰撞层规则：本子弹属于ProjectileSelf层；可以碰撞敌方战车、敌方子弹、场景环境，不会击中己方
        CollisionLayers::new(
            GameLayer::ProjectileSelf,
            [
                GameLayer::Default,
                GameLayer::VehicleOther,
                GameLayer::ProjectileOther,
                GameLayer::Environment,
            ],
        ),
        Mesh3d(setting.0.clone()),        // 绑定全局球体网格
        MeshMaterial3d(setting.1.clone()), // 绑定全局绿色发光材质
        LinearVelocity(vel),              // 赋予子弹初始飞行速度
        AngularVelocity(infantry.2.0),    // 继承战车自转的角速度，子弹附带自旋
        // 设置子弹生成的世界坐标：战车位置 + 云台旋转后的炮口偏移位置
        Transform::IDENTITY.with_translation(
            infantry.0.translation + (gimbal.0.rotation() * launch_offset.translation),
        ),
        // 子弹生命周期计时器，到达时间自动销毁
        ProjectileLifetime(Timer::from_seconds(
            config.projectile.lifetime,
            TimerMode::Once,
        )),
        Projectile, // 打上子弹标记组件，方便其他系统批量查询所有子弹
    ));
}

// ====================== 系统3：子弹空气阻力空气动力学系统（每一帧运行） ======================
/// 给飞行子弹施加空气拖拽阻力、模拟风场，实现子弹下坠、减速，还原真实弹道
pub fn projectile_aerodynamics(
    config: Res<SimulationConfig>,
    mut projectiles: Query<Forces, With<Projectile>>, // 查询所有子弹实体的受力组件
) {
    let aero = &config.projectile.aerodynamics;
    // 配置关闭空气动力学，直接退出
    if !aero.enabled {
        return;
    }

    let diameter = config.projectile.diameter;
    if diameter <= 0.0 {
        return;
    }
    // 空气密度、风阻系数强制下限0，防止负数造成推力反向
    let air_density = aero.air_density.max(0.0);
    let drag_coefficient = aero.drag_coefficient.max(0.0);
    // 空气密度/阻力系数为0 = 无空气阻力，无需计算
    if air_density == 0.0 || drag_coefficient == 0.0 {
        return;
    }

    // 计算球形子弹迎风截面积 S = π * r²
    let area = PI * (diameter * 0.5).powi(2);
    // 获取全局风速向量 [x,y,z]
    let wind = Vec3::new(aero.wind[0], aero.wind[1], aero.wind[2]);
    // 阻力公式前置系数：k = 0.5 * ρ * Cd * S
    let k = 0.5 * air_density * drag_coefficient * area;

    // 遍历每一颗子弹施加阻力
    for mut forces in projectiles.iter_mut() {
        // 子弹相对空气的速度 = 子弹自身速度 - 风速
        let v_rel = forces.linear_velocity() - wind;
        let speed = v_rel.length();
        // 速度几乎为0，忽略阻力计算
        if speed <= 1e-3 {
            continue;
        }
        // 空气阻力公式：F_drag = -k * |v| * v
        // 负号代表阻力方向与相对速度相反
        forces.apply_force(-k * speed * v_rel);
    }
}

// ====================== 系统4：过期子弹清理系统（每一帧运行） ======================
/// 遍历所有子弹，生命周期计时器走完后销毁实体，防止场景堆积大量子弹造成内存暴涨
pub fn cleanup_projectiles(
    time: Res<Time>,
    mut commands: Commands,
    mut projectiles: Query<(Entity, &mut ProjectileLifetime)>,
) {
    // 遍历全部子弹实体 + 生命周期计时器
    for (entity, mut lifetime) in &mut projectiles {
        // 计时器推进帧间隔时间
        lifetime.tick(time.delta());
        // 寿命耗尽，销毁该子弹实体
        if lifetime.is_finished() {
            commands.entity(entity).despawn();
        }
    }
}