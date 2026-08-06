use bevy::ecs::world::DeferredWorld;
// GPU纹理资源封装
use bevy::render::texture::GpuImage;
// Bevy全局异步线程池，用来做解码耗时任务
use bevy::tasks::AsyncComputeTaskPool;
use bevy::{
    image::TextureFormatPixelInfo,
    prelude::*,
    render::{
        Render, RenderApp, RenderSystems,
        // 渲染资产管理器，存放所有加载进GPU的图片纹理
        render_asset::RenderAssets,
        render_graph::{self, NodeRunError, RenderGraph, RenderGraphContext, RenderLabel},
        // 渲染资源：Buffer、纹理拷贝参数、纹理格式等WebGPU底层封装
        render_resource::{
            Buffer, BufferDescriptor, BufferUsages, Extent3d, MapMode, Origin3d,
            TexelCopyBufferInfo, TexelCopyBufferLayout, TexelCopyTextureInfo, TextureAspect,
            TextureFormat, TextureUsages,
        },
        renderer::{RenderContext, RenderDevice},
    },
};
use std::collections::VecDeque;
// 多线程安全容器
use std::sync::{Arc, Mutex};

// 最大正在执行GPU拷贝的帧数量，防止队列无限堆积撑爆显存
const MAX_IN_FLIGHT_FRAMES: usize = 2;

/// 当前采集帧类型：RGB彩色 / 深度32浮点数
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapturedFrameKind {
    Rgb8,
    Depth32F,
}

/// 全局采集配置资源，单个采集实例的参数
#[derive(Resource, Clone)]
pub struct CaptureConfig {
    pub width: u32,
    pub height: u32,
    // GPU纹理格式：Rgba8Unorm / Depth32Float
    pub texture_format: TextureFormat,
    pub frame_kind: CapturedFrameKind,
}

/// 已经从GPU读出、处理完毕的帧结构体，交付上层业务
pub struct CapturedFrame<'a> {
    pub kind: CapturedFrameKind,
    pub width: u32,
    pub height: u32,
    // 帧原始字节数据：RGB8三通道 / 浮点深度字节流
    pub data: &'a [u8],
}

// 类型别名：上层帧捕获处理器 trait 对象
type ToSyncSnapshot = Box<dyn GpuCaptureHandler>;
// 同步阶段回调 trait 对象
type DynSnapshotSync = Box<dyn SnapshotSync>;

// 渲染阶段全局资源：存放所有相机对应的纹理拷贝器
#[derive(Resource, Default, Deref, DerefMut)]
struct ImageCopiers(Vec<ImageCopier>);

// 标记渲染图是否已经挂载过采集节点，避免重复挂载渲染节点
#[derive(Resource, Default)]
struct ImageCopyDriverInstalled(bool);

/// 单路相机纹理拷贝管理器
struct ImageCopier {
    config: CaptureConfig,
    // 需要采集的渲染纹理句柄（相机渲染目标纹理）
    src_image: Handle<Image>,
    /// 帧队列：存放已经提交GPU拷贝、等待GPU完成映射的任务
    /// (GPU缓冲区, 帧回调列表, 宽度, 高度, 纹理格式)
    queue: Mutex<VecDeque<(Buffer, Vec<DynSnapshotSync>, u32, u32, TextureFormat)>>,
    // 空闲Buffer缓冲池，复用GPU缓冲区，减少alloc开销
    free_buffers: Arc<Mutex<Vec<Buffer>>>,
    // 上层注册的帧捕获回调集合
    snapshots: Arc<Vec<ToSyncSnapshot>>,
}

impl ImageCopier {
    /// 构造单路采集器
    pub fn new(
        config: CaptureConfig,
        src_image: Handle<Image>,
        snapshots: Arc<Vec<ToSyncSnapshot>>,
    ) -> ImageCopier {
        ImageCopier {
            config,
            src_image,
            queue: Mutex::new(VecDeque::new()),
            free_buffers: Arc::new(Mutex::new(Vec::new())),
            snapshots,
        }
    }

    /// 获取一个可用GPUBuffer：优先复用空闲池，没有则新建
    fn acquire_buffer(&self, render_device: &RenderDevice, size: u64) -> Buffer {
        // 先从空闲池拿缓存
        if let Some(buf) = self.free_buffers.lock().unwrap().pop() {
            return buf;
        }
        // 新建GPU缓冲区：支持CPU读取、允许纹理拷贝写入
        render_device.create_buffer(&BufferDescriptor {
            label: None,
            size,
            usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }
}

/// 去除GPU纹理行对齐产生的尾部填充字节
/// WebGPU要求每行像素字节必须对齐到256字节，多余部分填充0，需要裁剪掉无效padding
/// padded: 带对齐填充的原始buffer数据
/// row_bytes: 一行真实像素字节长度
/// aligned_row_bytes: GPU对齐后的行字节长度
fn unpad_rows(padded: &[u8], row_bytes: usize, aligned_row_bytes: usize, height: u32) -> Vec<u8> {
    // 刚好对齐，无需裁剪
    if row_bytes == aligned_row_bytes {
        return padded.to_vec();
    }
    let mut out = Vec::with_capacity(row_bytes * height as usize);
    // 按对齐行分割，每行只取有效像素部分
    for row in padded.chunks(aligned_row_bytes).take(height as usize) {
        out.extend_from_slice(&row[..row_bytes.min(row.len())]);
    }
    out
}

/// RGBA4通道带Alpha的纹理 → RGB三通道字节流，剔除Alpha通道，兼容Bgra/Rgba两种排布
fn padded_rgba_to_rgb(
    padded: &[u8],
    width: u32,
    height: u32,
    format: TextureFormat,
) -> Option<Vec<u8>> {
    let pixel_size = format.pixel_size().ok()?;
    // 仅处理4字节像素(RGBA8)
    if pixel_size != 4 {
        return None;
    }
    let row_bytes = width as usize * pixel_size;
    let aligned_row_bytes = RenderDevice::align_copy_bytes_per_row(row_bytes);

    match format {
        // BGRA格式：像素顺序 B G R A，转为 RGB
        TextureFormat::Bgra8UnormSrgb | TextureFormat::Bgra8Unorm => {
            let mut out = Vec::with_capacity(width as usize * height as usize * 3);
            for row in padded.chunks(aligned_row_bytes).take(height as usize) {
                let row = &row[..row_bytes.min(row.len())];
                for px in row.chunks_exact(4) {
                    out.extend_from_slice(&[px[2], px[1], px[0]]);
                }
            }
            Some(out)
        }
        // RGBA格式：R G B A，直接丢弃A通道
        TextureFormat::Rgba8UnormSrgb | TextureFormat::Rgba8Unorm => {
            let mut out = Vec::with_capacity(width as usize * height as usize * 3);
            for row in padded.chunks(aligned_row_bytes).take(height as usize) {
                let row = &row[..row_bytes.min(row.len())];
                for px in row.chunks_exact(4) {
                    out.extend_from_slice(&[px[0], px[1], px[2]]);
                }
            }
            Some(out)
        }
        _ => None,
    }
}

/// 判断纹理读取Aspect：深度纹理只读取深度平面，彩色纹理读取全部颜色平面
fn capture_texture_aspect(format: TextureFormat) -> TextureAspect {
    if matches!(
        format,
        TextureFormat::Depth16Unorm
            | TextureFormat::Depth24Plus
            | TextureFormat::Depth24PlusStencil8
            | TextureFormat::Depth32Float
            | TextureFormat::Depth32FloatStencil8
    ) {
        TextureAspect::DepthOnly
    } else {
        TextureAspect::All
    }
}

// ====================== 三层回调Trait 设计 ======================
// 生命周期拆分：渲染同步阶段 → 异步解码阶段 → 帧送达阶段
/// 阶段1：渲染线程同步创建异步回调对象，运行在Render线程
pub trait SnapshotSync: Send {
    fn captured(
        self: Box<Self>,
        world: &mut DeferredWorld,
        config: &CaptureConfig,
    ) -> Box<dyn SnapshotAsync>;
}

/// 阶段2：异步线程池解码完成后，送入最终回调，运行在异步线程
pub trait SnapshotAsync: Send {
    fn captured(&mut self, frame: CapturedFrame<'_>);
}

/// 阶段0：每一帧渲染开始前，同步查询上层是否需要采集本帧画面
pub trait GpuCaptureHandler: Send + Sync + 'static {
    fn captured(&self, world: &World) -> Option<Box<dyn SnapshotSync>>;
}

/// 渲染图节点标签，用于插入渲染管线顺序
#[derive(Debug, PartialEq, Eq, Clone, Hash, RenderLabel)]
struct ImageCopy;

/// 渲染图节点实现：在相机渲染完毕后，执行「纹理拷贝至GPU Buffer」指令
#[derive(Default)]
struct ImageCopyDriver;

impl render_graph::Node for ImageCopyDriver {
    fn run(
        &self,
        _graph: &mut RenderGraphContext,
        render_context: &mut RenderContext,
        world: &World,
    ) -> Result<(), NodeRunError> {
        // 没有采集器直接跳过
        let Some(copiers) = world.get_resource::<ImageCopiers>() else {
            return Ok(());
        };
        let Some(gpu_images) = world.get_resource::<RenderAssets<GpuImage>>() else {
            return Ok(());
        };

        // 遍历每一路相机采集器
        for copier in copiers.iter() {
            // 目标渲染纹理尚未上传GPU，跳过
            let Some(src_image) = gpu_images.get(&copier.src_image) else {
                continue;
            };

            // 计算纹理对齐字节，WebGPU拷贝行对齐规则
            let block_dimensions = src_image.texture_format.block_dimensions();
            let block_size = src_image.texture_format.block_copy_size(None).unwrap();
            let padded_bytes_per_row = RenderDevice::align_copy_bytes_per_row(
                (src_image.size.width as usize / block_dimensions.0 as usize) * block_size as usize,
            );
            let buffer_size = padded_bytes_per_row as u64 * src_image.size.height as u64;
            // 取出/创建GPU缓冲区
            let buffer = copier.acquire_buffer(render_context.render_device(), buffer_size);

            // 收集当前帧所有上层回调
            let snapshot: Vec<DynSnapshotSync> = copier
                .snapshots
                .iter()
                .filter_map(|handler| handler.captured(world))
                .collect();

            // 向GPU命令队列写入指令：纹理 → GPU缓冲区（GPU异步执行，不阻塞CPU）
            render_context.command_encoder().copy_texture_to_buffer(
                TexelCopyTextureInfo {
                    texture: &src_image.texture,
                    mip_level: 0,
                    origin: Origin3d::ZERO,
                    aspect: capture_texture_aspect(src_image.texture_format),
                },
                TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(
                            std::num::NonZero::<u32>::new(padded_bytes_per_row as u32)
                                .unwrap()
                                .into(),
                        ),
                        rows_per_image: None,
                    },
                },
                src_image.size,
            );

            // 将本次任务入队，等待后续GPU完成映射
            let mut queue = copier.queue.lock().unwrap();
            queue.push_back((
                buffer,
                snapshot,
                src_image.size.width,
                src_image.size.height,
                src_image.texture_format,
            ));

            // 超出最大并发帧数，丢弃最早帧、回收Buffer，防止显存溢出
            while queue.len() > MAX_IN_FLIGHT_FRAMES {
                if let Some((buffer, _, _, _, _)) = queue.pop_front() {
                    copier.free_buffers.lock().unwrap().push(buffer);
                }
            }
        }
        Ok(())
    }
}

/// Render阶段系统：渲染完成后，处理队列中已完成GPU拷贝的Buffer
/// 执行Buffer映射到CPU内存、异步解码逻辑
fn receive_image_from_buffer(mut world: DeferredWorld) {
    let copier_count = world.resource::<ImageCopiers>().len();
    if copier_count == 0 {
        return;
    }

    for idx in 0..copier_count {
        // 取出队列头部待处理帧
        let next = {
            let copiers = world.resource::<ImageCopiers>();
            let Some(copier) = copiers.get(idx) else {
                continue;
            };
            let mut guard = copier.queue.lock().unwrap();
            guard
                .pop_front()
                .map(|(buffer, snapshots, width, height, texture_format)| {
                    (
                        buffer,
                        snapshots,
                        width,
                        height,
                        texture_format,
                        copier.free_buffers.clone(),
                        copier.config.clone(),
                    )
                })
        };

        let Some((buffer, snapshots, width, height, texture_format, free_buffers, config)) = next
        else {
            continue;
        };

        // 创建oneshot通道，等待GPU映射完成后接收字节数据
        let (s, r) = futures::channel::oneshot::channel();
        let buffer_slice = buffer.slice(..);
        let buffer_for_map = buffer.clone();

        // 异步映射GPU缓冲区到CPU内存（GPU完成拷贝后触发回调）
        buffer_slice.map_async(MapMode::Read, move |res| {
            res.expect("Failed to map buffer");
            let buffer_slice = buffer_for_map.slice(..);
            // 将显存数据拷贝至CPU堆内存
            let data = buffer_slice.get_mapped_range();
            let dat = data.to_vec();
            drop(data);
            // 解除映射，归还缓冲区到空闲池复用
            buffer_for_map.unmap();
            free_buffers.lock().unwrap().push(buffer_for_map);
            // 把字节数据发送给异步解码线程
            s.send(dat).expect("Failed to send map update");
        });

        // 在DeferredWorld同步阶段初始化异步回调
        let snapshots: Vec<Box<dyn SnapshotAsync>> = snapshots
            .into_iter()
            .map(|v| v.captured(&mut world, &config))
            .collect();
        let frame_kind = config.frame_kind;

        // 送入Bevy全局异步线程池解码，避免阻塞渲染主线程
        AsyncComputeTaskPool::get()
            .spawn(async move {
                // 等待GPU映射完毕拿到原始带对齐Padding的字节
                let padded = r.await.expect("Failed to receive the map_async message");
                let frame_bytes = match frame_kind {
                    CapturedFrameKind::Rgb8 => {
                        // RGBA转RGB；失败则降级：去除padding后转RGB8
                        padded_rgba_to_rgb(&padded, width, height, texture_format).unwrap_or_else(
                            || {
                                let pixel_size = texture_format
                                    .pixel_size()
                                    .expect("Unsupported capture texture format");
                                let row_bytes = width as usize * pixel_size;
                                let aligned_row_bytes =
                                    RenderDevice::align_copy_bytes_per_row(row_bytes);
                                let unpadded =
                                    unpad_rows(&padded, row_bytes, aligned_row_bytes, height);
                                let mut bevy_image = Image::new_target_texture(
                                    width,
                                    height,
                                    texture_format,
                                    Some(texture_format),
                                );
                                bevy_image.data = Some(unpadded);
                                bevy_image.try_into_dynamic().unwrap().to_rgb8().into_raw()
                            },
                        )
                    }
                    CapturedFrameKind::Depth32F => {
                        // 深度纹理只去除行Padding，保留原始f32字节
                        let pixel_size = texture_format
                            .pixel_size()
                            .expect("Unsupported depth capture texture format");
                        let row_bytes = width as usize * pixel_size;
                        let aligned_row_bytes = RenderDevice::align_copy_bytes_per_row(row_bytes);
                        unpad_rows(&padded, row_bytes, aligned_row_bytes, height)
                    }
                };

                // 分发帧数据给所有注册的回调（ROS发帧、推理、存图等）
                for mut snapshot in snapshots {
                    snapshot.captured(CapturedFrame {
                        kind: frame_kind,
                        width,
                        height,
                        data: frame_bytes.as_slice(),
                    });
                }
            })
            .detach();
    }
}

/// 相机采集插件构造器
pub struct CameraCapturePlugin {
    config: CaptureConfig,
    snapshots: Arc<Vec<ToSyncSnapshot>>,
    handle: Handle<Image>,
    expose_config_resource: bool,
}

impl CameraCapturePlugin {
    /// 创建采集目标纹理+插件实例，自动创建渲染纹理
    pub fn new(
        app: &mut App,
        config: CaptureConfig,
        snapshots: Vec<ToSyncSnapshot>,
    ) -> (Self, Handle<Image>) {
        let extent = Extent3d {
            width: config.width,
            height: config.height,
            ..Default::default()
        };
        // 创建可被相机渲染、并且允许作为拷贝源的渲染纹理
        let mut render_target_image = Image::new_target_texture(
            extent.width,
            extent.height,
            config.texture_format,
            Some(config.texture_format),
        );
        // 增加 COPY_SRC 用途：允许GPU把纹理拷贝进Buffer
        render_target_image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
        let mut images = app.world_mut().resource_mut::<Assets<Image>>();
        let handle = images.add(render_target_image);

        (
            Self {
                config,
                snapshots: Arc::new(snapshots),
                handle: handle.clone(),
                expose_config_resource: true,
            },
            handle,
        )
    }

    /// 绑定已经存在的渲染纹理（复用已有相机渲染目标）
    pub fn from_existing_handle(
        config: CaptureConfig,
        handle: Handle<Image>,
        snapshots: Vec<ToSyncSnapshot>,
    ) -> Self {
        Self {
            config,
            snapshots: Arc::new(snapshots),
            handle,
            expose_config_resource: false,
        }
    }
}

impl Plugin for CameraCapturePlugin {
    // 允许实例化多次，实现多路相机同时采集
    fn is_unique(&self) -> bool {
        false
    }

    fn build(&self, app: &mut App) {
        if self.expose_config_resource {
            app.insert_resource(self.config.clone());
        }

        // 渲染子App（Bevy分离主线程/渲染线程）
        let render_app = app.sub_app_mut(RenderApp);
        render_app.world_mut().init_resource::<ImageCopiers>();
        render_app
            .world_mut()
            .init_resource::<ImageCopyDriverInstalled>();

        // 将当前采集器存入渲染阶段全局集合
        {
            let mut copiers = render_app.world_mut().resource_mut::<ImageCopiers>();
            copiers.push(ImageCopier::new(
                self.config.clone(),
                self.handle.clone(),
                self.snapshots.clone(),
            ));
        }

        // 全局只挂载一次渲染图节点，所有采集器共用同一个渲染节点
        let installed = render_app.world().resource::<ImageCopyDriverInstalled>().0;
        if !installed {
            let mut graph = render_app.world_mut().resource_mut::<RenderGraph>();
            // 向渲染图插入纹理拷贝节点
            graph.add_node(ImageCopy, ImageCopyDriver);
            // 挂载在相机渲染节点之后，保证相机渲染完成再拷贝纹理
            graph.add_node_edge(bevy::render::graph::CameraDriverLabel, ImageCopy);
            drop(graph);

            // 标记节点已安装，避免重复添加
            render_app
                .world_mut()
                .resource_mut::<ImageCopyDriverInstalled>()
                .0 = true;
            // 在渲染阶段末尾执行缓冲区映射处理系统
            render_app.add_systems(
                Render,
                receive_image_from_buffer.after(RenderSystems::Render),
            );
        }

        if self.expose_config_resource {
            render_app.insert_resource(self.config.clone());
        }
    }
}