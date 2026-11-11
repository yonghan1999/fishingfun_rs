//! M8: 迁移自 WowProcess.cs（M9 Keys.cs 常量由 windows crate 内置常量取代）。
//! 保真要点：
//! - 进程名匹配：Wow / WowClassic / Wow-64，小写比较（C# ProcessName 无扩展名，
//!   Toolhelp32 的 szExeFile 带 .exe，需先剥掉）；
//! - 按键：保持 50 + [0,75)ms 后抬起（与 C# 一致）；
//! - 输入绑定会话目标；每次新输入前验证前台归属，失败停止会话；
//! - 按下的键鼠必须成对释放，释放不受失焦或取消影响。
//!
//! 有意偏离 C#：输入注入从 PostMessage/ mouse_event 迁移到 SendInput。
//! SendInput 进入系统输入流，由前台窗口接收；仍属于可识别的注入输入。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use fishing_core::cancellation::CancellationToken;
use fishing_core::traits::{InputError, InputResult, Key, KeySender, MouseController};
use fishing_core::Point;
use rand::Rng;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, POINT};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
    KEYBD_EVENT_FLAGS, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, MAPVK_VK_TO_VSC,
    MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT, MOUSE_EVENT_FLAGS, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetCursorPos, GetForegroundWindow, GetSystemMetrics, GetWindowThreadProcessId,
    IsWindow, IsWindowVisible, WindowFromPoint, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

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
#[derive(Debug)]
pub struct WowKeySender {
    target: WowTarget,
    last_key: Option<Key>,
}

impl WowKeySender {
    pub fn new(target: WowTarget) -> Self {
        Self {
            target,
            last_key: None,
        }
    }
}

/// 会话只绑定一次目标，使用整数保存窗口标识以便在线程间传递。
#[derive(Debug, Clone, Copy)]
pub struct WowTarget {
    pid: u32,
    window: isize,
}

impl WowTarget {
    pub fn find() -> Result<Self, InputError> {
        let wow = find_wow_process(None).ok_or(InputError::TargetUnavailable)?;
        let target = Self {
            pid: wow.pid,
            window: wow.main_window.0 as isize,
        };
        target.validate()?;
        Ok(target)
    }

    fn hwnd(self) -> HWND {
        HWND(self.window as *mut _)
    }

    fn validate(self) -> InputResult {
        let mut pid = 0;
        unsafe {
            if !IsWindow(self.hwnd()).as_bool() {
                return Err(InputError::TargetUnavailable);
            }
            GetWindowThreadProcessId(self.hwnd(), Some(&mut pid));
        }
        if pid != self.pid {
            return Err(InputError::TargetUnavailable);
        }
        Ok(())
    }

    pub fn check_foreground(self) -> InputResult {
        self.validate()?;
        if active_window() != Some(self.hwnd()) {
            return Err(InputError::NotForeground);
        }
        Ok(())
    }

    /// 开始按钮会使 GUI 获得焦点，最多等待十秒让用户切回目标窗口。
    pub fn wait_foreground(self, cancel: &CancellationToken) -> InputResult {
        let start = std::time::Instant::now();
        loop {
            if cancel.is_cancelled() {
                return Err(InputError::Cancelled);
            }
            match self.check_foreground() {
                Ok(()) => return Ok(()),
                Err(InputError::NotForeground) if start.elapsed() < Duration::from_secs(10) => {}
                Err(error) => return Err(error),
            }
            if !cancel.wait(Duration::from_millis(25)) {
                return Err(InputError::Cancelled);
            }
        }
    }

    fn check_point(self, point: Point) -> InputResult {
        self.check_foreground()?;
        let mut pid = 0;
        unsafe {
            let window = WindowFromPoint(POINT {
                x: point.x,
                y: point.y,
            });
            GetWindowThreadProcessId(window, Some(&mut pid));
        }
        if pid != self.pid {
            return Err(InputError::OutsideTarget);
        }
        Ok(())
    }
}

/// SendInput 注入一次按键：虚拟键码 + 扫描码（MapVirtualKeyW 取得），
/// 扩展键补 KEYEVENTF_EXTENDEDKEY。失败时向调用方报告。
fn send_key(key: Key, key_up: bool) -> InputResult {
    let vk = key.vk_code();
    let scan = unsafe { MapVirtualKeyW(vk, MAPVK_VK_TO_VSC) };
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if key_up {
        flags |= KEYEVENTF_KEYUP;
    }
    // 方向键、Home/End、Win/Apps 等扩展键必须带 EXTENDEDKEY，否则目标程序
    // 可能识别成不同的键
    if matches!(
        vk,
        0x21..=0x28 | 0x2D | 0x2E | 0x5B | 0x5C | 0x5D | 0x6F | 0x90
    ) {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk as u16),
                wScan: scan as u16,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let sent = unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
    if sent != 1 {
        tracing::error!("SendInput 按键 {:?} 被系统拦截（UIPI 权限隔离?）", key);
        return Err(InputError::Rejected);
    }
    Ok(())
}

/// 注入一次鼠标事件。绝对移动时 dx/dy 需归一化到虚拟桌面 0..=65535。
fn send_mouse_input(dx: i32, dy: i32, flags: MOUSE_EVENT_FLAGS) -> InputResult {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let sent = unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
    if sent != 1 {
        tracing::error!("SendInput 鼠标事件被系统拦截");
        return Err(InputError::Rejected);
    }
    Ok(())
}

/// 把光标移动到屏幕坐标 (x, y)，失败由调用方终止后续点击。
fn send_mouse_move(x: i32, y: i32) -> InputResult {
    let (vx, vy, vw, vh) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    let norm = |v: i32, origin: i32, size: i32| -> i32 {
        ((i64::from(v) - i64::from(origin)) * 65535 / i64::from((size - 1).max(1))).clamp(0, 65535)
            as i32
    };
    send_mouse_input(
        norm(x, vx, vw),
        norm(y, vy, vh),
        MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
    )
}

/// 新按下前验证目标；配对释放不受失焦或取消影响。
impl KeySender for WowKeySender {
    fn press_key(&mut self, key: Key) -> InputResult {
        self.press_key_cancellable(key, &CancellationToken::default())
    }

    fn press_key_cancellable(&mut self, key: Key, cancel: &CancellationToken) -> InputResult {
        if cancel.is_cancelled() {
            return Err(InputError::Cancelled);
        }
        self.key_down(key)?;
        let completed = cancel.wait(Duration::from_millis(
            50 + rand::thread_rng().gen_range(0..75),
        ));
        self.key_up(key)?;
        if completed {
            Ok(())
        } else {
            Err(InputError::Cancelled)
        }
    }
    fn key_down(&mut self, key: Key) -> InputResult {
        self.target.check_foreground()?;
        if self.last_key.is_some() {
            return Err(InputError::Rejected);
        }
        send_key(key, false)?;
        self.last_key = Some(key);
        Ok(())
    }
    fn key_up(&mut self, key: Key) -> InputResult {
        if self.last_key == Some(key) {
            // 已成功按下的键必须释放，不再检查前台，避免切屏后卡键。
            send_key(key, true)?;
            self.last_key = None;
        }
        Ok(())
    }
}
impl Drop for WowKeySender {
    fn drop(&mut self) {
        if let Some(key) = self.last_key {
            let _ = send_key(key, true);
        }
    }
}

// 正常返回、错误和 panic 都尝试释放已按下的右键。
struct RightButtonGuard(bool);
impl RightButtonGuard {
    fn release(&mut self) -> InputResult {
        send_mouse_input(0, 0, MOUSEEVENTF_RIGHTUP)?;
        self.0 = false;
        Ok(())
    }
}
impl Drop for RightButtonGuard {
    fn drop(&mut self) {
        if self.0 {
            let _ = send_mouse_input(0, 0, MOUSEEVENTF_RIGHTUP);
        }
    }
}

#[derive(Debug)]
pub struct WowMouseController {
    target: WowTarget,
    humanize: bool,
}
impl WowMouseController {
    pub fn new(target: WowTarget) -> Self {
        Self {
            target,
            humanize: false,
        }
    }
    fn check(&self, cancel: &CancellationToken) -> InputResult {
        if cancel.is_cancelled() {
            return Err(InputError::Cancelled);
        }
        self.target.check_foreground()
    }
    fn move_cursor(&self, target: Point, cancel: &CancellationToken) -> InputResult {
        self.check(cancel)?;
        self.target.check_point(target)?;
        if !self.humanize {
            return send_mouse_move(target.x, target.y);
        }
        let mut rng = rand::thread_rng();
        let jittered = Point::new(
            target.x + rng.gen_range(-3..=3),
            target.y + rng.gen_range(-3..=3),
        );
        self.target.check_point(jittered)?;
        let mut from = POINT::default();
        unsafe {
            GetCursorPos(&mut from).map_err(|_| InputError::Rejected)?;
        }
        let dx = f64::from(jittered.x) - f64::from(from.x);
        let dy = f64::from(jittered.y) - f64::from(from.y);
        let steps = ((dx.hypot(dy) / 15.0) as u32).clamp(8, 45);
        for i in 1..=steps {
            self.check(cancel)?;
            let t = f64::from(i) / f64::from(steps);
            let ease = 1.0 - (1.0 - t).powi(3);
            let (jx, jy) = if i == steps {
                (0, 0)
            } else {
                (rng.gen_range(-2..=2), rng.gen_range(-2..=2))
            };
            send_mouse_move(
                from.x + (dx * ease) as i32 + jx,
                from.y + (dy * ease) as i32 + jy,
            )?;
            if !cancel.wait(Duration::from_millis(10 + rng.gen_range(0..7))) {
                return Err(InputError::Cancelled);
            }
        }
        Ok(())
    }
    fn click(&self, position: Point, cancel: &CancellationToken) -> InputResult {
        self.check(cancel)?;
        if !cancel.wait(Duration::from_millis(200)) {
            return Err(InputError::Cancelled);
        }
        self.move_cursor(position, cancel)?;
        let loot_delay = LOOT_DELAY_MS.load(Ordering::SeqCst);
        if !cancel.wait(Duration::from_millis(loot_delay)) {
            return Err(InputError::Cancelled);
        }
        self.check(cancel)?;
        let mut actual = POINT::default();
        unsafe {
            GetCursorPos(&mut actual).map_err(|_| InputError::Rejected)?;
        }
        // 等待期间用户可能移动鼠标；不能在新位置继续点击。
        let radius = if self.humanize { 4 } else { 1 };
        if (i64::from(actual.x) - i64::from(position.x)).abs() > radius
            || (i64::from(actual.y) - i64::from(position.y)).abs() > radius
        {
            return Err(InputError::OutsideTarget);
        }
        self.target.check_point(Point::new(actual.x, actual.y))?;
        send_mouse_input(0, 0, MOUSEEVENTF_RIGHTDOWN)?;
        let mut held = RightButtonGuard(true);
        // 即使等待被取消也要释放，点击已发出后返回成功以准确报告事件。
        cancel.wait(Duration::from_millis(
            30 + rand::thread_rng().gen_range(0..47),
        ));
        held.release()?;
        cancel.wait(Duration::from_millis(loot_delay / 2));
        Ok(())
    }
}
impl MouseController for WowMouseController {
    fn set_humanize(&mut self, enabled: bool) {
        self.humanize = enabled;
    }
    fn right_click_at(&mut self, position: Point) -> InputResult {
        self.right_click_cancellable(position, &CancellationToken::default())
    }
    fn right_click_cancellable(
        &mut self,
        position: Point,
        cancel: &CancellationToken,
    ) -> InputResult {
        self.click(position, cancel)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_target_rejects_input_without_injection() {
        let target = WowTarget { pid: 0, window: 0 };
        assert_eq!(
            target.check_foreground(),
            Err(InputError::TargetUnavailable)
        );
        let mut keys = WowKeySender::new(target);
        assert_eq!(keys.press_key(Key::D4), Err(InputError::TargetUnavailable));
        assert_eq!(keys.last_key, None);
        let mut mouse = WowMouseController::new(target);
        assert_eq!(
            mouse.right_click_at(Point::new(100, 100)),
            Err(InputError::TargetUnavailable)
        );
    }
    #[test]
    fn cancelled_mouse_operation_never_injects_input() {
        let target = WowTarget { pid: 0, window: 0 };
        let cancel = CancellationToken::default();
        cancel.cancel();
        let mut mouse = WowMouseController::new(target);
        assert_eq!(
            mouse.right_click_cancellable(Point::EMPTY, &cancel),
            Err(InputError::Cancelled)
        );
    }
    #[test]
    #[ignore = "需要真实 Windows 桌面会话并手动打开记事本"]
    fn finds_notepad_process_and_window() {
        let pid = find_process_by_names(&["notepad".into()]).expect("请先打开记事本");
        assert!(main_window_of_process(pid).is_some());
    }
}
