//! M2: 迁移自 PositionBiteWatcher.cs。
//! 保真要点：中位数索引算法 (count + 0.5) / 2 截断取整；
//! yDiff <= -strikeValue 判定咬钩。

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;

use crate::events::{FishingAction, FishingEvent};
use crate::timed_action::TimedAction;
use crate::traits::BiteWatcher;
use crate::Point;

/// 对应 C# PositionBiteWatcher。
pub struct PositionBiteWatcher {
    strike_value: i32,
    y_positions: Vec<i32>,
    y_diff: Arc<AtomicI32>,
    timer: Option<TimedAction>,
    /// C# Action<FishingEvent> FishingEventHandler（默认空实现 → None）
    event_handler: Option<Sender<FishingEvent>>,
}

impl PositionBiteWatcher {
    pub fn new(strike_value: i32) -> Self {
        PositionBiteWatcher {
            strike_value,
            y_positions: Vec::new(),
            y_diff: Arc::new(AtomicI32::new(0)),
            timer: None,
            event_handler: None,
        }
    }

    /// 对应 C# FishingEventHandler setter（FishBot.Start 中接线）。
    pub fn set_fishing_event_handler(&mut self, handler: Sender<FishingEvent>) {
        self.event_handler = Some(handler);
    }

    fn raise_event(&self, ev: FishingEvent) {
        if let Some(tx) = &self.event_handler {
            let _ = tx.send(ev);
        }
    }
}

impl BiteWatcher for PositionBiteWatcher {
    fn set_event_handler(&mut self, tx: Sender<FishingEvent>) {
        self.event_handler = Some(tx);
    }

    fn reset(&mut self, initial_bobber_position: Point) {
        self.raise_event(FishingEvent::new(FishingAction::Reset));

        self.y_positions = vec![initial_bobber_position.y];

        let tx = self.event_handler.clone();
        let y_diff = self.y_diff.clone();
        self.timer = Some(TimedAction::with_system_clock(
            move |_| {
                if let Some(tx) = &tx {
                    let _ = tx.send(FishingEvent {
                        action: FishingAction::BobberMove,
                        amplitude: y_diff.load(Ordering::SeqCst),
                    });
                }
            },
            Duration::from_millis(500),
            Duration::from_secs(25),
        ));
    }

    fn is_bite(&mut self, current_bobber_position: Point) -> bool {
        if !self.y_positions.contains(&current_bobber_position.y) {
            self.y_positions.push(current_bobber_position.y);
            self.y_positions.sort_unstable();
        }

        // C#: yPositions[(int)(((double)count + 0.5) / 2)]
        let idx = ((self.y_positions.len() as f64 + 0.5) / 2.0) as usize;
        let y_diff = self.y_positions[idx] - current_bobber_position.y;
        self.y_diff.store(y_diff, Ordering::SeqCst);

        let threshold_reached = y_diff <= -self.strike_value;

        if let Some(timer) = &mut self.timer {
            timer.execute_if_due();
        }

        if threshold_reached {
            // 这里只报告咬钩判定，Loot 由机器人在鼠标操作成功后发出。
            if let Some(timer) = &mut self.timer {
                timer.execute_now();
            }
            return true;
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    #[test]
    fn no_bite_during_normal_bobbing() {
        let mut w = PositionBiteWatcher::new(7);
        w.reset(Point::new(100, 200));
        // 正常漂浮：±3 像素
        for y in [200, 201, 199, 202, 198, 201, 200] {
            assert!(!w.is_bite(Point::new(100, y)), "y={y} 不应判定咬钩");
        }
    }

    #[test]
    fn sudden_drop_triggers_bite() {
        // 屏幕坐标 Y 向下增长：咬钩 = 浮漂下沉 = 当前 Y 显著大于中位数基线
        // （yDiff = 中位数 - 当前Y <= -strikeValue）
        let (tx, rx) = channel();
        let mut w = PositionBiteWatcher::new(7);
        w.set_fishing_event_handler(tx);
        w.reset(Point::new(100, 200));

        // 建立中位数基线
        w.is_bite(Point::new(100, 200));
        w.is_bite(Point::new(100, 201));

        // 下沉 7 像素（达到 strikeValue）
        assert!(w.is_bite(Point::new(100, 208)));

        // 检测到咬钩不代表已经执行拾取。
        let actions: Vec<_> = rx.try_iter().map(|e| e.action).collect();
        assert!(actions.contains(&FishingAction::Reset));
        assert!(!actions.contains(&FishingAction::Loot));
    }

    #[test]
    fn median_index_matches_csharp_truncation() {
        // C# 索引: (int)((count + 0.5) / 2)
        // count=1→0, 2→1, 3→1, 4→2, 5→2, 6→3
        let expected = [(1usize, 0usize), (2, 1), (3, 1), (4, 2), (5, 2), (6, 3)];
        for (count, want) in expected {
            let got = ((count as f64 + 0.5) / 2.0) as usize;
            assert_eq!(got, want, "count={count}");
        }
    }

    #[test]
    fn rise_never_triggers_bite() {
        // 上浮（Y 减小）使 yDiff 为正，永不触发
        let mut w = PositionBiteWatcher::new(7);
        w.reset(Point::new(100, 200));
        for y in [200, 195, 190, 180] {
            assert!(!w.is_bite(Point::new(100, y)));
        }
    }
}
