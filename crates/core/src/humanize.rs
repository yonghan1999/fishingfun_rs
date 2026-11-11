//! 拟人化节奏（扩展功能）：让自动化行为接近人类操作节奏，降低行为检测特征。
//!
//! - 咬钩反应延迟：人眼看到下沉到点击约需 150..450ms，原版是毫秒级立即点击；
//! - 抛竿间隔：每轮之间随机停顿，原版是无缝衔接的固定节拍；
//! - 周期休息：每 25..45 分钟休息 1..4 分钟，打破"永动机"式在线时长；
//! - 十分钟宏键抖动：触发窗口加 0..=90s 随机量，原版是精确的 600.000s 周期；
//! - 最长会话时长：达到上限自动停止（0 = 不限）。
//!
//! 所有延迟都是范围采样，可注入种子 RNG 复现；`disabled()` 全零，
//! 保持 C# 保真路径与现有测试的行为/速度不变。

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::time::{Duration, Instant};

/// 拟人化参数（闭区间，单位见字段名）。全零等价于关闭。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HumanizerConfig {
    /// 咬钩后到右键拾取的反应延迟
    pub reaction_ms: (u64, u64),
    /// 每轮抛竿之间的停顿
    pub between_casts_ms: (u64, u64),
    /// 十分钟宏键触发窗口的附加抖动上限（秒）
    pub ten_min_jitter_secs: u64,
    /// 两次休息之间的间隔（分钟）
    pub break_every_mins: (u64, u64),
    /// 单次休息时长（秒）
    pub break_secs: (u64, u64),
    /// 最长连续运行时长（分钟），0 = 不限
    pub max_session_mins: u64,
}

impl HumanizerConfig {
    /// 全零配置：所有拟人化行为关闭（C# 保真路径、测试默认）。
    pub fn disabled() -> Self {
        HumanizerConfig {
            reaction_ms: (0, 0),
            between_casts_ms: (0, 0),
            ten_min_jitter_secs: 0,
            break_every_mins: (0, 0),
            break_secs: (0, 0),
            max_session_mins: 0,
        }
    }
}

impl Default for HumanizerConfig {
    /// 保守的人类节奏默认值。
    fn default() -> Self {
        HumanizerConfig {
            reaction_ms: (150, 450),
            between_casts_ms: (800, 2500),
            ten_min_jitter_secs: 90,
            break_every_mins: (25, 45),
            break_secs: (60, 240),
            max_session_mins: 0,
        }
    }
}

/// 拟人化节奏发生器。持有 RNG 与调度状态（下次休息时刻、当前宏键周期）。
pub struct Humanizer {
    cfg: HumanizerConfig,
    rng: StdRng,
    session_start: Instant,
    next_break_at: Option<Instant>,
    ten_min_period: Duration,
}

impl Humanizer {
    pub fn new(cfg: HumanizerConfig) -> Self {
        Self::with_rng(cfg, StdRng::from_entropy())
    }

    /// 测试用：注入确定性 RNG。
    pub fn with_rng(cfg: HumanizerConfig, rng: StdRng) -> Self {
        let mut h = Humanizer {
            cfg,
            rng,
            session_start: Instant::now(),
            next_break_at: None,
            ten_min_period: Duration::ZERO,
        };
        h.next_break_at = h
            .breaks_enabled()
            .then(|| Instant::now() + Duration::from_secs(h.sample(h.cfg.break_every_mins) * 60));
        h.ten_min_period = h.sample_ten_min_period();
        h
    }

    /// 全零配置（等价于关闭）。
    pub fn disabled() -> Self {
        Self::new(HumanizerConfig::disabled())
    }

    fn breaks_enabled(&self) -> bool {
        self.cfg.break_every_mins != (0, 0) && self.cfg.break_secs != (0, 0)
    }

    /// 闭区间采样；自动容忍 lo > hi 的配置。
    fn sample(&mut self, range: (u64, u64)) -> u64 {
        let (lo, hi) = (range.0.min(range.1), range.0.max(range.1));
        if lo == hi {
            return lo;
        }
        self.rng.gen_range(lo..=hi)
    }

    /// 咬钩反应延迟；关闭时返回 ZERO。
    pub fn reaction_delay(&mut self) -> Duration {
        Duration::from_millis(self.sample(self.cfg.reaction_ms))
    }

    /// 每轮抛竿之间的停顿；关闭时返回 ZERO。
    pub fn between_casts_delay(&mut self) -> Duration {
        Duration::from_millis(self.sample(self.cfg.between_casts_ms))
    }

    fn sample_ten_min_period(&mut self) -> Duration {
        let jitter = if self.cfg.ten_min_jitter_secs == 0 {
            0
        } else {
            self.rng.gen_range(0..=self.cfg.ten_min_jitter_secs)
        };
        Duration::from_secs(10 * 60 + jitter)
    }

    /// 识别轮询间隔：拟人化开启时 8..=25ms 随机抖动（固定 10ms 节拍是机器特征），
    /// 关闭时固定 10ms（C# 保真）。
    pub fn poll_interval(&mut self) -> Duration {
        if self.cfg == HumanizerConfig::disabled() {
            Duration::from_millis(10)
        } else {
            Duration::from_millis(self.rng.gen_range(8..=25))
        }
    }

    /// 当前十分钟宏键周期（含抖动）。
    pub fn ten_min_period(&self) -> Duration {
        self.ten_min_period
    }

    /// 宏键触发后重新采样下一个周期。
    pub fn resample_ten_min(&mut self) {
        self.ten_min_period = self.sample_ten_min_period();
    }

    /// 到达休息时刻时返回休息时长并预约下一次；否则 None。
    pub fn break_due(&mut self) -> Option<Duration> {
        self.break_due_at(Instant::now())
    }

    fn break_due_at(&mut self, now: Instant) -> Option<Duration> {
        let due = self.next_break_at?;
        if now < due {
            return None;
        }
        let interval = Duration::from_secs(self.sample(self.cfg.break_every_mins) * 60);
        self.next_break_at = Some(now + interval);
        Some(Duration::from_secs(self.sample(self.cfg.break_secs)))
    }

    /// 是否超过最长连续运行时长（0 = 永不）。
    pub fn session_expired(&self) -> bool {
        self.cfg.max_session_mins > 0
            && self.session_start.elapsed() >= Duration::from_secs(self.cfg.max_session_mins * 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded(cfg: HumanizerConfig) -> Humanizer {
        Humanizer::with_rng(cfg, StdRng::seed_from_u64(42))
    }

    #[test]
    fn disabled_produces_zero_delays_and_no_breaks() {
        let mut h = Humanizer::disabled();
        assert_eq!(h.reaction_delay(), Duration::ZERO);
        assert_eq!(h.between_casts_delay(), Duration::ZERO);
        assert_eq!(h.ten_min_period(), Duration::from_secs(600));
        assert_eq!(h.break_due(), None);
        assert!(!h.session_expired());
    }

    #[test]
    fn samples_stay_within_configured_ranges() {
        let cfg = HumanizerConfig {
            reaction_ms: (150, 450),
            between_casts_ms: (800, 2500),
            ten_min_jitter_secs: 90,
            break_every_mins: (25, 45),
            break_secs: (60, 240),
            max_session_mins: 0,
        };
        let mut h = seeded(cfg);
        let mut reactions = std::collections::HashSet::new();
        for _ in 0..500 {
            let r = h.reaction_delay().as_millis() as u64;
            assert!((150..=450).contains(&r), "reaction {r}");
            reactions.insert(r);
            let b = h.between_casts_delay().as_millis() as u64;
            assert!((800..=2500).contains(&b), "between {b}");
            h.resample_ten_min();
            let p = h.ten_min_period().as_secs();
            assert!((600..=690).contains(&p), "ten-min period {p}");
        }
        // 采样应有多样性，不是常量
        assert!(reactions.len() > 10, "采样缺少多样性: {:?}", reactions);
    }

    #[test]
    fn swapped_range_is_tolerated() {
        let cfg = HumanizerConfig {
            reaction_ms: (450, 150),
            ..HumanizerConfig::disabled()
        };
        let mut h = seeded(cfg);
        let r = h.reaction_delay().as_millis() as u64;
        assert!((150..=450).contains(&r));
    }

    #[test]
    fn break_fires_at_scheduled_time_and_reschedules() {
        let cfg = HumanizerConfig {
            break_every_mins: (30, 30),
            break_secs: (90, 90),
            ..HumanizerConfig::disabled()
        };
        let mut h = seeded(cfg);
        let t0 = Instant::now();
        assert_eq!(h.break_due_at(t0), None, "未到时刻不休息");
        let at = t0 + Duration::from_secs(30 * 60 + 1);
        assert_eq!(
            h.break_due_at(at),
            Some(Duration::from_secs(90)),
            "到点应休息 90 秒"
        );
        // 下一次休息预约在 30 分钟后
        assert_eq!(h.break_due_at(at + Duration::from_secs(60)), None);
        assert!(h
            .break_due_at(at + Duration::from_secs(30 * 60 + 1))
            .is_some());
    }

    #[test]
    fn break_disabled_when_either_range_is_zero() {
        let mut h = seeded(HumanizerConfig {
            break_every_mins: (30, 30),
            break_secs: (0, 0),
            ..HumanizerConfig::disabled()
        });
        assert_eq!(h.next_break_at, None);
        assert_eq!(h.break_due(), None);
    }

    #[test]
    fn poll_interval_is_fixed_when_disabled_and_jittered_when_enabled() {
        let mut h = Humanizer::disabled();
        for _ in 0..10 {
            assert_eq!(h.poll_interval(), Duration::from_millis(10));
        }
        let mut h = seeded(HumanizerConfig::default());
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            let ms = h.poll_interval().as_millis() as u64;
            assert!((8..=25).contains(&ms), "轮询间隔 {ms}ms 超出 8..=25");
            seen.insert(ms);
        }
        assert!(seen.len() > 5, "轮询间隔缺少抖动: {:?}", seen);
    }
}
