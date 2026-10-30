// The character's state machine. Pure: events in, state + effects out. No DOM, no audio.
// This is the file the Rive state machine will mirror input-for-input.

import { CharacterEvent, CharacterState, Bounds, LINES, pick } from "./events";

export type ViewGesture = "heart" | "thumbs_up" | "click" | "type" | "nod" | "wobble" | "look_up" | "hop";

export type Effect =
  | { kind: "speak"; text: string; always: boolean } // always=true bypasses the narration toggle
  | { kind: "bubble"; text: string | null; variant: "speech" | "thinking" }
  | { kind: "gesture"; gesture: ViewGesture }
  | { kind: "move"; target: Bounds | "home" }
  | { kind: "sound"; name: "pop" | "chime" | "sad" | "sparkle" | "chirp" }
  | { kind: "confirm"; text: string | null };

export type Snapshot = {
  state: CharacterState;
  thinking: boolean;
  lookingFor: string | null;
  confirmText: string | null;
};

type Timer = { at: number; fire: (m: MochiModel) => Effect[] };

export class MochiModel {
  state: CharacterState = "idle";
  thinking = false;
  lookingFor: string | null = null;
  confirmText: string | null = null;
  private timers: Timer[] = [];
  private seed = 0;

  constructor(private rand: () => number = Math.random) {}

  snapshot(): Snapshot {
    return { state: this.state, thinking: this.thinking, lookingFor: this.lookingFor, confirmText: this.confirmText };
  }

  /** Apply one engine event. `now` is ms, used only to schedule the return-to-idle timers. */
  apply(ev: CharacterEvent, now: number): Effect[] {
    const fx: Effect[] = [];
    const line = (l: readonly string[]) => pick(l, this.rand());

    switch (ev.type) {
      case "wake": {
        this.cancelTimers();
        this.set("listening");
        const t = line(LINES.wake);
        fx.push({ kind: "speak", text: t, always: true }, { kind: "bubble", text: t, variant: "speech" }, { kind: "sound", name: "chirp" });
        break;
      }
      case "thinking": {
        this.thinking = ev.on;
        if (ev.on) {
          if (this.state === "idle" || this.state === "listening") this.set("working");
          fx.push({ kind: "bubble", text: "thinking", variant: "thinking" });
        } else {
          fx.push({ kind: "bubble", text: null, variant: "thinking" });
        }
        break;
      }
      case "say": {
        // Start lines, after-yes/no lines, blocked reasons, sleepy: always spoken.
        if (this.state === "idle" || this.state === "listening") this.set("working");
        fx.push({ kind: "speak", text: ev.text, always: true }, { kind: "bubble", text: ev.text, variant: "speech" });
        break;
      }
      case "narrate": {
        if (this.state !== "confirm" && this.state !== "lost") this.set("working");
        fx.push({ kind: "speak", text: ev.text, always: false }, { kind: "bubble", text: ev.text, variant: "speech" });
        break;
      }
      case "move_to": {
        if (this.state === "idle" || this.state === "listening") this.set("working");
        fx.push({ kind: "move", target: ev.bounds });
        break;
      }
      case "gesture": {
        fx.push({ kind: "gesture", gesture: ev.gesture }, { kind: "sound", name: "pop" });
        break;
      }
      case "lost": {
        this.cancelTimers();
        this.set("lost");
        this.lookingFor = ev.looking_for;
        const t = line(LINES.lost);
        fx.push(
          { kind: "gesture", gesture: "wobble" },
          { kind: "sound", name: "sad" },
          { kind: "speak", text: t, always: true },
          { kind: "bubble", text: t, variant: "speech" },
        );
        // After the wobble, look up at the user and wait.
        this.timers.push({ at: now + 700, fire: () => [{ kind: "gesture", gesture: "look_up" }] });
        break;
      }
      case "learning": {
        this.cancelTimers();
        this.set("learning");
        this.lookingFor = null;
        const t = line(LINES.learning);
        fx.push(
          { kind: "gesture", gesture: "nod" },
          { kind: "sound", name: "sparkle" },
          { kind: "speak", text: t, always: true },
          { kind: "bubble", text: t, variant: "speech" },
        );
        this.timers.push({ at: now + 1100, fire: (m) => { m.set("working"); return []; } });
        break;
      }
      case "confirm_request": {
        this.cancelTimers();
        this.set("confirm");
        this.confirmText = ev.text;
        fx.push(
          { kind: "gesture", gesture: "look_up" },
          { kind: "speak", text: ev.text, always: true },
          { kind: "bubble", text: ev.text, variant: "speech" },
          { kind: "confirm", text: ev.text },
        );
        break;
      }
      case "confirm_answered": {
        this.confirmText = null;
        fx.push({ kind: "confirm", text: null });
        if (ev.yes) {
          this.set("working");
          fx.push({ kind: "gesture", gesture: "nod" });
        }
        // On a no, the engine follows with say + cancel; stay put until then.
        break;
      }
      case "done": {
        this.cancelTimers();
        this.set("success");
        this.thinking = false;
        const t = line(LINES.success);
        fx.push(
          { kind: "bubble", text: null, variant: "thinking" },
          { kind: "speak", text: t, always: true },
          { kind: "bubble", text: t, variant: "speech" },
          { kind: "gesture", gesture: ev.kind === "heart" ? "heart" : "thumbs_up" },
          { kind: "gesture", gesture: "hop" },
          { kind: "sound", name: "chime" },
        );
        this.timers.push({
          at: now + 1600,
          fire: (m) => { m.set("idle"); return [{ kind: "move", target: "home" }, { kind: "bubble", text: null, variant: "speech" }]; },
        });
        break;
      }
      case "cancel": {
        this.cancelTimers();
        this.thinking = false;
        this.lookingFor = null;
        this.confirmText = null;
        // Let a closing line (after-no, blocked, sleepy) stay readable before going home.
        this.timers.push({
          at: now + 1400,
          fire: (m) => { m.set("idle"); return [{ kind: "move", target: "home" }, { kind: "bubble", text: null, variant: "speech" }]; },
        });
        fx.push({ kind: "confirm", text: null }, { kind: "bubble", text: null, variant: "thinking" });
        break;
      }
    }
    return fx;
  }

  /** Call every frame (or at least every 100 ms). Fires due timers. */
  tick(now: number): Effect[] {
    const due = this.timers.filter((t) => t.at <= now);
    if (due.length === 0) return [];
    this.timers = this.timers.filter((t) => t.at > now);
    return due.flatMap((t) => t.fire(this));
  }

  /** The user interrupted by speaking: stop talking, wait. */
  interrupt(): Effect[] {
    const t = pick(LINES.interrupted, this.rand());
    return [{ kind: "speak", text: t, always: true }, { kind: "bubble", text: t, variant: "speech" }];
  }

  private set(s: CharacterState) {
    this.state = s;
  }
  private cancelTimers() {
    this.timers = [];
  }
  // Exposed for deterministic tests.
  _seed() {
    return this.seed++;
  }
}
