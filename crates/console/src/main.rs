//! M10: 迁移自 FishingFunConsole/Program.cs。
//! 保真要点：strikeValue=7、cast 键 D4、ten-min 键 [D5]、启动先按 Spacebar 并
//! sleep 1500ms、bot 事件逐条记日志。

use std::sync::mpsc::channel;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use fishing_core::bite_watcher::PositionBiteWatcher;
use fishing_core::bobber_finder::SearchBobberFinder;
use fishing_core::classifier::{ClassifierMode, PixelClassifier};
use fishing_core::fish_bot::FishingBot;
use fishing_core::traits::Key;
use fishing_core::traits::KeySender;
use fishing_platform_windows::{
    is_wow_classic, WowKeySender, WowMouseController, WowScreenCapture,
};

/// FishingFun console（对应 C# Console 版；原发布名为 powershell.exe 属伪装 quirk）。
#[derive(Parser, Debug)]
#[command(
    name = "fishing-console",
    about = "FishingFun WoW 钓鱼机器人（控制台版）"
)]
struct Args {
    /// 使用蓝色羽毛模式（对应 C# args.Contains("blue")）
    #[arg(long)]
    blue: bool,

    /// 抛竿键（ConsoleKey 名，如 D4、F1、Spacebar）
    #[arg(long, default_value = "D4")]
    cast_key: String,

    /// 10 分钟宏键，逗号分隔（C# 版固定 [D5]）
    #[arg(long, default_value = "D5", value_delimiter = ',')]
    ten_min_keys: Vec<String>,

    /// 咬钩判定阈值（浮漂下沉像素数）
    #[arg(long, default_value_t = 7, value_parser = clap::value_parser!(i32).range(1..=100))]
    strike_value: i32,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();

    let mut pixel_classifier = PixelClassifier::new();
    if args.blue {
        println!("Blue mode");
        pixel_classifier.mode = ClassifierMode::Blue;
    }

    pixel_classifier.set_configuration(is_wow_classic());

    let bobber_finder = SearchBobberFinder::new(pixel_classifier, WowScreenCapture::new());
    let bite_watcher = PositionBiteWatcher::new(args.strike_value);

    let cast_key = parse_key(&args.cast_key)?;
    let ten_min_keys = args
        .ten_min_keys
        .iter()
        .map(|s| parse_key(s))
        .collect::<Result<Vec<_>>>()?;

    let (tx, rx) = channel();
    // C#: bot.FishingEventHandler += (b, e) => LogManager.GetLogger("Fishbot").Info(e)
    let log_thread = thread::spawn(move || {
        while let Ok(ev) = rx.recv() {
            tracing::info!("{ev}");
        }
    });

    let mut bot = FishingBot::new(
        Box::new(bobber_finder),
        Box::new(bite_watcher),
        Box::new(WowKeySender::new()),
        Box::new(WowMouseController::new()),
        cast_key,
        ten_min_keys,
        tx,
    );

    // C#: WowProcess.PressKey(ConsoleKey.Spacebar); Thread.Sleep(1500);
    let mut keys = WowKeySender::new();
    keys.press_key(Key::SPACEBAR);
    thread::sleep(Duration::from_millis(1500));

    bot.start();

    drop(log_thread);
    Ok(())
}

fn parse_key(name: &str) -> Result<Key> {
    Key::from_console_key_name(name).ok_or_else(|| {
        anyhow::anyhow!("无法识别的按键名: {name}（需为 ConsoleKey 枚举名，如 D4/F1/Spacebar）")
    })
}
