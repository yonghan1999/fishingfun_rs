const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const vm = require("node:vm");
const path = require("node:path");

const ui = path.join(__dirname, "../crates/gui/ui");

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
  const context = vm.createContext({
    window: {
      invokeWithFeedback: async command => {
        if (command === "start_bot") throw new Error("spawn failed");
        if (command === "bot_status") return { phase: "Idle", strike_value: 23 };
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
    FlyingFish: class { stop() {} }, setTimeout() {}, clearTimeout() {},
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
