//! Lock-free triple-buffer implementation for single-producer single-consumer IPC
//!
//! 无锁三缓冲实现，用于**单生产者单消费者(SPSC)**跨进程IPC
//! 使用原子变量完成同步，无mutex、无自旋等待（wait-free）。
//!
//! 生产者：向当前写槽写入数据，然后原子交换共享状态，标记新帧就绪。
//! 消费者：读取最新就绪槽，并清除新数据标记。
//!
//! 状态编码规则：
//! 最高bit：FLAG_NEW 新数据标记
//! 低2bit：就绪槽索引（取值0/1/2）
use crate::layout::{FLAG_NEW, INDEX_MASK};
use std::sync::atomic::Ordering;

/// 无锁三缓冲：生产者端
///
/// 写入当前写槽，通过原子交换共享状态来对外发布新帧。
/// 生产者独占持有 write_idx，同一时刻只能存在一个生产者实例。
///
/// # Safety
/// 调用方必须保证：同一个三缓冲实例，**最多只能存在一个TripleBufferProducer**。
pub struct TripleBufferProducer<'a, S> {
    /// 共享原子状态字节：FLAG_NEW(高位) | ready_slot_index(低2bit)
    state: &'a std::sync::atomic::AtomicU8,
    /// 生产者下一次要写入的槽索引
    write_idx: &'a mut u8,
    /// 三块数据缓冲区槽位 [slot0, slot1, slot2]
    slots: &'a mut [S; 3],
}

impl<'a, S> TripleBufferProducer<'a, S> {
    /// 构造生产者实例
    ///
    /// # Safety
    /// 调用方保证全局唯一生产者；多个生产者会在 write_idx 上产生数据竞争。
    pub unsafe fn new(
        state: &'a std::sync::atomic::AtomicU8,
        write_idx: &'a mut u8,
        slots: &'a mut [S; 3],
    ) -> Self {
        Self {
            state,
            write_idx,
            slots,
        }
    }

    /// 获取当前写槽的可变引用，用来填充帧数据
    pub fn borrow_mut(&mut self) -> &mut S {
        &mut self.slots[*self.write_idx as usize]
    }

    /// 发布当前写槽的数据（核心逻辑）
    ///
    /// 原子swap：把 `write_idx | FLAG_NEW` 存入共享state，
    /// 旧state的值作为返回值，旧state低2bit就是**下一个安全的写槽索引**。
    /// 保证生产者永远不会覆盖消费者正在读取的槽。
    pub fn publish(&mut self) {
        // AcqRel：写发布 + 同步，保证前面内存写入（图像数据）先于state更新
        let old = self
            .state
            .swap(*self.write_idx | FLAG_NEW, Ordering::AcqRel);

        // old的低2bit = 旧就绪槽索引，这个槽现在空闲，可以拿来写下一帧
        *self.write_idx = old & INDEX_MASK;
    }
}

/// 无锁三缓冲：消费者端
///
/// 通过CAS尝试清除FLAG_NEW标记，读取最新发布的槽。
/// CAS成功后，消费者独占该槽直到下一次publish。
///
/// # Safety
/// 调用方必须保证同一个三缓冲**最多只能存在一个消费者**。
pub struct TripleBufferConsumer<'a, S> {
    /// 共享原子状态字节：FLAG_NEW | ready_index
    state: &'a std::sync::atomic::AtomicU8,
    /// 消费者上一次成功读取的槽索引
    read_idx: &'a mut u8,
    /// 三块只读槽位，消费者只读不写
    slots: &'a [S; 3],
}

impl<'a, S> TripleBufferConsumer<'a, S> {
    /// 构造消费者实例
    ///
    /// # Safety
    /// 调用方保证全局唯一消费者；多个消费者会在 read_idx 产生数据竞争。
    pub unsafe fn new(
        state: &'a std::sync::atomic::AtomicU8,
        read_idx: &'a mut u8,
        slots: &'a [S; 3],
    ) -> Self {
        Self {
            state,
            read_idx,
            slots,
        }
    }

    /// 尝试获取最新就绪帧
    ///
    /// 逻辑：
    /// 1. load state，判断FLAG_NEW是否置位，没有新数据直接返回None
    /// 2. CAS：把 state 从 `ready_idx | FLAG_NEW` 替换成旧read_idx（清除FLAG_NEW）
    /// 3. CAS成功：更新read_idx，返回对应槽的引用
    /// 4. CAS失败：说明生产者在load和CAS之间publish了更新的帧，重试一次
    /// 最多重试1次，两次失败直接返回None，不阻塞。
    ///
    /// 返回 Some(&S) 拿到最新帧；None表示无新帧/抢帧失败
    pub fn borrow(&mut self) -> Option<&S> {
        let mut expected = self.state.load(Ordering::Acquire);
        // 没有新数据标记，直接返回
        if (expected & FLAG_NEW) == 0 {
            return None;
        }
        let mut ready_idx = expected & INDEX_MASK;
        let mut desired = *self.read_idx;

        // 第一次CAS尝试
        match self.state.compare_exchange_weak(
            expected,
            desired,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => {
                // CAS成功，抢占到该帧
                *self.read_idx = ready_idx;
                Some(&self.slots[ready_idx as usize])
            }
            Err(new_expected) => {
                // CAS失败：state被生产者更新了，读取新的state，重试一次
                expected = new_expected;
                if (expected & FLAG_NEW) == 0 {
                    return None;
                }
                ready_idx = expected & INDEX_MASK;
                desired = *self.read_idx;

                // 第二次（最后一次）CAS尝试
                match self.state.compare_exchange_weak(
                    expected,
                    desired,
                    Ordering::AcqRel,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        *self.read_idx = ready_idx;
                        Some(&self.slots[ready_idx as usize])
                    }
                    Err(_) => None, // 两次都失败，放弃，等待下一轮poll
                }
            }
        }
    }

    /// 仅查询是否存在新数据，**不消费**帧
    /// 用于非阻塞轮询判断，borrow才会真正取走帧
    #[must_use]
    #[allow(dead_code)]
    pub fn has_new_data(&self) -> bool {
        (self.state.load(Ordering::Acquire) & FLAG_NEW) != 0
    }
}
