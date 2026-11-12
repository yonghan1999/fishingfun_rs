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
  const startupStatus = el("startupStatus");
  startupStatus.hidden = status.phase !== "Starting" && status.phase !== "Stopping";
  if (status.phase === "Starting") {
    startupStatus.textContent = status.foreground_wait_secs != null
      ? `等待魔兽世界进入前台，请切回游戏。剩余 ${status.foreground_wait_secs} 秒，可点击停止取消。`
      : "正在启动钓鱼，请保持魔兽世界在前台。可点击停止取消。";
  } else if (status.phase === "Stopping") {
    startupStatus.textContent = "正在停止，请稍候……";
  } else {
    startupStatus.textContent = "";
  }
}

async function syncStatus() { applyStatus(await invoke("bot_status")); }
listen("bot-status", () => syncStatus());
listen("bot-error", ev => window.showOperationError(ev.payload));
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

listen("session-event", ev => {
  const { action, amplitude } = ev.payload;
  // 短码协议：M=浮漂移动 R=复位 L=拾取 C=抛竿（对应 FishingAction 的 serde rename）
  switch (action) {
    case "M":
      // C#：仅 Flipper 正面时加点
      if (!el("graphFlipper").classList.contains("flipped")) {
        chart.add(amplitude);
      }
      break;
    case "L":
      fish.start(previewArea.clientWidth, previewArea.clientHeight);
      lootingOverlay.classList.remove("hidden");
      break;
    case "C":
      chart.clearChart();
      lootingOverlay.classList.add("hidden");
      fish.stop();
      setImageBackgroundColour = true;
      break;
    case "R":
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
function keyCodeFromEvent(e) {
  let code = e.keyCode || 0;
  // WPF e.Key 会把修饰键报成 Left/Right 变体；浏览器 keyCode 是通用 VK，
  // 映射到对应的 ConsoleKey 值以保持与原版一致
  if (code === 16) code = 160;      // Shift -> LeftShift
  if (code === 17) code = 162;      // Control -> LeftControl
  if (code === 18) code = 164;      // Alt -> LeftAlt
  return code;
}

castKeyInput.addEventListener("keyup", async e => {
  const code = keyCodeFromEvent(e);
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

/* 周期宏：编辑草稿统一保存，失败保留草稿，不改变运行中的会话。 */
const macroPanel = el("macroPanel");
const macroInputs = [el("macroKey1"), el("macroKey2")];
const macroMinutes = el("macroMinutes");
const macroSaveStatus = el("macroSaveStatus");
let macroKeys = [null, null];
macroPanel.disabled = true;
function showMacroSettings(settings) {
  macroKeys = settings.keys.slice();
  macroInputs.forEach((input, i) => { input.value = settings.labels[i]; });
  macroMinutes.value = settings.minutes;
}
invoke("get_macro_settings").then(settings => {
  showMacroSettings(settings);
  macroPanel.disabled = false;
}).catch(() => {});
macroInputs.forEach((input, i) => {
  // 阻止浏览器快捷键与文字输入，按下时捕获一次，避免修饰键抬起覆盖目标键。
  input.addEventListener("keydown", e => {
    if (e.key === "Tab") return;
    e.preventDefault();
    if (e.repeat) return;
    const code = keyCodeFromEvent(e);
    if (!code) return;
    macroKeys[i] = code;
    input.value = /^Digit\d$/.test(e.code || "") ? `D${e.key}` : (e.code || e.key);
    macroSaveStatus.textContent = "尚未保存";
  });
  el(`clearMacroKey${i + 1}`).addEventListener("click", () => {
    macroKeys[i] = null;
    input.value = "未设置";
    macroSaveStatus.textContent = "尚未保存";
  });
});
macroMinutes.addEventListener("input", () => { macroSaveStatus.textContent = "尚未保存"; });
el("saveMacro").addEventListener("click", async () => {
  const minutes = Number(macroMinutes.value);
  if (!Number.isInteger(minutes) || minutes < 1 || minutes > 10080) {
    window.showOperationError("宏键周期必须为 1..10080 分钟的整数");
    return;
  }
  macroPanel.disabled = true;
  try {
    const settings = await invoke("set_macro_settings", { keys: macroKeys.slice(), minutes });
    showMacroSettings(settings);
    macroSaveStatus.textContent = "已保存，下次启动生效";
  } catch {
    macroSaveStatus.textContent = "保存失败，请重试";
  } finally {
    macroPanel.disabled = false;
  }
});

/* ---------------- 拟人化开关（扩展功能） ---------------- */

const humanizeToggle = el("humanizeToggle");
let savedHumanize = true;
humanizeToggle.disabled = true;
invoke("get_humanize").then(v => {
  savedHumanize = !!v;
  humanizeToggle.checked = savedHumanize;
  humanizeToggle.disabled = false;
}).catch(() => {}); // 读取失败时错误已显示，保持禁用，避免覆盖未知配置。
humanizeToggle.addEventListener("change", async () => {
  const requested = humanizeToggle.checked;
  humanizeToggle.disabled = true;
  try {
    await invoke("set_humanize", { on: requested });
    savedHumanize = requested;
  } catch {
    humanizeToggle.checked = savedHumanize;
  } finally {
    humanizeToggle.disabled = false;
  }
});

/* 定时动作：0 分钟表示关闭定时。设置保存后下次启动会话生效。 */
const scheduleMinutes = el("scheduleMinutes");
const scheduleAction = el("scheduleAction");
const saveSchedule = el("saveSchedule");
invoke("get_schedule").then(v => {
  scheduleMinutes.value = v.minutes;
  scheduleAction.value = v.action;
}).catch(() => {});
saveSchedule.addEventListener("click", async () => {
  const minutes = Number.parseInt(scheduleMinutes.value, 10);
  if (!Number.isInteger(minutes) || minutes < 0 || minutes > 10080) {
    window.showOperationError("定时分钟数必须在 0..10080 之间");
    return;
  }
  saveSchedule.disabled = true;
  try {
    await invoke("set_schedule", { minutes, action: scheduleAction.value });
  } finally {
    saveSchedule.disabled = false;
  }
});

/* ---------------- 初始化 ---------------- */

window.addEventListener("DOMContentLoaded", () => {
  chart = new AmplitudeChart(el("chartCanvas"));
  fish = new FlyingFish(el("fishLayer"));
  // C# SizeChanged 防抖后 flipper 最终停在正面 —— 初始即正面，无需动作
  setButtonStates(true);
  syncStatus();
});
