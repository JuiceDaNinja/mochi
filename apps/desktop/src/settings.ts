// The settings window: pick the brain, test it, ask Mochi something, manage memory.
// Plain DOM; no framework. In the browser (no Tauri) it renders with stub data.

import "./settings.css";
import { hasTauri } from "./mochi/bridge";

type BrainKind = "claude_code" | "local" | "mock";
type Settings = {
  brain: BrainKind;
  claude: { binary: string; model: string | null };
  local: { host: string; port: number; model: string; api_key: string | null; vision: boolean };
  narration: boolean;
  scale: number;
};
type LearnedTarget = { id: number; app_id: string; user_phrase: string; ax_label: string | null; hits: number; misses: number };
type Probe = { ok: boolean; detail: string };

const DEFAULTS: Settings = {
  brain: "claude_code",
  claude: { binary: "claude", model: null },
  local: { host: "127.0.0.1", port: 11434, model: "llama3.2", api_key: null, vision: false },
  narration: true,
  scale: 0.8,
};

// ---- backend (Tauri or browser stub) ----
const api = hasTauri()
  ? await (async () => {
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
      };
    })()
  : {
      get: async () => ({ ...DEFAULTS }),
      set: async () => {},
      probe: async () => ({ ok: false, detail: "Browser preview: no backend." }),
      start: async () => {},
      cancel: async () => {},
      targets: async () => [{ id: 1, app_id: "chrome.exe", user_phrase: "the reply button", ax_label: "Reply", hits: 3, misses: 0 }],
      forget: async () => {},
      wipe: async () => {},
    };

const root = document.getElementById("app")!;
let settings = await api.get();

function h(html: string): HTMLElement {
  const t = document.createElement("template");
  t.innerHTML = html.trim();
  return t.content.firstElementChild as HTMLElement;
}

async function render() {
  root.replaceChildren(
    h(`
    <div class="page">
      <header><img src="/icon-128.png" alt="" width="40" height="40"><div><h1>Mochi</h1><p>Settings</p></div></header>

      <section>
        <h2>Ask Mochi</h2>
        <form id="ask" class="row"><input id="task" type="text" placeholder="e.g. reply to Sara and say I'll be there at 6" autocomplete="off"><button type="submit" class="primary">Go</button><button type="button" id="stop">Stop</button></form>
        <p class="hint">Mochi works in whatever window is in front. It asks before sending, posting, deleting or closing unsaved work.</p>
      </section>

      <section>
        <h2>Brain</h2>
        <div class="choices">
          <label class="choice"><input type="radio" name="brain" value="claude_code"><div><b>Claude Code</b><span>Uses the Claude Code CLI you are signed into. Best results; sees screenshots.</span></div></label>
          <label class="choice"><input type="radio" name="brain" value="local"><div><b>Local AI</b><span>Any OpenAI-compatible server on your network (Ollama, LM Studio, llama.cpp).</span></div></label>
          <label class="choice"><input type="radio" name="brain" value="mock"><div><b>Demo</b><span>Scripted Gmail reply on a pretend screen. Nothing on your PC is touched.</span></div></label>
        </div>
        <div id="brain-claude" class="sub">
          <label>Command <input id="claude-binary" type="text"></label>
          <label>Model <input id="claude-model" type="text" placeholder="default (leave empty), or sonnet / opus"></label>
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

      <footer><button id="save" class="primary">Save</button><span id="saved" class="hint"></span></footer>
    </div>`),
  );

  const $ = <T extends HTMLElement>(sel: string) => root.querySelector<T>(sel)!;
  const radio = root.querySelector<HTMLInputElement>(`input[name=brain][value=${settings.brain}]`);
  if (radio) radio.checked = true;
  $<HTMLInputElement>("#claude-binary").value = settings.claude.binary;
  $<HTMLInputElement>("#claude-model").value = settings.claude.model ?? "";
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
    $("#brain-local").hidden = b !== "local";
  };
  showSub();
  root.querySelectorAll<HTMLInputElement>("input[name=brain]").forEach((r) => r.addEventListener("change", showSub));
  $<HTMLInputElement>("#scale").addEventListener("input", (e) => { $("#scale-v").textContent = `${Math.round(Number((e.target as HTMLInputElement).value) * 100)}%`; });

  const collect = (): Settings => ({
    brain: (root.querySelector<HTMLInputElement>("input[name=brain]:checked")?.value ?? "claude_code") as BrainKind,
    claude: { binary: $<HTMLInputElement>("#claude-binary").value.trim() || "claude", model: $<HTMLInputElement>("#claude-model").value.trim() || null },
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
}

function esc(s: string) {
  return s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);
}

await render();
