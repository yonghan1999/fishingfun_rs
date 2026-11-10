//! M7: 迁移自 WowScreen.cs。
//! 保真要点：截取主屏中央 1/4 区域——宽 W/2、高 H/2−100、原点 (W/4, H/4)。
//! 高度减 100 是原版有意行为，保留。
//! C# Graphics.CopyFromScreen 底层即 GDI BitBlt，此处直接调用等价 API。

use fishing_core::traits::{CapturedFrame, ScreenCapture};
use fishing_core::Point;
use image::RgbImage;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
    ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HGDIOBJ,
    SRCCOPY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, GetWindowRect, SM_CXSCREEN, SM_CYSCREEN,
};

/// 主屏尺寸（对应 Screen.PrimaryScreen.Bounds）。
fn primary_screen_size() -> (i32, i32) {
    unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) }
}

/// 对应 C# WowScreen 的静态截屏逻辑。
///
/// 注意 DPI：与 WinForms 版一致，进程需声明 system DPI aware，
/// 否则 GetSystemMetrics 返回缩放后的逻辑像素（见 docs/06-risks.md R2，
/// manifest 在 console/gui 打包时配置）。
#[derive(Debug, Default)]
pub struct WowScreenCapture;

impl WowScreenCapture {
    pub fn new() -> Self {
        WowScreenCapture
    }

    /// 截取主屏中央区域，返回 (宽 W/2, 高 H/2−100)，原点 (W/4, H/4)。
    fn capture_impl() -> anyhow::Result<CapturedFrame> {
        let (sw, sh) = primary_screen_size();
        let width = sw / 2;
        let height = sh / 2 - 100;
        let origin = Point::new(sw / 4, sh / 4);

        anyhow::ensure!(width > 0 && height > 0, "屏幕尺寸异常: {sw}x{sh}");

        unsafe {
            let screen_dc = GetDC(HWND::default());
            anyhow::ensure!(!screen_dc.is_invalid(), "GetDC 失败");

            let result = capture_region(screen_dc, origin.x, origin.y, width, height);
            ReleaseDC(HWND::default(), screen_dc);

            let pixels = result?;
            Ok(CapturedFrame {
                image: pixels,
                origin,
            })
        }
    }
}

/// BitBlt + GetDIBits：从 screen_dc 的 (x, y) 起取 width×height，转 RgbImage。
unsafe fn capture_region(
    screen_dc: windows::Win32::Graphics::Gdi::HDC,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> anyhow::Result<RgbImage> {
    unsafe {
        let mem_dc = CreateCompatibleDC(screen_dc);
        anyhow::ensure!(!mem_dc.is_invalid(), "CreateCompatibleDC 失败");

        let bmp = CreateCompatibleBitmap(screen_dc, width, height);
        anyhow::ensure!(!bmp.is_invalid(), "CreateCompatibleBitmap 失败");

        let old = SelectObject(mem_dc, HGDIOBJ(bmp.0));
        BitBlt(mem_dc, 0, 0, width, height, screen_dc, x, y, SRCCOPY)?;

        // 32bpp 自顶向下 DIB
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height, // 负值 = top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };

        let mut buf = vec![0u8; (width * height * 4) as usize];
        GetDIBits(
            mem_dc,
            bmp,
            0,
            height as u32,
            Some(buf.as_mut_ptr() as *mut _),
            &mut bmi,
            DIB_RGB_COLORS,
        );

        SelectObject(mem_dc, old);
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(mem_dc);

        // BGRA → RGB
        let mut img = RgbImage::new(width as u32, height as u32);
        let (pixels, _) = buf.as_chunks::<4>();
        for (dst, src) in img.pixels_mut().zip(pixels) {
            *dst = image::Rgb([src[2], src[1], src[0]]);
        }
        Ok(img)
    }
}

impl ScreenCapture for WowScreenCapture {
    fn capture(&mut self) -> CapturedFrame {
        match Self::capture_impl() {
            Ok(frame) => frame,
            Err(e) => {
                // C# 版截屏异常会在 bot 主循环被 catch；Rust 版此处容错返回 1x1 空帧
                // （等价于 C# 中"找不到浮漂"路径），避免中断主循环。
                tracing::error!("截屏失败: {e:#}");
                CapturedFrame {
                    image: RgbImage::new(1, 1),
                    origin: Point::EMPTY,
                }
            }
        }
    }
}

/// 截取任意窗口客户区位图（M11 GUI 颜色配置窗口的 Capture Screen 备用；
/// 当前版本保持与原版一致只截主屏中央区域）。
#[allow(dead_code)]
pub(crate) fn window_rect(hwnd: HWND) -> Option<RECT> {
    unsafe {
        let mut rc = RECT::default();
        GetWindowRect(hwnd, &mut rc).ok()?;
        Some(rc)
    }
}

#[cfg(test)]
mod tests {
    /// 冒烟测试需真实桌面环境，默认 ignore（docs/05-testing-strategy.md 第 3 层）。
    /// 运行: cargo test -p fishing-platform-windows -- --ignored
    #[test]
    #[ignore = "需要真实 Windows 桌面会话"]
    fn captures_primary_screen_centre() {
        use super::*;
        use fishing_core::traits::ScreenCapture;

        let (sw, sh) = super::primary_screen_size();
        let mut cap = WowScreenCapture::new();
        let frame = cap.capture();
        assert_eq!(frame.image.width(), (sw / 2) as u32);
        assert_eq!(frame.image.height(), (sh / 2 - 100) as u32);
        assert_eq!(frame.origin, Point::new(sw / 4, sh / 4));
        // 像素不应全黑（桌面极少纯黑）
        let sum: u64 = frame
            .image
            .pixels()
            .map(|p| p[0] as u64 + p[1] as u64 + p[2] as u64)
            .sum();
        assert!(sum > 0);
    }
}
