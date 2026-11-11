//! 单向取消信号：启动不会覆盖已收到的停止请求。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
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
