// 引入项目内部工具：图像捕获标记组件、Transform 拷贝工具函数
use crate::capture::{CaptureSource, copy_transform};
// 渲染资源用途标记
use bevy::asset::RenderAssetUsages;
use bevy::camera::RenderTarget;
// 3D渲染管线、深度预通道依赖
use bevy::core_pipeline::{
    core_3d::graph::{Core3d, Node3d},
    prepass::DepthPrepass,
};
use bevy::ecs::{query::QueryItem, system::lifetimeless::Read};
use bevy::prelude::*;
// 渲染子应用
use bevy::render::RenderApp;
// 提取到渲染世界的相机数据
use bevy::render::camera::ExtractedCamera;
use bevy::render::render_asset::RenderAssets;
// 渲染图节点、自定义渲染通道基础类型
use bevy::render::render_graph::{
    NodeRunError, RenderGraphContext, RenderGraphExt, RenderLabel, ViewNode, ViewNodeRunner,
};
// GPU 资源：指令编码器、纹理拷贝配置、纹理格式、用途掩码
use bevy::render::render_resource::{
    CommandEncoderDescriptor, Extent3d, Origin3d, TexelCopyTextureInfo, TextureAspect,
    TextureDimension, TextureFormat, TextureUsages,
};
// GPU 纹理资源封装
use bevy::render::texture::GpuImage;
// 每个视口对应的深度纹理资源
use bevy::render::view::ViewDepthTexture;

/// 深度捕获相机渲染层级
/// Bevy Camera.order：数值越小渲染越早
/// -101 比普通相机更早渲染，保证深度先绘制完成，不会被主相机遮挡
pub const DEPTH_CAPTURE_CAMERA_ORDER: isize = -101;

/// 全局资源：深度相机配置参数
#[derive(Resource, Clone, Copy)]
pub struct DepthCameraSettings {
    pub width: u32,        // 深度纹理宽度
    pub height: u32,       // 深度纹理高度
    pub fov_y: f32,        // 垂直视场角，和主相机保持一致才能对齐深度
    pub near: f32,         // 近裁剪面
    pub far: f32,          // 远裁剪面，深度有效范围 [near, far]
}

/// 组件标记：用于区分「专门用来采集深度的相机实体」
#[derive(Component)]
pub struct DepthCaptureCamera;

/// 生成深度采集相机实体，全局只生成一次避免重复创建
pub fn setup_depth_capture_camera(world: &mut World) {
    // 查询世界中是否已经存在深度相机，防止重复生成
    let depth_camera_exists = {
        let mut query = world.query_filtered::<Entity, With<DepthCaptureCamera>>();
        query.iter(world).next().is_some()
    };
    if depth_camera_exists {
        return;
    }

    // 读取全局深度相机配置
    let settings = *world.resource::<DepthCameraSettings>();

    // 生成深度相机实体
    world.spawn((
        Camera3d::default(),
        Camera {
            // 更早渲染
            order: DEPTH_CAPTURE_CAMERA_ORDER,
            ..default()
        },
        // 透视投影，和主相机投影参数一致，保证深度空间匹配
        Projection::Perspective(PerspectiveProjection {
            fov: settings.fov_y,
            near: settings.near,
            far: settings.far,
            ..default()
        }),
        // RenderTarget::None：相机正常渲染到屏幕管线，不直接渲染到自定义纹理
        // 后续依靠渲染节点拷贝深度缓冲，而非直接渲染到纹理（性能更优）
        RenderTarget::None {
            size: UVec2::new(settings.width, settings.height),
        },
        // 关闭多重采样抗锯齿，深度不需要AA，节省性能
        Msaa::Off,
        // 开启深度预渲染通道，管线会生成 ViewDepthTexture 深度纹理
        DepthPrepass,
        // 打上专属标记组件
        DepthCaptureCamera,
    ));
}

/// 同步逻辑：深度相机跟随主视觉相机 CaptureSource 的位置姿态
/// Single 单实体查询：
/// target：带 CaptureSource、不带深度相机标记的主相机 Transform（只读）
/// our：带 DepthCaptureCamera、不带 CaptureSource 的深度相机 Transform（可写）
pub fn sync_depth_capture_camera(
    target: Single<&Transform, (With<CaptureSource>, Without<DepthCaptureCamera>)>,
    mut our: Single<&mut Transform, (With<DepthCaptureCamera>, Without<CaptureSource>)>,
) {
    // 调用外部工具函数，把主相机位置、旋转、缩放完整拷贝给深度相机
    copy_transform(&target, &mut our);
}

/// 渲染管线标签：用来命名自定义深度拷贝渲染节点
#[derive(Clone, PartialEq, Eq, Hash, Debug, RenderLabel)]
struct CopyDepthTexturePass;

/// 自定义渲染图节点：执行「相机深度缓冲 → 全局深度纹理」拷贝逻辑
#[derive(Default)]
struct CopyDepthTextureNode;

/// 全局渲染资源：保存接收深度数据的目标纹理句柄
/// Deref/DerefMut 方便直接解包拿到 Handle<Image>
#[derive(Resource, Clone, Deref, DerefMut)]
struct CopyDepthTarget(Handle<Image>);

/// 标记资源：防止重复向渲染管线插入同一个渲染节点
#[derive(Resource, Default)]
struct CopyDepthNodeInstalled(bool);

/// 实现 ViewNode 渲染节点特征，插入 Bevy Core3D 渲染管线
impl ViewNode for CopyDepthTextureNode {
    /// 渲染阶段需要查询的数据：
    /// ExtractedCamera：已经提取到渲染子世界的相机基础数据
    /// ViewDepthTexture：该视口生成的深度纹理
    type ViewQuery = (Read<ExtractedCamera>, Read<ViewDepthTexture>);

    /// 每一帧渲染管线运行此节点
    fn run<'w>(
        &self,
        _graph: &mut RenderGraphContext,
        render_context: &mut bevy::render::renderer::RenderContext<'w>,
        (camera, depth_texture): QueryItem<'w, '_, Self::ViewQuery>,
        world: &'w World,
    ) -> Result<(), NodeRunError> {
        // 只处理我们自己创建的深度相机，跳过场景里其他普通相机
        if camera.order != DEPTH_CAPTURE_CAMERA_ORDER {
            return Ok(());
        }

        // 获取用来存放深度的目标纹理句柄
        let target = world.resource::<CopyDepthTarget>();
        // GPU 纹理资源容器
        let image_assets = world.resource::<RenderAssets<GpuImage>>();
        // 查找目标纹理对应的GPU资源，不存在直接跳过
        let Some(depth_image) = image_assets.get(target.0.id()) else {
            return Ok(());
        };

        // 向渲染上下文提交GPU拷贝任务（异步执行，不阻塞CPU主线程）
        render_context.add_command_buffer_generation_task(move |render_device| {
            // 创建GPU指令编码器，用于录制纹理拷贝指令
            let mut encoder = render_device.create_command_encoder(&CommandEncoderDescriptor {
                label: Some("copy capture depth to texture"),
            });

            // 录制纹理拷贝指令：把相机的深度缓冲，完整复制到自定义深度纹理
            encoder.copy_texture_to_texture(
                // 源纹理：当前相机的深度缓冲，只读取深度Aspect，忽略模板Stencil
                TexelCopyTextureInfo {
                    texture: &depth_texture.texture,
                    mip_level: 0,
                    origin: Origin3d::ZERO,
                    aspect: TextureAspect::DepthOnly,
                },
                // 目标纹理：外部传入的深度纹理
                TexelCopyTextureInfo {
                    texture: &depth_image.texture,
                    mip_level: 0,
                    origin: Origin3d::ZERO,
                    aspect: TextureAspect::DepthOnly,
                },
                // 拷贝区域尺寸
                Extent3d {
                    width: depth_image.size.width,
                    height: depth_image.size.height,
                    depth_or_array_layers: 1,
                },
            );

            // 生成指令缓冲区，送入GPU队列执行
            encoder.finish()
        });

        Ok(())
    }
}

/// 深度纹理拷贝插件主体
pub struct DepthTextureCopyPlugin {
    // 保存深度纹理句柄
    depth_texture: Handle<Image>,
}

impl DepthTextureCopyPlugin {
    /// 构造插件：创建空白深度GPU纹理，初始化纹理用途并注册到资源库
    /// 返回 (插件实例, 深度纹理句柄)，外部拿到句柄即可采样深度
    pub fn new(app: &mut App, width: u32, height: u32) -> (Self, Handle<Image>) {
        // 创建未初始化的2D深度纹理
        let mut depth_image = Image::new_uninit(
            Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            // 格式：32位浮点深度，精度最高，机器人测距首选
            TextureFormat::Depth32Float,
            RenderAssetUsages::default(),
        );
        // 设置纹理用途标记：
        // COPY_DST：允许作为拷贝目标（接收相机深度）
        // COPY_SRC：允许被再次拷贝
        // TEXTURE_BINDING：允许被材质/着色器采样读取深度
        depth_image.texture_descriptor.usage =
            TextureUsages::COPY_DST | TextureUsages::COPY_SRC | TextureUsages::TEXTURE_BINDING;

        // 将纹理插入全局图片资源管理器
        let mut images = app.world_mut().resource_mut::<Assets<Image>>();
        let depth_texture = images.add(depth_image);

        (
            Self {
                depth_texture: depth_texture.clone(),
            },
            depth_texture,
        )
    }
}

/// 插件生命周期实现
impl Plugin for DepthTextureCopyPlugin {
    fn build(&self, app: &mut App) {
        // Bevy 渲染逻辑运行在独立 RenderApp 子应用，必须在渲染子世界注册资源与渲染节点
        let render_app = app.sub_app_mut(RenderApp);

        // 初始化「节点是否已安装」标记资源
        render_app
            .world_mut()
            .init_resource::<CopyDepthNodeInstalled>();
        // 将目标深度纹理存入渲染子世界资源，渲染节点可以拿到
        render_app
            .world_mut()
            .insert_resource(CopyDepthTarget(self.depth_texture.clone()));

        // 避免重复插入渲染节点，防止管线错乱
        let installed = render_app.world().resource::<CopyDepthNodeInstalled>().0;
        if installed {
            return;
        }

        // 将自定义深度拷贝节点挂载到3D核心渲染管线 Core3d
        render_app.add_render_graph_node::<ViewNodeRunner<CopyDepthTextureNode>>(
            Core3d,
            CopyDepthTexturePass,
        );
        // 设置渲染顺序：
        // EndPrepasses（深度预通道结束） → 执行深度拷贝节点 → MainOpaquePass（不透明物体主渲染）
        // 保证深度缓冲生成完毕之后再拷贝，数据完整有效
        render_app.add_render_graph_edges(
            Core3d,
            (
                Node3d::EndPrepasses,
                CopyDepthTexturePass,
                Node3d::MainOpaquePass,
            ),
        );

        // 标记节点已挂载，后续插件重启不会重复插入
        render_app
            .world_mut()
            .resource_mut::<CopyDepthNodeInstalled>()
            .0 = true;
    }
}