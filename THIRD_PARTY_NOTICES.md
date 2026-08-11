# Third-party notices

The project source is MIT licensed. Third-party components keep their own
licenses.

The runtime loads `libmpv-2.dll` dynamically. The binary is not stored in Git;
`npm run libmpv:install` downloads the pinned LGPL build and verifies its
archive and DLL hashes. Exact mpv and FFmpeg revisions, build configuration,
source links, provenance, and license files are recorded in
[`native/third_party/mpv/DEPENDENCY_LICENSES.json`](native/third_party/mpv/DEPENDENCY_LICENSES.json).
The packaged runtime is distributed under LGPL-3.0-or-later and remains
replaceable by a compatible modified build.

The exact runtime is currently approved for local use and unsigned QA only.
Public binary redistribution stays blocked until the transitive linked-library
notice/license review recorded in the dependency manifest is complete.

Character media is outside the MIT grant. See
[`MEDIA_LICENSES.md`](MEDIA_LICENSES.md); unreviewed media must not be committed
or published.
