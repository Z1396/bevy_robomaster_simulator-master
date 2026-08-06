//! Talos 共享内存 IPC 模块 (Bevy 集成层)
//!
//! 提供与 C++ talos-cpp 通信的零拷贝共享内存接口。
//! 纯 IPC 数据结构与传输逻辑已移至 `talos-ipc` crate，此处只保留 Bevy 集成。
//!
//! 子模块:
//! - [`capture`]: 将 Bevy 渲染管线的图像与位姿数据采集并发布到共享内存
//! - [`ground_truth`]: 收集仿真器中的机器人/能量机关真值数据并发布
//! - [`plugin`]: 将 talos-ipc 通信能力集成为 Bevy Plugin，注册资源与系统

mod capture;
mod ground_truth;
mod plugin;

pub use plugin::TalosPlugin;
