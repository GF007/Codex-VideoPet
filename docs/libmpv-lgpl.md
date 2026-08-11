# LGPL libmpv runtime

The repository does not track `libmpv-2.dll`. Install the pinned, verified
runtime locally with:

```powershell
npm run libmpv:install
```

The installer verifies the release archive and extracted DLL against
`native/third_party/mpv/DEPENDENCY_LICENSES.json`. Packaging fails unless the
same DLL is present and `CodexPet.exe --validate-libmpv-license` confirms that
the runtime exposes client API 2.5+ and an explicit `-Dgpl=false` build.

The current runtime comes from the public `zhongfly/mpv-winbuild` workflow. Its
release archive has a GitHub artifact attestation, the mpv build disables GPL
mode, and the paired FFmpeg build reports LGPL version 3 or later without GPL
or nonfree configure switches. Exact source and build links are kept in the
machine-readable dependency manifest.

This is sufficient for local runtime and unsigned package QA, not a completed
legal review of every transitive library linked into the binary. The manifest
keeps `public_redistribution_eligible` false until those component notices and
license obligations are reviewed and packaged. Signed release mode enforces
that flag.

This dependency boundary does not grant rights to character media and does not
replace the signing and real-machine gates for a public Windows release.
