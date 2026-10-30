// Where Mochi is and how it gets there: home at bottom center, spring glides to targets,
// 24 px beside the target so it never covers it. Coordinates are window-local pixels.

import { Bounds } from "./events";

export type Point = { x: number; y: number };

/** Breathing room between Mochi's feet and the taskbar at home. */
const HOME_GAP = 12;

export class Mover {
  private pos: Point;
  private from: Point;
  private to: Point;
  private t0 = 0;
  private dur = 0;
  private raf: number | null = null;
  /** Screen-space origin of this window's monitor; targets arrive in global screen coords. */
  monitorOrigin: Point = { x: 0, y: 0 };
  onFrame: ((pos: Point) => void) | null = null;

  private k = 1;
  constructor(
    private el: HTMLElement,
    private size: () => { w: number; h: number },
    private viewport: () => { w: number; h: number },
    /** Height of the body alone; the root also holds the bubble and buttons above it. */
    private bodyH: () => number = () => 0,
    /** Lowest usable y (window-local): the top of the taskbar/dock. Defaults to the viewport bottom. */
    private floor: () => number = () => this.viewport().h,
  ) {
    this.pos = this.home();
    this.from = this.pos;
    this.to = this.pos;
    this.place(this.pos);
  }

  /** Bottom center, resting just above the taskbar. */
  home(): Point {
    const v = this.viewport();
    const s = this.size();
    return { x: (v.w - s.w) / 2, y: this.floor() - s.h - HOME_GAP };
  }

  current(): Point {
    return this.pos;
  }

  goHome() {
    this.glideTo(this.home());
  }

  /** Park beside a target (to its right if there is room, else left), vertically centred on it. */
  goBeside(target: Bounds) {
    const s = this.size();
    const v = this.viewport();
    const tx = target.x - this.monitorOrigin.x;
    const ty = target.y - this.monitorOrigin.y;
    const gap = 24;
    let x = tx + target.w + gap;
    if (x + s.w > v.w) x = tx - gap - s.w;
    x = Math.max(4, Math.min(v.w - s.w - 4, x));
    // Centre the body (bottom of the root) on the target, leaving the bubble above it.
    let y = ty + target.h / 2 - (s.h - this.bodyH() * 0.5);
    y = Math.max(4, Math.min(this.floor() - s.h - 4, y));
    this.glideTo({ x, y });
  }

  glideTo(p: Point) {
    this.from = { ...this.pos };
    this.to = p;
    const dist = Math.hypot(p.x - this.pos.x, p.y - this.pos.y);
    // About 1,200 px/s with a floor so short hops still read as motion.
    this.dur = Math.max(260, Math.min(1400, (dist / 1200) * 1000));
    this.t0 = performance.now();
    if (this.raf === null) this.raf = requestAnimationFrame(this.step);
  }

  private step = (now: number) => {
    const k = Math.min(1, (now - this.t0) / this.dur);
    this.k = k;
    const e = easeInOutBack(k);
    this.pos = { x: this.from.x + (this.to.x - this.from.x) * e, y: this.from.y + (this.to.y - this.from.y) * e };
    this.place(this.pos);
    this.onFrame?.(this.pos);
    if (k < 1) {
      this.raf = requestAnimationFrame(this.step);
    } else {
      this.raf = null;
    }
  };

  private place(p: Point) {
    // Tilt slightly into the direction of travel.
    const dx = this.to.x - this.from.x;
    const moving = this.k < 1;
    const tilt = moving ? Math.max(-8, Math.min(8, dx / 60)) * Math.sin(Math.PI * this.k) : 0;
    this.el.style.transform = `translate(${p.x.toFixed(1)}px, ${p.y.toFixed(1)}px) rotate(${tilt.toFixed(1)}deg)`;
  }
}

function easeInOutBack(x: number): number {
  const c1 = 1.70158 * 0.6;
  const c2 = c1 * 1.525;
  return x < 0.5
    ? (Math.pow(2 * x, 2) * ((c2 + 1) * 2 * x - c2)) / 2
    : (Math.pow(2 * x - 2, 2) * ((c2 + 1) * (x * 2 - 2) + c2) + 2) / 2;
}

/** Gaze vector from Mochi's eye centre toward the cursor, normalised to [-1, 1]. */
export function gazeToward(eyeCenter: Point, cursor: Point): Point {
  const dx = cursor.x - eyeCenter.x;
  const dy = cursor.y - eyeCenter.y;
  const d = Math.hypot(dx, dy) || 1;
  const k = Math.min(1, d / 160);
  return { x: (dx / d) * k, y: (dy / d) * k };
}
