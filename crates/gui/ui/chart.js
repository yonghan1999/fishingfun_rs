/* 振幅图表 — LiveCharts ConstantChangesChart 复刻（规格 docs/07 §4）
 * 三条线：数据主线(红 #F34336 6px + 圆形数据标签)、零线(蓝 1px + LightBlue 填充)、
 * 阈值线(黑 1px, y=-7)。X 轴窗口 [now-8s, now+1s] 滚动，无 X 标签/分隔线。 */

class AmplitudeChart {
  constructor(canvas) {
    this.canvas = canvas;
    this.ctx = canvas.getContext("2d");
    this.data = [];       // {t: ms, v: amplitude}
    this.zeroLine = [];   // 蓝线
    this.strikeLine = []; // 黑线（阈值 -7）
    this.clearChart();
    this._tick = this._tick.bind(this);
    requestAnimationFrame(this._tick);
  }

  add(value) {
    this.data.push({ t: Date.now(), v: value });
  }

  /* C# ClearChart：清空后零线 (now-12s,0)-(now+25s,0)，阈值线 y=-7 同区间 */
  clearChart() {
    const now = Date.now();
    this.data = [];
    this.zeroLine = [{ t: now - 12000, v: 0 }, { t: now + 25000, v: 0 }];
    this.strikeLine = [{ t: now - 12000, v: -7 }, { t: now + 25000, v: -7 }];
  }

  _tick() {
    this._draw();
    requestAnimationFrame(this._tick);
  }

  _draw() {
    const { canvas, ctx } = this;
    const dpr = window.devicePixelRatio || 1;
    const w = canvas.clientWidth, h = canvas.clientHeight;
    if (w === 0 || h === 0) return;
    if (canvas.width !== w * dpr || canvas.height !== h * dpr) {
      canvas.width = w * dpr; canvas.height = h * dpr;
    }
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, w, h);

    const now = Date.now();
    const xMin = now - 8000, xMax = now + 1000;   // SetAxisLimits: 后 8s 前 1s
    const yMin = -10, yMax = 5;
    const X = t => ((t - xMin) / (xMax - xMin)) * w;
    const Y = v => h - ((v - yMin) / (yMax - yMin)) * h;

    // Y 分隔线 #DADADA 2px
    ctx.strokeStyle = "#DADADA";
    ctx.lineWidth = 2;
    for (let y = yMin; y <= yMax; y += 5) {
      ctx.beginPath();
      ctx.moveTo(0, Y(y)); ctx.lineTo(w, Y(y));
      ctx.stroke();
    }

    // 零线：蓝 1px + LightBlue 填充（填到 y=0 区域下缘）
    const zero = this._window(this.zeroLine, xMin, xMax);
    if (zero.length >= 2) {
      ctx.beginPath();
      ctx.moveTo(X(zero[0].t), Y(zero[0].v));
      ctx.lineTo(X(zero[1].t), Y(zero[1].v));
      ctx.lineTo(X(zero[1].t), h);
      ctx.lineTo(X(zero[0].t), h);
      ctx.closePath();
      ctx.fillStyle = "rgba(173,216,230,.5)";
      ctx.fill();
      ctx.strokeStyle = "#2196F3";
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(X(zero[0].t), Y(zero[0].v));
      ctx.lineTo(X(zero[1].t), Y(zero[1].v));
      ctx.stroke();
    }

    // 阈值线：黑 1px
    const strike = this._window(this.strikeLine, xMin, xMax);
    if (strike.length >= 2) {
      ctx.strokeStyle = "#000";
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(X(strike[0].t), Y(strike[0].v));
      ctx.lineTo(X(strike[1].t), Y(strike[1].v));
      ctx.stroke();
    }

    // 数据主线：红 #F34336 6px，无平滑，圆形数据标签
    const pts = this._window(this.data, xMin - 1000, xMax);
    if (pts.length > 0) {
      ctx.strokeStyle = "#F34336";
      ctx.lineWidth = 6;
      ctx.lineJoin = "miter"; // LineSmoothness=0
      ctx.beginPath();
      pts.forEach((p, i) => {
        if (i === 0) ctx.moveTo(X(p.t), Y(p.v));
        else ctx.lineTo(X(p.t), Y(p.v));
      });
      ctx.stroke();

      // 数据标签：灰圆底 + 白数值
      ctx.textAlign = "center";
      ctx.textBaseline = "middle";
      ctx.font = "10px 'Segoe UI', sans-serif";
      for (const p of pts) {
        const cx = X(p.t), cy = Y(p.v);
        ctx.beginPath();
        ctx.arc(cx, cy, 9, 0, Math.PI * 2);
        ctx.fillStyle = "gray";
        ctx.fill();
        ctx.strokeStyle = "darkgray";
        ctx.lineWidth = 1;
        ctx.stroke();
        ctx.fillStyle = "#fff";
        ctx.fillText(String(p.v), cx, cy);
      }
    }
  }

  _window(series, min, max) {
    return series.filter(p => p.t >= min && p.t <= max);
  }
}
