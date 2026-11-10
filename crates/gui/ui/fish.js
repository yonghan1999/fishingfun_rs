/* 飞鱼动画 — FlyingFishAnimation 复刻（规格 docs/07 §7）
 * 30 条鱼、100×100、30ms 帧、越界回绕、index*33 < 高度才可见、旋转不实现（原 quirk）。 */

class FlyingFish {
  constructor(layer) {
    this.layer = layer;
    this.fishes = [];
    this.timer = null;
    for (let i = 0; i < 30; i++) {
      const img = document.createElement("img");
      img.src = "assets/FishingFunLogo.png";
      img.style.display = "none";
      layer.appendChild(img);
      this.fishes.push({
        img,
        index: i,
        x: 0, y: 0,
        speedX: 30 - rand(0, 60),   // C#: 30 - r.Next(60)
        speedY: rand(0, 30) + 30,   // C#: r.Next(30) + 30
      });
    }
  }

  start(width, height) {
    for (const f of this.fishes) {
      f.x = rand(0, width);
      f.y = rand(0, height);
      // C#: f.index * 33 < AnimationHeight ? Visible : Collapsed
      f.img.style.display = f.index * 33 < height ? "block" : "none";
      this._place(f);
    }
    if (this.timer) clearInterval(this.timer);
    this.timer = setInterval(() => {
      for (const f of this.fishes) {
        f.x = keepInBounds(f.x + f.speedX, width);
        f.y = keepInBounds(f.y + f.speedY, height);
        this._place(f);
      }
    }, 30);
  }

  stop() {
    if (this.timer) { clearInterval(this.timer); this.timer = null; }
    for (const f of this.fishes) f.img.style.display = "none";
  }

  _place(f) {
    f.img.style.left = f.x + "px";
    f.img.style.top = f.y + "px";
  }
}

/* C#: value < 0 ? max + value : value > max ? value - max : value */
function keepInBounds(value, max) {
  if (value < 0) return max + value;
  if (value > max) return value - max;
  return value;
}

function rand(min, max) { // C# Random.Next(min, max) = [min, max)
  return min + Math.floor(Math.random() * (max - min));
}
