//! M4: 迁移自 SearchBobberFinder.cs。
//! 保真要点：
//! - 有上次命中点时只在 ±40 像素窗口内搜索，未命中则全图重搜；
//! - 计分算法 O(n²)：每个匹配点统计其 ±10 像素（严格小于）邻域内的匹配点数，
//!   最高分获胜，同分取扫描序第一个（对应 LINQ OrderByDescending 稳定序 + FirstOrDefault）；
//! - 单帧匹配点 >1000 视为配置错误，清空并报错。
//!
//! 与 C# 的结构差异：C# 直接把匹配像素染红画回位图，Rust 版输出独立 mask，
//! 由 UI 叠加显示（见仓库 docs/architecture.md）。

use std::sync::mpsc::Sender;
use std::sync::{Arc, RwLock};

use image::RgbImage;

use crate::classifier::{ClassifierMode, PixelClassifier};
use crate::traits::{BobberFinder, CapturedFrame, ScreenCapture};
use crate::Point;

/// 发送给 UI 的一帧：截屏 + 匹配掩码 + 浮漂位图坐标（对应 C# BobberBitmapEvent）。
pub struct FrameEvent {
    pub frame: CapturedFrame,
    /// 与位图同尺寸，true = 该像素被分类器命中（C# 版把这些像素染成纯红/蓝）。
    pub mask: Vec<bool>,
    /// 浮漂位图坐标；None 对应 C# Point.Empty。
    pub bobber: Option<Point>,
}

/// 分类器来源：独占持有（console，快照即真源）或共享引用（GUI，对应 C# 中
/// MainWindow 与 ColourConfiguration 共享同一 IPixelClassifier 实例的行为）。
#[derive(Debug, Clone)]
enum ClassifierSource {
    Owned,
    Shared(Arc<RwLock<PixelClassifier>>),
}

/// 对应 C# SearchBobberFinder。
pub struct SearchBobberFinder<C: ScreenCapture> {
    source: ClassifierSource,
    /// 每帧 find() 时从 source 刷新的本地快照（避免逐像素加锁）
    classifier: PixelClassifier,
    capture: C,
    previous_location: Option<Point>,
    /// 对应 IImageProvider.BitmapEvent
    frame_handler: Option<Sender<FrameEvent>>,
}

impl<C: ScreenCapture> SearchBobberFinder<C> {
    pub fn new(classifier: PixelClassifier, capture: C) -> Self {
        SearchBobberFinder {
            classifier,
            source: ClassifierSource::Owned,
            capture,
            previous_location: None,
            frame_handler: None,
        }
    }

    /// GUI 用：共享分类器实例，颜色配置窗口的修改对运行中的 bot 实时生效
    /// （对应 C# 共享 IPixelClassifier 引用）。
    pub fn new_shared(classifier: Arc<RwLock<PixelClassifier>>, capture: C) -> Self {
        let snapshot = classifier.read().map(|c| c.clone()).unwrap_or_default();
        SearchBobberFinder {
            source: ClassifierSource::Shared(classifier),
            classifier: snapshot,
            capture,
            previous_location: None,
            frame_handler: None,
        }
    }

    pub fn set_frame_handler(&mut self, handler: Sender<FrameEvent>) {
        self.frame_handler = Some(handler);
    }

    /// 当前帧使用的分类器快照。
    pub fn classifier(&self) -> &PixelClassifier {
        &self.classifier
    }

    /// 每帧开始时从共享源刷新快照（Owned 时为空操作）。
    fn refresh_classifier(&mut self) {
        if let ClassifierSource::Shared(shared) = &self.source {
            if let Ok(latest) = shared.read() {
                self.classifier = latest.clone();
            }
        }
    }

    /// 对应私有方法 FindRedPoints：返回匹配点及掩码。
    fn find_red_points(&self, image: &RgbImage) -> (Vec<Point>, Vec<bool>) {
        let width = image.width() as i32;
        let height = image.height() as i32;

        // 有上次命中点时只搜索其 ±40 像素窗口
        let (min_x, max_x, min_y, max_y) = match self.previous_location {
            Some(prev) => (
                (prev.x - 40).max(0),
                (prev.x + 40).min(width),
                (prev.y - 40).max(0),
                (prev.y + 40).min(height),
            ),
            None => (0, width, 0, height),
        };

        let mut points = Vec::new();
        let mut mask = vec![false; (width * height) as usize];

        for x in min_x..max_x {
            for y in min_y..max_y {
                let p = image.get_pixel(x as u32, y as u32);
                if self.classifier.is_match(p[0], p[1], p[2]) {
                    points.push(Point::new(x, y));
                    mask[(y * width + x) as usize] = true;
                }
            }
        }

        if points.len() > 1000 {
            // E01 = 命中像素过多，需调整颜色分类参数
            tracing::error!("E01");
            points.clear();
            mask.fill(false);
        }

        (points, mask)
    }
}

/// 对应 C# 私有类 Score。pub(crate)：TrackingBobberFinder 复用同一计分算法。
pub(crate) struct Score {
    pub(crate) point: Point,
    pub(crate) count: usize,
}

impl Score {
    /// 生产路径（Phase 6 优化）：网格分桶，语义与 C# O(n²) 完全一致
    /// （±10 严格小于邻域计数、同分取扫描序第一个）。
    /// 等价性由 tests::score_points_grid_matches_naive_on_random_sets 回归保证。
    pub(crate) fn score_points(points: &[Point]) -> Option<Score> {
        // 单元格边长 10：满足 |dx|<10 且 |dy|<10 的点必落在相邻 3×3 格内
        use std::collections::HashMap;
        let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        for (i, p) in points.iter().enumerate() {
            grid.entry((p.x.div_euclid(10), p.y.div_euclid(10)))
                .or_default()
                .push(i);
        }

        let mut best: Option<Score> = None;
        for (i, p) in points.iter().enumerate() {
            let cx = p.x.div_euclid(10);
            let cy = p.y.div_euclid(10);
            let mut count = 0usize;
            for gx in cx - 1..=cx + 1 {
                for gy in cy - 1..=cy + 1 {
                    if let Some(cell) = grid.get(&(gx, gy)) {
                        count += cell
                            .iter()
                            .filter(|&&j| {
                                let s = points[j];
                                (s.x - p.x).abs() < 10 && (s.y - p.y).abs() < 10
                            })
                            .count();
                    }
                }
            }
            let better = match &best {
                None => true,
                Some(b) => count > b.count,
            };
            if better {
                let _ = i; // 扫描序即 C# 列表序
                best = Some(Score { point: *p, count });
            }
        }
        best
    }

    /// C# ScorePoints 的保真直译（O(n²)），保留为回归测试参照实现。
    #[cfg(test)]
    fn score_points_naive(points: &[Point]) -> Option<Score> {
        let mut best: Option<Score> = None;
        for p in points.iter() {
            let count = points
                .iter()
                .filter(|s| (s.x - p.x).abs() < 10 && (s.y - p.y).abs() < 10)
                .count();
            let better = match &best {
                None => true,
                Some(b) => count > b.count,
            };
            if better {
                best = Some(Score { point: *p, count });
            }
        }
        best
    }
}

impl<C: ScreenCapture> BobberFinder for SearchBobberFinder<C> {
    fn reset(&mut self) {
        self.previous_location = None;
    }

    fn find(&mut self) -> Option<Point> {
        self.refresh_classifier();
        let frame = self.capture.capture();

        let (points, mut mask) = self.find_red_points(&frame.image);
        let mut best = Score::score_points(&points);

        // C#: 有上次命中但窗口内没找到 → 全图重搜
        if self.previous_location.is_some() && best.is_none() {
            self.previous_location = None;
            let (points2, mask2) = self.find_red_points(&frame.image);
            best = Score::score_points(&points2);
            mask = mask2;
        }

        self.previous_location = best.map(|b| b.point);

        if let Some(tx) = &self.frame_handler {
            let _ = tx.send(FrameEvent {
                frame: CapturedFrame {
                    origin: frame.origin,
                    image: frame.image,
                },
                mask,
                bobber: self.previous_location,
            });
        }

        // 位图坐标 → 屏幕坐标（对应 WowScreen.GetScreenPositionFromBitmapPostion）
        self.previous_location
            .map(|p| Point::new(p.x + frame.origin.x, p.y + frame.origin.y))
    }
}

/// 辅助：与 C# 中"命中像素染色"等价的展示色。
pub fn highlight_colour(mode: ClassifierMode) -> [u8; 3] {
    match mode {
        ClassifierMode::Red => [255, 0, 0],
        ClassifierMode::Blue => [0, 0, 255],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    struct MockCapture {
        frame: Option<CapturedFrame>,
    }

    impl MockCapture {
        fn new(image: RgbImage) -> Self {
            MockCapture {
                frame: Some(CapturedFrame {
                    image,
                    origin: Point::new(480, 270), // 模拟主屏 1920x1080 的 W/4,H/4
                }),
            }
        }
    }

    impl ScreenCapture for MockCapture {
        fn capture(&mut self) -> CapturedFrame {
            self.frame.take().expect("capture 被调用次数超过预期")
        }
    }

    fn paint_block(img: &mut RgbImage, x0: u32, y0: u32, w: u32, h: u32, colour: [u8; 3]) {
        for x in x0..x0 + w {
            for y in y0..y0 + h {
                img.put_pixel(x, y, Rgb(colour));
            }
        }
    }

    #[test]
    fn finds_red_block_and_returns_screen_coords() {
        let mut img = RgbImage::new(200, 200);
        paint_block(&mut img, 50, 60, 10, 10, [255, 0, 0]);

        let mut finder = SearchBobberFinder::new(PixelClassifier::new(), MockCapture::new(img));
        let pos = finder.find().expect("应找到浮漂");
        // 同分时取扫描序第一个 = 块左上角 (50,60) + 原点 (480,270)
        assert_eq!(pos, Point::new(530, 330));
    }

    #[test]
    fn returns_none_when_no_red() {
        let img = RgbImage::new(100, 100);
        let mut finder = SearchBobberFinder::new(PixelClassifier::new(), MockCapture::new(img));
        assert_eq!(finder.find(), None);
    }

    #[test]
    fn clears_points_when_too_much_red() {
        // 整图红色 = 40000 点 > 1000 → 清空
        let mut img = RgbImage::new(200, 200);
        paint_block(&mut img, 0, 0, 200, 200, [255, 0, 0]);
        let mut finder = SearchBobberFinder::new(PixelClassifier::new(), MockCapture::new(img));
        assert_eq!(finder.find(), None);
    }

    #[test]
    fn denser_cluster_wins() {
        let mut img = RgbImage::new(300, 100);
        paint_block(&mut img, 10, 10, 5, 5, [255, 0, 0]); // 25 点
        paint_block(&mut img, 200, 10, 9, 9, [255, 0, 0]); // 81 点，密度更高
        let mut finder = SearchBobberFinder::new(PixelClassifier::new(), MockCapture::new(img));
        let pos = finder.find().expect("应找到浮漂");
        assert_eq!(pos.x, 200 + 480);
        assert_eq!(pos.y, 10 + 270);
    }

    #[test]
    fn second_search_uses_previous_location_window() {
        // 第一次找到后，第二次 capture 在同位置仍有红点 → 窗口内找到
        let mut img1 = RgbImage::new(200, 200);
        paint_block(&mut img1, 50, 60, 10, 10, [255, 0, 0]);
        let mut img2 = RgbImage::new(200, 200);
        paint_block(&mut img2, 52, 62, 10, 10, [255, 0, 0]); // 微移

        struct TwoFrames {
            frames: std::collections::VecDeque<CapturedFrame>,
        }
        impl ScreenCapture for TwoFrames {
            fn capture(&mut self) -> CapturedFrame {
                self.frames.pop_front().expect("frames 耗尽")
            }
        }

        let origin = Point::new(480, 270);
        let mut frames = std::collections::VecDeque::new();
        frames.push_back(CapturedFrame {
            image: img1,
            origin,
        });
        frames.push_back(CapturedFrame {
            image: img2,
            origin,
        });

        let mut finder = SearchBobberFinder::new(PixelClassifier::new(), TwoFrames { frames });
        let pos1 = finder.find().unwrap();
        assert_eq!(pos1, Point::new(530, 330));
        let pos2 = finder.find().unwrap();
        assert_eq!(pos2, Point::new(532, 332));
    }

    #[test]
    fn score_points_empty_returns_none() {
        assert!(Score::score_points(&[]).is_none());
    }

    /// Phase 6 回归：网格分桶实现必须与 C# 保真直译（O(n²)）结果一致，
    /// 包括计数值与同分时的扫描序平局规则。
    #[test]
    fn score_points_grid_matches_naive_on_random_sets() {
        use rand::rngs::StdRng;
        use rand::{Rng, SeedableRng};

        let mut rng = StdRng::seed_from_u64(20261111);
        for case in 0..200 {
            let n = rng.gen_range(0..120);
            let points: Vec<Point> = (0..n)
                .map(|_| {
                    // 混合分布：80% 聚集在 60×60 区域（大量平局/邻域重叠），
                    // 20% 散布全图，覆盖边界与稀疏情形
                    if rng.gen_bool(0.8) {
                        Point::new(rng.gen_range(0..60), rng.gen_range(0..60))
                    } else {
                        Point::new(rng.gen_range(0..960), rng.gen_range(0..540))
                    }
                })
                .collect();

            let fast = Score::score_points(&points);
            let slow = Score::score_points_naive(&points);
            match (fast, slow) {
                (None, None) => {}
                (Some(f), Some(s)) => assert_eq!(
                    (f.point, f.count),
                    (s.point, s.count),
                    "case={case} n={n} 结果不一致"
                ),
                _ => panic!("case={case} n={n} 一侧为 None"),
            }
        }
    }
}
