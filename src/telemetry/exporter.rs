//! `exporter` 模块
//!
//! 定义遥测导出器特征 `TelemetryExporter`，用于将帧遥测数据导出到不同目的地。
//! 实现该特征即可自定义导出逻辑（如日志、UDP 网络广播、文件存储、ROS 消息等）。

use super::FrameData;

/// 遥测导出器特征。
///
/// 实现该特征以定义自定义的遥测数据导出逻辑。
/// 每个导出器负责将 `FrameData` 遥测数据包输出到特定目的地。
pub trait TelemetryExporter {
    /// 处理一帧遥测数据。
    ///
    /// 参数：
    /// - `data`：当前帧的遥测数据包，包含时间戳、装甲标注和位姿信息。
    fn on_frame(&mut self, data: &FrameData);

    /// 获取导出器的名称标识。
    ///
    /// 返回值：一个静态字符串，用于标识该导出器的类型或用途。
    fn name(&self) -> &'static str;
}
