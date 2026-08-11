# Media rights ledger

No character video or generated-media payload is included in the initial
source-only Git history. Files under `VideoSource/V1` are ignored except for the
runtime configuration and palette contract.

Media must not be added to this repository or a public release until one ledger
entry per file records:

- stable pack ID and relative path;
- SHA-256 and byte length;
- creator/rightsholder and source;
- source-image or source-video authorization;
- generation provider/model and applicable terms date, when AI-assisted;
- permission for redistribution, modification and commercial use;
- required attribution or usage restrictions; and
- reviewer, review date and takedown contact.

Technical validation, a file hash, or possession of the media is not evidence
of redistribution rights. Unresolved entries remain private and release-blocking.
