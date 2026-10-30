# Mochi

A cute, lovable desktop helper you talk to from your phone. This repo covers **phases 1 and 2**
of the [technical architecture](https://claude.ai/artifact/LCwDxCP59STfvApw4cQi5N): the
character, the task engine, the safety gate, the teaching memory, two real brains (Claude Code
CLI and any local OpenAI-compatible model), and the Windows screen reader + input. Phase 3 adds
the Android app and the phone ↔ desktop link.

```
mochi/
├─ crates/
│  ├─ mochi-core/              Brain trait, prompt + parser, task loop, safety gate, SQLite memory, phone protocol.
│  ├─ mochi-brain-claude/      Claude Code CLI adapter (claude -p, stream-json in/out, resumed session per task).
│  ├─ mochi-brain-local/       OpenAI-compatible adapter for a model on your LAN (Ollama, LM Studio, llama.cpp, vLLM).
│  ├─ mochi-brain-mock/        Scripted scenarios + fake desktop for demos and tests.
│  └─ mochi-platform-windows/  UI Automation tree + GDI screenshot (Perception), SendInput (Actuator). Stubs elsewhere.
└─ apps/
   └─ desktop/           Tauri 2 + Vite/TypeScript. The character window.
      ├─ src/mochi/      model.ts (state machine) · view.ts (SVG Mochi) · motion.ts · voice.ts · bridge.ts · rive.ts
      ├─ src/settings.ts  Settings window: brain picker + test, "Ask Mochi" box, memory list.
      └─ src-tauri/      Rust shell: transparent always-on-top window, cursor + global-click polling, click-through, commands.
```

## Run it on Windows

Prerequisites (one time):

1. [Rust](https://rustup.rs) (stable) and the **Desktop development with C++** workload from Visual Studio Build Tools.
2. [Node.js 22](https://nodejs.org) and pnpm: `corepack enable`.
3. WebView2 is already on Windows 10/11.

Then:

```powershell
pnpm install
pnpm tauri dev          # the character appears at the bottom center of the monitor your cursor is on
```

Click Mochi to open its menu. Type in **Ask Mochi…** (for example "reply to Sara and say I'll be
there at 6" with Gmail in front) and Mochi reads the window through UI Automation, asks the brain
for one step at a time, narrates, and stops to ask "Send it?" before anything irreversible. The
three **Demos** run the scripted Gmail reply on a pretend screen and touch nothing on your PC.
When Mochi cries that it can't find something, click the thing you meant and it remembers.

**Settings…** picks the brain:

- **Claude Code** — needs the CLI installed and signed in once: `npm i -g @anthropic-ai/claude-code`, then run `claude` in any terminal to log in. Mochi runs it headless with no tools and a 1-turn limit; screenshots are attached when a window exposes too few controls.
- **Local AI** — host, port, model name; "Test connection" lists the server's models. Tick "can see images" only for a multimodal model.
- **Demo** — the scripted scenario.

Voice uses the Windows speech voices for now (pitched up); the hosted anime voice is wired in
`voice.ts` behind `HostedTtsVoice` and switches on once the connector exposes `/tts`.

`pnpm tauri build` produces an NSIS installer and an MSI under `apps/desktop/src-tauri/target/release/bundle/`.

## Design the character without the desktop shell

```powershell
pnpm dev                # http://localhost:1420 — same code, a dev panel instead of the menu, fake targets drawn on the page
pnpm test               # vitest (state machine) + cargo test (gate, memory, engine, mock scenarios)
```

## What is real and what is mocked

| Piece | Status |
| --- | --- |
| Character: 8 states, gestures (heart, thumbs up, click, type, wobble, nod, hop), eyes follow the cursor, speech/thinking bubble, glide to targets beside the element, return home | Real |
| State machine (`model.ts`) mirroring the character bible, with tests | Real |
| Task engine: snapshot → brain → gate → execute, confirm with 20 s timeout, Lost → teach → remembered, cancel, narration toggle | Real (`mochi-core`) |
| Safety gate: label/role/key rules, unsaved-window close, password-field block, per-app user labels, brain hint only adds caution | Real |
| Memory: SQLite `learned_target` / `app_setting` / `gate_log`, resolve by label then by position, auto-forget | Real |
| Phone ↔ desktop message types, pairing code | Types only (`protocol.rs`); server comes in phase 3 |
| Claude Code brain | Real; integration-tested against the live CLI (`cargo test -p mochi-brain-claude`) |
| Local AI brain | Real; tested against an in-process OpenAI-compatible fake server |
| Windows perception / input | Real code (`win.rs`), reviewed against the `windows` 0.62 crate source but **not yet compiled on Windows** |
| Settings window | Real: brain picker + test, Ask Mochi, memory list |
| Rive animation | Integration point in `rive.ts`; SVG character is used until `public/mochi.riv` exists (inputs documented in that file) |
| Voice | Windows speech voices in dev; hosted TTS adapter ready, no key yet |

## Phase 3 checklist

- [ ] Android app (Kotlin/Compose): wake word, speech-to-text, Mochi face (Rive), Yes/No, pairing approval screen.
- [ ] Desktop WebSocket server + mDNS advertisement, two-sided pairing with the 6-digit code (`protocol.rs` has the messages).
- [ ] Mirror `CharacterEvent`s to the phone as `DesktopMessage::State`; take `Utterance` as the task text.
- [ ] Hosted TTS proxy in the connector (`/tts`) with the key in Windows Credential Manager.
- [ ] Per-app allow list UI (the memory already has `app_setting.allowed`), global hotkey for the emergency stop.

## Notes

- The Tauri crate and `mochi-platform-windows/src/win.rs` were written against API names verified
  from the crate sources (Tauri 2.12, windows 0.62) and reviewed separately, but this environment
  could not compile Windows targets; the first `pnpm tauri dev` on Windows may surface a small fix
  or two. The core crates, both brain adapters and the frontend are fully tested.
- Claude Code runs as a subprocess per engine cycle (1–3 s each) with `--resume`; a long task is
  10–20 cycles.
- `cargo test` at the repo root runs only the portable crates (see `default-members` in `Cargo.toml`).
