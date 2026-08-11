# Source development without bundled media

The public repository is intentionally source-only. It does not track the local
`libmpv-2.dll`, character videos, MSI/ZIP outputs, performance evidence, or
machine-local authoring runs.

## Source-only verification

```powershell
npm ci
npm run source:check
```

`source:check` runs both frontend builds, 87 native tests that do not require
the checked-in media payload, the native Hook privacy/mapping self-test, and a
Rust release build. Two media-contract tests are skipped because they validate
the private development pack referenced by `VideoSource/V1/video-config.json`.

This proves compilation and unit-level behavior only. It does not validate
libmpv loading, hardware decode, animation continuity, real Codex Hook trust,
deep links, MSI lifecycle, code signing, or media rights.

## Running the overlay locally

A runnable development setup additionally needs:

1. a compatible `libmpv-2.dll`, provided through `CODEX_PET_MPV_DLL`, beside
   the executable, or in `native/third_party/mpv/`;
2. a complete nine-state media root provided through the absolute
   `CODEX_PET_VIDEO_DIR` path or one of the documented runtime fallback roots;
3. a valid `video-config.json` and `ui-palette.json`; and
4. user-authorized Codex Hooks for real task-state validation.

Local dependency/media files remain untracked. Do not publish or redistribute
them until the gates in [`THIRD_PARTY_NOTICES.md`](../THIRD_PARTY_NOTICES.md),
[`MEDIA_LICENSES.md`](../MEDIA_LICENSES.md), and
[`windows-release.md`](windows-release.md) are satisfied.

## Test tiers

| Tier | Command | Requires media/DLL | Meaning |
|---|---|---:|---|
| Source | `npm run source:check` | No | Compile and unit/source contracts |
| Media config | `npm run native:test` | Media files | Checked-in config/palette references |
| Local runtime | `npm run dev` | Yes | Real window/player behavior |
| Package QA | `npm run native:msi` | Yes | Unsigned internal installer only |
| Release | Signed runbook | Yes | Legal, signature and real-machine gates |
