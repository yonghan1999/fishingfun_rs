//! M7: 迁移自 WowScreen.cs。
//! 保真要点：截取主屏中央 1/4 区域——宽 W/2、高 H/2−100、原点 (W/4, H/4)。
//! 高度减 100 是原版有意行为，保留。
//! C# Graphics.CopyFromScreen 底层即 GDI BitBlt，此处直接调用等价 API。

use fishing_core::traits::{CapturedFrame, ScreenCapture};
use fishing_core::{Point, Rect};
use image::RgbImage;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
    ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC,
    HGDIOBJ, SRCCOPY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, GetWindowRect, SM_CXSCREEN, SM_CYSCREEN,
};

// 作用域守卫覆盖所有提前返回路径。
struct ScreenDc(HDC);
impl Drop for ScreenDc {
    fn drop(&mut self) {
        unsafe {
            ReleaseDC(HWND::default(), self.0);
        }
    }
}
struct MemoryDc(HDC);
impl Drop for MemoryDc {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteDC(self.0);
        }
    }
}
struct Bitmap(HBITMAP);
impl Drop for Bitmap {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(self.0 .0));
        }
    }
}
struct SelectedBitmap {
    dc: HDC,
    old: HGDIOBJ,
}
impl Drop for SelectedBitmap {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
        }
    }
}

/// 主屏尺寸（对应 Screen.PrimaryScreen.Bounds）。
fn primary_screen_size() -> (i32, i32) {
    unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) }
}

/// 对应 C# WowScreen 的静态截屏逻辑。
///
/// 注意 DPI：与 WinForms 版一致，进程需声明 system DPI aware，
/// 否则 GetSystemMetrics 返回缩放后的逻辑像素（见仓库 docs/development.md，
/// manifest 在 console/gui 打包时配置）。
#[derive(Debug, Default)]
pub struct WowScreenCapture;

impl WowScreenCapture {
    pub fn new() -> Self {
        WowScreenCapture
    }

    /// 截取主屏中央区域，返回 (宽 W/2, 高 H/2−100)，原点 (W/4, H/4)。
    pub fn try_capture() -> anyhow::Result<CapturedFrame> {
        let (sw, sh) = primary_screen_size();
        let width = sw / 2;
        let height = sh / 2 - 100;
        let origin = Point::new(sw / 4, sh / 4);

        anyhow::ensure!(width > 0 && height > 0, "屏幕尺寸异常: {sw}x{sh}");

        unsafe {
            let screen_dc = ScreenDc(GetDC(HWND::default()));
            anyhow::ensure!(!screen_dc.0.is_invalid(), "GetDC 失败");

            let pixels = capture_region(screen_dc.0, origin.x, origin.y, width, height)?;
            Ok(CapturedFrame {
                image: pixels,
                origin,
            })
        }
    }

    /// 只截取全帧位图中的一个子区域（位图坐标，见 ScreenCapture::capture_region）。
    /// 锁定跟踪后每帧只需 ±track_radius 小窗，读屏量和像素处理下降两个数量级。
    pub fn try_capture_region(region: Rect) -> anyhow::Result<CapturedFrame> {
        let (sw, sh) = primary_screen_size();
        let base = Point::new(sw / 4, sh / 4);
        let (max_w, max_h) = (sw / 2, sh / 2 - 100);
        anyhow::ensure!(max_w > 0 && max_h > 0, "屏幕尺寸异常: {sw}x{sh}");

        // 裁剪到全帧范围（位图坐标）
        let x0 = region.x.clamp(0, max_w);
        let y0 = region.y.clamp(0, max_h);
        let x1 = (region.x as i64 + region.width as i64).clamp(0, max_w as i64) as i32;
        let y1 = (region.y as i64 + region.height as i64).clamp(0, max_h as i64) as i32;
        anyhow::ensure!(x1 > x0 && y1 > y0, "子区域为空: {region:?}");

        unsafe {
            let screen_dc = ScreenDc(GetDC(HWND::default()));
            anyhow::ensure!(!screen_dc.0.is_invalid(), "GetDC 失败");

            let pixels = capture_region(screen_dc.0, base.x + x0, base.y + y0, x1 - x0, y1 - y0)?;
            Ok(CapturedFrame {
                image: pixels,
                origin: Point::new(base.x + x0, base.y + y0),
            })
        }
    }

    /// 失败时容错返回 1x1 空帧（等价于 C# 中"找不到浮漂"路径），避免中断主循环。
    /// 错误日志限频 5s，避免刷屏。
    fn capture_or_fallback(result: anyhow::Result<CapturedFrame>) -> CapturedFrame {
        match result {
            Ok(frame) => frame,
            Err(e) => {
                static LAST_ERROR: std::sync::Mutex<Option<std::time::Instant>> =
                    std::sync::Mutex::new(None);
                if let Ok(mut last) = LAST_ERROR.lock() {
                    if last.is_none_or(|time| time.elapsed() >= std::time::Duration::from_secs(5)) {
                        tracing::error!("截屏失败: {e:#}");
                        *last = Some(std::time::Instant::now());
                    }
                }
                CapturedFrame {
                    image: RgbImage::new(1, 1),
                    origin: Point::EMPTY,
                }
            }
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
        let mem_dc = MemoryDc(CreateCompatibleDC(screen_dc));
        anyhow::ensure!(!mem_dc.0.is_invalid(), "CreateCompatibleDC 失败");

        let bmp = Bitmap(CreateCompatibleBitmap(screen_dc, width, height));
        anyhow::ensure!(!bmp.0.is_invalid(), "CreateCompatibleBitmap 失败");

        let old = SelectObject(mem_dc.0, HGDIOBJ(bmp.0 .0));
        anyhow::ensure!(!old.is_invalid(), "SelectObject 失败");
        let selected = SelectedBitmap { dc: mem_dc.0, old };
        BitBlt(mem_dc.0, 0, 0, width, height, screen_dc, x, y, SRCCOPY)?;
        // GetDIBits 要求位图不再选入 DC。
        drop(selected);

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
        let copied = GetDIBits(
            mem_dc.0,
            bmp.0,
            0,
            height as u32,
            Some(buf.as_mut_ptr() as *mut _),
            &mut bmi,
            DIB_RGB_COLORS,
        );

        anyhow::ensure!(copied == height, "GetDIBits 读取不完整: {copied}/{height}");

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
        Self::capture_or_fallback(Self::try_capture())
    }

    fn capture_region(&mut self, region: Rect) -> CapturedFrame {
        Self::capture_or_fallback(Self::try_capture_region(region))
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
    #[test]
    #[ignore = "需要真实 Windows 桌面会话；覆盖成功和 BitBlt 错误路径的 GDI 回收"]
    fn capture_releases_gdi_on_success_and_failure() {
        use super::*;
        use windows::Win32::System::Threading::{
            GetCurrentProcess, GetGuiResources, GR_GDIOBJECTS,
        };
        unsafe {
            let dc = ScreenDc(GetDC(HWND::default()));
            assert!(!dc.0.is_invalid());
            capture_region(dc.0, 0, 0, 10, 10).unwrap();
            let frame = WowScreenCapture::try_capture().unwrap();
            assert!(frame.image.width() > 1 && frame.image.height() > 1);
            let before = GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS);
            for _ in 0..50 {
                capture_region(dc.0, 0, 0, 10, 10).unwrap();
                assert!(capture_region(HDC::default(), 0, 0, 10, 10).is_err());
            }
            assert_eq!(GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS), before);
        }
    }
    /// 冒烟测试需真实桌面环境，默认 ignore（见仓库 docs/development.md）。
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
