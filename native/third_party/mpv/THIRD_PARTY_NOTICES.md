# Third-party notices

## libmpv

- Component: mpv / libmpv
- Runtime file: `libmpv-2.dll`
- Version reported by the library: `mpv v0.41.0-39-ga58dd8ac4`
- Upstream commit: <https://github.com/mpv-player/mpv/commit/a58dd8ac4>
- Binary provider: <https://sourceforge.net/projects/mpv-player-windows/files/libmpv/>
- Downloaded archive: `mpv-dev-x86_64-20251228-git-a58dd8a.7z`
- Archive SHA-256: `DB71A7236D04B1D039B73D75988C72AD3B349CFA97F2C9BBBEC100A4C499A600`
- DLL SHA-256: `67176A5F00ECC53645B0D3BC6607046FB4225A76D6227A0ED3E7368DDA08EA26`

The bundled build does not declare `-Dgpl=false` in `mpv-configuration`, so it is treated as a GPL build. `LICENSE.GPL-2.0.txt` accompanies internal QA packages. mpv and the codec/runtime libraries linked into this community build retain their respective upstream copyrights and licenses.

This pinned binary is suitable for local development and acceptance testing. Before public or commercial redistribution, complete a dependency-level license audit and satisfy the corresponding-source obligations for the exact binary, or replace it with an audited build produced with the intended feature/license configuration.
