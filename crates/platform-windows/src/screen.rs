//! M7: 迁移自 WowScreen.cs。
//! 保真要点：截取主屏中央 1/4 区域——宽 W/2、高 H/2−100、原点 (W/4, H/4)。
//! 高度减 100 是原版有意行为，保留。
//! C# Graphics.CopyFromScreen 底层即 GDI BitBlt，此处直接调用等价 API。

use crate::wow_process::WowTarget;
use fishing_core::traits::{CapturedFrame, ScreenCapture};
use fishing_core::{Point, Rect};
use image::RgbImage;
use windows::Win32::Foundation::POINT;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
    ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC,
    HGDIOBJ, SRCCOPY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetSystemMetrics, GetWindowRect, IsIconic, SM_CXSCREEN, SM_CXVIRTUALSCREEN,
    SM_CYSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

/// 客户区几何始终使用物理屏幕坐标，允许副屏的负原点。
pub fn client_bounds(target: WowTarget) -> anyhow::Result<Rect> {
    target.validate()?;
    unsafe {
        anyhow::ensure!(!IsIconic(target.hwnd()).as_bool(), "游戏窗口已最小化");
        let mut rc = RECT::default();
        GetClientRect(target.hwnd(), &mut rc)?;
        let mut origin = POINT::default();
        anyhow::ensure!(
            ClientToScreen(target.hwnd(), &mut origin).as_bool(),
            "无法取得客户区坐标"
        );
        let bounds = Rect::new(origin.x, origin.y, rc.right, rc.bottom);
        anyhow::ensure!(bounds.width > 1 && bounds.height > 1, "游戏客户区为空");
        let desktop = Rect::new(
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        );
        anyhow::ensure!(
            contained(bounds, desktop),
            "游戏客户区为空或超出桌面，请完整显示游戏窗口"
        );
        Ok(bounds)
    }
}

fn contained(rect: Rect, limit: Rect) -> bool {
    rect.width > 0
        && rect.height > 0
        && rect.x >= limit.x
        && rect.y >= limit.y
        && i64::from(rect.x) + i64::from(rect.width) <= i64::from(limit.x) + i64::from(limit.width)
        && i64::from(rect.y) + i64::from(rect.height)
            <= i64::from(limit.y) + i64::from(limit.height)
}

/// 绑定会话目标；不允许截图期间窗口变化后继续使用旧坐标。
pub fn capture_client(
    target: WowTarget,
    expected: Option<Rect>,
    region: Option<Rect>,
) -> anyhow::Result<CapturedFrame> {
    let bounds = client_bounds(target)?;
    anyhow::ensure!(
        expected.is_none_or(|old| old == bounds),
        "游戏窗口位置或大小已改变，请重新开始"
    );
    let local = region.unwrap_or(Rect::new(0, 0, bounds.width, bounds.height));
    anyhow::ensure!(
        contained(local, Rect::new(0, 0, bounds.width, bounds.height)),
        "搜索区域超出游戏客户区，请重新框选"
    );
    let origin = Point::new(bounds.x + local.x, bounds.y + local.y);
    let image = unsafe {
        let dc = ScreenDc(GetDC(HWND::default()));
        anyhow::ensure!(!dc.0.is_invalid(), "GetDC 失败");
        capture_region(dc.0, origin.x, origin.y, local.width, local.height)?
    };
    anyhow::ensure!(
        client_bounds(target)? == bounds,
        "截图期间游戏窗口发生变化，请重新开始"
    );
    Ok(CapturedFrame { image, origin })
}

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
#[derive(Default)]
pub struct WowScreenCapture {
    target: Option<(
        WowTarget,
        Rect,
        fishing_core::cancellation::CancellationToken,
    )>,
}

impl WowScreenCapture {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn for_target(
        target: WowTarget,
        bounds: Rect,
        cancel: fishing_core::cancellation::CancellationToken,
    ) -> Self {
        Self {
            target: Some((target, bounds, cancel)),
        }
    }

    fn take(&self, region: Option<Rect>) -> CapturedFrame {
        let result = match &self.target {
            Some((target, bounds, cancel)) => {
                let result = capture_client(*target, Some(*bounds), region);
                if result.is_err() {
                    cancel.cancel();
                }
                result
            }
            None => match region {
                Some(region) => Self::try_capture_region(region),
                None => Self::try_capture(),
            },
        };
        Self::capture_or_fallback(result)
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
                        // P01 = 截屏失败（限频 5 秒）
                        tracing::error!("P01 {e:#}");
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
        self.take(None)
    }

    fn capture_region(&mut self, region: Rect) -> CapturedFrame {
        self.take(Some(region))
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
    fn client_region_accepts_negative_monitor_origin_and_rejects_overflow() {
        use super::*;
        let desktop = Rect::new(-1920, 0, 3840, 1080);
        assert!(contained(Rect::new(-1900, 10, 1800, 1000), desktop));
        assert!(!contained(Rect::new(-1921, 10, 1800, 1000), desktop));
        assert!(!contained(Rect::new(1900, 0, 100, 100), desktop));
        assert!(!contained(Rect::new(0, 0, 0, 100), desktop));
        assert!(!contained(
            Rect::new(i32::MAX - 1, 0, 100, 100),
            Rect::new(0, 0, i32::MAX, 1000)
        ));
    }

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
