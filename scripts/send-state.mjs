const states = new Set([
  "idle",
  "running-right",
  "running-left",
  "waving",
  "jumping",
  "failed",
  "waiting",
  "running",
  "review",
]);

const state = process.argv[2] ?? "idle";
const ttl = process.argv[3] ? Number(process.argv[3]) : undefined;

if (!states.has(state)) {
  process.stderr.write(`Unknown state: ${state}\n`);
  process.exitCode = 2;
} else {
  try {
    const headers = { "content-type": "application/json" };
    if (process.env.CODEX_PET_TOKEN) {
      headers.authorization = `Bearer ${process.env.CODEX_PET_TOKEN}`;
    }
    const response = await fetch("http://127.0.0.1:48193/v1/state", {
      method: "POST",
      headers,
      body: JSON.stringify({ state, ttl_ms: ttl, source: "manual-cli" }),
      signal: AbortSignal.timeout(800),
    });
    const result = await response.json();
    process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
    if (!response.ok) process.exitCode = 1;
  } catch (error) {
    process.stderr.write(`Codex Pet is not reachable: ${String(error)}\n`);
    process.exitCode = 1;
  }
}
