//! FishingFun core: 平台无关的钓鱼机器人核心逻辑。
//! 迁移自 FishingFun.Bot (C#)，行为保真优先。

pub mod bite_watcher;
pub mod bobber_finder;
pub mod classifier;
pub mod events;
pub mod fish_bot;
pub mod timed_action;
pub mod tracking_finder;
pub mod traits;

/// 屏幕/位图坐标点（对应 System.Drawing.Point）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub const EMPTY: Point = Point { x: 0, y: 0 };

    pub fn new(x: i32, y: i32) -> Self {
        Point { x, y }
    }

    pub fn is_empty(&self) -> bool {
        *self == Point::EMPTY
    }
}

impl std::fmt::Display for Point {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 与 C# Point.ToString() 格式对齐: "{X=1, Y=2}"
        write!(f, "{{X={}, Y={}}}", self.x, self.y)
    }
}

/// 位图坐标矩形（扩展功能：F3 水面搜索区域，docs/08-tracking-finder.md）。
/// 坐标系与 ScreenCapture 输出位图一致（原点在截屏左上角）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    pub fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x < self.x + self.width && p.y >= self.y && p.y < self.y + self.height
    }
}
