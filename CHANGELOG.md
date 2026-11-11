# Changelog

All notable changes to this project are documented here.

## [Unreleased]

### Added

- GitHub Actions CI running the full `just check` gate (fixtures, tests,
  vendored-dependency patch checks, formatting, strict Clippy).
- AGPL-3.0-only project license with vendored unstrip licensing clarified.
- Rust library and CLI for local, in-process recorded Go build-identity inventory
  using pinned unstrip 1.2.0 with the local strict-buildinfo-v1 patch.
- Deterministic schema-version-1 JSON preserving module versions, replacements,
  build settings and embedded provenance.
- Bounded 1 GiB regular-file input handling and SHA-256 of inspected bytes.
- Explicit partial/unavailable coverage and exit semantics; exit 0 now requires
  all three core walks (build identity, functions, inlining) to be decoded.
- Inline occurrences recovered from `FUNCDATA_InlTree` metadata through the
  moduledata gofunc base, with checked decoder outcomes: typed `inline_`
  issues, count-establishing pcdata scans, retained validated prefixes,
  observable entry caps, genuine-empty-tree distinction from truncated or
  unlocatable trees, and unresolved callee names kept as indexed occurrences.
  Layout is gated to the go1.20+ magic and the documented 16-byte
  `runtime.inlinedCall` families 1.20-1.26 plus source-inspected 1.27.
- Stripped PIE fixtures and function-name, body-span, and layout regressions.
- Noinline fixture variant (package-local inlining disabled) proving genuinely
  empty trees are inspected as complete, plus inline decoder regressions and
  whole-scanner inline tests.
- Application-side version-gated gofunc locator for the go1.27 moduledata field
  moves (the vendored locator predates them); documented for replacement when
  upstream handles go1.27.
- Controlled stripped/unstripped local Go fixtures and module/CLI safety tests.
- Retained standalone Go implementations from runtime function tables, including
  reflection-only methods, with checked nonzero file-backed text VA spans,
  optional file/line metadata, and conservative cached module-prefix attribution.
  Retained code is not runtime reachability; null attribution does not erase
  implementation evidence. Malformed/skipped rows produce incomplete coverage.
- Vendored inline decoder now reports truncated or unlocatable inline trees as
  explicit errors instead of clean empty successes (strict-inline-v1 patch).
- Vendored pclntab now resolves a zero header textStart from the statically
  present moduledata text base instead of the first executable section, so
  externally linked PIE builds report correct entry and container virtual
  addresses (strict-textstart-v1 patch); external-PIE fixture regression
  included.
- Vendored parser provenance, licenses, self-contained checked build-info and
  inline tests, and strict vendor formatting/library-test/Clippy gate.

### Fixed

- Checked build-header, varint and string bounds instead of unwinding on short
  or overflowing build-info input.
- Missing/malformed module metadata now yields explicit partial coverage,
  preserving raw Go version and validated evidence rather than reporting a
  decoded empty module inventory.
- Earlier non-header magic literals no longer mask intact later build-info
  headers during checked discovery.
