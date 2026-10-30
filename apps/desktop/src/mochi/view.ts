// The SVG Mochi. Implements MochiView; a Rive implementation can replace it file-for-file
// (see rive.ts) once the .riv asset exists. Everything here is driven by the model's effects.

import { CharacterState } from "./events";
import { ViewGesture } from "./model";

export interface MochiView {
  readonly root: HTMLElement;
  setState(state: CharacterState): void;
  /** Gaze direction in [-1, 1] on each axis; the pupils follow. */
  setGaze(dx: number, dy: number): void;
  gesture(g: ViewGesture): void;
  bubble(text: string | null, variant: "speech" | "thinking"): void;
  showConfirm(text: string | null, onAnswer: (yes: boolean) => void): void;
  /** Bounding box of the clickable body, in window pixels. */
  bodyRect(): DOMRect;
}

const SVG = `
<svg class="mochi-svg" viewBox="0 0 120 112" width="120" height="112" aria-label="Mochi">
  <defs>
    <radialGradient id="mochi-body" cx="42%" cy="34%" r="70%">
      <stop offset="0%" stop-color="#fffdf9"/>
      <stop offset="55%" stop-color="#fbf4ec"/>
      <stop offset="100%" stop-color="#ecdfd2"/>
    </radialGradient>
    <radialGradient id="mochi-sheen" cx="35%" cy="25%" r="40%">
      <stop offset="0%" stop-color="#ffffff" stop-opacity="0.9"/>
      <stop offset="100%" stop-color="#ffffff" stop-opacity="0"/>
    </radialGradient>
    <filter id="mochi-soft" x="-20%" y="-20%" width="140%" height="140%">
      <feGaussianBlur stdDeviation="1.2"/>
    </filter>
  </defs>
  <ellipse class="shadow" cx="60" cy="104" rx="34" ry="5" fill="#000" opacity="0.14" filter="url(#mochi-soft)"/>
  <g class="body-group">
    <g class="arm arm-l"><ellipse cx="9" cy="68" rx="10" ry="6.5" fill="#f8efe6" stroke="#e4d4c4" stroke-width="1"/></g>
    <g class="arm arm-r"><ellipse cx="111" cy="68" rx="10" ry="6.5" fill="#f8efe6" stroke="#e4d4c4" stroke-width="1"/></g>
    <path class="body" d="M60 8 C 96 8 112 34 112 60 C 112 90 90 102 60 102 C 30 102 8 90 8 60 C 8 34 24 8 60 8 Z" fill="url(#mochi-body)" stroke="#e6d8ca" stroke-width="1"/>
    <ellipse class="sheen" cx="44" cy="30" rx="22" ry="12" fill="url(#mochi-sheen)"/>
    <g class="blush"><ellipse cx="28" cy="70" rx="8" ry="4"/><ellipse cx="92" cy="70" rx="8" ry="4"/></g>
    <g class="eyes">
      <g class="eye eye-l" transform="translate(42 56)">
        <g class="pupil"><ellipse rx="6.2" ry="8.2" fill="#2b2420"/><circle cx="-2.2" cy="-3.2" r="2" fill="#fff"/><circle cx="2" cy="3.5" r="0.9" fill="#fff" opacity="0.7"/></g>
        <ellipse class="wet" cx="0" cy="5" rx="6.4" ry="3.6" fill="#8fd3ff" opacity="0"/>
        <path class="lid" d="M-9 2 Q0 7 9 2 L9 12 L-9 12 Z" fill="#fbf4ec" opacity="0"/>
        <path class="happy" d="M-7 1 Q0 -8 7 1" fill="none" stroke="#2b2420" stroke-width="3" stroke-linecap="round" opacity="0"/>
        <path class="tear" d="M5 6 q4 7 0 9 q-4 -2 0 -9 z" fill="#8fd3ff" opacity="0"/>
      </g>
      <g class="eye eye-r" transform="translate(78 56)">
        <g class="pupil"><ellipse rx="6.2" ry="8.2" fill="#2b2420"/><circle cx="-2.2" cy="-3.2" r="2" fill="#fff"/><circle cx="2" cy="3.5" r="0.9" fill="#fff" opacity="0.7"/></g>
        <ellipse class="wet" cx="0" cy="5" rx="6.4" ry="3.6" fill="#8fd3ff" opacity="0"/>
        <path class="lid" d="M-9 2 Q0 7 9 2 L9 12 L-9 12 Z" fill="#fbf4ec" opacity="0"/>
        <path class="happy" d="M-7 1 Q0 -8 7 1" fill="none" stroke="#2b2420" stroke-width="3" stroke-linecap="round" opacity="0"/>
        <path class="tear" d="M5 6 q4 7 0 9 q-4 -2 0 -9 z" fill="#8fd3ff" opacity="0"/>
      </g>
    </g>
    <g class="mouth" transform="translate(60 76)">
      <path class="m-idle" d="M-3 0 q3 2 6 0" fill="none" stroke="#9b7b6a" stroke-width="1.6" stroke-linecap="round"/>
      <path class="m-smile" d="M-6 -1 q6 7 12 0" fill="none" stroke="#9b7b6a" stroke-width="1.8" stroke-linecap="round" opacity="0"/>
      <path class="m-big" d="M-7 -1 q7 10 14 0 z" fill="#c0695f" opacity="0"/>
      <path class="m-wobble" d="M-7 1 q2 -3 4 0 t4 0 t4 0 t3 0" fill="none" stroke="#9b7b6a" stroke-width="1.6" stroke-linecap="round" opacity="0"/>
      <circle class="m-o" r="2.4" fill="#b97b6f" opacity="0"/>
    </g>
    <g class="sparkle" opacity="0"><path d="M0 -6 L1.5 -1.5 L6 0 L1.5 1.5 L0 6 L-1.5 1.5 L-6 0 L-1.5 -1.5 Z" fill="#ffd166"/></g>
    <g class="heart" opacity="0"><path d="M0 4 C -8 -4 -4 -12 0 -8 C 4 -12 8 -4 0 4 Z" fill="#ff5f7a"/></g>
    <g transform="translate(114 52)"><g class="thumb"><ellipse cx="-4" cy="18" rx="9" ry="6.5" fill="#f8efe6" stroke="#e4d4c4" stroke-width="1"/><rect x="-6" y="3" width="16" height="13" rx="5" fill="#f8efe6" stroke="#e4d4c4" stroke-width="1"/><rect x="2" y="-9" width="6" height="14" rx="3" fill="#f8efe6" stroke="#e4d4c4" stroke-width="1"/></g></g>
  </g>
</svg>`;

export class SvgMochiView implements MochiView {
  readonly root: HTMLElement;
  private readonly svg: SVGSVGElement;
  private readonly bubbleEl: HTMLDivElement;
  private readonly confirmEl: HTMLDivElement;
  private readonly pupils: SVGGElement[];
  private state: CharacterState = "idle";
  private gestureTimer: number | null = null;
  private bubbleTimer: number | null = null;
  private onAnswer: ((yes: boolean) => void) | null = null;

  constructor(parent: HTMLElement, scale = 0.8) {
    this.root = document.createElement("div");
    this.root.className = "mochi state-idle";
    this.root.style.setProperty("--mochi-scale", String(scale));
    this.root.innerHTML = `
      <div class="bubble" hidden><span class="bubble-text"></span><span class="dots"><i></i><i></i><i></i></span></div>
      <div class="confirm" hidden><button class="yes">Yes</button><button class="no">No</button></div>
      <div class="body-wrap">${SVG}</div>`;
    parent.appendChild(this.root);
    this.svg = this.root.querySelector("svg")!;
    this.bubbleEl = this.root.querySelector(".bubble")!;
    this.confirmEl = this.root.querySelector(".confirm")!;
    this.pupils = Array.from(this.svg.querySelectorAll<SVGGElement>(".pupil"));
    this.confirmEl.querySelector(".yes")!.addEventListener("click", () => this.onAnswer?.(true));
    this.confirmEl.querySelector(".no")!.addEventListener("click", () => this.onAnswer?.(false));
  }

  setState(state: CharacterState) {
    if (state === this.state) return;
    this.root.classList.remove(`state-${this.state}`);
    this.state = state;
    this.root.classList.add(`state-${state}`);
  }

  setGaze(dx: number, dy: number) {
    const cx = Math.max(-1, Math.min(1, dx)) * 3.2;
    const cy = Math.max(-1, Math.min(1, dy)) * 3.0;
    for (const p of this.pupils) p.setAttribute("transform", `translate(${cx.toFixed(2)} ${cy.toFixed(2)})`);
  }

  gesture(g: ViewGesture) {
    const cls = `g-${g}`;
    // Restart the animation even if the same gesture repeats.
    this.root.classList.remove(cls);
    void this.root.offsetWidth;
    this.root.classList.add(cls);
    const sticky = g === "look_up" || g === "type";
    if (this.gestureTimer) window.clearTimeout(this.gestureTimer);
    if (!sticky) {
      const ms = g === "heart" ? 1300 : g === "thumbs_up" ? 900 : g === "wobble" ? 700 : g === "click" ? 320 : g === "nod" ? 520 : 600;
      this.gestureTimer = window.setTimeout(() => this.root.classList.remove(cls), ms);
    }
    if (g !== "look_up") this.root.classList.remove("g-look_up");
    if (g !== "type") this.root.classList.remove("g-type");
  }

  bubble(text: string | null, variant: "speech" | "thinking") {
    if (this.bubbleTimer) window.clearTimeout(this.bubbleTimer);
    if (text === null) {
      // Hide only if the bubble currently shows this variant.
      if (this.bubbleEl.dataset.variant === variant || variant === "speech") {
        this.bubbleEl.hidden = true;
      }
      return;
    }
    this.bubbleEl.hidden = false;
    this.bubbleEl.dataset.variant = variant;
    this.bubbleEl.querySelector(".bubble-text")!.textContent = text;
    if (variant === "speech") {
      const ms = Math.max(1800, 900 + text.length * 70);
      this.bubbleTimer = window.setTimeout(() => {
        if (this.state !== "confirm" && this.state !== "lost") this.bubbleEl.hidden = true;
      }, ms);
    }
  }

  showConfirm(text: string | null, onAnswer: (yes: boolean) => void) {
    this.onAnswer = onAnswer;
    this.confirmEl.hidden = text === null;
  }

  bodyRect(): DOMRect {
    return (this.svg.querySelector(".body") as SVGGraphicsElement).getBoundingClientRect();
  }
}
