// The settings window: pick the brain, test it, ask Mochi something, manage memory.
// Plain DOM; no framework. In the browser (no Tauri) it renders with stub data.

import "./settings.css";
import { hasTauri } from "./mochi/bridge";

type BrainKind = "claude_code" | "codex" | "local" | "mock";
type Settings = {
  brain: BrainKind;
  claude: { binary: string; model: string | null };
  codex: { binary: string; model: string | null };
  local: { host: string; port: number; model: string; api_key: string | null; vision: boolean };
  narration: boolean;
  scale: number;
};
type LearnedTarget = { id: number; app_id: string; user_phrase: string; ax_label: string | null; hits: number; misses: number };
type Probe = { ok: boolean; detail: string };
type VoiceStatus = { endpoint: string | null; unavailable: string | null };
type LinkStatus = { port: number | null; fingerprint: string | null; secrets_backend: string; desktop_name: string; connected: string | null; devices: { device_id: string; name: string; paired_at: string }[] };
type PairRequest = { session: number; device_name: string; code: string };

const DEFAULTS: Settings = {
  brain: "claude_code",
  claude: { binary: "claude", model: "sonnet" },
  codex: { binary: "codex", model: null },
  local: { host: "127.0.0.1", port: 11434, model: "llama3.2", api_key: null, vision: false },
  narration: true,
  scale: 0.8,
};

// Anything that goes wrong shows on the page (release builds have no devtools).
let root: HTMLElement = document.getElementById("app") ?? document.createElement("div");
function showError(where: string, e: unknown) {
  const pre = document.createElement("pre");
  pre.className = "error";
  pre.textContent = `${where}: ${e instanceof Error ? e.message : String(e)}`;
  root.prepend(pre);
}

// ---- backend (Tauri or browser stub) ----
async function makeApi() {
  if (!hasTauri()) return browserApi();
  const { invoke } = await import("@tauri-apps/api/core");
  return {
        get: () => invoke<Settings>("get_settings"),
        set: (s: Settings) => invoke<void>("set_settings", { settings: s }),
        probe: () => invoke<Probe>("probe_brain"),
        start: (task: string) => invoke<void>("start_task", { task, scenario: null }),
        cancel: () => invoke<void>("cancel_task"),
        targets: () => invoke<LearnedTarget[]>("list_targets"),
        forget: (id: number) => invoke<void>("forget_target", { id }),
        wipe: () => invoke<void>("wipe_memory"),
        link: () => invoke<LinkStatus>("link_status"),
        voice: () => invoke<VoiceStatus>("voice_status"),
        pairAnswer: (session: number, yes: boolean) => invoke<void>("pair_answer", { session, yes }),
        forgetDevice: (device_id: string) => invoke<void>("forget_device", { deviceId: device_id }),
        onPair: async (cb: (r: PairRequest) => void) => { const { listen } = await import("@tauri-apps/api/event"); await listen<PairRequest>("mochi://pair-request", (e) => cb(e.payload)); },
        onLink: async (cb: () => void) => { const { listen } = await import("@tauri-apps/api/event"); await listen("mochi://link", () => cb()); },
        onTaskFinished: async (cb: (outcome: string) => void) => { const { listen } = await import("@tauri-apps/api/event"); await listen<string>("mochi://task-finished", (e) => cb(e.payload)); },
        close: async () => { onClose?.(); },
        log: (line: string) => invoke<void>("log_js", { line }).catch(() => {}),
        quit: () => invoke<void>("quit_app"),
      };
}
function browserApi() {
  return {
      get: async () => ({ ...DEFAULTS }),
      set: async () => {},
      probe: async () => ({ ok: false, detail: "Browser preview: no backend." }),
      start: async () => {},
      cancel: async () => {},
      targets: async () => [{ id: 1, app_id: "chrome.exe", user_phrase: "the reply button", ax_label: "Reply", hits: 3, misses: 0 }],
      forget: async () => {},
      wipe: async () => {},
      voice: async (): Promise<VoiceStatus> => ({ endpoint: null, unavailable: "browser preview" }),
      link: async (): Promise<LinkStatus> => ({ port: 51234, fingerprint: "3f9a1c77b2e04d5a", secrets_backend: "Windows Credential Manager", desktop_name: "DESKTOP-MOCHI", connected: null, devices: [{ device_id: "ab12", name: "Pixel 9", paired_at: "2026-10-30" }] }),
      pairAnswer: async () => {},
      forgetDevice: async () => {},
      onPair: async (cb: (r: PairRequest) => void) => { (window as any).fakePairRequest = () => cb({ session: 1, device_name: "Pixel 9", code: "482913" }); },
      onLink: async () => {},
      onTaskFinished: async () => {},
      close: async () => { onClose?.(); },
      log: async (line: string) => console.log(line),
      quit: async () => {},
    };
}
type Api = Awaited<ReturnType<typeof makeApi>>;
let api: Api;
let settings: Settings = { ...DEFAULTS };
let onClose: (() => void) | null = null;

function h(html: string): HTMLElement {
  const t = document.createElement("template");
  t.innerHTML = html.trim();
  return t.content.firstElementChild as HTMLElement;
}

async function render() {
  root.replaceChildren(
    h(`
    <div class="page">
      <header><img src="/icon-128.png" alt="" width="40" height="40"><div><h1>Mochi</h1><p>Settings</p></div><button id="close" class="close" title="Close">✕</button></header>

      <section>
        <h2>Ask Mochi</h2>
        <form id="ask" class="row"><input id="task" type="text" placeholder="e.g. reply to Sara and say I'll be there at 6" autocomplete="off"><button type="submit" class="primary">Go</button><button type="button" id="stop">Stop</button></form>
        <p class="hint">Mochi works in whatever window is in front. It asks before sending, posting, deleting or closing unsaved work.</p>
        <p id="last-task" class="hint"></p>
      </section>

      <section>
        <h2>Brain</h2>
        <div class="choices">
          <label class="choice"><input type="radio" name="brain" value="claude_code"><div><b>Claude Code</b><span>Uses the Claude Code CLI you are signed into. Best results; sees screenshots.</span></div></label>
          <label class="choice"><input type="radio" name="brain" value="codex"><div><b>Codex CLI</b><span>OpenAI's Codex CLI, signed in with your ChatGPT account or an OpenAI key.</span></div></label>
          <label class="choice"><input type="radio" name="brain" value="local"><div><b>Local AI</b><span>Any OpenAI-compatible server on your network (Ollama, LM Studio, llama.cpp).</span></div></label>
        </div>
        <div id="brain-claude" class="sub">
          <label>Command <input id="claude-binary" type="text"></label>
          <label>Model <input id="claude-model" type="text" placeholder="sonnet (default) · haiku · opus if your plan has it"></label>
        </div>
        <div id="brain-codex" class="sub">
          <label>Command <input id="codex-binary" type="text"></label>
          <label>Model <input id="codex-model" type="text" placeholder="default (leave empty), e.g. gpt-5-codex"></label>
        </div>
        <div id="brain-local" class="sub">
          <div class="row"><label>Host <input id="local-host" type="text"></label><label class="short">Port <input id="local-port" type="number" min="1" max="65535"></label></div>
          <label>Model <input id="local-model" type="text"></label>
          <label>API key <input id="local-key" type="password" placeholder="optional"></label>
          <label class="check"><input id="local-vision" type="checkbox"> Model can see images (send screenshots)</label>
        </div>
        <div class="row"><button id="probe">Test connection</button><span id="probe-result" class="hint"></span></div>
      </section>

      <section>
        <h2>Voice</h2>
        <p id="voice-line" class="hint"></p>
      </section>

      <section>
        <h2>Phone</h2>
        <p id="link-line" class="hint"></p>
        <div id="pair" class="pair" hidden>
          <b id="pair-name"></b> wants to connect. Approve only if the phone shows this code:
          <div class="code" id="pair-code"></div>
          <div class="row"><button id="pair-yes" class="primary">Approve</button><button id="pair-no">Deny</button></div>
        </div>
        <table id="devices"><thead><tr><th>Paired phone</th><th>Since</th><th></th></tr></thead><tbody></tbody></table>
        <p class="hint">On the phone, open Mochi and pick this computer from the list, or type the address shown above. Both apps must be open and on the same Wi-Fi.</p>
      </section>

      <section>
        <h2>Character</h2>
        <label class="check"><input id="narration" type="checkbox"> Talk while working (narration)</label>
        <label>Size <input id="scale" type="range" min="0.6" max="1.2" step="0.05"> <span id="scale-v"></span></label>
      </section>

      <section>
        <h2>What Mochi remembers</h2>
        <p class="hint">Places you showed Mochi when it was lost. Nothing about what you typed or read.</p>
        <table id="targets"><thead><tr><th>App</th><th>You said</th><th>Button</th><th>Used</th><th></th></tr></thead><tbody></tbody></table>
        <div class="row"><button id="wipe" class="danger">Forget everything</button></div>
      </section>

      <footer><button id="save" class="primary">Save</button><span id="saved" class="hint"></span><span style="flex:1"></span><button id="quit" class="danger">Quit Mochi</button></footer>
    </div>`),
  );

  const $ = <T extends HTMLElement>(sel: string) => root.querySelector<T>(sel)!;
  $("#close").addEventListener("click", () => void api.close());
  window.addEventListener("keydown", (e) => { if (e.key === "Escape") void api.close(); });
  const radio = root.querySelector<HTMLInputElement>(`input[name=brain][value=${settings.brain}]`);
  if (radio) radio.checked = true;
  $<HTMLInputElement>("#claude-binary").value = settings.claude.binary;
  $<HTMLInputElement>("#claude-model").value = settings.claude.model ?? "";
  $<HTMLInputElement>("#codex-binary").value = settings.codex?.binary ?? "codex";
  $<HTMLInputElement>("#codex-model").value = settings.codex?.model ?? "";
  $<HTMLInputElement>("#local-host").value = settings.local.host;
  $<HTMLInputElement>("#local-port").value = String(settings.local.port);
  $<HTMLInputElement>("#local-model").value = settings.local.model;
  $<HTMLInputElement>("#local-key").value = settings.local.api_key ?? "";
  $<HTMLInputElement>("#local-vision").checked = settings.local.vision;
  $<HTMLInputElement>("#narration").checked = settings.narration;
  $<HTMLInputElement>("#scale").value = String(settings.scale);
  $("#scale-v").textContent = `${Math.round(settings.scale * 100)}%`;

  const showSub = () => {
    const b = (root.querySelector<HTMLInputElement>("input[name=brain]:checked")?.value ?? "claude_code") as BrainKind;
    $("#brain-claude").hidden = b !== "claude_code";
    $("#brain-codex").hidden = b !== "codex";
    $("#brain-local").hidden = b !== "local";
  };
  showSub();
  root.querySelectorAll<HTMLInputElement>("input[name=brain]").forEach((r) => r.addEventListener("change", showSub));
  $<HTMLInputElement>("#scale").addEventListener("input", (e) => { $("#scale-v").textContent = `${Math.round(Number((e.target as HTMLInputElement).value) * 100)}%`; });

  const collect = (): Settings => ({
    brain: (root.querySelector<HTMLInputElement>("input[name=brain]:checked")?.value ?? "claude_code") as BrainKind,
    claude: { binary: $<HTMLInputElement>("#claude-binary").value.trim() || "claude", model: $<HTMLInputElement>("#claude-model").value.trim() || null },
    codex: { binary: $<HTMLInputElement>("#codex-binary").value.trim() || "codex", model: $<HTMLInputElement>("#codex-model").value.trim() || null },
    local: {
      host: $<HTMLInputElement>("#local-host").value.trim() || "127.0.0.1",
      port: Number($<HTMLInputElement>("#local-port").value) || 11434,
      model: $<HTMLInputElement>("#local-model").value.trim(),
      api_key: $<HTMLInputElement>("#local-key").value.trim() || null,
      vision: $<HTMLInputElement>("#local-vision").checked,
    },
    narration: $<HTMLInputElement>("#narration").checked,
    scale: Number($<HTMLInputElement>("#scale").value),
  });
  const save = async () => {
    settings = collect();
    await api.set(settings);
    $("#saved").textContent = "Saved.";
    setTimeout(() => ($("#saved").textContent = ""), 1500);
  };
  $("#save").addEventListener("click", () => void save());
  $("#quit").addEventListener("click", () => void api.quit());
  $("#probe").addEventListener("click", async () => {
    await save();
    $("#probe-result").textContent = "Testing…";
    const r = await api.probe();
    const el = $("#probe-result");
    el.textContent = r.detail;
    el.className = r.ok ? "hint ok" : "hint bad";
  });
  $<HTMLFormElement>("#ask").addEventListener("submit", async (e) => {
    e.preventDefault();
    const task = $<HTMLInputElement>("#task").value.trim();
    if (!task) return;
    await save();
    try {
      await api.start(task);
      $<HTMLInputElement>("#task").value = "";
      onClose?.(); // get out of the way: Mochi needs the user's window in front
    } catch (err) {
      $("#probe-result").textContent = String(err);
    }
  });
  $("#stop").addEventListener("click", () => void api.cancel());

  const fillTargets = async () => {
    const rows = await api.targets();
    const tb = $("#targets tbody");
    tb.replaceChildren(
      ...rows.map((t) => {
        const tr = h(`<tr><td>${esc(t.app_id)}</td><td>${esc(t.user_phrase)}</td><td>${esc(t.ax_label ?? "")}</td><td>${t.hits}×</td><td><button class="link">forget</button></td></tr>`);
        tr.querySelector("button")!.addEventListener("click", async () => { await api.forget(t.id); await fillTargets(); });
        return tr;
      }),
    );
    if (rows.length === 0) tb.appendChild(h(`<tr><td colspan="5" class="hint">Nothing yet. When Mochi gets lost, click the thing it is looking for.</td></tr>`));
  };
  await fillTargets();
  $("#wipe").addEventListener("click", async () => { await api.wipe(); await fillTargets(); });

  // The sidecar's /health says whether Mochi's own voice is ready; while it downloads the
  // model (~350 MB, first start only) the Windows voice fills in.
  const fillVoice = async () => {
    const line = $("#voice-line");
    const v = await api.voice();
    if (!v.endpoint) { line.textContent = `Windows voice (Mochi's own voice: ${v.unavailable ?? "not available"}).`; return; }
    try {
      const h = await (await fetch(`${v.endpoint}/health`)).json() as { state: string; progress: number; detail: string };
      switch (h.state) {
        case "ready": line.textContent = "Mochi's own voice, running on this computer — speech never leaves it."; return;
        case "downloading": line.textContent = `Downloading Mochi's voice (${Math.round(h.progress * 100)}%, ${h.detail}) — using the Windows voice meanwhile.`; break;
        case "loading": line.textContent = "Loading Mochi's voice… using the Windows voice meanwhile."; break;
        case "error": line.textContent = `Mochi's voice could not start (${h.detail}); using the Windows voice.`; return;
        default: line.textContent = "Starting Mochi's voice… using the Windows voice meanwhile.";
      }
    } catch {
      line.textContent = "Starting Mochi's voice… using the Windows voice meanwhile.";
    }
    setTimeout(() => void fillVoice(), 2000);
  };
  await fillVoice();

  const fillLink = async () => {
    const st = await api.link();
    const addr = st.port ? `this computer is “${st.desktop_name}”, port ${st.port}${st.fingerprint ? `, encrypted (cert ${st.fingerprint}…)` : ""}` : "phone link is not running";
    $("#link-line").textContent = `${st.connected ? `Connected to ${st.connected} — ${addr}.` : `No phone connected — ${addr}.`} Secrets are kept in ${st.secrets_backend}.`;
    const tb = $("#devices tbody");
    tb.replaceChildren(
      ...st.devices.map((d) => {
        const tr = h(`<tr><td>${esc(d.name)}</td><td>${esc(d.paired_at)}</td><td><button class="link">forget</button></td></tr>`);
        tr.querySelector("button")!.addEventListener("click", async () => { await api.forgetDevice(d.device_id); await fillLink(); });
        return tr;
      }),
    );
    if (st.devices.length === 0) tb.appendChild(h(`<tr><td colspan="3" class="hint">No phone paired yet.</td></tr>`));
  };
  await fillLink();
  await api.onLink(fillLink);
  await api.onTaskFinished((outcome) => {
    const el = $("#last-task");
    const failed = /Failed|Blocked|GaveUp/.test(outcome);
    el.textContent = `Last task: ${outcome.replace(/^(\w+)\s*\{\s*(?:error|reason|summary|description):\s*"?/, "$1 — ").replace(/"?\s*\}$/, "")}`;
    el.className = failed ? "hint bad" : "hint ok";
  });
  await api.onPair((r) => {
    $("#pair").hidden = false;
    $("#pair-name").textContent = r.device_name;
    $("#pair-code").textContent = r.code.replace(/(\d{3})(\d{3})/, "$1 $2");
    const done = async (yes: boolean) => { $("#pair").hidden = true; await api.pairAnswer(r.session, yes); setTimeout(() => void fillLink(), 500); };
    $("#pair-yes").onclick = () => void done(true);
    $("#pair-no").onclick = () => void done(false);
  });
}

function esc(s: string) {
  return s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);
}

/**
 * Mount the settings UI into `container`. Used as a floating panel inside Mochi's own
 * window (main.ts) and, for development, as a standalone page (settings.html).
 */
export async function mountSettings(container: HTMLElement, close?: () => void) {
  root = container;
  onClose = close ?? null;
  try {
    api = await makeApi();
    await api.log("settings: mounting");
    settings = await api.get();
    await render();
    await api.log("settings: rendered");
  } catch (e) {
    showError("settings failed to start", e);
  }
}

/** Re-read state when the panel is reopened (link status, memory). */
export async function refreshSettings() {
  if (!api) return;
  try {
    settings = await api.get();
    await render();
  } catch (e) {
    showError("settings refresh", e);
  }
}

if (document.getElementById("app")) {
  window.addEventListener("error", (e) => showError("error", e.message));
  window.addEventListener("unhandledrejection", (e) => showError("rejection", (e as PromiseRejectionEvent).reason));
  void mountSettings(document.getElementById("app")!, () => window.close());
}
