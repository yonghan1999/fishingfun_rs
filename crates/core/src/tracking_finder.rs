//! F3 扩展：锁定跟踪浮漂查找器（docs/08-tracking-finder.md）。
//! 相对 SearchBobberFinder（C# 保真版，保留给 console）的改进：
//! - 团块密度过滤：稀疏笔画（玩家红名/蓝名）不参与计分；
//! - 连续帧确认：全局候选须连续出现才锁定，路过干扰免疫；
//! - 锁定后小窗跟踪 + 丢失容忍：波浪短暂盖住羽毛时保持上次位置；
//! - 可选水面搜索区域：全局搜索只认框内。

use std::sync::mpsc::Sender;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use image::RgbImage;

use crate::bobber_finder::{FrameEvent, Score};
use crate::classifier::PixelClassifier;
use crate::traits::{BobberFinder, CapturedFrame, ScreenCapture};
use crate::{Point, Rect};

/// 可调参数（默认值见 docs/08 §2）。
#[derive(Debug, Clone)]
pub struct TrackingConfig {
    /// 密度过滤邻域半径（窗口边长 = 2r+1）
    pub density_radius: i32,
    /// 邻域内至少这么多同色像素才算有效匹配
    pub density_min: u32,
    /// 全局候选需连续出现的总帧数（含首帧）
    pub confirm_frames: u32,
    /// 确认期帧间位置容差（px）
    pub confirm_tolerance: i32,
    /// 锁定后小窗搜索半径（px）
    pub track_radius: i32,
    /// 帧间位移上限，超过视为丢失而非跟随（px）
    pub jump_tolerance: i32,
    /// 锁定后丢失容忍时长（墙钟，与帧率无关）
    pub lost_tolerance: Duration,
}

impl Default for TrackingConfig {
    fn default() -> Self {
        TrackingConfig {
            density_radius: 3,
            density_min: 30,
            confirm_frames: 2,
            confirm_tolerance: 15,
            track_radius: 80,
            jump_tolerance: 40,
            lost_tolerance: Duration::from_secs(1),
        }
    }
}

/// 分类器/搜索区域来源：Owned 用于测试与离线，Shared 用于 GUI 实时共享
/// （与 SearchBobberFinder::ClassifierSource 同一模式）。
#[derive(Debug, Clone)]
enum SharedSource<T> {
    Owned,
    Shared(Arc<RwLock<T>>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Searching,
    Confirming { candidate: Point, count: u32 },
    Locked { pos: Point },
}

/// 对应 SearchBobberFinder 的扩展替代实现，实现同一 BobberFinder trait。
pub struct TrackingBobberFinder<C: ScreenCapture> {
    cfg: TrackingConfig,
    classifier_source: SharedSource<PixelClassifier>,
    region_source: SharedSource<Option<Rect>>,
    /// 每帧刷新的本地快照（避免逐像素加锁）
    classifier: PixelClassifier,
    region: Option<Rect>,
    capture: C,
    state: State,
    /// Locked 状态下最后一次命中的墙钟时刻（丢失容忽略）
    last_seen: Option<Instant>,
    frame_handler: Option<Sender<FrameEvent>>,
}

impl<C: ScreenCapture> TrackingBobberFinder<C> {
    pub fn new(classifier: PixelClassifier, capture: C) -> Self {
        TrackingBobberFinder {
            cfg: TrackingConfig::default(),
            classifier_source: SharedSource::Owned,
            region_source: SharedSource::Owned,
            classifier,
            region: None,
            capture,
            state: State::Searching,
            last_seen: None,
            frame_handler: None,
        }
    }

    /// GUI 用：共享分类器与搜索区域，配置窗口修改实时生效。
    pub fn new_shared(
        classifier: Arc<RwLock<PixelClassifier>>,
        region: Arc<RwLock<Option<Rect>>>,
        capture: C,
    ) -> Self {
        let snapshot = classifier.read().map(|c| c.clone()).unwrap_or_default();
        let region_snapshot = region.read().ok().and_then(|r| *r);
        TrackingBobberFinder {
            cfg: TrackingConfig::default(),
            classifier_source: SharedSource::Shared(classifier),
            region_source: SharedSource::Shared(region),
            classifier: snapshot,
            region: region_snapshot,
            capture,
            state: State::Searching,
            last_seen: None,
            frame_handler: None,
        }
    }

    pub fn with_config(mut self, cfg: TrackingConfig) -> Self {
        self.cfg = cfg;
        self
    }

    pub fn set_frame_handler(&mut self, handler: Sender<FrameEvent>) {
        self.frame_handler = Some(handler);
    }

    fn refresh_shared(&mut self) {
        if let SharedSource::Shared(shared) = &self.classifier_source {
            if let Ok(latest) = shared.read() {
                self.classifier = latest.clone();
            }
        }
        if let SharedSource::Shared(shared) = &self.region_source {
            if let Ok(latest) = shared.read() {
                self.region = *latest;
            }
        }
    }

    /// 像素分类 + 密度过滤，返回过滤后的匹配点与掩码。
    fn find_match_points(&self, image: &RgbImage) -> (Vec<Point>, Vec<bool>) {
        let width = image.width() as i32;
        let height = image.height() as i32;
        let n = (width * height) as usize;

        // 1) 逐像素分类
        let mut raw = vec![false; n];
        for y in 0..height {
            for x in 0..width {
                let p = image.get_pixel(x as u32, y as u32);
                if self.classifier.is_match(p[0], p[1], p[2]) {
                    raw[(y * width + x) as usize] = true;
                }
            }
        }

        // 2) 密度过滤（积分图）：稀疏笔画剔除
        let filtered = density_filter(&raw, width, height, self.cfg.density_radius, self.cfg.density_min);

        let mut points = Vec::new();
        for y in 0..height {
            for x in 0..width {
                if filtered[(y * width + x) as usize] {
                    points.push(Point::new(x, y));
                }
            }
        }

        // 沿用 C#：单帧匹配点 >1000 视为配置错误
        if points.len() > 1000 {
            tracing::error!(
                "Error: Too much of the feather colour in this image, please adjust the colour configuration !"
            );
            points.clear();
            return (points, vec![false; n]);
        }

        (points, filtered)
    }

    /// 在 points 中取 window 内（±track_radius 围绕 center）计分最高者。
    fn best_in_window(&self, points: &[Point], center: Point) -> Option<Point> {
        let r = self.cfg.track_radius;
        let windowed: Vec<Point> = points
            .iter()
            .copied()
            .filter(|p| (p.x - center.x).abs() <= r && (p.y - center.y).abs() <= r)
            .collect();
        Score::score_points(&windowed).map(|s| s.point)
    }

    /// Searching/Confirming 失败后的全局搜索（受搜索区域约束）。
    /// 返回锁定结果（仅当 confirm_frames <= 1 时立即锁定）。
    fn search_global(&mut self, points: &[Point]) -> Option<Point> {
        let candidates: Vec<Point> = match self.region {
            Some(region) => points
                .iter()
                .copied()
                .filter(|p| region.contains(*p))
                .collect(),
            None => points.to_vec(),
        };
        let best = Score::score_points(&candidates).map(|s| s.point);
        match best {
            Some(p) => {
                if self.cfg.confirm_frames <= 1 {
                    self.state = State::Locked { pos: p };
                    self.last_seen = Some(Instant::now());
                    Some(p)
                } else {
                    self.state = State::Confirming {
                        candidate: p,
                        count: 1,
                    };
                    None
                }
            }
            None => {
                self.state = State::Searching;
                None
            }
        }
    }
}

/// 密度过滤：匹配像素仅当其 (2r+1)² 邻域内同色像素数 ≥ min_count 时保留。
/// 积分图一次构建，逐像素窗口和 O(1)。
fn density_filter(mask: &[bool], width: i32, height: i32, radius: i32, min_count: u32) -> Vec<bool> {
    let iw = (width + 1) as usize;
    let mut integral = vec![0u32; ((width + 1) * (height + 1)) as usize];
    for y in 0..height {
        let mut row_sum = 0u32;
        for x in 0..width {
            if mask[(y * width + x) as usize] {
                row_sum += 1;
            }
            integral[(y as usize + 1) * iw + (x as usize + 1)] =
                integral[y as usize * iw + (x as usize + 1)] + row_sum;
        }
    }

    let window_sum = |cx: i32, cy: i32| -> u32 {
        let x0 = (cx - radius).max(0);
        let y0 = (cy - radius).max(0);
        let x1 = (cx + radius + 1).min(width);
        let y1 = (cy + radius + 1).min(height);
        let (x0, y0, x1, y1) = (x0 as usize, y0 as usize, x1 as usize, y1 as usize);
        // 先加后减，避免中间步骤下溢（debug 溢出检查）
        (integral[y1 * iw + x1] + integral[y0 * iw + x0])
            - (integral[y0 * iw + x1] + integral[y1 * iw + x0])
    };

    let mut out = vec![false; (width * height) as usize];
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) as usize;
            if mask[i] && window_sum(x, y) >= min_count {
                out[i] = true;
            }
        }
    }
    out
}

fn dist_within(a: Point, b: Point, tol: i32) -> bool {
    (a.x - b.x).abs() <= tol && (a.y - b.y).abs() <= tol
}

impl<C: ScreenCapture> BobberFinder for TrackingBobberFinder<C> {
    fn reset(&mut self) {
        self.state = State::Searching;
        self.last_seen = None;
    }

    fn find(&mut self) -> Option<Point> {
        self.refresh_shared();
        let frame = self.capture.capture();
        let (points, mask) = self.find_match_points(&frame.image);

        let now = Instant::now();
        let result: Option<Point> = match self.state {
            State::Locked { pos } => {
                let hit = self
                    .best_in_window(&points, pos)
                    .filter(|&b| dist_within(b, pos, self.cfg.jump_tolerance));
                match hit {
                    Some(b) => {
                        self.state = State::Locked { pos: b };
                        self.last_seen = Some(now);
                        Some(b)
                    }
                    None => {
                        // 波浪盖住等短暂遮挡：容忍期内保持上次位置
                        let held = self
                            .last_seen
                            .map(|t| now.duration_since(t) < self.cfg.lost_tolerance)
                            .unwrap_or(false);
                        if held {
                            Some(pos)
                        } else {
                            self.search_global(&points)
                        }
                    }
                }
            }
            State::Confirming { candidate, count } => {
                let hit = self
                    .best_in_window(&points, candidate)
                    .filter(|&b| dist_within(b, candidate, self.cfg.confirm_tolerance));
                match hit {
                    Some(b) => {
                        if count + 1 >= self.cfg.confirm_frames {
                            self.state = State::Locked { pos: b };
                            self.last_seen = Some(now);
                            Some(b)
                        } else {
                            self.state = State::Confirming {
                                candidate: b,
                                count: count + 1,
                            };
                            None
                        }
                    }
                    None => self.search_global(&points),
                }
            }
            State::Searching => self.search_global(&points),
        };

        if let Some(tx) = &self.frame_handler {
            let _ = tx.send(FrameEvent {
                frame: CapturedFrame {
                    origin: frame.origin,
                    image: frame.image,
                },
                mask,
                bobber: result,
            });
        }

        // 位图坐标 → 屏幕坐标
        result.map(|p| Point::new(p.x + frame.origin.x, p.y + frame.origin.y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    const ORIGIN: Point = Point { x: 480, y: 270 };

    struct Frames {
        frames: std::collections::VecDeque<CapturedFrame>,
    }

    impl Frames {
        fn new(images: Vec<RgbImage>) -> Self {
            Frames {
                frames: images
                    .into_iter()
                    .map(|image| CapturedFrame { image, origin: ORIGIN })
                    .collect(),
            }
        }
    }

    impl ScreenCapture for Frames {
        fn capture(&mut self) -> CapturedFrame {
            self.frames
                .pop_front()
                .expect("capture 被调用次数超过预期")
        }
    }

    fn paint_block(img: &mut RgbImage, x0: u32, y0: u32, w: u32, h: u32, colour: [u8; 3]) {
        for x in x0..x0 + w {
            for y in y0..y0 + h {
                img.put_pixel(x, y, Rgb(colour));
            }
        }
    }

    /// 画一条 2px 宽水平笔画（模拟玩家名字文字，稀疏）
    fn paint_stroke(img: &mut RgbImage, x0: u32, y0: u32, len: u32, colour: [u8; 3]) {
        for x in x0..x0 + len {
            for dy in 0..2 {
                img.put_pixel(x, y0 + dy, Rgb(colour));
            }
        }
    }

    fn red_block_image(w: u32, h: u32, x: u32, y: u32, size: u32) -> RgbImage {
        let mut img = RgbImage::new(w, h);
        paint_block(&mut img, x, y, size, size, [255, 0, 0]);
        img
    }

    /// 快捷构造：confirm_frames=2，其余默认
    fn finder_with(frames: Vec<RgbImage>) -> TrackingBobberFinder<Frames> {
        TrackingBobberFinder::new(PixelClassifier::new(), Frames::new(frames))
    }

    #[test]
    fn dense_block_locks_after_confirm_frames() {
        // 第 1 帧：候选确认中 → None；第 2 帧同位置 → 锁定并返回
        let img = red_block_image(200, 200, 50, 60, 10);
        let mut f = finder_with(vec![img.clone(), img]);
        assert_eq!(f.find(), None, "首帧应在确认中");
        let pos = f.find().expect("第 2 帧应锁定");
        // 密度过滤侵蚀边缘，锚点为块内（非 C# 的块角点），允许块内任意位置
        assert!(
            (50..60).contains(&(pos.x - 480)) && (60..70).contains(&(pos.y - 270)),
            "锁定点应在色块内: {pos}"
        );
    }

    #[test]
    fn sparse_strokes_never_lock() {
        // 多条 2px 笔画（模拟名字）：密度过滤后无有效点
        let mut img = RgbImage::new(300, 200);
        for i in 0..6 {
            paint_stroke(&mut img, 20, 20 + i * 10, 30, [255, 0, 0]);
        }
        let mut f = finder_with(vec![img.clone(), img.clone(), img]);
        assert_eq!(f.find(), None);
        assert_eq!(f.find(), None);
        assert_eq!(f.find(), None);
    }

    #[test]
    fn locked_tracks_small_movements() {
        // 锁定（50,60），随后移动到 (55,63)（< jump 40），应跟随
        let frames = vec![
            red_block_image(200, 200, 50, 60, 10),
            red_block_image(200, 200, 50, 60, 10),
            red_block_image(200, 200, 55, 63, 10),
        ];
        let mut f = finder_with(frames);
        f.find();
        let p2 = f.find().expect("第 2 帧锁定");
        assert!((50..60).contains(&(p2.x - 480)) && (60..70).contains(&(p2.y - 270)));
        let p3 = f.find().expect("移动后仍锁定");
        assert!(
            (55..65).contains(&(p3.x - 480)) && (63..73).contains(&(p3.y - 270)),
            "应跟随到新块内: {p3}"
        );
    }

    #[test]
    fn brief_occlusion_holds_last_position() {
        // 锁定后羽毛消失（被波浪盖住）：容忍期内保持上次位置
        let frames = vec![
            red_block_image(200, 200, 50, 60, 10),
            red_block_image(200, 200, 50, 60, 10),
            RgbImage::new(200, 200), // 无红
            RgbImage::new(200, 200), // 仍无红（1s 容忍远未到）
        ];
        let mut f = finder_with(frames);
        f.find();
        let locked = f.find().unwrap();
        assert_eq!(f.find(), Some(locked), "遮挡帧应保持上次位置");
        assert_eq!(f.find(), Some(locked), "容忍期内继续保持");
    }

    #[test]
    fn lost_beyond_tolerance_researches_globally() {
        // lost_tolerance=0：锁定后丢失立即全局重搜；无红 → None 回 Searching
        let frames = vec![
            red_block_image(200, 200, 50, 60, 10),
            red_block_image(200, 200, 50, 60, 10),
            RgbImage::new(200, 200),
        ];
        let cfg = TrackingConfig {
            lost_tolerance: Duration::ZERO,
            ..Default::default()
        };
        let mut f = TrackingBobberFinder::new(PixelClassifier::new(), Frames::new(frames))
            .with_config(cfg);
        f.find();
        assert!(f.find().is_some());
        assert_eq!(f.find(), None, "容忍期过后应放弃并重搜（无果→None）");
    }

    #[test]
    fn teleport_beyond_jump_tolerance_is_loss_not_follow() {
        // 锁定 (50,60)；色块瞬移到 (120,60)（在 ±80 小窗内但 > jump 40）→ 视为丢失，
        // 容忍期内保持原位置，而不是跟过去
        let frames = vec![
            red_block_image(200, 200, 50, 60, 10),
            red_block_image(200, 200, 50, 60, 10),
            red_block_image(200, 200, 120, 60, 10),
        ];
        let mut f = finder_with(frames);
        f.find();
        let locked = f.find().unwrap();
        assert_eq!(f.find(), Some(locked), "瞬移应保持原位而非跟随");
    }

    #[test]
    fn region_restricts_global_search() {
        // 色块在区域外 → 永不锁定
        let img = red_block_image(300, 200, 200, 50, 10);
        let region = Arc::new(RwLock::new(Some(Rect::new(0, 0, 100, 200))));
        let mut f = TrackingBobberFinder::new_shared(
            Arc::new(RwLock::new(PixelClassifier::new())),
            region,
            Frames::new(vec![img.clone(), img]),
        );
        assert_eq!(f.find(), None);
        assert_eq!(f.find(), None);
    }

    #[test]
    fn region_allows_block_inside() {
        let img = red_block_image(300, 200, 50, 50, 10);
        let region = Arc::new(RwLock::new(Some(Rect::new(0, 0, 100, 200))));
        let mut f = TrackingBobberFinder::new_shared(
            Arc::new(RwLock::new(PixelClassifier::new())),
            region,
            Frames::new(vec![img.clone(), img]),
        );
        f.find();
        let pos = f.find().expect("区域内应锁定");
        assert!((50..60).contains(&(pos.x - 480)) && (50..60).contains(&(pos.y - 270)));
    }

    #[test]
    fn frame_event_mask_is_density_filtered() {
        // 一个致密块 + 一条稀疏笔画：掩码只含致密块像素
        let mut img = RgbImage::new(200, 200);
        paint_block(&mut img, 50, 60, 10, 10, [255, 0, 0]);
        paint_stroke(&mut img, 120, 30, 40, [255, 0, 0]);

        let (tx, rx) = std::sync::mpsc::channel();
        let mut f = finder_with(vec![img]);
        f.set_frame_handler(tx);
        f.find();

        let ev = rx.try_recv().expect("应有帧事件");
        let width = 200i32;
        // 块内部像素在掩码中（边缘像素被密度侵蚀，取块中心附近）
        assert!(ev.mask[(64 * width + 55) as usize]);
        // 笔画像素被密度过滤剔除
        assert!(!ev.mask[(30 * width + 130) as usize]);
    }
}
