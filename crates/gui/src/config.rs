//! 固定配置目录、严格校验与同目录原子替换。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use fishing_core::{traits::Key, Rect};
use serde::{Deserialize, Serialize};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub cast_key: i32,
    pub strike_value: i32,
    pub search_region: Option<Rect>,
    /// 拟人化节奏开关（扩展功能；见 fishing_core::humanize）
    pub humanize: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            cast_key: 52,
            strike_value: 7,
            search_region: None,
            humanize: true,
        }
    }
}

impl Settings {
    fn validate(&self) -> Result<(), String> {
        if Key::from_console_key_value(self.cast_key).is_none() {
            return Err("抛竿键无效".into());
        }
        validate_strike(self.strike_value)?;
        if let Some(region) = self.search_region {
            validate_region(region)?;
        }
        Ok(())
    }
}

pub fn validate_strike(value: i32) -> Result<(), String> {
    if !(1..=100).contains(&value) {
        return Err("咬钩阈值必须在 1..100 之间".into());
    }
    Ok(())
}

pub fn validate_region(rect: Rect) -> Result<(), String> {
    if rect.x < 0
        || rect.y < 0
        || rect.width <= 0
        || rect.height <= 0
        || rect.x.checked_add(rect.width).is_none()
        || rect.y.checked_add(rect.height).is_none()
    {
        return Err("搜索区域必须为非负坐标、正宽高且不能溢出".into());
    }
    Ok(())
}

fn parse_region(text: &str) -> Option<Rect> {
    let values: Vec<i32> = text
        .trim()
        .split(',')
        .map(|s| s.trim().parse())
        .collect::<Result<_, _>>()
        .ok()?;
    let [x, y, width, height] = values.as_slice() else {
        return None;
    };
    let rect = Rect::new(*x, *y, *width, *height);
    validate_region(rect).ok()?;
    Some(rect)
}

pub struct ConfigStore {
    path: PathBuf,
    settings: Mutex<Settings>,
}

impl ConfigStore {
    pub fn load(directory: &Path, legacy: &Path) -> Result<Self, String> {
        let path = directory.join("settings.json");
        let settings = match std::fs::read_to_string(&path) {
            Ok(text) => {
                let settings: Settings = serde_json::from_str(&text)
                    .map_err(|e| format!("配置文件 {} 无效：{e}", path.display()))?;
                settings.validate()?;
                settings
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let mut settings = Settings::default();
                if let Some(key) = std::fs::read_to_string(legacy.join("keybind.txt"))
                    .ok()
                    .and_then(|s| s.trim().parse().ok())
                    .filter(|v| Key::from_console_key_value(*v).is_some())
                {
                    settings.cast_key = key;
                }
                if let Some(strike) = std::fs::read_to_string(legacy.join("strikevalue.txt"))
                    .ok()
                    .and_then(|s| s.trim().parse().ok())
                    .filter(|v| validate_strike(*v).is_ok())
                {
                    settings.strike_value = strike;
                }
                settings.search_region = std::fs::read_to_string(legacy.join("searchregion.txt"))
                    .ok()
                    .and_then(|s| parse_region(&s));
                write_atomic(&path, &settings)?;
                settings
            }
            Err(e) => return Err(format!("读取配置 {} 失败：{e}", path.display())),
        };
        Ok(Self {
            path,
            settings: Mutex::new(settings),
        })
    }

    pub fn get(&self) -> Settings {
        self.settings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn update(&self, change: impl FnOnce(&mut Settings)) -> Result<(), String> {
        let mut current = self.settings.lock().map_err(|e| e.to_string())?;
        let mut next = current.clone();
        change(&mut next);
        next.validate()?;
        write_atomic(&self.path, &next)?;
        *current = next;
        Ok(())
    }
}

fn write_atomic(path: &Path, settings: &Settings) -> Result<(), String> {
    use std::io::Write;
    let directory = path.parent().ok_or("配置路径缺少目录")?;
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let temp = directory.join(format!(
        ".settings-{}-{}.tmp",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|e| e.to_string())?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        drop(file);
        // Windows 上 std::fs::rename 使用可替换已有文件的 MoveFileExW。
        std::fs::rename(&temp, path).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result.map_err(|e| format!("保存配置 {} 失败：{e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_migration_runs_once_and_invalid_json_is_not_overwritten() {
        let directory = std::env::temp_dir().join(format!(
            "fishing-migrate-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let legacy = directory.join("legacy");
        let fixed = directory.join("fixed");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("keybind.txt"), "65").unwrap();
        std::fs::write(legacy.join("strikevalue.txt"), "23").unwrap();
        std::fs::write(legacy.join("searchregion.txt"), "1,bad,2,3,4").unwrap();
        let store = ConfigStore::load(&fixed, &legacy).unwrap();
        assert_eq!(store.get().cast_key, 65);
        assert_eq!(store.get().strike_value, 23);
        assert_eq!(store.get().search_region, None);
        assert_eq!(
            ConfigStore::load(&fixed, &directory.join("other-cwd"))
                .unwrap()
                .get()
                .strike_value,
            23
        );
        drop(store);
        std::fs::write(fixed.join("settings.json"), "invalid").unwrap();
        assert!(ConfigStore::load(&fixed, &legacy).is_err());
        assert_eq!(
            std::fs::read_to_string(fixed.join("settings.json")).unwrap(),
            "invalid"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn region_parser_rejects_bad_fields_and_overflow() {
        for text in [
            "1,bad,2,3,4",
            "1,2,3,4,5",
            "-1,0,10,10",
            "0,0,0,1",
            "2147483647,0,1,1",
        ] {
            assert!(parse_region(text).is_none(), "{text}");
        }
        assert_eq!(parse_region("1,2,3,4"), Some(Rect::new(1, 2, 3, 4)));
    }

    #[test]
    fn persists_atomically_and_failed_write_keeps_memory_and_disk() {
        let directory = std::env::temp_dir().join(format!(
            "fishing-config-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let store = ConfigStore::load(&directory, &directory).unwrap();
        store.update(|s| s.strike_value = 23).unwrap();
        assert_eq!(
            ConfigStore::load(&directory, &directory)
                .unwrap()
                .get()
                .strike_value,
            23
        );
        assert!(store.update(|s| s.strike_value = -1).is_err());
        // 文件占用使 Windows 原子替换失败，配置不能提前在内存中生效。
        let file = std::fs::File::open(&store.path).unwrap();
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            drop(file);
            let locked = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(&store.path)
                .unwrap();
            assert!(store.update(|s| s.strike_value = 24).is_err());
            assert_eq!(store.get().strike_value, 23);
            drop(locked);
        }
        #[cfg(not(windows))]
        drop(file);
        assert_eq!(
            ConfigStore::load(&directory, &directory)
                .unwrap()
                .get()
                .strike_value,
            23
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
}
