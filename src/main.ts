import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./style.css";

type PetState =
  | "idle"
  | "running-right"
  | "running-left"
  | "waving"
  | "jumping"
  | "failed"
  | "waiting"
  | "running"
  | "review";

interface StateSnapshot {
  state: PetState;
  previous_state: PetState;
  revision: number;
  source: string;
  updated_at_ms: number;
}

interface SessionActivity {
  id: string;
  state: PetState;
  previous_state: PetState;
  revision: number;
  title: string;
  activity: string;
  source: string;
  updated_at_ms: number;
  priority: number;
  unread: boolean;
  deep_link: string;
}

interface SessionsSnapshot {
  revision: number;
  selected_session_id: string | null;
  session_count: number;
  sessions: SessionActivity[];
}

interface Bootstrap {
  state: StateSnapshot;
  sessions: SessionsSnapshot;
  video_path: string;
  video_catalog: Record<PetState, string[]>;
  decoder_session_id: string;
  endpoint: string;
  click_through: boolean;
  always_on_top: boolean;
}

interface VisibilityPayload {
  visible: boolean;
}

function requiredElement<T extends Element>(selector: string): T {
  const element = document.querySelector<T>(selector);
  if (!element) throw new Error(`overlay DOM is incomplete: ${selector}`);
  return element;
}

const pet = requiredElement<HTMLElement>("#pet");
const video = requiredElement<HTMLVideoElement>("#pet-video");
const stateLabel = requiredElement<HTMLElement>("#state-label");
const stateDetail = requiredElement<HTMLElement>("#state-detail");
const sessionCount = requiredElement<HTMLElement>("#session-count");
const sessionEmpty = requiredElement<HTMLElement>("#session-empty");
const sessionList = requiredElement<HTMLUListElement>("#session-list");
const fatal = requiredElement<HTMLElement>("#fatal");

const appWindow = getCurrentWindow();
let mediaStartedReported = false;
let windowVisible = true;
let latestRevision = -1;
let latestSessionsRevision = -1;
let latestState: PetState | null = null;
let activeVideoState: PetState | null = null;
let currentVideoPath: string | null = null;
let videoCatalogReady = false;
let mediaGeneration = 0;
let videoCatalog = {} as Record<PetState, string[]>;
const lastVideoByState: Partial<Record<PetState, string>> = {};

const petStates: PetState[] = [
  "idle",
  "running-right",
  "running-left",
  "waving",
  "jumping",
  "failed",
  "waiting",
  "running",
  "review",
];

const stateDetails: Record<PetState, string> = {
  idle: "等待 Codex Session",
  "running-right": "Subagent 正在工作",
  "running-left": "Subagent 正在工作",
  waving: "收到新的任务",
  jumping: "任务取得进展",
  failed: "任务遇到阻塞",
  waiting: "需要你的输入或授权",
  running: "Codex 正在工作",
  review: "任务已完成，等待查看",
};

const activityLabels: Record<string, string> = {
  starting: "Session 已连接",
  working: "正在处理任务",
  command: "正在运行命令",
  reading: "正在读取文件",
  editing: "正在编辑文件",
  searching: "正在搜索信息",
  waiting: "等待输入或授权",
  recovering: "遇到错误，正在处理",
  ready: "已完成，点击查看",
  subagent: "Subagent 正在工作",
  idle: "当前空闲",
};

function applyState(snapshot: StateSnapshot): void {
  if (snapshot.revision < latestRevision) return;
  latestRevision = snapshot.revision;
  latestState = snapshot.state;
  pet.dataset.state = snapshot.state;
  stateLabel.textContent = snapshot.state.toUpperCase();
  stateDetail.textContent = stateDetails[snapshot.state];
  document.title = `Codex Pet · ${snapshot.state}`;
  void ensureStateVideo(snapshot.state);
}

function randomIndex(length: number): number {
  const value = new Uint32Array(1);
  crypto.getRandomValues(value);
  return value[0] % length;
}

function selectStateVideo(state: PetState): string | null {
  const candidates = videoCatalog[state] ?? [];
  if (candidates.length === 0) return null;
  const previous = lastVideoByState[state];
  const choices = candidates.length > 1 ? candidates.filter((path) => path !== previous) : candidates;
  return choices[randomIndex(choices.length)] ?? null;
}

function videoFilename(path: string): string {
  return path.split(/[\\/]/).pop() ?? "video";
}

async function ensureStateVideo(state: PetState, force = false): Promise<void> {
  if (!videoCatalogReady) return;
  if (!force && activeVideoState === state && currentVideoPath) return;

  const selected = selectStateVideo(state);
  if (!selected) {
    fatal.hidden = false;
    fatal.textContent = `状态 ${state} 没有可用视频`;
    await reportMedia("error", `missing video for ${state}`);
    return;
  }

  activeVideoState = state;
  currentVideoPath = selected;
  lastVideoByState[state] = selected;
  const generation = ++mediaGeneration;
  video.pause();
  video.src = convertFileSrc(selected);
  video.load();
  fatal.hidden = true;
  fatal.textContent = "";
  void reportMedia("source-switched", videoFilename(selected));
  await syncPlayback(generation);
}

function activityLabel(value: string): string {
  return activityLabels[value] ?? "状态已更新";
}

async function openSession(sessionId: string): Promise<void> {
  try {
    await invoke("open_codex_session", { sessionId });
  } catch {
    stateDetail.textContent = "无法打开这个 Codex Session";
  }
}

function renderSessions(snapshot: SessionsSnapshot): void {
  if (snapshot.revision < latestSessionsRevision) return;
  latestSessionsRevision = snapshot.revision;
  sessionCount.textContent = String(snapshot.session_count);
  sessionEmpty.hidden = snapshot.sessions.length > 0;
  sessionList.replaceChildren();

  for (const session of snapshot.sessions) {
    const item = document.createElement("li");
    const button = document.createElement("button");
    const row = document.createElement("span");
    const title = document.createElement("span");
    const state = document.createElement("span");
    const activity = document.createElement("span");

    button.type = "button";
    button.className = "session-card";
    button.dataset.taskCard = "";
    button.dataset.sessionId = session.id;
    button.setAttribute("aria-current", String(snapshot.selected_session_id === session.id));
    button.setAttribute("aria-label", `${session.title}，${session.state}，点击打开`);
    button.addEventListener("pointerdown", (event) => event.stopPropagation());
    button.addEventListener("click", () => void openSession(session.id));

    row.className = "session-card-row";
    title.className = "session-title";
    title.textContent = session.title;
    state.className = "session-state";
    state.dataset.state = session.state;
    state.textContent = session.state;
    activity.className = "session-activity";
    activity.textContent = activityLabel(session.activity);

    row.append(title, state);
    button.append(row, activity);
    item.append(button);
    sessionList.append(item);
  }

  const selected = snapshot.sessions.find(
    (session) => session.id === snapshot.selected_session_id,
  );
  if (selected) {
    stateDetail.textContent = `${selected.title} · ${activityLabel(selected.activity)}`;
  }
}

async function reportMedia(event: string, detail?: string): Promise<void> {
  try {
    await invoke("report_media_event", { event, detail: detail ?? null });
  } catch {
    // Diagnostics must never disrupt playback.
  }
}

async function syncPlayback(expectedGeneration = mediaGeneration): Promise<void> {
  if (!currentVideoPath) return;
  if (!windowVisible || document.hidden) {
    video.pause();
    if (expectedGeneration === mediaGeneration) await reportMedia("paused");
    return;
  }

  try {
    await video.play();
    if (expectedGeneration === mediaGeneration) await reportMedia("playing");
  } catch (error) {
    if (expectedGeneration === mediaGeneration) {
      await reportMedia("play-rejected", String(error).slice(0, 180));
    }
  }
}

async function bootstrap(): Promise<void> {
  // Subscribe before reading the snapshot, then reject out-of-order revisions.
  // This closes both the bootstrap gap and concurrent HTTP emit reordering.
  await listen<StateSnapshot>("pet-state", ({ payload }) => applyState(payload));
  await listen<SessionsSnapshot>("pet-sessions", ({ payload }) => renderSessions(payload));
  const data = await invoke<Bootstrap>("get_bootstrap");
  videoCatalog = Object.fromEntries(
    petStates.map((state) => [state, Array.isArray(data.video_catalog[state]) ? data.video_catalog[state] : []]),
  ) as Record<PetState, string[]>;
  applyState(data.state);
  renderSessions(data.sessions);
  videoCatalogReady = true;
  await ensureStateVideo(latestState ?? data.state.state, true);

  await listen<VisibilityPayload>("pet-visibility", async ({ payload }) => {
    windowVisible = payload.visible;
    await syncPlayback();
  });
}

video.addEventListener("loadedmetadata", async () => {
  fatal.hidden = true;
  fatal.textContent = "";
  if (!mediaStartedReported) {
    mediaStartedReported = true;
    await reportMedia("decoder-started");
  }
  await reportMedia("media-ready", currentVideoPath ? videoFilename(currentVideoPath) : undefined);
});

video.addEventListener("error", async () => {
  const code = video.error?.code ?? 0;
  fatal.hidden = false;
  fatal.textContent = `视频加载失败（MediaError ${code}）`;
  await reportMedia("error", `MediaError ${code}`);
});

document.addEventListener("visibilitychange", () => void syncPlayback());
appWindow.onResized(async () => {
  windowVisible = !(await appWindow.isMinimized());
  await syncPlayback();
});

pet.addEventListener("pointerdown", async (event) => {
  const target = event.target instanceof Element ? event.target : null;
  if (event.button === 0 && !target?.closest("[data-task-card]")) {
    await appWindow.startDragging();
  }
});

bootstrap().catch(async (error: unknown) => {
  fatal.hidden = false;
  fatal.textContent = `启动失败：${String(error).slice(0, 240)}`;
  await reportMedia("bootstrap-error", String(error).slice(0, 180));
});
