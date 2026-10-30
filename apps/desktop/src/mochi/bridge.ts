// How the character window talks to the engine. Under Tauri, the Rust side runs the
// task loop and emits events; in a plain browser (vite dev), a scripted runner stands in
// so the character can be designed and screenshotted without the desktop shell.

import { emit, listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { Bounds, CharacterEvent } from "./events";

export type Cursor = { x: number; y: number };
export type MonitorInfo = { id: number; x: number; y: number; w: number; h: number; scale: number; primary: boolean; work_bottom: number };

export interface Bridge {
  readonly kind: "tauri" | "browser";
  onEvent(cb: (ev: CharacterEvent) => void): void;
  /** Global cursor position in the character window's pixel space. */
  onCursor(cb: (c: Cursor) => void): void;
  /** A mouse click anywhere on the desktop (outside Mochi's own window). */
  onGlobalClick(cb: () => void): void;
  /** Settings changed in the settings window. */
  onSettings(cb: (s: { scale: number; narration: boolean }) => void): void;
  initialSettings(): Promise<{ scale: number; narration: boolean } | null>;
  /** A phone wants to pair; show the code and ask. */
  onPairRequest(cb: (r: { session: number; device_name: string; code: string }) => void): void;
  pairAnswer(session: number, yes: boolean): Promise<void>;
  /** The user started speaking on the phone: stop talking. */
  onInterrupt(cb: () => void): void;
  /** Append a line to the desktop's mochi.log (release builds have no console). */
  log(line: string): Promise<void>;
  /** Close Mochi completely (tells the phone first). */
  quit(): Promise<void>;
  /** Real task from the user's words; `scenario` runs a scripted demo instead. */
  startTask(task: string, scenario?: string): Promise<void>;
  openSettings(): Promise<void>;
  answer(yes: boolean): Promise<void>;
  /** The user pointed at something while Mochi was lost. Browser: a fake element id. */
  teach(elementId?: number): Promise<void>;
  setNarration(on: boolean): Promise<void>;
  cancel(): Promise<void>;
  /** Tell the shell where Mochi's body is so it can toggle click-through. */
  reportBodyRect(r: { x: number; y: number; w: number; h: number }): void;
  monitor(): Promise<MonitorInfo>;
}

export function hasTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export class TauriBridge implements Bridge {
  readonly kind = "tauri" as const;
  onEvent(cb: (ev: CharacterEvent) => void) {
    void listen<CharacterEvent>("mochi://event", (e) => cb(e.payload));
  }
  onCursor(cb: (c: Cursor) => void) {
    void listen<Cursor>("mochi://cursor", (e) => cb(e.payload));
  }
  onGlobalClick(cb: () => void) { void listen("mochi://global-click", () => cb()); }
  onSettings(cb: (s: { scale: number; narration: boolean }) => void) { void listen<{ scale: number; narration: boolean }>("mochi://settings", (e) => cb(e.payload)); }
  initialSettings() { return invoke<{ scale: number; narration: boolean }>("get_settings"); }
  onPairRequest(cb: (r: { session: number; device_name: string; code: string }) => void) { void listen<{ session: number; device_name: string; code: string }>("mochi://pair-request", (e) => cb(e.payload)); }
  pairAnswer(session: number, yes: boolean) { return invoke<void>("pair_answer", { session, yes }); }
  onInterrupt(cb: () => void) { void listen("mochi://interrupt", () => cb()); }
  log(line: string) { return invoke<void>("log_js", { line }).catch(() => {}); }
  quit() { return invoke<void>("quit_app"); }
  startTask(task: string, scenario?: string) { return invoke<void>("start_task", { task, scenario: scenario ?? null }); }
  openSettings() { return invoke<void>("open_settings"); }
  answer(yes: boolean) { return invoke<void>("answer", { yes }); }
  teach(elementId?: number) { return invoke<void>("teach", { elementId: elementId ?? null }); }
  setNarration(on: boolean) { return invoke<void>("set_narration", { on }); }
  cancel() { return invoke<void>("cancel_task"); }
  reportBodyRect(r: { x: number; y: number; w: number; h: number }) { void emit("mochi://body-rect", r); }
  monitor() { return invoke<MonitorInfo>("primary_monitor"); }
}

/** Dev stand-in: replays the bible's Gmail scenario with the same pauses the engine produces. */
export class BrowserBridge implements Bridge {
  readonly kind = "browser" as const;
  private handlers: Array<(ev: CharacterEvent) => void> = [];
  private waitingConfirm: ((yes: boolean) => void) | null = null;
  private waitingTeach: (() => void) | null = null;
  private cancelled = false;
  private narrationOn = true;
  /** Fake targets so move_to has somewhere to go; dev page draws them. */
  targets: Record<string, Bounds> = {};

  onEvent(cb: (ev: CharacterEvent) => void) { this.handlers.push(cb); }
  onCursor(cb: (c: Cursor) => void) {
    window.addEventListener("mousemove", (e) => cb({ x: e.clientX, y: e.clientY }));
  }
  onGlobalClick(cb: () => void) {
    window.addEventListener("mousedown", (e) => { if (!(e.target as HTMLElement).closest?.(".mochi")) cb(); });
  }
  onSettings() {}
  async initialSettings() { return null; }
  onPairRequest(cb: (r: { session: number; device_name: string; code: string }) => void) {
    (window as any).fakePairRequest = () => cb({ session: 1, device_name: "Pixel 9", code: "482 913" });
  }
  async pairAnswer() {}
  onInterrupt() {}
  async log(line: string) { console.log("[mochi]", line); }
  async quit() { alert("Quit (browser preview: nothing to close)"); }
  async answer(yes: boolean) { this.waitingConfirm?.(yes); }
  async teach() { this.waitingTeach?.(); }
  async setNarration(on: boolean) { this.narrationOn = on; }
  async cancel() { this.cancelled = true; this.waitingConfirm?.(false); this.waitingTeach?.(); }
  reportBodyRect() {}
  async monitor(): Promise<MonitorInfo> {
    return { id: 0, x: 0, y: 0, w: window.innerWidth, h: window.innerHeight, scale: window.devicePixelRatio, primary: true, work_bottom: window.innerHeight };
  }

  private emit(ev: CharacterEvent) { for (const h of this.handlers) h(ev); }
  private sleep(ms: number) { return new Promise((r) => setTimeout(r, ms)); }
  private narrate(text: string) { if (this.narrationOn) this.emit({ type: "narrate", text }); }
  private async think(ms: number) {
    this.emit({ type: "thinking", on: true });
    await this.sleep(ms);
    this.emit({ type: "thinking", on: false });
  }
  private async act(target: string, gesture: "click" | "type", hold = 500) {
    const b = this.targets[target] ?? { x: window.innerWidth / 2, y: window.innerHeight / 2, w: 80, h: 30 };
    this.emit({ type: "move_to", bounds: b, monitor_id: 0 });
    await this.sleep(700);
    this.emit({ type: "gesture", gesture });
    await this.sleep(hold);
  }
  private confirm(text: string): Promise<boolean> {
    this.emit({ type: "confirm_request", text });
    return new Promise((res) => {
      const timer = setTimeout(() => res(false), 20_000);
      this.waitingConfirm = (yes) => { clearTimeout(timer); this.waitingConfirm = null; res(yes); };
    });
  }
  private lost(lookingFor: string): Promise<void> {
    this.emit({ type: "lost", looking_for: lookingFor });
    return new Promise((res) => { this.waitingTeach = () => { this.waitingTeach = null; res(); }; });
  }

  async openSettings() { window.open("/settings.html", "mochi-settings", "width=520,height=640"); }

  async startTask(_task: string, scenario = "gmail") {
    this.cancelled = false;
    this.emit({ type: "wake" });
    await this.sleep(900);
    this.emit({ type: "say", text: "Okay! On it!" });
    await this.sleep(700);

    if (scenario === "close_unsaved") {
      await this.think(800);
      this.narrate("Closing this window…");
      await this.act("close", "click", 0);
      const yes = await this.confirm("This one closes without saving… okay?");
      this.emit({ type: "confirm_answered", yes });
      if (!yes) return this.finishNo();
      this.emit({ type: "say", text: "Here goes!" });
      await this.sleep(600);
      this.emit({ type: "done", kind: "thumbs_up", summary: "Closed" });
      return;
    }

    await this.think(900);
    this.narrate("Opening Gmail…");
    await this.act("tab", "click");
    if (this.cancelled) return;
    await this.think(1000);
    this.narrate("Looking for Sara…");
    await this.act("thread", "click");
    await this.think(700);
    this.narrate("Found it!");
    await this.sleep(600);

    if (scenario === "gmail_lost") {
      await this.think(900);
      this.narrate("Now the reply button…");
      await this.sleep(900);
      await this.lost("the reply button");
      if (this.cancelled) return;
      this.emit({ type: "learning" });
      await this.sleep(1300);
    } else {
      await this.think(800);
      await this.act("reply", "click");
    }
    await this.think(800);
    this.narrate("Typing now…");
    await this.act("body", "type", 1800);
    await this.think(700);
    this.narrate("Almost there…");
    await this.act("send", "click", 0);
    const yes = await this.confirm("Send it?");
    this.emit({ type: "confirm_answered", yes });
    if (!yes) return this.finishNo();
    this.emit({ type: "say", text: "Okay! Sending!" });
    await this.sleep(500);
    this.emit({ type: "gesture", gesture: "click" });
    await this.sleep(600);
    this.emit({ type: "done", kind: "heart", summary: "Reply sent to Sara" });
  }

  private async finishNo() {
    this.emit({ type: "say", text: "Phew, okay. Not sending." });
    await this.sleep(300);
    this.emit({ type: "cancel" });
  }
}
