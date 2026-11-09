# Changelog

All notable changes to this project are documented here.

## [Unreleased]

### Added

- Rust library and CLI for local, in-process recorded Go build-identity inventory
  using pinned unstrip 1.2.0 with the local strict-buildinfo-v1 patch.
- Deterministic schema-version-1 JSON preserving module versions, replacements,
  build settings and embedded provenance.
- Bounded 1 GiB regular-file input handling and SHA-256 of inspected bytes.
- Explicit partial/unavailable coverage and exit semantics; function and inline
  recovery remain unattempted.
- Controlled stripped/unstripped local Go fixtures and module/CLI safety tests.
- Vendored parser provenance, licenses, self-contained checked build-info tests,
  and strict vendor formatting/library-test/Clippy gate.

### Fixed

- Checked build-header, varint and string bounds instead of unwinding on short
  or overflowing build-info input.
- Missing/malformed module metadata now yields explicit partial coverage,
  preserving raw Go version and validated evidence rather than reporting a
  decoded empty module inventory.
- Earlier non-header magic literals no longer mask intact later build-info
  headers during checked discovery.
