// ============================================================================
// 模块名：ros2::image
// 作  用：ROS2 图像压缩工具，将 RGB 原始像素编码为 JPEG 压缩图像消息
// 职  责：
//   1. 接收 RGB 像素数据与图像尺寸，封装为 sensor_msgs/CompressedImage
//   2. 使用 `image` 库的 JpegEncoder 进行 JPEG 编码
//   3. 保留消息头（时间戳、frame_id），格式字段固定为 "jpeg"
// ============================================================================

use r2r::sensor_msgs::msg::CompressedImage;
use r2r::std_msgs::msg::Header;

/// 将 RGB 原始像素数据压缩为 JPEG 并封装为 ROS2 CompressedImage 消息。
///
/// # 参数
/// - `header`：消息头（含时间戳与 frame_id，由调用方填充）
/// - `width`：图像宽度（像素）
/// - `height`：图像高度（像素）
/// - `data`：RGB 像素数据，长度应为 `width * height * 3`
///
/// # 返回值
/// 返回 `CompressedImage`，`format` 字段为 "jpeg"，`data` 为 JPEG 编码后的字节流。
///
/// # 算法步骤
/// 1. 使用 `ImageBuffer::from_raw` 将原始字节包装为 Rgb 图像缓冲区
/// 2. 创建内存 `Cursor` 作为输出缓冲区，构造 `JpegEncoder`
/// 3. 调用 `encode_image` 进行 JPEG 编码（失败时 panic）
/// 4. 取出 Cursor 内部字节流，封装为 CompressedImage 返回
pub fn compress_image(header: Header, width: u32, height: u32, data: &[u8]) -> CompressedImage {
    use image::codecs::jpeg::JpegEncoder;
    use image::{ImageBuffer, Rgb};
    use std::io::Cursor;

    // 将原始字节包装为 Rgb<u8> 图像缓冲区（不拷贝，仅借用切片）
    let buffer: ImageBuffer<Rgb<u8>, _> = ImageBuffer::from_raw(width, height, data).unwrap();

    // 使用内存 Cursor 作为 JPEG 编码输出
    let mut cursor = Cursor::new(Vec::new());
    let mut encoder = JpegEncoder::new(&mut cursor);
    // 执行 JPEG 编码；失败时直接 panic（仿真环境下视为不可恢复错误）
    encoder.encode_image(&buffer).expect("JPEG encode failed");
    // 取出编码后的字节流
    let compressed_data = cursor.into_inner();

    CompressedImage {
        header,
        // 格式字段固定为 "jpeg"，订阅端据此选择解码器
        format: "jpeg".to_string(),
        data: compressed_data,
    }
}
