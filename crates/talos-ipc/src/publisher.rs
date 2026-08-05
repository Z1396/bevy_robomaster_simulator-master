// ====================================================================
// 模块名: publisher
// 作用:   共享内存生产者，由 Bevy 仿真器主进程持有
// 职责:   1. 创建 meta 与 image_pool 两块共享内存
//         2. 初始化 ShmHeader 与所有 TripleBuffer 的正确初始状态
//         3. 提供发布图像、位姿、相机内参、底盘观测、真值等高层 API
// 说明:   所有 publish_* 方法都通过 TripleBufferProducer 或直接写入
//         共享内存对应字段实现零拷贝。注意 ShmRegion::create 的零填充
//         会破坏 TripleBuffer 初始状态，必须手动重新初始化。
// ====================================================================

use crate::layout::*;
use crate::shm::{ShmError, ShmRegion};
use crate::triple_buffer::TripleBufferProducer;
use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

/// 共享内存生产者
///
/// 持有 meta_region（元数据）与 image_pool（图像数据）两块共享内存。
/// 仿真器主进程通过它向 C++ talos-cpp 发布各类数据。
pub struct ShmPublisher {
    /// 元数据共享内存区域，存放头部、三缓冲、相机内参等
    meta_region: ShmRegion,
    /// 图像池共享内存区域，存放 3 帧 RGB 图像
    image_pool: ShmRegion,
    /// 当前轮转使用的图像 buffer 索引（0/1/2）
    current_buffer_id: u8,
}

impl ShmPublisher {
    /// 创建生产者并初始化共享内存
    ///
    /// 算法步骤:
    ///   1. 创建 meta_region 与 image_pool 两块共享内存
    ///   2. 写入 ShmHeader（魔数、版本、创建时间、心跳、分辨率）
    ///   3. 调用 init_triple_buffer 修正所有三缓冲的初始状态
    ///      （ShmRegion::create 的零填充会破坏初始 state）
    ///
    /// 返回: 包裹在 Result 中的 ShmPublisher
    pub fn create() -> Result<Self, ShmError> {
        let mut meta_region = ShmRegion::create(SHM_NAME_META, size_of::<ShmMetaRegion>())?;
        let image_pool = ShmRegion::create(SHM_NAME_IMAGE_POOL, IMAGE_POOL_SIZE)?;

        unsafe {
            let meta = meta_region.as_mut::<ShmMetaRegion>();

            // 初始化 header：写入魔数、版本、时间戳与分辨率
            meta.header = ShmHeader {
                magic: SHM_MAGIC,
                version: SHM_VERSION,
                created_ns: Self::now_ns(),
                heartbeat_ns: Self::now_ns(),
                image_width: IMAGE_WIDTH,
                image_height: IMAGE_HEIGHT,
                _pad: [0; 32],
            };

            // 初始化所有 TripleBuffer (CRITICAL: 零填充破坏了正确的初始状态)
            // 正确初始状态: state=1 (ready slot), write_idx=0, read_idx=2
            Self::init_triple_buffer(&mut meta.image);
            for pose in &mut meta.poses {
                Self::init_triple_buffer(pose);
            }
            Self::init_triple_buffer(&mut meta.gimbal_cmd);
        }

        Ok(Self {
            meta_region,
            image_pool,
            current_buffer_id: 0,
        })
    }

    /// 发布一帧图像
    ///
    /// 算法步骤:
    ///   1. 断言数据长度等于 IMAGE_SIZE
    ///   2. 轮转选择当前 buffer_id，并更新下一帧的 current_buffer_id
    ///   3. 用 copy_nonoverlapping 将图像数据拷贝到 image_pool 对应槽位
    ///   4. 通过 TripleBufferProducer 更新 ImageMeta 并 publish
    ///
    /// 参数:
    ///   - data: RGB8 像素数据，长度必须等于 IMAGE_SIZE
    ///   - seq: 帧序号
    ///   - timestamp_ns: 时间戳（纳秒）
    pub fn publish_image(&mut self, data: &[u8], seq: u64, timestamp_ns: u64) {
        assert_eq!(data.len(), IMAGE_SIZE, "Image size mismatch");

        // 轮转使用 0/1/2 三个 buffer，避免与消费者正在读取的 buffer 冲突
        let buffer_id = self.current_buffer_id;
        self.current_buffer_id = (self.current_buffer_id + 1) % 3;

        unsafe {
            // 计算目标地址并执行零拷贝内存复制
            let pool_ptr = self.image_pool.as_ptr();
            let dst = pool_ptr.add(buffer_id as usize * IMAGE_SIZE);
            std::ptr::copy_nonoverlapping(data.as_ptr(), dst, IMAGE_SIZE);
        }

        unsafe {
            let meta = self.meta_region.as_mut::<ShmMetaRegion>();
            // 构造生产者，更新 ImageMeta 后发布
            let mut producer = TripleBufferProducer::new(
                &meta.image.state,
                &mut meta.image.write_idx,
                &mut meta.image.slots,
            );

            let slot = producer.borrow_mut();
            slot.seq = seq;
            slot.timestamp_ns = timestamp_ns;
            slot.width = IMAGE_WIDTH;
            slot.height = IMAGE_HEIGHT;
            slot.buffer_id = buffer_id;
            slot.format = 0;
            producer.publish();
        }
    }

    /// 发布位姿（不带辅助数据）
    ///
    /// 是 publish_pose_with_aux 的便捷封装，aux_f32 全部置零。
    ///
    /// 参数:
    ///   - index: 位姿通道索引（Gimbal/Odom/Muzzle/Camera/ChassisObservation）
    ///   - position: 平移向量
    ///   - quaternion: 旋转四元数
    ///   - frame_seq: 帧序号
    ///   - timestamp_ns: 时间戳（纳秒）
    pub fn publish_pose(
        &mut self,
        index: PoseIndex,
        position: [f32; 3],
        quaternion: [f32; 4],
        frame_seq: u64,
        timestamp_ns: u64,
    ) {
        self.publish_pose_with_aux(
            index,
            position,
            quaternion,
            [0.0; 4],
            frame_seq,
            timestamp_ns,
        );
    }

    /// 发布位姿（带 4 个 f32 辅助数据）
    ///
    /// 算法步骤:
    ///   1. 取出对应 PoseIndex 的 PoseTripleBuffer
    ///   2. 通过 TripleBufferProducer 写入 PoseMeta 各字段
    ///   3. 将 aux_f32 序列化为 16 字节小端字节流写入 _pad
    ///   4. publish 发布
    ///
    /// 参数:
    ///   - aux_f32: 4 个 f32 辅助数据，通过 _pad 字段传递（用于兼容旧通道）
    pub fn publish_pose_with_aux(
        &mut self,
        index: PoseIndex,
        position: [f32; 3],
        quaternion: [f32; 4],
        aux_f32: [f32; 4],
        frame_seq: u64,
        timestamp_ns: u64,
    ) {
        unsafe {
            let meta = self.meta_region.as_mut::<ShmMetaRegion>();
            let pose_buf = &mut meta.poses[index as usize];
            let mut producer = TripleBufferProducer::new(
                &pose_buf.state,
                &mut pose_buf.write_idx,
                &mut pose_buf.slots,
            );

            let slot = producer.borrow_mut();
            slot.frame_seq = frame_seq;
            slot.position = position;
            slot.quaternion = quaternion;
            slot.timestamp_ns = timestamp_ns;
            // 将辅助 f32 数组压缩进 _pad 字段，供旧版消费者读取
            slot._pad = aux_f32_to_bytes(aux_f32);

            producer.publish();
        }
    }

    /// 设置相机内参（通常只在启动时调用一次）
    pub fn set_camera_info(&mut self, info: CameraInfo) {
        unsafe {
            let meta = self.meta_region.as_mut::<ShmMetaRegion>();
            meta.camera_info = info;
        }
    }

    /// 发布底盘观测数据（直接覆盖写入，非三缓冲）
    pub fn publish_chassis_observation(&mut self, observation: ChassisObservation) {
        unsafe {
            let meta = self.meta_region.as_mut::<ShmMetaRegion>();
            meta.chassis_observation = observation;
        }
    }

    /// 发布真值批量数据（直接覆盖写入）
    pub fn publish_ground_truth(&mut self, batch: &GroundTruthBatch) {
        unsafe {
            let meta = self.meta_region.as_mut::<ShmMetaRegion>();
            meta.ground_truth = *batch;
        }
    }

    /// 发布运行时状态（如是否跟随）
    pub fn publish_runtime_state(&mut self, state: RuntimeState) {
        unsafe {
            let meta = self.meta_region.as_mut::<ShmMetaRegion>();
            meta.runtime_state = state;
        }
    }

    /// 更新心跳时间戳
    ///
    /// 由 Bevy 周期性调用，C++ 端通过检查 heartbeat_ns 判断生产者是否存活。
    pub fn update_heartbeat(&mut self) {
        unsafe {
            let meta = self.meta_region.as_mut::<ShmMetaRegion>();
            meta.header.heartbeat_ns = Self::now_ns();
        }
    }

    /// 获取当前 UNIX 纳秒时间戳
    fn now_ns() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    }

    /// 初始化 TripleBuffer 到正确的初始状态
    ///
    /// ShmRegion::create() 使用零填充，会破坏 TripleBuffer 的正确初始状态。
    /// 必须手动重新初始化。
    ///
    /// 正确初始状态:
    /// - state = 1 (ready slot 是 1, 无 FLAG_NEW)
    /// - write_idx = 0 (生产者写入 slot 0)
    /// - read_idx = 2 (消费者上次读取 slot 2)
    fn init_triple_buffer(buf: &mut impl TripleBufferInit) {
        buf.init_state();
    }
}

/// 将 4 个 f32 转换为 16 字节小端字节流
///
/// 用于将辅助数据塞进 PoseMeta._pad 字段。逐个 f32 调用 to_le_bytes
/// 后按顺序拼接，保证跨平台字节序一致。
fn aux_f32_to_bytes(aux_f32: [f32; 4]) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    for (i, value) in aux_f32.iter().enumerate() {
        bytes[i * 4..(i + 1) * 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes
}

/// Trait for initializing triple buffer state
/// 三缓冲状态初始化 trait，为不同槽位类型提供统一的 init_state 接口
trait TripleBufferInit {
    /// 将 state/write_idx/read_idx 设置为正确的初始值
    fn init_state(&mut self);
}

impl TripleBufferInit for ImageTripleBuffer {
    fn init_state(&mut self) {
        // state=1：初始就绪槽位为 1，无 FLAG_NEW
        self.state.store(1, Ordering::Relaxed);
        // 生产者从 0 开始写，消费者初始读取 2，三者互不冲突
        self.write_idx = 0;
        self.read_idx = 2;
    }
}

impl TripleBufferInit for PoseTripleBuffer {
    fn init_state(&mut self) {
        self.state.store(1, Ordering::Relaxed);
        self.write_idx = 0;
        self.read_idx = 2;
    }
}

impl TripleBufferInit for GimbalTripleBuffer {
    fn init_state(&mut self) {
        self.state.store(1, Ordering::Relaxed);
        self.write_idx = 0;
        self.read_idx = 2;
    }
}
