//! M3: 迁移自 TimedAction.cs。
//! 唯一为可测性做的结构调整：时钟可注入（默认系统时钟，测试用手动时钟）。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 时钟抽象（为可测性注入，见仓库 docs/development.md）。
pub trait Clock: Send {
    fn now(&self) -> Instant;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// 测试用手动时钟。
#[derive(Debug, Clone)]
pub struct ManualClock {
    inner: Arc<Mutex<Instant>>,
}

impl ManualClock {
    pub fn new() -> Self {
        ManualClock {
            inner: Arc::new(Mutex::new(Instant::now())),
        }
    }

    pub fn advance(&self, d: Duration) {
        let mut guard = self.inner.lock().unwrap();
        *guard += d;
    }
}

impl Default for ManualClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Instant {
        *self.inner.lock().unwrap()
    }
}

/// 对应 C# TimedAction。
///
/// - 距上次动作执行超过 `action_timeout` 时再次执行动作（并重置动作计时）；
/// - `execute_if_due` 在总时长未超过 `max_time` 时返回 true。
pub struct TimedAction<C: Clock = SystemClock> {
    action: Box<dyn FnMut(u64) + Send>,
    action_timeout: Duration,
    max_time: Duration,
    clock: C,
    /// C# stopwatch：动作间隔计时
    stopwatch_start: Instant,
    /// C# maxTime：总时长计时
    max_time_start: Instant,
}

impl<C: Clock> TimedAction<C> {
    /// action 回调收到已逝去的总秒数（对应 C# 的 a.ElapsedSecs）。
    pub fn new(
        action: impl FnMut(u64) + Send + 'static,
        action_timeout: Duration,
        max_time: Duration,
        clock: C,
    ) -> Self {
        let now = clock.now();
        TimedAction {
            action: Box::new(action),
            action_timeout,
            max_time,
            clock,
            stopwatch_start: now,
            max_time_start: now,
        }
    }

    pub fn elapsed_secs(&self) -> u64 {
        self.clock
            .now()
            .duration_since(self.max_time_start)
            .as_secs()
    }

    /// 对应 C# ExecuteNow。
    pub fn execute_now(&mut self) {
        let secs = self.elapsed_secs();
        (self.action)(secs);
    }

    /// 对应 C# ExecuteIfDue：返回总时长是否仍未用尽。
    pub fn execute_if_due(&mut self) -> bool {
        let now = self.clock.now();
        if now.duration_since(self.stopwatch_start) > self.action_timeout {
            let secs = self.elapsed_secs();
            (self.action)(secs);
            self.stopwatch_start = now;
        }
        now.duration_since(self.max_time_start) < self.max_time
    }
}

impl TimedAction<SystemClock> {
    pub fn with_system_clock(
        action: impl FnMut(u64) + Send + 'static,
        action_timeout: Duration,
        max_time: Duration,
    ) -> Self {
        Self::new(action, action_timeout, max_time, SystemClock)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn fires_action_after_interval_and_resets() {
        let clock = ManualClock::new();
        let fired = Arc::new(AtomicU32::new(0));
        let fired2 = fired.clone();
        let mut ta = TimedAction::new(
            move |_| {
                fired2.fetch_add(1, Ordering::SeqCst);
            },
            Duration::from_millis(1000),
            Duration::from_secs(25),
            clock.clone(),
        );

        assert!(ta.execute_if_due());
        assert_eq!(fired.load(Ordering::SeqCst), 0);

        clock.advance(Duration::from_millis(1001));
        assert!(ta.execute_if_due());
        assert_eq!(fired.load(Ordering::SeqCst), 1);

        // 间隔被重置，立刻再调用不触发
        assert!(ta.execute_if_due());
        assert_eq!(fired.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn returns_false_after_max_time() {
        let clock = ManualClock::new();
        let mut ta = TimedAction::new(
            |_| {},
            Duration::from_millis(100),
            Duration::from_secs(5),
            clock.clone(),
        );

        clock.advance(Duration::from_secs(4));
        assert!(ta.execute_if_due());
        clock.advance(Duration::from_secs(2));
        assert!(!ta.execute_if_due());
    }

    #[test]
    fn execute_now_passes_elapsed_secs() {
        let clock = ManualClock::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        let mut ta = TimedAction::new(
            move |secs| {
                seen2.lock().unwrap().push(secs);
            },
            Duration::from_millis(100),
            Duration::from_secs(60),
            clock.clone(),
        );
        clock.advance(Duration::from_secs(7));
        ta.execute_now();
        assert_eq!(seen.lock().unwrap().as_slice(), &[7]);
    }
}
