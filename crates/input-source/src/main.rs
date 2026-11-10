//! M12: 迁移自 IdentifyInputSource/cpp/InputSource.cpp（微软官方示例的衍生）。
//! 显示最近一次输入的来源（设备类型 + Hardware/Injected/System），
//! 用于验证 bot 的 PostMessage/mouse_event 注入是否被识别为 Injected。
//! 原 C# 宿主 InteractWithInputSource 依赖 Interceptor 内核驱动库，不在迁移范围
//! （注入发送功能已由 M8 WowProcess 覆盖，可直接用 bot 本体测试本工具）。

#![windows_subsystem = "windows"]

use std::sync::atomic::{AtomicI32, Ordering};
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, DrawTextW, EndPaint, FillRect, GetStockObject, InvalidateRect, UpdateWindow,
    DT_LEFT, DT_TOP, HBRUSH, PAINTSTRUCT, WHITE_BRUSH,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::GetCurrentInputMessageSource;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, LoadCursorW, LoadIconW,
    PostQuitMessage, RegisterClassExW, SetProcessDPIAware, ShowWindow, TranslateMessage,
    CW_USEDEFAULT, IDC_ARROW, IDI_APPLICATION, MSG, SW_SHOWNORMAL, WINDOW_EX_STYLE, WM_DESTROY,
    WM_DISPLAYCHANGE, WM_KEYFIRST, WM_KEYLAST, WM_MOUSEFIRST, WM_MOUSELAST, WM_PAINT,
    WM_POINTERWHEEL, WM_TOUCH, WNDCLASSEXW, WS_OVERLAPPEDWINDOW,
};

/// C++ 全局 g_inputSource（deviceType/originId 两个枚举值以 i32 存储）
static DEVICE_TYPE: AtomicI32 = AtomicI32::new(0);
static ORIGIN_ID: AtomicI32 = AtomicI32::new(0);

// INPUT_MESSAGE_DEVICE_TYPE
const IMDT_UNAVAILABLE: i32 = 0;
const IMDT_KEYBOARD: i32 = 1;
const IMDT_MOUSE: i32 = 2;
const IMDT_TOUCH: i32 = 4;
const IMDT_PEN: i32 = 8;
const IMDT_TOUCHPAD: i32 = 16;

// INPUT_MESSAGE_ORIGIN_ID
const IMO_UNAVAILABLE: i32 = 0;
const IMO_HARDWARE: i32 = 1;
const IMO_INJECTED: i32 = 2;
const IMO_SYSTEM: i32 = 4;

fn main() -> windows::core::Result<()> {
    unsafe {
        let hinstance = GetModuleHandleW(None)?;

        let class_name = w!("InputSourceApp");
        let wcex = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: windows::Win32::UI::WindowsAndMessaging::CS_HREDRAW
                | windows::Win32::UI::WindowsAndMessaging::CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: std::mem::size_of::<isize>() as i32,
            hInstance: hinstance.into(),
            hIcon: LoadIconW(None, IDI_APPLICATION)?,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hbrBackground: HBRUSH(std::ptr::null_mut()),
            lpszMenuName: windows::core::PCWSTR::null(),
            lpszClassName: class_name,
            hIconSm: LoadIconW(None, IDI_APPLICATION)?,
        };
        RegisterClassExW(&wcex);

        let _ = SetProcessDPIAware();

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class_name,
            w!("Input Source Identification Sample"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            640,
            480,
            None,
            None,
            hinstance,
            None,
        )?;

        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        let _ = UpdateWindow(hwnd);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        if (WM_MOUSEFIRST..=WM_MOUSELAST).contains(&message)
            || (WM_KEYFIRST..=WM_KEYLAST).contains(&message)
            || (WM_TOUCH..=WM_POINTERWHEEL).contains(&message)
        {
            let mut src = std::mem::zeroed();
            let _ = GetCurrentInputMessageSource(&mut src);
            DEVICE_TYPE.store(src.deviceType.0, Ordering::SeqCst);
            ORIGIN_ID.store(src.originId.0, Ordering::SeqCst);
            let _ = InvalidateRect(hwnd, None, false);
        }

        match message {
            WM_PAINT | WM_DISPLAYCHANGE => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                on_render(hdc, &ps.rcPaint);
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(1)
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

/// 对应 C++ OnRender：白底绘制 Source/Origin 两行文本。
unsafe fn on_render(hdc: windows::Win32::Graphics::Gdi::HDC, rc: &RECT) {
    unsafe {
        FillRect(hdc, rc, HBRUSH(GetStockObject(WHITE_BRUSH).0));

        let device = match DEVICE_TYPE.load(Ordering::SeqCst) {
            IMDT_UNAVAILABLE => "Unavailable",
            IMDT_KEYBOARD => "Keyboard",
            IMDT_MOUSE => "Mouse",
            IMDT_TOUCH => "Touch",
            IMDT_PEN => "Pen",
            IMDT_TOUCHPAD => "TouchPad",
            _ => "Unavailable",
        };
        let origin = match ORIGIN_ID.load(Ordering::SeqCst) {
            IMO_UNAVAILABLE => "Unavailable",
            IMO_HARDWARE => "Hardware",
            IMO_INJECTED => "Injected",
            IMO_SYSTEM => "System",
            _ => "Unavailable",
        };

        let text = format!("Source: {device}\nOrigin: {origin}");
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        let mut rc2 = *rc;
        DrawTextW(hdc, &mut wide, &mut rc2, DT_TOP | DT_LEFT);
    }
}
