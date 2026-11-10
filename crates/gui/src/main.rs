//! M11: FishingFun GUI（Tauri 2）— WPF 版一比一复刻的后端。
//! 职责：bot 线程生命周期管理、预览帧/钓鱼事件/日志 → 前端事件桥接、
//! 分类器参数共享、keybind.txt 读写（与 C# 版格式互通）。
//! 前端规格：migration/docs/07-ui-replica-spec.md

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::Cursor;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use base64::Engine;
use fishing_core::bite_watcher::PositionBiteWatcher;
use fishing_core::bobber_finder::{highlight_colour, FrameEvent};
use fishing_core::classifier::{ClassifierMode, PixelClassifier};
use fishing_core::events::FishingEvent;
use fishing_core::fish_bot::FishingBot;
use fishing_core::tracking_finder::TrackingBobberFinder;
use fishing_core::traits::{Key, KeySender, ScreenCapture};
use fishing_core::{Point, Rect};
use fishing_platform_windows::{WowKeySender, WowMouseController, WowScreenCapture, LOOT_DELAY_MS};
use image::{ImageFormat, Rgb, RgbImage};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

const KEYBIND_FILE: &str = "keybind.txt";
/// 咬钩判定阈值持久化文件（扩展功能，原版无此配置）
const STRIKE_VALUE_FILE: &str = "strikevalue.txt";
/// 水面搜索区域持久化文件（扩展功能，格式 "x,y,w,h"，截屏位图坐标）
const SEARCH_REGION_FILE: &str = "searchregion.txt";
/// 咬钩判定阈值（像素）：浮漂下沉超过该值才提竿。原版 C# 硬编码 7，
/// 海水起伏大时容易误判，故改为可调（扩展功能，默认保持 7）。
static STRIKE_VALUE: AtomicI32 = AtomicI32::new(7);
/// 预览帧推送上限（C# 版受 GetPixel 性能限制天然低帧率；Rust 截屏快，
/// 限帧避免 IPC 洪泛，见 docs/03-tech-stack.md）
const FRAME_INTERVAL: Duration = Duration::from_millis(66);

struct BotHandle {
    stop: Arc<AtomicBool>,
    cast_key: Arc<Mutex<Key>>,
}

struct AppState {
    /// 共享分类器（对应 C# MainWindow 与 ColourConfiguration 共享同一实例）
    classifier: Arc<RwLock<PixelClassifier>>,
    /// 共享水面搜索区域（扩展功能 F3，None = 全图）
    search_region: Arc<RwLock<Option<Rect>>>,
    bot: Mutex<Option<BotHandle>>,
}

// ---------------------------------------------------------------- 日志桥接

/// tracing → 前端日志列表（对应 C# MainWindow 注册为 log4net IAppender）。
static LOG_TX: OnceLock<Sender<String>> = OnceLock::new();

struct LogWriter;

impl std::io::Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Some(tx) = LOG_TX.get() {
            let line = String::from_utf8_lossy(buf).trim_end().to_string();
            if !line.is_empty() {
                let _ = tx.send(line);
            }
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------- 帧处理

#[derive(Serialize, Clone)]
struct FramePayload {
    png_b64: String,
    bobber: Option<Point>,
    /// 首帧平均色（预览区背景，对应 BitmapExtension.GetBackgroundColourBrush）
    avg_colour: [u8; 3],
}

/// 对应 C# BitmapExtension.GetBackgroundColourBrush。
fn average_colour(img: &RgbImage) -> [u8; 3] {
    let (mut r, mut g, mut b) = (0u64, 0u64, 0u64);
    for p in img.pixels() {
        r += p[0] as u64;
        g += p[1] as u64;
        b += p[2] as u64;
    }
    let n = (img.width() * img.height()).max(1) as u64;
    [(r / n) as u8, (g / n) as u8, (b / n) as u8]
}

/// 对应 C# ReticleDrawer.Draw：浮漂点置白 + ±40 处四角、边长 15、白色线宽 2。
fn draw_reticle(img: &mut RgbImage, point: Point) {
    let white = Rgb([255u8, 255, 255]);
    set_pixel_safe(img, point.x, point.y, white);

    let corner = 15i32;
    let rec = 40i32;
    let corners = [
        (point.x - rec, point.y - rec, corner, corner),
        (point.x - rec, point.y + rec, corner, -corner),
        (point.x + rec, point.y - rec, -corner, corner),
        (point.x + rec, point.y + rec, -corner, -corner),
    ];
    for (cx, cy, dx, dy) in corners {
        draw_thick_line(img, (cx + dx, cy), (cx, cy), white, 2);
        draw_thick_line(img, (cx, cy), (cx, cy + dy), white, 2);
    }
}

fn set_pixel_safe(img: &mut RgbImage, x: i32, y: i32, c: Rgb<u8>) {
    if x >= 0 && y >= 0 && (x as u32) < img.width() && (y as u32) < img.height() {
        img.put_pixel(x as u32, y as u32, c);
    }
}

/// 简单直线 + 线宽（GDI+ 抗锯齿不复刻，视觉效果差异可忽略）
fn draw_thick_line(img: &mut RgbImage, from: (i32, i32), to: (i32, i32), c: Rgb<u8>, w: i32) {
    let (mut x, mut y) = from;
    let (tx, ty) = to;
    let dx = (tx - x).signum();
    let dy = (ty - y).signum();
    loop {
        for ox in -w / 2..=w / 2 {
            for oy in -w / 2..=w / 2 {
                set_pixel_safe(img, x + ox, y + oy, c);
            }
        }
        if (x, y) == (tx, ty) {
            break;
        }
        x += dx;
        y += dy;
    }
}

fn encode_frame(ev: &FrameEvent, mode: ClassifierMode) -> Option<FramePayload> {
    let mut img = ev.frame.image.clone();

    // C# SearchBobberFinder 把命中像素染成纯红/蓝（ProcessPixel）
    let hl = highlight_colour(mode);
    let width = ev.frame.image.width() as i32;
    for (i, &hit) in ev.mask.iter().enumerate() {
        if hit {
            let x = i as i32 % width;
            let y = i as i32 / width;
            set_pixel_safe(&mut img, x, y, Rgb(hl));
        }
    }

    // C# MainWindow：ReticleDrawer 总是绘制，未命中时在 (0,0)（保留该 quirk）
    draw_reticle(&mut img, ev.bobber.unwrap_or(Point::EMPTY));

    let avg = average_colour(&img);
    let mut buf = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut buf, ImageFormat::Png)
        .ok()?;

    Some(FramePayload {
        png_b64: base64::engine::general_purpose::STANDARD.encode(buf.into_inner()),
        bobber: ev.bobber,
        avg_colour: avg,
    })
}

// ---------------------------------------------------------------- 命令

#[tauri::command]
fn start_bot(app: AppHandle, state: State<AppState>) -> Result<(), String> {
    let guard = state.bot.lock().map_err(|e| e.to_string())?;
    if guard.is_some() {
        return Ok(()); // C#: bot == null 检查，运行中重复点击无效
    }
    drop(guard);

    let classifier = state.classifier.clone();
    let search_region = state.search_region.clone();
    let app2 = app.clone();

    // C# Play_Click：先按 Spacebar，sleep 1500，再启动 bot 线程
    std::thread::spawn(move || {
        let mut keys = WowKeySender::new();
        keys.press_key(Key::SPACEBAR);
        std::thread::sleep(Duration::from_millis(1500));

        // 预览帧桥（限帧）；高亮色按分类器当前模式逐帧读取
        let (frame_tx, frame_rx) = channel::<FrameEvent>();
        let frame_app = app2.clone();
        let frame_classifier = classifier.clone();
        std::thread::spawn(move || {
            let mut last = Instant::now() - FRAME_INTERVAL;
            while let Ok(ev) = frame_rx.recv() {
                if last.elapsed() < FRAME_INTERVAL {
                    continue;
                }
                last = Instant::now();
                let mode = frame_classifier
                    .read()
                    .map(|c| c.mode)
                    .unwrap_or(ClassifierMode::Red);
                if let Some(payload) = encode_frame(&ev, mode) {
                    let _ = frame_app.emit("frame", payload);
                }
            }
        });

        // 钓鱼事件桥
        let (ev_tx, ev_rx) = channel::<FishingEvent>();
        let ev_app = app2.clone();
        std::thread::spawn(move || {
            while let Ok(ev) = ev_rx.recv() {
                let _ = ev_app.emit("fishing-event", ev);
            }
        });

        // F3：GUI 版换用锁定跟踪查找器（密度过滤/连续帧确认/丢失容忍/区域限制），
        // console 版仍用 C# 保真的 SearchBobberFinder（docs/08）
        let mut finder =
            TrackingBobberFinder::new_shared(classifier, search_region, WowScreenCapture::new());
        finder.set_frame_handler(frame_tx);

        let keybind = read_keybind_value().unwrap_or(Key::D4);
        let strike = STRIKE_VALUE.load(Ordering::SeqCst);
        // C# GUI 版 ten-min 键固定 [D5, D6]
        let mut bot = FishingBot::new(
            Box::new(finder),
            Box::new(PositionBiteWatcher::new(strike)),
            Box::new(WowKeySender::new()),
            Box::new(WowMouseController::new()),
            keybind,
            vec![Key::D5, Key::D6],
            ev_tx,
        );

        let handle = BotHandle {
            stop: bot.stop_handle(),
            cast_key: bot.cast_key_handle(),
        };
        if let Some(state) = app2.try_state::<AppState>() {
            *state.bot.lock().unwrap() = Some(handle);
        }

        bot.start();

        // bot 退出（对应 C# BotThread 末尾 SetButtonStates(true)）
        if let Some(state) = app2.try_state::<AppState>() {
            *state.bot.lock().unwrap() = None;
        }
        let _ = app2.emit("bot-stopped", ());
    });

    Ok(())
}

#[tauri::command]
fn stop_bot(state: State<AppState>) -> Result<(), String> {
    let guard = state.bot.lock().map_err(|e| e.to_string())?;
    if let Some(h) = guard.as_ref() {
        h.stop.store(false, Ordering::SeqCst);
    }
    Ok(())
}

#[tauri::command]
fn bot_running(state: State<AppState>) -> bool {
    state.bot.lock().map(|b| b.is_some()).unwrap_or(false)
}

/// C# KeyBindChooser：keybind.txt 存 ConsoleKey 整数值，读取失败回退 D4。
fn read_keybind_value() -> Option<Key> {
    let text = std::fs::read_to_string(KEYBIND_FILE).ok()?;
    let v: i32 = text.trim().parse().ok()?;
    Key::from_console_key_value(v)
}

#[tauri::command]
fn get_cast_key(state: State<AppState>) -> String {
    let running = state
        .bot
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|h| *h.cast_key.lock().unwrap()));
    let key = running.or_else(read_keybind_value).unwrap_or(Key::D4);
    key.display_text()
}

/// C# KeyBind_KeyUp：e.Key 转 ConsoleKey（数值==VK 码）成功才保存并通知 bot。
/// 前端直接传 KeyboardEvent.keyCode（WebView2 中即 Win32 VK 码）。
#[tauri::command]
fn set_cast_key(code: i32, state: State<AppState>) -> Result<bool, String> {
    let Some(key) = Key::from_console_key_value(code) else {
        return Ok(false);
    };
    std::fs::write(KEYBIND_FILE, (key.vk_code() as i32).to_string()).map_err(|e| e.to_string())?;
    if let Ok(guard) = state.bot.lock() {
        if let Some(h) = guard.as_ref() {
            *h.cast_key.lock().unwrap() = key;
        }
    }
    Ok(true)
}

#[derive(Serialize)]
struct ClassifierConfig {
    colour_multiplier: f64,
    colour_closeness_multiplier: f64,
    mode: String,
}

#[tauri::command]
fn get_classifier_config(state: State<AppState>) -> Result<ClassifierConfig, String> {
    let c = state.classifier.read().map_err(|e| e.to_string())?;
    Ok(ClassifierConfig {
        colour_multiplier: c.colour_multiplier,
        colour_closeness_multiplier: c.colour_closeness_multiplier,
        mode: format!("{:?}", c.mode),
    })
}

/// C# 颜色配置窗口：滑条拖动中实时写参数（TwoWay 绑定）。
#[tauri::command]
fn set_classifier_params(
    colour_multiplier: f64,
    colour_closeness_multiplier: f64,
    state: State<AppState>,
) -> Result<(), String> {
    let mut c = state.classifier.write().map_err(|e| e.to_string())?;
    c.colour_multiplier = colour_multiplier;
    c.colour_closeness_multiplier = colour_closeness_multiplier;
    Ok(())
}

#[tauri::command]
fn set_classifier_mode(mode: String, state: State<AppState>) -> Result<(), String> {
    let mut c = state.classifier.write().map_err(|e| e.to_string())?;
    c.mode = match mode.as_str() {
        "Blue" => ClassifierMode::Blue,
        _ => ClassifierMode::Red,
    };
    Ok(())
}

#[tauri::command]
fn get_loot_delay() -> u64 {
    LOOT_DELAY_MS.load(Ordering::SeqCst)
}

#[tauri::command]
fn set_loot_delay(ms: u64) {
    LOOT_DELAY_MS.store(ms, Ordering::SeqCst);
}

fn read_strike_value() -> i32 {
    std::fs::read_to_string(STRIKE_VALUE_FILE)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(7)
}

#[tauri::command]
fn get_strike_value() -> i32 {
    STRIKE_VALUE.load(Ordering::SeqCst)
}

#[tauri::command]
fn set_strike_value(v: i32) {
    let v = v.clamp(1, 100);
    STRIKE_VALUE.store(v, Ordering::SeqCst);
    let _ = std::fs::write(STRIKE_VALUE_FILE, v.to_string());
}

/// 颜色配置窗口 Capture Screen（对应 C# WowScreen.GetBitmap）。
#[tauri::command]
fn capture_screen() -> Result<String, String> {
    let mut cap = WowScreenCapture::new();
    let frame = cap.capture();
    let mut buf = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(frame.image)
        .write_to(&mut buf, ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(base64::engine::general_purpose::STANDARD.encode(buf.into_inner()))
}

// ---------------------------------------------------------------- 搜索区域（F3 扩展）

fn read_search_region() -> Option<Rect> {
    let text = std::fs::read_to_string(SEARCH_REGION_FILE).ok()?;
    let parts: Vec<i32> = text
        .trim()
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    if parts.len() == 4 && parts[2] > 0 && parts[3] > 0 {
        Some(Rect::new(parts[0], parts[1], parts[2], parts[3]))
    } else {
        None
    }
}

#[tauri::command]
fn get_search_region(state: State<AppState>) -> Option<Rect> {
    state.search_region.read().ok().and_then(|r| *r)
}

/// 颜色配置窗口拖框结果（截屏位图坐标），持久化到 searchregion.txt。
#[tauri::command]
fn set_search_region(
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    state: State<AppState>,
) -> Result<(), String> {
    if width <= 0 || height <= 0 {
        return Err("区域宽高必须为正".into());
    }
    let rect = Rect::new(x, y, width, height);
    *state.search_region.write().map_err(|e| e.to_string())? = Some(rect);
    std::fs::write(SEARCH_REGION_FILE, format!("{x},{y},{width},{height}"))
        .map_err(|e| e.to_string())?;
    tracing::info!("搜索区域已设置: {x},{y} {width}x{height}");
    Ok(())
}

#[tauri::command]
fn clear_search_region(state: State<AppState>) -> Result<(), String> {
    *state.search_region.write().map_err(|e| e.to_string())? = None;
    let _ = std::fs::remove_file(SEARCH_REGION_FILE);
    tracing::info!("搜索区域已清除（恢复全图搜索）");
    Ok(())
}

/// 创建颜色配置窗口（C# new ColourConfiguration(...).Show()，可多开）。
fn spawn_colour_window(app: &AppHandle) -> Result<(), String> {
    let label = format!(
        "colour-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    );
    WebviewWindowBuilder::new(app, &label, WebviewUrl::App("colour.html".into()))
        .title("颜色配置")
        .inner_size(800.0, 450.0)
        // 用户实际在全屏（无边框）WoW 前使用：置顶+聚焦，避免窗口开到游戏后面
        // 看起来像"打不开"（对 C# 版非置顶行为的有意修正，见 migration/logs）
        .always_on_top(true)
        .focused(true)
        .build()
        .map_err(|e| format!("create colour window '{label}': {e}"))?;
    tracing::info!("colour window '{label}' opened");
    Ok(())
}

/// C# Settings_Click。
/// 注意：Windows 上在同步命令里创建窗口会导致 WebView2 永不加载（白屏），
/// 必须用 async 命令（Tauri 官方已知问题说明）。
#[tauri::command]
async fn open_colour_window(app: AppHandle) -> Result<(), String> {
    spawn_colour_window(&app).inspect_err(|e| tracing::error!("{e}"))
}

// ---------------------------------------------------------------- main

fn main() {
    let (log_tx, log_rx) = channel::<String>();
    let _ = LOG_TX.set(log_tx);
    tracing_subscriber::fmt()
        .with_writer(|| LogWriter)
        .with_ansi(false)
        .init();

    tauri::Builder::default()
        .manage(AppState {
            classifier: Arc::new(RwLock::new(PixelClassifier::new())),
            search_region: Arc::new(RwLock::new(read_search_region())),
            bot: Mutex::new(None),
        })
        .setup(|app| {
            // C# MainWindow 构造：pixelClassifier.SetConfiguration(IsWowClassic())
            {
                let state = app.state::<AppState>();
                let mut c = state.classifier.write().unwrap();
                c.set_configuration(fishing_platform_windows::is_wow_classic());
            }

            // 日志转发到全部窗口
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                while let Ok(line) = log_rx.recv() {
                    let _ = handle.emit("log", line);
                }
            });

            // 读取持久化的咬钩判定阈值
            STRIKE_VALUE.store(read_strike_value(), Ordering::SeqCst);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            start_bot,
            stop_bot,
            bot_running,
            get_cast_key,
            set_cast_key,
            get_classifier_config,
            set_classifier_params,
            set_classifier_mode,
            get_loot_delay,
            set_loot_delay,
            get_strike_value,
            set_strike_value,
            capture_screen,
            open_colour_window,
            get_search_region,
            set_search_region,
            clear_search_region,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
