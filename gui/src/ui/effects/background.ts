// Adapted from an internal effects library.
// ========== ANIMATED BACKGROUND RENDERER ==========
// Renders grid, starfield, matrix, aurora, and sunset backgrounds on a canvas

export type BackgroundType = 'grid' | 'starfield' | 'matrix' | 'aurora' | 'sunset' | 'image' | 'none';

export interface BackgroundRendererOptions {
  canvas: HTMLCanvasElement;
  type?: BackgroundType;
  /** Image URL -- used when type is 'image'. Must be a valid https URL. */
  imageUrl?: string;
  animLoopAdd?: (fn: () => void) => void;
  animLoopRemove?: (fn: () => void) => void;
}

interface Star {
  x: number;
  y: number;
  z: number;
  twinkle: number;
}

interface MatrixColumn {
  y: number;
  speed: number;
  len: number;
}

interface AuroraOrb {
  x: number;
  y: number;
  rx: number;
  ry: number;
  hue: number;
  speed: number;
  phase: number;
}

const MATRIX_CHARS =
  'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789@#$%^&*()' +
  'アイウエオカキクケコ' +
  'サシスセソタチツテト' +
  'ナニヌネノ';

export class BackgroundRenderer {
  private canvas: HTMLCanvasElement;
  private ctx: CanvasRenderingContext2D;
  private w = 0;
  private h = 0;
  private type: BackgroundType;
  private image: HTMLImageElement | null = null;
  private imageReady = false;
  private time = 0;

  // Starfield
  private stars: Star[] = [];
  // Matrix
  private matrixCols: MatrixColumn[] = [];
  private matrixColCount = 0;
  // Aurora
  private auroraOrbs: AuroraOrb[] = [];

  private animLoopAdd: ((fn: () => void) => void) | null = null;
  private animLoopRemove: ((fn: () => void) => void) | null = null;
  private rafId = 0;
  private useOwnLoop: boolean;

  constructor(opts: BackgroundRendererOptions) {
    this.canvas = opts.canvas;
    const ctx = this.canvas.getContext('2d');
    if (!ctx) throw new Error('Failed to get 2D context');
    this.ctx = ctx;
    this.type = opts.type ?? 'grid';
    this.animLoopAdd = opts.animLoopAdd ?? null;
    this.animLoopRemove = opts.animLoopRemove ?? null;
    this.useOwnLoop = !opts.animLoopAdd;

    this.resize();
    window.addEventListener('resize', this.resize);

    this.init();

    if (opts.imageUrl) {
      this.loadImage(opts.imageUrl);
    }

    if (this.useOwnLoop) {
      const loop = (): void => {
        this.render();
        this.rafId = requestAnimationFrame(loop);
      };
      this.rafId = requestAnimationFrame(loop);
    } else {
      this.animLoopAdd?.(this.render);
    }
  }

  private resize = (): void => {
    this.w = this.canvas.width = window.innerWidth;
    this.h = this.canvas.height = window.innerHeight;
    this.init();
  };

  private init(): void {
    if (this.type === 'starfield') this.initStars();
    if (this.type === 'matrix') this.initMatrix();
    if (this.type === 'aurora') this.initAurora();
  }

  private initStars(): void {
    this.stars = [];
    const count = Math.floor(this.w * this.h / 3000);
    for (let i = 0; i < count; i++) {
      this.stars.push({
        x: Math.random() * this.w,
        y: Math.random() * this.h,
        z: Math.random() * 3 + 0.5,
        twinkle: Math.random() * Math.PI * 2,
      });
    }
  }

  private initMatrix(): void {
    this.matrixColCount = Math.floor(this.w / 16);
    this.matrixCols = [];
    for (let i = 0; i < this.matrixColCount; i++) {
      this.matrixCols.push({
        y: Math.random() * this.h,
        speed: 1 + Math.random() * 3,
        len: 8 + Math.floor(Math.random() * 20),
      });
    }
  }

  private initAurora(): void {
    this.auroraOrbs = [];
    for (let i = 0; i < 5; i++) {
      this.auroraOrbs.push({
        x: Math.random() * this.w,
        y: this.h * 0.2 + Math.random() * this.h * 0.3,
        rx: this.w * (0.15 + Math.random() * 0.2),
        ry: this.h * (0.08 + Math.random() * 0.12),
        hue: 200 + Math.random() * 120,
        speed: 0.2 + Math.random() * 0.3,
        phase: Math.random() * Math.PI * 2,
      });
    }
  }

  private loadImage(url: string): void {
    this.image = new Image();
    this.image.onload = (): void => { this.imageReady = true; };
    this.image.src = url;
  }

  setType(type: BackgroundType, imageUrl?: string): void {
    this.type = type;
    this.imageReady = false;
    this.image = null;
    this.init();
    if (imageUrl) {
      this.loadImage(imageUrl);
    }
    this.canvas.style.display = type === 'none' ? 'none' : 'block';
  }

  private renderGrid(): void {
    const { w, h, ctx } = this;
    const midY = h * 0.55;
    this.time += 0.008;
    ctx.clearRect(0, 0, w, h);

    // Sky gradient
    const sky = ctx.createLinearGradient(0, 0, 0, midY);
    sky.addColorStop(0, 'rgba(10, 0, 30, 1)');
    sky.addColorStop(1, 'rgba(60, 10, 80, 0.6)');
    ctx.fillStyle = sky;
    ctx.fillRect(0, 0, w, midY);

    // Sun glow
    const glow = ctx.createRadialGradient(w / 2, midY, 0, w / 2, midY, w * 0.5);
    glow.addColorStop(0, 'rgba(255, 60, 172, 0.35)');
    glow.addColorStop(0.4, 'rgba(94, 186, 239, 0.12)');
    glow.addColorStop(1, 'rgba(0, 0, 0, 0)');
    ctx.fillStyle = glow;
    ctx.fillRect(0, midY - h * 0.3, w, h * 0.6);

    // Grid lines horizontal
    ctx.lineWidth = 1;
    const linesH = 20;
    for (let d = 0; d < linesH; d++) {
      const g = d / linesH;
      const offset = (this.time * 0.5) % (1 / linesH);
      const py = midY + (h - midY) * Math.pow(g + offset, 1.8);
      if (py > h) continue;
      ctx.strokeStyle = `rgba(94, 186, 239, ${0.08 + g * 0.15})`;
      ctx.beginPath();
      ctx.moveTo(0, py);
      ctx.lineTo(w, py);
      ctx.stroke();
    }

    // Grid lines vertical (converging to horizon)
    const linesV = 24;
    ctx.strokeStyle = 'rgba(94, 186, 239, 0.12)';
    for (let d = -linesV / 2; d <= linesV / 2; d++) {
      const gx = (d / (linesV / 2)) * w * 1.2;
      ctx.beginPath();
      ctx.moveTo(w / 2 + gx, h);
      ctx.lineTo(w / 2, midY);
      ctx.stroke();
    }

    // Sun semicircle
    const sunR = Math.min(w, h) * 0.12;
    const sunGrad = ctx.createLinearGradient(w / 2, midY - sunR, w / 2, midY + sunR * 0.3);
    sunGrad.addColorStop(0, 'rgba(255, 60, 172, 0.8)');
    sunGrad.addColorStop(0.5, 'rgba(255, 140, 50, 0.6)');
    sunGrad.addColorStop(1, 'rgba(255, 60, 172, 0)');
    ctx.fillStyle = sunGrad;
    ctx.beginPath();
    ctx.arc(w / 2, midY, sunR, Math.PI, 0);
    ctx.fill();

    // Sun scan lines
    ctx.save();
    ctx.beginPath();
    ctx.arc(w / 2, midY, sunR, Math.PI, 0);
    ctx.clip();
    for (let d = 0; d < 6; d++) {
      const lineY = midY - sunR + (sunR * 2 * d / 6) + (this.time * 20 % (sunR * 2 / 6));
      ctx.fillStyle = 'rgba(10, 0, 30, 0.7)';
      ctx.fillRect(w / 2 - sunR, lineY, sunR * 2, 3 + d * 0.8);
    }
    ctx.restore();
  }

  private renderStarfield(): void {
    const { w, h, ctx } = this;
    ctx.clearRect(0, 0, w, h);

    const bg = ctx.createRadialGradient(w / 2, h / 2, 0, w / 2, h / 2, w * 0.7);
    bg.addColorStop(0, 'rgba(20, 5, 30, 0.3)');
    bg.addColorStop(1, 'rgba(0, 0, 0, 0)');
    ctx.fillStyle = bg;
    ctx.fillRect(0, 0, w, h);

    const t = performance.now() * 0.001;
    for (const star of this.stars) {
      star.y -= star.z * 0.15;
      if (star.y < -5) { star.y = h + 5; star.x = Math.random() * w; }

      const blink = 0.5 + 0.5 * Math.sin(t * 1.5 + star.twinkle);
      const alpha = (0.3 + star.z * 0.2) * blink;

      ctx.beginPath();
      ctx.arc(star.x, star.y, star.z * 0.6, 0, Math.PI * 2);
      ctx.fillStyle = `rgba(255, 255, 255, ${alpha})`;
      ctx.fill();

      if (star.z > 2.5) {
        ctx.beginPath();
        ctx.arc(star.x, star.y, star.z * 1.5, 0, Math.PI * 2);
        ctx.fillStyle = `rgba(200, 180, 255, ${alpha * 0.15})`;
        ctx.fill();
      }
    }

    // Occasional shooting star
    if (Math.random() < 0.002) {
      const sx = Math.random() * w;
      const sy = Math.random() * h * 0.5;
      ctx.strokeStyle = 'rgba(255, 255, 255, 0.6)';
      ctx.lineWidth = 1.5;
      ctx.beginPath();
      ctx.moveTo(sx, sy);
      ctx.lineTo(sx + 60, sy + 30);
      ctx.stroke();
    }
  }

  private renderMatrix(): void {
    const { w, ctx } = this;
    ctx.fillStyle = 'rgba(0, 0, 0, 0.06)';
    ctx.fillRect(0, 0, w, this.h);
    ctx.font = '14px monospace';

    for (let i = 0; i < this.matrixColCount; i++) {
      const col = this.matrixCols[i];
      if (!col) continue;
      const x = i * 16;
      ctx.fillStyle = 'rgba(0, 255, 65, 0.9)';
      ctx.fillText(MATRIX_CHARS[Math.floor(Math.random() * MATRIX_CHARS.length)] ?? ' ', x, col.y);

      for (let j = 1; j < col.len; j++) {
        const charY = col.y - j * 16;
        if (charY < 0) break;
        ctx.fillStyle = `rgba(0, 255, 65, ${0.4 * (1 - j / col.len)})`;
        ctx.fillText(MATRIX_CHARS[Math.floor(Math.random() * MATRIX_CHARS.length)] ?? ' ', x, charY);
      }

      col.y += col.speed * 4;
      if (col.y - col.len * 16 > this.h) {
        col.y = -Math.random() * this.h * 0.5;
        col.speed = 1 + Math.random() * 3;
        col.len = 8 + Math.floor(Math.random() * 20);
      }
    }
  }

  private renderAurora(): void {
    const { w, h, ctx } = this;
    ctx.clearRect(0, 0, w, h);
    const t = performance.now() * 0.001;

    for (const orb of this.auroraOrbs) {
      const ox = orb.x + Math.sin(t * orb.speed + orb.phase) * w * 0.15;
      const oy = orb.y + Math.cos(t * orb.speed * 0.7 + orb.phase) * h * 0.05;
      const grad = ctx.createRadialGradient(ox, oy, 0, ox, oy, orb.rx);
      grad.addColorStop(0, `hsla(${orb.hue + Math.sin(t) * 20}, 70%, 55%, 0.12)`);
      grad.addColorStop(0.5, `hsla(${orb.hue + 30}, 60%, 40%, 0.06)`);
      grad.addColorStop(1, 'rgba(0, 0, 0, 0)');
      ctx.fillStyle = grad;
      ctx.beginPath();
      ctx.ellipse(ox, oy, orb.rx, orb.ry, 0, 0, Math.PI * 2);
      ctx.fill();
    }

    for (let i = 0; i < 3; i++) {
      const bandY = h * 0.25 + i * h * 0.12 + Math.sin(t * 0.5 + i) * 20;
      const band = ctx.createLinearGradient(0, bandY - 20, 0, bandY + 20);
      band.addColorStop(0, 'rgba(0, 0, 0, 0)');
      band.addColorStop(0.5, `hsla(${260 + i * 40 + Math.sin(t * 0.3) * 20}, 60%, 50%, 0.04)`);
      band.addColorStop(1, 'rgba(0, 0, 0, 0)');
      ctx.fillStyle = band;
      ctx.fillRect(0, bandY - 30, w, 60);
    }
  }

  private renderImage(): void {
    const { w, h, ctx } = this;
    ctx.clearRect(0, 0, w, h);
    if (!this.imageReady || !this.image) return;

    const imgRatio = this.image.width / this.image.height;
    const canvasRatio = w / h;
    let dw: number, dh: number, dx: number, dy: number;
    if (canvasRatio > imgRatio) {
      dw = w; dh = w / imgRatio;
      dx = 0; dy = (h - dh) / 2;
    } else {
      dh = h; dw = h * imgRatio;
      dx = (w - dw) / 2; dy = 0;
    }
    ctx.drawImage(this.image, dx, dy, dw, dh);
  }

  private renderNone(): void {
    this.ctx.clearRect(0, 0, this.w, this.h);
  }

  private render = (): void => {
    switch (this.type) {
      case 'grid':      this.renderGrid(); break;
      case 'starfield': this.renderStarfield(); break;
      case 'matrix':    this.renderMatrix(); break;
      case 'aurora':    this.renderAurora(); break;
      case 'sunset':
      case 'image':     this.renderImage(); break;
      case 'none':      this.renderNone(); break;
    }
  };

  destroy(): void {
    window.removeEventListener('resize', this.resize);
    if (this.useOwnLoop) {
      cancelAnimationFrame(this.rafId);
    } else {
      this.animLoopRemove?.(this.render);
    }
  }
}
