/* 主窗口逻辑 — MainWindow.xaml.cs 复刻（当前行为见仓库 docs/ui.md） */

const invoke = window.invokeWithFeedback;
const { listen } = window.__TAURI__.event;

const el = id => document.getElementById(id);
const screenshot = el("screenshot");
const previewArea = el("previewArea");
const lootingOverlay = el("lootingOverlay");
const logList = el("logList");

let chart, fish;
let setImageBackgroundColour = true; // C# 同名标记：Cast 后下一帧重取背景色
let logIndex = 0;
let hideCardsTimer = null;

/* ---------------- 按钮 ---------------- */

el("btnPlay").addEventListener("click", async () => {
  setButtonStates(false);
  try { await invoke("start_bot"); }
  finally { await syncStatus(); }
  // C#：10 分钟后自动隐藏两张卡（翻到背面）
  if (hideCardsTimer) clearTimeout(hideCardsTimer);
  hideCardsTimer = setTimeout(() => {
    el("graphFlipper").classList.add("flipped");
    el("logFlipper").classList.add("flipped");
  }, 10 * 60 * 1000);
});

el("btnStop").addEventListener("click", async () => {
  el("btnStop").disabled = true;
  try { await invoke("stop_bot"); }
  finally { await syncStatus(); }
});
el("btnSettings").addEventListener("click", () => invoke("open_colour_window"));
el("btnCastKey").addEventListener("click", () => el("castKeyInput").focus());

/* C# SetButtonStates */
function setButtonStates(playEnabled) {
  el("btnPlay").disabled = !playEnabled;
  el("btnStop").disabled = playEnabled;
  el("playImg").classList.toggle("hidden", !playEnabled);
  el("playImgDisabled").classList.toggle("hidden", playEnabled);
  el("stopImg").classList.toggle("hidden", playEnabled);
  el("stopImgDisabled").classList.toggle("hidden", !playEnabled);
}

/* bot 线程退出（对应 C# BotThread 末尾 SetButtonStates(true)） */
function applyStatus(status) {
  setButtonStates(status.phase === "Idle");
  if (status.phase === "Stopping") el("btnStop").disabled = true;
  if (chart) chart.setStrikeValue(status.strike_value);
}

async function syncStatus() { applyStatus(await invoke("bot_status")); }
listen("bot-status", () => syncStatus());
listen("bot-stopped", () => {
  if (hideCardsTimer) clearTimeout(hideCardsTimer);
  hideCardsTimer = null;
  lootingOverlay.classList.add("hidden");
  if (fish) fish.stop();
  syncStatus();
});

/* ---------------- Flipper ---------------- */

document.querySelectorAll("[data-flip]").forEach(btn => {
  btn.addEventListener("click", () => {
    el(btn.dataset.flip).classList.toggle("flipped");
  });
});

/* ---------------- 预览帧 ---------------- */

listen("frame", ev => {
  const p = ev.payload;
  screenshot.src = "data:image/png;base64," + p.png_b64;
  // C# SetBackgroundImageColour：仅每次 Cast 后的首帧设置一次
  if (setImageBackgroundColour) {
    setImageBackgroundColour = false;
    const [r, g, b] = p.avg_colour;
    previewArea.style.background = `rgb(${r},${g},${b})`;
  }
});

/* ---------------- 钓鱼事件（见仓库 docs/architecture.md） ---------------- */

listen("fishing-event", ev => {
  const { action, amplitude } = ev.payload;
  switch (action) {
    case "BobberMove":
      // C#：仅 Flipper 正面时加点
      if (!el("graphFlipper").classList.contains("flipped")) {
        chart.add(amplitude);
      }
      break;
    case "Loot":
      fish.start(previewArea.clientWidth, previewArea.clientHeight);
      lootingOverlay.classList.remove("hidden");
      break;
    case "Cast":
      chart.clearChart();
      lootingOverlay.classList.add("hidden");
      fish.stop();
      setImageBackgroundColour = true;
      break;
    case "Reset":
      break; // 原版 UI 无直接反应
  }
});

/* ---------------- 日志（Insert(0) 顶部插入，三列） ---------------- */

listen("log", ev => {
  const row = document.createElement("div");
  row.className = "log-row";
  const time = document.createElement("span");
  time.className = "log-time";
  time.textContent = new Date().toLocaleTimeString("zh-CN", { hour12: false });
  const idx = document.createElement("span");
  idx.className = "log-index";
  idx.textContent = String(logIndex++);
  const msg = document.createElement("span");
  msg.className = "log-msg";
  msg.textContent = ev.payload;
  row.append(time, idx, msg);
  logList.prepend(row);
  while (logList.childElementCount > 500) logList.lastElementChild.remove();
});

/* ---------------- 按键绑定（KeyBindChooser 复刻） ---------------- */

const castKeyInput = el("castKeyInput");

/* C# CastKey_Focus：聚焦清空 */
castKeyInput.addEventListener("focus", () => { castKeyInput.value = ""; });

/* C# KeyBind_KeyUp：WPF e.Key → ConsoleKey 直转（数值==VK 码），成功才保存；
 * 失败则文本框清空。WebView2 的 KeyboardEvent.keyCode 即 Win32 VK 码，
 * 与 ConsoleKey 数值一一对应，天然覆盖全键盘（含 -、;、方向键等）。 */
castKeyInput.addEventListener("keyup", async e => {
  let code = e.keyCode || 0;
  // WPF e.Key 会把修饰键报成 Left/Right 变体；浏览器 keyCode 是通用 VK，
  // 映射到对应的 ConsoleKey 值以保持与原版一致
  if (code === 16) code = 160;      // Shift -> LeftShift
  if (code === 17) code = 162;      // Control -> LeftControl
  if (code === 18) code = 164;      // Alt -> LeftAlt
  const ok = code ? await invoke("set_cast_key", { code }) : false;
  if (ok) {
    castKeyInput.value = await invoke("get_cast_key");
    // C# CastKeyChanged：焦点跳到 Settings 按钮
    el("btnSettings").focus();
  } else {
    castKeyInput.value = "";
  }
});

/* 启动时读取持久化的按键（C# ReadConfiguration） */
invoke("get_cast_key").then(t => { castKeyInput.value = t; });

/* ---------------- 拟人化开关（扩展功能） ---------------- */

const humanizeToggle = el("humanizeToggle");
invoke("get_humanize").then(v => { humanizeToggle.checked = !!v; });
humanizeToggle.addEventListener("change", () => invoke("set_humanize", { on: humanizeToggle.checked }));

/* ---------------- 初始化 ---------------- */

window.addEventListener("DOMContentLoaded", () => {
  chart = new AmplitudeChart(el("chartCanvas"));
  fish = new FlyingFish(el("fishLayer"));
  // C# SizeChanged 防抖后 flipper 最终停在正面 —— 初始即正面，无需动作
  setButtonStates(true);
  syncStatus();
});
