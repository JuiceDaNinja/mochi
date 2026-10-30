# Mochi — Project Log

**Status as of 2026-10-30:** v0.1.12 released. Desktop (Windows) + Android apps built, paired, and tested on a real PC and Pixel phone with the Claude Code, Codex CLI, and local-AI brains.

- Repository: https://github.com/JuiceDaNinja/mochi
- Latest release: https://github.com/JuiceDaNinja/mochi/releases/tag/v0.1.12
- Character Bible (Claude Doc): https://claude.ai/artifact/YQkY1EALnqBAfiRUbQAGrE
- Technical Architecture (Claude Doc): https://claude.ai/artifact/LCwDxCP59STfvApw4cQi5N

---

## 1. What Mochi is

Mochi is a cute, lovable AI assistant character that lives at the bottom of your monitor and does tasks on your computer while you talk to her from your phone.

Two apps, one character:

| Device | Role |
|---|---|
| **Desktop (Windows)** | Shows Mochi, reads the screen, moves the mouse/keyboard, runs the "brain" |
| **Phone (Android)** | Voice input, mirrors Mochi's face, big **Yes / No** buttons for confirmations |

The brain is swappable: **Claude Code CLI**, **OpenAI Codex CLI**, or any **local OpenAI-compatible model** (Ollama, LM Studio, llama.cpp, vLLM) over `host:port`.

---

## 2. Character decisions (Character Bible)

| Topic | Decision |
|---|---|
| Personality | Cute, lovable, expressive; talks back while working; gives hearts / thumbs-up |
| Age / gender | None — but an anime-style girl voice |
| Name | "Mochi", fixed, cannot be changed |
| Language | English only (for now) |
| Narration | Mochi narrates what she's doing; can be toggled on/off |
| Thinking state | "thinking…" bubble above her head |
| Home position | Bottom center of whichever monitor the cursor is on; eyes follow the cursor |
| Working | Moves around the monitors with clicking / typing motions during a task |
| Key-moment signals | Sad / about-to-cry face + voice when she can't find something |
| Teaching | "Can't find it — show me?" → you click the element → she remembers it for next time |
| Last-step confirm | Always asks one more time ("Are you sure?" / "Send it?") in a cute voice before any irreversible action |
| Voice | Hosted TTS only (no local TTS); Windows voices as stand-in until the hosted voice is wired |

---

## 3. Architecture decisions (Technical Architecture)

| Topic | Decision |
|---|---|
| Desktop shell | **Tauri 2** (transparent, always-on-top, click-through window) |
| Character art | **Rive** (SVG face as stand-in until `mochi.riv` exists) |
| Brains | Claude Code CLI first; local AI; Codex CLI (added later) |
| Platforms | Windows + Android first; macOS / iOS later |
| Pairing | Approval on **both** devices with a matching 6-digit code (X25519 key exchange → code; HMAC-SHA256 auth) |
| Connection rule | Both apps must be running; link persists until the network drops; nothing wakes a sleeping PC |
| Link port | Fixed **47321** (fallback bind if taken); phone discovers via mDNS and follows the desktop's address |
| Safety | SafetyGate: every irreversible action stops for Yes/No; blocked actions logged |
| Memory | SQLite: learned UI targets (per app/window), per-app settings, gate log |

---

## 4. Code layout

```
mochi/
├─ crates/
│  ├─ mochi-core/              Brain trait, prompt render/parse, TaskEngine loop
│  │                           (snapshot → brain → gate → execute), SafetyGate,
│  │                           SQLite Memory, phone protocol, CharacterEvent runtime,
│  │                           launch.rs (CLI program resolution on Windows)
│  ├─ mochi-brain-claude/      claude -p, stream-json in/out, --tools "" --max-turns 1,
│  │                           session resumed per task, default model "sonnet"
│  ├─ mochi-brain-codex/       codex exec --json -o <file>, AGENTS.md rules,
│  │                           exec resume <thread> per task, fresh-session fallback
│  ├─ mochi-brain-local/       OpenAI-compatible /v1/chat/completions
│  ├─ mochi-brain-mock/        Scripted scenarios + fake desktop (tests)
│  ├─ mochi-platform-windows/  UI Automation tree, GDI screenshot, SendInput,
│  │                           target_window() skips Mochi's own process
│  └─ mochi-link/              tokio-tungstenite WS server, mdns-sd, X25519 pairing,
│                              HMAC auth, ping/pong keepalive, PairProgress events
└─ apps/
   ├─ desktop/                 Tauri 2 + Vite/TypeScript
   │  ├─ src/main.ts           Menu (Ask Mochi, narration, Settings, Stop, Quit),
   │  │                        click-through hit rects, pair-request bubble, teach-on-click
   │  ├─ src/settings.ts       In-window settings panel: brain picker + Test connection,
   │  │                        Ask Mochi, last task result, Phone (status/port/approve/devices),
   │  │                        memory list, Quit
   │  ├─ src/mochi/            model.ts (state machine) · view.ts (SVG face + gestures) ·
   │  │                        motion.ts · voice.ts (Web Speech, HostedTtsVoice hook) · rive.ts
   │  └─ src-tauri/src/        lib.rs (commands, engine wiring, mochi.log), settings.rs
   └─ android/                 Kotlin + Compose: NSD discovery, OkHttp WebSocket, Tink X25519,
                               EncryptedSharedPreferences, SpeechRecognizer, Canvas MochiFace,
                               gesture burst, Yes/No
```

### Task loop (mochi-core `engine.rs`)

1. **Snapshot** the foreground app window (UI Automation elements + optional screenshot).
2. Attach **memory hints** (elements the user taught for this app/window).
3. Send task + screen + history to the **brain**; parse exactly one action + narration.
4. **SafetyGate** decides: execute / confirm (Yes/No on both devices) / block.
5. **Execute** via SendInput; record hit/miss for memory targets.
6. Repeat until the brain reports *done* (summary + gesture kind: thumbs-up / heart) or *can't find* (→ cry → teach).

### Brain CLI invocations

| Brain | Command |
|---|---|
| Claude Code | `claude -p --input-format stream-json --output-format stream-json --verbose --tools "" --max-turns 1 --session-id <id>` / `--resume <id>` |
| Codex | first step: `codex exec -C <work_dir> --json --skip-git-repo-check -o <last_msg>`; later steps: `codex exec resume <thread> --json --skip-git-repo-check -o <last_msg>` |
| Local | `POST http://host:port/v1/chat/completions` (+ `/v1/models` for the test button) |

---

## 5. Build & release pipeline (GitHub Actions)

`.github/workflows/build.yml`:

- Runs Rust + Android unit tests on every push.
- Builds the Windows Tauri bundle and the Android debug APK.
- Reads the version from `apps/desktop/src-tauri/tauri.conf.json` and stamps it into **every** artifact name and the APK `versionName` / `versionCode`.
- Build errors are pushed to a `ci-logs` branch so they can be read without the Actions UI.
- Releases are created by `workflow_dispatch` with a `tag` input (tag pushes are blocked by the proxy):

  ```
  gh api -X POST repos/JuiceDaNinja/mochi/actions/workflows/build.yml/dispatches \
     -f ref=main -f 'inputs[tag]=v0.1.12'
  ```

Release assets per version:

```
Mochi_<v>_x64-setup.exe       NSIS installer
Mochi_<v>_x64-portable.exe    no-install exe
Mochi_<v>_x64_en-US.msi
Mochi_<v>_android.apk
```

Version bump = `sed 0.1.N→0.1.N+1` in `tauri.conf.json`, `Cargo.toml`, `apps/desktop/package.json`.

---

## 6. Version history

| Version | Change |
|---|---|
| Phase 1 | Character, task engine, safety gate, memory, mock brain |
| Phase 2 | Claude Code + local-AI brains, Windows UI Automation + SendInput, settings window |
| Phase 3 | Phone link (mDNS, X25519 pairing, HMAC auth, keepalive), pairing UI, Android app |
| CI | Windows installer + APK on GitHub Actions; ci-logs branch; manual release trigger |
| 0.1.1 | Fix Yes/No click-through; settings diagnostics; `mochi.log`; Android pairing trail |
| 0.1.2 | Pairing: newest request supersedes pending ones; phone keeps a single socket |
| 0.1.3 | Settings as a floating panel **inside** the character window (fixed blank/unclosable window) |
| 0.1.4 | Fixed link port 47321; phone follows the desktop's address; skip Mochi's own window when reading the screen; per-cycle task logging |
| 0.1.5 | Claude brain defaults to **sonnet**; Test connection runs a real prompt |
| 0.1.6 | Phone shows success face + heart / thumbs-up gesture |
| 0.1.7 | Fix thumbs-up gesture sticking after the hop |
| 0.1.8 | One version stamped into APK name/versionName and portable exe |
| 0.1.9 | **Codex CLI brain**: adapter crate, settings option, connection test |
| 0.1.10 | Demos removed from the UI; **Quit Mochi** button |
| 0.1.11 | Resolve `.cmd`/`.bat` CLI shims on Windows (Codex via npm); last task result shown in Settings |
| 0.1.12 | Codex: `exec resume` no longer passes `-C` (exit code 2 fix); retry as a fresh session if resume fails |

---

## 7. Bugs found on real devices and how they were fixed

| Symptom | Cause | Fix |
|---|---|---|
| Yes/No buttons unclickable | Click-through hit test only covered the body | Hit rect = union of body + menu + confirm + bubble + settings panel |
| Settings window blank, can't close | Separate Tauri window failed to load assets | Settings became an in-window floating panel |
| Phone kept showing the code after approval | Pair request and approval on two different sockets | Newest PairRequest supersedes pending ones; phone keeps one socket |
| Phone connected to a stale port | Random port per run | Fixed port 47321 + discovery follows the desktop address |
| Mochi "read" her own window | Snapshot took the foreground window = Mochi | `target_window()` skips own pid, foregrounds next app window |
| `claude-opus-5-5 not accessible` | Default model not available on the user's plan | Default model → `sonnet` |
| Thumbs-up stuck on desktop | Shared cleanup timer cancelled by the next gesture | Per-gesture cleanup timer map |
| Phone showed only text on success | No success face / gesture in the protocol | `Done { summary, kind }` + success hold on phone |
| Codex: "a bit sleepy… try again" (step 0) | `codex` on Windows is a `.cmd` shim, `Command` can't spawn it | `launch.rs`: PATH + PATHEXT resolution, `.cmd` via `cmd /C` |
| Codex: step 1 `exit code 2 … try '--help'` | `codex exec resume` doesn't accept `-C` | Resume passes only the thread id; fresh-session fallback |
| Claude returned narration only | Prompt said "exactly ONE event" | Contract = narration **and** action; adapter nudge |
| Rust `MutexGuard` lifetime error in `quit_app` | Guard held across `await` | Clone handles in a block, then await |

---

## 8. Using Mochi (quick guide)

**Install:** run `Mochi_<v>_x64-setup.exe` on Windows; install `Mochi_<v>_android.apk` on the phone.

**Pick a brain (Settings…):**
- **Claude Code** — `npm i -g @anthropic-ai/claude-code`, run `claude` once to sign in (Claude subscription; no API key needed). Default model `sonnet`.
- **Codex CLI** — `npm i -g @openai/codex`, `codex login` once.
- **Local AI** — host, port, model name; tick "can see images" only for multimodal models.

Press **Test connection** to verify.

**Pair the phone:** both on the same Wi-Fi, both apps open → pick the PC on the phone → same 6 digits on both screens → **Approve** on phone + **Yes** on PC. Reconnects automatically afterwards.

**Give a task:** click Mochi → **Ask Mochi…** (or **Talk** on the phone). She narrates, moves, and asks "Send it?" before anything irreversible — answer on either device.

**Teach:** when she cries "can't find it — show me?", click the element you meant. It's remembered for that app.

**Logs:** `%APPDATA%\app.mochi.desktop\mochi.log` (per-cycle lines: window, elements, brain → action, gate decision, brain errors with CLI stderr).

**Quit:** menu → **Quit Mochi**.

---

## 9. Minimum model requirements (as tested)

| Brain | Works |
|---|---|
| Claude Code | `sonnet` (default) — reliable; `haiku` for simple tasks; `opus` only if your plan allows |
| Codex CLI | default `codex` model from `codex login` works |
| Local AI | needs an instruction-following model that returns the JSON action contract; vision models only if "can see images" is on |

---

## 10. Open question discussed: "watch me and remember the whole task"

**Today:** teaching stores one UI element per "show me" (app, window, element role/name, position) and feeds it to the brain as a hint next time; hits/misses adjust confidence.

**Not yet built:** recording a whole sequence (open app → click → type → send) and replaying it when asked for "the same task." Proposed design (~1 day):

1. **Watch-me mode** — global input hook records clicks/keys with the UI element under each (not pixels), plus app/window per step. Started either when Mochi fails ("want to show me how?") or from the menu.
2. **Save** under a name (user-given or brain-summarized).
3. **Recall** — saved recordings are offered to the brain as a plan; replay runs *through the normal loop* so the confirm gate and "can't find it" fallback still apply if the screen changed (robust replay, not blind replay).

Decision pending from the user.

---

## 11. Remaining roadmap (Phase 4)

- TLS on the phone link
- Secrets in Windows Credential Manager
- Hosted anime TTS voice (`HostedTtsVoice` in `voice.ts`)
- Rive character (`mochi.riv`) on both desktop and phone
- "Hey Mochi" wake word (Porcupine) on the phone
- Task recording / replay (section 10)
- macOS + iOS
