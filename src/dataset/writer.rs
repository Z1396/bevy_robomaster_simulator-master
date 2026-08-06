//! `writer` 模块
//!
//! 数据集写入器，负责将每帧画面与装甲标注保存为标注格式文件。
//! 输出格式：
//! - `images/` 目录：JPG 格式的渲染画面
//! - `label/` 目录：TXT 格式的标签文件，每行一块装甲的标注信息
//!   格式：`<color_id> <type_id> <label_id> <x1> <y1> <x2> <y2> <x3> <y3> <x4> <y4>`

// 装甲类型、装甲编号枚举（项目内部装甲定义）
use crate::robomaster::prelude::{ArmorLabel, ArmorType};
// Bevy引擎基础类型
use bevy::prelude::*;
// image库：用于JPG图片编码保存
use image::ExtendedColorType::Rgb8;
use image::codecs::jpeg::JpegEncoder;
// 文件系统操作
use std::fs::{File, create_dir_all};
use std::io::ErrorKind::Other;
use std::io::{BufWriter, Error, Write};
use std::path::{Path, PathBuf};

/// 装甲所属阵营颜色，映射为数字类别id。
///
/// 每个变体对应一个 u8 类别编号，用于写入标签文件。
#[repr(u8)] // 强制枚举底层为u8，方便直接转数字写入标签文件
#[derive(Debug, Copy, Clone)]
pub enum ArmorColor {
    /// 蓝方装甲
    Blue = 0,
    /// 红方装甲
    Red = 1,
    /// 灰色中立装甲/无效装甲
    Gray = 2,
    /// 紫色特殊装甲（哨兵/能量机关装甲）
    Purple = 3,
}

/// 单块装甲完整标注信息。
///
/// 包含装甲的阵营颜色、类型、编号以及四个角点的归一化坐标。
/// 用于写入数据集标签文件，每块装甲对应一行标注。
#[derive(Debug, Clone)]
pub struct ArmorEntry {
    /// 装甲所属阵营颜色
    pub color: ArmorColor,
    /// 装甲类型：大装甲/小装甲/哨兵装甲等
    pub typ: ArmorType,
    /// 装甲编号：0~5 对应车辆各个装甲板
    pub label: ArmorLabel,
    /// 装甲四个角的归一化像素坐标 [左上、右上、右下、左下]，关键点检测必备
    pub points: [Vec2; 4],
}

/// 数据集写入管理器
pub struct DatasetWriter {
    image_dir: PathBuf,  // 图片保存路径 xxx/images
    label_dir: PathBuf,  // 标签txt保存路径 xxx/label
    seq: u64,            // 帧序号，自增命名 frame_000001、frame_000002……
}

impl DatasetWriter {
    /// 初始化数据集文件夹，不存在则自动创建 images / label 两个子目录
    pub fn new(directory: &str) -> std::io::Result<Self> {
        let base = Path::new(directory);
        let image_dir = base.join("images");
        let label_dir = base.join("label");

        // 递归创建文件夹，目录不存在自动生成
        create_dir_all(&image_dir)?;
        create_dir_all(&label_dir)?;

        Ok(Self {
            image_dir,
            label_dir,
            seq: 0,
        })
    }

    /// 生成下一帧文件名 frame_000001、frame_000002，6位数字补齐，方便排序
    fn next_frame_name(&mut self) -> String {
        self.seq += 1;
        format!("frame_{:06}", self.seq)
    }

    /// 核心写入方法：保存一张画面 + 对应的装甲标签
    /// height/width：图像高宽
    /// data：RGB24格式像素缓冲区
    /// entries：当前画面里所有装甲的标注数组
    pub fn write_entry(
        &mut self,
        height: u32,
        width: u32,
        data: &[u8],
        entries: &[ArmorEntry],
    ) -> std::io::Result<()> {
        let frame = self.next_frame_name();

        // 1. 将RGB像素流编码为jpg图片存入images文件夹
        self.save_image(
            height,
            width,
            data,
            &self.image_dir.join(format!("{}.jpg", frame)),
        )?;

        // 2. 创建标签txt文件，缓冲写入提升IO性能
        let mut writer =
            BufWriter::new(File::create(self.label_dir.join(format!("{}.txt", frame)))?);

        // 遍历画面里每一块装甲，写入一行标签
        for entry in entries {
            // 先写入3个类别编号：阵营颜色、装甲类型、装甲编号
            write!(
                writer,
                "{} {} {}",
                entry.color as u8, entry.typ as u8, entry.label as u8
            )?;
            // 依次写入4个角点的 x y 像素坐标，保留6位小数
            for p in &entry.points {
                write!(writer, " {:.6} {:.6}", p.x, p.y)?;
            }
            // 换行，一块装甲占一行
            writeln!(writer)?;
        }

        writer.flush()?;
        Ok(())
    }

    /// 将RGB原始像素数组压缩保存为JPG图片
    fn save_image(&self, height: u32, width: u32, data: &[u8], path: &Path) -> std::io::Result<()> {
        // 创建JPG编码器，写入文件
        JpegEncoder::new(&mut File::create(path)?)
            // 编码RGB8格式像素数据
            .encode(data, width, height, Rgb8)
            // 将image库的错误转换为标准IO错误
            .map_err(|e| Error::new(Other, e))?;
        Ok(())
    }
}