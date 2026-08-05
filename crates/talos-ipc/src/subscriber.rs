// ====================================================================
// 模块名: subscriber
// 作用:   共享内存消费者，由 C++ talos-cpp 或其他外部进程的 Rust 端持有
// 职责:   1. 连接已存在的 meta 共享内存区域
//         2. 校验魔数与版本号
//         3. 提供接收云台指令、查询底盘观测等高层 API
// 说明:   消费者只打开 meta_region，不需要 image_pool（图像数据由
//         C++ 端通过 buffer_id 自行映射读取）。所有读取操作均通过
//         TripleBufferConsumer 实现无锁并发读取。
// ====================================================================

use crate::layout::*;
use crate::shm::{ShmError, ShmRegion};
use crate::triple_buffer::TripleBufferConsumer;

/// 共享内存消费者
///
/// 持有 meta_region 共享内存的引用，从中读取生产者发布的数据。
pub struct ShmSubscriber {
    /// 元数据共享内存区域（只读消费）
    meta_region: ShmRegion,
}

impl ShmSubscriber {
    /// 连接已存在的共享内存
    ///
    /// 算法步骤:
    ///   1. 以消费者身份打开 meta 共享内存
    ///   2. 校验魔数是否等于 SHM_MAGIC
    ///   3. 校验版本号是否等于 SHM_VERSION
    ///   4. 任一校验失败则返回 InvalidSize 错误
    ///
    /// 返回: 包裹在 Result 中的 ShmSubscriber
    pub fn connect() -> Result<Self, ShmError> {
        let meta_region = ShmRegion::open(SHM_NAME_META, size_of::<ShmMetaRegion>())?;

        unsafe {
            let meta = meta_region.as_ref::<ShmMetaRegion>();
            // 校验魔数，确认连接的是 talos 协议共享内存
            if meta.header.magic != SHM_MAGIC {
                return Err(ShmError::InvalidSize);
            }
            // 校验版本号，确保 ABI 兼容
            if meta.header.version != SHM_VERSION {
                return Err(ShmError::InvalidSize);
            }
        }

        Ok(Self { meta_region })
    }

    /// 接收最新的云台指令
    ///
    /// 通过 TripleBufferConsumer 的 borrow 取出最新数据。若无新数据
    /// 或 CAS 竞争失败，则返回 None。
    ///
    /// 返回: Some(GimbalCmd) 表示成功读取；None 表示无新数据
    pub fn recv_gimbal_cmd(&mut self) -> Option<GimbalCmd> {
        unsafe {
            let meta = self.meta_region.as_mut::<ShmMetaRegion>();
            // 构造消费者，从 gimbal_cmd 三缓冲读取最新指令
            let mut consumer = TripleBufferConsumer::new(
                &meta.gimbal_cmd.state,
                &mut meta.gimbal_cmd.read_idx,
                &meta.gimbal_cmd.slots,
            );

            // copied() 将 &GimbalCmd 转换为 GimbalCmd（实现 Copy）
            consumer.borrow().copied()
        }
    }

    /// 非消费式检查是否有新的云台指令
    ///
    /// 仅读取 state 的 FLAG_NEW 标志，不修改共享状态。
    pub fn has_gimbal_cmd(&self) -> bool {
        unsafe {
            let meta = self.meta_region.as_ref::<ShmMetaRegion>();
            (meta
                .gimbal_cmd
                .state
                .load(std::sync::atomic::Ordering::Acquire)
                & FLAG_NEW)
                != 0
        }
    }

    /// 读取底盘观测数据
    ///
    /// 底盘观测采用直接覆盖写入（非三缓冲），因此这里直接读取最新值。
    /// 通过 timestamp_ns 是否为 0 判断数据是否有效。
    ///
    /// 返回: Some(ChassisObservation) 表示有有效数据；None 表示尚未发布
    pub fn chassis_observation(&self) -> Option<ChassisObservation> {
        unsafe {
            let meta = self.meta_region.as_ref::<ShmMetaRegion>();
            let observation = meta.chassis_observation;
            // timestamp_ns == 0 表示从未发布过，视为无效
            if observation.timestamp_ns == 0 {
                None
            } else {
                Some(observation)
            }
        }
    }
}
