//! 装甲标记点提取模块
//!
//! 定义了 MarkerData 组件，用于存储从装甲 Mesh 中提取的 4 个标记点坐标。
//! 标记点用于识别装甲在三维空间中的位置和朝向。

use crate::robomaster::prelude::extract_vertices;
use bevy::math::Vec3;
use bevy::mesh::Mesh;
use bevy::prelude::{Component, Deref, DerefMut};

#[derive(Component, Deref, DerefMut, Clone)]
pub struct MarkerData(pub [Vec3; 4]);

pub fn extract_markers(mesh: &Mesh) -> Option<[Vec3; 4]> {
    let vertices = extract_vertices(mesh)?;
    if vertices.len() != 4 {
        panic!("Expected 4 vertices but got {}", vertices.len());
    }
    Some(vertices.as_slice().try_into().unwrap())
}
