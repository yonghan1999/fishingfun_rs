//! M11: FishingFun GUI（Tauri 2）— WPF 版一比一复刻的后端。
//! 职责：bot 线程生命周期管理、预览帧/钓鱼事件/日志 → 前端事件桥接、
//! 分类器参数共享、配置持久化与旧版文本配置迁移。
//! 当前界面行为与配置见仓库 docs/ui.md。
//!
//! 运行期错误日志使用短码（S90 工作线程异常、S91 桥接线程异常、C90 配置窗口已打开），
//! 避免二进制携带可识别的英文句串；面向用户的中文操作提示保持可读。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, sync_channel, SyncSender};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use base64::Engine;
use fishing_core::bite_watcher::PositionBiteWatcher;
use fishing_core::bobber_finder::{highlight_colour, FrameEvent};
use fishing_core::classifier::{ClassifierMode, PixelClassifier};
use fishing_core::events::FishingEvent;
use fishing_core::fish_bot::FishingBot;
use fishing_core::humanize::HumanizerConfig;
use fishing_core::tracking_finder::TrackingBobberFinder;
use fishing_core::traits::{InputError, Key, KeySender};
use fishing_core::{Point, Rect};
use fishing_platform_windows::{
    shutdown_system, WowKeySender, WowMouseController, WowScreenCapture, WowTarget, LOOT_DELAY_MS,
};
use image::{ImageFormat, Rgb, RgbImage};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

mod config;
mod session;
use config::ConfigStore;
use session::{Phase, ScheduledAction, Sessions};

struct AppState {
    /// 共享分类器（对应 C# MainWindow 与 ColourConfiguration 共享同一实例）
    classifier: Arc<RwLock<PixelClassifier>>,
    /// 共享水面搜索区域（扩展功能 F3，None = 全图）
    search_region: Arc<RwLock<Option<Rect>>>,
    bot: Sessions,
    config: ConfigStore,
    closing: AtomicBool,
}

// ---------------------------------------------------------------- 日志桥接

/// tracing → 前端日志列表（对应 C# MainWindow 注册为 log4net IAppender）。
static LOG_TX: OnceLock<SyncSender<String>> = OnceLock::new();

struct LogWriter;

impl std::io::Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Some(tx) = LOG_TX.get() {
            let line = String::from_utf8_lossy(buf).trim_end().to_string();
            if !line.is_empty() {
                let _ = tx.try_send(line);
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
    /// 浮漂位图坐标；前端未消费该字段，序列化名用短码避免命名进入二进制。
    #[serde(rename = "p")]
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

fn encode_frame(ev: FrameEvent, mode: ClassifierMode) -> Option<FramePayload> {
    let mut img = ev.frame.image;

    // C# SearchBobberFinder 把命中像素染成纯红/蓝（ProcessPixel）
    let hl = highlight_colour(mode);
    let width = img.width() as i32;
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

#[derive(Serialize, Clone)]
struct BotStatus {
    phase: Phase,
    strike_value: i32,
}

fn status(state: &AppState) -> BotStatus {
    match state.bot.current() {
        Some(session) => BotStatus {
            phase: session.phase(),
            strike_value: session.strike,
        },
        None => BotStatus {
            phase: Phase::Idle,
            strike_value: state.config.get().strike_value,
        },
    }
}

#[tauri::command]
fn bot_status(state: State<AppState>) -> BotStatus {
    status(&state)
}

#[tauri::command]
fn start_bot(app: AppHandle, state: State<AppState>) -> Result<(), String> {
    if state.closing.load(Ordering::Acquire) {
        return Err("应用正在退出".into());
    }
    let target = WowTarget::find().map_err(|e| e.to_string())?;
    let config = state.config.get();
    let key = Key::from_console_key_value(config.cast_key).ok_or("抛竿键无效")?;
    let session = state.bot.reserve(key, config.strike_value)?;
    let config_humanize = config.humanize;
    let macro_keys = config
        .macro_keys
        .into_iter()
        .flatten()
        .filter_map(Key::from_console_key_value)
        .collect();
    let macro_interval_secs = u64::from(config.macro_interval_minutes) * 60;
    let schedule_minutes = config.schedule_minutes;
    let schedule_action = match config.schedule_action.as_str() {
        "close_wow" => ScheduledAction::CloseWow,
        "shutdown" => ScheduledAction::Shutdown,
        _ => ScheduledAction::StopFishing,
    };
    let classifier = state.classifier.clone();
    let search_region = state.search_region.clone();
    let app2 = app.clone();
    let worker_session = session.clone();
    let spawned = std::thread::Builder::new()
        .name("fishing-session".into())
        .spawn(move || {
            let session = worker_session;
            let mut frame_thread = None;
            let mut event_thread = None;
            // 即使线程 panic，仍取消桥接线程并清理本次会话。
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if session.cancel.is_cancelled() {
                    return;
                }
                tracing::info!("请在十秒内切回游戏窗口。");
                let mut keys = WowKeySender::new(target);
                if let Err(error) = target
                    .wait_foreground(&session.cancel)
                    .and_then(|_| keys.press_key_cancellable(Key::SPACEBAR, &session.cancel))
                {
                    if error != InputError::Cancelled {
                        let _ = app2.emit("bot-error", error.to_string());
                    }
                    return;
                }
                if !session.cancel.wait(Duration::from_millis(1500)) || !session.run() {
                    return;
                }
                let _ = app2.emit("bot-status", ());

                if schedule_minutes > 0 {
                    let timer_cancel = session.cancel.clone();
                    let timer_target = target;
                    std::thread::spawn(move || {
                        if timer_cancel.wait(Duration::from_secs(u64::from(schedule_minutes) * 60))
                        {
                            timer_cancel.cancel();
                            match schedule_action {
                                ScheduledAction::StopFishing => {}
                                ScheduledAction::CloseWow => {
                                    if let Err(error) = timer_target.terminate() {
                                        tracing::error!("定时关闭魔兽世界失败: {error}");
                                    }
                                }
                                ScheduledAction::Shutdown => {
                                    if let Err(error) = shutdown_system() {
                                        tracing::error!("定时关机失败: {error}");
                                    }
                                }
                            }
                        }
                    });
                }

                let (frame_tx, frame_rx) = fishing_core::latest::channel::<FrameEvent>();
                let frame_app = app2.clone();
                let frame_classifier = classifier.clone();
                let frame_cancel = session.cancel.clone();
                frame_thread = Some(std::thread::spawn(move || {
                    while let Some(ev) = frame_rx.recv() {
                        if frame_cancel.is_cancelled() {
                            break;
                        }
                        let mode = frame_classifier
                            .read()
                            .map(|c| c.mode)
                            .unwrap_or(ClassifierMode::Red);
                        if let Some(payload) = encode_frame(ev, mode) {
                            if frame_cancel.is_cancelled() {
                                break;
                            }
                            let _ = frame_app.emit("frame", payload);
                        }
                    }
                }));
                let (ev_tx, ev_rx) = channel::<FishingEvent>();
                let ev_app = app2.clone();
                let event_cancel = session.cancel.clone();
                event_thread = Some(std::thread::spawn(move || {
                    while let Ok(ev) = ev_rx.recv() {
                        if event_cancel.is_cancelled() {
                            break;
                        }
                        let _ = ev_app.emit("session-event", ev);
                    }
                }));
                let mut finder = TrackingBobberFinder::new_shared(
                    classifier,
                    search_region,
                    WowScreenCapture::new(),
                );
                finder.set_frame_handler(frame_tx);
                let mut humanize_cfg = if config_humanize {
                    HumanizerConfig::default()
                } else {
                    HumanizerConfig::disabled()
                };
                humanize_cfg.macro_interval_secs = macro_interval_secs;
                let mut bot = FishingBot::new(
                    Box::new(finder),
                    Box::new(PositionBiteWatcher::new(session.strike)),
                    Box::new(WowKeySender::new(target)),
                    Box::new(WowMouseController::new(target)),
                    key,
                    macro_keys,
                    ev_tx,
                )
                .with_session(session.cancel.clone(), session.cast_key.clone())
                .with_humanizer(humanize_cfg);
                bot.start();
                if let Some(error) = bot.last_error() {
                    let _ = app2.emit("bot-error", error.to_string());
                }
                session.cancel.cancel();
                drop(bot);
            }));
            session.cancel.cancel();
            // panic 展开同样会释放生产端；标记会话结束前必须回收桥接线程。
            for worker in [frame_thread, event_thread].into_iter().flatten() {
                if worker.join().is_err() {
                    tracing::error!("S91");
                }
            }
            if result.is_err() {
                tracing::error!("S90");
                let _ = app2.emit("bot-error", "机器人异常退出，请查看日志");
            }
            if let Some(state) = app2.try_state::<AppState>() {
                // 在会话锁仍占位时发送退出事件，防止新会话先启动后收到旧退出通知。
                state.bot.finish_with(&session, || {
                    let _ = app2.emit("bot-stopped", ());
                });
            }
        });
    if let Err(error) = spawned {
        session.cancel.cancel();
        state.bot.finish(&session);
        return Err(error.to_string());
    }
    Ok(())
}

#[tauri::command]
fn stop_bot(state: State<AppState>) {
    if let Some(session) = state.bot.current() {
        session.stop();
    }
}

#[tauri::command]
fn bot_running(state: State<AppState>) -> bool {
    state.bot.current().is_some()
}

#[tauri::command]
fn get_cast_key(state: State<AppState>) -> String {
    let key = state
        .bot
        .current()
        .map(|s| *s.cast_key.lock().unwrap_or_else(|e| e.into_inner()))
        .or_else(|| Key::from_console_key_value(state.config.get().cast_key))
        .unwrap_or(Key::D4);
    key.display_text()
}

#[tauri::command]
fn set_cast_key(code: i32, state: State<AppState>) -> Result<bool, String> {
    let Some(key) = Key::from_console_key_value(code) else {
        return Ok(false);
    };
    state.config.update(|c| c.cast_key = code)?;
    if let Some(session) = state.bot.current() {
        *session.cast_key.lock().map_err(|e| e.to_string())? = key;
    }
    Ok(true)
}

#[derive(Serialize)]
struct MacroSettings {
    keys: [Option<i32>; 2],
    labels: [String; 2],
    minutes: u32,
}

fn macro_settings(settings: config::Settings) -> MacroSettings {
    MacroSettings {
        keys: settings.macro_keys,
        labels: settings.macro_keys.map(|code| {
            code.and_then(Key::from_console_key_value)
                .and_then(Key::console_key_name)
                .unwrap_or("未设置")
                .to_string()
        }),
        minutes: settings.macro_interval_minutes,
    }
}

#[tauri::command]
fn get_macro_settings(state: State<AppState>) -> MacroSettings {
    macro_settings(state.config.get())
}

#[tauri::command]
fn set_macro_settings(
    keys: [Option<i32>; 2],
    minutes: u32,
    state: State<AppState>,
) -> Result<MacroSettings, String> {
    config::validate_macro(keys, minutes)?;
    state.config.update(|c| {
        c.macro_keys = keys;
        c.macro_interval_minutes = minutes;
    })?;
    Ok(macro_settings(state.config.get()))
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
    if !colour_multiplier.is_finite()
        || !(0.0..=3.0).contains(&colour_multiplier)
        || !colour_closeness_multiplier.is_finite()
        || !(0.0..=5.0).contains(&colour_closeness_multiplier)
    {
        return Err("颜色乘数必须在 0..3、接近度乘数必须在 0..5 之间".into());
    }
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
        "Red" => ClassifierMode::Red,
        _ => return Err("未知的羽毛颜色模式".into()),
    };
    Ok(())
}

#[tauri::command]
fn get_loot_delay() -> u64 {
    LOOT_DELAY_MS.load(Ordering::SeqCst)
}

#[tauri::command]
fn set_loot_delay(ms: u64) -> Result<(), String> {
    if ms > 3500 {
        return Err("拾取延迟必须在 0..3500ms 之间".into());
    }
    LOOT_DELAY_MS.store(ms, Ordering::SeqCst);
    Ok(())
}

#[tauri::command]
fn get_strike_value(state: State<AppState>) -> i32 {
    state.config.get().strike_value
}

/// 拟人化节奏开关（扩展功能，下次启动机器人生效）。
#[tauri::command]
fn get_humanize(state: State<AppState>) -> bool {
    state.config.get().humanize
}

#[tauri::command]
fn set_humanize(on: bool, state: State<AppState>) -> Result<(), String> {
    state.config.update(|c| c.humanize = on)?;
    tracing::info!("拟人化节奏: {}", if on { "开启" } else { "关闭" });
    Ok(())
}

#[derive(Serialize, Clone)]
struct ScheduleSettings {
    minutes: u32,
    action: String,
}

#[tauri::command]
fn get_schedule(state: State<AppState>) -> ScheduleSettings {
    let settings = state.config.get();
    ScheduleSettings {
        minutes: settings.schedule_minutes,
        action: settings.schedule_action,
    }
}

#[tauri::command]
fn set_schedule(minutes: u32, action: String, state: State<AppState>) -> Result<(), String> {
    if minutes > 7 * 24 * 60 {
        return Err("定时分钟数必须在 0..10080 之间".into());
    }
    if !matches!(action.as_str(), "stop_fishing" | "close_wow" | "shutdown") {
        return Err("定时动作无效".into());
    }
    state.config.update(|c| {
        c.schedule_minutes = minutes;
        c.schedule_action = action;
    })
}

#[tauri::command]
fn set_strike_value(v: i32, app: AppHandle, state: State<AppState>) -> Result<(), String> {
    config::validate_strike(v)?;
    state.config.update(|c| c.strike_value = v)?;
    let _ = app.emit("bot-status", ());
    Ok(())
}

/// 颜色配置窗口 Capture Screen（对应 C# WowScreen.GetBitmap）。
#[tauri::command]
fn capture_screen() -> Result<String, String> {
    let frame = WowScreenCapture::try_capture().map_err(|e| e.to_string())?;
    let mut buf = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(frame.image)
        .write_to(&mut buf, ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(base64::engine::general_purpose::STANDARD.encode(buf.into_inner()))
}

// ---------------------------------------------------------------- 搜索区域（F3 扩展）

#[tauri::command]
fn get_search_region(state: State<AppState>) -> Option<Rect> {
    state.search_region.read().ok().and_then(|r| *r)
}

/// 颜色配置窗口拖框结果（截屏位图坐标），保存成功后才更新共享区域。
#[tauri::command]
fn set_search_region(
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    state: State<AppState>,
) -> Result<(), String> {
    let rect = Rect::new(x, y, width, height);
    config::validate_region(rect)?;
    let mut region = state.search_region.write().map_err(|e| e.to_string())?;
    state.config.update(|c| c.search_region = Some(rect))?;
    *region = Some(rect);
    tracing::info!("搜索区域已设置: {x},{y} {width}x{height}");
    Ok(())
}

#[tauri::command]
fn clear_search_region(state: State<AppState>) -> Result<(), String> {
    let mut region = state.search_region.write().map_err(|e| e.to_string())?;
    state.config.update(|c| c.search_region = None)?;
    *region = None;
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
        // 看起来像"打不开"（对 C# 版非置顶行为的有意修正，见仓库 docs/ui.md）
        .always_on_top(true)
        .focused(true)
        .build()
        .map_err(|e| format!("C91 {label}: {e}"))?;
    tracing::info!("C90");
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
    let (log_tx, log_rx) = sync_channel::<String>(512);
    let _ = LOG_TX.set(log_tx);
    tracing_subscriber::fmt()
        .with_writer(|| LogWriter)
        .with_ansi(false)
        .init();

    tauri::Builder::default()
        .on_window_event(|window, event| {
            if window.label() == "main"
                && matches!(event, tauri::WindowEvent::CloseRequested { .. })
            {
                if let Some(session) = window.state::<AppState>().bot.current() {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                    }
                    session.stop();
                    let state = window.state::<AppState>();
                    if !state.closing.swap(true, Ordering::AcqRel) {
                        let app = window.app_handle().clone();
                        std::thread::spawn(move || {
                            if session.wait_finished(Duration::from_secs(10)) {
                                app.exit(0);
                            } else {
                                app.state::<AppState>()
                                    .closing
                                    .store(false, Ordering::Release);
                                let _ = app.emit(
                                    "bot-error",
                                    "停止清理超过十秒，应用暂未退出，请稍后重试关闭",
                                );
                            }
                        });
                    }
                }
            }
        })
        .setup(|app| {
            let directory = app.path().app_config_dir()?;
            let config = ConfigStore::load(&directory, &std::env::current_dir()?)
                .map_err(std::io::Error::other)?;
            let settings = config.get();
            app.manage(AppState {
                classifier: Arc::new(RwLock::new(PixelClassifier::new())),
                search_region: Arc::new(RwLock::new(settings.search_region)),
                bot: Sessions::default(),
                config,
                closing: AtomicBool::new(false),
            });
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

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            start_bot,
            stop_bot,
            bot_running,
            bot_status,
            get_cast_key,
            set_cast_key,
            get_macro_settings,
            set_macro_settings,
            get_classifier_config,
            set_classifier_params,
            set_classifier_mode,
            get_loot_delay,
            set_loot_delay,
            get_strike_value,
            set_strike_value,
            get_humanize,
            set_humanize,
            get_schedule,
            set_schedule,
            capture_screen,
            open_colour_window,
            get_search_region,
            set_search_region,
            clear_search_region,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
