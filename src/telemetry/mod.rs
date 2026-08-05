// =====================================================================
// 模块名：telemetry
// 作用：遥测系统入口与管线管理
// 职责：
//   1. 通过 TelemetryPlugin 接入 Bevy ECS 生命周期，在 PostUpdate 阶段
//      自动构造帧遥测数据（FrameData）并分发给所有注册的导出器；
//   2. 通过 TelemetryPipeline 全局资源统一管理多个遥测导出实例，
//      支持同时向多目的地（日志、UDP、文件、ROS、控制台等）上报；
//   3. 声明并导出 exporter（导出器特征）与 frame_data（帧数据结构）子模块。
// =====================================================================

// 私有模块声明：只在当前文件所在模块内部可见，外部无法直接访问 exporter、frame_data 模块
mod exporter;
mod frame_data;

// 把 exporter、frame_data 模块内所有公开项全部导出到当前作用域，外部 crate 导入本模块后可直接使用其中类型
pub use exporter::*;
pub use frame_data::*;

// 导入 Bevy 引擎全部常用前置类型（App、Plugin、Resource、System、ResMut、Res、Time 等）
use bevy::prelude::*;

/// 遥测插件，接入 Bevy ECS 生命周期，作为遥测系统的入口插件。
///
/// 用户在 `app.add_plugin(TelemetryPlugin)` 即可开启整套遥测上报功能。
pub struct TelemetryPlugin;

// 实现 Bevy 的 Plugin 特征，让 TelemetryPlugin 成为标准 Bevy 插件
impl Plugin for TelemetryPlugin {
    /// Bevy 插件构建函数，程序启动注册系统、资源。
    fn build(&self, app: &mut App) {
        // 初始化全局资源 TelemetryPipeline，全局唯一，用来管理所有遥测导出器
        app.init_resource::<TelemetryPipeline>()
            // 在 PostUpdate 阶段注册遥测分发系统
            // PostUpdate：所有实体更新、物理计算、渲染前置逻辑完成之后执行，保证拿到本帧最终状态数据
            .add_systems(PostUpdate, telemetry_dispatch);
    }
}

/// 全局遥测管线资源（全局单例 Resource）。
///
/// 作用：统一管理多个遥测导出实例，支持同时多目的地上报
/// （日志、UDP、文件、ROS、控制台等）。
#[derive(Resource, Default)]
pub struct TelemetryPipeline {
    /// 动态数组，存放任意实现 TelemetryExporter 特征的导出器。
    /// `Box<dyn Trait>`：特征对象，存放不同类型的导出器；
    /// `Send + Sync`：满足 Bevy 多线程安全约束。
    exporters: Vec<Box<dyn TelemetryExporter + Send + Sync>>,
}

impl TelemetryPipeline {
    /// 添加一个遥测导出器。
    ///
    /// 参数：
    /// - `exporter`：任意实现遥测导出特征、线程安全、生命周期静态的类型实例。
    pub fn add_exporter<E: TelemetryExporter + Send + Sync + 'static>(&mut self, exporter: E) {
        // 装箱转为特征对象存入数组，实现异构容器（数组里可以放日志导出、网络导出等不同类型实例）
        self.exporters.push(Box::new(exporter));
    }

    /// 分发帧数据：遍历所有导出器，依次调用各自的帧回调方法上报数据。
    ///
    /// 参数：
    /// - `data`：本帧遥测数据包（只读引用）。
    pub fn dispatch(&mut self, data: &FrameData) {
        for exporter in &mut self.exporters {
            exporter.on_frame(data);
        }
    }

    /// 获取当前注册的导出器数量。
    pub fn exporter_count(&self) -> usize {
        self.exporters.len()
    }
}

/// Bevy 系统：每帧自动执行，构造帧遥测数据包并分发上报。
///
/// 在 PostUpdate 阶段运行，确保拿到本帧最终状态数据。
fn telemetry_dispatch(mut pipeline: ResMut<TelemetryPipeline>, time: Res<Time>) {
    // 如果没有注册任何导出器，直接退出，避免无用遍历，节约性能
    if pipeline.exporter_count() == 0 {
        return;
    }

    // 组装当前帧遥测数据包 FrameData
    let frame_data = FrameData {
        // 当前程序运行的总时间（秒，浮点高精度时间戳），用于帧时序对齐
        timestamp: time.elapsed_secs_f64(),
        // 装甲板检测结果数组，后续感知系统会填充装甲坐标、颜色、ID等
        armors: Vec::new(),
        // 各机器人/相机位姿表：实体ID → 位姿（平移+旋转）
        poses: std::collections::HashMap::new(),
    };

    // 将本帧数据分发给所有注册的导出器完成上报
    pipeline.dispatch(&frame_data);
}
