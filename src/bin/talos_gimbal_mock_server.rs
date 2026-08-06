// 命令行参数解析库，解析启动参数 input/loop/fps/log_every
use clap::Parser;
// FFmpeg Rust绑定，实现视频解码、图像缩放、像素格式转换
use ffmpeg_next as ffmpeg;
use std::error::Error;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// 自研IPC库，基于共享内存实现零拷贝进程通信，替代本地模块通信
// ShmPublisher：共享内存发布端 → 向外推送图像、位姿、相机内参
// ShmSubscriber：共享内存订阅端 → 接收自瞄进程下发的云台控制指令
// IMAGE_WIDTH/HEIGHT/SIZE：全局统一图像尺寸，SIZE=W*H*3(RGB24)
use talos_ipc::{
    CameraInfo, IMAGE_HEIGHT, IMAGE_SIZE, IMAGE_WIDTH, PoseIndex, ShmPublisher, ShmSubscriber,
};

// 通用错误别名：任意错误类型，支持跨线程Send+Sync，简化Result写法
type DynError = Box<dyn Error + Send + Sync + 'static>;

/// 命令行参数结构体，启动时通过命令行传入配置
#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Talos mock server: ffmpeg 视频输入 + 接收 GimbalCmd"
)]
struct Args {
    /// 视频源：本地文件路径 / rtsp:// / rtmp:// / http:// 流媒体地址
    #[arg(long, short)]
    input: String,

    /// 视频播放至末尾EOF后是否循环重播
    #[arg(long, default_value_t = false)]
    r#loop: bool,

    /// 图像发布帧率节流，模拟真实相机帧率；<=0 关闭限流，全速推送
    #[arg(long, default_value_t = 30.0)]
    fps: f64,

    /// 每推送N帧打印一次进度日志，避免日志刷屏
    #[arg(long, default_value_t = 60)]
    log_every: u64,
}

fn main() -> Result<(), DynError> {
    // 解析终端传入的命令行参数
    let mut args = Args::parse();
    // 修复macOS录屏文件名特殊空格问题，校验文件路径合法性
    args.input = resolve_input(&args.input)?;
    // 全局初始化FFmpeg解码器、编码器、缩放模块
    ffmpeg::init()?;

    // 创建共享内存发布对象，开辟共享内存区域用于写图像与位姿
    let mut publisher = ShmPublisher::create()?;
    // 连接共享内存订阅通道，监听自瞄进程发来的云台指令
    let mut subscriber = ShmSubscriber::connect()?;
    // 将默认相机内参写入共享内存，视觉算法进程可直接读取标定参数
    publisher.set_camera_info(default_camera_info());

    // 根据fps参数计算帧间隔时间，用于帧率限流
    let frame_interval = if args.fps > 0.0 {
        // 1/帧率 = 每一帧需要间隔的时间
        Some(Duration::from_secs_f64(1.0 / args.fps))
    } else {
        // fps<=0 不限流，解码多快推多快
        None
    };
    // 全局帧序号，全局单调递增，用于多进程帧时序对齐、帧同步
    let mut frame_seq = 0_u64;

    println!(
        "talos mock server started: input={}, loop={}, fps={}",
        args.input, args.r#loop, args.fps
    );

    // 外层循环：实现视频循环重播逻辑
    loop {
        // 解码完整一轮视频并推送所有帧，返回本轮推送帧数
        let published = publish_one_input(
            &args.input,
            &mut publisher,
            &mut subscriber,
            &mut frame_seq,
            frame_interval,
            args.log_every.max(1), // 最小打印间隔1帧，防止0报错
        )?;

        // 两种退出条件：
        // 1. 未开启循环播放；
        // 2. 本轮解码没有推送任何帧（视频损坏、空流、读取失败）；
        // 满足任意一种则终止循环
        if !args.r#loop || published == 0 {
            break;
        }
    }

    println!("talos mock server stopped, total published frames={frame_seq}");
    Ok(())
}

/// 解码单个视频源完整生命周期：打开视频→解包→解码→缩放→推送至共享内存
/// input：视频地址；published：本轮成功推送的帧数量
fn publish_one_input(
    input: &str,
    publisher: &mut ShmPublisher,
    subscriber: &mut ShmSubscriber,
    frame_seq: &mut u64,
    frame_interval: Option<Duration>,
    log_every: u64,
) -> Result<u64, DynError> {
    // 打开视频封装上下文（支持mp4/mov/h264/rtsp等所有FFmpeg支持的封装格式）
    let mut ictx = ffmpeg::format::input(input)?;
    // 自动选取视频流，跳过音频、字幕流
    let input_stream = ictx
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or_else(|| format!("no video stream found in input: {input}"))?;
    let video_stream_index = input_stream.index();

    // 用视频流的编码参数构造解码器上下文
    let context = ffmpeg::codec::context::Context::from_parameters(input_stream.parameters())?;
    let mut decoder = context.decoder().video()?;

    // 创建图像缩放器 SwsContext：
    // 原始帧格式/宽高 → RGB24格式 + 固定工程分辨率 IMAGE_WIDTH×IMAGE_HEIGHT
    // BILINEAR：双线性插值缩放，画质均衡、速度适中
    let mut scaler = ffmpeg::software::scaling::context::Context::get(
        decoder.format(),
        decoder.width(),
        decoder.height(),
        ffmpeg::format::Pixel::RGB24,
        IMAGE_WIDTH,
        IMAGE_HEIGHT,
        ffmpeg::software::scaling::flag::Flags::BILINEAR,
    )?;

    let mut decoded = ffmpeg::util::frame::Video::empty(); // 解码器输出原始帧缓存
    let mut rgb_frame = ffmpeg::util::frame::Video::empty();// 缩放后的RGB帧缓存
    let mut rgb_buf = vec![0_u8; IMAGE_SIZE]; // 紧凑连续RGB内存缓冲区（无对齐填充）
    let mut published = 0_u64; // 本轮循环已推送帧数
    let mut next_deadline = Instant::now(); // 帧率限流时间基准

    // 遍历视频所有数据包packet（音视频分离后的数据包）
    for (stream, packet) in ictx.packets() {
        // 只处理视频流数据包，过滤音频、字幕包
        if stream.index() != video_stream_index {
            continue;
        }

        // 将数据包送入解码器队列
        decoder.send_packet(&packet)?;
        // 循环取出解码器解码完成的画面帧
        while decoder.receive_frame(&mut decoded).is_ok() {
            // 原始帧缩放 + 像素格式转为RGB24
            scaler.run(&decoded, &mut rgb_frame)?;
            // 剔除FFmpeg行对齐填充字节，转为连续紧凑RGB数组存入rgb_buf
            copy_frame_rgb24(&rgb_frame, &mut rgb_buf)?;
            // 推送当前帧图像、模拟位姿，并尝试接收云台指令
            publish_frame(publisher, subscriber, &rgb_buf, *frame_seq);
            *frame_seq += 1;
            published += 1;

            // 每隔log_every帧打印一次进度
            if published % log_every == 0 {
                println!("published {published} frames in this pass, global seq={frame_seq}");
            }

            // 帧率限流逻辑，模拟真实相机固定输出帧率
            if let Some(interval) = frame_interval {
                next_deadline += interval;
                // 距离下一帧时刻还有时间，则睡眠等待
                if let Some(wait) = next_deadline.checked_duration_since(Instant::now()) {
                    thread::sleep(wait);
                } else {
                    // 解码耗时过长已经超时，重置时间基准，避免睡眠延迟不断累积
                    next_deadline = Instant::now();
                }
            }
        }
    }

    // 发送EOF结束标记，取出解码器缓冲区残留的最后几帧画面
    decoder.send_eof()?;
    while decoder.receive_frame(&mut decoded).is_ok() {
        scaler.run(&decoded, &mut rgb_frame)?;
        copy_frame_rgb24(&rgb_frame, &mut rgb_buf)?;
        publish_frame(publisher, subscriber, &rgb_buf, *frame_seq);
        *frame_seq += 1;
        published += 1;
    }

    Ok(published)
}

/// 完成单帧发布逻辑：写入图像、推送模拟位姿、更新心跳、监听云台下发指令
fn publish_frame(
    publisher: &mut ShmPublisher,
    subscriber: &mut ShmSubscriber,
    rgb_frame: &[u8],
    frame_seq: u64,
) {
    let timestamp_ns = now_ns();
    // 1. 将RGB图像写入共享内存，下游视觉进程零拷贝读取
    publisher.publish_image(rgb_frame, frame_seq, timestamp_ns);
    // 2. 推送机器人各个刚体的模拟位姿（里程计、云台、枪口、相机）
    publish_mock_poses(publisher, frame_seq, timestamp_ns);
    // 3. 更新进程心跳，视觉进程依靠心跳判断相机服务是否存活
    publisher.update_heartbeat();

    // 非阻塞读取自瞄进程下发的云台控制指令（无指令则直接返回None，不阻塞主线程）
    if let Some(cmd) = subscriber.recv_gimbal_cmd() {
        println!(
            "recv gimbal cmd: ts={} yaw={:.2} pitch={:.2} dist={:.3} fire={}",
            cmd.timestamp_ns, cmd.yaw_deg, cmd.pitch_deg, cmd.distance_m, cmd.fire_advice
        );
    }
}

/// 发布模拟位姿：全部使用单位四元数（无旋转），纯Mock调试用
/// PoseIndex：枚举区分不同刚体：Odom里程计/Gimbal云台/Muzzle枪口/Camera相机
fn publish_mock_poses(publisher: &mut ShmPublisher, frame_seq: u64, timestamp_ns: u64) {
    // 单位四元数 [w, x, y, z] = [1,0,0,0] 代表无旋转姿态
    let ident = [1.0, 0.0, 0.0, 0.0];
    // 里程计位姿：原点位置、无旋转
    publisher.publish_pose(
        PoseIndex::Odom,
        [0.0, 0.0, 0.0],
        ident,
        frame_seq,
        timestamp_ns,
    );
    // 云台位姿
    publisher.publish_pose(
        PoseIndex::Gimbal,
        [0.0, 0.0, 0.0],
        ident,
        frame_seq,
        timestamp_ns,
    );
    // 枪口位姿：Z轴抬高0.2m，模拟真实枪口高度
    publisher.publish_pose(
        PoseIndex::Muzzle,
        [0.0, 0.0, 0.2],
        ident,
        frame_seq,
        timestamp_ns,
    );
    // 相机位姿
    publisher.publish_pose(
        PoseIndex::Camera,
        [0.0, 0.0, 0.0],
        ident,
        frame_seq,
        timestamp_ns,
    );
}

/// 核心函数：剔除FFmpeg帧的行对齐填充字节，将带stride的帧转为紧凑连续RGB24数组
/// FFmpeg为满足CPU内存对齐加速，每行像素末尾会填充无用字节(stride > width*3)
/// 必须逐行拷贝裁剪，否则生成的RGB图像错乱花屏
fn copy_frame_rgb24(frame: &ffmpeg::util::frame::Video, dst: &mut [u8]) -> Result<(), DynError> {
    // 校验目标缓冲区尺寸严格等于 W*H*3，防止内存越界
    if dst.len() != IMAGE_SIZE {
        return Err(format!(
            "rgb dst size mismatch: expect {}, got {}",
            IMAGE_SIZE,
            dst.len()
        )
        .into());
    }

    let data = frame.data(0);        // RGB平面像素起始地址
    let stride = frame.stride(0);    // FFmpeg每行实际字节长度（含对齐填充）
    let row_bytes = IMAGE_WIDTH as usize * 3; // 一行有效像素字节数（不含填充）
    let height = IMAGE_HEIGHT as usize;

    // 逐行拷贝，丢弃每行末尾的对齐填充字节
    for y in 0..height {
        let src_start = y * stride;
        let src_end = src_start + row_bytes;
        let dst_start = y * row_bytes;
        let dst_end = dst_start + row_bytes;

        // 边界防护，防止越界读取帧内存
        if src_end > data.len() {
            return Err(format!(
                "frame buffer too small: src_end={}, data_len={}",
                src_end,
                data.len()
            )
            .into());
        }
        // 只拷贝有效像素部分，丢弃填充
        dst[dst_start..dst_end].copy_from_slice(&data[src_start..src_end]);
    }
    Ok(())
}

/// 构造默认相机内参CameraInfo，写入共享内存供视觉PnP解算读取
/// 使用理想针孔相机模型：无畸变、主点在画面中心
fn default_camera_info() -> CameraInfo {
    CameraInfo {
        timestamp_ns: now_ns(),
        fx: IMAGE_WIDTH as f64,    // 等效焦距简易赋值
        fy: IMAGE_HEIGHT as f64,
        cx: IMAGE_WIDTH as f64 / 2.0, // 主点x=画面中心
        cy: IMAGE_HEIGHT as f64 / 2.0,
        distortion: [0.0; 5], // 5阶径向畸变系数全部置0，无畸变仿真
        width: IMAGE_WIDTH,
        height: IMAGE_HEIGHT,
        _pad: [0; 24], // 内存对齐占位填充字节，保证结构体内存布局对齐
    }
}

/// 获取当前系统UTC时间戳(纳秒)，用于帧时序同步、时间戳标记
fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// 路径修复函数：专门兼容 macOS 截屏文件名的 U+202F 窄不间断空格Bug
/// macOS录屏文件名类似 `xxx 2:30 PM.mov`，PM/AM前不是普通空格，是特殊不间断空格，直接open会找不到文件
fn resolve_input(input: &str) -> Result<String, DynError> {
    // 如果是网络流媒体地址(rtsp:// http://)，直接放行无需校验本地文件
    if input.contains("://") {
        return Ok(input.to_string());
    }
    // 原始路径存在，直接返回
    if Path::new(input).exists() {
        return Ok(input.to_string());
    }

    // 方案1：解析转义后的unicode空格
    let decoded = input
        .replace("\\u{202f}", "\u{202f}")
        .replace("\\u202f", "\u{202f}");
    if Path::new(&decoded).exists() {
        return Ok(decoded);
    }

    // 方案2：自动把 " AM" / " PM" 前面普通空格替换为macOS专属窄不间断空格
    let nbsp_fixed = input
        .replace(" AM", "\u{202f}AM")
        .replace(" PM", "\u{202f}PM");
    if Path::new(&nbsp_fixed).exists() {
        return Ok(nbsp_fixed);
    }

    // 全部修复方案失败，抛出错误并给出macOS文件名解决方案提示
    Err(format!(
        "input not found: {input}\nHint: macOS screen recording filenames often contain U+202F before AM/PM. \
Use tab completion or wildcard like: .../5.44.33*PM.mov"
    )
    .into())
}