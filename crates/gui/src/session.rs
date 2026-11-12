//! 会话占位在启动线程前完成，退出清理仅影响同一会话。

use fishing_core::{cancellation::CancellationToken, traits::Key};
use serde::Serialize;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

#[derive(Clone, Copy, Serialize, PartialEq, Eq, Debug)]
pub enum Phase {
    Idle,
    Starting,
    Running,
    Stopping,
}

#[derive(Clone, Copy, Serialize, PartialEq, Eq, Debug)]
pub enum ScheduledAction {
    StopFishing,
    CloseWow,
    Shutdown,
}

pub struct Session {
    pub cancel: CancellationToken,
    pub cast_key: Arc<Mutex<Key>>,
    pub strike: i32,
    phase: Mutex<Phase>,
    pub foreground_wait_secs: Mutex<Option<u64>>,
    finished: (Mutex<bool>, Condvar),
}

#[derive(Default)]
pub struct Sessions(Mutex<Option<Arc<Session>>>);

impl Sessions {
    pub fn reserve(&self, key: Key, strike: i32) -> Result<Arc<Session>, String> {
        let mut current = self.0.lock().map_err(|e| e.to_string())?;
        if current.is_some() {
            return Err("机器人正在启动、运行或停止，请稍候".into());
        }
        let session = Arc::new(Session {
            cancel: CancellationToken::default(),
            cast_key: Arc::new(Mutex::new(key)),
            strike,
            phase: Mutex::new(Phase::Starting),
            foreground_wait_secs: Mutex::new(None),
            finished: (Mutex::new(false), Condvar::new()),
        });
        *current = Some(session.clone());
        Ok(session)
    }

    pub fn current(&self) -> Option<Arc<Session>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn finish(&self, session: &Arc<Session>) {
        self.finish_with(session, || {});
    }

    pub fn finish_with(&self, session: &Arc<Session>, notify: impl FnOnce()) {
        let mut current = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if current.as_ref().is_some_and(|s| Arc::ptr_eq(s, session)) {
            *current = None;
            notify();
        }
        *session.finished.0.lock().unwrap_or_else(|e| e.into_inner()) = true;
        session.finished.1.notify_all();
    }
}

impl Session {
    /// 在后台关闭流程中等待会话及桥接线程清理，不能阻塞 UI 事件线程。
    pub fn wait_finished(&self, timeout: Duration) -> bool {
        let finished = self.finished.0.lock().unwrap_or_else(|e| e.into_inner());
        let (finished, _) = self
            .finished
            .1
            .wait_timeout_while(finished, timeout, |done| !*done)
            .unwrap_or_else(|e| e.into_inner());
        *finished
    }

    pub fn stop(&self) {
        let mut phase = self.phase.lock().unwrap_or_else(|e| e.into_inner());
        self.cancel.cancel();
        *phase = Phase::Stopping;
    }

    pub fn run(&self) -> bool {
        let mut phase = self.phase.lock().unwrap_or_else(|e| e.into_inner());
        if self.cancel.is_cancelled() {
            return false;
        }
        *phase = Phase::Running;
        true
    }

    pub fn phase(&self) -> Phase {
        *self.phase.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shutdown_waits_for_finish_and_can_time_out_without_finishing() {
        let sessions = Arc::new(Sessions::default());
        let session = sessions.reserve(Key::D4, 7).unwrap();
        session.stop();
        assert!(!session.wait_finished(Duration::from_millis(1)));
        let worker_sessions = sessions.clone();
        let worker_session = session.clone();
        let worker = std::thread::spawn(move || worker_sessions.finish(&worker_session));
        assert!(session.wait_finished(Duration::from_secs(1)));
        worker.join().unwrap();
        assert!(sessions.current().is_none());
    }
    #[test]
    fn cancellation_during_startup_cannot_be_overwritten_and_reservation_is_exclusive() {
        let sessions = Sessions::default();
        let first = sessions.reserve(Key::D4, 7).unwrap();
        assert!(sessions.reserve(Key::D4, 7).is_err());
        first.stop();
        assert!(!first.run());
        assert_eq!(first.phase(), Phase::Stopping);
        sessions.finish(&first);
        let second = sessions.reserve(Key::D4, 13).unwrap();
        sessions.finish(&first);
        assert!(Arc::ptr_eq(&sessions.current().unwrap(), &second));
        assert!(second.run());
        assert_eq!(second.phase(), Phase::Running);
    }
}
