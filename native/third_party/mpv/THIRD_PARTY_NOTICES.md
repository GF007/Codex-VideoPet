# libmpv runtime notice

Codex Pet uses a dynamically loaded `libmpv-2.dll`. The public source tree does
not commit that binary; `scripts/install-libmpv-lgpl.ps1` downloads one pinned
artifact and verifies it before local use or packaging.

The accepted runtime is built from mpv with `-Dgpl=false` and from FFmpeg
without `--enable-gpl`, `--enable-nonfree`, x264, x265, or Rubber Band. The
runtime reports mpv `v0.41.0-922-gf4d13e1c2`, FFmpeg
`N-126039-g6bbc22dc0`, and FFmpeg license `LGPL version 3 or later`.

Exact source commits, build workflow, artifact URL, SHA-256 values, accepted
configuration, and license files are recorded in `DEPENDENCY_LICENSES.json`.
mpv, FFmpeg, and their linked components retain their upstream copyrights and
licenses. The application remains dynamically linked so the runtime DLL can be
replaced with a compatible modified build.
