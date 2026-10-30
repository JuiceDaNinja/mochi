import "./styles.css";
import { Bridge, BrowserBridge, TauriBridge, hasTauri } from "./mochi/bridge";
import { MochiModel, Effect } from "./mochi/model";
import { Mover, gazeToward } from "./mochi/motion";
import { loadRiveView } from "./mochi/rive";
import { MochiView, SvgMochiView } from "./mochi/view";
import { Sfx, Voice, WebSpeechVoice } from "./mochi/voice";

async function boot() {
  const bridge: Bridge = hasTauri() ? new TauriBridge() : new BrowserBridge();
  window.addEventListener("error", (e) => void bridge.log(`character window error: ${e.message} @${e.filename}:${e.lineno}`));
  window.addEventListener("unhandledrejection", (e) => void bridge.log(`character window rejection: ${String((e as PromiseRejectionEvent).reason)}`));
  const stage = document.getElementById("stage")!;
  const initial = await bridge.initialSettings();
  const scale = Number(new URLSearchParams(location.search).get("scale") ?? (bridge.kind === "browser" ? 1.1 : initial?.scale ?? 0.8));

  const view: MochiView = (await loadRiveView(stage)) ?? new SvgMochiView(stage, scale);
  const model = new MochiModel();
  const voice: Voice = new WebSpeechVoice();
  const sfx = new Sfx();
  const mon = await bridge.monitor();

  const mover = new Mover(
    view.root,
    () => ({ w: view.root.offsetWidth, h: view.root.offsetHeight }),
    () => ({ w: window.innerWidth, h: window.innerHeight }),
    () => view.bodyRect().height,
  );
  mover.monitorOrigin = { x: mon.x, y: mon.y };

  // --- Effects -> view / voice / motion ---
  const run = (fx: Effect[]) => {
    for (const f of fx) {
      switch (f.kind) {
        case "speak": voice.speak(f.text, { always: f.always }); break;
        case "bubble": view.bubble(f.text, f.variant); break;
        case "gesture": view.gesture(f.gesture); break;
        case "sound": sfx.play(f.name); break;
        case "move": f.target === "home" ? mover.goHome() : mover.goBeside(f.target); break;
        case "confirm": view.showConfirm(f.text, (yes) => void bridge.answer(yes)); break;
      }
    }
    view.setState(model.state);
  };

  bridge.onEvent((ev) => run(model.apply(ev, performance.now())));

  // Click Mochi while idle: a small menu (demo scenarios, narration, stop).
  const menu = buildMenu(bridge, voice, model);
  view.root.appendChild(menu.el);

  // Timers (success -> home, learning -> working) and body-rect reporting. The shell uses
  // the rect to decide where the window stops being click-through, so include the menu.
  let lastRect = "";
  const frame = () => {
    run(model.tick(performance.now()));
    const r = view.bodyRect();
    let box = { x: r.x, y: r.y, w: r.width, h: r.height };
    // Anything clickable that hangs above the body must be inside the hit rectangle too:
    // the menu, the Yes/No buttons, the bubble they sit under.
    for (const sel of [".menu", ".confirm", ".bubble"]) {
      const el = view.root.querySelector<HTMLElement>(sel);
      if (!el || el.hidden) continue;
      const m = el.getBoundingClientRect();
      if (m.width === 0) continue;
      const x1 = Math.min(box.x, m.x), y1 = Math.min(box.y, m.y);
      const x2 = Math.max(box.x + box.w, m.x + m.width), y2 = Math.max(box.y + box.h, m.y + m.height);
      box = { x: x1, y: y1, w: x2 - x1, h: y2 - y1 };
    }
    const key = `${box.x | 0},${box.y | 0},${box.w | 0},${box.h | 0}`;
    if (key !== lastRect) {
      lastRect = key;
      bridge.reportBodyRect(box);
    }
    requestAnimationFrame(frame);
  };
  requestAnimationFrame(frame);

  // --- Eyes follow the cursor (with a small lag so it feels alive) ---
  let target = { x: 0, y: 0 };
  let gaze = { x: 0, y: 0 };
  bridge.onCursor((c) => {
    if (model.state === "confirm") { target = { x: 0, y: -1 }; return; } // keep looking up at the user
    const r = view.bodyRect();
    const eye = { x: r.x + r.width / 2, y: r.y + r.height * 0.45 };
    target = gazeToward(eye, { x: c.x - mon.x, y: c.y - mon.y });
  });
  const eyeLoop = () => {
    gaze = { x: gaze.x + (target.x - gaze.x) * 0.18, y: gaze.y + (target.y - gaze.y) * 0.18 };
    view.setGaze(gaze.x, gaze.y);
    requestAnimationFrame(eyeLoop);
  };
  requestAnimationFrame(eyeLoop);

  // --- Peek: cursor resting on Mochi; click opens the menu ---
  const bodyWrap = view.root.querySelector(".body-wrap")!;
  bodyWrap.addEventListener("mouseenter", () => { if (model.state === "idle") view.gesture("hop"); });
  bodyWrap.addEventListener("click", (e) => {
    e.stopPropagation();
    if (model.state === "idle") menu.toggle();
  });
  // Teaching: while Lost, the user clicks the thing they want Mochi to find.
  bridge.onGlobalClick(() => { if (model.state === "lost") void bridge.teach(); });

  // A phone asks to pair: Mochi shows the code and waits for a yes on this side too.
  bridge.onPairRequest((r) => {
    const code = r.code.replace(/^(\d{3})(\d{3})$/, "$1 $2");
    const text = `Phone “${r.device_name}” wants to connect.\nSame code on the phone? ${code}`;
    view.bubble(text, "speech");
    voice.speak(`A phone wants to connect. Is the code ${code.split("").join(" ")}?`, { always: true });
    view.gesture("look_up");
    view.showConfirm(text, (yes) => {
      view.showConfirm(null, () => {});
      view.bubble(yes ? "Yay, we're friends now!" : "Okay, not this one.", "speech");
      voice.speak(yes ? "Yay, we're friends now!" : "Okay, not this one.", { always: true });
      void bridge.pairAnswer(r.session, yes);
    });
  });
  bridge.onInterrupt(() => voice.stop());

  // Live settings: size and narration.
  if (initial) voice.setNarration(initial.narration);
  bridge.onSettings((s) => {
    voice.setNarration(s.narration);
    view.root.style.setProperty("--mochi-scale", String(s.scale));
    if (model.state === "idle") mover.goHome();
  });

  // Keyboard: Y / N answer a confirm, Esc cancels.
  window.addEventListener("keydown", (e) => {
    if (model.state === "confirm" && (e.key === "y" || e.key === "Y")) void bridge.answer(true);
    if (model.state === "confirm" && (e.key === "n" || e.key === "N")) void bridge.answer(false);
    if (e.key === "Escape") void bridge.cancel();
  });

  window.addEventListener("resize", () => { if (model.state === "idle") mover.goHome(); });

  if (bridge.kind === "browser") devPanel(bridge as BrowserBridge, voice, model);
  (window as any).mochi = { model, view, bridge, mover, voice };
}

/** The in-character menu: shown when Mochi is clicked while idle. Works in both shells. */
function buildMenu(bridge: Bridge, voice: Voice, model: MochiModel) {
  const el = document.createElement("div");
  el.className = "menu";
  el.hidden = true;
  el.innerHTML = `
    <form class="ask"><input type="text" placeholder="Ask Mochi…" aria-label="Ask Mochi" autocomplete="off"><button type="submit">Go</button></form>
    <div class="menu-sep">Demos</div>
    <button data-s="gmail">Reply to Sara</button>
    <button data-s="gmail_lost">Same, but I get lost</button>
    <button data-s="close_unsaved">Close an unsaved note</button>
    <div class="menu-sep"></div>
    <label><input type="checkbox" id="menu-narr" checked> Talk while working</label>
    <button data-x="settings">Settings…</button>
    <button data-x="stop">Stop</button>`;
  const close = () => { el.hidden = true; };
  const form = el.querySelector<HTMLFormElement>("form.ask")!;
  const input = form.querySelector("input")!;
  form.addEventListener("submit", (e) => {
    e.preventDefault();
    const task = input.value.trim();
    if (!task) return;
    input.value = "";
    close();
    void bridge.startTask(task);
  });
  el.querySelectorAll<HTMLButtonElement>("button[data-s]").forEach((b) =>
    b.addEventListener("click", () => { close(); void bridge.startTask("demo", b.dataset.s!); }),
  );
  el.querySelector<HTMLButtonElement>('button[data-x="settings"]')!.addEventListener("click", () => { close(); void bridge.openSettings(); });
  el.querySelector<HTMLButtonElement>('button[data-x="stop"]')!.addEventListener("click", () => { close(); void bridge.cancel(); });
  el.querySelector<HTMLInputElement>("#menu-narr")!.addEventListener("change", (e) => {
    const on = (e.target as HTMLInputElement).checked;
    voice.setNarration(on);
    void bridge.setNarration(on);
  });
  window.addEventListener("mousedown", (e) => { if (!el.hidden && !(e.target as HTMLElement).closest(".menu, .body-wrap")) close(); });
  return { el, toggle: () => { if (model.state !== "idle") return; el.hidden = !el.hidden; if (!el.hidden) setTimeout(() => input.focus(), 50); } };
}

/** Browser-only controls for designing the character. Hidden under Tauri. */
function devPanel(bridge: BrowserBridge, voice: Voice, model: MochiModel) {
  document.body.classList.add("dev");
  const panel = document.createElement("div");
  panel.className = "dev-panel";
  panel.innerHTML = `
    <button data-s="gmail">▶ Gmail reply</button>
    <button data-s="gmail_lost">▶ Gmail (gets lost)</button>
    <button data-s="close_unsaved">▶ Close unsaved</button>
    <button data-x="cancel">■ Stop</button>
    <label><input type="checkbox" id="narr" checked> narration</label>
    <span class="hint">Click anywhere when Mochi is lost to show it the button. Y / N answer a confirm.</span>`;
  document.body.appendChild(panel);
  // Fake on-screen targets for the browser demo.
  const place = (name: string, x: number, y: number, w: number, h: number) => {
    bridge.targets[name] = { x, y, w, h };
    const d = document.createElement("div");
    d.className = "fake-target";
    Object.assign(d.style, { left: `${x}px`, top: `${y}px`, width: `${w}px`, height: `${h}px`, display: "block" });
    d.title = name;
    document.body.appendChild(d);
  };
  const W = window.innerWidth, H = window.innerHeight;
  place("tab", W * 0.12, 80, 200, 32);
  place("thread", W * 0.3, H * 0.3, W * 0.5, 44);
  place("reply", W * 0.3, H * 0.62, 90, 36);
  place("body", W * 0.3, H * 0.45, W * 0.5, 120);
  place("send", W * 0.3, H * 0.74, 90, 36);
  place("close", W - 60, 70, 28, 28);

  panel.querySelectorAll<HTMLButtonElement>("button[data-s]").forEach((b) =>
    b.addEventListener("click", () => { if (model.state === "idle") void bridge.startTask("demo", b.dataset.s!); }),
  );
  panel.querySelector<HTMLButtonElement>("button[data-x]")!.addEventListener("click", () => void bridge.cancel());
  panel.querySelector<HTMLInputElement>("#narr")!.addEventListener("change", (e) => {
    const on = (e.target as HTMLInputElement).checked;
    voice.setNarration(on);
    void bridge.setNarration(on);
  });
}

boot().catch((e) => { console.error(e); document.body.insertAdjacentHTML("beforeend", `<pre style="color:#a04050;background:#fff;padding:8px">${String(e)}</pre>`); });
