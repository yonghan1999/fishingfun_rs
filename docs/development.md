# 开发、验证与已知限制

## 工具链与依赖

环境安装和基本命令见[README](../README.md)。`rust-toolchain.toml` 固定 Rust 1.99.1，所有 crate 从 workspace 继承 `rust-version` 和现有 MIT 许可元数据。原仓库地址保留在 workspace 中。

提交 `Cargo.lock`，日常检查使用 `--locked`，避免构建时隐式更新版本。首次构建需要下载依赖；本机已有完整缓存时可以追加 `--offline`。不要用 `--ignore-rust-version` 绕过工具链约束：锁定的 image 版本需要较新 Rust，平台代码也使用了较新的标准库 API。

前端直接由 Tauri 的 `frontendDist = ./ui` 嵌入，修改页面后重新 `cargo run -p fishing-gui --locked` 即可。控制台的 `build.rs` 注入 DPI manifest；GUI 使用 Tauri/tao 的 DPI 配置，避免叠加同名 manifest 资源。

## 自动检查

本地运行与 `.github/workflows/ci.yml` 相同的检查：

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --workspace --locked
node --test "tests/ui.test.cjs"
```

Clippy 警告视为错误。完整 `cargo build` 会实际链接三个程序；`cargo check` 只能验证类型与部分构建步骤，不能替代链接检查。CI 使用 Windows runner，Node 24 检查 JavaScript 语法，PowerShell Parser 检查根目录 `.ps1` 语法。

## 测试覆盖

`core` 测试覆盖颜色公式、普通/增强查找、坐标转换、掩码、遮挡/跳变、区域限制、下沉判断、定时器和宏键，并新增取消过程、最新帧覆盖、框外干扰和局部密度对拍。GUI 模块测试覆盖会话独占、启动阶段取消、配置迁移、严格解析及写入失败不改变内存/磁盘。前端测试覆盖实际阈值、日志容量、启动失败恢复和保存错误提示。

覆盖边界：核心测试没有执行真实游戏交互。`find_bobber_returns_none_when_never_found` 只检查 mock 查找失败及没有点击，不证明完整 5 秒超时；取消路径和会话状态已有独立回归，但不等同于全链路长时间运行或完整 25 秒等待测试。

`platform-windows` 的截图、GDI 回收和记事本测试需要桌面环境，默认 `#[ignore]`：

```powershell
# 只截屏；应在正常显示内容的桌面执行。
cargo test -p fishing-platform-windows --locked captures_primary_screen_centre -- --ignored

# 成功截屏及 BitBlt 失败后，GDI 资源数量应回到原值。
cargo test -p fishing-platform-windows --locked capture_releases_gdi_on_success_and_failure -- --ignored --test-threads=1

# 手动打开记事本后验证进程/窗口查找。
cargo test -p fishing-platform-windows --locked finds_notepad_process_and_window -- --ignored

```

不在 CI 中自动运行依赖桌面和人工观察的测试。

## GUI 冒烟与人工检查

先构建 GUI，再执行 `smoke-gui.ps1` 检查窗口存活，或执行 `smoke-shot.ps1` 获取 PNG。脚本可以从其他工作目录调用，始终以仓库根目录为程序工作目录；默认从 Cargo metadata 解析构建目录，也可通过 `-ExePath` 指定外部路径。

脚本只启动和回收自己的实例，异常时也释放截图资源和进程。它检查窗口句柄及短时间存活，不通过调用机器人来验证界面。PNG 成功写入不能证明 WebView2 已完整渲染；需要检查截图是否白屏、控件是否出现。

人工验证以下行为：

1. 主窗口出现预览、按钮、曲线和日志卡片；调整窗口大小后布局可用。
2. 打开颜色配置窗口，确认显示内容、截图取色、拖动区域和清除区域正常。
3. 修改抛竿键、下沉阈值、区域，关闭重开后检查持久化；更换启动目录后仍读取同一 `settings.json`。
4. 在合适的测试环境核对按键绑定，再验证开始、停止、曲线、拾取以及重新开始。此步骤会发送输入，不能由界面存活测试代替。

## 已知限制

- **停止边界**：启动延迟、识别循环、拾取等待及按键保持时间可取消；会话截止时刻共享到平台等待。正在进行的系统调用不会被强行中断，已按下的键鼠需配对释放，因此不保证硬实时停止。关闭主窗口等待清理，十秒超时保留窗口并提示。
- **捕获范围**：固定截取主显示器中央区域，不随游戏窗口移动；独占全屏、遮挡、切屏、显示器和 DPI 改动可能影响图像与坐标。需要在实际环境核验。
- **输入与权限**：键盘和鼠标均通过 SendInput 进入系统输入流。会话绑定目标 PID/HWND，开始时最多等待 60 秒切回目标，每 100ms 检查前台归属；GUI 每秒显示剩余等待时间。后续前台切到其他进程或输入失败即停止并提示。新按下前检查、已按下的键鼠配对释放；查询与注入不是原子操作，检查后瞬间切屏仍有竞态。系统权限隔离（UIPI）可能拦截输入，错误向上传递。进程匹配取遇到的第一个候选，不提供多开实例选择。
- **参数与显示**：咬钩阈值在下次启动机器人时更新，当前会话的参考线保持实际旧值；空闲时显示下一次启动值。颜色参数/拾取延迟暂不持久化。
- **配置恢复**：首次启动从当时工作目录迁移旧文本文件，之后使用固定应用目录。损坏 JSON 会阻止启动并报告错误，需用户修正或备份后移除；不会静默覆盖。
- **长时间运行**：帧槽和日志有界，重复颜色/截屏错误限频。饱和时旧预览被替换，日志可能丢弃；完整游戏环境仍需长时间测量。GDI 回收测试只覆盖当前构建与桌面环境。
- **迁移差异**：不恢复 C# 的 log4net flush 线程，日志统一由 tracing 输出。没有原迁移规格和历史日志，代码中的迁移编号仅作为溯源线索。
- **分发**：当前只验证源码构建和本地可执行程序，未启用安装包，也没有签名、自动更新或发布流水线。

## 日志短码与编译产物去特征

C# 原版的日志原文（`FishBot Created.`、`Bobber start position:` 等）属于公开样本中的明文字符串，直接保留会让编译产物带上可识别特征，因此运行日志统一改为短码，对照表写在对应模块的顶部注释里（注释不进入二进制）：

| 位置 | 短码 |
| --- | --- |
| `core/src/fish_bot.rs` | S00–S12：会话生命周期、抛竿、拾取、等待 |
| `core/src/classifier.rs` | C00/C01：配色参数方案 |
| `core/src/bobber_finder.rs`、`tracking_finder.rs` | E01/E02：命中像素过多 |
| `platform-windows/src/screen.rs`、`wow_process.rs` | P01–P04：截屏与输入失败 |
| `gui/src/main.rs` | S90/S91、C90/C91：工作线程与配置窗口 |

新增日志时沿用短码，不要写入可读英文句串；面向用户的中文操作提示保留可读文本。

事件协议同样使用短码：`FishingAction` 的 serde 名为 `M`/`R`/`L`/`C`，Tauri 事件名为 `session-event`，前端在 `ui/app.js` 中对应处理。修改枚举时保持三处同步。

编译期配置：

- `.cargo/config.toml` 用 `--remap-path-prefix` 重映射本机绝对路径。**仓库移动目录、更换 Windows 用户名或自定义 `CARGO_HOME`/`RUSTUP_HOME` 后必须同步更新前缀**，否则路径会重新进入二进制。
- `[profile.release]` 设置 `strip = "symbols"`。**不要加 `panic = "abort"`**：GUI 依赖 `catch_unwind` 隔离 bot 工作线程，abort 会使该保护失效。
- `--remap-path-prefix` 只作用于 rustc 的源文件路径，对 `env!("CARGO_MANIFEST_DIR")` 展开无效。GUI 产物中仍会嵌入 GUI crate 的清单绝对路径（Tauri 用于前端资源定位）；发布构建若需消除，应在中性路径下检出源码再构建。

仍保留的命名特征：crate 名（`fishing-gui`、`fishing-core`、`fishing-platform-windows`）、模块名（`wow_process`）与部分源文件名（`bobber_finder.rs`）会进入二进制，需要重命名 crate/模块/文件才能消除，本次未改动。`humanize` 是配置字段与命令名，属通用词汇且涉及配置兼容，予以保留。

验证方式：构建后用字符串提取工具扫描产物，对比特征词命中数（本仓库曾用 Python 正则 `[\x20-\x7e]{6,}` 提取并检索 `fish`/`bobber`/`Cast`/`Loot` 等关键词）。
