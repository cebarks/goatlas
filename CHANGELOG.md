# Changelog

All notable changes to this project are documented here.

## [Unreleased]

### Added

- Rust library and CLI for local, in-process recorded Go build-identity inventory
  using pinned unstrip 1.2.0.
- Deterministic schema-version-1 JSON preserving module versions, replacements,
  build settings and embedded provenance.
- Bounded 1 GiB regular-file input handling and SHA-256 of inspected bytes.
- Explicit partial/unavailable coverage and exit semantics; function and inline
  recovery remain unattempted.
- Controlled stripped/unstripped local Go fixtures and module/CLI safety tests.
