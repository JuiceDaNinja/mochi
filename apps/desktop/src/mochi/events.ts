// Mirrors crates/mochi-core/src/runtime.rs. Keep the two in sync.

export type Bounds = { x: number; y: number; w: number; h: number };

export type CharacterState =
  | "idle"
  | "listening"
  | "thinking"
  | "working"
  | "success"
  | "lost"
  | "confirm"
  | "learning";

export type DoneKind = "heart" | "thumbs_up";
export type EngineGesture = "click" | "type";

export type CharacterEvent =
  | { type: "wake" }
  | { type: "thinking"; on: boolean }
  | { type: "narrate"; text: string }
  | { type: "move_to"; bounds: Bounds; monitor_id: number }
  | { type: "gesture"; gesture: EngineGesture }
  | { type: "lost"; looking_for: string }
  | { type: "learning" }
  | { type: "confirm_request"; text: string }
  | { type: "confirm_answered"; yes: boolean }
  | { type: "done"; kind: DoneKind; summary: string }
  | { type: "say"; text: string }
  | { type: "cancel" };

/** Fixed lines, copied from runtime.rs so the browser demo speaks the same words. */
export const LINES = {
  wake: ["Mm? I'm here!", "Yes! What should I do?", "Listening…"],
  success: ["Done!", "All sent! Yay!", "Finished! That was fun.", "Ta-da!"],
  lost: [
    "Mochi… can't find it…",
    "Where is it… I looked everywhere…",
    "I'm sorry… can you show me?",
    "It's not here… hmm… hmm…",
  ],
  learning: ["Oh! There it is! Thank you!", "I'll remember that one!", "Got it, got it. Next time I'll know."],
  interrupted: ["Oh! Stopping.", "Okay, I'll wait."],
} as const;

export function pick<T>(list: readonly T[], seed = Math.random()): T {
  return list[Math.floor(seed * list.length) % list.length];
}
