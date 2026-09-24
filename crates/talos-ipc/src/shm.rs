//! Shared memory region abstraction (backed by memmap2)
//!
//! Uses memory-mapped files instead of POSIX shared memory (`shm_open`),
//! providing a pure Rust implementation that works on any platform.
//! Files are stored under `/tmp/` with the given logical name.
// 共享内存区域封装，底层使用 memmap2 内存映射文件
// 不使用 POSIX shm_open，改用普通文件mmap实现跨平台共享内存
// 共享内存文件存放于 /tmp 目录，使用传入的逻辑名作为文件名

use memmap2::MmapMut;          // memmap2可变内存映射，映射文件到进程虚拟地址空间
use std::fs::OpenOptions;      // 文件打开选项
use std::io::{self, Write};    // IO错误、写入trait
use std::path::PathBuf;        // 路径类型

/// 共享内存操作可能抛出的错误枚举
#[derive(Debug)]
pub enum ShmError {
    /// 底层IO错误：文件创建、读写、打开失败等
    IoError(io::Error),
    /// 文件内存映射mmap失败
    MapFailed,
    /// 已存在的共享内存文件尺寸小于预期，布局不匹配
    InvalidSize,
}

// 实现Display trait，用于打印可读错误信息
impl std::fmt::Display for ShmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShmError::IoError(e) => write!(f, "IO error: {}", e),
            ShmError::MapFailed => write!(f, "mmap failed"),
            ShmError::InvalidSize => write!(f, "invalid size"),
        }
    }
}

// 实现标准Error trait，兼容 ? 传播、错误处理框架
impl std::error::Error for ShmError {}

// 从标准io::Error自动转换为本模块ShmError，方便?运算符自动包装
impl From<io::Error> for ShmError {
    fn from(e: io::Error) -> Self {
        ShmError::IoError(e)
    }
}

/// 根据共享内存逻辑名称，生成对应的文件系统路径
/// 自动去掉名称开头的/，最终文件放在 /tmp/ 下
fn shm_path(name: &str) -> PathBuf {
    // 清理名称开头的斜杠，防止路径注入
    let clean_name = name.trim_start_matches('/');
    PathBuf::from("/tmp").join(clean_name)
}

/// RAII封装：内存映射共享内存区域
///
/// 生产者调用 create() 创建文件、清零、mmap映射；
/// Drop时，如果是owner生产者，则删除底层文件。
///
/// # Safety
///
/// 该结构体标记Send + Sync。MmapMut本身提供独占可变访问；
/// **并发读写安全由调用方保证**，本层只做内存映射，不内置锁。
pub struct ShmRegion {
    /// 内存映射对象，持有映射到进程地址空间的共享内存缓冲区
    mmap: MmapMut,
    /// 底层磁盘文件路径，Drop时用来删除文件
    path: PathBuf,
    /// 所有权标记：true=生产者（创建方），销毁时删除文件；false=消费者，不删文件
    is_owner: bool,
}

// 安全标记：ShmRegion可以跨线程转移
unsafe impl Send for ShmRegion {}
// 安全标记：ShmRegion可以多线程共享引用
// 注意：只是允许跨线程持有，**并发读写同步需要上层自己做（自旋锁/信号量）**
unsafe impl Sync for ShmRegion {}

impl ShmRegion {
    /// 创建一块全新共享内存（生产者侧）
    ///
    /// 创建/截断底层文件，分配size字节并全部填0，刷盘，然后mmap映射成可读写内存。
    ///
    /// # Arguments
    /// * `name` - 共享内存逻辑名称，作为/tmp下的文件名
    /// * `size` - 共享内存总字节大小
    ///
    /// # Errors
    /// 文件创建、写零、mmap映射失败返回ShmError
    pub fn create(name: &str, size: usize) -> Result<Self, ShmError> {
        let path = shm_path(name);
        // 打开文件：读写，不存在则创建，存在则截断清空
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)?;

        // 设置文件占用的磁盘大小
        file.set_len(size as u64)?;
        // 写入全0，保证文件有真实磁盘空间（避免稀疏文件空洞）
        file.write_all(&vec![0u8; size])?;
        // 强制刷入磁盘，确保文件内容落盘后再mmap
        file.sync_all()?;

        // 重新打开文件，执行内存映射
        let file = OpenOptions::new().read(true).write(true).open(&path)?;
        // 把文件映射到进程虚拟内存，得到可变映射
        let mmap = unsafe { MmapMut::map_mut(&file)? };

        Ok(Self {
            mmap,
            path,
            is_owner: true, // 创建方拥有所有权，退出删文件
        })
    }

    /// 打开已经存在的共享内存（消费者侧）
    ///
    /// 打开/tmp下的共享文件，做mmap；校验文件大小≥预期size，防止布局不匹配。
    ///
    /// # Arguments
    /// * `name` - 共享内存逻辑名称
    /// * `size` - 期望的最小字节长度
    ///
    /// # Errors
    /// 文件尺寸不足返回 ShmError::InvalidSize
    pub fn open(name: &str, size: usize) -> Result<Self, ShmError> {
        let path = shm_path(name);
        let file = OpenOptions::new().read(true).write(true).open(&path)?;

        // 获取文件元信息，校验文件大小
        let metadata = file.metadata()?;
        if metadata.len() < size as u64 {
            return Err(ShmError::InvalidSize);
        }

        // 映射到内存
        let mmap = unsafe { MmapMut::map_mut(&file)? };
        Ok(Self {
            mmap,
            path,
            is_owner: false, // 消费者无所有权，drop不删除文件
        })
    }

    /// 获取共享内存起始裸指针
    pub fn as_ptr(&self) -> *mut u8 {
        self.mmap.as_ptr() as *mut u8
    }

    /// 获取共享内存总字节长度
    pub fn size(&self) -> usize {
        self.mmap.len()
    }

    /// 将共享内存解释为类型T的不可变引用
    ///
    /// # Safety
    /// 调用方必须保证：
    /// 1. T 的内存布局和共享内存里的数据完全匹配
    /// 2. T 使用 #[repr(C)]，对齐满足硬件要求
    /// 3. 同一时刻不存在可变别名（Rust别名规则）
    pub unsafe fn as_ref<T>(&self) -> &T {
        unsafe { &*(self.mmap.as_ptr() as *const T) }
    }

    /// 将共享内存解释为类型T的可变引用
    ///
    /// # Safety
    /// 调用方必须保证：
    /// 1. T 的内存布局匹配共享内存
    /// 2. T 使用 #[repr(C)]，对齐合规
    /// 3. 同一时间没有其他任何引用（可变/不可变）访问这块内存
    pub unsafe fn as_mut<T>(&mut self) -> &mut T {
        unsafe { &mut *(self.mmap.as_ptr() as *mut T) }
    }

    /// 强制将内存映射脏页刷新写入底层文件
    /// 注意：mmap默认会由操作系统后台自动刷盘；flush是主动同步
    pub fn flush(&self) -> Result<(), ShmError> {
        self.mmap.flush()?;
        Ok(())
    }
}

// RAII析构：Drop自动清理
impl Drop for ShmRegion {
    /// 对象销毁时：只有owner生产者才删除/tmp下的底层文件
    /// 消费者打开的实例drop只解除mmap映射，不删文件
    fn drop(&mut self) {
        if self.is_owner {
            // 删除共享内存 backing 文件；忽略删除失败（进程崩溃场景下文件残留）
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
