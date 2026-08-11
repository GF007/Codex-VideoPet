# Private authoring boundary

The local checkout may contain machine-specific media generation and QA scripts
in this directory. They are intentionally excluded from the public source
history because they can reference local files, provider configuration,
generated prompts, private provenance and media whose redistribution rights
have not been approved.

Only this boundary document is tracked. A future public authoring tool must use
explicit CLI arguments/environment variables, emit no credentials or absolute
user paths, and produce a reviewed rights/provenance manifest before its output
is eligible for a release.
