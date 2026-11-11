//! M8: 迁移自 WowProcess.cs（M9 Keys.cs 常量由 windows crate 内置常量取代）。
//! 保真要点：
//! - 进程名匹配：Wow / WowClassic / Wow-64，小写比较（C# ProcessName 无扩展名，
//!   Toolhelp32 的 szExeFile 带 .exe，需先剥掉）；
//! - 按键：PostMessage(WM_KEYDOWN/UP, vk, 0)，按下保持 50 + [0,75)ms；
//! - 拾取用 LiamCooper 流程：RightUp → 移动 → 等 LootDelay → RightDown/Up →
//!   恢复原窗口焦点与光标；
//! - 找不到 WoW 进程时记日志并跳过（C# 返回 null 后同样跳过，不抛异常）。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use fishing_core::cancellation::CancellationToken;
use fishing_core::traits::{Key, KeySender, MouseController};
use fishing_core::Point;
use rand::Rng;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, POINT, WPARAM};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    mouse_event, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetCursorPos, GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
    IsWindowVisible, PostMessageW, SetCursorPos, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_RBUTTONDOWN, WM_RBUTTONUP,
};

/// C# Keys.cs 中 VK_RMB（PostMessage 鼠标消息的 wParam）
const VK_RMB: usize = 0x02;

/// 对应 C# WowProcess.LootDelay 静态字段（GUI 颜色配置窗口可实时改）。
pub static LOOT_DELAY_MS: AtomicU64 = AtomicU64::new(2000);

/// 找到的 WoW 进程信息（对应 C# Process 的所需子集）。
#[derive(Debug, Clone, Copy)]
pub struct WowProcess {
    pub pid: u32,
    pub main_window: HWND,
}

/// 对应 C# WowProcess.Get：按进程名查找 WoW，找不到记日志返回 None。
pub fn find_wow_process(extra_name: Option<&str>) -> Option<WowProcess> {
    let default_names = ["wow", "wowclassic", "wow-64"];
    let names: Vec<String> = match extra_name {
        Some(n) if !n.is_empty() => vec![n.to_lowercase()],
        _ => default_names.iter().map(|s| s.to_string()).collect(),
    };

    let pid = find_process_by_names(&names);
    match pid {
        Some(pid) => {
            let hwnd = main_window_of_process(pid)?;
            Some(WowProcess {
                pid,
                main_window: hwnd,
            })
        }
        None => {
            tracing::error!(
                "Failed to find the wow process, tried: {}",
                names.join(", ")
            );
            None
        }
    }
}

/// 对应 C# WowProcess.IsWowClassic。
pub fn is_wow_classic() -> bool {
    find_process_by_names_full(&["wow".into(), "wowclassic".into(), "wow-64".into()])
        .map(|(_, name)| name.to_lowercase().contains("classic"))
        .unwrap_or(false)
}

fn find_process_by_names(names: &[String]) -> Option<u32> {
    find_process_by_names_full(names).map(|(pid, _)| pid)
}

fn find_process_by_names_full(names: &[String]) -> Option<(u32, String)> {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };

        let mut found = None;
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let exe = exe_file_name(&entry);
                // C# Process.ProcessName 不含 ".exe"
                let base = exe
                    .strip_suffix(".exe")
                    .or_else(|| exe.strip_suffix(".EXE"))
                    .unwrap_or(&exe)
                    .to_lowercase();
                if names.contains(&base) {
                    found = Some((entry.th32ProcessID, exe.clone()));
                    break;
                }
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = windows::Win32::Foundation::CloseHandle(snapshot);
        found
    }
}

fn exe_file_name(entry: &PROCESSENTRY32W) -> String {
    let len = entry
        .szExeFile
        .iter()
        .position(|c| *c == 0)
        .unwrap_or(entry.szExeFile.len());
    String::from_utf16_lossy(&entry.szExeFile[..len])
}

/// 对应 C# Process.MainWindowHandle：该进程第一个可见顶层窗口。
fn main_window_of_process(pid: u32) -> Option<HWND> {
    struct State {
        pid: u32,
        hwnd: Option<HWND>,
    }

    unsafe extern "system" fn callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let state = &mut *(lparam.0 as *mut State);
        let mut window_pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut window_pid));
        if window_pid == state.pid && IsWindowVisible(hwnd).as_bool() {
            state.hwnd = Some(hwnd);
            return BOOL(0); // 停止枚举
        }
        BOOL(1)
    }

    let mut state = State { pid, hwnd: None };
    unsafe {
        let _ = EnumWindows(Some(callback), LPARAM(&mut state as *mut State as isize));
    }
    state.hwnd
}

fn window_title(hwnd: HWND) -> String {
    unsafe {
        let mut buf = [0u16; 512];
        let len = GetWindowTextW(hwnd, &mut buf);
        String::from_utf16_lossy(&buf[..len as usize])
    }
}

/// 前台窗口进程的主窗口（对应 C# GetActiveProcess().MainWindowHandle）。
fn active_window() -> Option<HWND> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            None
        } else {
            Some(hwnd)
        }
    }
}

/// 对应 C# WowProcess 的按键功能（实现 core KeySender）。
#[derive(Debug, Default)]
pub struct WowKeySender {
    last_key: Option<Key>,
}

impl WowKeySender {
    pub fn new() -> Self {
        Self::default()
    }
}

impl KeySender for WowKeySender {
    /// C#: KeyDown → sleep(50 + Next(0,75)) → KeyUp
    fn press_key(&mut self, key: Key) {
        self.key_down(key);
        std::thread::sleep(Duration::from_millis(
            50 + rand::thread_rng().gen_range(0..75),
        ));
        self.key_up(key);
    }

    fn key_down(&mut self, key: Key) {
        self.last_key = Some(key);
        if let Some(wow) = find_wow_process(None) {
            unsafe {
                let _ = PostMessageW(
                    wow.main_window,
                    WM_KEYDOWN,
                    WPARAM(key.vk_code() as usize),
                    LPARAM(0),
                );
            }
        }
    }

    fn key_up(&mut self, key: Key) {
        if let Some(wow) = find_wow_process(None) {
            unsafe {
                let _ = PostMessageW(
                    wow.main_window,
                    WM_KEYUP,
                    WPARAM(key.vk_code() as usize),
                    LPARAM(0),
                );
            }
        }
    }
}

/// 对应 C# WowProcess 的鼠标功能（实现 core MouseController）。
#[derive(Debug, Default)]
pub struct WowMouseController;

impl WowMouseController {
    pub fn new() -> Self {
        Self
    }

    /// C# RightClickMouse_Original：缓慢移动后 PostMessage 右键（保留作备用）。
    #[allow(dead_code)]
    pub fn right_click_postmessage(&self, position: Point) {
        let Some(wow) = find_wow_process(None) else {
            return;
        };
        unsafe {
            let mut old = POINT::default();
            let _ = GetCursorPos(&mut old);

            for i in (1..=20).rev() {
                let _ = SetCursorPos(position.x + i, position.y + i);
                std::thread::sleep(Duration::from_millis(1));
            }
            std::thread::sleep(Duration::from_millis(1000));

            let _ = PostMessageW(wow.main_window, WM_RBUTTONDOWN, WPARAM(VK_RMB), LPARAM(0));
            std::thread::sleep(Duration::from_millis(
                30 + rand::thread_rng().gen_range(0..47),
            ));
            let _ = PostMessageW(wow.main_window, WM_RBUTTONUP, WPARAM(VK_RMB), LPARAM(0));

            self.refocus(wow.main_window, old);
        }
    }

    /// C# RightClickMouse()：不移动光标的 PostMessage 右键（保留作备用）。
    #[allow(dead_code)]
    pub fn right_click_plain(&self) {
        if let Some(wow) = find_wow_process(None) {
            unsafe {
                let _ = PostMessageW(wow.main_window, WM_RBUTTONDOWN, WPARAM(VK_RMB), LPARAM(0));
                std::thread::sleep(Duration::from_millis(
                    30 + rand::thread_rng().gen_range(0..47),
                ));
                let _ = PostMessageW(wow.main_window, WM_RBUTTONUP, WPARAM(VK_RMB), LPARAM(0));
            }
        }
    }

    /// C# LeftClickMouse()（保留作备用）。
    #[allow(dead_code)]
    pub fn left_click_plain(&self) {
        if let Some(wow) = find_wow_process(None) {
            unsafe {
                let _ = PostMessageW(wow.main_window, WM_LBUTTONDOWN, WPARAM(VK_RMB), LPARAM(0));
                std::thread::sleep(Duration::from_millis(
                    30 + rand::thread_rng().gen_range(0..47),
                ));
                let _ = PostMessageW(wow.main_window, WM_LBUTTONUP, WPARAM(VK_RMB), LPARAM(0));
            }
        }
    }

    /// C# RefocusOnOldScreen：把焦点交还给之前的窗口并恢复光标。
    /// C# 中 KeyDown(Escape) 发给的是 WoW 进程窗口，保持一致。
    fn refocus(&self, wow_hwnd: HWND, old_cursor: POINT) {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            let Some(active) = active_window() else {
                return;
            };
            if window_title(active) != window_title(wow_hwnd) {
                // 右键点回原窗口
                let _ = PostMessageW(active, WM_RBUTTONDOWN, WPARAM(VK_RMB), LPARAM(0));
                std::thread::sleep(Duration::from_millis(30));
                let _ = PostMessageW(active, WM_RBUTTONUP, WPARAM(VK_RMB), LPARAM(0));

                // 向 WoW 窗口发 Escape
                let _ = PostMessageW(
                    wow_hwnd,
                    WM_KEYDOWN,
                    WPARAM(Key::ESCAPE.vk_code() as usize),
                    LPARAM(0),
                );
                std::thread::sleep(Duration::from_millis(30));
                let _ = PostMessageW(
                    wow_hwnd,
                    WM_KEYUP,
                    WPARAM(Key::ESCAPE.vk_code() as usize),
                    LPARAM(0),
                );

                let _ = SetCursorPos(old_cursor.x, old_cursor.y);
            }
        }));
        if let Err(e) = result {
            tracing::error!("RefocusOnOldScreen 异常: {e:?}");
        }
    }
}

impl MouseController for WowMouseController {
    /// C# RightClickMouse → RightClickMouse_LiamCooper 流程，逐步保真。
    fn right_click_at(&mut self, position: Point) {
        self.right_click_cancellable(position, &CancellationToken::default());
    }

    fn right_click_cancellable(&mut self, position: Point, cancel: &CancellationToken) {
        if cancel.is_cancelled() {
            return;
        }
        let Some(wow) = find_wow_process(None) else {
            return;
        };
        let loot_delay = LOOT_DELAY_MS.load(Ordering::SeqCst);

        unsafe {
            mouse_event(MOUSEEVENTF_RIGHTUP, position.x, position.y, 0, 0);
            let mut old = POINT::default();

            if !cancel.wait(Duration::from_millis(200)) {
                return;
            }
            if GetCursorPos(&mut old).is_err() {
                return;
            }
            let _ = SetCursorPos(position.x, position.y);
            if !cancel.wait(Duration::from_millis(loot_delay)) {
                let _ = SetCursorPos(old.x, old.y);
                return;
            }
            mouse_event(MOUSEEVENTF_RIGHTDOWN, position.x, position.y, 0, 0);
            std::thread::sleep(Duration::from_millis(
                30 + rand::thread_rng().gen_range(0..47),
            ));
            mouse_event(MOUSEEVENTF_RIGHTUP, position.x, position.y, 0, 0);

            // 按下后必须成对释放；取消后只恢复光标，不再发送焦点/按键消息。
            if cancel.is_cancelled() {
                let _ = SetCursorPos(old.x, old.y);
                return;
            }
            self.refocus(wow.main_window, old);
            cancel.wait(Duration::from_millis(loot_delay / 2));
        }
    }
}

/// 冒烟测试（见仓库 docs/development.md：以记事本为假目标）。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "需要真实 Windows 桌面会话并手动打开记事本"]
    fn finds_notepad_process_and_window() {
        let pid = find_process_by_names(&["notepad".into()]);
        assert!(pid.is_some(), "请先打开记事本");
        let hwnd = main_window_of_process(pid.unwrap());
        assert!(hwnd.is_some());
    }

    #[test]
    #[ignore = "需要真实 Windows 桌面会话并手动打开记事本，观察字符输入"]
    fn postmessage_types_into_notepad() {
        // 人工验证：记事本应收到 'A' 键
        let pid = find_process_by_names(&["notepad".into()]).expect("请先打开记事本");
        let hwnd = main_window_of_process(pid).expect("记事本窗口");
        unsafe {
            let _ = PostMessageW(
                hwnd,
                WM_KEYDOWN,
                WPARAM(Key::A.vk_code() as usize),
                LPARAM(0),
            );
            let _ = PostMessageW(hwnd, WM_KEYUP, WPARAM(Key::A.vk_code() as usize), LPARAM(0));
        }
    }
}
