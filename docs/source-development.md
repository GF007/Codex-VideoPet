# Source development

The public repository does not track `libmpv-2.dll`, character videos,
installers, QA evidence, or machine-local authoring output.

## Verify source without media

```powershell
npm ci
npm run source:check
```

This checks the pinned libmpv manifest, frontend builds, media-independent Rust
tests, Hook privacy mapping, and the native release build. It does not prove
video playback, real Codex Hook trust, installer lifecycle, signing, or media
rights.

## Run locally

```powershell
npm ci
npm run libmpv:install
$env:CODEX_PET_VIDEO_DIR = 'C:\absolute\path\to\authorized-video-pack'
npm run native:build
.\native\target\release\codex-pet-native.exe --setup
```

The media root must contain a valid `video-config.json`, `ui-palette.json`, and
all nine state resources. The setup wizard merges the 11 owned Hooks, but the
user must still trust every item in Codex `/hooks`.

## Validation tiers

| Tier | Command | Extra inputs | Meaning |
|---|---|---|---|
| Source | `npm run source:check` | None | Compile and source contracts |
| Runtime | `npm run dev` | libmpv + media | Local window/player behavior |
| Package QA | `npm run native:msi` | libmpv + approved media | Unsigned internal installer |
| Release | Signed runbook | Rights + certificate + real-machine evidence | Public artifact gate |

Dependency details are in [`libmpv-lgpl.md`](libmpv-lgpl.md). Media and release
requirements are in [`../MEDIA_LICENSES.md`](../MEDIA_LICENSES.md) and
[`windows-release.md`](windows-release.md).
