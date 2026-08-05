// ============================================================================
// 模块名：ros2::topic
// 作  用：ROS2 话题抽象层，统一管理发布器与订阅器的创建与消息通道
// 职  责：
//   1. 定义 `RosTopic` trait，统一描述话题名、消息类型与 QoS 配置
//   2. 提供 `TopicPublisher` / `TopicSubscriber` 资源，封装异步消息通道
//   3. 通过 `topic!` 宏批量声明所有话题，并生成 `register_pub` / `register_sub` 注册函数
//   4. 发布采用 mpsc 通道 + 异步任务池，订阅采用独立线程轮询
// ============================================================================

use bevy::prelude::{App, Resource};
use bevy::tasks::AsyncComputeTaskPool;
use bevy::tasks::futures_lite::StreamExt;
use bevy::tasks::futures_lite::future::block_on;
use futures::SinkExt;
use futures::channel::mpsc;
use futures::channel::mpsc::{Sender, TryRecvError};
use r2r::geometry_msgs::msg::PoseStamped;
use r2r::rm_interfaces::msg::GimbalCmd;
use r2r::sensor_msgs::msg::{CameraInfo, CompressedImage, Image, PointCloud2};
use r2r::std_msgs::msg::String as RosString;
use r2r::tf2_msgs::msg::TFMessage;
use r2r::visualization_msgs::msg::Marker;
use r2r::{Node, QosProfile, WrappedTypesupport};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

/// 话题发布器资源：封装 mpsc 通道的发送端，提供非阻塞的 `publish` 接口。
///
/// 内部通过 `AsyncComputeTaskPool` 异步发送消息，避免阻塞 Bevy 主线程。
/// 通道缓冲区大小为 1024，足以应对瞬时高频发布。
#[derive(Resource, Clone)]
pub struct TopicPublisher<T: RosTopic> {
    /// mpsc 通道发送端，消息类型由 `RosTopic::T` 决定
    sender: Sender<T::T>,
}

impl<T: RosTopic> TopicPublisher<T> {
    /// 创建发布器，仅供本模块内部使用（注册时调用）。
    pub(super) fn new(sender: Sender<T::T>) -> Self {
        TopicPublisher { sender }
    }

    /// 异步发布一条消息。
    ///
    /// 克隆发送端后在异步任务池中执行 `send`，调用方无需等待。
    /// 若通道已满或关闭，消息会被静默丢弃（`let _ =`）。
    pub fn publish(&self, message: T::T) {
        let mut sender = self.sender.clone();
        AsyncComputeTaskPool::get()
            .spawn(async move {
                let _ = sender.send(message).await;
            })
            .detach();
    }
}

/// 话题订阅器资源：使用 `Arc<Mutex<Option<T>>>` 缓存最新收到的消息。
///
/// 设计为"只保留最新一条"语义：每次收到新消息都会覆盖旧的，
/// `try_recv` 取出后会清空缓存，避免重复处理。
#[derive(Resource)]
pub struct TopicSubscriber<T: RosTopic> {
    /// 最新消息缓存，由订阅线程写入，由 Bevy 系统读取
    receiver: Arc<Mutex<Option<T::T>>>,
}

impl<T: RosTopic> TopicSubscriber<T> {
    /// 创建订阅器，初始化为空缓存。仅供本模块内部使用。
    pub(super) fn new() -> Self {
        TopicSubscriber {
            receiver: Arc::new(Mutex::new(None)),
        }
    }

    /// 尝试取出最新的一条消息。
    ///
    /// # 返回值
    /// - `Ok(Some(msg))`：成功取出一条消息（取后即空）
    /// - `Ok(None)`：当前无消息
    ///
    /// 注意：本实现始终返回 `Ok`，`TryRecvError` 仅用于保持接口与标准通道一致。
    pub fn try_recv(&self) -> Result<Option<T::T>, TryRecvError> {
        Ok(self.receiver.lock().unwrap().take())
    }
}

/// 创建订阅器并启动后台轮询线程。
///
/// 由于 r2r 的订阅是异步的（基于 `Stream`），需要在一个独立线程中
/// 通过 `block_on` 阻塞等待消息，收到后写入共享缓存。
///
/// # 参数
/// - `node`：ROS2 节点，用于创建订阅
/// - `signal`：停止信号，置为 true 时线程退出
///
/// # 算法步骤
/// 1. 调用 `node.subscribe` 创建订阅器（话题名与 QoS 由 `T::TOPIC` / `T::QOS` 决定）
/// 2. 创建 `TopicSubscriber` 并克隆其内部 `Arc<Mutex>` 用于跨线程访问
/// 3. 启动独立线程，循环 `block_on(subscriber.next())`：
///    - 收到消息则写入缓存（覆盖旧的）
///    - 收到 None 则 continue
///    - 检测停止信号则退出
fn subscriber<T: RosTopic>(node: &mut Node, signal: Arc<AtomicBool>) -> TopicSubscriber<T> {
    let mut subscriber = node.subscribe::<T::T>(T::TOPIC, T::QOS).unwrap();
    let sub = TopicSubscriber::new();
    let mutex = sub.receiver.clone();
    std::thread::spawn(move || {
        while !signal.load(std::sync::atomic::Ordering::Acquire) {
            match block_on(subscriber.next()) {
                // 收到新消息：覆盖缓存中的旧消息
                Some(msg) => {
                    mutex.lock().unwrap().replace(msg);
                }
                // 流暂时无消息：继续轮询
                None => continue,
            }
        }
    });
    sub
}

/// 创建发布器并启动后台异步发送任务。
///
/// # 参数
/// - `node`：ROS2 节点，用于创建发布器
/// - `signal`：停止信号，置为 true 时任务退出
///
/// # 算法步骤
/// 1. 创建容量 1024 的 mpsc 通道
/// 2. 调用 `node.create_publisher` 创建 ROS2 发布器
/// 3. 在 `AsyncComputeTaskPool` 中启动异步任务，循环从通道接收消息并调用 `publisher.publish`
/// 4. 返回通道发送端封装的 `TopicPublisher`
fn publisher<T: RosTopic>(node: &mut Node, signal: Arc<AtomicBool>) -> TopicPublisher<T> {
    // 创建容量 1024 的有界通道，平衡内存占用与背压
    let (sender, mut receiver) = mpsc::channel(1024);

    let publisher = node.create_publisher(T::TOPIC, T::QOS).unwrap();

    // 异步任务：从通道消费消息并转发到 ROS2 发布器
    AsyncComputeTaskPool::get()
        .spawn(async move {
            while !signal.load(std::sync::atomic::Ordering::Acquire) {
                match receiver.next().await {
                    // 收到消息：转发到 ROS2
                    Some(m) => {
                        let _ = publisher.publish(&m);
                    }
                    // 通道关闭：退出
                    None => break,
                }
            }
        })
        .detach();
    TopicPublisher::new(sender)
}

/// 订阅器批量注册宏：为每个话题类型创建 `TopicSubscriber` 资源并插入 App。
///
/// 用法：`subscriber!(signal, app, node, TopicA, TopicB, ...)`
#[macro_export]
macro_rules! subscriber {
    ($signal:expr, $app:ident, $node:ident, $($topic:ty),* $(,)?) => {
        $(
            $app.insert_resource($crate::ros2::topic::subscriber::<$topic>($node, $signal));
        )*
    };
}

/// ROS2 话题抽象 trait：统一描述一个话题的名称、消息类型与 QoS 配置。
///
/// 每个实现该 trait 的零大小类型（ZST）代表一个具体话题，
/// 配合 `topic!` 宏可批量声明并自动生成注册函数。
pub trait RosTopic {
    /// 消息类型，必须实现 `WrappedTypesupport`（r2r 的消息类型 trait）且可跨线程发送
    type T: WrappedTypesupport + Send + 'static;
    /// 话题名称（如 "/image_raw"）
    const TOPIC: &'static str;
    /// QoS 配置（可靠性、持久性等）
    const QOS: QosProfile;
}

/// 话题声明宏：批量定义话题类型并生成 `register_pub` / `register_sub` 注册函数。
///
/// # 用法
/// ```ignore
/// topic!(
///     pub {
///         "/image_raw" as Image as ImageRawTopic;
///         ...
///     }
///     sub {
///         "/rm_gimbal/cmd" as GimbalCmd as GimbalCmdTopic with QosProfile::sensor_data();
///     }
/// );
/// ```
/// - `pub { ... }`：声明发布话题，生成 `register_pub(atomic, app, node)`
/// - `sub { ... }`：声明订阅话题，生成 `register_sub(atomic, app, node)`
/// - 每条声明格式：`"<topic_url>" as <MsgType> as <TopicName> [with <qos>]`
macro_rules! topic {
    // 基本形式：声明单个话题类型并实现 RosTopic
    ($topic:ident, $msg_typ:ty, $url:literal, $qos:expr) => {
        #[derive(Clone)]
        pub struct $topic;
        impl RosTopic for $topic {
            type T = $msg_typ;
            const TOPIC: &'static str = $url;
            const QOS: QosProfile = $qos;
        }
    };
    // 省略 QoS 的便捷形式：使用默认 QoS
    ($topic:ident, $msg_typ:ty, $url:literal) => {
        topic!($topic, $msg_typ, $url, ::r2r::QosProfile::default());
    };
    // 发布话题块：声明所有话题并生成 register_pub 函数
    (pub {$($url:literal as $msg_typ:ty as $topic:ident $(with $qos: expr)?;)*} $($remaining:tt)*) => {
        $(
            topic!($topic, $msg_typ, $url $(, $qos)?);
        )*

        /// 注册所有发布器：为每个发布话题创建 `TopicPublisher` 资源并插入 App。
        pub fn register_pub(atomic:Arc<AtomicBool>, app:&mut App, node:&mut Node) {
            $(
                app.insert_resource(publisher::<$topic>(node, atomic.clone()));
            )*
        }
        topic!($($remaining)*);
    };
    // 订阅话题块：声明所有话题并生成 register_sub 函数
    (sub {$($url:literal as $msg_typ:ty as $topic:ident $(with $qos: expr)?;)*} $($remaining:tt)*) => {
        $(
            topic!($topic, $msg_typ, $url $(, $qos)?);
        )*

        /// 注册所有订阅器：为每个订阅话题创建 `TopicSubscriber` 资源并插入 App。
        pub fn register_sub(atomic:Arc<AtomicBool>, app:&mut App, node:&mut Node) {
            $crate::subscriber!(atomic, app, node, $($topic,)*);
        }
        topic!($($remaining)*);
    };
    // 终止分支
    ( )=>{}
}

// 批量声明所有 ROS2 话题：
//
// 发布话题（pub）：
//   - /camera_info            : CameraInfo        相机内参
//   - /image_raw              : Image             原始 RGB 图像
//   - /image_compressed       : CompressedImage   JPEG 压缩图像
//   - /livox/lidar            : PointCloud2       Livox 雷达点云
//   - /tf                     : TFMessage         TF 树
//   - /simulator/marker       : Marker            可视化标记（装甲板等）
//   - /gimbal_pose            : PoseStamped       云台位姿
//   - /odom_pose              : PoseStamped       里程计位姿
//   - /muzzle_pose            : PoseStamped       枪口位姿
//   - /camera_pose            : PoseStamped       相机位姿
//   - /simulator/tech_core/state : String         科技中心状态 JSON
//
// 订阅话题（sub）：
//   - /rm_gimbal/cmd          : GimbalCmd         云台控制指令（QoS: sensor_data）
topic!(
    pub {
        "/camera_info" as CameraInfo as CameraInfoTopic;
        "/image_raw" as Image as ImageRawTopic;
        "/image_compressed" as CompressedImage as ImageCompressedTopic;
        "/livox/lidar" as PointCloud2 as LivoxPointCloudTopic;
        "/tf" as TFMessage as GlobalTransformTopic;
        "/simulator/marker" as Marker as OutpostMarkerTopic;
        "/gimbal_pose" as PoseStamped as GimbalPoseTopic;
        "/odom_pose" as PoseStamped as OdomPoseTopic;
        "/muzzle_pose" as PoseStamped as MuzzlePoseTopic;
        "/camera_pose" as PoseStamped as CameraPoseTopic;
        "/simulator/tech_core/state" as RosString as TechCoreStateTopic;
    }
    sub {
        "/rm_gimbal/cmd" as GimbalCmd as GimbalCmdTopic with QosProfile::sensor_data();
    }
);
