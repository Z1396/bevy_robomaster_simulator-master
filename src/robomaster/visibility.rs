// 工具函数：设置实体 Visibility 显隐状态
use crate::util::bevy::set_visibility;
use bevy::app::App;
// 材质资源句柄ID
use bevy::asset::AssetId;
// 线性RGBA颜色，用于材质自发光 emissive
use bevy::color::LinearRgba;
// ECS系统参数读写标记，Read/Write 避免重复借用冲突
use bevy::ecs::system::lifetimeless::{Read, Write};
use bevy::prelude::{Children, Plugin, Resource};
use bevy::{
    asset::{Assets, Handle},
    // 实体可见性组件
    camera::visibility::Visibility,
    ecs::{
        entity::Entity,
        system::{Query, ResMut, SystemParam},
    },
    // PBR物理材质、材质绑定组件
    pbr::{MeshMaterial3d, StandardMaterial},
};
use std::collections::HashMap;
use std::hash::Hash;

/// # StatefulAppearance
/// Bevy SystemParam，系统参数集，集中管理外观修改所需所有查询/资源
/// 被各个状态更新系统注入，统一用来改材质、改显隐
#[derive(SystemParam)]
pub struct StatefulAppearance<'w, 's> {
    /// 全局材质资源仓库，可以新增/修改StandardMaterial材质
    materials: ResMut<'w, Assets<StandardMaterial>>,
    /// 材质静音缓存资源，缓存"熄灭无光版本材质"
    cache: ResMut<'w, MaterialCache>,
    /// 查询：写入实体绑定的材质（用来替换不同状态的材质）
    mesh_materials: Query<'w, 's, Write<MeshMaterial3d<StandardMaterial>>>,
    /// 查询：写入实体Visibility可见性，控制显示/隐藏
    visibilities: Query<'w, 's, Write<Visibility>>,
}

/// 控制器统一行为特征：任意控制器都可以根据状态更新外观
pub trait Control {
    /// 根据当前激活状态，执行材质/显隐切换
    /// state：当前部件生命周期状态
    /// param：外观操作上下文
    fn set(&mut self, state: Activation, param: &mut StatefulAppearance);
}

/// 部件四阶段生命周期枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Activation {
    Deactivated = 0,  // 未激活、熄灭、待机
    Activating = 1,   // 激活过程中
    Activated = 2,    // 已经激活、正常工作
    Completed = 3,    // 任务完成、结束阶段
}

/// 外观控制器枚举：三种控制模式
pub enum Controller {
    /// 材质控制器
    /// 绑定实体 + 四个阶段分别对应的材质句柄
    Material(
        Entity,
        Handle<StandardMaterial>, // Deactivated材质
        Handle<StandardMaterial>, // Activating材质
        Handle<StandardMaterial>, // Activated材质
        Handle<StandardMaterial>, // Completed材质
    ),

    /// 显隐控制器
    /// 每个状态对应要显示的实体，其余状态实体隐藏
    Visibility(
        Option<Entity>, // Deactivated阶段显示的实体
        Option<Entity>, // Activating阶段显示的实体
        Option<Entity>, // Activated阶段显示的实体
        Option<Entity>, // Completed阶段显示的实体
    ),

    /// 组合控制器：嵌套多个Controller，实现材质+显隐同时控制
    Combined(Vec<Controller>),
}

/// 为控制器实现统一 Control 特征分发逻辑
impl Control for Controller {
    fn set(&mut self, state: Activation, param: &mut StatefulAppearance) {
        match self {
            // ========== 材质分支：根据状态替换实体材质 ==========
            Self::Material(entity, deactivated, activating, activated, completed) => {
                // 根据当前状态选出对应材质句柄
                let apply = match state {
                    Activation::Deactivated => deactivated,
                    Activation::Activating => activating,
                    Activation::Activated => activated,
                    Activation::Completed => completed,
                };
                // 查询到实体材质组件并替换材质
                if let Ok(mut mesh_material) = param.mesh_materials.get_mut(*entity) {
                    mesh_material.0 = apply.clone()
                }
            }

            // ========== 显隐分支：只展示当前状态对应的实体，其余全部隐藏 ==========
            Self::Visibility(deactivated, activating, activated, completed) => {
                let (show, hide) = match state {
                    // 当前状态需要展示的实体、其余三个状态的实体全部隐藏
                    Activation::Deactivated => (deactivated, [activating, activated, completed]),
                    Activation::Activating => (activating, [deactivated, activated, completed]),
                    Activation::Activated => (activated, [deactivated, activating, completed]),
                    Activation::Completed => (completed, [deactivated, activating, activated]),
                };
                // 遍历需要隐藏的实体，全部设为 Hidden
                for entity in hide.into_iter().flatten() {
                    set_visibility(*entity, Visibility::Hidden, &mut param.visibilities).unwrap();
                }
                // 展示当前状态对应的实体
                if let Some(show) = show {
                    set_visibility(*show, Visibility::Visible, &mut param.visibilities).unwrap();
                }
            }

            // ========== 组合模式：递归执行内部所有控制器 ==========
            Self::Combined(vec) => {
                for c in vec {
                    c.set(state, param);
                }
            }
        }
    }
}

/// Controller 静态构造方法，方便外部创建控制器
impl Controller {
    /// 构建显隐控制器
    pub fn new_visibility(
        deactivated: Option<Entity>,
        activating: Option<Entity>,
        activated: Option<Entity>,
        completed: Option<Entity>,
    ) -> Self {
        Self::Visibility(deactivated, activating, activated, completed)
    }

    /// 构建材质控制器
    pub fn new_material(
        entity: Entity,
        deactivated: Handle<StandardMaterial>,
        activating: Handle<StandardMaterial>,
        activated: Handle<StandardMaterial>,
        completed: Handle<StandardMaterial>,
    ) -> Self {
        Self::Material(entity, deactivated, activating, activated, completed)
    }

    /// 构建组合控制器
    pub fn new_combined(v: Vec<Controller>) -> Self {
        Self::Combined(v)
    }
}

/// 全局材质缓存资源
/// 用途：把原有发光材质生成「自发光关闭的静音版本」，存入哈希表缓存，避免重复克隆材质
#[derive(Resource, Default)]
struct MaterialCache {
    /// key：原始材质ID，value：无光静音材质句柄
    muted: HashMap<AssetId<StandardMaterial>, Handle<StandardMaterial>>,
}

impl MaterialCache {
    /// 获取某个材质对应的无光版本，不存在则创建并存入缓存
    fn ensure_muted(
        &mut self,
        handle: &Handle<StandardMaterial>,
        materials: &mut Assets<StandardMaterial>,
    ) -> Handle<StandardMaterial> {
        let id = handle.id();
        // 缓存命中，直接返回
        if let Some(existing) = self.muted.get(&id) {
            return existing.clone();
        }
        // 获取原始材质，取不到则返回原材质兜底
        let Some(original) = materials.get(handle) else {
            return handle.clone();
        };
        // 复制材质，关闭自发光，做成熄灭状态材质
        let mut clone = original.clone();
        clone.emissive = LinearRgba::BLACK;
        clone.emissive_exposure_weight = 0.0;
        // 将新材质存入全局材质库
        let muted_handle = materials.add(clone);
        // 写入缓存
        self.muted.insert(id, muted_handle.clone());
        muted_handle
    }
}

/// 构造控制器时的回调参数别名
/// (当前实体, 外观操作上下文)
pub type ConstructData<'w, 's, 'g> = (Entity, &'g mut StatefulAppearance<'w, 's>);

impl<'w, 's> StatefulAppearance<'w, 's> {
    /// 查询实体当前是否可见
    pub fn visible(&self, entity: Entity) -> bool {
        match self.visibilities.get(entity) {
            Ok(v) => v != Visibility::Hidden,
            // 查不到Visibility组件默认视为可见
            Err(_) => true,
        }
    }
}

/// 外观控制器生成器：遍历实体自身+所有子孙节点，批量生成Controller
#[derive(SystemParam)]
pub struct StatefulAppearanceCreator<'w, 's> {
    pub appearance: StatefulAppearance<'w, 's>,
    /// 查询实体父子Children关系，用于遍历子孙实体
    children: Query<'w, 's, Read<Children>>,
}

impl<'w, 's> StatefulAppearanceCreator<'w, 's> {
    /// 遍历单个实体 + 它所有后代，逐个执行回调生成控制器，最终合并为Combined
    fn as_combined<F: for<'g> Fn(ConstructData<'w, 's, 'g>) -> Result<Controller, ()>>(
        &mut self,
        entity: Entity,
        f: &F,
    ) -> Controller {
        let mut swaps = vec![];
        // 处理本体
        if let Ok(v) = f((entity, &mut self.appearance)) {
            swaps.push(v);
        }
        // 递归遍历所有子孙实体一并处理
        for child in self.children.iter_descendants(entity) {
            if let Ok(v) = f((child, &mut self.appearance)) {
                swaps.push(v);
            }
        }
        Controller::new_combined(swaps)
    }

    /// 批量传入一批根实体，批量生成总控制器
    pub fn create_controller<F: for<'g> Fn(ConstructData<'w, 's, 'g>) -> Result<Controller, ()>>(
        &mut self,
        entities: Vec<Entity>,
        f: F,
    ) -> Controller {
        let mut controllers = Vec::new();
        for entity in entities {
            controllers.push(self.as_combined(entity, &f));
        }
        Controller::new_combined(controllers)
    }
}

/// 高阶函数：快速构建材质切换回调
/// 逻辑：读取实体当前材质，自动分离「发光on材质 / 无光off材质」，交给外部回调组装材质控制器
pub fn material_raw<F>(f: F) -> impl Fn(ConstructData) -> Result<Controller, ()>
where
    F: Fn(Entity, Handle<StandardMaterial>, Handle<StandardMaterial>) -> Controller,
{
    move |value: ConstructData| -> Result<Controller, ()> {
        let (entity, param) = value;
        // 获取实体绑定的材质
        if let Ok(mut mesh_material) = param.mesh_materials.get_mut(entity) {
            // 取出该材质对应的无光熄灭材质
            let off = param
                .cache
                .ensure_muted(&mesh_material.0, &mut param.materials);
            // 把实体当前材质替换成无光材质，取出原始发光材质on
            let on = std::mem::replace(&mut mesh_material.0, off.clone());
            // 交给外部闭包构造材质控制器
            Ok(f(entity, on, off))
        } else {
            Err(())
        }
    }
}

// ===================== 辅助宏：赋值分支hack =====================
/// 内部赋值工具宏，根据传入状态标识符，给对应变量赋值
#[macro_export]
macro_rules! internal_assign_hack {
    (@internal deactivated, $value:expr, $d:ident, $a:ident, $ac:ident, $c:ident) => {
        $d = $value;
    };
    (@internal activating, $value:expr, $d:ident, $a:ident, $ac:ident, $c:ident) => {
        $a = $value;
    };
    (@internal activated, $value:expr, $d:ident, $a:ident, $ac:ident, $c:ident) => {
        $ac = $value;
    };
    (@internal completed, $value:expr, $d:ident, $a:ident, $ac:ident, $c:ident) => {
        $c = $value;
    };
}

// ===================== 业务语法糖宏：visibility! 快速创建显隐控制器回调 =====================
/// 示例：visibility(activated)
/// 含义：只有Activated阶段显示当前实体，其余阶段隐藏
#[macro_export]
macro_rules! visibility {
    ($($state:ident),* $(,)?) => {
        |value: $crate::robomaster::visibility::ConstructData| -> Result<$crate::robomaster::visibility::Controller, ()> {
            use ::std::option::Option::{Some, None};
            use $crate::internal_assign_hack;
            // 四个阶段默认无绑定实体
            let mut _deactivated = None;
            let mut _activating = None;
            let mut _activated = None;
            let mut _completed = None;

            // 遍历传入的状态标识符，将当前实体绑定到对应阶段
            $(
                internal_assign_hack!(@internal $state, Some(value.0), _deactivated, _activating, _activated, _completed);
            )*

            Ok($crate::robomaster::visibility::Controller::new_visibility(_deactivated, _activating, _activated, _completed))
        }
    };
}

// ===================== 业务语法糖宏：material! 快速创建材质控制器回调 =====================
/// 示例：material(on = {activated, activating})
/// 含义：激活中、已激活阶段使用发光材质，其余阶段熄灭无光材质
#[macro_export]
macro_rules! material {
    ( on = {$($on:ident),* $(,)?}) => {
         $crate::robomaster::visibility::material_raw(|entity, on, off| {
             use $crate::internal_assign_hack;

             // 默认全部阶段使用无光材质off
             let mut _deactivated = off.clone();
             let mut _activating = off.clone();
             let mut _activated = off.clone();
             let mut _completed = off.clone();

             // 指定的阶段替换为发光材质on
             $(
                internal_assign_hack!(@internal $on, on.clone(), _deactivated, _activating, _activated, _completed);
             )*
             $crate::robomaster::visibility::Controller::new_material(entity, _deactivated, _activating, _activated, _completed)
         })
    };
}

/// 本模块插件，只初始化材质缓存资源
#[derive(Default)]
pub(super) struct StatefulAppearancePlugin;

impl Plugin for StatefulAppearancePlugin {
    fn build(&self, app: &mut App) {
        // 插入材质缓存资源，全局唯一
        app.init_resource::<MaterialCache>();
    }
}