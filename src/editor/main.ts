import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import "./style.css";

type SelectionMode = "weighted_random" | "fixed";
type TransitionPolicy = "immediate" | "finish_current_clip";

interface VideoLoop {
  start_ms: number;
  end_ms: number;
}

interface VideoClipConfig {
  file: string;
  enabled: boolean;
  weight: number;
  loop?: VideoLoop;
}

interface StateVideoConfig {
  selection: SelectionMode;
  transition_policy: TransitionPolicy;
  clips: VideoClipConfig[];
}

interface VideoConfig {
  schema_version: number;
  states: Record<string, StateVideoConfig>;
}

interface EditorBootstrap {
  schema: {
    driver_version: string;
    states: string[];
  };
  videoRoot: string;
  configPath: string;
  source: "configured" | "legacy_scan";
  fingerprint: string;
  config: VideoConfig;
  resolvedPaths: Record<string, string[]>;
}

interface RuntimeSync {
  status: "saved" | "loaded" | "applied" | "pending" | "error";
  message: string;
  runtimeFingerprint?: string;
  playbackFingerprint?: string;
}

interface EditorSaveResult {
  bootstrap: EditorBootstrap;
  runtimeSync: RuntimeSync;
}

const app = document.querySelector<HTMLDivElement>("#app");
if (!app) throw new Error("editor root is missing");

app.innerHTML = `
  <main class="studio-shell">
    <header class="topbar">
      <div class="brand-lockup">
        <span class="brand-mark" aria-hidden="true"><i></i><i></i><i></i></span>
        <div>
          <p class="eyebrow">CODEX PET / AUTHORING</p>
          <h1>Video Studio</h1>
        </div>
      </div>
      <div class="source-readout">
        <span class="readout-label">STATE SOURCE</span>
        <strong id="driver-version">loading</strong>
        <span id="source-mode" class="source-mode">—</span>
      </div>
      <div class="toolbar">
        <button id="runtime-sync" class="button button-quiet" type="button">检查运行同步</button>
        <button id="save-config" class="button button-primary" type="button">保存并应用</button>
      </div>
    </header>

    <section class="sync-strip" id="sync-strip" data-status="pending">
      <span class="sync-light" aria-hidden="true"></span>
      <span id="sync-copy">正在读取运行时唯一状态源…</span>
      <code id="config-fingerprint">—</code>
    </section>

    <section class="workspace">
      <aside class="state-panel panel">
        <div class="panel-heading">
          <div>
            <p class="eyebrow">01 / FIXED LOGIC</p>
            <h2>状态源</h2>
          </div>
          <span id="state-count" class="count-pill">0</span>
        </div>
        <p class="panel-note">来自 native 运行时。这里只选择，不可新增、改名或连线。</p>
        <nav id="state-list" class="state-list" aria-label="运行时状态"></nav>
      </aside>

      <section class="preview-panel panel">
        <div class="panel-heading preview-heading">
          <div>
            <p class="eyebrow">02 / LIVE PREVIEW</p>
            <h2 id="preview-title">选择一个状态</h2>
          </div>
          <div class="preview-actions">
            <button id="simulate-pick" class="button button-quiet" type="button">抽取一次</button>
            <button id="toggle-play" class="transport-button" type="button" aria-label="播放或暂停">▶</button>
          </div>
        </div>

        <div class="video-stage">
          <video id="preview-video" muted playsinline></video>
          <div id="video-placeholder" class="video-placeholder">
            <span>NO CLIP SELECTED</span>
            <small>从右侧资源池选择视频</small>
          </div>
          <div class="stage-grid" aria-hidden="true"></div>
          <div class="stage-badge"><span id="preview-state">—</span><em>PREVIEW</em></div>
        </div>

        <div class="timeline-block">
          <div class="time-readout">
            <span id="current-time">00:00.000</span>
            <span id="clip-name">—</span>
            <span id="duration-time">00:00.000</span>
          </div>
          <div class="timeline-track">
            <div id="loop-band" class="loop-band"></div>
            <input id="scrubber" type="range" min="0" max="1000" value="0" aria-label="视频时间轴" />
          </div>
          <div class="loop-controls">
            <div class="loop-field">
              <label for="loop-start">LOOP A / ms</label>
              <input id="loop-start" type="number" min="0" step="1" placeholder="未设置" />
              <button id="set-loop-a" type="button">取当前帧</button>
            </div>
            <div class="loop-arrow" aria-hidden="true">A ↔ B</div>
            <div class="loop-field">
              <label for="loop-end">LOOP B / ms</label>
              <input id="loop-end" type="number" min="1" step="1" placeholder="未设置" />
              <button id="set-loop-b" type="button">取当前帧</button>
            </div>
            <button id="clear-loop" class="button button-quiet clear-loop" type="button">播完整片</button>
          </div>
          <p id="preview-message" class="preview-message">未设置 A–B 时，整片播完后由运行时从当前状态资源池选择下一条；设置后 A–B 在状态保持期间循环。</p>
        </div>
      </section>

      <aside class="asset-panel panel">
        <div class="panel-heading">
          <div>
            <p class="eyebrow">03 / ASSET POOL</p>
            <h2>视频资源</h2>
          </div>
          <select id="selection-mode" aria-label="资源选择模式">
            <option value="weighted_random">加权随机</option>
            <option value="fixed">固定第一条</option>
          </select>
          <select id="transition-policy" aria-label="状态切换策略">
            <option value="immediate">立即响应切换</option>
            <option value="finish_current_clip">当前片段播完</option>
          </select>
        </div>
        <div id="drop-zone" class="drop-zone">
          <strong>拖入视频</strong>
          <span>MP4 / WebM / M4V · 自动复制到当前状态目录</span>
        </div>
        <div id="asset-summary" class="asset-summary"></div>
        <div id="clip-list" class="clip-list"></div>
      </aside>
    </section>

    <footer class="statusbar">
      <span id="config-path">video-config.json</span>
      <span id="dirty-state">READ ONLY</span>
    </footer>
  </main>
`;

const elements = {
  driverVersion: required("driver-version"),
  sourceMode: required("source-mode"),
  save: requiredButton("save-config"),
  syncButton: requiredButton("runtime-sync"),
  syncStrip: required("sync-strip"),
  syncCopy: required("sync-copy"),
  fingerprint: required("config-fingerprint"),
  stateCount: required("state-count"),
  stateList: required("state-list"),
  previewTitle: required("preview-title"),
  previewState: required("preview-state"),
  previewVideo: requiredVideo("preview-video"),
  videoPlaceholder: required("video-placeholder"),
  togglePlay: requiredButton("toggle-play"),
  simulatePick: requiredButton("simulate-pick"),
  currentTime: required("current-time"),
  durationTime: required("duration-time"),
  clipName: required("clip-name"),
  scrubber: requiredInput("scrubber"),
  loopBand: required("loop-band"),
  loopStart: requiredInput("loop-start"),
  loopEnd: requiredInput("loop-end"),
  setLoopA: requiredButton("set-loop-a"),
  setLoopB: requiredButton("set-loop-b"),
  clearLoop: requiredButton("clear-loop"),
  previewMessage: required("preview-message"),
  selectionMode: requiredSelect("selection-mode"),
  transitionPolicy: requiredSelect("transition-policy"),
  dropZone: required("drop-zone"),
  assetSummary: required("asset-summary"),
  clipList: required("clip-list"),
  configPath: required("config-path"),
  dirtyState: required("dirty-state"),
};

let bootstrap: EditorBootstrap | null = null;
let draft: VideoConfig | null = null;
let selectedState = "";
let selectedClipFile = "";
let dirty = false;
let saving = false;
let lastSimulationFile = "";
let loadedDurationMs = 0;

void initialize();

async function initialize(): Promise<void> {
  try {
    applyBootstrap(await invoke<EditorBootstrap>("editor_bootstrap"));
    await getCurrentWebview().onDragDropEvent((event) => {
      const payload = event.payload;
      elements.dropZone.classList.toggle("is-dragging", payload.type === "over");
      if (payload.type === "drop") void importVideos(payload.paths);
    });
    setSync("saved", "配置已读取。修改后保存，运行中的 Pet 会自动热重载。", bootstrap?.fingerprint);
  } catch (error) {
    setSync("error", readableError(error));
    elements.previewMessage.textContent = "编辑器初始化失败，请检查资源目录和配置文件。";
  }
}

function applyBootstrap(next: EditorBootstrap): void {
  bootstrap = next;
  draft = structuredClone(next.config);
  selectedState = next.schema.states.includes(selectedState) ? selectedState : next.schema.states[0] ?? "";
  const selectedClips = draft.states[selectedState]?.clips ?? [];
  selectedClipFile = selectedClips.some((clip) => clip.file === selectedClipFile)
    ? selectedClipFile
    : selectedClips.find((clip) => clip.enabled)?.file ?? selectedClips[0]?.file ?? "";
  dirty = next.source === "legacy_scan";
  elements.driverVersion.textContent = next.schema.driver_version;
  elements.sourceMode.textContent = next.source === "configured" ? "CONFIG ACTIVE" : "LEGACY DERIVED";
  elements.sourceMode.dataset.mode = next.source;
  elements.stateCount.textContent = String(next.schema.states.length).padStart(2, "0");
  elements.configPath.textContent = next.configPath;
  elements.fingerprint.textContent = next.fingerprint;
  renderAll();
}

function renderAll(): void {
  renderStates();
  renderAssets();
  syncPreviewSelection();
  updateDirtyUi();
}

function renderStates(): void {
  if (!bootstrap || !draft) return;
  elements.stateList.replaceChildren();
  bootstrap.schema.states.forEach((state, index) => {
    const config = draft?.states[state];
    const enabled = config?.clips.filter((clip) => clip.enabled).length ?? 0;
    const button = document.createElement("button");
    button.type = "button";
    button.className = `state-item${state === selectedState ? " is-active" : ""}${enabled === 0 ? " is-invalid" : ""}`;
    button.innerHTML = `<span class="state-index">${String(index + 1).padStart(2, "0")}</span><span class="state-copy"><strong></strong><small>${enabled} enabled</small></span><span class="state-dot"></span>`;
    button.querySelector("strong")!.textContent = state;
    button.addEventListener("click", () => {
      selectedState = state;
      const clips = currentStateConfig()?.clips ?? [];
      selectedClipFile = clips.find((clip) => clip.enabled)?.file ?? clips[0]?.file ?? "";
      lastSimulationFile = "";
      renderAll();
    });
    elements.stateList.append(button);
  });
}

function renderAssets(): void {
  const stateConfig = currentStateConfig();
  if (!stateConfig) return;
  elements.selectionMode.value = stateConfig.selection;
  elements.transitionPolicy.value = stateConfig.transition_policy;
  elements.clipList.replaceChildren();
  const enabled = stateConfig.clips.filter((clip) => clip.enabled);
  const totalWeight = enabled.reduce((sum, clip) => sum + clip.weight, 0);
  const firstEnabledIndex = stateConfig.clips.findIndex((clip) => clip.enabled);
  let selectionSummary = "无启用资源";
  if (stateConfig.selection === "fixed") {
    selectionSummary = "固定第一条";
  } else if (enabled.length > 1) {
    selectionSummary = "自动避免连续重复";
  } else if (enabled.length === 1) {
    selectionSummary = "单资源自然重复";
  }
  const transitionSummary = stateConfig.transition_policy === "finish_current_clip"
    ? "当前片段播完后切换"
    : "立即响应状态切换";
  elements.assetSummary.textContent = `${stateConfig.clips.length} 条资源 · ${enabled.length} 条参与选择 · ${selectionSummary} · ${transitionSummary}`;

  stateConfig.clips.forEach((clip, index) => {
    let probability = 0;
    if (stateConfig.selection === "fixed") {
      probability = index === firstEnabledIndex ? 100 : 0;
    } else if (clip.enabled && totalWeight > 0) {
      probability = (clip.weight / totalWeight) * 100;
    }
    const row = document.createElement("article");
    row.className = `clip-card${clip.file === selectedClipFile ? " is-selected" : ""}${clip.enabled ? "" : " is-disabled"}`;
    row.tabIndex = 0;
    row.innerHTML = `
      <button class="clip-select" type="button" aria-label="预览此视频"><span class="play-glyph">▶</span></button>
      <div class="clip-main">
        <strong class="clip-file"></strong>
        <span class="clip-loop"></span>
      </div>
      <label class="compact-field weight-field"><span>权重</span><input type="number" min="1" max="1000000" step="1" /></label>
      <span class="probability">${probability.toFixed(probability >= 10 ? 0 : 1)}%</span>
      <label class="switch" title="是否参与当前状态选择"><input type="checkbox" /><span></span></label>
      <button class="remove-clip" type="button" aria-label="从配置移除">×</button>
    `;
    row.querySelector<HTMLElement>(".clip-file")!.textContent = fileLeaf(clip.file);
    row.querySelector<HTMLElement>(".clip-loop")!.textContent = clip.loop
      ? `intro → ${clip.loop.start_ms}ms · loop ${clip.loop.start_ms}–${clip.loop.end_ms}ms`
      : "full clip · select next on end";
    const weight = row.querySelector<HTMLInputElement>(".weight-field input")!;
    weight.value = String(clip.weight);
    weight.disabled = !clip.enabled;
    weight.addEventListener("change", () => {
      clip.weight = clampInteger(weight.valueAsNumber, 1, 1_000_000);
      markDirty();
      renderAssets();
    });
    const enabledToggle = row.querySelector<HTMLInputElement>(".switch input")!;
    enabledToggle.checked = clip.enabled;
    enabledToggle.addEventListener("change", () => {
      clip.enabled = enabledToggle.checked;
      markDirty();
      renderAll();
    });
    const select = (): void => {
      selectedClipFile = clip.file;
      syncPreviewSelection();
      renderAssets();
    };
    row.querySelector<HTMLButtonElement>(".clip-select")!.addEventListener("click", select);
    row.addEventListener("dblclick", select);
    row.addEventListener("keydown", (event) => {
      if (event.key === "Enter") select();
    });
    row.querySelector<HTMLButtonElement>(".remove-clip")!.addEventListener("click", () => {
      stateConfig.clips.splice(index, 1);
      if (selectedClipFile === clip.file) {
        selectedClipFile = stateConfig.clips.find((item) => item.enabled)?.file ?? stateConfig.clips[0]?.file ?? "";
      }
      markDirty();
      renderAll();
    });
    elements.clipList.append(row);
  });
}

function syncPreviewSelection(): void {
  const clip = currentClip();
  elements.previewTitle.textContent = selectedState || "选择一个状态";
  elements.previewState.textContent = selectedState || "—";
  elements.clipName.textContent = clip ? fileLeaf(clip.file) : "—";
  if (!clip || !bootstrap) {
    elements.previewVideo.removeAttribute("src");
    delete elements.previewVideo.dataset.file;
    delete elements.previewVideo.dataset.path;
    elements.previewVideo.load();
    elements.videoPlaceholder.hidden = false;
    loadedDurationMs = 0;
    syncLoopControls();
    return;
  }
  const path = resolvedPath(selectedState, clip.file);
  if (!path) {
    elements.videoPlaceholder.hidden = false;
    elements.previewMessage.textContent = "该资源尚未解析到素材根目录。保存前请重新导入。";
    return;
  }
  const url = convertFileSrc(path);
  if (elements.previewVideo.dataset.path !== path) {
    elements.previewVideo.dataset.file = clip.file;
    elements.previewVideo.dataset.path = path;
    elements.previewVideo.src = url;
    elements.previewVideo.load();
  }
  elements.videoPlaceholder.hidden = true;
  syncLoopControls();
}

function syncLoopControls(): void {
  const loop = currentClip()?.loop;
  elements.loopStart.value = loop ? String(loop.start_ms) : "";
  elements.loopEnd.value = loop ? String(loop.end_ms) : "";
  updateLoopBand();
}

function updateLoopFromInputs(): void {
  const clip = currentClip();
  if (!clip) return;
  const start = elements.loopStart.value === "" ? null : Math.max(0, Math.round(elements.loopStart.valueAsNumber));
  const end = elements.loopEnd.value === "" ? null : Math.max(1, Math.round(elements.loopEnd.valueAsNumber));
  if (start === null && end === null) {
    delete clip.loop;
    markDirty();
    renderAssets();
    updateLoopBand();
    return;
  }
  if (start === null || end === null || start >= end) {
    elements.previewMessage.textContent = "Loop A 与 B 必须同时设置，并满足 A < B。";
    elements.previewMessage.dataset.level = "error";
    updateDirtyUi();
    return;
  }
  clip.loop = { start_ms: start, end_ms: end };
  elements.previewMessage.textContent = "A 之前播放一次，A–B 在状态保持期间循环。";
  elements.previewMessage.dataset.level = "ok";
  markDirty();
  renderAssets();
  updateLoopBand();
}

function updateLoopBand(): void {
  const loop = currentClip()?.loop;
  const duration = loadedDurationMs;
  if (!loop || duration <= 0) {
    elements.loopBand.style.left = "0";
    elements.loopBand.style.width = "0";
    return;
  }
  const left = Math.min(100, (loop.start_ms / duration) * 100);
  const right = Math.min(100, (loop.end_ms / duration) * 100);
  elements.loopBand.style.left = `${left}%`;
  elements.loopBand.style.width = `${Math.max(0, right - left)}%`;
}

function markDirty(): void {
  dirty = true;
  updateDirtyUi();
}

function updateDirtyUi(): void {
  const validation = validateDraft();
  elements.save.disabled = saving || !dirty || validation !== null;
  elements.save.textContent = saving ? "正在同步…" : "保存并应用";
  elements.dirtyState.textContent = validation ?? (dirty ? "UNSAVED CHANGES" : "CONFIG SAVED");
  elements.dirtyState.dataset.level = validation ? "error" : dirty ? "dirty" : "saved";
}

function validateDraft(): string | null {
  if (!bootstrap || !draft) return "CONFIG UNAVAILABLE";
  for (const state of bootstrap.schema.states) {
    const stateConfig = draft.states[state];
    if (!stateConfig) return `MISSING ${state}`;
    if (!stateConfig.clips.some((clip) => clip.enabled)) return `${state} HAS NO ENABLED CLIP`;
    for (const clip of stateConfig.clips) {
      if (!Number.isInteger(clip.weight) || clip.weight < 1 || clip.weight > 1_000_000) return `INVALID WEIGHT`;
      if (clip.loop && (clip.loop.start_ms < 0 || clip.loop.start_ms >= clip.loop.end_ms)) return `INVALID LOOP`;
      if (clip.file === selectedClipFile && clip.loop && loadedDurationMs > 0 && clip.loop.end_ms > loadedDurationMs) {
        return `LOOP EXCEEDS DURATION`;
      }
    }
  }
  return null;
}

async function saveConfig(): Promise<void> {
  if (!draft || saving || validateDraft()) return;
  saving = true;
  updateDirtyUi();
  setSync("pending", "正在保存配置并等待运行时读取回执…");
  try {
    const result = await invoke<EditorSaveResult>("editor_save_config", {
      config: draft,
      expectedVideoRoot: bootstrap!.videoRoot,
    });
    applyBootstrap(result.bootstrap);
    dirty = false;
    updateDirtyUi();
    showRuntimeSync(result.runtimeSync);
  } catch (error) {
    setSync("error", readableError(error));
  } finally {
    saving = false;
    updateDirtyUi();
  }
}

async function importVideos(paths: string[]): Promise<void> {
  if (!selectedState || paths.length === 0 || saving) return;
  saving = true;
  updateDirtyUi();
  elements.dropZone.classList.add("is-busy");
  setSync("pending", `正在导入 ${paths.length} 个视频到 ${selectedState}…`);
  try {
    if (dirty && draft) {
      const saved = await invoke<EditorSaveResult>("editor_save_config", {
        config: draft,
        expectedVideoRoot: bootstrap!.videoRoot,
      });
      applyBootstrap(saved.bootstrap);
      dirty = false;
    }
    const result = await invoke<EditorSaveResult>("editor_import_videos", {
      state: selectedState,
      paths,
      expectedVideoRoot: bootstrap!.videoRoot,
    });
    applyBootstrap(result.bootstrap);
    dirty = false;
    updateDirtyUi();
    showRuntimeSync(result.runtimeSync);
  } catch (error) {
    setSync("error", readableError(error));
  } finally {
    saving = false;
    elements.dropZone.classList.remove("is-busy", "is-dragging");
    updateDirtyUi();
  }
}

function simulatePick(): void {
  const stateConfig = currentStateConfig();
  if (!stateConfig) return;
  const enabled = stateConfig.clips.filter((clip) => clip.enabled);
  const choices = stateConfig.selection === "fixed"
    ? enabled
    : enabled.length > 1
      ? enabled.filter((clip) => clip.file !== lastSimulationFile)
      : enabled;
  if (choices.length === 0) return;
  let selected: VideoClipConfig;
  if (stateConfig.selection === "fixed") {
    selected = choices[0];
  } else {
    const total = choices.reduce((sum, clip) => sum + clip.weight, 0);
    let target = Math.random() * total;
    selected = choices[choices.length - 1];
    for (const clip of choices) {
      target -= clip.weight;
      if (target < 0) {
        selected = clip;
        break;
      }
    }
  }
  lastSimulationFile = selected.file;
  selectedClipFile = selected.file;
  renderAssets();
  syncPreviewSelection();
  void elements.previewVideo.play();
}

function showRuntimeSync(sync: RuntimeSync): void {
  setSync(sync.status, sync.message, sync.runtimeFingerprint ?? bootstrap?.fingerprint);
}

function setSync(status: RuntimeSync["status"], message: string, fingerprint?: string): void {
  elements.syncStrip.dataset.status = status;
  elements.syncCopy.textContent = message;
  if (fingerprint) elements.fingerprint.textContent = fingerprint;
}

function currentStateConfig(): StateVideoConfig | undefined {
  return draft?.states[selectedState];
}

function currentClip(): VideoClipConfig | undefined {
  return currentStateConfig()?.clips.find((clip) => clip.file === selectedClipFile);
}

function resolvedPath(state: string, file: string): string | undefined {
  if (!bootstrap) return undefined;
  const index = bootstrap.config.states[state]?.clips.findIndex((clip) => clip.file === file) ?? -1;
  return index >= 0 ? bootstrap.resolvedPaths[state]?.[index] : undefined;
}

elements.save.addEventListener("click", () => void saveConfig());
elements.syncButton.addEventListener("click", async () => {
  if (!bootstrap) return;
  setSync("pending", "正在读取运行时配置与播放指纹…");
  try {
    showRuntimeSync(
      await invoke<RuntimeSync>("editor_runtime_sync", {
        expectedFingerprint: bootstrap.fingerprint,
        expectedVideoRoot: bootstrap.videoRoot,
      }),
    );
  } catch (error) {
    setSync("error", readableError(error));
  }
});
elements.selectionMode.addEventListener("change", () => {
  const config = currentStateConfig();
  if (!config) return;
  config.selection = elements.selectionMode.value as SelectionMode;
  markDirty();
  renderAssets();
});
elements.transitionPolicy.addEventListener("change", () => {
  const config = currentStateConfig();
  if (!config) return;
  config.transition_policy = elements.transitionPolicy.value as TransitionPolicy;
  markDirty();
  renderAssets();
});
elements.simulatePick.addEventListener("click", simulatePick);
elements.togglePlay.addEventListener("click", () => {
  if (elements.previewVideo.paused) void elements.previewVideo.play();
  else elements.previewVideo.pause();
});
elements.previewVideo.addEventListener("play", () => {
  elements.togglePlay.textContent = "Ⅱ";
});
elements.previewVideo.addEventListener("pause", () => {
  elements.togglePlay.textContent = "▶";
});
elements.previewVideo.addEventListener("loadedmetadata", () => {
  loadedDurationMs = Math.round(elements.previewVideo.duration * 1000);
  elements.durationTime.textContent = formatTime(loadedDurationMs);
  updateLoopBand();
  const loop = currentClip()?.loop;
  if (loop && loop.end_ms > loadedDurationMs) {
    elements.previewMessage.textContent = `Loop B 超过视频时长 ${loadedDurationMs}ms，保存前请修正。`;
    elements.previewMessage.dataset.level = "error";
  }
  updateDirtyUi();
});
elements.previewVideo.addEventListener("timeupdate", () => {
  const currentMs = Math.round(elements.previewVideo.currentTime * 1000);
  const duration = loadedDurationMs;
  const loop = currentClip()?.loop;
  if (loop && currentMs >= loop.end_ms && loop.end_ms <= duration) {
    elements.previewVideo.currentTime = loop.start_ms / 1000;
    if (!elements.previewVideo.paused) void elements.previewVideo.play();
  }
  elements.currentTime.textContent = formatTime(currentMs);
  if (duration > 0) elements.scrubber.value = String(Math.round((currentMs / duration) * 1000));
});
elements.previewVideo.addEventListener("error", () => {
  elements.previewMessage.textContent = "WebView 无法解码该视频；配置不会因此自动通过运行时验收。";
  elements.previewMessage.dataset.level = "error";
});
elements.scrubber.addEventListener("input", () => {
  if (loadedDurationMs <= 0) return;
  elements.previewVideo.currentTime = (Number(elements.scrubber.value) / 1000) * (loadedDurationMs / 1000);
});
elements.loopStart.addEventListener("change", updateLoopFromInputs);
elements.loopEnd.addEventListener("change", updateLoopFromInputs);
elements.setLoopA.addEventListener("click", () => {
  elements.loopStart.value = String(Math.round(elements.previewVideo.currentTime * 1000));
  if (elements.loopEnd.value === "") elements.loopEnd.value = String(Math.max(1, loadedDurationMs));
  updateLoopFromInputs();
});
elements.setLoopB.addEventListener("click", () => {
  elements.loopEnd.value = String(Math.round(elements.previewVideo.currentTime * 1000));
  if (elements.loopStart.value === "") elements.loopStart.value = "0";
  updateLoopFromInputs();
});
elements.clearLoop.addEventListener("click", () => {
  elements.loopStart.value = "";
  elements.loopEnd.value = "";
  updateLoopFromInputs();
});
window.addEventListener("keydown", (event) => {
  const target = event.target as HTMLElement | null;
  if (event.code === "Space" && !target?.matches("input, select, button")) {
    event.preventDefault();
    elements.togglePlay.click();
  }
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
    event.preventDefault();
    elements.save.click();
  }
});

function required(id: string): HTMLElement {
  const element = document.getElementById(id);
  if (!element) throw new Error(`missing #${id}`);
  return element;
}

function requiredButton(id: string): HTMLButtonElement {
  return required(id) as HTMLButtonElement;
}

function requiredInput(id: string): HTMLInputElement {
  return required(id) as HTMLInputElement;
}

function requiredSelect(id: string): HTMLSelectElement {
  return required(id) as HTMLSelectElement;
}

function requiredVideo(id: string): HTMLVideoElement {
  return required(id) as HTMLVideoElement;
}

function readableError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function fileLeaf(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}

function clampInteger(value: number, min: number, max: number): number {
  if (!Number.isFinite(value)) return min;
  return Math.min(max, Math.max(min, Math.round(value)));
}

function formatTime(milliseconds: number): string {
  const safe = Math.max(0, Math.round(milliseconds));
  const minutes = Math.floor(safe / 60_000);
  const seconds = Math.floor((safe % 60_000) / 1000);
  const millis = safe % 1000;
  return `${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}.${String(millis).padStart(3, "0")}`;
}
