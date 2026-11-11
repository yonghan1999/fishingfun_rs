//! M8: 迁移自 WowProcess.cs（M9 Keys.cs 常量由 windows crate 内置常量取代）。
//! 保真要点：
//! - 进程名匹配：Wow / WowClassic / Wow-64，小写比较（C# ProcessName 无扩展名，
//!   Toolhelp32 的 szExeFile 带 .exe，需先剥掉）；
//! - 按键：保持 50 + [0,75)ms 后抬起（与 C# 一致）；
//! - 拾取用 LiamCooper 流程：RightUp → 拟人化移动 → 等 LootDelay → RightDown/Up →
//!   恢复原窗口焦点与光标；
//! - 找不到 WoW 进程时记日志并跳过（C# 返回 null 后同样跳过，不抛异常）。
//!
//! 有意偏离 C#：输入注入从 PostMessage/ mouse_event 迁移到 SendInput。
//! PostMessage 绕过系统输入队列（无扫描码、无队列上下文，是行为检测的典型特征）；
//! SendInput 进入 win32k 输入流，与真实输入走同一路径。代价是按键由前台窗口
//! 接收：WoW 不在前台时会记警告日志（拾取的光标移动本来就要求游戏可见）。

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
    MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
    KEYBD_EVENT_FLAGS, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, MAPVK_VK_TO_VSC,
    MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT, MOUSE_EVENT_FLAGS, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetCursorPos, GetForegroundWindow, GetSystemMetrics, GetWindowTextW,
    GetWindowThreadProcessId, IsWindowVisible, PostMessageW, SetCursorPos, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN,
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

/// SendInput 注入一次按键：虚拟键码 + 扫描码（MapVirtualKeyW 取得），
/// 扩展键补 KEYEVENTF_EXTENDEDKEY。返回 false 表示被 UIPI 等拦截。
fn send_key(key: Key, key_up: bool) -> bool {
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
        return false;
    }
    true
}

/// 注入一次鼠标事件。绝对移动时 dx/dy 需归一化到虚拟桌面 0..=65535。
fn send_mouse_input(dx: i32, dy: i32, flags: MOUSE_EVENT_FLAGS) -> bool {
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
        return false;
    }
    true
}

/// 把光标移动到屏幕坐标 (x, y)：走输入流的绝对移动（低级钩子可见、
/// 产生正常 WM_MOUSEMOVE），比 SetCursorPos 更接近真实鼠标。
fn send_mouse_move(x: i32, y: i32) {
    let (vx, vy, vw, vh) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    let norm = |v: i32, origin: i32, size: i32| -> i32 {
        (i64::from(v - origin) * 65535 / i64::from((size - 1).max(1))).clamp(0, 65535) as i32
    };
    send_mouse_input(
        norm(x, vx, vw),
        norm(y, vy, vh),
        MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
    );
}

/// SendInput 的按键由前台窗口接收；WoW 不在前台时按键会误入其他窗口，
/// 限频警告（30s），避免日志刷屏。
fn warn_if_wow_not_foreground() {
    let ok = find_wow_process(None)
        .and_then(|wow| active_window().map(|active| active.0 == wow.main_window.0))
        .unwrap_or(false);
    if ok {
        return;
    }
    static LAST_WARN: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    if let Ok(mut last) = LAST_WARN.lock() {
        if last.is_none_or(|time| time.elapsed() >= Duration::from_secs(30)) {
            tracing::warn!("WoW 窗口不在前台：SendInput 按键将发送给当前前台窗口");
            *last = Some(std::time::Instant::now());
        }
    }
}

impl KeySender for WowKeySender {
    /// C#: KeyDown → sleep(50 + Next(0,75)) → KeyUp
    fn press_key(&mut self, key: Key) {
        warn_if_wow_not_foreground();
        self.key_down(key);
        std::thread::sleep(Duration::from_millis(
            50 + rand::thread_rng().gen_range(0..75),
        ));
        self.key_up(key);
    }

    fn key_down(&mut self, key: Key) {
        self.last_key = Some(key);
        send_key(key, false);
    }

    fn key_up(&mut self, key: Key) {
        send_key(key, true);
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

    /// 拟人化移动光标（扩展功能）：目标点 ±3px 随机偏移（人点不中精确像素），
    /// 缓出曲线分步移动（起步快、收尾慢），步间 10..17ms、每步 ±2px 微扰。
    /// 取消时光标停在中途（像人手离开鼠标），返回 false。
    fn move_cursor_humanlike(&self, target: Point, cancel: &CancellationToken) -> bool {
        let mut rng = rand::thread_rng();
        let jittered = Point::new(
            target.x + rng.gen_range(-3..=3),
            target.y + rng.gen_range(-3..=3),
        );
        unsafe {
            let mut from = POINT::default();
            if GetCursorPos(&mut from).is_err() {
                return false;
            }
            let dx = f64::from(jittered.x - from.x);
            let dy = f64::from(jittered.y - from.y);
            let dist = (dx * dx + dy * dy).sqrt();
            let steps = ((dist / 15.0) as u32).clamp(8, 45);
            for i in 1..=steps {
                if cancel.is_cancelled() {
                    return false;
                }
                let t = f64::from(i) / f64::from(steps);
                let ease = 1.0 - (1.0 - t).powi(3);
                // 终点不叠加微扰，保证落点在 ±3px 目标内
                let (jx, jy) = if i == steps {
                    (0, 0)
                } else {
                    (rng.gen_range(-2..=2), rng.gen_range(-2..=2))
                };
                send_mouse_move(
                    from.x + (dx * ease) as i32 + jx,
                    from.y + (dy * ease) as i32 + jy,
                );
                std::thread::sleep(Duration::from_millis(10 + rng.gen_range(0..7)));
            }
        }
        true
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
            // 初始 RightUp 在原光标位置（C# mouse_event 无 MOVE 标志时坐标被忽略）
            send_mouse_input(0, 0, MOUSEEVENTF_RIGHTUP);
            let mut old = POINT::default();

            if !cancel.wait(Duration::from_millis(200)) {
                return;
            }
            if GetCursorPos(&mut old).is_err() {
                return;
            }
            // 拟人化：替换 C# 的瞬时 SetCursorPos 瞬移（行为检测的典型特征）。
            // 取消/失败时光标回原位，不再继续点击。
            if !self.move_cursor_humanlike(position, cancel) {
                send_mouse_move(old.x, old.y);
                return;
            }
            if !cancel.wait(Duration::from_millis(loot_delay)) {
                send_mouse_move(old.x, old.y);
                return;
            }
            send_mouse_input(0, 0, MOUSEEVENTF_RIGHTDOWN);
            std::thread::sleep(Duration::from_millis(
                30 + rand::thread_rng().gen_range(0..47),
            ));
            send_mouse_input(0, 0, MOUSEEVENTF_RIGHTUP);

            // 按下后必须成对释放；取消后只恢复光标，不再发送焦点/按键消息。
            if cancel.is_cancelled() {
                send_mouse_move(old.x, old.y);
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
