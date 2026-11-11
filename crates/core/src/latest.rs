//! 容量为一的最新值通道：新值覆盖旧值，生产者不等待消费者处理。

use std::sync::{Arc, Condvar, Mutex};

struct Slot<T> {
    value: Option<T>,
    closed: bool,
}

type Shared<T> = Arc<(Mutex<Slot<T>>, Condvar)>;

pub struct LatestSender<T>(Shared<T>);
pub struct LatestReceiver<T>(Shared<T>);

pub fn channel<T>() -> (LatestSender<T>, LatestReceiver<T>) {
    let shared = Arc::new((
        Mutex::new(Slot {
            value: None,
            closed: false,
        }),
        Condvar::new(),
    ));
    (LatestSender(shared.clone()), LatestReceiver(shared))
}

impl<T> LatestSender<T> {
    pub fn send(&self, value: T) {
        let mut slot = self.0 .0.lock().unwrap_or_else(|e| e.into_inner());
        if !slot.closed {
            slot.value = Some(value);
            self.0 .1.notify_one();
        }
    }
}

impl<T> Drop for LatestSender<T> {
    fn drop(&mut self) {
        let mut slot = self.0 .0.lock().unwrap_or_else(|e| e.into_inner());
        slot.closed = true;
        self.0 .1.notify_one();
    }
}

impl<T> LatestReceiver<T> {
    pub fn recv(&self) -> Option<T> {
        let mut slot = self.0 .0.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(value) = slot.value.take() {
                return Some(value);
            }
            if slot.closed {
                return None;
            }
            slot = self.0 .1.wait(slot).unwrap_or_else(|e| e.into_inner());
        }
    }
}

impl<T> Drop for LatestReceiver<T> {
    fn drop(&mut self) {
        let mut slot = self.0 .0.lock().unwrap_or_else(|e| e.into_inner());
        slot.closed = true;
        slot.value = None;
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn slow_consumer_receives_latest_and_sender_drop_closes_channel() {
        let (tx, rx) = super::channel();
        for value in 0..1000 {
            tx.send(value);
        }
        drop(tx);
        assert_eq!(rx.recv(), Some(999));
        assert_eq!(rx.recv(), None);
    }
}
