// ============================================================================
// 模块名：ros2::plugin
// 作  用：ROS2 通信插件入口，整合所有 ROS2 话题发布/订阅与系统调度
// 职  责：
//   1. 注册并初始化 ROS2 节点（simulator），创建独立的 spin 线程驱动通信
//   2. 维护 TF 树发布（map -> odom -> gimbal_link -> muzzle/camera 等）
//   3. 处理外部订阅的云台控制指令（GimbalCmd），驱动云台旋转与开火
//   4. 发布能量机关（Power Rune）与装甲板（Armor）的可视化 Marker
//   5. 发布科技中心（TechCore）的状态 JSON
//   6. 在 AppExit 时安全停止 ROS2 线程，保证资源释放
// ============================================================================

use crate::arc_mutex;
use crate::capture::CaptureSource;
use crate::capture::driver::{CaptureConfig, CapturedFrameKind};
use crate::components::{
    Controlled, InfantryChassis, InfantryGimbal, InfantryLaunchOffset, SubscribeAutoAim,
};
use crate::config::SimulationConfig;
use crate::robomaster::prelude::{ArmorRoot, PowerRune, RuneIndex, TechCore, tech_core_state_json};
use crate::ros2::capture::{RosCaptureContext, RosCapturePlugin};
use crate::ros2::livox::{RosLivoxContext, RosLivoxPlugin};
use crate::ros2::prelude::AverageRateLimiter;
use crate::ros2::prelude::transform;
use crate::ros2::topic::*;
use crate::systems::projectile_launch;
use crate::util::entity_query::HierarchyQuery;
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use r2r::ClockType::SystemTime;
use r2r::geometry_msgs::msg::{Point, Pose, Vector3};
use r2r::std_msgs::msg::{ColorRGBA, String as RosString};
use r2r::visualization_msgs::msg::Marker;
use r2r::{Clock, Context, Node, std_msgs::msg::Header, tf2_msgs::msg::TFMessage};
use std::collections::HashMap;
use std::f32::consts::PI;
use std::time::Duration;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
};

/// 辅助宏：对 `(Arc<Mutex<T>>)` 形态的资源进行解包，返回内部 `T` 的可变借用。
/// 主要用于快速访问 `RoboMasterClock` 等被 `Mutex` 包裹的资源。
macro_rules! res_unwrap {
    ($res:tt) => {
        $res.0.lock().unwrap()
    };
}

/// 停止信号资源：通过 `Arc<AtomicBool>` 与 ROS2 spin 线程通信，
/// 当 AppExit 触发时置为 true，通知 spin 线程退出循环。
#[derive(Resource, Deref, DerefMut)]
struct StopSignal(Arc<AtomicBool>);

/// ROS2 spin 线程句柄资源：保存独立 spin 线程的 `JoinHandle`，
/// 用于在应用退出时 join 线程，确保 ROS2 资源被正确释放。
#[derive(Resource, Deref, DerefMut)]
struct SpinThreadHandle(Option<JoinHandle<()>>);

/// RoboMaster 时钟资源：使用 r2r 的 `Clock`（基于 SystemTime），
/// 为所有发布消息提供统一的时间戳，保证 TF 树与消息时间同步。
#[derive(Resource, Deref, DerefMut)]
pub struct RoboMasterClock(pub Arc<Mutex<Clock>>);

/// 开火频率限制器资源：限制 `projectile_launch` 系统的调用频率，
/// 避免外部 fire_advice 指令持续为 true 时导致每帧开火（默认 10Hz）。
#[derive(Resource, Deref, DerefMut)]
struct FireRateLimiter(AverageRateLimiter);

/// 科技中心状态发布频率限制器资源：限制 `/simulator/tech_core/state` 话题的发布频率（默认 20Hz）。
#[derive(Resource, Deref, DerefMut)]
struct TechCoreStateRateLimiter(AverageRateLimiter);

/// TF 树构造宏：以声明式语法描述坐标系父子关系，自动生成 `TransformStamped` 列表。
///
/// 用法示例：
/// ```ignore
/// tf_tree! {
///     stamp: <时间戳>;
///     "map" {
///         "odom" as (translation, rotation) for publisher {
///             "gimbal_link" as (...) { ... }
///         }
///     }
/// }
/// ```
/// - `stamp`：所有帧共用的时间戳
/// - 每个 `name as (translation, rotation) for publisher` 节点：
///   - 在 TF 树中添加一帧
///   - 同时通过 `publisher` 发布对应 `PoseStamped`（可选）
/// - 支持 `for ... in iter` 形式批量生成同构子节点（如能量机关、装甲板）
macro_rules! tf_tree {
    // 入口：初始化 transform_stamped 向量，从根坐标系开始递归构造
    (stamp: $stamp:expr;$root:literal { $($content:tt)* }) => {{
        let stamp = $stamp;
        let mut transform_stamped = vec![];
        let _parent = $root;
        let _current = $root;
        tf_tree!(@frame transform_stamped, stamp, _parent, _current, $($content)*);

        transform_stamped
    }};

    // 构造消息头：使用当前坐标系名称作为 frame_id
    (@header $stamp:ident, $current:ident) => {
        Header {
            stamp: $stamp.clone(),
            frame_id: $current.to_string(),
        }
    };

    // 单一子节点分支：声明 `name as (translation, rotation) for publisher { children }`
    (@frame $tf_vec:ident, $stamp:ident, $parent:ident, $current:ident,
        $curr_name:literal as ($translation:expr, $rotation:expr) $(for $pub_:ident)?
        {$($children:tt)*}
        $($remaining:tt)*
    ) => {
        {
            // 进入子节点：parent 指向当前帧，current 更新为新节点名
            let $parent = &$current;
            let $current = $curr_name;
            $crate::add_tf_frame!($tf_vec, tf_tree!(@header $stamp, $parent), $current, $translation, $rotation);
            // 若存在 publisher，则同步发布零偏移 PoseStamped（代表该坐标系原点）
            $(
                $pub_.publish($crate::pose!(tf_tree!(@header $stamp, $current)));
            )*
            // 递归处理子节点的 children
            tf_tree!(@frame $tf_vec, $stamp, $parent, $current, $($children)*);
        }
        // 继续处理同级剩余节点
        tf_tree!(@frame $tf_vec, $stamp, $parent, $current, $($remaining)*);
    };

    // 循环子节点分支：对迭代器中每个元素生成一个子节点（用于能量机关/装甲板等批量场景）
    (@frame $tf_vec:ident, $stamp:ident, $parent:ident, $current:ident,
    $(let $p_name:ident = $p_expr:expr;)*
        for ($($elem:tt),+$(,)?) in $iter:ident {
            $(let $name:ident = $expr:expr;)*
            pub $curr_name:ident as ($translation:expr, $rotation:expr) $(for $pub_:ident)?;
            $($children:tt)*
        }
        $($remaining:tt)*
    ) => {
        // 先求值循环外部的 let 绑定
        $(let $p_name = $p_expr;)*
        for ($($elem),+) in $iter {
            // 循环内部的 let 绑定
            $(let $name = $expr;)*
            let $parent = &$current;
            let $current = $curr_name;
            $crate::add_tf_frame!($tf_vec, tf_tree!(@header $stamp, $parent), $current, $translation, $rotation);
            $(
                $pub_.publish($crate::pose!(tf_tree!(@header $stamp, $current)));
            )*
            tf_tree!(@frame $tf_vec, $stamp, $parent, $current, $($children)*);
        }
        tf_tree!(@frame $tf_vec, $stamp, $parent, $current, $($remaining)*);
    };

    // 终止分支：处理空 children / 分隔符 / 结尾
    (@frame $tf_vec:ident, $stamp:ident, $parent:ident, $current:ident, $(;)? $(,)? $({})?) => { };
}

/// 捕获并发布能量机关、装甲板、TF 树与各坐标系位姿。
///
/// 该系统在 `Update` 阶段、`TransformSystems::Propagate` 之后运行，
/// 保证读取到的是本帧传播后的最终 `GlobalTransform`。
///
/// # 参数
/// - `camera`：捕获相机（CaptureSource）的全局变换，用于推导相机相对云台的位姿
/// - `gimbal`：受控云台（InfantryGimbal + Controlled）的全局变换，作为 odom 坐标系
/// - `muzzle_offset`：枪管发射偏移组件的本地变换，用于推导 muzzle 坐标系
/// - `runes`：所有能量机关实体（用于在 map 下发布 power_rune 坐标系）
/// - `targets`：能量机关目标点（RuneIndex），仅保留已激活（_ACTIVATED）的
/// - `clock`：ROS2 时钟，提供消息时间戳
/// - `tf_publisher`：`/tf` 话题发布器
/// - `gimbal_pose_pub` / `odom_pose_pub` / `muzzle_pose_pub` / `camera_pose_pub`：对应位姿话题
/// - `center` / `qq` / `armor`：用于查询装甲板中心点的层级查询
/// - `marker_pub`：`/simulator/marker` 话题发布器（装甲板可视化）
///
/// # 算法步骤
/// 1. 计算相机、枪管相对云台的局部位姿（reparented_to）
/// 2. 收集已激活的能量机关目标点，按能量机关实体分组
/// 3. 通过 `tf_tree!` 宏构造完整的 TF 树（map -> odom -> gimbal_link -> muzzle/camera）
/// 4. 追加能量机关、装甲板坐标系到 TF 树
/// 5. 为每个装甲板发布 CUBE 类型的 Marker（绿色，0.3s 生命周期）
/// 6. 发布整棵 TF 树到 `/tf` 话题
fn capture_rune(
    camera: Single<&GlobalTransform, With<CaptureSource>>,
    gimbal: Single<&GlobalTransform, (With<Controlled>, With<InfantryGimbal>)>,
    muzzle_offset: Single<
        (&GlobalTransform, &Transform),
        (With<InfantryLaunchOffset>, With<Controlled>),
    >,

    runes: Query<(Entity, &GlobalTransform, &PowerRune)>,
    targets: Query<(&GlobalTransform, &RuneIndex, &Name)>,

    clock: ResMut<RoboMasterClock>,
    tf_publisher: ResMut<TopicPublisher<GlobalTransformTopic>>,
    gimbal_pose_pub: ResMut<TopicPublisher<GimbalPoseTopic>>,
    odom_pose_pub: ResMut<TopicPublisher<OdomPoseTopic>>,
    muzzle_pose_pub: ResMut<TopicPublisher<MuzzlePoseTopic>>,
    camera_pose_pub: ResMut<TopicPublisher<CameraPoseTopic>>,
    center: Query<(Entity, &GlobalTransform)>,
    qq: HierarchyQuery,
    armor: Query<(Entity, &GlobalTransform, &ArmorRoot)>,
    marker_pub: ResMut<TopicPublisher<OutpostMarkerTopic>>,
) {
    let cam_transform = camera.into_inner();
    let gimbal = gimbal.into_inner();
    // 相机相对云台的局部变换（用于发布 camera_link 坐标系）
    let cam_rel = cam_transform.reparented_to(gimbal);
    // 枪管相对云台的局部变换（用于发布 muzzle 坐标系）
    let muzzle_rel = muzzle_offset.0.reparented_to(gimbal);

    // 注意：使用 debug! 而非 info!，避免每帧打印拖累 ROS2 构建下的 FPS
    debug!(
        "[ROS2] ODOM pos: [{:.4}, {:.4}, {:.4}]",
        gimbal.translation().x,
        gimbal.translation().y,
        gimbal.translation().z
    );
    debug!(
        "[ROS2] CAMERA_REL pos: [{:.4}, {:.4}, {:.4}]",
        cam_rel.translation.x, cam_rel.translation.y, cam_rel.translation.z
    );
    // 收集已激活的能量机关目标点，按所属能量机关实体分组
    // 仅保留名称包含 "_ACTIVATED" 的目标点（避免重复使用同一目标）
    let mut targets = targets.into_iter().fold(
        HashMap::<Entity, Vec<(String, Transform)>>::new(),
        |mut map, (tf, target, name)| {
            // 只使用已激活的目标
            if !name.contains("_ACTIVATED") {
                return map;
            }
            let Ok((rune_entity, rune_tf, rune)) = runes.get(target.rune) else {
                return map;
            };
            map.entry(rune_entity).or_default().push((
                format!("power_rune_{:?}_{:?}", rune.mode(), target.target)
                    .to_string()
                    .to_lowercase(),
                tf.reparented_to(rune_tf),
            ));
            map
        },
    );

    debug!(
        "[ROS2] MUZZLE pos: [{:.4}, {:.4}, {:.4}]",
        muzzle_rel.translation.x, muzzle_rel.translation.y, muzzle_rel.translation.z
    );
    // 计算云台姿态：组合云台旋转、枪管本地旋转，并叠加 ZYX 欧拉角修正（绕 Z 转 PI/2）
    // 最终转换为 ZXY 欧拉角序列以便日志输出
    let rot = (gimbal.rotation()
        * muzzle_offset.1.rotation
        * Quat::from_euler(EulerRot::ZYX, 0.0, 0.0, PI / 2.0))
    .to_euler(EulerRot::ZXY);
    debug!(
        "[ROS2] GIMBAL rpy: [{:.4}, {:.4}, {:.4}]",
        rot.0.to_degrees(),
        rot.1.to_degrees(),
        rot.2.to_degrees()
    );

    // 构造完整 TF 树：
    //   map
    //   └─ odom（云台位置，姿态为单位四元数）—— 同步发布 odom_pose
    //      └─ gimbal_link（云台姿态，叠加枪管旋转与 ZYX 修正）—— 同步发布 gimbal_pose
    //         ├─ muzzle（枪管位置）→ muzzle_link —— 同步发布 muzzle_pose
    //         └─ camera_link（相机位置）→ camera_optical_frame（光学坐标系旋转）—— 同步发布 camera_pose
    //   └─ power_rune_<mode>（每个能量机关）→ power_rune_<mode>_<target>（已激活目标点）
    //   └─ armor_<id>（每个装甲板的中心点）
    let transform_stamped = tf_tree! {
        stamp: Clock::to_builtin_time(&res_unwrap!(clock).get_now().unwrap());

        "map" {
            "odom" as (gimbal.translation(), Quat::IDENTITY) for odom_pose_pub {
                "gimbal_link" as (Vec3::ZERO, gimbal.rotation() * muzzle_offset.1.rotation * Quat::from_euler(EulerRot::ZYX, 0.0, 0.0, PI / 2.0)) for gimbal_pose_pub {
                    "muzzle" as (muzzle_rel.translation, Quat::IDENTITY) {
                        "muzzle_link" as (Vec3::ZERO, Quat::IDENTITY) for muzzle_pose_pub{}
                    }
                    "camera_link" as (cam_rel.translation, Quat::IDENTITY) for camera_pose_pub {
                        "camera_optical_frame" as (Vec3::ZERO, Quat::from_euler(EulerRot::ZYX, -PI / 2.0, PI, PI / 2.0)) {}
                    }
                }
            }
            // 为每个能量机关发布一个坐标系，并追加其已激活的目标点子坐标系
            for (rune_entity, transform, rune) in runes {
                let name = format!("power_rune_{:?}", rune.mode()).to_string().to_lowercase();
                let tf = transform.compute_transform();
                pub name as (tf.translation, tf.rotation);
                let targets = targets.remove(&rune_entity).unwrap_or_default();
                for (name, tf) in targets {
                    pub name as (tf.translation, tf.rotation);
                }
            }
            // 为每个装甲板发布一个坐标系（基于其 CENTER 子实体的全局变换）
            for (entity, _transform, armor) in armor {
                let name = format!("armor_{:?}", armor.id.as_usize())
                    .to_string()
                    .to_lowercase();
                let tf = center.get(qq.of(entity).suffix("CENTER").any().one().unwrap()).unwrap().1.compute_transform();
                pub name as (tf.translation, tf.rotation);
            }
        }
    };

    // 为每个装甲板发布一个 CUBE Marker 用于 RViz 可视化
    let stamp = Clock::to_builtin_time(&res_unwrap!(clock).get_now().unwrap());
    for (entity, tf, armor) in armor {
        // 查询装甲板的 CENTER 子实体全局变换
        let mut tff = center
            .get(qq.of(entity).suffix("CENTER").any().one().unwrap())
            .unwrap()
            .1
            .compute_transform();
        // 叠加装甲板自身旋转与 ZYX 修正（绕 Z 转 -PI/2）
        tff.rotation = tf.rotation() * Quat::from_euler(EulerRot::ZYX, 0.0, 0.0, -PI / 2.0);
        // 将 Bevy 坐标系转换为 ROS 坐标系
        let tf = transform(tff);
        marker_pub.publish(Marker {
            header: Header {
                stamp: stamp.clone(),
                frame_id: "map".to_string(),
            }
            .clone(),
            ns: "armors".to_string(),
            id: armor.id.as_usize() as i32,
            type_: Marker::CUBE as i32,
            action: Marker::ADD as i32,
            pose: Pose {
                position: Point {
                    x: tf.translation.x,
                    y: tf.translation.y,
                    z: tf.translation.z,
                },
                orientation: tf.rotation,
            },
            // 装甲板尺寸：3cm × 15cm × 12.5cm
            scale: Vector3 {
                x: 0.03,
                y: 0.15,
                z: 0.125,
            },
            // 绿色标记（alpha=0 表示由 RViz 主题决定可见性）
            color: ColorRGBA {
                r: 0.0,
                g: 1.0,
                b: 0.0,
                a: 0.0,
            },
            // Marker 生命周期 0.3s，过期后自动消失
            lifetime: r2r::builtin_interfaces::msg::Duration {
                sec: 0,
                nanosec: 300000000,
            },
            frame_locked: false,
            points: vec![],
            colors: vec![],
            texture_resource: "".to_string(),
            texture: Default::default(),
            uv_coordinates: vec![],
            text: "".to_string(),
            mesh_resource: "".to_string(),
            mesh_file: Default::default(),
            mesh_use_embedded_materials: false,
        });
    }

    // 一次性发布整棵 TF 树
    tf_publisher.publish(TFMessage {
        transforms: transform_stamped,
    });
}

/// 处理订阅的云台控制指令（GimbalCmd），驱动云台旋转与开火。
///
/// 该系统仅在 `SubscribeAutoAim` 资源为 true 时运行（由外部 UI/配置开关控制）。
///
/// # 参数
/// - `time`：Bevy 时间资源，用于驱动频率限制器
/// - `commands`：命令队列，用于在满足开火条件时排队执行 `projectile_launch`
/// - `gimbal_cmd`：云台指令订阅器（接收 /rm_gimbal/cmd）
/// - `fire_rate_limiter`：开火频率限制器（默认 10Hz）
/// - `gimbal`：受控云台的本地 Transform 与 InfantryGimbal 数据
/// - `muzzle_offset`：枪管偏移的全局变换，用于计算当前姿态误差
///
/// # 算法步骤
/// 1. 推进开火频率限制器
/// 2. 循环消费订阅器中所有积压指令：
///    - distance == -1.0 表示无效指令，直接返回
///    - fire_advice 为 true 且频率允许时，排队执行 projectile_launch
///    - 将 yaw/pitch（度）转换为弧度，pitch 减去 90 度补偿
///    - 计算期望姿态与当前姿态的差值四元数，叠加到云台 Transform
fn process_subscription(
    time: Res<Time>,
    mut commands: Commands,
    gimbal_cmd: ResMut<TopicSubscriber<GimbalCmdTopic>>,
    mut fire_rate_limiter: ResMut<FireRateLimiter>,
    gimbal: Single<
        (&mut Transform, &mut InfantryGimbal),
        (
            With<Controlled>,
            Without<InfantryChassis>,
            Without<InfantryLaunchOffset>,
        ),
    >,
    muzzle_offset: Single<
        (&GlobalTransform, &Transform),
        (With<InfantryLaunchOffset>, With<Controlled>),
    >,
) {
    let (mut gimbal_transform, mut gimbal_data) = gimbal.into_inner();
    // 推进频率限制器，累积时间预算
    fire_rate_limiter.tick(time.delta());
    loop {
        // 尝试接收一条指令；无指令时退出循环
        let Ok(Some(cmd)) = gimbal_cmd.try_recv() else {
            return;
        };
        // distance == -1.0 表示外部无有效目标，停止处理
        if cmd.distance == -1.0 {
            return;
        }
        // 开火建议：受频率限制器约束，避免每帧开火
        if cmd.fire_advice {
            if fire_rate_limiter.allow() {
                commands.queue(|w: &mut World| {
                    w.run_system_once(projectile_launch).unwrap();
                });
            }
        }
        // 指令中 yaw/pitch 单位为度；pitch 减 90 度以匹配仿真器坐标约定
        let yaw_f32 = (cmd.yaw as f32).to_radians();
        let pitch_f32 = (cmd.pitch as f32 - 90.0).to_radians();
        gimbal_data.local_yaw = yaw_f32;
        gimbal_data.pitch = pitch_f32;
        // 计算期望姿态（YXZ 欧拉序）
        let expected_rotation = Quat::from_euler(EulerRot::YXZ, yaw_f32, pitch_f32, 0.0);
        // 当前枪管全局旋转
        let current_rotation = muzzle_offset.0.rotation();
        // 姿态误差：delta = expected * current^{-1}
        let delta = expected_rotation * current_rotation.inverse();
        // 将误差叠加到云台本地旋转上
        gimbal_transform.rotation = delta * gimbal_transform.rotation;
    }
}

/// 发布科技中心（TechCore）状态 JSON 到 `/simulator/tech_core/state` 话题。
///
/// # 参数
/// - `time`：Bevy 时间，用于提供 elapsed_secs_f64
/// - `clock`：ROS2 时钟，提供消息时间戳
/// - `limiter`：发布频率限制器（默认 20Hz）
/// - `cores`：所有 TechCore 组件查询
/// - `state_pub`：科技中心状态话题发布器
///
/// # 算法步骤
/// 1. 推进频率限制器；若未到发布时刻则直接返回
/// 2. 获取当前 ROS2 时间戳
/// 3. 调用 `tech_core_state_json` 生成状态 JSON 字符串并发布
fn publish_tech_core_state(
    time: Res<Time>,
    clock: Res<RoboMasterClock>,
    mut limiter: ResMut<TechCoreStateRateLimiter>,
    cores: Query<&TechCore>,
    state_pub: Res<TopicPublisher<TechCoreStateTopic>>,
) {
    limiter.tick(time.delta());
    if !limiter.allow() {
        return;
    }

    let stamp = Clock::to_builtin_time(&res_unwrap!(clock).get_now().unwrap());
    state_pub.publish(RosString {
        data: tech_core_state_json(
            stamp.sec,
            stamp.nanosec,
            time.elapsed_secs_f64(),
            cores.iter(),
        ),
    });
}

/// 清理 ROS2 系统资源：在 AppExit 时安全停止 spin 线程。
///
/// 该系统运行在 `Last` 阶段，监听 `AppExit` 事件。
///
/// # 算法步骤
/// 1. 检测到 AppExit 事件后，将停止信号置为 true（Release 序保证可见性）
/// 2. 取出 spin 线程句柄并 join，等待线程退出
/// 3. 根据 join 结果输出日志（成功/失败）
fn cleanup_ros2_system(
    mut exit: MessageReader<AppExit>,
    stop_signal: Res<StopSignal>,
    mut handle_res: ResMut<SpinThreadHandle>,
) {
    if exit.read().len() > 0 {
        // 通知 spin 线程退出循环
        stop_signal.store(true, Ordering::Release);
        if let Some(handle) = handle_res.take() {
            info!("Waiting for ROS 2 spin thread to join...");
            match handle.join() {
                Ok(_) => info!("ROS 2 thread successfully joined. Safe to exit."),
                Err(_) => error!("WARNING: ROS 2 thread panicked or failed to join."),
            }
        }
    }
}

/// ROS2 插件入口：整合所有 ROS2 通信功能。
///
/// 使用方式：在 Bevy App 上调用 `app.add_plugins(ROS2Plugin)`。
#[derive(Default)]
pub struct ROS2Plugin {}

impl Plugin for ROS2Plugin {
    /// 构建 ROS2 插件，完成节点创建、话题注册、系统调度与线程启动。
    ///
    /// # 算法步骤
    /// 1. 读取 `SimulationConfig`，获取相机/Livox/捕获相关配置
    /// 2. 创建 ROS2 节点（名为 "simulator"，命名空间 "robomaster"）
    /// 3. 调用 `register_pub` / `register_sub` 注册所有发布器与订阅器
    /// 4. 从 App 中移除图像/Livox 相关的发布器资源，转移所有权到子插件上下文
    /// 5. 创建 ROS2 时钟（SystemTime），计算相机 FOV、捕获配置、Livox 发布参数
    /// 6. 插入频率限制器、停止信号、时钟等资源
    /// 7. 注册 `RosCapturePlugin`（彩色相机捕获与发布）
    /// 8. 注册 `cleanup_ros2_system`、`process_subscription`、`capture_rune`、`publish_tech_core_state` 系统
    /// 9. 启动独立 spin 线程，循环调用 `node.spin_once`（1ms 间隔）
    /// 10. 若配置启用 Livox，再注册 `RosLivoxPlugin`（深度相机转点云发布）
    fn build(&self, app: &mut App) {
        let sim_config = app
            .world()
            .get_resource::<SimulationConfig>()
            .cloned()
            .unwrap_or_default();
        // 创建 ROS2 节点：节点名 simulator，命名空间 robomaster
        let mut node = Node::create(Context::create().unwrap(), "simulator", "robomaster").unwrap();
        // 停止信号：由 AppExit 触发，通知 spin 线程与各异步任务退出
        let signal_arc = Arc::new(AtomicBool::new(false));

        // 注册所有发布器与订阅器（具体话题列表见 topic.rs 中的 topic! 宏展开）
        register_pub(signal_arc.clone(), app, &mut node);
        register_sub(signal_arc.clone(), app, &mut node);

        // 从 App 中移除图像/相机信息/Livox 相关的发布器资源，
        // 将其所有权转移到 RosCaptureContext / RosLivoxContext 中
        let camera_info = app
            .world_mut()
            .remove_resource::<TopicPublisher<CameraInfoTopic>>()
            .unwrap();
        let image_raw = app
            .world_mut()
            .remove_resource::<TopicPublisher<ImageRawTopic>>()
            .unwrap();
        let image_compressed = app
            .world_mut()
            .remove_resource::<TopicPublisher<ImageCompressedTopic>>()
            .unwrap();
        let livox_pointcloud = app
            .world_mut()
            .remove_resource::<TopicPublisher<LivoxPointCloudTopic>>()
            .unwrap();

        // 创建 ROS2 时钟（基于系统时间）
        let clock = arc_mutex!(Clock::create(SystemTime).unwrap());
        // 计算相机垂直 FOV（弧度）
        let fov_y = sim_config.camera.fov.to_radians();
        // 彩色相机捕获配置：RGB8 格式
        let color_capture_config = CaptureConfig {
            width: sim_config.capture.color.width,
            height: sim_config.capture.color.height,
            texture_format: TextureFormat::bevy_default(),
            frame_kind: CapturedFrameKind::Rgb8,
        };
        // 深度相机捕获配置：Depth32Float 格式
        let depth_capture_config = CaptureConfig {
            width: sim_config.capture.depth.width,
            height: sim_config.capture.depth.height,
            texture_format: TextureFormat::Depth32Float,
            frame_kind: CapturedFrameKind::Depth32F,
        };
        // Livox 发布频率（下限 0.1Hz，避免除零）
        let publish_freq = sim_config.livox_ros.publish_freq.max(0.1);
        // 每次发布的目标点数 = 每秒点数 / 发布频率
        let points_per_publish =
            ((sim_config.livox_ros.points_per_second as f32) / publish_freq).max(1.0) as usize;

        app.insert_resource(RoboMasterClock(clock.clone()))
            .insert_resource(StopSignal(signal_arc.clone()))
            // 开火频率限制：10Hz
            .insert_resource(FireRateLimiter(AverageRateLimiter::from_hz(10.0)))
            // 科技中心状态发布频率：20Hz
            .insert_resource(TechCoreStateRateLimiter(AverageRateLimiter::from_hz(20.0)))
            .add_plugins(RosCapturePlugin {
                config: color_capture_config,
                context: RosCaptureContext {
                    clock: clock.clone(),
                    fov_y,
                    publish_compressed: false,
                    camera_info,
                    image_raw,
                    image_compressed,
                },
            })
            // Last 阶段：监听 AppExit，清理 ROS2 线程
            .add_systems(Last, cleanup_ros2_system)
            // Update 阶段：处理订阅的云台指令（仅在 SubscribeAutoAim 为 true 时运行）
            .add_systems(
                Update,
                process_subscription
                    .run_if(|enabled: Res<SubscribeAutoAim>| enabled.load(Ordering::Acquire)),
            )
            // Update 阶段：在 Transform 传播之后捕获并发布 TF/位姿
            .add_systems(Update, capture_rune.after(TransformSystems::Propagate))
            // Update 阶段：发布科技中心状态
            .add_systems(Update, publish_tech_core_state)
            // 启动独立的 ROS2 spin 线程：循环调用 spin_once（1ms 间隔），直到收到停止信号
            .insert_resource(SpinThreadHandle(Some(thread::spawn(move || {
                while !signal_arc.load(Ordering::Acquire) {
                    node.spin_once(Duration::from_millis(1));
                }
            }))));

        // 若配置启用 Livox 雷达，注册对应的子插件
        if sim_config.livox_ros.enabled {
            app.add_plugins(RosLivoxPlugin {
                config: depth_capture_config,
                context: RosLivoxContext {
                    clock,
                    frame_id: sim_config.livox_ros.frame_id.clone(),
                    fov_y,
                    near: sim_config.capture.depth.near,
                    far: sim_config.capture.depth.far,
                    // 发布周期（纳秒）= 1e9 / 频率
                    publish_period_ns: (1_000_000_000.0 / publish_freq) as u64,
                    points_per_publish,
                    // 线数下限 1，避免除零
                    line_num: sim_config.livox_ros.line_num.max(1),
                    tag_default: sim_config.livox_ros.tag_default,
                    intensity_default: sim_config.livox_ros.intensity_default,
                    pointcloud: livox_pointcloud,
                    last_publish_ns: Arc::new(AtomicU64::new(0)),
                },
            });
        }
    }
}
