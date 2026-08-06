//! `dataset` 模块
//!
//! 该模块负责数据集生成与导出，包含以下子模块：
//! - `occlusion`: 装甲遮挡检测与可见性判定
//! - `prelude`: 数据集捕获流水线核心逻辑，包括世界坐标到屏幕坐标的投影、装甲屏幕坐标排序、数据集快照创建等
//! - `writer`: 数据集写入器，将画面帧与装甲标注保存为 JPG 图像和标签文件

pub mod occlusion;
pub mod prelude;
pub mod writer;
