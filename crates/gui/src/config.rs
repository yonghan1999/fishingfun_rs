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
    pub schedule_minutes: u32,
    pub schedule_action: String,
    pub macro_keys: [Option<i32>; 2],
    pub macro_interval_minutes: u32,
    pub classifier_mode: String,
    pub colour_multiplier: f64,
    pub colour_closeness_multiplier: f64,
    pub loot_delay_ms: u64,
    pub search_region_size: Option<[i32; 2]>,
    #[serde(default)]
    pub capture_version: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            cast_key: 52,
            strike_value: 7,
            search_region: None,
            humanize: true,
            schedule_minutes: 0,
            schedule_action: "stop_fishing".into(),
            macro_keys: [Some(53), Some(54)],
            macro_interval_minutes: 10,
            classifier_mode: "Red".into(),
            colour_multiplier: 0.5,
            colour_closeness_multiplier: 2.0,
            loot_delay_ms: 2000,
            search_region_size: None,
            capture_version: 1,
        }
    }
}

impl Settings {
    fn validate(&self) -> Result<(), String> {
        validate_classifier(self.colour_multiplier, self.colour_closeness_multiplier)?;
        if !matches!(self.classifier_mode.as_str(), "Red" | "Blue") || self.loot_delay_ms > 3500 {
            return Err("颜色模式或拾取延迟无效".into());
        }
        if self.capture_version > 1 {
            return Err("配置来自更新版本，请升级应用".into());
        }
        if self.capture_version == 1
            && self.search_region.is_some()
            && self.search_region_size.is_none()
        {
            return Err("搜索区域缺少原始客户区尺寸，请重新框选".into());
        }
        if let Some([w, h]) = self.search_region_size {
            if w <= 1
                || h <= 1
                || self.search_region.is_some_and(|r| {
                    i64::from(r.x) + i64::from(r.width) > i64::from(w)
                        || i64::from(r.y) + i64::from(r.height) > i64::from(h)
                })
            {
                return Err("搜索区域超出原始客户区".into());
            }
        }
        if Key::from_console_key_value(self.cast_key).is_none() {
            return Err("抛竿键无效".into());
        }
        validate_strike(self.strike_value)?;
        validate_macro(self.macro_keys, self.macro_interval_minutes)?;
        if let Some(region) = self.search_region {
            validate_region(region)?;
        }
        if self.schedule_minutes > 7 * 24 * 60 {
            return Err("定时分钟数必须在 0..10080 之间".into());
        }
        if !matches!(
            self.schedule_action.as_str(),
            "stop_fishing" | "close_wow" | "shutdown"
        ) {
            return Err("定时动作无效".into());
        }
        Ok(())
    }
}

pub fn validate_classifier(colour: f64, closeness: f64) -> Result<(), String> {
    if !colour.is_finite()
        || !(0.0..=3.0).contains(&colour)
        || !closeness.is_finite()
        || !(0.0..=5.0).contains(&closeness)
    {
        return Err("颜色乘数必须在 0..3、接近度乘数必须在 0..5 之间".into());
    }
    Ok(())
}

impl Settings {
    pub fn classifier(&self) -> fishing_core::classifier::PixelClassifier {
        fishing_core::classifier::PixelClassifier {
            mode: if self.classifier_mode == "Blue" {
                fishing_core::classifier::ClassifierMode::Blue
            } else {
                fishing_core::classifier::ClassifierMode::Red
            },
            colour_multiplier: self.colour_multiplier,
            colour_closeness_multiplier: self.colour_closeness_multiplier,
        }
    }

    pub fn region_for(&self, width: i32, height: i32) -> Option<Rect> {
        let r = self.search_region?;
        let [w, h] = self.search_region_size?;
        if width <= 1 || height <= 1 {
            return None;
        }
        let x = ((i64::from(r.x) * i64::from(width) / i64::from(w)) as i32).min(width - 1);
        let y = ((i64::from(r.y) * i64::from(height) / i64::from(h)) as i32).min(height - 1);
        let right =
            ((i64::from(r.x) + i64::from(r.width)) * i64::from(width) / i64::from(w)) as i32;
        let bottom =
            ((i64::from(r.y) + i64::from(r.height)) * i64::from(height) / i64::from(h)) as i32;
        Some(Rect::new(x, y, (right - x).max(1), (bottom - y).max(1)))
    }

    fn migrate(&mut self) {
        if self.capture_version == 0 {
            // 旧区域属于主屏中央截图，不能直接用于客户区。
            self.search_region = None;
            self.search_region_size = None;
            self.capture_version = 1;
        }
    }
}

pub fn validate_macro(keys: [Option<i32>; 2], minutes: u32) -> Result<(), String> {
    if !(1..=10080).contains(&minutes) {
        return Err("宏键周期必须为 1..10080 分钟的整数".into());
    }
    for code in keys.into_iter().flatten() {
        if code == 0
            || Key::from_console_key_value(code)
                .and_then(Key::console_key_name)
                .is_none()
        {
            return Err("宏按键无效".into());
        }
    }
    Ok(())
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

#[cfg(test)]
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
    recovery: Mutex<Option<String>>,
}

impl ConfigStore {
    pub fn load(directory: &Path, legacy: &Path) -> Result<Self, String> {
        let path = directory.join("settings.json");
        let settings = match std::fs::read_to_string(&path) {
            Ok(text) => {
                let mut settings: Settings = serde_json::from_str(&text)
                    .map_err(|e| format!("配置文件 {} 无效：{e}", path.display()))?;
                settings.validate()?;
                settings.migrate();
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
                // 旧主屏位图区域无法安全迁移为客户区坐标，保留旧文件供参考。
                write_atomic(&path, &settings)?;
                settings
            }
            Err(e) => return Err(format!("读取配置 {} 失败：{e}", path.display())),
        };
        Ok(Self {
            path,
            settings: Mutex::new(settings),
            recovery: Mutex::new(None),
        })
    }

    /// 配置损坏时保持原文件不动，允许用户通过界面导入或恢复。
    pub fn load_or_recover(directory: &Path, legacy: &Path) -> Result<Self, String> {
        match Self::load(directory, legacy) {
            Ok(store) => Ok(store),
            Err(error) if directory.join("settings.json").is_file() => Ok(Self {
                path: directory.join("settings.json"),
                settings: Mutex::new(Settings::default()),
                recovery: Mutex::new(Some(error)),
            }),
            Err(error) => Err(error),
        }
    }

    pub fn recovery_error(&self) -> Option<String> {
        self.recovery
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn replace(&self, text: Option<&str>) -> Result<PathBuf, String> {
        let mut next = match text {
            Some(text) => {
                if text.len() > 64 * 1024 {
                    return Err("配置文件超过 64KiB".into());
                }
                serde_json::from_str::<Settings>(text).map_err(|e| format!("配置无效：{e}"))?
            }
            None => Settings::default(),
        };
        next.validate()?;
        next.migrate();
        let mut current = self.settings.lock().map_err(|e| e.to_string())?;
        let backup = self.path.with_file_name(format!(
            "settings-backup-{}-{}-{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        // 备份失败就不覆盖，损坏文件也原样保留。
        use std::io::Write;
        let bytes = std::fs::read(&self.path).map_err(|e| e.to_string())?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&backup)
            .map_err(|e| e.to_string())?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        write_atomic(&self.path, &next)?;
        *current = next;
        *self.recovery.lock().map_err(|e| e.to_string())? = None;
        Ok(backup)
    }

    pub fn get(&self) -> Settings {
        self.settings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn update(&self, change: impl FnOnce(&mut Settings)) -> Result<(), String> {
        let mut current = self.settings.lock().map_err(|e| e.to_string())?;
        if self.recovery_error().is_some() {
            return Err("配置损坏，请先在设置恢复中导入配置或恢复默认值".into());
        }
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
    fn colour_delay_and_relative_region_survive_restart_and_restore() {
        let directory = std::env::temp_dir().join(format!(
            "settings-roundtrip-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let store = ConfigStore::load(&directory, &directory).unwrap();
        store
            .update(|s| {
                s.classifier_mode = "Blue".into();
                s.colour_multiplier = 1.25;
                s.colour_closeness_multiplier = 1.5;
                s.loot_delay_ms = 350;
                s.search_region = Some(Rect::new(100, 50, 200, 100));
                s.search_region_size = Some([1000, 500]);
            })
            .unwrap();
        let restored = ConfigStore::load(&directory, &directory).unwrap();
        let settings = restored.get();
        assert_eq!(
            settings.classifier().mode,
            fishing_core::classifier::ClassifierMode::Blue
        );
        assert_eq!(settings.loot_delay_ms, 350);
        assert_eq!(settings.colour_multiplier, 1.25);
        assert_eq!(
            settings.region_for(2000, 1000),
            Some(Rect::new(200, 100, 400, 200))
        );
        let exported = serde_json::to_string(&settings).unwrap();
        let original = std::fs::read_to_string(&store.path).unwrap();
        let backup = restored.replace(None).unwrap();
        assert_eq!(std::fs::read_to_string(backup).unwrap(), original);
        assert_eq!(restored.get().loot_delay_ms, 2000);
        restored.replace(Some(&exported)).unwrap();
        assert_eq!(restored.get().loot_delay_ms, 350);
        for invalid in [
            r#"{"colour_multiplier":9}"#,
            r#"{"loot_delay_ms":3501}"#,
            r#"{"classifier_mode":"Green"}"#,
        ] {
            assert!(restored.replace(Some(invalid)).is_err());
            assert_eq!(restored.get().loot_delay_ms, 350);
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn corrupt_settings_recovery_preserves_original_until_explicit_restore() {
        let directory = std::env::temp_dir().join(format!(
            "settings-recovery-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("settings.json"), "broken").unwrap();
        let store = ConfigStore::load_or_recover(&directory, &directory).unwrap();
        assert!(store.recovery_error().is_some());
        assert!(store.update(|s| s.loot_delay_ms = 0).is_err());
        assert!(store.replace(Some("invalid")).is_err());
        assert_eq!(std::fs::read_to_string(&store.path).unwrap(), "broken");
        let backup = store.replace(None).unwrap();
        assert_eq!(std::fs::read_to_string(backup).unwrap(), "broken");
        assert!(store.recovery_error().is_none());
        assert!(ConfigStore::load(&directory, &directory).is_ok());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn legacy_screen_region_is_cleared_and_tiny_scaled_region_stays_inside() {
        let mut old: Settings =
            serde_json::from_str(r#"{"search_region":{"x":10,"y":10,"width":20,"height":20}}"#)
                .unwrap();
        old.migrate();
        assert_eq!(old.search_region, None);
        old.search_region = Some(Rect::new(999, 499, 1, 1));
        old.search_region_size = Some([1000, 500]);
        assert_eq!(old.region_for(2, 2), Some(Rect::new(1, 1, 1, 1)));
    }

    #[test]
    fn old_settings_keep_default_macros_and_macro_validation_is_strict() {
        let old: Settings = serde_json::from_str(r#"{"cast_key":52,"humanize":false}"#).unwrap();
        assert_eq!(old.macro_keys, [Some(53), Some(54)]);
        assert_eq!(old.macro_interval_minutes, 10);
        assert!(old.validate().is_ok());
        for minutes in [0, 10081, u32::MAX] {
            assert!(validate_macro([None, None], minutes).is_err());
        }
        for code in [-1, 0, 256, 65535] {
            assert!(validate_macro([Some(code), None], 10).is_err());
        }
        assert!(validate_macro([None, Some(112)], 1).is_ok());
        assert!(validate_macro([None, None], 10080).is_ok());
        let mut changed = old;
        changed.macro_keys = [Some(65), None];
        changed.macro_interval_minutes = 3;
        let restored: Settings =
            serde_json::from_str(&serde_json::to_string(&changed).unwrap()).unwrap();
        assert_eq!(restored.macro_keys, [Some(65), None]);
        assert_eq!(restored.macro_interval_minutes, 3);
    }

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
        store
            .update(|s| {
                s.macro_keys = [None, Some(112)];
                s.macro_interval_minutes = 2;
            })
            .unwrap();
        let restored = ConfigStore::load(&directory, &directory).unwrap().get();
        assert_eq!(restored.macro_keys, [None, Some(112)]);
        assert_eq!(restored.macro_interval_minutes, 2);
        assert!(store.update(|s| s.macro_interval_minutes = 0).is_err());
        assert_eq!(store.get().macro_interval_minutes, 2);
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
            assert!(store.replace(None).is_err());
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
