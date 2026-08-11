# V1 media contract

`video-config.json` and `ui-palette.json` document the current nine-state V1
runtime contract. The referenced video files are intentionally excluded from
the source repository pending media-rights review and resource-pack versioning.

For local playback, provide a complete approved media directory and set the
absolute `CODEX_PET_VIDEO_DIR` environment variable before starting the app.
The runtime still validates that every enabled clip exists inside that root.

Do not place V2 `enter`, `loop`, `exit`, `once` or direct-transition assets in
this directory until a phase-aware loader and `motion-manifest.json` are active.
