//! 方案 A：Windows Graphics Capture（WGC）捕获层。
//!
//! 采用「即用即建」模式：每次截图请求时，针对前台窗口新建一个 WGC 会话，
//! 抓取第一帧 FP16（Rgba16F）原始数据后立即停止会话。冷启动延迟约 100–300ms，
//! 对截图场景无感，且避免了「常驻会话 + 窗口切换重建」的长生命周期复杂度。
//!
//! 关键点：`ColorFormat::Rgba16F` 拿到的线性 HDR 数据，`save_as_image` 会直接拒绝，
//! 因此这里只取原始字节，交给 `tonemap` 模块处理。

use windows_capture::capture::{Context, GraphicsCaptureApiHandler};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::InternalCaptureControl;
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
    MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};
use windows_capture::window::Window;

/// 捕获到的一帧 HDR 原始数据
#[derive(Debug, Clone)]
pub struct CapturedFrame {
    pub width: u32,
    pub height: u32,
    /// Rgba16F 无 padding 的原始字节（每像素 8 字节：R,G,B,A 各 2 字节 f16）
    pub rgba16f: Vec<u8>,
}

/// 单帧捕获 handler：在第一次帧回调时拷贝数据并停止会话
struct SingleFrameCapture {
    frame: Option<CapturedFrame>,
    error: Option<String>,
}

impl GraphicsCaptureApiHandler for SingleFrameCapture {
    type Flags = ();
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn new(_ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self {
            frame: None,
            error: None,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame<'_>,
        capture_control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        if self.frame.is_some() {
            return Ok(());
        }

        match frame.buffer() {
            Ok(fb) => {
                let width = fb.width();
                let height = fb.height();
                // 剥离行 padding，得到紧凑的 Rgba16F 字节流。
                // 【注意】必须用返回值：windows-capture 2.0.1 的实现里，
                // 帧无行 padding 时直接返回帧内部缓冲、不写入传入的 buf
                // （写入分支只在 has_padding() 为真时走）。直接用 buf 会让
                // 无 padding 帧（如 Mafia 2560x1440）抓到 0 字节。
                let mut buf = Vec::new();
                let no_pad = fb.as_nopadding_buffer(&mut buf);
                self.frame = Some(CapturedFrame {
                    width,
                    height,
                    rgba16f: no_pad.to_vec(),
                });
            }
            Err(e) => {
                self.error = Some(e.to_string());
            }
        }
        // 拿到一帧即停，不再等后续帧
        capture_control.stop();
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// 从指定 HWND 抓取一帧 FP16 HDR 原始数据（即用即建，阻塞直到拿到帧或超时）
pub fn capture_window_fp16(hwnd: isize) -> anyhow::Result<CapturedFrame> {
    let window = Window::from_raw_hwnd(hwnd as *mut std::ffi::c_void);

    let settings = Settings::new(
        window,
        CursorCaptureSettings::WithoutCursor,
        DrawBorderSettings::WithoutBorder,
        SecondaryWindowSettings::Default,
        // 截图场景不设节流，确保拿到最新帧
        MinimumUpdateIntervalSettings::Custom(std::time::Duration::ZERO),
        DirtyRegionSettings::Default,
        ColorFormat::Rgba16F,
        (),
    );

    // start_free_threaded 不接管当前线程，返回的 CaptureControl 可通过 callback() 访问 handler
    let control = SingleFrameCapture::start_free_threaded(settings)
        .map_err(|e| anyhow::anyhow!("启动 WGC 捕获失败: {e}"))?;

    // 轮询等待帧（带 8 秒超时：覆盖 WGC 冷启动预热与系统繁忙场景）
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    let frame = loop {
        {
            let cb = control.callback();
            let guard = cb.lock();
            if let Some(f) = &guard.frame {
                break f.clone();
            }
            if let Some(err) = &guard.error {
                anyhow::bail!("捕获帧失败: {err}");
            }
        }
        if std::time::Instant::now() > deadline {
            let _ = control.stop();
            anyhow::bail!("捕获帧超时（可能是窗口已关闭或 DRM 受保护内容）");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };

    // 帧已拿到，优雅停止捕获会话
    let _ = control.stop();
    Ok(frame)
}
