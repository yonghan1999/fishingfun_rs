const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const vm = require("node:vm");
const path = require("node:path");

const ui = path.join(__dirname, "../crates/gui/ui");

function macroUi(invoke) {
  const elements = new Map();
  const errors = [];
  const context = vm.createContext({
    window: {
      invokeWithFeedback: invoke,
      showOperationError: error => errors.push(error),
      __TAURI__: { event: { listen() {} } },
      addEventListener() {},
    },
    document: {
      getElementById(id) {
        if (!elements.has(id)) elements.set(id, { value: "", handlers: {}, addEventListener(name, fn) { this.handlers[name] = fn; } });
        return elements.get(id);
      },
      querySelectorAll() { return []; },
    },
  });
  vm.runInContext(fs.readFileSync(path.join(ui, "app.js"), "utf8"), context);
  return { el: id => elements.get(id), errors };
}

test("周期宏捕获按键、清空、保存及失败重试，严格拒绝无效分钟数", async () => {
  const saves = [];
  let rejectSave;
  let fail = true;
  const { el, errors } = macroUi(async (command, args) => {
    if (command === "get_macro_settings") return { keys: [53, 54], labels: ["D5", "D6"], minutes: 10 };
    if (command === "set_macro_settings") {
      saves.push(JSON.parse(JSON.stringify(args)));
      if (fail) return new Promise((_, reject) => { rejectSave = reject; });
      return { ...args, labels: ["F1", "未设置"] };
    }
  });
  assert.equal(el("macroPanel").disabled, true);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(el("macroPanel").disabled, false);
  assert.equal(el("macroKey1").value, "D5");
  assert.equal(el("macroMinutes").value, 10);
  let prevented = false;
  el("macroKey1").handlers.keydown({ key: "F1", code: "F1", keyCode: 112, preventDefault() { prevented = true; } });
  assert(prevented);
  el("clearMacroKey2").handlers.click();
  assert.equal(el("macroKey2").value, "未设置");
  for (const invalid of ["", "0", "1.5", "10081", "abc"]) {
    el("macroMinutes").value = invalid;
    await el("saveMacro").handlers.click();
  }
  assert.equal(errors.length, 5);
  assert.equal(saves.length, 0);
  el("macroMinutes").value = "3";
  const saving = el("saveMacro").handlers.click();
  assert.equal(el("macroPanel").disabled, true);
  rejectSave(new Error("disk full"));
  await saving;
  assert.equal(el("macroPanel").disabled, false);
  assert.match(el("macroSaveStatus").textContent, /保存失败/);
  assert.equal(el("macroKey1").value, "F1");
  fail = false;
  await el("saveMacro").handlers.click();
  assert.deepEqual(saves, [{ keys: [112, null], minutes: 3 }, { keys: [112, null], minutes: 3 }]);
  assert.match(el("macroSaveStatus").textContent, /已保存，下次启动生效/);
});

test("周期宏读取失败保持禁用，避免覆盖未知配置", async () => {
  const { el } = macroUi(async command => {
    if (command === "get_macro_settings") throw new Error("invalid config");
  });
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(el("macroPanel").disabled, true);
});

test("定时分钟数严格拒绝小数、空值和越界，恢复失败保留现有设置", async () => {
  const saved = [];
  const restored = [];
  const { el, errors } = macroUi(async (command, args) => {
    if (command === "get_macro_settings") return { keys: [53, 54], labels: ["D5", "D6"], minutes: 10 };
    if (command === "get_schedule") return { minutes: 0, action: "stop_fishing" };
    if (command === "set_schedule") saved.push(args);
    if (command === "restore_settings") { restored.push(args); throw new Error("backup failed"); }
  });
  await new Promise(setImmediate);
  for (const value of ["", " ", "1.5", "1abc", "-1", "10081"]) {
    el("scheduleMinutes").value = value;
    await el("saveSchedule").handlers.click();
  }
  assert.equal(saved.length, 0);
  assert.equal(errors.length, 6);
  el("scheduleMinutes").value = "2";
  await el("saveSchedule").handlers.click();
  assert.equal(saved[0].minutes, 2);
  await el("resetSettings").handlers.click();
  assert.equal(restored[0].text, null);
  assert.equal(el("scheduleMinutes").value, "2");
  assert.equal(el("resetSettings").disabled, false);
});

test("诊断显示计数与剩余时间，损坏配置阻止启动，恢复后同步全部设置", async () => {
  const elements = new Map(), listeners = {};
  let ready, poll;
  let recovery = "JSON 损坏";
  const context = vm.createContext({
    window: {
      invokeWithFeedback: async command => {
        if (command === "get_settings_recovery") return recovery;
        if (command === "get_macro_settings") return { keys: [112, null], labels: ["F1", "未设置"], minutes: 3 };
        if (command === "get_humanize") return false;
        if (command === "get_cast_key") return "A";
        if (command === "get_schedule") return { minutes: 20, action: "stop_fishing" };
        if (command === "bot_status") return { phase: "Idle", strike_value: 7 };
        if (command === "get_diagnostics") return { activity: "已锁定，等待咬钩", target_pid: 42, client: { width: 1920, height: 1080, x: -1920, y: 0 }, captures: 50, capture_errors: 1, capture_ms: 2.5, consecutive_misses: 3, remaining_secs: 61, last_error: "测试错误" };
      },
      __TAURI__: { event: { listen(name, fn) { listeners[name] = fn; } } },
      addEventListener(_, fn) { ready = fn; },
    },
    document: {
      getElementById(id) {
        if (!elements.has(id)) elements.set(id, { value: "", handlers: {}, classList: { toggle() {}, add() {}, remove() {} }, addEventListener(name, fn) { this.handlers[name] = fn; } });
        return elements.get(id);
      }, querySelectorAll() { return []; },
    },
    AmplitudeChart: class { setStrikeValue() {} }, FlyingFish: class {},
    setInterval(fn) { poll = fn; }, setTimeout() {}, clearTimeout() {},
  });
  vm.runInContext(fs.readFileSync(path.join(ui, "app.js"), "utf8"), context);
  ready();
  await new Promise(setImmediate);
  assert.equal(elements.get("btnPlay").disabled, true);
  assert.equal(elements.get("diagnosticsPanel").open, true);
  assert.match(elements.get("runtimeStatus").textContent, /已锁定.*2 分钟/);
  assert.match(elements.get("diagnosticsText").textContent, /-1920.*\n截图：50 帧，失败 1 次/);
  recovery = null;
  await listeners["settings-restored"]();
  await poll();
  assert.equal(elements.get("btnPlay").disabled, false);
  assert.equal(elements.get("castKeyInput").value, "A");
  assert.equal(elements.get("macroKey1").value, "F1");
  assert.equal(elements.get("scheduleMinutes").value, 20);
  assert.equal(elements.get("humanizeToggle").checked, false);
});

test("设置与诊断导出调用对应后端并生成本机 JSON 下载", async () => {
  const downloads = [], blobs = [], listeners = {};
  const elements = new Map();
  const context = vm.createContext({
    window: {
      invokeWithFeedback: async command => {
        if (command === "export_settings") return '{"loot_delay_ms":350}';
        if (command === "export_diagnostics") return '{"version":"0.1.0"}';
      }, __TAURI__: { event: { listen(name, fn) { listeners[name] = fn; } } }, addEventListener() {},
    },
    document: {
      getElementById(id) { if (!elements.has(id)) elements.set(id, { handlers: {}, addEventListener(name, fn) { this.handlers[name] = fn; } }); return elements.get(id); },
      querySelectorAll() { return []; }, body: { appendChild() {} },
      createElement() { return { click() { downloads.push({ name: this.download, href: this.href }); }, remove() {} }; },
    },
    Blob: class { constructor(parts) { blobs.push(parts.join("")); } },
    URL: { createObjectURL() { return "blob:local"; }, revokeObjectURL() {} }, setTimeout(fn) { fn(); },
  });
  vm.runInContext(fs.readFileSync(path.join(ui, "app.js"), "utf8"), context);
  await elements.get("exportSettings").handlers.click();
  await elements.get("exportDiagnostics").handlers.click();
  assert.deepEqual(downloads.map(d => d.name), ["settings-backup.json", "diagnostics.json"]);
  assert.deepEqual(blobs, ['{"loot_delay_ms":350}', '{"version":"0.1.0"}']);
});

test("颜色和延迟在拖动结束才保存，失败回读，恢复事件清除旧截图并同步模式", async () => {
  const elements = new Map(), listeners = {}, calls = [];
  let config = { colour_multiplier: 0.5, colour_closeness_multiplier: 2, mode: "Red" };
  const ctx2d = { createImageData() { return { data: new Uint8ClampedArray(256 * 256 * 4) }; }, putImageData() {} };
  const context = vm.createContext({
    window: {
      invokeWithFeedback: async (command, args) => {
        calls.push({ command, args });
        if (command === "get_classifier_config") return config;
        if (command === "get_loot_delay") return 250;
        if (command === "get_strike_value") return 7;
        if (command === "set_classifier_params" || command === "set_loot_delay") throw new Error("disk full");
      }, __TAURI__: { event: { listen(name, fn) { listeners[name] = fn; } } },
    },
    document: {
      getElementById(id) {
        if (!elements.has(id)) elements.set(id, { value: "100", style: {}, handlers: {}, addEventListener(name, fn) { this.handlers[name] = fn; }, getContext() { return ctx2d; } });
        return elements.get(id);
      },
    },
  });
  vm.runInContext(fs.readFileSync(path.join(ui, "colour.js"), "utf8"), context);
  await new Promise(setImmediate);
  const multiplier = elements.get("colourMultiplier");
  multiplier.value = "125";
  await multiplier.handlers.input();
  assert.equal(calls.filter(c => c.command === "set_classifier_params").length, 0);
  await multiplier.handlers.change();
  assert.equal(calls.find(c => c.command === "set_classifier_params").args.colourMultiplier, 1.25);
  assert.equal(multiplier.value, 50);
  elements.get("lootDelay").value = "900";
  await elements.get("lootDelay").handlers.change();
  assert.equal(elements.get("lootDelay").value, 250);
  config = { colour_multiplier: 1, colour_closeness_multiplier: 1, mode: "Blue" };
  listeners["settings-restored"]();
  await new Promise(setImmediate);
  assert.equal(elements.get("cmbColors").value, "Blue");
  assert.equal(elements.get("colourMultiplier").value, 100);
  assert.equal(elements.get("wowCanvas").style.display, "none");
  assert.equal(elements.get("wowPlaceholder").style.display, "block");
});

test("图表阈值更新和清图保持当前会话阈值，纵轴能显示大阈值", () => {
  const lines = [];
  const ctx = new Proxy({}, { get: (_, name) => (...args) => {
    if (name === "lineTo") lines.push(args);
  }, set: () => true });
  const canvas = { width: 400, height: 200, clientWidth: 400, clientHeight: 200, getContext: () => ctx };
  const context = vm.createContext({ window: { devicePixelRatio: 1 }, requestAnimationFrame: () => {} });
  vm.runInContext(fs.readFileSync(path.join(ui, "chart.js"), "utf8") + "\nglobalThis.Chart = AmplitudeChart", context);
  const chart = new context.Chart(canvas);
  chart.setStrikeValue(100);
  chart.clearChart();
  assert.equal(chart.strikeValue, 100);
  assert.equal(chart.strikeLine[0].v, -100);
  chart._draw();
  assert(lines.every(([, y]) => Number.isFinite(y)));
  assert(lines.some(([x, y]) => x === 400 && y > 0 && y < 200));
});

test("IPC 保存失败向用户显示错误并保持拒绝语义", async () => {
  let alert;
  const context = vm.createContext({
    window: { __TAURI__: { core: { invoke: async () => { throw new Error("disk full"); } } } },
    document: {
      getElementById: () => alert,
      createElement: () => ({ style: {}, setAttribute() {}, addEventListener() {} }),
      body: { appendChild(element) { alert = element; } },
    },
  });
  vm.runInContext(fs.readFileSync(path.join(ui, "common.js"), "utf8"), context);
  await assert.rejects(context.window.invokeWithFeedback("set_strike_value", { v: 12 }), /disk full/);
  assert.match(alert.textContent, /操作失败.*disk full/);
});

test("日志条目有上限，启动失败恢复按钮，图表使用后端实际阈值", async () => {
  const elements = new Map();
  function element() {
    const children = [];
    return { disabled: false, value: "", style: {}, dataset: {},
      classList: { toggle() {}, add() {}, remove() {}, contains() { return false; } },
      handlers: {}, addEventListener(name, fn) { this.handlers[name] = fn; },
      prepend(row) { children.unshift(row); }, append() {},
      get childElementCount() { return children.length; },
      get lastElementChild() { return { remove() { children.pop(); } }; },
    };
  }
  const listeners = {};
  let domReady;
  let threshold;
  let botStatus = { phase: "Idle", strike_value: 23, foreground_wait_secs: null };
  const context = vm.createContext({
    window: {
      invokeWithFeedback: async command => {
        if (command === "start_bot") throw new Error("spawn failed");
        if (command === "bot_status") return botStatus;
        if (command === "get_cast_key") return "4";
        if (command === "get_humanize") return true;
      },
      __TAURI__: { event: { listen(name, fn) { listeners[name] = fn; } } },
      addEventListener(_, fn) { domReady = fn; },
    },
    document: {
      getElementById(id) { if (!elements.has(id)) elements.set(id, element()); return elements.get(id); },
      querySelectorAll() { return []; }, createElement: element,
    },
    AmplitudeChart: class { setStrikeValue(value) { threshold = value; } },
    FlyingFish: class { stop() {} }, setTimeout() {}, clearTimeout() {}, setInterval() {},
  });
  vm.runInContext(fs.readFileSync(path.join(ui, "app.js"), "utf8"), context);
  domReady();
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(threshold, 23);
  for (let i = 0; i < 1200; i++) listeners.log({ payload: `row ${i}` });
  assert.equal(elements.get("logList").childElementCount, 500);
  await assert.rejects(elements.get("btnPlay").handlers.click(), /spawn failed/);
  assert.equal(elements.get("btnPlay").disabled, false);
  assert.equal(elements.get("btnStop").disabled, true);
  botStatus = { ...botStatus, phase: "Starting", foreground_wait_secs: 60 };
  await listeners["bot-status"]();
  assert.equal(elements.get("startupStatus").hidden, false);
  assert.match(elements.get("startupStatus").textContent, /等待魔兽世界进入前台.*60 秒.*停止取消/);
  assert.equal(elements.get("btnStop").disabled, false);
  botStatus.foreground_wait_secs = 59;
  await listeners["bot-status"]();
  assert.match(elements.get("startupStatus").textContent, /59 秒/);
  botStatus.foreground_wait_secs = null;
  await listeners["bot-status"]();
  assert.match(elements.get("startupStatus").textContent, /正在启动钓鱼/);
  botStatus.phase = "Running";
  await listeners["bot-status"]();
  assert.equal(elements.get("startupStatus").hidden, true);
  botStatus.phase = "Stopping";
  await listeners["bot-status"]();
  assert.match(elements.get("startupStatus").textContent, /正在停止/);
  assert.equal(elements.get("btnStop").disabled, true);
  botStatus.phase = "Idle";
  await listeners["bot-status"]();
  assert.equal(elements.get("startupStatus").hidden, true);
  assert.equal(elements.get("startupStatus").textContent, "");
});

test("拟人化保存失败恢复已保存状态，保存期间禁用，后台错误可见", async () => {
  const elements = new Map();
  const listeners = {};
  let rejectSave;
  let backendError;
  const context = vm.createContext({
    window: {
      invokeWithFeedback: async command => {
        if (command === "get_humanize") return false;
        if (command === "set_humanize") return new Promise((_, reject) => { rejectSave = reject; });
      },
      showOperationError(error) { backendError = error; },
      __TAURI__: { event: { listen(name, fn) { listeners[name] = fn; } } },
      addEventListener() {},
    },
    document: {
      getElementById(id) {
        if (!elements.has(id)) elements.set(id, { handlers: {}, addEventListener(name, fn) { this.handlers[name] = fn; } });
        return elements.get(id);
      },
      querySelectorAll() { return []; },
    },
  });
  vm.runInContext(fs.readFileSync(path.join(ui, "app.js"), "utf8"), context);
  const toggle = elements.get("humanizeToggle");
  assert.equal(toggle.disabled, true);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(toggle.checked, false);
  assert.equal(toggle.disabled, false);
  toggle.checked = true;
  const saving = toggle.handlers.change();
  assert.equal(toggle.disabled, true);
  rejectSave(new Error("disk full"));
  await saving;
  assert.equal(toggle.checked, false);
  assert.equal(toggle.disabled, false);
  listeners["bot-error"]({ payload: "目标窗口不在前台" });
  assert.equal(backendError, "目标窗口不在前台");
});
