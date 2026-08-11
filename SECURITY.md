# Security policy

## Supported version

Only the latest commit on `main` is currently supported. There is no supported
public binary release yet.

## Reporting a vulnerability

Use GitHub's private vulnerability reporting / Security Advisory flow for this
repository. Do not open a public issue containing a credential, private Hook
configuration, prompt, transcript, working directory, tool input/output,
Session snapshot, task ID, or other user data.

Include the affected commit or version, Windows version, expected/actual
behavior and a minimal redacted reproduction. No response-time SLA is promised
during the public-preview stage.

## Local trust boundary

Codex VideoPet binds its HTTP diagnostics and event interface to loopback. A
Bearer token is enforced only when `CODEX_PET_TOKEN` is configured consistently
for both the application and Hook entrypoint. Other processes running as the
same Windows user may otherwise read diagnostics or submit local state/events.

The Hook contract intentionally excludes prompt, transcript, working directory
and tool input/output. Persistent state under
`%LOCALAPPDATA%\com.local.codex-pet-overlay` should still be treated as private
diagnostic data.
