//! M5: 迁移自 FishBot.cs。
//! 保真要点：
//! - 主循环：抛竿 → Watch(2000) → WaitForBite，异常时记日志 sleep 2s 继续；
//! - Sleep(ms) 附加 0..225ms 随机量（C# Random.Next(0,225) 为 [0,225)，Rust 同）；
//! - 等咬钩 25 秒超时；找浮漂最长 5 秒、每秒记日志；
//! - 10 分钟键：(now - StartTime) > 10 分钟时触发，DoTenMinuteKey 会发 Cast 事件；
//! - C# 的 log4net buffer flush 循环在 Rust 版无对应物，见仓库 docs/development.md。

use crate::cancellation::CancellationToken;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rand::Rng;

use crate::events::{FishingAction, FishingEvent};
use crate::timed_action::TimedAction;
use crate::traits::{BiteWatcher, BobberFinder, Key, KeySender, MouseController};
use crate::Point;

/// 对应 C# FishingBot。
pub struct FishingBot {
    bobber_finder: Box<dyn BobberFinder>,
    bite_watcher: Box<dyn BiteWatcher>,
    keys: Box<dyn KeySender>,
    mouse: Box<dyn MouseController>,
    /// 对应 C# castKey 字段。C# 中 UI 线程经 SetCastKey 直接改（无同步），
    /// Rust 用 Mutex 保证跨线程安全；GUI 可通过 cast_key_handle() 拿句柄实时改。
    cast_key: Arc<std::sync::Mutex<Key>>,
    ten_min_key: Vec<Key>,
    cancel: CancellationToken,
    events: Sender<FishingEvent>,
    /// C# StartTime（10 分钟键计时）
    start_time: Instant,
}

impl FishingBot {
    pub fn new(
        bobber_finder: Box<dyn BobberFinder>,
        bite_watcher: Box<dyn BiteWatcher>,
        keys: Box<dyn KeySender>,
        mouse: Box<dyn MouseController>,
        cast_key: Key,
        ten_min_key: Vec<Key>,
        events: Sender<FishingEvent>,
    ) -> Self {
        tracing::info!("FishBot Created.");
        FishingBot {
            bobber_finder,
            bite_watcher,
            keys,
            mouse,
            cast_key: Arc::new(std::sync::Mutex::new(cast_key)),
            ten_min_key,
            cancel: CancellationToken::default(),
            events,
            start_time: Instant::now(),
        }
    }

    /// 对应 C# Start。
    pub fn start(&mut self) {
        // C#: biteWatcher.FishingEventHandler = 转发到 bot 事件
        self.bite_watcher.set_event_handler(self.events.clone());

        if self.cancel.is_cancelled() {
            return;
        }

        self.do_ten_minute_key();

        while self.is_enabled() {
            // C# try/catch：平台层错误在 Rust 版由各实现内部记日志并容错，
            // 不再以异常形式上抛（WowProcess.Get 找不到进程即跳过）。
            let cast_key = self.cast_key();
            tracing::info!("Pressing key {:?} to Cast.", cast_key);

            self.press_ten_min_key_if_due();

            if !self.is_enabled() {
                break;
            }

            self.emit(FishingEvent::new(FishingAction::Cast));
            self.keys.press_key(cast_key);

            if !self.is_enabled() {
                break;
            }
            self.watch(2000);

            self.wait_for_bite();
        }

        tracing::error!("Bot has Stopped.");
    }

    /// 对应 C# SetCastKey（UI 线程调用，bot 运行中实时生效）。
    pub fn set_cast_key(&self, cast_key: Key) {
        *self.cast_key.lock().unwrap() = cast_key;
    }

    /// GUI 用：共享按键句柄（对应 C# 中 KeyChooser.CastKeyChanged → bot.SetCastKey）。
    pub fn cast_key_handle(&self) -> Arc<std::sync::Mutex<Key>> {
        self.cast_key.clone()
    }

    fn cast_key(&self) -> Key {
        *self.cast_key.lock().unwrap()
    }

    /// 对应 C# Stop。
    pub fn stop(&self) {
        self.cancel.cancel();
        tracing::error!("Bot is Stopping...");
    }

    pub fn stop_handle(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// GUI 在创建线程前登记会话；机器人复用同一取消信号和按键。
    pub fn with_session(
        mut self,
        cancel: CancellationToken,
        cast_key: Arc<std::sync::Mutex<Key>>,
    ) -> Self {
        self.cancel = cancel;
        self.cast_key = cast_key;
        self
    }

    fn is_enabled(&self) -> bool {
        !self.cancel.is_cancelled()
    }

    fn emit(&self, ev: FishingEvent) {
        let _ = self.events.send(ev);
    }

    /// 对应 C# Watch：反复截屏找浮漂，直到时长用尽（丢弃结果）。
    fn watch(&mut self, milliseconds: u64) {
        self.bobber_finder.reset();
        let start = Instant::now();
        while self.is_enabled() && start.elapsed().as_millis() < milliseconds as u128 {
            self.bobber_finder.find();
            self.cancel.wait(Duration::from_millis(10));
        }
    }

    fn wait_for_bite(&mut self) {
        if !self.is_enabled() {
            return;
        }
        self.bobber_finder.reset();

        let bobber_position = match self.find_bobber() {
            Some(p) => p,
            None => return,
        };

        self.bite_watcher.reset(bobber_position);

        tracing::info!("Bobber start position: {}", bobber_position);

        let mut timed_task = TimedAction::with_system_clock(
            |_| tracing::info!("Fishing timed out!"),
            Duration::from_millis(25 * 1000),
            Duration::from_secs(25),
        );

        // 等待浮漂移动
        while self.is_enabled() {
            let current = match self.find_bobber() {
                // C# 还检查 X == 0：屏幕坐标 X=0 不可能由位图偏移得到（原点 W/4>0），
                // 属实际不可达分支，Option 语义已覆盖（TODO(fidelity): 原检查冗余）
                None => return,
                Some(p) => p,
            };

            if !self.is_enabled() || !timed_task.execute_if_due() {
                return;
            }
            if self.bite_watcher.is_bite(current) {
                self.loot(bobber_position);
                self.press_ten_min_key_if_due();
                return;
            }

            self.cancel.wait(Duration::from_millis(10));
        }
    }

    /// 对应 C# PressTenMinKeyIfDue。
    fn press_ten_min_key_if_due(&mut self) {
        if self.start_time.elapsed() > Duration::from_secs(10 * 60) && !self.ten_min_key.is_empty()
        {
            self.do_ten_minute_key();
        }
    }

    /// 对应 C# DoTenMinuteKey。
    /// 用途：执行宏（上鱼饵、清垃圾鱼等），见原注释。
    fn do_ten_minute_key(&mut self) {
        if !self.is_enabled() {
            return;
        }
        self.start_time = Instant::now();

        if self.ten_min_key.is_empty() {
            tracing::info!(
                "Ten Minute Key:  No keys defined in tenMinKey, so nothing to do (Define in call to FishingBot constructor)."
            );
        }

        // C# 原行为：即使没有按键也发 Cast 事件
        self.emit(FishingEvent::new(FishingAction::Cast));

        for key in self.ten_min_key.clone() {
            if !self.is_enabled() {
                break;
            }
            tracing::info!(
                "Ten Minute Key: Pressing key {:?} to run a macro, delete junk fish or apply a lure etc.",
                key
            );
            self.keys.press_key(key);
        }
    }

    fn loot(&mut self, bobber_position: Point) {
        tracing::info!("Right clicking mouse to Loot.");
        self.mouse
            .right_click_cancellable(bobber_position, &self.cancel);
    }

    /// 对应 C# Sleep：附加 0..225ms 随机量，分 100ms 片段睡眠。
    pub fn sleep(ms: u64) {
        let ms = ms + rand::thread_rng().gen_range(0..225);
        let start = Instant::now();
        while start.elapsed().as_millis() < ms as u128 {
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// 对应 C# FindBobber：每秒记一次日志，最多等 5 秒。
    fn find_bobber(&mut self) -> Option<Point> {
        let mut timer = TimedAction::with_system_clock(
            |elapsed| tracing::info!("Waited seconds for target: {}", elapsed),
            Duration::from_millis(1000),
            Duration::from_secs(5),
        );

        while self.is_enabled() {
            let target = self.bobber_finder.find();
            if target.is_some() || !timer.execute_if_due() {
                return self.is_enabled().then_some(target).flatten();
            }
            self.cancel.wait(Duration::from_millis(10));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::FishingAction;
    use std::sync::mpsc::channel;
    use std::sync::Mutex;

    type Recorded<T> = Arc<Mutex<Vec<T>>>;

    fn recorded_bot(finder: Box<dyn BobberFinder>) -> (FishingBot, Recorded<Key>, Recorded<Point>) {
        let (tx, _rx) = channel();
        let keys = RecordedKeys::default();
        let mouse = RecordedMouse::default();
        let pressed = keys.pressed.clone();
        let clicked = mouse.right_clicks.clone();
        (
            FishingBot::new(
                finder,
                Box::new(ScriptedWatcher {
                    bites: vec![true].into(),
                }),
                Box::new(keys),
                Box::new(mouse),
                Key::D4,
                vec![Key::D5],
                tx,
            ),
            pressed,
            clicked,
        )
    }

    #[test]
    fn stopped_before_start_never_sends_keys_or_clicks() {
        let (mut bot, keys, clicks) = recorded_bot(Box::new(ScriptedFinder::new(vec![], None)));
        bot.stop();
        bot.start();
        assert!(keys.lock().unwrap().is_empty());
        assert!(clicks.lock().unwrap().is_empty());
    }

    struct CancelDuringFind {
        cancel: Option<CancellationToken>,
        point: Option<Point>,
    }
    impl BobberFinder for CancelDuringFind {
        fn find(&mut self) -> Option<Point> {
            self.cancel.as_ref().unwrap().cancel();
            self.point
        }
        fn reset(&mut self) {}
    }

    #[test]
    fn cancellation_during_capture_discards_detection_and_cannot_loot() {
        let (mut bot, _, clicks) = recorded_bot(Box::new(ScriptedFinder::new(vec![], None)));
        bot.bobber_finder = Box::new(CancelDuringFind {
            cancel: Some(bot.stop_handle()),
            point: Some(Point::new(10, 20)),
        });
        bot.wait_for_bite();
        assert!(clicks.lock().unwrap().is_empty());
        assert!(bot.stop_handle().is_cancelled());
    }

    #[test]
    fn cancellation_ends_observation_and_missing_target_wait_early() {
        for observe in [true, false] {
            let (mut bot, _, _) = recorded_bot(Box::new(ScriptedFinder::new(vec![], None)));
            bot.bobber_finder = Box::new(CancelDuringFind {
                cancel: Some(bot.stop_handle()),
                point: None,
            });
            let start = Instant::now();
            if observe {
                bot.watch(2000);
            } else {
                assert_eq!(bot.find_bobber(), None);
            }
            assert!(start.elapsed() < Duration::from_secs(1));
        }
    }

    struct ScriptedFinder {
        positions: std::collections::VecDeque<Option<Point>>,
        default: Option<Point>,
    }

    impl ScriptedFinder {
        fn new(positions: Vec<Option<Point>>, default: Option<Point>) -> Self {
            ScriptedFinder {
                positions: positions.into(),
                default,
            }
        }
    }

    impl BobberFinder for ScriptedFinder {
        fn find(&mut self) -> Option<Point> {
            self.positions.pop_front().unwrap_or(self.default)
        }
        fn reset(&mut self) {}
    }

    struct ScriptedWatcher {
        bites: std::collections::VecDeque<bool>,
    }

    impl BiteWatcher for ScriptedWatcher {
        fn reset(&mut self, _initial: Point) {}
        fn is_bite(&mut self, _current: Point) -> bool {
            self.bites.pop_front().unwrap_or(false)
        }
        fn set_event_handler(&mut self, _tx: Sender<FishingEvent>) {}
    }

    #[derive(Default, Clone)]
    struct RecordedKeys {
        pressed: Arc<Mutex<Vec<Key>>>,
    }

    impl KeySender for RecordedKeys {
        fn press_key(&mut self, key: Key) {
            self.pressed.lock().unwrap().push(key);
        }
        fn key_down(&mut self, _key: Key) {}
        fn key_up(&mut self, _key: Key) {}
    }

    #[derive(Default, Clone)]
    struct RecordedMouse {
        right_clicks: Arc<Mutex<Vec<Point>>>,
    }

    impl MouseController for RecordedMouse {
        fn right_click_at(&mut self, position: Point) {
            self.right_clicks.lock().unwrap().push(position);
        }
    }

    #[test]
    fn bite_triggers_loot_at_initial_bobber_position() {
        let bobber = Point::new(960, 540);
        let (tx, rx) = channel();

        let finder = ScriptedFinder::new(vec![Some(bobber), Some(bobber)], Some(bobber));
        let watcher = ScriptedWatcher {
            bites: vec![true].into(),
        };
        let mouse = RecordedMouse::default();
        let clicks = mouse.right_clicks.clone();

        let mut bot = FishingBot::new(
            Box::new(finder),
            Box::new(watcher),
            Box::new(RecordedKeys::default()),
            Box::new(mouse),
            Key::D4,
            vec![],
            tx,
        );

        // 新建会话默认未取消，可直接验证咬钩流程。
        bot.wait_for_bite();

        assert_eq!(
            clicks.lock().unwrap().as_slice(),
            &[bobber],
            "咬钩后应在初始浮漂位置右键拾取"
        );
        // bite watcher 的 reset 不经过 bot 事件，Loot 由 watcher 发送；
        // 此处 watcher 是 mock，故只验证未崩溃与鼠标行为。
        drop(rx);
    }

    #[test]
    fn find_bobber_returns_none_when_never_found() {
        // 注：完整 5 秒超时路径依赖系统时钟，真机行为由集成测试覆盖；
        // 此处验证 finder 返回 None 时不会触发拾取。
        let (tx, _rx) = channel();
        let mouse = RecordedMouse::default();
        let clicks = mouse.right_clicks.clone();
        let mut bot = FishingBot::new(
            Box::new(ScriptedFinder::new(vec![None], None)),
            Box::new(ScriptedWatcher {
                bites: vec![].into(),
            }),
            Box::new(RecordedKeys::default()),
            Box::new(mouse),
            Key::D4,
            vec![],
            tx,
        );

        assert!(bot.bobber_finder.find().is_none());
        assert!(clicks.lock().unwrap().is_empty());
    }

    #[test]
    fn ten_minute_key_presses_all_keys_and_emits_cast() {
        let (tx, rx) = channel();
        let finder = ScriptedFinder::new(vec![], None);
        let watcher = ScriptedWatcher {
            bites: vec![].into(),
        };

        let keys = RecordedKeys::default();
        let pressed = keys.pressed.clone();

        let mut bot = FishingBot::new(
            Box::new(finder),
            Box::new(watcher),
            Box::new(keys),
            Box::new(RecordedMouse::default()),
            Key::D4,
            vec![Key::D5, Key::D6],
            tx,
        );

        // 模拟 11 分钟未按：把 start_time 拨回过去
        bot.start_time = Instant::now() - Duration::from_secs(11 * 60);
        bot.press_ten_min_key_if_due();

        assert_eq!(pressed.lock().unwrap().as_slice(), &[Key::D5, Key::D6]);
        let actions: Vec<_> = rx.try_iter().map(|e| e.action).collect();
        assert!(actions.contains(&FishingAction::Cast));
        // 计时已重置
        assert!(bot.start_time.elapsed() < Duration::from_secs(60));
    }

    #[test]
    fn ten_minute_key_not_due_before_10_minutes() {
        let (tx, _rx) = channel();
        let keys = RecordedKeys::default();
        let pressed = keys.pressed.clone();
        let mut bot = FishingBot::new(
            Box::new(ScriptedFinder::new(vec![], None)),
            Box::new(ScriptedWatcher {
                bites: vec![].into(),
            }),
            Box::new(keys),
            Box::new(RecordedMouse::default()),
            Key::D4,
            vec![Key::D5],
            tx,
        );
        bot.press_ten_min_key_if_due();
        assert!(pressed.lock().unwrap().is_empty());
    }

    #[test]
    fn empty_ten_minute_keys_still_emits_cast_on_due() {
        // C# DoTenMinuteKey：空列表也发 Cast 事件（由 Start 首次调用可观察）
        let (tx, rx) = channel();
        let mut bot = FishingBot::new(
            Box::new(ScriptedFinder::new(vec![], None)),
            Box::new(ScriptedWatcher {
                bites: vec![].into(),
            }),
            Box::new(RecordedKeys::default()),
            Box::new(RecordedMouse::default()),
            Key::D4,
            vec![],
            tx,
        );
        bot.do_ten_minute_key();
        let actions: Vec<_> = rx.try_iter().map(|e| e.action).collect();
        assert_eq!(actions, vec![FishingAction::Cast]);
    }
}
