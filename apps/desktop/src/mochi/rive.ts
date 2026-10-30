// Rive integration point. When `public/mochi.riv` exists with a state machine named
// "Mochi", this view replaces SvgMochiView. Inputs the .riv must expose (model.ts mirrors them):
//
//   number  state      0 idle · 1 listening · 2 working · 3 success · 4 lost · 5 confirm · 6 learning
//   number  gazeX      -1..1       number  gazeY  -1..1
//   trigger heart · thumbsUp · click · nod · wobble · hop · lookUp
//   boolean typing · thinking
//
// Until the asset exists, loadRiveView() resolves to null and main.ts falls back to SVG.

import { CharacterState } from "./events";
import { ViewGesture } from "./model";
import { MochiView } from "./view";

const STATE_INDEX: Record<CharacterState, number> = { idle: 0, listening: 1, thinking: 2, working: 2, success: 3, lost: 4, confirm: 5, learning: 6 };

export async function loadRiveView(parent: HTMLElement, src = "/mochi.riv"): Promise<MochiView | null> {
  try {
    const head = await fetch(src, { method: "HEAD" });
    if (!head.ok || !(head.headers.get("content-type") ?? "").includes("octet")) return null;
    const rive = await import("@rive-app/canvas");
    return await new Promise<MochiView | null>((resolve) => {
      const root = document.createElement("div");
      root.className = "mochi state-idle";
      root.innerHTML = `<div class="bubble" hidden><span class="bubble-text"></span><span class="dots"><i></i><i></i><i></i></span></div>
        <div class="confirm" hidden><button class="yes">Yes</button><button class="no">No</button></div>
        <div class="body-wrap"><canvas width="240" height="224" style="width:120px;height:112px"></canvas></div>`;
      parent.appendChild(root);
      const canvas = root.querySelector("canvas")!;
      const r = new rive.Rive({
        src,
        canvas,
        autoplay: true,
        stateMachines: "Mochi",
        onLoad: () => {
          const inputs = r.stateMachineInputs("Mochi") ?? [];
          const byName = (n: string) => inputs.find((i) => i.name === n);
          let onAnswer: ((yes: boolean) => void) | null = null;
          root.querySelector(".yes")!.addEventListener("click", () => onAnswer?.(true));
          root.querySelector(".no")!.addEventListener("click", () => onAnswer?.(false));
          const bubbleEl = root.querySelector<HTMLDivElement>(".bubble")!;
          resolve({
            root,
            setState(s) { const i = byName("state"); if (i) i.value = STATE_INDEX[s]; },
            setGaze(x, y) { const gx = byName("gazeX"); const gy = byName("gazeY"); if (gx) gx.value = x; if (gy) gy.value = y; },
            gesture(g: ViewGesture) {
              if (g === "type") { const t = byName("typing"); if (t) t.value = true; return; }
              const name = g === "thumbs_up" ? "thumbsUp" : g === "look_up" ? "lookUp" : g;
              byName(name)?.fire?.();
              const t = byName("typing"); if (t) t.value = false;
            },
            bubble(text, variant) {
              bubbleEl.hidden = text === null;
              if (text !== null) { bubbleEl.dataset.variant = variant; bubbleEl.querySelector(".bubble-text")!.textContent = text; }
              const th = byName("thinking"); if (th) th.value = variant === "thinking" && text !== null;
            },
            showConfirm(text, cb) { onAnswer = cb; root.querySelector<HTMLDivElement>(".confirm")!.hidden = text === null; },
            bodyRect: () => canvas.getBoundingClientRect(),
          });
        },
        onLoadError: () => resolve(null),
      });
    });
  } catch {
    return null;
  }
}
