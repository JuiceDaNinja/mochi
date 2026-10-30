import { describe, expect, it } from "vitest";
import { MochiModel, Effect } from "./model";
import { CharacterEvent } from "./events";

const kinds = (fx: Effect[]) => fx.map((f) => (f.kind === "gesture" ? `gesture:${f.gesture}` : f.kind));

function run(events: Array<[number, CharacterEvent]>, until = 10_000) {
  const m = new MochiModel(() => 0);
  const trace: Array<{ t: number; state: string; fx: string[] }> = [];
  let t = 0;
  for (const [at, ev] of events) {
    while (t < at) {
      t += 100;
      const fx = m.tick(t);
      if (fx.length) trace.push({ t, state: m.state, fx: kinds(fx) });
    }
    const fx = m.apply(ev, t);
    trace.push({ t, state: m.state, fx: kinds(fx) });
  }
  while (t < until) {
    t += 100;
    const fx = m.tick(t);
    if (fx.length) trace.push({ t, state: m.state, fx: kinds(fx) });
  }
  return { m, trace };
}

describe("Mochi state machine", () => {
  it("runs the Gmail reply: wake → working → confirm → success → home", () => {
    const { m, trace } = run([
      [0, { type: "wake" }],
      [500, { type: "say", text: "Okay! On it!" }],
      [600, { type: "thinking", on: true }],
      [1500, { type: "thinking", on: false }],
      [1500, { type: "narrate", text: "Opening Gmail…" }],
      [1500, { type: "move_to", bounds: { x: 10, y: 10, w: 50, h: 20 }, monitor_id: 0 }],
      [2000, { type: "gesture", gesture: "click" }],
      [3000, { type: "confirm_request", text: "Send it?" }],
      [5000, { type: "confirm_answered", yes: true }],
      [5500, { type: "done", kind: "heart", summary: "sent" }],
    ]);
    const states = trace.map((x) => x.state);
    expect(states[0]).toBe("listening");
    expect(states).toContain("working");
    expect(states).toContain("confirm");
    expect(states).toContain("success");
    expect(m.state).toBe("idle");
    const last = trace.at(-1)!;
    expect(last.fx).toContain("move");
    const doneStep = trace.find((x) => x.fx.includes("gesture:heart"))!;
    expect(doneStep.fx).toContain("gesture:hop");
    expect(doneStep.fx).toContain("speak");
  });

  it("confirm shows the fallback buttons and hides them on answer", () => {
    const m = new MochiModel(() => 0);
    const fx = m.apply({ type: "confirm_request", text: "Send it?" }, 0);
    expect(fx).toContainEqual({ kind: "confirm", text: "Send it?" });
    expect(fx).toContainEqual({ kind: "gesture", gesture: "look_up" });
    expect(m.confirmText).toBe("Send it?");
    const fx2 = m.apply({ type: "confirm_answered", yes: false }, 1);
    expect(fx2).toContainEqual({ kind: "confirm", text: null });
    expect(m.state).toBe("confirm"); // stays until the engine's say + cancel arrive
  });

  it("lost wobbles, then looks up, then learning nods and returns to working", () => {
    const { trace, m } = run([
      [0, { type: "lost", looking_for: "the reply button" }],
      [3000, { type: "learning" }],
    ], 5000);
    expect(trace[0].state).toBe("lost");
    expect(trace[0].fx).toContain("gesture:wobble");
    expect(trace[0].fx).toContain("sound");
    const lookUp = trace.find((x) => x.fx.includes("gesture:look_up"))!;
    expect(lookUp.t).toBeGreaterThanOrEqual(700);
    const learn = trace.find((x) => x.state === "learning")!;
    expect(learn.fx).toContain("gesture:nod");
    expect(m.state).toBe("working");
    expect(m.lookingFor).toBeNull();
  });

  it("narration is speak(always=false); fixed lines are always=true", () => {
    const m = new MochiModel(() => 0);
    const n = m.apply({ type: "narrate", text: "Typing now…" }, 0).find((f) => f.kind === "speak")!;
    expect(n).toMatchObject({ always: false });
    const c = m.apply({ type: "confirm_request", text: "Send it?" }, 0).find((f) => f.kind === "speak")!;
    expect(c).toMatchObject({ always: true });
  });

  it("cancel clears everything and goes home after the closing line", () => {
    const { m, trace } = run([
      [0, { type: "confirm_request", text: "Send it?" }],
      [1000, { type: "confirm_answered", yes: false }],
      [1000, { type: "say", text: "Phew, okay. Not sending." }],
      [1000, { type: "cancel" }],
    ], 4000);
    expect(m.state).toBe("idle");
    expect(m.confirmText).toBeNull();
    expect(trace.at(-1)!.fx).toContain("move");
  });

  it("thinking overlays working and clears on done", () => {
    const m = new MochiModel(() => 0);
    m.apply({ type: "thinking", on: true }, 0);
    expect(m.state).toBe("working");
    expect(m.thinking).toBe(true);
    m.apply({ type: "done", kind: "thumbs_up", summary: "x" }, 0);
    expect(m.thinking).toBe(false);
    expect(m.state).toBe("success");
  });
});
