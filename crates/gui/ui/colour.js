/* 颜色配置窗口逻辑 — ColourConfiguration.xaml.cs 复刻（规格 docs/07 §8）
 * 两阶段行为：滑条拖动中（input）只更新文字/立方图；
 * 拖动结束（change，对应 Thumb.DragCompleted）才描边 + 重算截图高亮。
 * 扩展：点击截图取色、咬钩判定阈值可调（见 migration/logs/F2）。 */

const { invoke } = window.__TAURI__.core;
const el = id => document.getElementById(id);

let mode = "Red";               // ClassifierMode
let colourMultiplier = 0.5;     // 滑条值 / 100
let closenessMultiplier = 2.0;
let primaryColor = "红色", secondaryColor = "蓝色";
let screenImg = null;           // capture 的 Image（C# ScreenCapture，初始 null→占位图）
let rawCanvas = null;           // 原始截图离屏 canvas（取色用，避免读到高亮后的像素）
let hlCanvas = null;            // 高亮后的截图缓存（区域框直接在其上绘制，避免反复重算高亮）
let region = null;              // 搜索区域 {x,y,w,h}（截屏位图坐标），null = 全图
let dragStart = null;           // 拖框起点（位图坐标），拖拽中非 null

/* C# PixelClassifier.IsMatch 逐字复刻 */
function isBigger(red, other) { return red * colourMultiplier > other; }
function areClose(c1, c2) {
  const max = Math.max(c1, c2), min = Math.min(c1, c2);
  return min * closenessMultiplier > max - 20;
}
function isMatch(r, g, b) {
  if (mode === "Red") return isBigger(r, g) && isBigger(r, b) && areClose(b, g);
  return isBigger(b, g) && isBigger(b, r) && areClose(r, g);
}

/* ---------------- 立方图（RenderColour） ---------------- */

function renderCube(renderMatchedArea) {
  const findColour = parseInt(el("findColour").value, 10);
  const canvas = el("cubeCanvas");
  const ctx = canvas.getContext("2d");
  const img = ctx.createImageData(256, 256);
  const matched = new Uint8Array(256 * 256);

  for (let i = 0; i < 256; i++) {
    for (let g = 0; g < 256; g++) {
      // Red 模式：r=滑条值, b=i；Blue 模式：r=i, b=滑条值
      let r, b;
      if (mode === "Blue") { r = i; b = findColour; } else { r = findColour; b = i; }
      const off = (g * 256 + i) * 4;
      img.data[off] = r; img.data[off + 1] = g; img.data[off + 2] = b;
      img.data[off + 3] = 255;
      if (isMatch(r, g, b)) matched[g * 256 + i] = 1;
    }
  }

  /* C# MarkEdgeOfRedArea：4 邻接有缺失即为边缘，染白 */
  if (renderMatchedArea) {
    for (let y = 0; y < 256; y++) {
      for (let x = 0; x < 256; x++) {
        if (!matched[y * 256 + x]) continue;
        let close = 0;
        if (x > 0 && matched[y * 256 + x - 1]) close++;
        if (x < 255 && matched[y * 256 + x + 1]) close++;
        if (y > 0 && matched[(y - 1) * 256 + x]) close++;
        if (y < 255 && matched[(y + 1) * 256 + x]) close++;
        if (close < 4) {
          const off = (y * 256 + x) * 4;
          img.data[off] = img.data[off + 1] = img.data[off + 2] = 255;
        }
      }
    }
  }
  ctx.putImageData(img, 0, 0);

  if (renderMatchedArea) renderScreenshotHighlight();
}

/* ---------------- 截图高亮（MarkHighlightOnBitmap） ---------------- */

function renderScreenshotHighlight() {
  if (!screenImg) return;
  // 高亮结果渲染到离屏缓存；可见 canvas 由 blit() 负责（叠加搜索区域框）
  if (!hlCanvas) hlCanvas = document.createElement("canvas");
  hlCanvas.width = screenImg.naturalWidth;
  hlCanvas.height = screenImg.naturalHeight;
  const ctx = hlCanvas.getContext("2d");
  ctx.drawImage(screenImg, 0, 0);
  const img = ctx.getImageData(0, 0, hlCanvas.width, hlCanvas.height);
  const [hr, hg, hb] = mode === "Blue" ? [0, 0, 255] : [255, 0, 0];
  for (let i = 0; i < img.data.length; i += 4) {
    if (isMatch(img.data[i], img.data[i + 1], img.data[i + 2])) {
      img.data[i] = hr; img.data[i + 1] = hg; img.data[i + 2] = hb;
    }
  }
  ctx.putImageData(img, 0, 0);
  blit();
}

/* 高亮缓存 → 可见 canvas，并叠加搜索区域框 / 拖框预览（黄框） */
function blit(liveRect) {
  if (!hlCanvas) return;
  const canvas = el("wowCanvas");
  canvas.width = hlCanvas.width;
  canvas.height = hlCanvas.height;
  const ctx = canvas.getContext("2d");
  ctx.drawImage(hlCanvas, 0, 0);
  ctx.lineWidth = 2;
  ctx.strokeStyle = "#ffe600";
  if (region) ctx.strokeRect(region.x, region.y, region.w, region.h);
  if (liveRect) ctx.strokeRect(liveRect.x, liveRect.y, liveRect.w, liveRect.h);
}

/* ---------------- 截图交互：单击取色 / 拖拽框选搜索区域 ---------------- */

function toBitmap(e) {
  const canvas = el("wowCanvas");
  return {
    x: Math.floor(e.offsetX * canvas.width / canvas.clientWidth),
    y: Math.floor(e.offsetY * canvas.height / canvas.clientHeight),
  };
}

function normRect(a, b) {
  const canvas = el("wowCanvas");
  const x = Math.max(0, Math.min(a.x, b.x));
  const y = Math.max(0, Math.min(a.y, b.y));
  const x2 = Math.min(canvas.width, Math.max(a.x, b.x));
  const y2 = Math.min(canvas.height, Math.max(a.y, b.y));
  return { x, y, w: x2 - x, h: y2 - y };
}

/* 单击取色：从原始截图读像素，按 R>B 自动切换红/蓝模式，再设置色值滑条 */
function pickColour(pt) {
  if (!rawCanvas) return;
  const d = rawCanvas.getContext("2d").getImageData(pt.x, pt.y, 1, 1).data;
  const [r, g, b] = d;

  // 自动判断模式：红点选红模式，蓝点选蓝模式（仍可手动改下拉框）
  const newMode = r > b ? "Red" : "Blue";
  if (newMode !== mode) {
    mode = newMode;
    el("cmbColors").value = mode;
    if (mode === "Red") { primaryColor = "红色"; secondaryColor = "蓝色"; }
    else { primaryColor = "蓝色"; secondaryColor = "红色"; }
    invoke("set_classifier_mode", { mode });
    updateColourText();
  }

  const v = mode === "Blue" ? b : r;
  el("pickSwatch").style.background = `rgb(${r},${g},${b})`;
  el("pickInfo").textContent = `已取色 rgb(${r}, ${g}, ${b}) → ${mode === "Blue" ? "蓝" : "红"}模式，色值设为 ${v}`;

  el("findColour").value = v;
  el("labelRed").textContent = v;
  renderCube(true);
}

function updateRegionInfo() {
  el("regionInfo").textContent = region
    ? `搜索区域：仅框内 (${region.x}, ${region.y}) ${region.w}×${region.h}`
    : "搜索区域：全图（在截图上拖拽可框选水面）";
}

const DRAG_THRESHOLD = 5; // 位移小于此值视为单击（取色）

el("wowCanvas").addEventListener("mousedown", e => {
  if (!screenImg || !hlCanvas) return;
  dragStart = toBitmap(e);
});

el("wowCanvas").addEventListener("mousemove", e => {
  if (!dragStart) return;
  const cur = toBitmap(e);
  if (Math.abs(cur.x - dragStart.x) < DRAG_THRESHOLD &&
      Math.abs(cur.y - dragStart.y) < DRAG_THRESHOLD) return;
  blit(normRect(dragStart, cur));
});

el("wowCanvas").addEventListener("mouseup", async e => {
  if (!dragStart) return;
  const start = dragStart;
  dragStart = null;
  const cur = toBitmap(e);
  if (Math.abs(cur.x - start.x) < DRAG_THRESHOLD &&
      Math.abs(cur.y - start.y) < DRAG_THRESHOLD) {
    pickColour(start); // 单击 = 取色
    return;
  }
  const r = normRect(start, cur);
  if (r.w <= 0 || r.h <= 0) { blit(); return; }
  await invoke("set_search_region", { x: r.x, y: r.y, width: r.w, height: r.h });
  region = r;
  updateRegionInfo();
  blit();
});

el("wowCanvas").addEventListener("mouseleave", () => {
  if (dragStart) { dragStart = null; blit(); }
});

el("btnClearRegion").addEventListener("click", async () => {
  await invoke("clear_search_region");
  region = null;
  updateRegionInfo();
  blit();
});

/* ---------------- 事件接线 ---------------- */

/* 色值滑条：拖动中只重染立方图（RenderColour(false)） */
el("findColour").addEventListener("input", () => {
  el("labelRed").textContent = el("findColour").value;
  renderCube(false);
});
el("findColour").addEventListener("change", () => renderCube(true));

/* 乘数滑条：C# TwoWay 绑定 → 拖动中实时写参数 + 更新文字，拖完才重渲染 */
function updateColourText() {
  el("labelColourMultiplier").textContent =
    `${primaryColor} × ${colourMultiplier} 必须大于绿色和${secondaryColor}`;
  el("labelColourClosenessMultiplier").textContent =
    `绿色和${secondaryColor}需要多接近：${closenessMultiplier}`;
  el("colourLabel").textContent = primaryColor + ":";
}

el("colourMultiplier").addEventListener("input", async () => {
  colourMultiplier = parseInt(el("colourMultiplier").value, 10) / 100;
  updateColourText();
  await pushParams();
});
el("colourMultiplier").addEventListener("change", () => renderCube(true));

el("colourClosenessMultiplier").addEventListener("input", async () => {
  closenessMultiplier = parseInt(el("colourClosenessMultiplier").value, 10) / 100;
  updateColourText();
  await pushParams();
});
el("colourClosenessMultiplier").addEventListener("change", () => renderCube(true));

async function pushParams() {
  await invoke("set_classifier_params", {
    colourMultiplier,
    colourClosenessMultiplier: closenessMultiplier,
  });
}

/* 咬钩判定阈值：拖动中实时保存（下次开始时生效） */
el("strikeValue").addEventListener("input", () => {
  const v = parseInt(el("strikeValue").value, 10);
  el("labelStrike").textContent = `咬钩判定：浮漂下沉 ${v} 像素时提竿（海水起伏大时调大）`;
  invoke("set_strike_value", { v });
});

/* Capture Screen：重新截屏并全量渲染 */
el("btnCapture").addEventListener("click", async () => {
  const b64 = await invoke("capture_screen");
  screenImg = new Image();
  screenImg.onload = () => {
    el("wowPlaceholder").style.display = "none";
    el("wowCanvas").style.display = "block";
    // 保留一份原始截图供取色（显示用的 canvas 会被高亮染色）
    rawCanvas = document.createElement("canvas");
    rawCanvas.width = screenImg.naturalWidth;
    rawCanvas.height = screenImg.naturalHeight;
    rawCanvas.getContext("2d").drawImage(screenImg, 0, 0);
    renderCube(true);
  };
  screenImg.src = "data:image/png;base64," + b64;
});

/* 追踪羽毛下拉（C# SelectedIndex: Blue→0, Red→1） */
el("cmbColors").addEventListener("change", async () => {
  mode = el("cmbColors").value;
  if (mode === "Red") { primaryColor = "红色"; secondaryColor = "蓝色"; }
  else { primaryColor = "蓝色"; secondaryColor = "红色"; }
  await invoke("set_classifier_mode", { mode });
  updateColourText();
  renderCube(true);
});

/* 拾取延迟滑条：实时写全局（C# LootDelay_ValueChanged） */
el("lootDelay").addEventListener("input", () => {
  invoke("set_loot_delay", { ms: parseInt(el("lootDelay").value, 10) });
});

/* ---------------- 初始化 ---------------- */

(async () => {
  const cfg = await invoke("get_classifier_config");
  colourMultiplier = cfg.colour_multiplier;
  closenessMultiplier = cfg.colour_closeness_multiplier;
  mode = cfg.mode;
  if (mode === "Blue") { primaryColor = "蓝色"; secondaryColor = "红色"; }
  el("colourMultiplier").value = Math.round(colourMultiplier * 100);
  el("colourClosenessMultiplier").value = Math.round(closenessMultiplier * 100);
  el("cmbColors").value = mode;
  el("lootDelay").value = await invoke("get_loot_delay");
  el("strikeValue").value = await invoke("get_strike_value");
  el("labelStrike").textContent = `咬钩判定：浮漂下沉 ${el("strikeValue").value} 像素时提竿（海水起伏大时调大）`;
  // 恢复上次框选的水面搜索区域
  const saved = await invoke("get_search_region");
  if (saved) region = { x: saved.x, y: saved.y, w: saved.width, h: saved.height };
  updateRegionInfo();
  updateColourText();
  // C# 初始不自动渲染截图（占位图 quirk 保留），立方图渲染一次
  renderCube(false);
})();
