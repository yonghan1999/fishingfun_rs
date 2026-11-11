//! 单向取消信号：启动不会覆盖已收到的停止请求。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

#[derive(Default)]
struct CancellationState {
    cancelled: AtomicBool,
    deadline: OnceLock<Instant>,
}

#[derive(Clone, Default)]
pub struct CancellationToken(Arc<CancellationState>);

impl CancellationToken {
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
            || self
                .0
                .deadline
                .get()
                .is_some_and(|at| Instant::now() >= *at)
    }

    /// 单次会话的截止时刻，所有克隆及平台等待共享；不会覆盖已有截止时刻。
    pub fn set_deadline(&self, deadline: Instant) {
        let _ = self.0.deadline.set(deadline);
    }

    /// 每 10ms 检查取消；返回 false 表示等待被取消。
    pub fn wait(&self, duration: Duration) -> bool {
        let start = Instant::now();
        while !self.is_cancelled() {
            let Some(remaining) = duration.checked_sub(start.elapsed()) else {
                return true;
            };
            std::thread::sleep(remaining.min(Duration::from_millis(10)));
        }
        false
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn deadline_interrupts_shared_wait() {
        use super::*;
        let cancel = CancellationToken::default();
        let shared = cancel.clone();
        cancel.set_deadline(Instant::now() + Duration::from_millis(30));
        let start = Instant::now();
        assert!(!shared.wait(Duration::from_secs(5)));
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(cancel.is_cancelled());
    }

    #[test]
    fn pending_delay_is_interrupted() {
        use super::*;
        let cancel = CancellationToken::default();
        let worker_cancel = cancel.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            tx.send(()).unwrap();
            worker_cancel.wait(Duration::from_secs(5))
        });
        rx.recv().unwrap();
        cancel.cancel();
        assert!(!worker.join().unwrap());
    }
}
