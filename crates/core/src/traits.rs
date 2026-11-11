//! M6: 平台与组件抽象。方向与 C# 接口相反：core 定义能力 trait，
//! platform-windows 负责实现；core 内部组件也实现相应 trait 以便 mock 测试。

use crate::{Point, Rect};
use image::RgbImage;

/// 一帧截屏：位图 + 其在屏幕坐标系中的原点偏移。
/// 对应 C# WowScreen.GetBitmap() + GetScreenPositionFromBitmapPostion() 的语义。
pub struct CapturedFrame {
    pub image: RgbImage,
    /// 位图左上角在屏幕坐标系中的位置（C# 版为主屏 W/4, H/4）。
    pub origin: Point,
}

/// 屏幕捕获能力（对应 IImageProvider 的数据来源 WowScreen）。
pub trait ScreenCapture {
    fn capture(&mut self) -> CapturedFrame;

    /// 只截取全帧位图中的一个子区域（位图坐标，调用方需保证在帧范围内）。
    /// 返回帧的 origin 为全帧原点 + 区域偏移。
    /// 默认实现：整帧截取后裁剪——行为正确但无性能收益；平台实现应直接
    /// 截取该区域以减少读屏量和像素拷贝（锁定跟踪后每帧只需小窗）。
    fn capture_region(&mut self, region: Rect) -> CapturedFrame {
        let frame = self.capture();
        let (w, h) = (frame.image.width() as i32, frame.image.height() as i32);
        let x = region.x.clamp(0, w);
        let y = region.y.clamp(0, h);
        let cw = region.width.min(w - x).max(1) as u32;
        let ch = region.height.min(h - y).max(1) as u32;
        let image = image::imageops::crop_imm(&frame.image, x as u32, y as u32, cw, ch).to_image();
        CapturedFrame {
            image,
            origin: Point::new(frame.origin.x + x, frame.origin.y + y),
        }
    }
}

/// 浮漂查找器（对应 IBobberFinder）。
pub trait BobberFinder {
    /// 返回浮漂的屏幕坐标；找不到返回 None（对应 C# Point.Empty 哨兵）。
    fn find(&mut self) -> Option<Point>;
    fn reset(&mut self);
}

/// 咬钩观察者（对应 IBiteWatcher）。
pub trait BiteWatcher {
    fn reset(&mut self, initial_bobber_position: Point);
    fn is_bite(&mut self, current_bobber_position: Point) -> bool;
    /// 对应 C# FishingEventHandler setter（FishBot.Start 中接线）。
    fn set_event_handler(&mut self, tx: std::sync::mpsc::Sender<crate::events::FishingEvent>);
}

/// 按键发送能力（对应 WowProcess.PressKey/KeyDown/KeyUp）。
pub trait KeySender {
    fn press_key(&mut self, key: Key);
    fn key_down(&mut self, key: Key);
    fn key_up(&mut self, key: Key);
}

/// 鼠标能力（对应 WowProcess.RightClickMouse 等）。
pub trait MouseController {
    fn right_click_at(&mut self, position: Point);

    /// 平台实现可进一步在拾取前等待期间响应取消。
    fn right_click_cancellable(
        &mut self,
        position: Point,
        cancel: &crate::cancellation::CancellationToken,
    ) {
        if !cancel.is_cancelled() {
            self.right_click_at(position);
        }
    }
}

/// 可投递的按键。对应 C# ConsoleKey：其枚举数值与 Win32 VK 码一致，
/// C# 版直接拿枚举整数值当 PostMessage 的 wParam，keybind.txt 也存该整数。
/// Rust 版以 VK 码为唯一真相（newtype），名字表覆盖 C# ConsoleKey 全部成员，
/// 与 C# `Enum.TryParse<ConsoleKey>` / `(ConsoleKey)int.Parse` 行为对齐。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key(u32);

/// C# ConsoleKey 全成员表（枚举名 ↔ 数值/VK 码）。
const CONSOLE_KEY_NAMES: &[(&str, u32)] = &[
    ("Backspace", 8),
    ("Tab", 9),
    ("Clear", 12),
    ("Enter", 13),
    ("Pause", 19),
    ("Escape", 27),
    ("Spacebar", 32),
    ("PageUp", 33),
    ("PageDown", 34),
    ("End", 35),
    ("Home", 36),
    ("LeftArrow", 37),
    ("UpArrow", 38),
    ("RightArrow", 39),
    ("DownArrow", 40),
    ("Select", 41),
    ("Print", 42),
    ("Execute", 43),
    ("PrintScreen", 44),
    ("Insert", 45),
    ("Delete", 46),
    ("Help", 47),
    ("D0", 48),
    ("D1", 49),
    ("D2", 50),
    ("D3", 51),
    ("D4", 52),
    ("D5", 53),
    ("D6", 54),
    ("D7", 55),
    ("D8", 56),
    ("D9", 57),
    ("A", 65),
    ("B", 66),
    ("C", 67),
    ("D", 68),
    ("E", 69),
    ("F", 70),
    ("G", 71),
    ("H", 72),
    ("I", 73),
    ("J", 74),
    ("K", 75),
    ("L", 76),
    ("M", 77),
    ("N", 78),
    ("O", 79),
    ("P", 80),
    ("Q", 81),
    ("R", 82),
    ("S", 83),
    ("T", 84),
    ("U", 85),
    ("V", 86),
    ("W", 87),
    ("X", 88),
    ("Y", 89),
    ("Z", 90),
    ("LeftWindows", 91),
    ("RightWindows", 92),
    ("Applications", 93),
    ("Sleep", 95),
    ("NumPad0", 96),
    ("NumPad1", 97),
    ("NumPad2", 98),
    ("NumPad3", 99),
    ("NumPad4", 100),
    ("NumPad5", 101),
    ("NumPad6", 102),
    ("NumPad7", 103),
    ("NumPad8", 104),
    ("NumPad9", 105),
    ("Multiply", 106),
    ("Add", 107),
    ("Separator", 108),
    ("Subtract", 109),
    ("Decimal", 110),
    ("Divide", 111),
    ("F1", 112),
    ("F2", 113),
    ("F3", 114),
    ("F4", 115),
    ("F5", 116),
    ("F6", 117),
    ("F7", 118),
    ("F8", 119),
    ("F9", 120),
    ("F10", 121),
    ("F11", 122),
    ("F12", 123),
    ("F13", 124),
    ("F14", 125),
    ("F15", 126),
    ("F16", 127),
    ("F17", 128),
    ("F18", 129),
    ("F19", 130),
    ("F20", 131),
    ("F21", 132),
    ("F22", 133),
    ("F23", 134),
    ("F24", 135),
    ("NumLock", 144),
    ("Scroll", 145),
    ("LeftShift", 160),
    ("RightShift", 161),
    ("LeftControl", 162),
    ("RightControl", 163),
    ("LeftAlt", 164),
    ("RightAlt", 165),
    ("BrowserBack", 166),
    ("BrowserForward", 167),
    ("BrowserRefresh", 168),
    ("BrowserStop", 169),
    ("BrowserSearch", 170),
    ("BrowserFavorites", 171),
    ("BrowserHome", 172),
    ("VolumeMute", 173),
    ("VolumeDown", 174),
    ("VolumeUp", 175),
    ("MediaNext", 176),
    ("MediaPrevious", 177),
    ("MediaStop", 178),
    ("MediaPlay", 179),
    ("LaunchMail", 180),
    ("LaunchMediaSelect", 181),
    ("LaunchApp1", 182),
    ("LaunchApp2", 183),
    ("Oem1", 186),
    ("OemPlus", 187),
    ("OemComma", 188),
    ("OemMinus", 189),
    ("OemPeriod", 190),
    ("Oem2", 191),
    ("Oem3", 192),
    ("Oem4", 219),
    ("Oem5", 220),
    ("Oem6", 221),
    ("Oem7", 222),
    ("Oem8", 223),
    ("Oem102", 226),
    ("Process", 229),
    ("Packet", 231),
    ("Attention", 246),
    ("CrSel", 247),
    ("ExSel", 248),
    ("EraseEndOfFile", 249),
    ("Play", 250),
    ("Zoom", 251),
    ("NoName", 252),
    ("Pa1", 253),
    ("OemClear", 254),
];

impl Key {
    // 代码中直接引用的常用键（常量名沿用原枚举变体名）。
    pub const ESCAPE: Key = Key(27);
    pub const SPACEBAR: Key = Key(32);
    pub const D0: Key = Key(48);
    pub const D1: Key = Key(49);
    pub const D2: Key = Key(50);
    pub const D3: Key = Key(51);
    pub const D4: Key = Key(52);
    pub const D5: Key = Key(53);
    pub const D6: Key = Key(54);
    pub const D7: Key = Key(55);
    pub const D8: Key = Key(56);
    pub const D9: Key = Key(57);
    pub const A: Key = Key(65);

    /// Win32 virtual-key code（与 ConsoleKey 数值一致）。
    pub fn vk_code(self) -> u32 {
        self.0
    }

    /// 从 C# ConsoleKey 枚举整数值还原（keybind.txt 持久化格式互通）。
    /// C# `(ConsoleKey)int.Parse(...)` 接受任意整数；VK 码有效范围 0..=0xFF。
    pub fn from_console_key_value(v: i32) -> Option<Key> {
        (0..=0xFF).contains(&v).then_some(Key(v as u32))
    }

    /// 对应 C# Enum.TryParse<ConsoleKey>（按枚举名解析，大小写敏感）。
    pub fn from_console_key_name(name: &str) -> Option<Key> {
        CONSOLE_KEY_NAMES
            .iter()
            .find(|(n, _)| *n == name)
            .map(|&(_, v)| Key(v))
    }

    /// C# ConsoleKey.ToString()：已知值返回枚举名，未知值无名字。
    pub fn console_key_name(self) -> Option<&'static str> {
        CONSOLE_KEY_NAMES
            .iter()
            .find(|(_, v)| *v == self.0)
            .map(|&(n, _)| n)
    }

    /// 对应 C# KeyBindChooser.GetCastKeyText 的显示规则:
    /// 单字符原样；"Dx" 显示数字；其他显示 "?"。
    pub fn display_text(self) -> String {
        let name = self.console_key_name().unwrap_or("");
        if name.len() == 1 {
            return name.to_string();
        }
        if name.len() == 2 && name.starts_with('D') {
            return name[1..].to_string();
        }
        "?".to_string()
    }
}
