# Third-party notices

This source preview does not track the local `libmpv-2.dll` binary or publish a
redistributable runtime package.

The development checkout currently targets a pinned community libmpv build.
Its recorded version, provider and SHA-256 values are documented in
[`native/third_party/mpv/THIRD_PARTY_NOTICES.md`](native/third_party/mpv/THIRD_PARTY_NOTICES.md).
That notice is provenance information, not proof that all redistribution
obligations have been completed.

Before publishing an executable, MSI, portable archive, or dependency bundle:

1. audit the exact libmpv, FFmpeg, codec and runtime build configuration;
2. preserve every applicable license and attribution;
3. satisfy corresponding-source or other distribution obligations;
4. generate and review a dependency inventory/SBOM; and
5. keep third-party signatures and publisher identities distinct from this project.

Rust, npm and build-time dependencies retain their respective upstream
licenses. Lockfiles are committed for reproducibility; release attribution is
still a separate acceptance gate.
