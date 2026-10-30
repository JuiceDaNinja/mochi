# Mochi

A cute, lovable desktop helper you talk to from your phone. This repo is **phase 1** of the
[technical architecture](https://claude.ai/artifact/LCwDxCP59STfvApw4cQi5N): the character,
the task engine, the safety gate and the teaching memory, running against a scripted brain and
a pretend Gmail window. Phase 2 plugs in the Claude Code CLI and the real Windows screen.

```
mochi/
├─ crates/
│  ├─ mochi-core/        Rust. Brain trait, task loop, safety gate, SQLite memory, phone protocol. 20 tests.
│  └─ mochi-brain-mock/  Rust. Scripted scenarios + fake desktop, so the character can be built first.
└─ apps/
   └─ desktop/           Tauri 2 + Vite/TypeScript. The character window.
      ├─ src/mochi/      model.ts (state machine) · view.ts (SVG Mochi) · motion.ts · voice.ts · bridge.ts · rive.ts
      └─ src-tauri/      Rust shell: transparent always-on-top window, cursor polling, click-through, commands.
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

Click Mochi to open its menu and run a demo: **Reply to Sara**, **Same, but I get lost** (click anywhere
on screen to "show" Mochi the button when it cries), or **Close an unsaved note**. Mochi asks
"Send it?" before the irreversible step; click Yes/No under the bubble. Voice uses the Windows
speech voices for now (pitched up); the hosted anime voice is wired in `voice.ts` behind
`HostedTtsVoice` and switches on once the connector exposes `/tts`.

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
| Brain | **Mock** (`MockBrain`): scripted Gmail reply. `Brain` trait is the interface the Claude Code CLI adapter implements |
| Screen perception / input | **Mock** (`FakeDesktop`). Phase 2: Windows UI Automation + `SendInput` behind the `Perception` / `Actuator` traits |
| Rive animation | Integration point in `rive.ts`; SVG character is used until `public/mochi.riv` exists (inputs documented in that file) |
| Voice | Windows speech voices in dev; hosted TTS adapter ready, no key yet |

## Phase 2 checklist

- [ ] `crates/mochi-brain-claude`: spawn `claude -p --output-format stream-json` with `SYSTEM_PROMPT` from `brain.rs`, parse one `StepEvent` per turn.
- [ ] `crates/mochi-brain-local`: OpenAI-compatible chat over `http://<ip>:<port>`, same prompt.
- [ ] `crates/mochi-platform-windows`: UI Automation tree → `ScreenSnapshot`; `SendInput` for `Actuator`; element under cursor for teaching.
- [ ] Settings window: brain picker (Claude Code / local IP:port), per-app allow list and narration, learned targets list with delete.
- [ ] Global hotkey + "Mochi stop" for the emergency stop.

## Notes

- The Tauri crate was written against Tauri 2.12 API names verified from source, but this
  environment had no GTK, so it has not been compiled here; the first `pnpm tauri dev` on Windows
  may surface a small fix or two. The core crates and the frontend are fully tested.
- `cargo test` at the repo root runs only the portable crates (see `default-members` in `Cargo.toml`).
