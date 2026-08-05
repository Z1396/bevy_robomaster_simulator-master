// ====================================================================
// 模块名: layout
// 作用:   定义 talos-ipc 共享内存的物理布局与所有 C-ABI 兼容结构体
// 职责:   1. 定义图像分辨率、缓存行大小、版本号等常量
//         2. 定义 ImageMeta / PoseMeta / GimbalCmd / CameraInfo 等元数据结构
//         3. 定义三缓冲包装类型（ImageTripleBuffer 等）
//         4. 定义 ShmMetaRegion 顶层布局，串联所有数据通道
// 说明:   所有结构体均使用 #[repr(C, align(N))] 保证与 C++ talos-cpp
//         二进制兼容；每个结构体后均有 const assert 校验尺寸与偏移。
// ====================================================================

use std::sync::atomic::AtomicU8;

/// 图像宽度（像素），与相机捕获配置一致
pub const IMAGE_WIDTH: u32 = 1440;
/// 图像高度（像素），与相机捕获配置一致
pub const IMAGE_HEIGHT: u32 = 1080;

/// 缓存行大小（字节），用于对齐以避免 false sharing
pub const CACHE_LINE_SIZE: usize = 64;
/// 共享内存魔数，用于校验连接的目标是否为本协议
pub const SHM_MAGIC: u32 = 0x54414C05;
/// 共享内存协议版本号，C++ 与 Rust 必须一致
pub const SHM_VERSION: u32 = 2;

/// 图像通道数（RGB8 = 3）
pub const IMAGE_CHANNELS: u32 = 3;
/// 单帧图像字节数 = 宽 × 高 × 通道数
pub const IMAGE_SIZE: usize = (IMAGE_WIDTH * IMAGE_HEIGHT * IMAGE_CHANNELS) as usize;
/// 图像池大小 = 3 帧图像，配合三缓冲使用
pub const IMAGE_POOL_SIZE: usize = IMAGE_SIZE * 3;
/// 元数据共享内存的逻辑名称（映射到 /tmp 下的文件名）
pub const SHM_NAME_META: &str = "talos_ipc_meta";
/// 图像池共享内存的逻辑名称
pub const SHM_NAME_IMAGE_POOL: &str = "talos_ipc_image_pool";

/// 三缓冲状态字节的"有新数据"标志位（最高位）
pub const FLAG_NEW: u8 = 0x80;
/// 三缓冲状态字节的槽位索引掩码（低 2 位，取值 0..3）
pub const INDEX_MASK: u8 = 0x03;

/// 单帧图像的元数据
///
/// 与图像数据分离存储，仅描述时间戳、尺寸与图像池中的 buffer_id。
#[repr(C, align(32))]
#[derive(Debug, Clone, Copy, Default)]
pub struct ImageMeta {
    /// 帧序号，单调递增
    pub seq: u64,
    /// 时间戳（纳秒，UNIX epoch）
    pub timestamp_ns: u64,
    /// 图像宽度
    pub width: u32,
    /// 图像高度
    pub height: u32,
    /// 图像池中对应的 buffer 索引（0/1/2）
    pub buffer_id: u8,
    /// 像素格式标识（0 = RGB8）
    pub format: u8,
    /// 对齐填充
    pub _pad: [u8; 6],
}
const _: () = assert!(size_of::<ImageMeta>() == 32);

/// 位姿元数据，存放在三缓冲槽位中
///
/// 用于云台、里程计、枪口、相机等多种位姿的发布。
#[repr(C, align(64))]
#[derive(Debug, Clone, Copy)]
pub struct PoseMeta {
    /// 帧序号
    pub frame_seq: u64,
    /// 平移向量 (x, y, z)
    pub position: [f32; 3],
    /// 旋转四元数 (w, x, y, z) 或 (x, y, z, w)，由上层约定
    pub quaternion: [f32; 4],
    /// 时间戳（纳秒）
    pub timestamp_ns: u64,
    /// 辅助数据填充字节（4 个 f32 = 16 字节），用于兼容旧通道
    pub _pad: [u8; 16],
}
const _: () = assert!(size_of::<PoseMeta>() == 64);

impl Default for PoseMeta {
    fn default() -> Self {
        Self {
            frame_seq: 0,
            position: [0.0; 3],
            quaternion: [0.0; 4],
            timestamp_ns: 0,
            _pad: [0; 16],
        }
    }
}

/// C++ talos-cpp 下发的云台控制指令
///
/// 由外部自瞄程序计算后写入，仿真器订阅后驱动云台转动与开火。
#[repr(C, align(32))]
#[derive(Debug, Clone, Copy, Default)]
pub struct GimbalCmd {
    /// 时间戳（纳秒）
    pub timestamp_ns: u64,
    /// 目标 yaw 角（度）
    pub yaw_deg: f32,
    /// 目标 pitch 角（度）
    pub pitch_deg: f32,
    /// 目标距离（米）；-1.0 表示无效指令
    pub distance_m: f32,
    /// 开火建议：1 = 开火，其他 = 不开火
    pub fire_advice: u8,
    /// 对齐填充
    pub _pad: [u8; 11],
}
const _: () = assert!(size_of::<GimbalCmd>() == 32);

/// 相机内参，发布一次后供 C++ 端读取
///
/// 包含针孔模型参数与畸变系数，C++ 端据此做 PnP 解算等。
#[repr(C, align(64))]
#[derive(Debug, Clone, Copy, Default)]
pub struct CameraInfo {
    /// 时间戳（纳秒）
    pub timestamp_ns: u64,
    /// x 方向焦距（像素单位）
    pub fx: f64,
    /// y 方向焦距（像素单位）
    pub fy: f64,
    /// 主点 x 坐标（像素）
    pub cx: f64,
    /// 主点 y 坐标（像素）
    pub cy: f64,
    /// 畸变系数（k1, k2, p1, p2, k3），当前未使用畸变
    pub distortion: [f64; 5],
    /// 图像宽度
    pub width: u32,
    /// 图像高度
    pub height: u32,
    /// 对齐填充
    pub _pad: [u8; 24],
}
const _: () = assert!(size_of::<CameraInfo>() == 128);

/// 底盘观测数据，融合 IMU、轮速计等传感器
///
/// 由仿真器发布，C++ 自瞄程序据此进行速度补偿与运动预测。
#[repr(C, align(64))]
#[derive(Debug, Clone, Copy)]
pub struct ChassisObservation {
    /// 帧序号
    pub frame_seq: u64,
    /// 时间戳（纳秒）
    pub timestamp_ns: u64,
    /// 距上一帧的时间间隔（秒）
    pub dt_s: f32,
    /// 机体系线速度 (vx, vy)
    pub v_body: [f32; 2],
    /// z 轴角速度（弧度/秒）
    pub wz_radps: f32,
    /// 四个轮子的线速度（米/秒）
    pub wheel_linear_mps: [f32; 4],
    /// 四个轮子的角速度（弧度/秒）
    pub wheel_angular_radps: [f32; 4],
    /// 机体系加速度 (ax, ay)
    pub a_body: [f32; 2],
    /// z 轴角加速度（弧度/秒²）
    pub alpha_z_radps2: f32,
    /// 欧拉角 roll/pitch/yaw（弧度）
    pub rpy_rad: [f32; 3],
    /// 陀螺仪三轴角速度（弧度/秒）
    pub gyro_xyz_radps: [f32; 3],
    /// 加速度计三轴加速度（米/秒²）
    pub accel_xyz_mps2: [f32; 3],
    /// 对齐填充
    pub _pad: [u8; 16],
}
const _: () = assert!(size_of::<ChassisObservation>() == 128);

impl Default for ChassisObservation {
    fn default() -> Self {
        Self {
            frame_seq: 0,
            timestamp_ns: 0,
            dt_s: 0.0,
            v_body: [0.0; 2],
            wz_radps: 0.0,
            wheel_linear_mps: [0.0; 4],
            wheel_angular_radps: [0.0; 4],
            a_body: [0.0; 2],
            alpha_z_radps2: 0.0,
            rpy_rad: [0.0; 3],
            gyro_xyz_radps: [0.0; 3],
            accel_xyz_mps2: [0.0; 3],
            _pad: [0; 16],
        }
    }
}

/// 图像三缓冲包装
///
/// state 是生产者-消费者共享的原子状态字节，slots 存放三份 ImageMeta。
#[repr(C, align(64))]
pub struct ImageTripleBuffer {
    /// 状态字节：高 1 位 FLAG_NEW，低 2 位为最新就绪槽位索引
    pub state: AtomicU8,
    /// 生产者当前写入槽位索引
    pub write_idx: u8,
    /// 消费者上一次读取槽位索引
    pub read_idx: u8,
    /// 对齐填充，确保 state 独占缓存行
    pub _pad1: [u8; 61],
    /// 三个槽位
    pub slots: [ImageMeta; 3],
}
const _: () = assert!(size_of::<ImageTripleBuffer>() == 192);

/// 位姿三缓冲包装，结构与 ImageTripleBuffer 类似
#[repr(C, align(64))]
pub struct PoseTripleBuffer {
    /// 状态字节
    pub state: AtomicU8,
    /// 生产者写入槽位索引
    pub write_idx: u8,
    /// 消费者读取槽位索引
    pub read_idx: u8,
    /// 对齐填充
    pub _pad1: [u8; 61],
    /// 三个位姿槽位
    pub slots: [PoseMeta; 3],
}
const _: () = assert!(size_of::<PoseTripleBuffer>() == 256);

/// 云台指令三缓冲包装
#[repr(C, align(64))]
pub struct GimbalTripleBuffer {
    /// 状态字节
    pub state: AtomicU8,
    /// 生产者写入槽位索引
    pub write_idx: u8,
    /// 消费者读取槽位索引
    pub read_idx: u8,
    /// 对齐填充
    pub _pad1: [u8; 61],
    /// 三个云台指令槽位
    pub slots: [GimbalCmd; 3],
}
const _: () = assert!(size_of::<GimbalTripleBuffer>() == 192);

/// 共享内存顶层头部信息
///
/// 存放魔数、版本号、心跳时间戳等，供消费者校验连接有效性。
#[repr(C, align(64))]
pub struct ShmHeader {
    /// 魔数，必须等于 SHM_MAGIC
    pub magic: u32,
    /// 协议版本号
    pub version: u32,
    /// 共享内存创建时间戳（纳秒）
    pub created_ns: u64,
    /// 最近一次心跳时间戳（纳秒），生产者周期性更新
    pub heartbeat_ns: u64,
    /// 图像宽度
    pub image_width: u32,
    /// 图像高度
    pub image_height: u32,
    /// 对齐填充
    pub _pad: [u8; 32],
}
const _: () = assert!(size_of::<ShmHeader>() == 64);

/// 单批真值中最多容纳的目标（机器人）数量
pub const GROUND_TRUTH_MAX_TARGETS: usize = 16;
/// 单批真值中最多容纳的能量机关数量
pub const GROUND_TRUTH_MAX_RUNES: usize = 4;

/// 单个机器人的真值信息
///
/// 用于评估自瞄算法的检测精度，包含真实位姿、阵营、装甲标识等。
#[repr(C, align(32))]
#[derive(Debug, Clone, Copy, Default)]
pub struct GroundTruthTarget {
    /// 帧序号
    pub frame_seq: u64,
    /// 时间戳（纳秒）
    pub timestamp_ns: u64,
    /// 队伍：0 = Red，1 = Blue
    pub team: u8,
    /// 装甲标签编号
    pub armor_label: u8,
    /// 是否为前哨站（1 = 是）
    pub is_outpost: u8,
    /// 对齐填充
    pub _pad1: u8,
    /// 在 ROS 坐标系下的位置 (x, y, z)
    pub position: [f32; 3],
    /// z 轴角速度（弧度/秒）
    pub vyaw: f32,
    /// yaw 角（弧度）
    pub yaw: f32,
    /// 对齐填充
    pub _pad: [u8; 24],
}
const _: () = assert!(size_of::<GroundTruthTarget>() == 64);

/// 单个能量机关的真值信息
///
/// 包含旋转角度、方向、正弦参数等，用于评估能量机关打击算法。
#[repr(C, align(64))]
#[derive(Debug, Clone, Copy)]
pub struct GroundTruthRune {
    /// 帧序号
    pub frame_seq: u64,
    /// 时间戳（纳秒）
    pub timestamp_ns: u64,
    /// 队伍：0 = Red，1 = Blue
    pub team: u8,
    /// 能量机关模式：0 = Small，1 = Large
    pub rune_mode: u8,
    /// 机构状态：0 = Inactive，1 = Activating，2 = Activated，3 = Failed
    pub mechanism_state: u8,
    /// 对齐填充
    pub _pad1: u8,
    /// 中心点在里程计坐标系下的位置
    pub r_center_odom: [f32; 3],
    /// 半径（未使用，保留字段）
    pub radius: f32,
    /// 当前旋转角度（弧度）
    pub current_angle: f32,
    /// 滚动角速度（未使用，保留字段）
    pub v_roll: f32,
    /// 旋转方向：1 = 顺时针，-1 = 逆时针
    pub direction: i32,
    /// 正弦摆动振幅
    pub sin_amplitude: f32,
    /// 正弦摆动角速度
    pub sin_omega: f32,
    /// 正弦相位（未使用，保留字段）
    pub sin_phase: f32,
    /// 正弦相位偏移
    pub sin_offset: f32,
    /// 已激活后的相对时间（秒）
    pub relative_time: f32,
    /// 当前待击打扇叶编号，-1 表示无效
    pub blade_id: i32,
    /// 5 个扇叶的激活状态
    pub target_activations: [u8; 5],
    /// 对齐填充
    pub _pad: [u8; 20],
}
const _: () = assert!(size_of::<GroundTruthRune>() == 128);

impl Default for GroundTruthRune {
    fn default() -> Self {
        Self {
            frame_seq: 0,
            timestamp_ns: 0,
            team: 0,
            rune_mode: 0,
            mechanism_state: 0,
            _pad1: 0,
            r_center_odom: [0.0; 3],
            radius: 0.0,
            current_angle: 0.0,
            v_roll: 0.0,
            direction: 0,
            sin_amplitude: 0.0,
            sin_omega: 0.0,
            sin_phase: 0.0,
            sin_offset: 0.0,
            relative_time: 0.0,
            // 默认无有效扇叶
            blade_id: -1,
            target_activations: [0; 5],
            _pad: [0; 20],
        }
    }
}

/// 单帧真值批量数据
///
/// 同时携带多个机器人与能量机关的真值，由生产者一次性发布。
#[repr(C, align(64))]
#[derive(Debug, Clone, Copy)]
pub struct GroundTruthBatch {
    /// 帧序号
    pub frame_seq: u64,
    /// 时间戳（纳秒）
    pub timestamp_ns: u64,
    /// 实际写入的目标数量
    pub target_count: u32,
    /// 实际写入的能量机关数量
    pub rune_count: u32,
    /// 目标数组
    pub targets: [GroundTruthTarget; GROUND_TRUTH_MAX_TARGETS],
    /// 能量机关数组
    pub runes: [GroundTruthRune; GROUND_TRUTH_MAX_RUNES],
    /// 对齐填充
    pub _pad: [u8; 64],
}
const _: () = assert!(size_of::<GroundTruthBatch>() == 1664);

impl Default for GroundTruthBatch {
    fn default() -> Self {
        Self {
            frame_seq: 0,
            timestamp_ns: 0,
            target_count: 0,
            rune_count: 0,
            targets: [GroundTruthTarget::default(); GROUND_TRUTH_MAX_TARGETS],
            runes: [GroundTruthRune::default(); GROUND_TRUTH_MAX_RUNES],
            _pad: [0; 64],
        }
    }
}

/// 运行时状态，用于向 C++ 端同步仿真器的开关量
///
/// 例如是否处于跟随模式等。
#[repr(C, align(64))]
#[derive(Debug, Clone, Copy)]
pub struct RuntimeState {
    /// 时间戳（纳秒）
    pub timestamp_ns: u64,
    /// 是否处于自瞄跟随状态：1 = 跟随，0 = 未跟随
    pub following: u8,
    /// 对齐填充
    pub _pad: [u8; 55],
}
const _: () = assert!(size_of::<RuntimeState>() == 64);

impl Default for RuntimeState {
    fn default() -> Self {
        Self {
            timestamp_ns: 0,
            following: 0,
            _pad: [0; 55],
        }
    }
}

/// 顶层共享内存区域布局
///
/// 把头部、各通道三缓冲、相机内参、底盘观测、真值与运行时状态串联
/// 成一块连续内存。生产者与消费者必须按此布局解释共享内存。
#[repr(C)]
pub struct ShmMetaRegion {
    /// 头部信息（魔数、版本、心跳等）
    pub header: ShmHeader,
    /// 图像元数据三缓冲
    pub image: ImageTripleBuffer,
    /// 位姿三缓冲数组，索引由 PoseIndex 决定
    pub poses: [PoseTripleBuffer; 5],
    /// 云台指令三缓冲（消费者读取，生产者不写）
    pub gimbal_cmd: GimbalTripleBuffer,
    /// 相机内参
    pub camera_info: CameraInfo,
    /// 底盘观测数据
    pub chassis_observation: ChassisObservation,
    /// 真值批量数据
    pub ground_truth: GroundTruthBatch,
    /// 运行时状态
    pub runtime_state: RuntimeState,
}
// 以下 const assert 锁定关键字段的大小与偏移，防止误改布局破坏 ABI 兼容
const _: () = assert!(size_of::<ShmMetaRegion>() == 3712);
const _: () = assert!(std::mem::offset_of!(ShmMetaRegion, camera_info) == 1728);
const _: () = assert!(std::mem::offset_of!(ShmMetaRegion, chassis_observation) == 1856);
const _: () = assert!(std::mem::offset_of!(ShmMetaRegion, ground_truth) == 1984);
const _: () = assert!(std::mem::offset_of!(ShmMetaRegion, runtime_state) == 3648);

/// 位姿通道索引枚举
///
/// 用作 poses 数组的下标，区分不同物理含义的位姿。
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoseIndex {
    /// 云台位姿（带旋转四元数）
    Gimbal = 0,
    /// 里程计位姿（机器人整体在世界系的位置）
    Odom = 1,
    /// 枪口位姿（相对云台）
    Muzzle = 2,
    /// 相机位姿（相对云台）
    Camera = 3,
    // Legacy compatibility channel.
    // New integrations should consume `ShmMetaRegion::chassis_observation` instead.
    /// 旧版兼容通道：通过 pose 槽位 4 传递底盘观测摘要
    /// 新集成应改用 ShmMetaRegion::chassis_observation 字段
    ChassisObservation = 4,
}

impl Default for ImageTripleBuffer {
    fn default() -> Self {
        Self {
            // state = 1：初始就绪槽位为 1，且无 FLAG_NEW
            state: AtomicU8::new(1),
            // 生产者从槽位 0 开始写入
            write_idx: 0,
            // 消费者初始 read_idx 为 2，避免与 write_idx 冲突
            read_idx: 2,
            _pad1: [0; 61],
            slots: [ImageMeta::default(); 3],
        }
    }
}

impl Default for PoseTripleBuffer {
    fn default() -> Self {
        Self {
            state: AtomicU8::new(1),
            write_idx: 0,
            read_idx: 2,
            _pad1: [0; 61],
            slots: [PoseMeta::default(); 3],
        }
    }
}

impl Default for GimbalTripleBuffer {
    fn default() -> Self {
        Self {
            state: AtomicU8::new(1),
            write_idx: 0,
            read_idx: 2,
            _pad1: [0; 61],
            slots: [GimbalCmd::default(); 3],
        }
    }
}

impl Default for ShmHeader {
    fn default() -> Self {
        Self {
            magic: SHM_MAGIC,
            version: SHM_VERSION,
            created_ns: 0,
            heartbeat_ns: 0,
            image_width: IMAGE_WIDTH,
            image_height: IMAGE_HEIGHT,
            _pad: [0; 32],
        }
    }
}

impl Default for ShmMetaRegion {
    fn default() -> Self {
        Self {
            header: ShmHeader::default(),
            image: ImageTripleBuffer::default(),
            // 5 个位姿通道，分别对应 PoseIndex 的 5 个变体
            poses: [
                PoseTripleBuffer::default(),
                PoseTripleBuffer::default(),
                PoseTripleBuffer::default(),
                PoseTripleBuffer::default(),
                PoseTripleBuffer::default(),
            ],
            gimbal_cmd: GimbalTripleBuffer::default(),
            camera_info: CameraInfo::default(),
            chassis_observation: ChassisObservation::default(),
            ground_truth: GroundTruthBatch::default(),
            runtime_state: RuntimeState::default(),
        }
    }
}
