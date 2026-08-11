import { randomBytes } from "node:crypto";
import { mkdir, open, rename, unlink } from "node:fs/promises";
import { join } from "node:path";

const endpoint = "http://127.0.0.1:48193/v1/codex-event";
const maxInputBytes = 512 * 1024;

function shallowFailure(input) {
  const response = input?.tool_response;
  return (
    input?.is_error === true ||
    response?.is_error === true ||
    response?.ok === false ||
    (Number.isFinite(response?.exit_code) && response.exit_code !== 0)
  );
}

function activityForTool(toolName) {
  const tool = String(toolName ?? "").toLowerCase();
  if (tool === "bash" || tool.includes("exec") || tool.includes("shell")) return "command";
  if (tool.includes("read") || tool.includes("view")) return "reading";
  if (tool.includes("write") || tool.includes("edit") || tool.includes("patch")) return "editing";
  if (tool.includes("search") || tool.includes("find") || tool.includes("web")) return "searching";
  if (tool === "agent" || tool.includes("spawn_agent")) return "subagent";
  return "working";
}

export function mapHook(input) {
  const event = input?.hook_event_name ?? input?.event_name ?? input?.event;
  switch (event) {
    case "SessionStart":
      return { state: "idle", activity: "starting" };
    case "SessionEnd":
      return { state: "idle", activity: "idle" };
    case "UserPromptSubmit":
      return { state: "running", activity: "working" };
    case "PreToolUse":
      return { state: "running", activity: activityForTool(input?.tool_name) };
    case "PermissionRequest":
      return { state: "waiting", activity: "waiting" };
    case "PostToolUse":
      return shallowFailure(input)
        ? { state: "running", activity: "recovering" }
        : { state: "running", activity: "working" };
    case "PreCompact":
    case "PostCompact":
      return { state: "running", activity: "working" };
    case "SubagentStart":
      return { state: "running", activity: "subagent" };
    case "SubagentStop":
      return { state: "running", activity: "working" };
    case "Stop":
      return { state: "review", activity: "ready" };
    default:
      return null;
  }
}

function isSessionId(value) {
  return /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(
    String(value ?? ""),
  );
}

function safeTurnId(value) {
  const turnId = String(value ?? "");
  return /^[A-Za-z0-9_-]{1,80}$/.test(turnId) ? turnId : undefined;
}

function eventId() {
  return randomBytes(32).toString("hex");
}

function pendingEventsDirectory() {
  const localAppData = process.env.LOCALAPPDATA;
  return localAppData
    ? join(localAppData, "com.local.codex-pet-overlay", "session-data", "events")
    : null;
}

async function readStdin() {
  const chunks = [];
  let size = 0;
  for await (const chunk of process.stdin) {
    size += chunk.length;
    if (size > maxInputBytes) return null;
    chunks.push(chunk);
  }
  try {
    return JSON.parse(Buffer.concat(chunks).toString("utf8"));
  } catch {
    return null;
  }
}

export function buildPayload(mapping, input) {
  const sessionId = String(input?.session_id ?? "");
  if (!isSessionId(sessionId)) return null;
  const event = String(input?.hook_event_name ?? input?.event_name ?? input?.event ?? "");
  return {
    schema_version: 1,
    event_id: eventId(),
    session_id: sessionId,
    turn_id: safeTurnId(input?.turn_id),
    hook_event_name: event,
    ...mapping,
    emitted_at_ms: Date.now(),
  };
}

async function persistPendingEvent(payload) {
  const directory = pendingEventsDirectory();
  if (!directory) return null;
  await mkdir(directory, { recursive: true });
  const finalPath = join(directory, `${payload.event_id}.json`);
  const temporaryPath = join(directory, `${payload.event_id}.${process.pid}.tmp`);
  const file = await open(temporaryPath, "wx", 0o600);
  try {
    await file.writeFile(JSON.stringify(payload), "utf8");
    await file.sync();
  } finally {
    await file.close();
  }
  await rename(temporaryPath, finalPath);
  return finalPath;
}

async function notify(mapping, input) {
  const payload = buildPayload(mapping, input);
  if (!payload) return;
  let pendingPath = null;
  try {
    pendingPath = await persistPendingEvent(payload);
  } catch {
    // Keep live delivery best-effort even if the local recovery queue is unavailable.
  }
  const headers = { "content-type": "application/json" };
  if (process.env.CODEX_PET_TOKEN) headers.authorization = `Bearer ${process.env.CODEX_PET_TOKEN}`;
  const response = await fetch(endpoint, {
    method: "POST",
    headers,
    // Never forward the Codex hook body: it may contain prompts, tool data,
    // working paths, transcript paths, or assistant output.
    body: JSON.stringify(payload),
    signal: AbortSignal.timeout(250),
  });
  if (response.ok && pendingPath) {
    try {
      await unlink(pendingPath);
    } catch {
      // The native snapshot already acknowledged this event; replay remains idempotent.
    }
  }
}

async function selfTest() {
  const cases = [
    ["SessionStart", "idle"],
    ["SessionEnd", "idle"],
    ["UserPromptSubmit", "running"],
    ["PreToolUse", "running"],
    ["PermissionRequest", "waiting"],
    ["PostToolUse", "running"],
    ["PreCompact", "running"],
    ["PostCompact", "running"],
    ["SubagentStart", "running"],
    ["SubagentStop", "running"],
    ["Stop", "review"],
  ];
  for (const [event, expected] of cases) {
    const actual = mapHook({ hook_event_name: event, agent_id: "test" })?.state;
    if (actual !== expected) {
      throw new Error(`${event}: expected ${expected}, received ${actual}`);
    }
  }
  if (mapHook({ hook_event_name: "PostToolUse", tool_response: { exit_code: 1 } })?.activity !== "recovering") {
    throw new Error("failed tool activity mapping is incorrect");
  }
  const payload = buildPayload(mapHook({ hook_event_name: "PreToolUse" }), {
    hook_event_name: "PreToolUse",
    session_id: "019fcc02-569f-7790-92ff-209aaca4275c",
    turn_id: "turn-1",
    prompt: "must-not-persist",
    transcript_path: "must-not-persist",
    cwd: "must-not-persist",
    tool_name: "must-not-persist",
    tool_response: { output: "must-not-persist" },
  });
  const serialized = JSON.stringify(payload);
  for (const sensitive of ["prompt", "transcript", "cwd", "tool_name", "tool_response", "must-not-persist"]) {
    if (serialized.includes(sensitive)) throw new Error(`sensitive field leaked: ${sensitive}`);
  }
}

const selfTestMode = process.argv.includes("--self-test");
try {
  if (selfTestMode) {
    await selfTest();
  } else {
    const input = await readStdin();
    const mapping = input && mapHook(input);
    if (mapping) await notify(mapping, input);
  }
} catch (error) {
  if (selfTestMode) {
    process.stderr.write(`Hook self-test failed: ${String(error)}\n`);
    process.exitCode = 1;
  }
  // Production hooks are observational only. Never print, block, or steer Codex.
}

if (!selfTestMode) process.exitCode = 0;
