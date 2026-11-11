//! F3 扩展：锁定跟踪浮漂查找器（见仓库 docs/tracking.md）。
//! 相对 SearchBobberFinder（C# 保真版，保留给 console）的改进：
//! - 团块密度过滤：稀疏笔画（玩家红名/蓝名）不参与计分；
//! - 连续帧确认：全局候选须连续出现才锁定，路过干扰免疫；
//! - 锁定后小窗跟踪 + 丢失容忍：波浪短暂盖住羽毛时保持上次位置；
//! - 可选水面搜索区域：全局搜索只认框内。

use crate::latest::LatestSender;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use image::RgbImage;

use crate::bobber_finder::{FrameEvent, Score};
use crate::classifier::PixelClassifier;
use crate::traits::{BobberFinder, CapturedFrame, ScreenCapture};
use crate::{Point, Rect};

/// 可调参数（默认值见仓库 docs/tracking.md）。
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
    frame_handler: Option<LatestSender<FrameEvent>>,
    raw: Vec<bool>,
    integral: Vec<u32>,
    points: Vec<Point>,
    last_preview: Option<Instant>,
    last_colour_warning: Option<Instant>,
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
            raw: Vec::new(),
            integral: Vec::new(),
            points: Vec::new(),
            last_preview: None,
            last_colour_warning: None,
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
            raw: Vec::new(),
            integral: Vec::new(),
            points: Vec::new(),
            last_preview: None,
            last_colour_warning: None,
        }
    }

    pub fn with_config(mut self, cfg: TrackingConfig) -> Self {
        assert!(
            cfg.density_radius >= 0
                && cfg.track_radius >= 0
                && cfg.confirm_tolerance >= 0
                && cfg.jump_tolerance >= 0
        );
        self.cfg = cfg;
        self
    }

    pub fn set_frame_handler(&mut self, handler: LatestSender<FrameEvent>) {
        self.frame_handler = Some(handler);
    }

    fn refresh_shared(&mut self) {
        if let SharedSource::Shared(shared) = &self.classifier_source {
            if let Ok(latest) = shared.read() {
                if self.classifier != *latest {
                    self.state = State::Searching;
                    self.last_seen = None;
                }
                self.classifier = latest.clone();
            }
        }
        if let SharedSource::Shared(shared) = &self.region_source {
            if let Ok(latest) = shared.read() {
                if self.region != *latest {
                    self.state = State::Searching;
                    self.last_seen = None;
                }
                self.region = *latest;
            }
        }
    }

    /// 像素分类 + 密度过滤，返回过滤后的匹配点与掩码。
    fn find_match_points(
        &mut self,
        image: &RgbImage,
        bounds: Rect,
        preview: bool,
    ) -> (Vec<Point>, Vec<bool>) {
        let mut points = std::mem::take(&mut self.points);
        points.clear();
        let mut mask = if preview {
            vec![false; image.width() as usize * image.height() as usize]
        } else {
            Vec::new()
        };
        if bounds.width == 0 || bounds.height == 0 {
            return (points, mask);
        }
        let radius = self.cfg.density_radius;
        let halo = clip(
            Rect::new(
                bounds.x.saturating_sub(radius),
                bounds.y.saturating_sub(radius),
                bounds.width.saturating_add(radius.saturating_mul(2)),
                bounds.height.saturating_add(radius.saturating_mul(2)),
            ),
            Rect::new(0, 0, image.width() as i32, image.height() as i32),
        );
        let width = halo.width as usize;
        let height = halo.height as usize;
        self.raw.resize(width * height, false);
        self.integral.resize((width + 1) * (height + 1), 0);
        self.integral.fill(0);
        let stride = width + 1;
        for y in 0..height {
            let mut row_sum = 0;
            for x in 0..width {
                let pixel = image.get_pixel(halo.x as u32 + x as u32, halo.y as u32 + y as u32);
                let hit = self.classifier.is_match(pixel[0], pixel[1], pixel[2]);
                self.raw[y * width + x] = hit;
                row_sum += u32::from(hit);
                self.integral[(y + 1) * stride + x + 1] =
                    self.integral[y * stride + x + 1] + row_sum;
            }
        }
        for y in bounds.y..bounds.y + bounds.height {
            for x in bounds.x..bounds.x + bounds.width {
                let (lx, ly) = ((x - halo.x) as usize, (y - halo.y) as usize);
                if !self.raw[ly * width + lx] {
                    continue;
                }
                let r = radius as usize;
                let (x0, y0) = (lx.saturating_sub(r), ly.saturating_sub(r));
                let (x1, y1) = ((lx + r + 1).min(width), (ly + r + 1).min(height));
                let count = self.integral[y1 * stride + x1] + self.integral[y0 * stride + x0]
                    - self.integral[y0 * stride + x1]
                    - self.integral[y1 * stride + x0];
                if count >= self.cfg.density_min {
                    points.push(Point::new(x, y));
                    if preview {
                        mask[y as usize * image.width() as usize + x as usize] = true;
                    }
                }
            }
        }
        if points.len() > 1000 {
            if self
                .last_colour_warning
                .is_none_or(|time| time.elapsed() >= Duration::from_secs(5))
            {
                tracing::warn!(
                    "Too many feather-colour pixels in search area; adjust colour settings"
                );
                self.last_colour_warning = Some(Instant::now());
            }
            points.clear();
            mask.fill(false);
        }
        (points, mask)
    }

    fn global_bounds(&self, image: &RgbImage) -> Rect {
        let full = Rect::new(0, 0, image.width() as i32, image.height() as i32);
        self.region.map_or(full, |region| clip(region, full))
    }

    fn active_bounds(&self, image: &RgbImage) -> Rect {
        let global = self.global_bounds(image);
        let center = match self.state {
            State::Searching => return global,
            State::Confirming { candidate, .. } => candidate,
            State::Locked { pos } => pos,
        };
        let r = self.cfg.track_radius;
        clip(
            Rect::new(
                center.x.saturating_sub(r),
                center.y.saturating_sub(r),
                r.saturating_mul(2).saturating_add(1),
                r.saturating_mul(2).saturating_add(1),
            ),
            global,
        )
    }

    /// 在 points 中取 window 内（±track_radius 围绕 center）计分最高者。
    fn best_in_window(&self, points: &[Point], center: Point) -> Option<Point> {
        debug_assert!(points
            .iter()
            .all(|p| dist_within(*p, center, self.cfg.track_radius)));
        Score::score_points(points).map(|s| s.point)
    }

    /// Searching/Confirming 失败后的全局搜索（受搜索区域约束）。
    /// 返回锁定结果（仅当 confirm_frames <= 1 时立即锁定）。
    fn search_global(&mut self, points: &[Point]) -> Option<Point> {
        let best = Score::score_points(points).map(|s| s.point);
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

fn clip(rect: Rect, limit: Rect) -> Rect {
    let x = rect.x.max(limit.x);
    let y = rect.y.max(limit.y);
    let end_x = (rect.x as i64 + rect.width as i64).min(limit.x as i64 + limit.width as i64);
    let end_y = (rect.y as i64 + rect.height as i64).min(limit.y as i64 + limit.height as i64);
    Rect::new(
        x,
        y,
        (end_x - x as i64).max(0) as i32,
        (end_y - y as i64).max(0) as i32,
    )
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
        let preview = self.frame_handler.is_some()
            && self
                .last_preview
                .is_none_or(|t| t.elapsed() >= Duration::from_millis(66));
        let bounds = self.active_bounds(&frame.image);
        let (mut points, mut mask) = self.find_match_points(&frame.image, bounds, preview);

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
                            self.points = points;
                            (points, mask) = self.find_match_points(
                                &frame.image,
                                self.global_bounds(&frame.image),
                                preview,
                            );
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
                    None => {
                        self.points = points;
                        (points, mask) = self.find_match_points(
                            &frame.image,
                            self.global_bounds(&frame.image),
                            preview,
                        );
                        self.search_global(&points)
                    }
                }
            }
            State::Searching => self.search_global(&points),
        };

        self.points = points;
        if preview {
            if let Some(tx) = &self.frame_handler {
                self.last_preview = Some(Instant::now());
                tx.send(FrameEvent {
                    frame: CapturedFrame {
                        origin: frame.origin,
                        image: frame.image,
                    },
                    mask,
                    bobber: result,
                });
            }
        }

        // 位图坐标 → 屏幕坐标
        result.map(|p| Point::new(p.x + frame.origin.x, p.y + frame.origin.y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    #[test]
    fn large_outside_colour_does_not_invalidate_region_or_locked_tracking() {
        let mut img = red_block_image(500, 300, 50, 50, 10);
        paint_block(&mut img, 300, 30, 100, 100, [255, 0, 0]);
        let region = Arc::new(RwLock::new(Some(Rect::new(0, 0, 150, 150))));
        let mut finder = TrackingBobberFinder::new_shared(
            Arc::new(RwLock::new(PixelClassifier::new())),
            region,
            Frames::new(vec![img.clone(), img.clone(), img]),
        );
        assert_eq!(finder.find(), None);
        let first = finder.find().expect("框外大色块不能干扰锁定");
        assert_eq!(finder.find(), Some(first));
        assert!(finder.raw.len() < 170 * 170, "锁定后不应处理全图");
    }

    #[test]
    fn region_update_unlocks_and_never_holds_position_outside_new_region() {
        let img = red_block_image(300, 200, 50, 50, 10);
        let region = Arc::new(RwLock::new(None));
        let mut finder = TrackingBobberFinder::new_shared(
            Arc::new(RwLock::new(PixelClassifier::new())),
            region.clone(),
            Frames::new(vec![img.clone(), img.clone(), img]),
        );
        finder.find();
        assert!(finder.find().is_some());
        *region.write().unwrap() = Some(Rect::new(200, 0, 100, 200));
        assert_eq!(finder.find(), None);
    }

    #[test]
    fn local_density_matches_full_frame_at_edges_and_reuses_scratch_buffers() {
        use rand::{Rng, SeedableRng};
        let mut rng = rand::rngs::StdRng::seed_from_u64(17);
        let image = RgbImage::from_fn(90, 70, |_, _| {
            if rng.gen_bool(0.7) {
                Rgb([255, 0, 0])
            } else {
                Rgb([0, 0, 0])
            }
        });
        let mut finder = finder_with(vec![]);
        let (full, _) = finder.find_match_points(&image, Rect::new(0, 0, 90, 70), true);
        // 避免 >1000 点保护影响对拍，使用较严格的密度条件。
        if full.is_empty() {
            finder.cfg.density_min = 40;
        }
        let (full, _) = finder.find_match_points(&image, Rect::new(0, 0, 90, 70), true);
        assert!(!full.is_empty());
        for bounds in [
            Rect::new(0, 0, 10, 20),
            Rect::new(80, 50, 10, 20),
            Rect::new(20, 20, 25, 25),
        ] {
            let expected: Vec<_> = full
                .iter()
                .copied()
                .filter(|p| bounds.contains(*p))
                .collect();
            let (local, _) = finder.find_match_points(&image, bounds, true);
            assert_eq!(local, expected);
            finder.points = local;
            let raw_ptr = finder.raw.as_ptr();
            let integral_ptr = finder.integral.as_ptr();
            let (points, _) = finder.find_match_points(&image, bounds, false);
            assert_eq!(raw_ptr, finder.raw.as_ptr());
            assert_eq!(integral_ptr, finder.integral.as_ptr());
            finder.points = points;
        }
    }

    #[test]
    #[ignore = "手工性能测量：cargo test -p fishing-core --release benchmark_search_areas -- --ignored --nocapture"]
    fn benchmark_search_areas() {
        for (width, height) in [(960, 440), (1280, 620), (1920, 980)] {
            let image = red_block_image(width, height, 300, 200, 10);
            let mut finder = finder_with(vec![]);
            for bounds in [
                Rect::new(0, 0, width as i32, height as i32),
                Rect::new(225, 125, 161, 161),
            ] {
                let start = Instant::now();
                for _ in 0..100 {
                    let (points, _) =
                        finder.find_match_points(std::hint::black_box(&image), bounds, false);
                    assert!(!points.is_empty());
                    finder.points = points;
                }
                println!(
                    "{width}x{height} area={}x{} classify_density_us={:.1}",
                    bounds.width,
                    bounds.height,
                    start.elapsed().as_secs_f64() * 10_000.0
                );
            }
        }
    }

    const ORIGIN: Point = Point { x: 480, y: 270 };

    struct Frames {
        frames: std::collections::VecDeque<CapturedFrame>,
    }

    impl Frames {
        fn new(images: Vec<RgbImage>) -> Self {
            Frames {
                frames: images
                    .into_iter()
                    .map(|image| CapturedFrame {
                        image,
                        origin: ORIGIN,
                    })
                    .collect(),
            }
        }
    }

    impl ScreenCapture for Frames {
        fn capture(&mut self) -> CapturedFrame {
            self.frames.pop_front().expect("capture 被调用次数超过预期")
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
        let mut f =
            TrackingBobberFinder::new(PixelClassifier::new(), Frames::new(frames)).with_config(cfg);
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

        let (tx, rx) = crate::latest::channel();
        let mut f = finder_with(vec![img]);
        f.set_frame_handler(tx);
        f.find();

        let ev = rx.recv().expect("应有帧事件");
        let width = 200i32;
        // 块内部像素在掩码中（边缘像素被密度侵蚀，取块中心附近）
        assert!(ev.mask[(64 * width + 55) as usize]);
        // 笔画像素被密度过滤剔除
        assert!(!ev.mask[(30 * width + 130) as usize]);
    }
}
