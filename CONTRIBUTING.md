# Contributing

Thanks for helping improve Codex VideoPet. This repository currently accepts
source-code and documentation contributions for the Windows native overlay.

## Development environment

- Windows 10 or 11
- Rust stable with the MSVC target
- Node.js 22 and npm
- .NET 8 and WiX only when working on the MSI path

Start with the source-only checks. They do not require private media; the
libmpv manifest is always checked and a local DLL is verified when present:

```powershell
npm ci
npm run source:check
```

See [`docs/source-development.md`](docs/source-development.md) before running
the asset-gated player, palette, MSI or performance workflows.

## Pull requests

- Keep `native/` as the only production runtime. Do not silently restore the
  legacy Tauri/WebView2 application as a release entrypoint.
- Preserve the minimal Hook envelope: no prompt, transcript, working directory,
  tool input or tool output.
- Do not infer active Codex tasks from `thread/list`; Hooks remain the realtime
  activity source and metadata is enrichment only.
- Keep V2 phase assets outside `VideoSource/V1` until the phase controller and
  motion manifest are active.
- Do not commit generated outputs, local Session/Hook state, credentials,
  screenshots containing user data, DLLs, installers or unreviewed media.
- Update tests and nearby documentation when behavior changes.

Before requesting review, run:

```powershell
npm run source:check
git diff --check
```

Asset or release changes additionally require a completed rights/dependency
review and the real Windows/Codex acceptance evidence described in
[`docs/windows-release.md`](docs/windows-release.md).

## Security and private data

Do not put secrets or sensitive diagnostics in an issue or pull request. Follow
[`SECURITY.md`](SECURITY.md) for private reporting. Contributions are submitted
under the repository's [MIT License](LICENSE) unless a different written
agreement is established.
