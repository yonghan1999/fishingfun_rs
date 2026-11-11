//! Windows 平台层：截屏（M7）、WoW 进程/键鼠注入（M8，M9 常量由 windows crate 取代）。

pub mod screen;
pub mod wow_process;

pub use screen::WowScreenCapture;
pub use wow_process::{
    find_wow_process, is_wow_classic, WowKeySender, WowMouseController, WowTarget, LOOT_DELAY_MS,
};
