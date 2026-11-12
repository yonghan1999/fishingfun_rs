# FishingFun Rust

FishingFun 的 Rust 迁移版本，面向 Windows，通过屏幕颜色识别浮漂、监测浮漂下沉并发送键鼠操作。提供 Tauri 图形界面、命令行入口和独立的输入来源观察工具。

## 环境准备

- Windows 10/11 x64，交互式桌面会话。
- [Rustup](https://rustup.rs/)；仓库的 `rust-toolchain.toml` 固定 Rust 1.99.1，包含 rustfmt 和 Clippy。
- [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) 的“使用 C++ 的桌面开发”工作负载，以及 Windows SDK；使用 MSVC 目标 `x86_64-pc-windows-msvc`。
- GUI 运行需要 [Microsoft Edge WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/)。
- Node.js 24：仅用于前端 JavaScript 语法检查。GUI 是静态 HTML/CSS/JS，无需 npm、前端打包器或 Tauri CLI。

首次进入仓库后执行 `rustup show active-toolchain`，Rustup 会按仓库声明安装工具链。默认的旧版 stable 不会覆盖仓库工具链。

## 构建与运行

以下命令均在仓库根目录的 PowerShell 中执行：

```powershell
cargo build --workspace --locked
cargo run -p fishing-gui --locked
```

GUI 可配置抛竿按键、羽毛颜色、颜色分类参数、拾取延迟和咬钩阈值；在颜色配置窗口点击截图可取色，拖动截图可指定水面搜索区域。产品名和主窗口标题沿用当前配置中的 `Google Chrome`。

控制台先查看参数，再按需启动：

```powershell
cargo run -p fishing-console --locked -- --help
cargo run -p fishing-console --locked -- --blue --cast-key D4 --ten-min-keys D5,D6 --strike-value 7
```

控制台默认抛竿键为 D4、十分钟宏键为 D5、下沉阈值为 7 像素。GUI 的周期宏默认按 D5、D6，周期为 10 分钟。主窗口“周期宏”区域中点击宏键输入框并按下目标键即可修改，支持数字、字母、F1 等单个按键；可单独清空，两个都清空则不发送宏键。填写 1..10080 的整数分钟数并点击“保存宏设置”，下次启动机器人生效。启动机器人会先发送空格，再依次执行两个已配置宏键；以后在周期到期后执行。应先核对游戏内按键绑定。

点击开始后，请在十秒内切回游戏窗口；等待期间可以停止。目标失焦、关闭或系统拒绝输入时，本次会话停止并显示错误，需要重新开始。拾取动画表示右键操作已执行，不代表已确认获得物品。

拟人化节奏默认开启：咬钩后随机延迟 150..450ms 再拾取、每轮抛竿之间停顿 0.8..2.5s、每 25..45 分钟休息 1..4 分钟、十分钟宏键触发窗口加 0..=90s 抖动，拾取时光标沿缓出曲线移动并带 ±3px 落点偏移。控制台用 `--no-humanize` 恢复原版固定节拍；GUI 在主窗口工具栏勾选"拟人化"开关（下次启动机器人生效）。

GUI 工具栏支持定时动作：填写分钟数（0 表示关闭定时），选择“停止钓鱼”“关闭魔兽世界”或“关闭电脑”，点击“保存”后下次启动会话生效。定时关闭魔兽世界只结束本次会话绑定的 WoW 进程；定时关机调用 Windows 系统关机命令。

关闭拟人化也关闭鼠标曲线与落点偏移。实际点击耗时还包括鼠标移动、固定 200ms 等待和可配置的拾取延迟（默认 2000ms），反应延迟不是完整拾取耗时。

输入来源工具是独立的 Win32 窗口，显示收到的输入设备类型和来源：

```powershell
cargo run -p input-source --locked
```

发布构建：

```powershell
cargo build --workspace --release --locked
```

默认产物在 `target/debug/` 或 `target/release/`，分别为 `fishing-gui.exe`、`fishing-console.exe`、`input-source.exe`。支持 Cargo 标准的 `CARGO_TARGET_DIR`。目前未启用安装包生成（`bundle.active = false`）。

GUI 的产品名和窗口标题是 `Google Chrome`；用 `& "./package.ps1"` 构建时会额外把 GUI 复制为 `Google Chrome.exe`，运行副本可使进程名与标题一致（Rust 目标名不允许空格，因此不能在 Cargo.toml 直接命名）。

## 配置与使用边界

GUI 将配置原子保存到 Tauri 固定配置目录，Windows 通常为 `%APPDATA%/com.chrome.viewer/settings.json`，不受启动工作目录影响。早期版本使用 `com.fishingfun.app` 标识；为避免在程序中保留旧标识字符串，应用不做自动迁移，需要沿用旧设置时可手动把旧目录的 `settings.json` 复制到新目录。

| JSON 字段 | 内容 | 默认值 |
| --- | --- | --- |
| `cast_key` | ConsoleKey/Win32 VK 整数 | 52（D4） |
| `strike_value` | 1..100 下沉阈值整数 | 7 |
| `search_region` | `{x,y,width,height}`，截图位图坐标 | null（全图搜索） |
| `humanize` | 拟人化节奏开关（下次启动生效） | true |
| `macro_keys` | 两个周期宏键的 VK 整数，null 表示跳过该键 | `[53,54]`（D5、D6） |
| `macro_interval_minutes` | 周期宏基础间隔，1..10080 分钟 | 10 |
| `schedule_minutes` | 定时动作延迟分钟数，0 表示关闭 | 0 |
| `schedule_action` | `stop_fishing`、`close_wow` 或 `shutdown` | `stop_fishing` |

首次创建 JSON 时，从当时工作目录的 `keybind.txt`、`strikevalue.txt`、`searchregion.txt` 导入合法值，旧文件保留。已有 JSON 后不再重复导入；JSON 损坏会报告错误，不静默覆盖。保存成功后设置才生效，运行中保存失败会在窗口底部显示。

抛竿键、颜色参数、拾取延迟及搜索区域可在运行中更新。咬钩阈值在下次启动机器人时生效；颜色参数和拾取延迟当前不跨应用重启保存。控制台使用命令行参数，不读取这些 GUI 配置文件。

截屏固定取主显示器中央区域：原点 `(W/4,H/4)`，宽 `W/2`，高 `H/2-100`；并非按游戏窗口捕获。游戏水面和浮漂需要在此范围内可见。锁定浮漂后的非预览帧只截取跟踪小窗（±80px），减少读屏量。进程名匹配 `wow`、`wowclassic`、`wow-64`。窗口位置、多显示器、DPI 和权限边界见[开发与限制说明](docs/development.md)。

键鼠输入通过 SendInput 进入系统输入流。每次会话绑定目标 PID/HWND，在新按下前检查前台归属，拾取还检查实际落点；检查与系统输入不是原子操作，无法保证检查后瞬间切屏时绝不误入其他窗口。已按下的键鼠会尝试配对释放。关闭主窗口时等待会话清理，超过十秒保留窗口并提示。

## 验证

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --workspace --locked
node --test "tests/ui.test.cjs"
Get-ChildItem -LiteralPath "crates/gui/ui" -Filter "*.js" | ForEach-Object { node --check "$($_.FullName)" }
```

GUI 冒烟脚本只启动界面，不点击“开始”，因此不会主动执行钓鱼流程：

```powershell
& "./smoke-gui.ps1"
& "./smoke-shot.ps1"
& "./smoke-gui.ps1" -Profile release
```

截图默认写入仓库根目录的 `gui-smoke.png`。可用 `-ExePath` 指定现有程序、`-TimeoutSeconds` 指定窗口等待时间，截图脚本可用 `-OutputPath` 指定已有目录内的输出文件。默认可执行文件路径通过 `cargo metadata` 获取，支持自定义构建目录；脚本退出时回收其启动的进程。检查失败会抛出错误，作为独立 PowerShell 进程运行时返回非零退出码。

Windows CI 对推送和 PR 执行格式、Clippy、工作区测试、完整构建及脚本语法检查。需要真实桌面的截图/输入测试默认忽略，手工验证方法及测试覆盖边界见[开发说明](docs/development.md)。

## 代码导航

| 目录 | 职责 |
| --- | --- |
| `crates/core` | 平台无关的颜色分类、浮漂查找、咬钩判断和钓鱼流程 |
| `crates/platform-windows` | GDI 截屏、进程/窗口查找、键鼠操作 |
| `crates/gui` | Tauri 2 后端、静态 Web 前端及资源 |
| `crates/console` | Clap 命令行入口与 DPI manifest |
| `crates/input-source` | 输入设备和输入来源观察工具 |

- [架构与数据流](docs/architecture.md)
- [开发、验证及已知限制](docs/development.md)
- [GUI 行为与配置](docs/ui.md)
- [浮漂锁定跟踪算法](docs/tracking.md)

源码保留了 C# 迁移模块编号和行为对照注释；以上文档根据当前 Rust 实现整理，不代表已恢复原项目的迁移规格或历史记录。
