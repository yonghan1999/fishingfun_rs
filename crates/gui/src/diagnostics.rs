//! 会话诊断快照和有大小上限的文件日志。
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use fishing_core::traits::{BobberFinder, CapturedFrame, ScreenCapture};
use fishing_core::{Point, Rect};
use fishing_platform_windows::{capture_client, WowTarget};
use serde::Serialize;

#[derive(Default, Clone, Serialize)]
pub struct Snapshot {
    pub activity: String,
    pub target_pid: Option<u32>,
    pub client: Option<Rect>,
    pub captures: u64,
    pub capture_errors: u64,
    pub consecutive_misses: u64,
    pub capture_ms: f64,
    pub last_error: Option<String>,
    pub remaining_secs: Option<u64>,
}

#[derive(Default)]
pub struct Diagnostics {
    pub snapshot: Mutex<Snapshot>,
    pub deadline: Mutex<Option<Instant>>,
}

impl Diagnostics {
    pub fn get(&self) -> Snapshot {
        let mut snapshot = self
            .snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        snapshot.remaining_secs = self
            .deadline
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .map(|at| at.saturating_duration_since(Instant::now()).as_secs());
        snapshot
    }
    pub fn activity(&self, activity: &str) {
        self.snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .activity = activity.into();
    }
    pub fn error(&self, error: String) {
        self.snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .last_error = Some(error);
    }
}

pub struct ClientCapture {
    pub target: WowTarget,
    pub bounds: Rect,
    pub diagnostics: Arc<Diagnostics>,
    pub cancel: fishing_core::cancellation::CancellationToken,
}

impl ClientCapture {
    fn take(&self, region: Option<Rect>) -> CapturedFrame {
        let start = Instant::now();
        let result = capture_client(self.target, Some(self.bounds), region);
        let mut d = self
            .diagnostics
            .snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        d.captures += 1;
        d.capture_ms = start.elapsed().as_secs_f64() * 1000.0;
        match result {
            Ok(frame) => frame,
            Err(error) => {
                d.capture_errors += 1;
                d.last_error = Some(error.to_string());
                tracing::error!("P01 {error:#}");
                self.cancel.cancel();
                CapturedFrame {
                    image: image::RgbImage::new(1, 1),
                    origin: Point::EMPTY,
                }
            }
        }
    }
}

impl ScreenCapture for ClientCapture {
    fn capture(&mut self) -> CapturedFrame {
        self.take(None)
    }
    fn capture_region(&mut self, region: Rect) -> CapturedFrame {
        self.take(Some(region))
    }
}

pub struct ObservedFinder<F> {
    pub finder: F,
    pub diagnostics: Arc<Diagnostics>,
}
impl<F: BobberFinder> BobberFinder for ObservedFinder<F> {
    fn find(&mut self) -> Option<Point> {
        let result = self.finder.find();
        let mut d = self
            .diagnostics
            .snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        d.consecutive_misses = if result.is_some() {
            0
        } else {
            d.consecutive_misses + 1
        };
        result
    }
    fn reset(&mut self) {
        self.finder.reset();
    }
}

pub struct FileLog {
    path: PathBuf,
}
impl FileLog {
    pub fn new(directory: PathBuf) -> Result<Self, String> {
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let path = directory.join("runtime.log");
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        Ok(Self { path })
    }
    pub fn append(&self, line: &str) -> std::io::Result<()> {
        // 当前文件和一个历史文件，各不超过约 2MiB。
        if std::fs::metadata(&self.path)?.len() + line.len() as u64 > 2 * 1024 * 1024 {
            let old = self.path.with_extension("log.1");
            if old.exists() {
                std::fs::remove_file(&old)?;
            }
            std::fs::rename(&self.path, old)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        file.write_all(line.as_bytes())?;
        file.flush()
    }
    pub fn tail(&self) -> std::io::Result<String> {
        use std::io::{Read, Seek, SeekFrom};
        let mut file = std::fs::File::open(&self.path)?;
        let length = file.metadata()?.len();
        file.seek(SeekFrom::Start(length.saturating_sub(128 * 1024)))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rolling_log_keeps_previous_file_and_exports_bounded_tail() {
        let directory = std::env::temp_dir().join(format!(
            "diagnostics-log-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let log = FileLog::new(directory.clone()).unwrap();
        log.append(&"a".repeat(2 * 1024 * 1024)).unwrap();
        log.append("latest\n").unwrap();
        assert_eq!(
            std::fs::metadata(directory.join("runtime.log.1"))
                .unwrap()
                .len(),
            2 * 1024 * 1024
        );
        assert_eq!(log.tail().unwrap(), "latest\n");
        log.append(&"b".repeat(200 * 1024)).unwrap();
        assert_eq!(log.tail().unwrap().len(), 128 * 1024);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn diagnostic_snapshot_retains_error_and_clamps_expired_countdown() {
        let diagnostics = Arc::new(Diagnostics::default());
        diagnostics.error("目标窗口已关闭".into());
        diagnostics.activity("寻找浮漂");
        *diagnostics.deadline.lock().unwrap() =
            Some(Instant::now() - std::time::Duration::from_secs(1));
        let snapshot = diagnostics.get();
        assert_eq!(snapshot.remaining_secs, Some(0));
        assert_eq!(snapshot.last_error.as_deref(), Some("目标窗口已关闭"));
        assert_eq!(snapshot.activity, "寻找浮漂");
    }
}
