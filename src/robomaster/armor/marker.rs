//! 装甲标记点提取模块
//!
//! 定义了 MarkerData 组件，用于存储从装甲 Mesh 中提取的 4 个标记点坐标。
//! 标记点用于识别装甲在三维空间中的位置和朝向。

use crate::robomaster::prelude::extract_vertices;
use bevy::math::Vec3;
use bevy::mesh::Mesh;
use bevy::prelude::{Component, Deref, DerefMut};

#[derive(Component, Deref, DerefMut, Clone)]
/// 装甲标记点组件，存储 4 个标记点的三维坐标，用于定位和识别装甲
pub struct MarkerData(pub [Vec3; 4]);

/// 从装甲 Mesh 中提取 4 个标记点
///
/// 要求网格必须恰好包含 4 个顶点，否则会 panic。
/// 内部调用 extract_vertices 获取顶点列表，然后转换为固定大小数组。
pub fn extract_markers(mesh: &Mesh) -> Option<[Vec3; 4]> {
    let vertices = extract_vertices(mesh)?;
    if vertices.len() != 4 {
        panic!("Expected 4 vertices but got {}", vertices.len());
    }
    Some(vertices.as_slice().try_into().unwrap())
}
