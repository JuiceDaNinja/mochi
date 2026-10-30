// Voice pipeline. Production: Mochi's own voice from the local Kokoro sidecar (tts/), reached
// through HostedTtsVoice as POST /tts → audio, with the fixed library cached. Fallback while the
// model downloads, and in builds without the sidecar: the browser's own speech synthesis,
// pitched up, or captions only. Narration respects the toggle; fixed lines (always=true) do not.

export interface Voice {
  speak(text: string, opts: { always: boolean; interrupt?: boolean }): void;
  stop(): void;
  setNarration(on: boolean): void;
  narration(): boolean;
}

/** Sound effects: short synthesized blips so no audio assets are needed yet. */
export class Sfx {
  private ctx: AudioContext | null = null;
  enabled = true;

  play(name: "pop" | "chime" | "sad" | "sparkle" | "chirp") {
    if (!this.enabled) return;
    try {
      this.ctx ??= new (window.AudioContext || (window as any).webkitAudioContext)();
      const c = this.ctx;
      const t = c.currentTime;
      const tone = (f0: number, f1: number, dur: number, type: OscillatorType = "sine", gain = 0.08, delay = 0) => {
        const o = c.createOscillator();
        const g = c.createGain();
        o.type = type;
        o.frequency.setValueAtTime(f0, t + delay);
        o.frequency.exponentialRampToValueAtTime(f1, t + delay + dur);
        g.gain.setValueAtTime(0.0001, t + delay);
        g.gain.exponentialRampToValueAtTime(gain, t + delay + 0.01);
        g.gain.exponentialRampToValueAtTime(0.0001, t + delay + dur);
        o.connect(g).connect(c.destination);
        o.start(t + delay);
        o.stop(t + delay + dur + 0.02);
      };
      switch (name) {
        case "pop": tone(520, 260, 0.08, "triangle", 0.06); break;
        case "chirp": tone(880, 1320, 0.12, "sine", 0.05); break;
        case "chime": tone(880, 880, 0.18, "sine", 0.06); tone(1320, 1320, 0.22, "sine", 0.05, 0.12); break;
        case "sparkle": tone(1568, 2093, 0.12, "sine", 0.04); tone(2093, 2637, 0.14, "sine", 0.035, 0.1); break;
        case "sad": tone(440, 300, 0.35, "sine", 0.05); break;
      }
    } catch {
      /* no audio in this context */
    }
  }
}

export class WebSpeechVoice implements Voice {
  private on = true;
  private voice: SpeechSynthesisVoice | null = null;
  constructor() {
    const choose = () => {
      const voices = window.speechSynthesis?.getVoices?.() ?? [];
      // Prefer a female English voice; names vary per OS.
      const prefs = [/zira/i, /aria/i, /jenny/i, /samantha/i, /karen/i, /google uk english female/i, /female/i, /en-US/i, /en/i];
      for (const p of prefs) {
        const v = voices.find((v) => p.test(`${v.name} ${v.lang}`));
        if (v) { this.voice = v; break; }
      }
    };
    choose();
    window.speechSynthesis?.addEventListener?.("voiceschanged", choose);
  }
  speak(text: string, opts: { always: boolean; interrupt?: boolean }) {
    if (!opts.always && !this.on) return;
    const synth = window.speechSynthesis;
    if (!synth) return;
    if (opts.interrupt !== false) synth.cancel();
    const u = new SpeechSynthesisUtterance(text);
    if (this.voice) u.voice = this.voice;
    u.pitch = 1.5; // bright, anime-leaning
    u.rate = 1.08;
    u.volume = 0.9;
    synth.speak(u);
  }
  stop() { window.speechSynthesis?.cancel(); }
  setNarration(on: boolean) { this.on = on; }
  narration() { return this.on; }
}

/**
 * HTTP TTS: POST { text } to `endpoint`, play the audio that comes back. Today the endpoint
 * is the local sidecar (http://127.0.0.1:<port>/tts, WAV); any failure — sidecar still
 * downloading its model, not bundled, crashed — goes to `fallback` so Mochi always speaks.
 */
export class HostedTtsVoice implements Voice {
  private on = true;
  private cache = new Map<string, string>();
  private current: HTMLAudioElement | null = null;
  constructor(private endpoint: string, private fallback: Voice = new CaptionOnlyVoice()) {}
  async speak(text: string, opts: { always: boolean; interrupt?: boolean }) {
    if (!opts.always && !this.on) return;
    if (opts.interrupt !== false) this.stop();
    try {
      let url = this.cache.get(text);
      if (!url) {
        const r = await fetch(this.endpoint, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ text }) });
        if (!r.ok) throw new Error(`tts ${r.status}`);
        url = URL.createObjectURL(await r.blob());
        if (text.length < 60) this.cache.set(text, url); // the fixed library stays cached
      }
      this.current = new Audio(url);
      await this.current.play();
    } catch {
      this.fallback.speak(text, opts);
    }
  }
  stop() { this.current?.pause(); this.current = null; this.fallback.stop(); }
  setNarration(on: boolean) { this.on = on; this.fallback.setNarration(on); }
  narration() { return this.on; }
}

/** Offline: the bubble already shows the words; this just keeps the interface honest. */
export class CaptionOnlyVoice implements Voice {
  private on = true;
  speak() {}
  stop() {}
  setNarration(on: boolean) { this.on = on; }
  narration() { return this.on; }
}
